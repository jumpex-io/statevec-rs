import hashlib
import json
import pathlib
import platform
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import MagicMock, patch

from sv_local import Deployment, checked_artifacts, initialize, read, record_artifacts, run_foreground, write
from sv_local import cluster
from sv_local.__main__ import LOCAL_PROFILE_FORMAT, main


@unittest.skipUnless(sys.platform == "linux", "release bundles target native Linux GNU")
class ReleaseBundle(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.workspace = pathlib.Path(temporary.name)
        self.bundle = self.workspace / "original"
        self.bundle.mkdir()
        (self.bundle / "bin").mkdir()
        (self.bundle / "profiles").mkdir()
        paths = {"node": "bin/sv-node", "admin": "bin/sv-admin", "sdk": "sdk.tar"}
        for name, path in paths.items():
            (self.bundle / path).write_text(name)
        write(self.bundle / "profiles/local.json", {
            "format": LOCAL_PROFILE_FORMAT, "version": cluster.CONFIG_VERSION,
            "artifacts": "../artifacts.json", "sdk_revision": "2"*40, "base_port": 24100,
            "node": {"cluster_name": "bank"}, "core": {"initial_core_version": "a"*64},
        })
        files = {name: hashlib.sha256((self.bundle / name).read_bytes()).hexdigest()
                 for name in (*paths.values(), "profiles/local.json")}
        self.manifest = {"format": "statevec.release-manifest", "version": 1, "release_version": "1.0.0-rc.1",
                         "source_revision": "1"*40, "sdk_commit": "2"*40,
                         "target": f"{platform.machine()}-unknown-linux-gnu", "artifacts": paths,
                         "files": files, "digests": {label: files[path] for label, path in paths.items()}}
        write(self.bundle / "artifacts.json", self.manifest)

    def initialize_from_profile(self, bundle, root):
        with patch("sys.argv", ["sv-local", "init", str(bundle / "profiles/local.json"), str(root),
                                "--store-path", str(self.workspace / "materials")]):
            main()

    def test_moved_bundle_selects_the_shipped_node_and_a_fresh_genesis(self):
        moved = self.bundle.rename(self.workspace / "moved")
        roots = [self.workspace / name for name in ("first", "rebuilt")]
        genesis = []
        for root in roots:
            self.initialize_from_profile(moved, root)
            deployment = Deployment(root, time.monotonic() + 1)
            self.assertEqual(deployment.binary("node"), str(moved / "bin/sv-node"))
            self.assertEqual(deployment.spec["sdk_revision"], "2"*40)
            genesis.append(read(root / "node-1.json")["genesis_config_hash"])
        self.assertEqual([len(value) for value in genesis], [32, 32])
        self.assertNotEqual(*genesis)

    def test_changed_bundle_file_is_refused_before_creating_a_root(self):
        (self.bundle / "bin/sv-node").write_text("a different build")
        root = self.workspace / "cluster"
        with self.assertRaisesRegex(ValueError, "path/digest mismatch: bin/sv-node"):
            self.initialize_from_profile(self.bundle, root)
        self.assertFalse(root.exists())

    def test_profile_refuses_another_sdk_commit_even_if_all_hashes_match(self):
        self.manifest["sdk_commit"] = "3"*40
        write(self.bundle / "artifacts.json", self.manifest)
        root = self.workspace / "cluster"
        with self.assertRaisesRegex(ValueError, "same SDK revision"):
            self.initialize_from_profile(self.bundle, root)
        self.assertFalse(root.exists())

    def test_wrong_linux_target_is_refused_before_creating_a_root(self):
        self.manifest["target"] = "different-unknown-linux-gnu"
        write(self.bundle / "artifacts.json", self.manifest)
        root = self.workspace / "cluster"
        with self.assertRaisesRegex(ValueError, "matching Linux GNU target"):
            self.initialize_from_profile(self.bundle, root)
        self.assertFalse(root.exists())


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
        self.core = {"store_path": str(self.directory / "materials"), "initial_core_version": "a"*64}

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
            dict(self.core, max_load_attempts=7, max_artifact_bytes=1048576))
        first, second = read(root / "node-9.json"), read(root / "node-20.json")
        self.assertEqual(first["ingress_addr"], "127.0.0.1:24100")
        self.assertEqual(second["ingress_addr"], "127.0.0.1:24104")
        self.assertEqual(first["admin_listen_port"], 24103)
        self.assertEqual(second["admin_listen_port"], 24107)
        self.assertEqual(first["admin_listen_ip"], "127.0.0.1")
        self.assertEqual(read(root / "deployment.json")["format"], cluster.DEPLOYMENT_FORMAT)
        self.assertEqual(read(root / "deployment.json")["version"], cluster.CONFIG_VERSION)
        self.assertEqual(first["initial_voters"], [9])
        self.assertEqual(first["core_plugin"], dict(self.core, max_load_attempts=7, max_artifact_bytes=1048576))
        self.assertIsNone(second["projector_plugin"])
        # Each member's checkpoint root is its own sibling of its data root.
        for peer, config in ((9, first), (20, second)):
            self.assertEqual(config["data_root"], str(root.resolve() / f"root-{peer}"))
            self.assertEqual(config["checkpoint_root"], str(root.resolve() / f"checkpoints-{peer}"))
            self.assertTrue(pathlib.Path(config["checkpoint_root"]).is_dir())
            self.assertEqual(pathlib.Path(config["checkpoint_root"]).stat().st_mode & 0o777, 0o700)
        with self.assertRaises(FileExistsError):
            initialize(root, self.manifest, {"initial_voters": [9]}, 24100,
                self.core)
        self.assertEqual(read(root / "node-9.json"), first)

    def test_cli_defaults_to_one_voter_and_resolves_store_from_profile(self):
        profile = self.directory / "profile.json"
        root = self.directory / "cluster"
        write(profile, {"format": LOCAL_PROFILE_FORMAT, "version": cluster.CONFIG_VERSION,
                        "artifacts": "artifacts.json", "base_port": 24100,
                        "node": {"cluster_name": "local", "cluster_mode": "cross_check"},
                        "core": dict(self.core, store_path="materials")})
        with patch("sys.argv", ["sv-local", "init", str(profile), str(root)]):
            main()
        node = read(root / "node-1.json")
        self.assertEqual(node["core_plugin"], self.core)
        self.assertEqual(node["initial_voters"], [1])
        self.assertEqual([peer["peer_id"] for peer in node["peers"]], [1])
        self.assertIsNone(node["projector_plugin"])
        self.assertEqual(read(root / "deployment.json")["learners"], [])
        self.assertEqual(node["client_reads"], {"enabled": True, "leader": True}, "its only node is the leader")

    def test_only_the_single_voter_profile_enables_client_reads_unless_the_profile_sets_them(self):
        for name, template, expected in (
                ("three_voters", {"initial_voters": [1, 2, 3]}, None),
                ("voter_and_learner", {"initial_voters": [1], "learner_only_peers": [2]}, None),
                ("explicit_off", {"client_reads": {"enabled": False}}, {"enabled": False}),
                ("explicit_followers", {"initial_voters": [1, 2, 3], "client_reads": {"enabled": True}},
                 {"enabled": True})):
            with self.subTest(name):
                root = self.directory / name
                initialize(root, self.manifest, template, 24100, self.core)
                for config in root.glob("node-*.json"):
                    self.assertEqual(read(config).get("client_reads"), expected, config.name)

    def test_legacy_core_and_relative_store_refuse_without_creating_root(self):
        root = self.directory / "cluster"
        for core in ({"artifact": "core", "config_path": str(self.file)},
                     dict(self.core, store_path="relative"),
                     dict(self.core, path=str(self.file))):
            with self.subTest(core=core), self.assertRaises(ValueError):
                initialize(root, self.manifest, {"initial_voters": [1]}, 24100, core)
            self.assertFalse(root.exists())

    def test_a_template_checkpoint_root_refuses_before_creating_root(self):
        # One shared value would give every member the same checkpoint root.
        root = self.directory / "cluster"
        with self.assertRaisesRegex(ValueError, "per-node fields"):
            initialize(root, self.manifest, {"checkpoint_root": str(self.directory / "shared")}, 24100, self.core)
        self.assertFalse(root.exists())

    def test_bad_init_refuses_before_creating_root(self):
        root = self.directory / "cluster"
        for voters, learners, port in (([1], [1], 24100), ([0], [], 24100), ([True], [], 24100),
                                       ([1], [], 65534), ([], [4], 24100)):
            with self.subTest(voters=voters, learners=learners, port=port):
                with self.assertRaises(ValueError):
                    initialize(root, self.manifest, {"initial_voters": voters, "learner_only_peers": learners},
                               port, self.core)
                self.assertFalse(root.exists())

    def test_admin_port_range_and_metrics_overlap_refuse_before_writing(self):
        for port in (1023, 65533):
            root = self.directory / str(port)
            with self.assertRaisesRegex(ValueError, "port range"):
                initialize(root, self.manifest, {}, port, self.core)
            self.assertFalse(root.exists())
        for host in ("127.0.0.1", "0.0.0.0", "[::]", "[::ffff:127.0.0.1]"):
            root = self.directory / "overlap"
            with self.assertRaisesRegex(ValueError, "overlaps"):
                initialize(root, self.manifest, {"metrics_listen_address": f"{host}:24103"}, 24100, self.core)
            self.assertFalse(root.exists())
        root = self.directory / "last-range"
        initialize(root, self.manifest, {}, 65532, self.core)
        self.assertEqual(read(root / "node-1.json")["admin_listen_port"], 65535)

    def test_old_deployment_layout_refuses_before_any_process_launch(self):
        root = self.directory / "old"
        initialize(root, self.manifest, {}, 24100, self.core)
        metadata = read(root / "deployment.json")
        del metadata["format"]
        (root / "deployment.json").write_text(json.dumps(metadata))
        with patch.object(cluster.subprocess, "Popen") as launch:
            with self.assertRaisesRegex(ValueError, "deployment format"):
                Deployment(root, time.monotonic() + 5)
            launch.assert_not_called()

    def test_cli_duplicate_artifact_label_refuses_without_output(self):
        output = self.directory / "new.json"
        with patch("sys.argv", ["sv-local", "artifacts", str(output), "--node", str(self.file),
            "--admin", str(self.file), "--host-revision", "1"*40, "--sdk-revision", "2"*40,
            "--file", "node=other"]), self.assertRaises(SystemExit):
            main()
        self.assertFalse(output.exists())


class Membership(unittest.TestCase):
    def test_mutation_uses_local_config_identity_and_refuses_failed_inspection(self):
        instance = object.__new__(Deployment)
        instance.root = pathlib.Path("/fixture")
        instance.port, instance.voters, instance.learners = 24000, [1, 2, 3], []
        instance.deadline = time.monotonic() + 5
        instance.check = lambda: None
        instance.binary = lambda name: f"/bin/{name}"
        inspected = subprocess.CompletedProcess([], 0, json.dumps({
            "format": "statevec.inspect", "version": 1, "configured_cluster_identity": "a"*64
        }))
        answered = subprocess.CompletedProcess([], 4, json.dumps({"version": 1, "category": "target_mismatch"}))
        with patch.object(cluster.subprocess, "run", side_effect=[inspected, answered]) as run:
            code, reply = instance.admin(2, "add-learner", "--peer-id", "4")
        self.assertEqual((code, reply["category"]), (4, "target_mismatch"))
        self.assertEqual(run.call_args_list[0].args[0], ["/bin/node", "inspect", "/fixture/node-2.json"])
        self.assertEqual(run.call_args_list[1].args[0], ["/bin/admin", "--admin-port", "24007",
            "--timeout-ms", "500", "--cluster-identity", "a"*64, "--expected-peer", "2",
            "add-learner", "--peer-id", "4"])
        with patch.object(cluster.subprocess, "run", side_effect=subprocess.CalledProcessError(2, "inspect")) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                instance.admin(2, "add-learner", "--peer-id", "4")
            self.assertEqual(run.call_count, 1, "failed local inspection cannot send a mutation")

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
    def test_stop_between_spawn_and_adoption_reaps_the_child_and_starts_no_next_peer(self):
        real_popen = subprocess.Popen
        for kind in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            with self.subTest(signal=kind), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                manifest = root / "artifacts.json"
                record_artifacts(manifest, {name: sys.executable for name in ("node", "admin", "core")},
                                 "1" * 40, "2" * 40)
                initialize(root / "cluster", manifest, {"initial_voters": [1, 2]}, 24100,
                           {"store_path": str(root / "materials"), "initial_core_version": "a"*64})
                deployment = Deployment(root / "cluster", time.monotonic() + 5)
                created = []

                def spawn_then_stop(_args, **kwargs):
                    # Change only the child executable at the physical spawn seam.
                    # Production spawn/adopt/stop and actual signal delivery remain.
                    kwargs["stdout"] = subprocess.PIPE
                    child = real_popen([sys.executable, "-c",
                        "import signal,sys; signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); "
                        "print('ready',flush=True); signal.pause()"], **kwargs)
                    created.append(child)
                    with selectors.DefaultSelector() as selector:
                        selector.register(child.stdout, selectors.EVENT_READ)
                        self.assertTrue(selector.select(timeout=2), "child must install its stop handler")
                        self.assertEqual(child.stdout.readline(), b"ready\n")
                    signal.raise_signal(kind)
                    return child

                try:
                    with patch.object(cluster.subprocess, "Popen", side_effect=spawn_then_stop):
                        run_foreground(deployment, "fresh")
                    self.assertEqual(len(created), 1, "stop must prevent the next voter from starting")
                    self.assertEqual(created[0].returncode, 0, "the accepted child must be stopped and joined")
                    self.assertEqual(deployment.children, [])
                    self.assertTrue((deployment.root / "node-1.json").exists(), "stop must preserve config/data")
                finally:
                    for child in created:
                        if child.poll() is None:
                            child.kill()
                        child.wait(timeout=2)
                        child.stdout.close()

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
