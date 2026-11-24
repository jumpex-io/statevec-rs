"""Standard local lifecycle; application bootstrap and services stay outside."""
import argparse
import pathlib
import time

from . import Deployment, initialize, read, record_artifacts, run_foreground
from .cluster import CONFIG_VERSION

LOCAL_PROFILE_FORMAT = "statevec.local-profile"


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
    init = sub.add_parser("init", help="generate local Node configs (one voter by default; no Core initialization)")
    init.add_argument("profile", type=pathlib.Path)
    init.add_argument("root", type=pathlib.Path)
    init.add_argument("--store-path", type=pathlib.Path, help="override the provisioned Core store path")
    init.add_argument("--projector-config", type=pathlib.Path, help="override the Projector application config path")
    init.add_argument("--base-port", type=int, help="override the first peer's ingress port")
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
        if profile.get("format") != LOCAL_PROFILE_FORMAT:
            parser.error("unsupported local profile format")
        if type(profile.get("version")) is not int or profile["version"] != CONFIG_VERSION:
            parser.error("unsupported local profile version")
        required = {"format", "version", "artifacts", "node", "base_port", "core"}
        if not required <= profile.keys() or profile.keys() - (required | {"projector", "sdk_revision"}):
            parser.error("unknown or missing local profile field")
        def resolve(path):
            return str((args.profile.resolve().parent / path).resolve())
        core = profile["core"]
        if args.store_path is not None:
            core["store_path"] = str(args.store_path.resolve())
        elif "store_path" in core:
            core["store_path"] = resolve(core["store_path"])
        else:
            parser.error("core requires store_path in the profile or --store-path")
        projector = profile.get("projector")
        if projector is not None:
            if args.projector_config is not None:
                projector["config_path"] = str(args.projector_config.resolve())
            elif "config_path" in projector:
                projector["config_path"] = resolve(projector["config_path"])
            else:
                parser.error("projector requires config_path in the profile or --projector-config")
        elif args.projector_config is not None:
            parser.error("--projector-config requires a projector profile")
        base_port = profile["base_port"] if args.base_port is None else args.base_port
        initialize(args.root, resolve(profile["artifacts"]), profile["node"], base_port, core, projector,
                   sdk_revision=profile.get("sdk_revision"), profile=args.profile)
    else:
        run_foreground(Deployment(args.root, time.monotonic()+60), args.mode)


if __name__ == "__main__":
    main()
