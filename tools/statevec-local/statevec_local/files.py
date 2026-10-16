"""Explicit local artifact selection. Hashes do not attest build provenance."""
import hashlib
import json
import pathlib
import re


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read(path):
    with pathlib.Path(path).open("rb") as source:
        data = source.read(1_048_577)
    if len(data) > 1_048_576:
        raise ValueError("configuration exceeds 1 MiB")
    value = json.loads(data, object_pairs_hook=_object)
    if not isinstance(value, dict):
        raise ValueError("configuration must be an object")
    return value


def write(path, value):
    pathlib.Path(path).write_text(json.dumps(value, indent=2) + "\n")


def _revision(value):
    if not isinstance(value, str) or not re.fullmatch("[0-9a-f]{40}", value):
        raise ValueError("revision requires a complete lowercase 40-hex SHA")


def _label(value):
    if not isinstance(value, str) or not re.fullmatch("[a-z][a-z0-9_-]{0,63}", value):
        raise ValueError("artifact/process name must be a short ASCII label")


def _digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def record_artifacts(output, files, host_revision, sdk_revision):
    _revision(host_revision)
    _revision(sdk_revision)
    if not {"node", "admin"} <= files.keys():
        raise ValueError("node and admin artifacts are required")
    spec = {"host_revision": host_revision, "sdk_revision": sdk_revision, "files": {}}
    for name, file in files.items():
        _label(name)
        path = pathlib.Path(file).resolve(strict=True)
        spec["files"][name] = {"path": str(path), "sha256": _digest(path)}
    with pathlib.Path(output).open("x") as destination:
        json.dump(spec, destination, indent=2)
        destination.write("\n")
    return spec


def checked_artifacts(path, *, sdk_revision=None):
    spec = read(path)
    _revision(spec.get("host_revision"))
    _revision(spec.get("sdk_revision"))
    if sdk_revision is not None and spec["sdk_revision"] != sdk_revision:
        raise ValueError("application and artifacts require the same SDK revision")
    files = spec.get("files")
    if not isinstance(files, dict) or not {"node", "admin"} <= files.keys():
        raise ValueError("node and admin artifacts are required")
    for name, item in files.items():
        _label(name)
        artifact = pathlib.Path(item["path"])
        if not artifact.is_absolute() or _digest(artifact) != item["sha256"]:
            raise ValueError(f"artifact path/digest mismatch: {name}")
    return spec
