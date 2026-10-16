"""Single-machine foreground processes; Node remains the recovery authority."""
import copy
import dataclasses
import json
import pathlib
import signal
import subprocess
import time

from .files import _label, checked_artifacts, read, write


def _peer_ids(values):
    if not isinstance(values, list) or any(type(x) is not int or not 0 < x < 2**64 for x in values):
        raise ValueError("peer IDs must be nonzero u64 integers")
    return values


def initialize(root, artifacts, node, base_port, core, projector=None):
    """Write new local Node configs; plugin config bytes remain application-owned.

    core/projector contain artifact (manifest label), target and config_path.
    The optional projector is attached to each declared learner. Node validates
    its own complete configuration on launch; this helper never repairs roots.
    """
    spec = checked_artifacts(artifacts)
    root = pathlib.Path(root).resolve()
    node = copy.deepcopy(node)
    voters = _peer_ids(node["initial_voters"])
    learners = _peer_ids(node.get("learner_only_peers", []))
    ids = voters + learners
    if not voters or len(ids) > 16 or len(set(ids)) != len(ids):
        raise ValueError("require voters and at most 16 distinct local peers")
    if type(base_port) is not int or not 1024 <= base_port <= 65536 - len(ids)*3:
        raise ValueError("local peer port range exceeds 1024..=65535")
    reserved = {"peer_id", "data_root", "peers", "ingress_addr", "admin_socket_path",
                "core_plugin", "projector_plugin"}
    if reserved & node.keys():
        raise ValueError("node template contains per-node fields")

    def plugin(binding):
        value = dict(binding)
        artifact = spec["files"][value.pop("artifact")]
        config = pathlib.Path(value["config_path"])
        if not config.is_absolute():
            raise ValueError("plugin config_path must be absolute")
        return dict(value, path=artifact["path"], sha256=artifact["sha256"])

    core_binding = plugin(core)
    projector_binding = plugin(projector) if projector is not None else None
    if projector_binding is not None and not learners:
        raise ValueError("a projector needs a learner")
    peers = [{"peer_id": peer, "control_addr": f"127.0.0.1:{base_port+slot*3+1}",
              "bulk_addr": f"127.0.0.1:{base_port+slot*3+2}"} for slot, peer in enumerate(ids)]
    # No mkdir, socket or existing-root write precedes validation above. A later
    # physical write failure leaves a partial new directory for operator inspection.
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    write(root / "artifacts.json", spec)
    write(root / "deployment.json", {"version": 1, "port": base_port,
          "voters": voters, "learners": learners})
    for slot, peer in enumerate(ids):
        (root / f"root-{peer}").mkdir(mode=0o700)
        config = dict(node, version=1, peer_id=peer, peers=peers,
            data_root=str(root / f"root-{peer}"), ingress_addr=f"127.0.0.1:{base_port+slot*3}",
            admin_socket_path=str(root / f"admin-{peer}.sock"), core_plugin=core_binding,
            projector_plugin=projector_binding if peer in learners else None)
        write(root / f"node-{peer}.json", config)


@dataclasses.dataclass
class Child:
    name: str
    process: subprocess.Popen
    signal_exit_ok: bool = False


class Deployment:
    def __init__(self, root, deadline):
        self.root = pathlib.Path(root).resolve()
        self.deadline = deadline
        self.spec = checked_artifacts(self.root / "artifacts.json")
        self.config = read(self.root / "deployment.json")
        if self.config.get("version") != 1:
            raise ValueError("unsupported local deployment version")
        self.port = self.config["port"]
        self.voters = _peer_ids(self.config["voters"])
        self.learners = _peer_ids(self.config["learners"])
        if not self.voters or len(set(self.voters+self.learners)) != len(self.voters+self.learners):
            raise ValueError("invalid local deployment peer IDs")
        self.children = []
        self._stop_requested = False

    def binary(self, name):
        return self.spec["files"][name]["path"]

    def request_stop(self):
        # Signal handlers only latch intent. Raising between physical Popen and
        # adopt_child would strand an accepted child outside cleanup custody.
        self._stop_requested = True

    def check(self):
        if self._stop_requested:
            raise KeyboardInterrupt
        if time.monotonic() >= self.deadline:
            raise TimeoutError(f"local deployment deadline; inspect logs in {self.root}")
        for child in self.children:
            if child.process.poll() is not None:
                raise RuntimeError(f"{child.name} exited {child.process.returncode}; see {self.root / (child.name + '.log')}")

    def adopt_child(self, name, process, *, signal_exit_ok=False):
        self.children.append(Child(name, process, signal_exit_ok))

    def spawn(self, name, binary, *args, signal_exit_ok=False):
        self.check()
        _label(name)
        with (self.root / (name + ".log")).open("ab") as log:
            child = subprocess.Popen([self.binary(binary), *map(str, args)], cwd=self.root,
                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        self.adopt_child(name, child, signal_exit_ok=signal_exit_ok)

    def admin(self, peer, *args):
        self.check()
        result = subprocess.run([self.binary("admin"), "--socket", str(self.root / f"admin-{peer}.sock"),
            "--timeout-ms", "500", *args], capture_output=True, timeout=1, check=False)
        if len(result.stdout) > 32768:
            raise ValueError("admin JSON exceeds the public output bound")
        reply = json.loads(result.stdout)
        if not isinstance(reply, dict) or reply.get("version") != 1:
            raise ValueError("unsupported admin JSON")
        return result.returncode, reply

    def admit_learner(self, learner):
        uncertain = False
        while True:
            self.check()
            for peer in self.voters:
                code, state = self.admin(peer, "query-membership")
                if code == 0 and state["category"] == "applied_membership":
                    if str(learner) in state["learners"]:
                        return
                    if state["role"] == "leader" and not uncertain:
                        code, reply = self.admin(peer, "add-learner", "--peer-id", str(learner))
                        if code == 0 and reply["category"] == "proposal_accepted":
                            uncertain = True
                        elif code == 4 and reply["category"] == "owner_refusal" and reply.get("admission") == "not_admitted":
                            transient = {"ready_pending", "apply_pending", "not_leader", "leader_not_current",
                                         "leader_not_applied", "not_serving", "proposal_busy", "pending_configuration"}
                            if reply["reason"]["kind"] not in transient:
                                raise RuntimeError(f"learner admission refused; reconcile configuration/state: {reply}")
                        elif reply.get("admission") not in ("not_sent", "not_admitted"):
                            uncertain = True
            time.sleep(0.01)

    def start(self, mode):
        if mode not in ("fresh", "existing") or self.children:
            raise ValueError("select fresh/existing once on a stopped deployment")
        for peer in self.voters:
            self.spawn(f"node-{peer}", "node", mode, self.root / f"node-{peer}.json")
        for peer in self.learners:
            self.admit_learner(peer)
            self.spawn(f"node-{peer}", "node", mode, self.root / f"node-{peer}.json")

    def stop(self):
        failures = []
        deadline = time.monotonic() + 5
        for child in reversed(self.children):
            signaled = child.process.poll() is None
            if signaled:
                try:
                    child.process.terminate()
                except OSError as error:
                    failures.append((child.name, "terminate", error.errno))
            try:
                code = child.process.wait(timeout=max(0.01, deadline-time.monotonic()))
            except subprocess.TimeoutExpired:
                try:
                    child.process.kill()
                    child.process.wait(timeout=1)
                    code = "stop deadline; forced SIGKILL"
                except (OSError, subprocess.TimeoutExpired) as error:
                    failures.append((child.name, "kill/reap", type(error).__name__))
                    continue
            except OSError as error:
                failures.append((child.name, "wait", error.errno))
                continue
            if code != 0 and not (child.signal_exit_ok and signaled and code == -signal.SIGTERM):
                failures.append((child.name, code))
        self.children = [child for child in self.children if child.process.poll() is None]
        if failures:
            raise RuntimeError(f"child stop failures (data retained): {failures}")


def run_foreground(deployment, mode):
    """Own the direct child processes through partial startup and all exits."""
    def stop(_signum, _frame):
        deployment.request_stop()
    previous = {kind: signal.signal(kind, stop) for kind in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        deployment.start(mode)
        deployment.check()
        deployment.deadline = float("inf")
        print(f"processes started; application readiness is separate: {deployment.root}", flush=True)
        while True:
            deployment.check()
            time.sleep(0.1)
    except KeyboardInterrupt:
        pass
    finally:
        # Repeated stop signals cannot interrupt resource retirement.
        for kind in previous:
            signal.signal(kind, signal.SIG_IGN)
        try:
            deployment.stop()
        finally:
            for kind, handler in previous.items():
                signal.signal(kind, handler)
