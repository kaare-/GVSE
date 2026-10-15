"""Invoke Aseprite in batch mode with Lua scripts and CLI export flags."""

from __future__ import annotations

import json
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import List, Mapping, Optional, Sequence

from .find_aseprite import AsepriteBinary, require_aseprite

PIPELINE_ROOT = Path(__file__).resolve().parent.parent
LUA_DIR = PIPELINE_ROOT / "lua"


@dataclass
class RunResult:
    argv: List[str]
    returncode: int
    stdout: str
    stderr: str
    aseprite: AsepriteBinary

    @property
    def ok(self) -> bool:
        return self.returncode == 0


def _script_params(params: Mapping[str, str]) -> List[str]:
    out: List[str] = []
    for key, value in params.items():
        out.extend(["--script-param", f"{key}={value}"])
    return out


def run_aseprite(
    args: Sequence[str],
    *,
    aseprite: Optional[AsepriteBinary] = None,
    timeout: float = 120.0,
) -> RunResult:
    binary = aseprite or require_aseprite()
    argv = [str(binary.path), "-b", *args]
    proc = subprocess.run(
        argv,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    return RunResult(
        argv=argv,
        returncode=proc.returncode,
        stdout=proc.stdout or "",
        stderr=proc.stderr or "",
        aseprite=binary,
    )


def run_lua(
    script_name: str,
    params: Mapping[str, str],
    *,
    aseprite: Optional[AsepriteBinary] = None,
    extra_args: Optional[Sequence[str]] = None,
) -> RunResult:
    script = LUA_DIR / script_name
    if not script.is_file():
        raise FileNotFoundError(f"Lua script missing: {script}")
    args = [*_script_params(params), "--script", str(script)]
    if extra_args:
        args.extend(extra_args)
    return run_aseprite(args, aseprite=aseprite)


def export_spritesheet(
    source: Path,
    sheet: Path,
    data: Path,
    *,
    sheet_type: str = "horizontal",
    list_tags: bool = True,
    list_layers: bool = True,
    aseprite: Optional[AsepriteBinary] = None,
) -> RunResult:
    sheet.parent.mkdir(parents=True, exist_ok=True)
    data.parent.mkdir(parents=True, exist_ok=True)
    args: List[str] = [str(source)]
    if list_tags:
        args.append("--list-tags")
    if list_layers:
        args.append("--list-layers")
    args.extend(
        [
            "--sheet-type",
            sheet_type,
            "--sheet",
            str(sheet),
            "--data",
            str(data),
            "--format",
            "json-array",
        ]
    )
    return run_aseprite(args, aseprite=aseprite)


def save_as_png(source: Path, dest: Path, *, aseprite: Optional[AsepriteBinary] = None) -> RunResult:
    dest.parent.mkdir(parents=True, exist_ok=True)
    return run_aseprite(
        [str(source), "--save-as", str(dest)],
        aseprite=aseprite,
    )


def write_temp_job(payload: Mapping) -> Path:
    tmp = tempfile.NamedTemporaryFile(
        prefix="gvse-art-job-", suffix=".json", delete=False, mode="w", encoding="utf-8"
    )
    with tmp:
        json.dump(payload, tmp, indent=2)
        tmp.write("\n")
    return Path(tmp.name)
