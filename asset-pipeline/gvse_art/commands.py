"""High-level art create/import/export/validate operations."""

from __future__ import annotations

import json
import shutil
from dataclasses import asdict
from pathlib import Path
from typing import Any, Dict, Optional

from . import ase_writer, pngutil
from .find_aseprite import find_aseprite
from .remote_manifest import load_manifest, stage_manifest_assets
from .runner import PIPELINE_ROOT, export_spritesheet, run_lua, write_temp_job
from .spec import dump_job_for_lua, load_spec
from .validate import validate_sheet_meta, validate_source


class ArtError(RuntimeError):
    pass


def _refuse_overwrite(path: Path, *, force: bool) -> None:
    if path.exists() and not force:
        raise ArtError(
            f"refusing to overwrite existing file: {path} (pass --force to replace)"
        )


def _offline_create(spec_path: Path, *, force: bool) -> Dict[str, Any]:
    spec = load_spec(spec_path)
    if spec.source_aseprite is None:
        raise ArtError("spec missing paths.source_aseprite")
    _refuse_overwrite(spec.source_aseprite, force=force)
    frames = spec.raster_frames()
    ase_writer.write_aseprite(
        spec.source_aseprite,
        spec.width,
        spec.height,
        frames,
        frame_durations_ms=spec.durations(),
        tag_name=spec.tag,
        layer_name=spec.layer,
    )
    sheet_path = spec.export_sheet
    data_path = spec.export_data
    meta: Dict[str, Any] = {
        "mode": "offline-fixture-writer",
        "aseprite_available": False,
        "note": (
            "Generated without Aseprite CLI. Re-run `art create` with Aseprite "
            "installed to verify Lua/CLI path."
        ),
        "source": str(spec.source_aseprite),
    }
    if sheet_path is not None and data_path is not None:
        sheet_pixels = pngutil.horizontal_spritesheet(frames, spec.width, spec.height)
        pngutil.write_rgba_png(
            sheet_path, spec.width * len(frames), spec.height, sheet_pixels
        )
        frames_meta = []
        for i, dur in enumerate(spec.durations()):
            frames_meta.append(
                {
                    "filename": f"{spec.name}-{i}.png",
                    "frame": {
                        "x": i * spec.width,
                        "y": 0,
                        "w": spec.width,
                        "h": spec.height,
                    },
                    "duration": dur,
                }
            )
        payload = {
            "frames": frames_meta,
            "meta": {
                "app": "gvse-art-offline",
                "image": sheet_path.name,
                "size": {"w": spec.width * len(frames), "h": spec.height},
                "frameTags": [
                    {"name": spec.tag, "from": 0, "to": len(frames) - 1, "direction": "forward"}
                ],
                "layers": [{"name": spec.layer}],
                "gvse": {
                    "spec": str(spec_path),
                    "transparent_background": spec.transparent_background,
                    "provenance": spec.provenance,
                },
            },
        }
        data_path.parent.mkdir(parents=True, exist_ok=True)
        data_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        meta["sheet"] = str(sheet_path)
        meta["data"] = str(data_path)
    return meta


def create_from_spec(spec_path: Path, *, force: bool = False) -> Dict[str, Any]:
    spec_path = spec_path.resolve()
    spec = load_spec(spec_path)
    if spec.source_aseprite is None:
        raise ArtError("spec missing paths.source_aseprite")

    binary = find_aseprite()
    if binary is None:
        return _offline_create(spec_path, force=force)

    _refuse_overwrite(spec.source_aseprite, force=force)
    job_path = write_temp_job({})
    try:
        dump_job_for_lua(spec, job_path)
        result = run_lua(
            "create_from_spec.lua",
            {"job": str(job_path)},
            aseprite=binary,
        )
        if not result.ok:
            raise ArtError(
                "Aseprite create failed:\n"
                + (result.stderr or result.stdout or f"exit {result.returncode}")
            )
        meta: Dict[str, Any] = {
            "mode": "aseprite-lua",
            "aseprite_available": True,
            "aseprite": str(binary.path),
            "version": binary.version,
            "source": str(spec.source_aseprite),
            "argv": result.argv,
        }
        if spec.export_sheet and spec.export_data:
            exp = export_spritesheet(
                spec.source_aseprite,
                spec.export_sheet,
                spec.export_data,
                aseprite=binary,
            )
            if not exp.ok:
                raise ArtError(
                    "Aseprite export failed:\n"
                    + (exp.stderr or exp.stdout or f"exit {exp.returncode}")
                )
            # Attach GvsE provenance into a sidecar (do not rewrite Aseprite JSON).
            side = spec.export_data.with_suffix(".gvse.json")
            side.write_text(
                json.dumps(
                    {
                        "spec": str(spec_path),
                        "transparent_background": spec.transparent_background,
                        "provenance": spec.provenance,
                        "aseprite_data": str(spec.export_data),
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            meta["sheet"] = str(spec.export_sheet)
            meta["data"] = str(spec.export_data)
            meta["gvse_sidecar"] = str(side)
        return meta
    finally:
        job_path.unlink(missing_ok=True)


def import_png(
    input_png: Path,
    dest_aseprite: Path,
    *,
    width: Optional[int] = None,
    height: Optional[int] = None,
    palette_json: Optional[Path] = None,
    force: bool = False,
) -> Dict[str, Any]:
    input_png = input_png.resolve()
    dest_aseprite = dest_aseprite.resolve()
    if not input_png.is_file():
        raise ArtError(f"input PNG missing: {input_png}")
    _refuse_overwrite(dest_aseprite, force=force)

    binary = find_aseprite()
    if binary is None:
        # Offline: copy pixels into a one-frame ASE via fixture writer.
        w, h, pixels = pngutil.read_rgba_png(input_png)
        if (width is not None and width != w) or (height is not None and height != h):
            raise ArtError(
                f"offline import cannot resize {w}x{h} → {width}x{height}; "
                "install Aseprite for resize/palette processing"
            )
        ase_writer.write_aseprite(dest_aseprite, w, h, [pixels], frame_durations_ms=[100])
        return {
            "mode": "offline-fixture-writer",
            "aseprite_available": False,
            "source": str(dest_aseprite),
            "input": str(input_png),
            "note": "Imported without Aseprite CLI (resize/palette Lua untested).",
        }

    params = {
        "input": str(input_png),
        "output": str(dest_aseprite),
    }
    if width:
        params["width"] = str(width)
    if height:
        params["height"] = str(height)
    if palette_json:
        params["palette"] = str(palette_json.resolve())

    result = run_lua("import_png.lua", params, aseprite=binary)
    if not result.ok:
        raise ArtError(
            "Aseprite import failed:\n"
            + (result.stderr or result.stdout or f"exit {result.returncode}")
        )
    return {
        "mode": "aseprite-lua",
        "aseprite_available": True,
        "aseprite": str(binary.path),
        "version": binary.version,
        "source": str(dest_aseprite),
        "input": str(input_png),
        "argv": result.argv,
    }


def import_remote_manifest(
    manifest_path: Path,
    dest_dir: Path,
    dest_aseprite: Path,
    *,
    force: bool = False,
) -> Dict[str, Any]:
    manifest = load_manifest(manifest_path)
    staged = stage_manifest_assets(manifest, dest_dir)
    primary = manifest.primary_png
    if primary is None:
        raise ArtError("manifest has no PNG asset")
    staged_png = dest_dir / primary.filename
    result = import_png(staged_png, dest_aseprite, force=force)
    result["manifest"] = str(manifest_path.resolve())
    result["job_id"] = manifest.job_id
    result["staged"] = [str(p) for p in staged]
    return result


def export_source(
    source: Path,
    sheet: Path,
    data: Path,
    *,
    sheet_type: str = "horizontal",
) -> Dict[str, Any]:
    source = source.resolve()
    if not source.is_file():
        raise ArtError(f"source missing: {source}")
    binary = find_aseprite()
    if binary is None:
        raise ArtError(
            "Aseprite required for `art export` (spritesheet JSON from CLI). "
            "Set ASEPRITE_PATH or install Aseprite."
        )
    result = export_spritesheet(
        source, sheet.resolve(), data.resolve(), sheet_type=sheet_type, aseprite=binary
    )
    if not result.ok:
        raise ArtError(
            "Aseprite export failed:\n"
            + (result.stderr or result.stdout or f"exit {result.returncode}")
        )
    return {
        "mode": "aseprite-cli",
        "aseprite_available": True,
        "aseprite": str(binary.path),
        "version": binary.version,
        "sheet": str(sheet.resolve()),
        "data": str(data.resolve()),
        "argv": result.argv,
    }


def validate(
    source: Optional[Path] = None,
    data: Optional[Path] = None,
    *,
    expect_width: Optional[int] = None,
    expect_height: Optional[int] = None,
    expect_min_frames: Optional[int] = None,
) -> Dict[str, Any]:
    reports = []
    ok = True
    if source is not None:
        report = validate_source(
            source.resolve(),
            expect_width=expect_width,
            expect_height=expect_height,
            expect_min_frames=expect_min_frames,
        )
        ok = ok and report.ok
        reports.append(
            {
                "path": str(report.path),
                "ok": report.ok,
                "aseprite_tested": report.aseprite_tested,
                "width": report.width,
                "height": report.height,
                "frames": report.frames,
                "issues": [asdict(i) for i in report.issues],
            }
        )
    if data is not None:
        report = validate_sheet_meta(data.resolve())
        ok = ok and report.ok
        reports.append(
            {
                "path": str(report.path),
                "ok": report.ok,
                "width": report.width,
                "height": report.height,
                "frames": report.frames,
                "issues": [asdict(i) for i in report.issues],
            }
        )
    if not reports:
        raise ArtError("validate requires --source and/or --data")
    return {"ok": ok, "reports": reports, "pipeline_root": str(PIPELINE_ROOT)}


def which_aseprite() -> Dict[str, Any]:
    found = find_aseprite()
    if found is None:
        return {
            "available": False,
            "ASEPRITE_PATH": None,
            "hint": "Set ASEPRITE_PATH to your licensed Aseprite binary.",
        }
    return {
        "available": True,
        "path": str(found.path),
        "version": found.version,
        "source": found.source,
    }


def preserve_source_copy(src: Path, provenance_dir: Path) -> Path:
    """Copy an artist/source file into provenance without mutating the original."""
    provenance_dir.mkdir(parents=True, exist_ok=True)
    dest = provenance_dir / src.name
    if src.resolve() == dest.resolve():
        return dest
    shutil.copy2(src, dest)
    return dest
