"""Locate a licensed Aseprite binary without assuming PATH.

Search order:
1. ``ASEPRITE_PATH`` environment variable (file or directory)
2. Common install locations (Steam, /opt, Applications, local builds)
3. ``PATH`` via ``shutil.which``

This module never downloads or redistributes Aseprite.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional


@dataclass(frozen=True)
class AsepriteBinary:
    path: Path
    version: Optional[str]
    source: str  # env | common | path

    @property
    def available(self) -> bool:
        return self.path.is_file() and os.access(self.path, os.X_OK)


def _expand_candidates() -> list[tuple[str, Path]]:
    home = Path.home()
    names = ("aseprite", "Aseprite")
    dirs = [
        home / ".local/share/Steam/steamapps/common/Aseprite",
        home / ".steam/steam/steamapps/common/Aseprite",
        home / ".steam/debian-installation/steamapps/common/Aseprite",
        Path("/usr/bin"),
        Path("/usr/local/bin"),
        Path("/opt/aseprite"),
        Path("/opt/Aseprite"),
        home / "aseprite/build/bin",
        home / "Applications/Aseprite",
        home / ".local/bin",
        Path("/Applications/Aseprite.app/Contents/MacOS"),
        Path("C:/Program Files/Aseprite"),
        Path("C:/Program Files (x86)/Aseprite"),
    ]
    out: list[tuple[str, Path]] = []
    for d in dirs:
        for name in names:
            out.append(("common", d / name))
        # Steam layout sometimes nests the binary one level deeper.
        out.append(("common", d / "aseprite.exe"))
    return out


def _version_of(binary: Path) -> Optional[str]:
    try:
        proc = subprocess.run(
            [str(binary), "--version"],
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    text = (proc.stdout or proc.stderr or "").strip()
    if not text:
        return None
    return text.splitlines()[0].strip()


def _from_env() -> Optional[AsepriteBinary]:
    raw = os.environ.get("ASEPRITE_PATH", "").strip()
    if not raw:
        return None
    path = Path(raw).expanduser()
    if path.is_dir():
        for name in ("aseprite", "Aseprite", "aseprite.exe"):
            candidate = path / name
            if candidate.is_file():
                path = candidate
                break
    if path.is_file() and os.access(path, os.X_OK):
        return AsepriteBinary(path=path.resolve(), version=_version_of(path), source="env")
    return None


def find_aseprite(
    extra_paths: Optional[Iterable[Path]] = None,
) -> Optional[AsepriteBinary]:
    """Return the first usable Aseprite binary, or ``None`` if missing."""
    env_hit = _from_env()
    if env_hit is not None:
        return env_hit

    candidates: list[tuple[str, Path]] = []
    if extra_paths:
        for p in extra_paths:
            candidates.append(("common", Path(p)))
    candidates.extend(_expand_candidates())

    seen: set[Path] = set()
    for source, path in candidates:
        try:
            resolved = path.expanduser().resolve()
        except OSError:
            continue
        if resolved in seen:
            continue
        seen.add(resolved)
        if resolved.is_file() and os.access(resolved, os.X_OK):
            return AsepriteBinary(
                path=resolved, version=_version_of(resolved), source=source
            )

    which = shutil.which("aseprite") or shutil.which("Aseprite")
    if which:
        path = Path(which).resolve()
        return AsepriteBinary(path=path, version=_version_of(path), source="path")
    return None


def require_aseprite() -> AsepriteBinary:
    found = find_aseprite()
    if found is None:
        raise FileNotFoundError(
            "Aseprite binary not found. Set ASEPRITE_PATH to the executable "
            "(do not redistribute licensed Aseprite into this repo). "
            "See asset-pipeline/README.md."
        )
    return found
