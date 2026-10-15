"""Minimal PNG encode/decode (stdlib only) for fixtures and offline exports."""

from __future__ import annotations

import struct
import zlib
from pathlib import Path
from typing import List, Sequence, Tuple

RGBA = Tuple[int, int, int, int]


def _chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def write_rgba_png(path: Path, width: int, height: int, pixels: Sequence[RGBA]) -> None:
    if width <= 0 or height <= 0:
        raise ValueError("width and height must be positive")
    if len(pixels) != width * height:
        raise ValueError(f"expected {width * height} pixels, got {len(pixels)}")
    raw = bytearray()
    i = 0
    for _y in range(height):
        raw.append(0)  # filter None
        for _x in range(width):
            r, g, b, a = pixels[i]
            raw.extend((r & 0xFF, g & 0xFF, b & 0xFF, a & 0xFF))
            i += 1
    compressed = zlib.compress(bytes(raw), 9)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", compressed)
    png += _chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def write_rgba_png_rows(path: Path, rows: Sequence[Sequence[RGBA]]) -> None:
    height = len(rows)
    if height == 0:
        raise ValueError("empty image")
    width = len(rows[0])
    flat: List[RGBA] = []
    for row in rows:
        if len(row) != width:
            raise ValueError("ragged rows")
        flat.extend(row)
    write_rgba_png(path, width, height, flat)


def read_rgba_png(path: Path) -> tuple[int, int, List[RGBA]]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"not a PNG: {path}")
    offset = 8
    width = height = None
    color_type = None
    idat = bytearray()
    while offset + 8 <= len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        tag = data[offset + 4 : offset + 8]
        chunk = data[offset + 8 : offset + 8 + length]
        offset += 12 + length
        if tag == b"IHDR":
            width, height, bit_depth, color_type, *_ = struct.unpack(">IIBBBBB", chunk)
            if bit_depth != 8 or color_type not in (2, 6):
                raise ValueError(
                    f"unsupported PNG (bit_depth={bit_depth}, color_type={color_type})"
                )
        elif tag == b"IDAT":
            idat.extend(chunk)
        elif tag == b"IEND":
            break
    if width is None or height is None or color_type is None:
        raise ValueError("missing IHDR")
    raw = zlib.decompress(bytes(idat))
    channels = 4 if color_type == 6 else 3
    stride = 1 + width * channels
    if len(raw) < stride * height:
        raise ValueError("truncated PNG image data")
    pixels: List[RGBA] = []
    for y in range(height):
        row = raw[y * stride : (y + 1) * stride]
        if row[0] != 0:
            raise ValueError("unsupported PNG filter (only None/0)")
        p = 1
        for _x in range(width):
            if channels == 4:
                pixels.append((row[p], row[p + 1], row[p + 2], row[p + 3]))
            else:
                pixels.append((row[p], row[p + 1], row[p + 2], 255))
            p += channels
    return width, height, pixels


def horizontal_spritesheet(
    frames: Sequence[Sequence[RGBA]], width: int, height: int
) -> List[RGBA]:
    sheet: List[RGBA] = []
    for y in range(height):
        for frame in frames:
            base = y * width
            sheet.extend(frame[base : base + width])
    return sheet


def parse_hex_color(value: str) -> RGBA:
    text = value.strip().lstrip("#")
    if len(text) == 6:
        r = int(text[0:2], 16)
        g = int(text[2:4], 16)
        b = int(text[4:6], 16)
        return (r, g, b, 255)
    if len(text) == 8:
        r = int(text[0:2], 16)
        g = int(text[2:4], 16)
        b = int(text[4:6], 16)
        a = int(text[6:8], 16)
        return (r, g, b, a)
    raise ValueError(f"invalid hex color: {value}")


def transparent() -> RGBA:
    return (0, 0, 0, 0)
