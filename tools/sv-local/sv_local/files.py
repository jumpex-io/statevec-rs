"""Select an installed bundle or an explicit local artifact set."""
import hashlib
import json
import os
import pathlib
import platform
import re
import sys


RELEASE_FORMAT = "statevec.release-manifest"


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


def _release_artifacts(path, manifest, profile):
    if type(manifest.get("version")) is not int or manifest["version"] != 1:
        raise ValueError("unsupported release manifest version")
    target = f"{platform.machine()}-unknown-linux-gnu"
    libc = ""
    if sys.platform == "linux":
        try:
            libc = os.confstr("CS_GNU_LIBC_VERSION") or ""
        except (ValueError, OSError):
            pass
    if not libc.startswith("glibc ") or manifest.get("target") != target:
        raise ValueError(f"release manifest requires a matching Linux GNU target, host is {target}")
    version = manifest.get("release_version")
    if not isinstance(version, str) or not version:
        raise ValueError("release manifest requires a release version")

    inventory = manifest.get("files")
    roles = manifest.get("artifacts")
    declared = manifest.get("digests")
    if (not isinstance(inventory, dict) or not isinstance(roles, dict)
            or not {"node", "admin", "sdk"} <= roles.keys() or not isinstance(declared, dict)
            or roles.keys() != declared.keys()):
        raise ValueError("incomplete release artifact inventory")

    root = pathlib.Path(path).resolve().parent
    files = {}
    for name, sha256 in inventory.items():
        relative = pathlib.PurePosixPath(name)
        if relative.is_absolute() or not relative.parts or ".." in relative.parts or relative.as_posix() != name:
            raise ValueError(f"invalid relative release artifact path: {name}")
        asset = (root / relative).resolve()
        if (not asset.is_relative_to(root) or not asset.is_file()
                or not isinstance(sha256, str) or not re.fullmatch("[a-f0-9]{64}", sha256)
                or _digest(asset) != sha256):
            raise ValueError(f"release artifact path/digest mismatch: {name}")
        files[name] = {"path": str(asset), "sha256": sha256}

    if profile is not None:
        profile = pathlib.Path(profile).resolve()
        if profile.is_relative_to(root) and profile.relative_to(root).as_posix() not in files:
            raise ValueError(f"profile is missing from the release inventory: {profile}")

    selected = {}
    for label, name in roles.items():
        _label(label)
        if not isinstance(name, str) or name not in files or inventory[name] != declared[label]:
            raise ValueError(f"release artifact selection/digest mismatch: {label}")
        selected[label] = files[name]
    return {"host_revision": manifest.get("source_revision"), "sdk_revision": manifest.get("sdk_commit"),
            "release_version": version, "files": selected}


def checked_artifacts(path, *, sdk_revision=None, profile=None):
    spec = read(path)
    is_release = spec.get("format") == RELEASE_FORMAT
    if is_release:
        spec = _release_artifacts(path, spec, profile)
    _revision(spec.get("host_revision"))
    _revision(spec.get("sdk_revision"))
    if sdk_revision is not None and spec["sdk_revision"] != sdk_revision:
        raise ValueError("application and artifacts require the same SDK revision")
    if is_release:
        return spec
    files = spec.get("files")
    if not isinstance(files, dict) or not {"node", "admin"} <= files.keys():
        raise ValueError("node and admin artifacts are required")
    for name, item in files.items():
        _label(name)
        artifact = pathlib.Path(item["path"])
        if not artifact.is_absolute() or _digest(artifact) != item["sha256"]:
            raise ValueError(f"artifact path/digest mismatch: {name}")
    return spec
