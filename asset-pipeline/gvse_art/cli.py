"""Cursor-facing CLI: ``art create|import|export|validate|which|import-remote``."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Optional, Sequence

from . import __version__
from .commands import (
    ArtError,
    create_from_spec,
    export_source,
    import_png,
    import_remote_manifest,
    validate,
    which_aseprite,
)
from .find_aseprite import find_aseprite


def _print_json(payload: object) -> None:
    print(json.dumps(payload, indent=2))


def _add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--force",
        action="store_true",
        help="allow overwriting an existing .aseprite source",
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="art",
        description=(
            "GvsE headless Aseprite asset pipeline. "
            "Does not redistribute Aseprite; set ASEPRITE_PATH when needed."
        ),
    )
    parser.add_argument("--version", action="version", version=f"gvse-art {__version__}")
    sub = parser.add_subparsers(dest="command", required=True)

    p_which = sub.add_parser("which", help="locate Aseprite binary")
    p_which.set_defaults(func=_cmd_which)

    p_create = sub.add_parser("create", help="create sprite from JSON spec via Lua")
    p_create.add_argument("--spec", required=True, type=Path, help="path to sprite spec JSON")
    _add_common(p_create)
    p_create.set_defaults(func=_cmd_create)

    p_import = sub.add_parser("import", help="import PNG into .aseprite")
    p_import.add_argument("--input", required=True, type=Path, help="input PNG")
    p_import.add_argument("--output", required=True, type=Path, help="output .aseprite")
    p_import.add_argument("--width", type=int, default=None)
    p_import.add_argument("--height", type=int, default=None)
    p_import.add_argument(
        "--palette",
        type=Path,
        default=None,
        help="optional palette JSON map of hex colors",
    )
    _add_common(p_import)
    p_import.set_defaults(func=_cmd_import)

    p_remote = sub.add_parser(
        "import-remote",
        help="consume a local remote-art manifest fixture, then import PNG",
    )
    p_remote.add_argument("--manifest", required=True, type=Path)
    p_remote.add_argument(
        "--stage-dir",
        required=True,
        type=Path,
        help="directory for staged PNG + provenance (never overwrites artist sources)",
    )
    p_remote.add_argument("--output", required=True, type=Path, help="output .aseprite")
    _add_common(p_remote)
    p_remote.set_defaults(func=_cmd_import_remote)

    p_export = sub.add_parser("export", help="export spritesheet + JSON via Aseprite CLI")
    p_export.add_argument("--source", required=True, type=Path)
    p_export.add_argument("--sheet", required=True, type=Path)
    p_export.add_argument("--data", required=True, type=Path)
    p_export.add_argument(
        "--sheet-type",
        default="horizontal",
        choices=["horizontal", "vertical", "rows", "columns", "packed"],
    )
    p_export.set_defaults(func=_cmd_export)

    p_val = sub.add_parser("validate", help="validate .aseprite and/or sheet JSON")
    p_val.add_argument("--source", type=Path, default=None)
    p_val.add_argument("--data", type=Path, default=None)
    p_val.add_argument("--expect-width", type=int, default=None)
    p_val.add_argument("--expect-height", type=int, default=None)
    p_val.add_argument("--expect-min-frames", type=int, default=None)
    p_val.set_defaults(func=_cmd_validate)

    return parser


def _cmd_which(_args: argparse.Namespace) -> int:
    _print_json(which_aseprite())
    return 0 if find_aseprite() else 2


def _cmd_create(args: argparse.Namespace) -> int:
    _print_json(create_from_spec(args.spec, force=args.force))
    return 0


def _cmd_import(args: argparse.Namespace) -> int:
    _print_json(
        import_png(
            args.input,
            args.output,
            width=args.width,
            height=args.height,
            palette_json=args.palette,
            force=args.force,
        )
    )
    return 0


def _cmd_import_remote(args: argparse.Namespace) -> int:
    _print_json(
        import_remote_manifest(
            args.manifest,
            args.stage_dir,
            args.output,
            force=args.force,
        )
    )
    return 0


def _cmd_export(args: argparse.Namespace) -> int:
    _print_json(
        export_source(
            args.source,
            args.sheet,
            args.data,
            sheet_type=args.sheet_type,
        )
    )
    return 0


def _cmd_validate(args: argparse.Namespace) -> int:
    payload = validate(
        source=args.source,
        data=args.data,
        expect_width=args.expect_width,
        expect_height=args.expect_height,
        expect_min_frames=args.expect_min_frames,
    )
    _print_json(payload)
    return 0 if payload.get("ok") else 1


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(list(argv) if argv is not None else None)
    try:
        return int(args.func(args))
    except ArtError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    except FileNotFoundError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
