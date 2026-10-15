"""Sprite generation specs (JSON) — configurable size/palette/frames."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from .pngutil import RGBA, parse_hex_color, transparent

PixelOp = Tuple[int, int, str]  # x, y, hex color


@dataclass
class FrameSpec:
    duration_ms: int
    pixels: List[PixelOp] = field(default_factory=list)


@dataclass
class SpriteSpec:
    name: str
    width: int
    height: int
    transparent_background: bool
    palette: Dict[str, str]
    frames: List[FrameSpec]
    tag: str = "idle"
    layer: str = "organism"
    source_aseprite: Optional[Path] = None
    export_sheet: Optional[Path] = None
    export_data: Optional[Path] = None
    provenance: Dict[str, Any] = field(default_factory=dict)
    raw: Dict[str, Any] = field(default_factory=dict)

    def resolved_palette(self) -> Dict[str, RGBA]:
        return {k: parse_hex_color(v) for k, v in self.palette.items()}

    def raster_frames(self) -> List[List[RGBA]]:
        out: List[List[RGBA]] = []
        for frame in self.frames:
            canvas = [transparent() for _ in range(self.width * self.height)]
            for x, y, color in frame.pixels:
                if not (0 <= x < self.width and 0 <= y < self.height):
                    raise ValueError(
                        f"pixel out of bounds in {self.name}: ({x},{y}) "
                        f"for {self.width}x{self.height}"
                    )
                canvas[y * self.width + x] = parse_hex_color(color)
            out.append(canvas)
        return out

    def durations(self) -> List[int]:
        return [f.duration_ms for f in self.frames]


def _parse_pixels(entries: Sequence[Any]) -> List[PixelOp]:
    pixels: List[PixelOp] = []
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError(f"pixel entry must be object, got {entry!r}")
        pixels.append((int(entry["x"]), int(entry["y"]), str(entry["color"])))
    return pixels


def load_spec(path: Path, base_dir: Optional[Path] = None) -> SpriteSpec:
    path = path.resolve()
    base = (base_dir or path.parent).resolve()
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("spec root must be an object")

    size = data.get("size") or {}
    width = int(data.get("width") or size.get("width"))
    height = int(data.get("height") or size.get("height"))
    frames_raw = data.get("frames") or []
    frames = [
        FrameSpec(
            duration_ms=int(fr.get("duration_ms", data.get("frame_duration_ms", 100))),
            pixels=_parse_pixels(fr.get("pixels") or []),
        )
        for fr in frames_raw
    ]
    if not frames:
        raise ValueError("spec needs at least one frame")

    paths = data.get("paths") or {}

    def _rel(key: str) -> Optional[Path]:
        raw = paths.get(key) or data.get(key)
        if not raw:
            return None
        p = Path(raw)
        return p if p.is_absolute() else (base / p).resolve()

    return SpriteSpec(
        name=str(data.get("name") or path.stem),
        width=width,
        height=height,
        transparent_background=bool(data.get("transparent_background", True)),
        palette={str(k): str(v) for k, v in (data.get("palette") or {}).items()},
        frames=frames,
        tag=str(data.get("tag") or "idle"),
        layer=str(data.get("layer") or "organism"),
        source_aseprite=_rel("source_aseprite") or _rel("aseprite"),
        export_sheet=_rel("export_sheet") or _rel("sheet"),
        export_data=_rel("export_data") or _rel("data"),
        provenance=dict(data.get("provenance") or {}),
        raw=data,
    )


def dump_job_for_lua(spec: SpriteSpec, job_path: Path) -> None:
    """Write a resolved job JSON consumed by ``create_from_spec.lua``."""
    frames = []
    for fr in spec.frames:
        frames.append(
            {
                "duration_ms": fr.duration_ms,
                "pixels": [
                    {"x": x, "y": y, "color": color} for x, y, color in fr.pixels
                ],
            }
        )
    job = {
        "name": spec.name,
        "width": spec.width,
        "height": spec.height,
        "transparent_background": spec.transparent_background,
        "palette": spec.palette,
        "tag": spec.tag,
        "layer": spec.layer,
        "frames": frames,
        "source_aseprite": str(spec.source_aseprite) if spec.source_aseprite else "",
    }
    job_path.parent.mkdir(parents=True, exist_ok=True)
    job_path.write_text(json.dumps(job, indent=2) + "\n", encoding="utf-8")
