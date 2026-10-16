import json
import pathlib
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import MagicMock, patch

from statevec_local import Deployment, checked_artifacts, initialize, read, record_artifacts, run_foreground
from statevec_local import cluster
from statevec_local.__main__ import main


class Files(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name)
        self.file = self.directory / "artifact"
        self.file.write_bytes(b"local build")
        self.manifest = self.directory / "artifacts.json"
        self.files = {name: self.file for name in ("node", "admin", "core")}
        record_artifacts(self.manifest, self.files, "1"*40, "2"*40)

    def test_digest_checks_actual_bytes_and_pair(self):
        self.assertEqual(checked_artifacts(self.manifest, sdk_revision="2"*40)["host_revision"], "1"*40)
        with self.assertRaisesRegex(ValueError, "SDK revision"):
            checked_artifacts(self.manifest, sdk_revision="3"*40)
        self.file.write_bytes(b"different build")
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            checked_artifacts(self.manifest)

    def test_manifest_does_not_overwrite(self):
        before = self.manifest.read_bytes()
        with self.assertRaises(FileExistsError):
            record_artifacts(self.manifest, self.files, "1"*40, "2"*40)
        self.assertEqual(self.manifest.read_bytes(), before)

    def test_json_objects_reject_duplicates_arrays_and_oversize(self):
        for value in ('{"x":1,"x":2}', '[]', '{"x":"' + "x"*1_048_576 + '"}'):
            with self.subTest(length=len(value)):
                self.file.write_text(value)
                with self.assertRaises(ValueError):
                    read(self.file)

    def test_init_keeps_application_binding_and_arbitrary_peer_ids(self):
        root = self.directory / "cluster"
        initialize(root, self.manifest, {"cluster_name": "bank", "initial_voters": [9],
            "learner_only_peers": [20], "cluster_mode": "cmd_only"}, 24100,
            {"artifact": "core", "config_path": str(self.directory / "business.json"), "target": "test-target"})
        first, second = read(root / "node-9.json"), read(root / "node-20.json")
        self.assertEqual(first["ingress_addr"], "127.0.0.1:24100")
        self.assertEqual(second["ingress_addr"], "127.0.0.1:24103")
        self.assertEqual(first["initial_voters"], [9])
        self.assertEqual(first["core_plugin"]["config_path"], str(self.directory / "business.json"))
        self.assertEqual(first["core_plugin"]["sha256"], checked_artifacts(self.manifest)["files"]["core"]["sha256"])
        self.assertIsNone(second["projector_plugin"])
        with self.assertRaises(FileExistsError):
            initialize(root, self.manifest, {"initial_voters": [9]}, 24100,
                {"artifact": "core", "config_path": str(self.file)})
        self.assertEqual(read(root / "node-9.json"), first)

    def test_bad_init_refuses_before_creating_root(self):
        root = self.directory / "cluster"
        for voters, learners, port in (([1], [1], 24100), ([0], [], 24100), ([True], [], 24100),
                                       ([1], [], 65534), ([], [4], 24100)):
            with self.subTest(voters=voters, learners=learners, port=port):
                with self.assertRaises(ValueError):
                    initialize(root, self.manifest, {"initial_voters": voters, "learner_only_peers": learners},
                               port, {"artifact": "core", "config_path": str(self.file)})
                self.assertFalse(root.exists())

    def test_cli_duplicate_artifact_label_refuses_without_output(self):
        output = self.directory / "new.json"
        with patch("sys.argv", ["statevec-local", "artifacts", str(output), "--node", str(self.file),
            "--admin", str(self.file), "--host-revision", "1"*40, "--sdk-revision", "2"*40,
            "--file", "node=other"]), self.assertRaises(SystemExit):
            main()
        self.assertFalse(output.exists())


class Membership(unittest.TestCase):
    def deployment(self, admin):
        instance = object.__new__(Deployment)
        instance.voters = [1, 2, 3]
        instance.check = lambda: None
        instance.admin = admin
        return instance

    def test_permanent_refusal_stops_without_reproposing(self):
        calls = []
        def admin(peer, command, *args):
            calls.append(command)
            if command == "query-membership":
                return 0, {"category": "applied_membership", "role": "leader", "learners": []}
            return 4, {"category": "owner_refusal", "admission": "not_admitted",
                       "reason": {"kind": "membership_invalid_peer", "peer": "4"}}
        with self.assertRaisesRegex(RuntimeError, "membership_invalid_peer"):
            self.deployment(admin).admit_learner(4)
        self.assertEqual(calls, ["query-membership", "add-learner"])

    def test_accepted_and_unknown_are_observed_not_reproposed(self):
        for reply in ((0, {"category": "proposal_accepted"}),
                      (5, {"category": "outcome_unknown", "admission": "unknown"}),
                      (3, {"category": "io_failure", "admission": "unknown"})):
            with self.subTest(reply=reply):
                calls = []
                def admin(peer, command, *args):
                    calls.append(command)
                    if command == "add-learner":
                        return reply
                    return 0, {"category": "applied_membership", "role": "leader",
                               "learners": ["4"] if len(calls) > 8 else []}
                with patch.object(cluster.time, "sleep"):
                    self.deployment(admin).admit_learner(4)
                self.assertEqual(calls.count("add-learner"), 1)
                self.assertGreater(calls.count("query-membership"), 3)

    def test_missing_write_reply_is_not_retried(self):
        calls = []
        def admin(peer, command, *args):
            calls.append(command)
            if command == "add-learner":
                raise json.JSONDecodeError("truncated output", "", 0)
            return 0, {"category": "applied_membership", "role": "leader", "learners": []}
        with self.assertRaises(json.JSONDecodeError):
            self.deployment(admin).admit_learner(4)
        self.assertEqual(calls.count("add-learner"), 1)

    def test_existing_membership_never_proposes(self):
        admin = MagicMock(return_value=(0, {"category": "applied_membership", "role": "follower", "learners": ["20"]}))
        self.deployment(admin).admit_learner(20)
        admin.assert_called_once_with(1, "query-membership")


class Processes(unittest.TestCase):
    def test_partial_start_failure_retires_and_preserves_original_error(self):
        deployment = MagicMock()
        deployment.start.side_effect = FileNotFoundError("second executable")
        with self.assertRaisesRegex(FileNotFoundError, "second executable"):
            run_foreground(deployment, "fresh")
        deployment.stop.assert_called_once()

    def test_hangup_uses_cleanup_and_restores_handlers(self):
        deployment = MagicMock()
        deployment.start.side_effect = KeyboardInterrupt
        before = {kind: signal.getsignal(kind) for kind in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        run_foreground(deployment, "existing")
        deployment.stop.assert_called_once()
        self.assertEqual({kind: signal.getsignal(kind) for kind in before}, before)

    def test_real_child_is_reaped_and_data_is_not_deleted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            data = root / "persistent"
            data.write_text("do not reset")
            child = subprocess.Popen([sys.executable, "-c",
                "import signal,sys; signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); print('ready',flush=True); signal.pause()"],
                stdout=subprocess.PIPE, text=True)
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(child.stdout, selectors.EVENT_READ)
                    self.assertTrue(selector.select(timeout=2), "physical child must become ready")
                    self.assertEqual(child.stdout.readline(), "ready\n")
                deployment = object.__new__(Deployment)
                deployment.children = []
                deployment.adopt_child("node-1", child)
                deployment.stop()
                self.assertEqual(child.returncode, 0)
                self.assertEqual(deployment.children, [])
                self.assertEqual(data.read_text(), "do not reset")
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=2)
                child.stdout.close()

    def test_node_error_is_not_normalized_to_success(self):
        child = subprocess.Popen([sys.executable, "-c", "raise SystemExit(5)"])
        child.wait(timeout=2)
        deployment = object.__new__(Deployment)
        deployment.children = []
        deployment.adopt_child("node-1", child)
        with self.assertRaisesRegex(RuntimeError, "node-1.*5"):
            deployment.stop()
        self.assertEqual(deployment.children, [])

    def test_one_stop_error_does_not_skip_other_children(self):
        deployment = object.__new__(Deployment)
        deployment.children = []
        first, second = MagicMock(), MagicMock()
        for process in (first, second):
            process.poll.side_effect = [None, 0]
            process.wait.return_value = 0
        second.terminate.side_effect = OSError(5, "physical failure")
        deployment.adopt_child("first", first)
        deployment.adopt_child("second", second)
        with self.assertRaisesRegex(RuntimeError, "terminate.*5"):
            deployment.stop()
        first.terminate.assert_called_once()
        first.wait.assert_called_once()
        second.wait.assert_called_once()
        self.assertEqual(deployment.children, [])


if __name__ == "__main__":
    unittest.main()
