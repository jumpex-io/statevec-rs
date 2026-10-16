"""Standard local lifecycle; application bootstrap and services stay outside."""
import argparse
import pathlib
import time

from . import Deployment, initialize, read, record_artifacts, run_foreground


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    artifacts = sub.add_parser("artifacts", help="record explicitly supplied local files; no download/build")
    artifacts.add_argument("output", type=pathlib.Path)
    artifacts.add_argument("--node", type=pathlib.Path, required=True)
    artifacts.add_argument("--admin", type=pathlib.Path, required=True)
    artifacts.add_argument("--host-revision", required=True)
    artifacts.add_argument("--sdk-revision", required=True)
    artifacts.add_argument("--file", action="append", default=[], metavar="LABEL=PATH")
    init = sub.add_parser("init", help="generate a new local cluster from an application profile")
    init.add_argument("profile", type=pathlib.Path)
    init.add_argument("root", type=pathlib.Path)
    run = sub.add_parser("run", help="foreground official nodes; Ctrl-C stops and joins children")
    run.add_argument("root", type=pathlib.Path)
    run.add_argument("mode", choices=("fresh", "existing"))
    args = parser.parse_args()
    if args.command == "artifacts":
        files = {"node": args.node, "admin": args.admin}
        for item in args.file:
            name, separator, path = item.partition("=")
            if not separator or name in files:
                parser.error("--file requires a distinct LABEL=PATH")
            files[name] = pathlib.Path(path)
        record_artifacts(args.output, files, args.host_revision, args.sdk_revision)
    elif args.command == "init":
        profile = read(args.profile)
        if profile.get("version") != 1 or set(profile) - {"version", "artifacts", "node", "base_port", "core", "projector"}:
            parser.error("unsupported local profile version/fields")
        def resolve(path):
            return str((args.profile.resolve().parent / path).resolve())
        for name in ("core", "projector"):
            if profile.get(name) is not None:
                profile[name]["config_path"] = resolve(profile[name]["config_path"])
        initialize(args.root, resolve(profile["artifacts"]), profile["node"], profile["base_port"],
                   profile["core"], profile.get("projector"))
    else:
        run_foreground(Deployment(args.root, time.monotonic()+60), args.mode)


if __name__ == "__main__":
    main()
