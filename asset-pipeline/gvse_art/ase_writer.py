"""Minimal `.aseprite` (ASE) writer for offline fixtures.

Primary production path is the Aseprite CLI + Lua scripts. This writer exists
so the sample organism can ship a native editable source when Aseprite is not
installed in CI. Prefer regenerating via ``art create`` once Aseprite is present.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path
from typing import List, Sequence, Tuple

RGBA = Tuple[int, int, int, int]

ASE_MAGIC = 0xA5E0
FRAME_MAGIC = 0xF1FA
CHUNK_LAYER = 0x2004
CHUNK_CEL = 0x2005
CHUNK_FRAME_TAGS = 0x2018
CHUNK_COLOR_PROFILE = 0x2007
CEL_RAW = 0
CEL_COMPRESSED = 2


def _u16(n: int) -> bytes:
    return struct.pack("<H", n & 0xFFFF)


def _i16(n: int) -> bytes:
    return struct.pack("<h", n)


def _u32(n: int) -> bytes:
    return struct.pack("<I", n & 0xFFFFFFFF)


def _string(text: str) -> bytes:
    raw = text.encode("utf-8")
    return _u16(len(raw)) + raw


def _chunk(chunk_type: int, body: bytes) -> bytes:
    return _u32(len(body) + 6) + _u16(chunk_type) + body


def _layer_chunk(name: str = "Layer 1") -> bytes:
    # flags: visible | editable
    flags = 1 | 2
    layer_type = 0  # normal
    child_level = 0
    default_width = 0
    default_height = 0
    blend_mode = 0
    opacity = 255
    body = (
        _u16(flags)
        + _u16(layer_type)
        + _u16(child_level)
        + _u16(default_width)
        + _u16(default_height)
        + _u16(blend_mode)
        + bytes([opacity])
        + bytes(3)
        + _string(name)
    )
    return _chunk(CHUNK_LAYER, body)


def _cel_chunk(
    layer_index: int,
    x: int,
    y: int,
    opacity: int,
    width: int,
    height: int,
    pixels: Sequence[RGBA],
    compressed: bool = True,
) -> bytes:
    if len(pixels) != width * height:
        raise ValueError("pixel count mismatch")
    rgba = bytearray()
    for r, g, b, a in pixels:
        rgba.extend((r & 0xFF, g & 0xFF, b & 0xFF, a & 0xFF))
    if compressed:
        cel_type = CEL_COMPRESSED
        image = zlib.compress(bytes(rgba), 9)
    else:
        cel_type = CEL_RAW
        image = bytes(rgba)
    body = (
        _u16(layer_index)
        + _i16(x)
        + _i16(y)
        + bytes([opacity & 0xFF])
        + _u16(cel_type)
        + bytes(5)
        + _u16(width)
        + _u16(height)
        + image
    )
    return _chunk(CHUNK_CEL, body)


def _color_profile_srgb() -> bytes:
    # type 1 = sRGB, flags 0, fixed gamma unused
    body = _u16(1) + _u16(0) + struct.pack("<f", 1.0) + bytes(8)
    return _chunk(CHUNK_COLOR_PROFILE, body)


def _frame_tags_chunk(tag_name: str, from_frame: int, to_frame: int) -> bytes:
    # one tag, forward animation
    body = _u16(1) + bytes(8)
    body += (
        _u16(from_frame)
        + _u16(to_frame)
        + bytes([0])  # AniDir forward
        + bytes(8)
        + bytes([0, 0, 0, 255])  # tag color RGBA
        + bytes(1)
        + _string(tag_name)
    )
    return _chunk(CHUNK_FRAME_TAGS, body)


def _frame(duration_ms: int, chunks: Sequence[bytes]) -> bytes:
    body = b"".join(chunks)
    # header: size, magic, old chunks, duration, reserved, new chunks
    header = (
        _u32(0)  # patched below
        + _u16(FRAME_MAGIC)
        + _u16(0)  # old chunks field (0 when using new)
        + _u16(duration_ms)
        + bytes(2)
        + _u32(len(chunks))
    )
    frame = bytearray(header + body)
    struct.pack_into("<I", frame, 0, len(frame))
    return bytes(frame)


def write_aseprite(
    path: Path,
    width: int,
    height: int,
    frames: Sequence[Sequence[RGBA]],
    frame_durations_ms: Sequence[int] | None = None,
    tag_name: str = "idle",
    layer_name: str = "organism",
) -> None:
    if width <= 0 or height <= 0:
        raise ValueError("invalid size")
    n = len(frames)
    if n == 0:
        raise ValueError("no frames")
    if frame_durations_ms is None:
        durations = [100] * n
    else:
        durations = list(frame_durations_ms)
        if len(durations) != n:
            raise ValueError("duration count mismatch")

    frame_blobs: List[bytes] = []
    for i, pixels in enumerate(frames):
        chunks: List[bytes] = []
        if i == 0:
            chunks.append(_color_profile_srgb())
            chunks.append(_layer_chunk(layer_name))
            if n > 1:
                chunks.append(_frame_tags_chunk(tag_name, 0, n - 1))
        chunks.append(
            _cel_chunk(0, 0, 0, 255, width, height, pixels, compressed=True)
        )
        frame_blobs.append(_frame(durations[i], chunks))

    header = bytearray(128)
    struct.pack_into("<I", header, 0, 0)  # size patched later
    struct.pack_into("<H", header, 4, ASE_MAGIC)
    struct.pack_into("<H", header, 6, n)
    struct.pack_into("<H", header, 8, width)
    struct.pack_into("<H", header, 10, height)
    struct.pack_into("<H", header, 12, 32)  # RGBA
    struct.pack_into("<I", header, 14, 1)  # layer opacity valid
    struct.pack_into("<H", header, 18, durations[0])
    # bytes 20-127 zero / reserved
    struct.pack_into("<B", header, 28, 0)  # palette entry size unused
    struct.pack_into("<H", header, 30, 0)  # number of colors 0 = 256 for indexed
    struct.pack_into("<B", header, 32, 1)  # pixel width
    struct.pack_into("<B", header, 33, 1)  # pixel height
    struct.pack_into("<h", header, 34, 0)  # grid x
    struct.pack_into("<h", header, 36, 0)  # grid y
    struct.pack_into("<H", header, 38, 16)
    struct.pack_into("<H", header, 40, 16)

    blob = bytes(header) + b"".join(frame_blobs)
    out = bytearray(blob)
    struct.pack_into("<I", out, 0, len(out))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(out))
