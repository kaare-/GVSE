"""Validate Aseprite sources and export metadata."""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional

from .find_aseprite import find_aseprite
from .runner import run_aseprite


@dataclass
class ValidationIssue:
    level: str  # error | warning | info
    message: str


@dataclass
class ValidationReport:
    path: Path
    ok: bool
    issues: List[ValidationIssue] = field(default_factory=list)
    aseprite_tested: bool = False
    width: Optional[int] = None
    height: Optional[int] = None
    frames: Optional[int] = None

    def add(self, level: str, message: str) -> None:
        self.issues.append(ValidationIssue(level, message))


def _read_ase_header(path: Path) -> tuple[int, int, int]:
    data = path.read_bytes()
    if len(data) < 128:
        raise ValueError("file too small for ASE header")
    magic = struct.unpack_from("<H", data, 4)[0]
    if magic != 0xA5E0:
        raise ValueError(f"bad ASE magic: 0x{magic:04X}")
    frames = struct.unpack_from("<H", data, 6)[0]
    width = struct.unpack_from("<H", data, 8)[0]
    height = struct.unpack_from("<H", data, 10)[0]
    return width, height, frames


def validate_source(
    path: Path,
    *,
    expect_width: Optional[int] = None,
    expect_height: Optional[int] = None,
    expect_min_frames: Optional[int] = None,
    require_transparency_hint: bool = False,
) -> ValidationReport:
    report = ValidationReport(path=path, ok=True)
    if not path.is_file():
        report.ok = False
        report.add("error", f"missing file: {path}")
        return report

    try:
        width, height, frames = _read_ase_header(path)
    except ValueError as exc:
        report.ok = False
        report.add("error", f"ASE header: {exc}")
        return report

    report.width = width
    report.height = height
    report.frames = frames
    report.add("info", f"header {width}x{height}, frames={frames}")

    if expect_width is not None and width != expect_width:
        report.ok = False
        report.add("error", f"width {width} != expected {expect_width}")
    if expect_height is not None and height != expect_height:
        report.ok = False
        report.add("error", f"height {height} != expected {expect_height}")
    if expect_min_frames is not None and frames < expect_min_frames:
        report.ok = False
        report.add(
            "error", f"frames {frames} < expected minimum {expect_min_frames}"
        )
    if require_transparency_hint and frames < 1:
        report.ok = False
        report.add("error", "expected at least one frame for transparent sprite")

    binary = find_aseprite()
    if binary is None:
        report.add(
            "warning",
            "Aseprite not installed — CLI list-tags/list-layers untested",
        )
        return report

    result = run_aseprite(
        [str(path), "--list-layers", "--list-tags"],
        aseprite=binary,
    )
    report.aseprite_tested = True
    if not result.ok:
        report.ok = False
        report.add(
            "error",
            f"aseprite exit {result.returncode}: {result.stderr.strip() or result.stdout.strip()}",
        )
    else:
        report.add("info", "aseprite --list-layers/--list-tags ok")
        if result.stdout.strip():
            report.add("info", f"aseprite stdout:\n{result.stdout.strip()}")
    return report


def validate_sheet_meta(data_path: Path) -> ValidationReport:
    report = ValidationReport(path=data_path, ok=True)
    if not data_path.is_file():
        report.ok = False
        report.add("error", f"missing metadata: {data_path}")
        return report
    try:
        payload = json.loads(data_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        report.ok = False
        report.add("error", f"invalid JSON: {exc}")
        return report
    frames = payload.get("frames")
    meta = payload.get("meta") or {}
    if not frames:
        report.ok = False
        report.add("error", "metadata missing frames")
    else:
        report.frames = len(frames) if isinstance(frames, list) else None
        report.add("info", f"metadata frames entries={report.frames}")
    size = meta.get("size") or {}
    if "w" in size and "h" in size:
        report.width = int(size["w"])
        report.height = int(size["h"])
    return report
