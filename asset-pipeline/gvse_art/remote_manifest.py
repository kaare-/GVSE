"""Local fixture boundary for future remote ComfyUI art jobs.

Expected remote shape (planned, not hardcoded to any host):

- ``POST /v1/art/jobs``
- ``GET /v1/art/jobs/{id}``
- ``GET /v1/art/jobs/{id}/assets/{filename}``

GvsE consumes downloaded PNGs + a manifest, then runs them through Aseprite.
"""

from __future__ import annotations

import json
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional


MANIFEST_SCHEMA_VERSION = 1


@dataclass
class RemoteAsset:
    filename: str
    role: str  # png | preview | other
    path: Path
    width: Optional[int] = None
    height: Optional[int] = None
    sha256: Optional[str] = None


@dataclass
class RemoteManifest:
    schema_version: int
    job_id: str
    status: str
    assets: List[RemoteAsset]
    prompt: Optional[str] = None
    seed: Optional[int] = None
    tags: List[str] = field(default_factory=list)
    raw: Dict[str, Any] = field(default_factory=dict)

    @property
    def primary_png(self) -> Optional[RemoteAsset]:
        for asset in self.assets:
            if asset.role == "png":
                return asset
        for asset in self.assets:
            if asset.path.suffix.lower() == ".png":
                return asset
        return None


def load_manifest(path: Path) -> RemoteManifest:
    path = path.resolve()
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("manifest root must be an object")
    version = int(data.get("schema_version", 0))
    if version != MANIFEST_SCHEMA_VERSION:
        raise ValueError(
            f"unsupported manifest schema_version {version}; "
            f"expected {MANIFEST_SCHEMA_VERSION}"
        )
    base = path.parent
    assets: List[RemoteAsset] = []
    for entry in data.get("assets") or []:
        filename = str(entry["filename"])
        rel = entry.get("path") or filename
        asset_path = Path(rel)
        if not asset_path.is_absolute():
            asset_path = (base / asset_path).resolve()
        assets.append(
            RemoteAsset(
                filename=filename,
                role=str(entry.get("role") or "png"),
                path=asset_path,
                width=entry.get("width"),
                height=entry.get("height"),
                sha256=entry.get("sha256"),
            )
        )
    return RemoteManifest(
        schema_version=version,
        job_id=str(data.get("job_id") or path.stem),
        status=str(data.get("status") or "succeeded"),
        assets=assets,
        prompt=data.get("prompt"),
        seed=data.get("seed"),
        tags=[str(t) for t in (data.get("tags") or [])],
        raw=data,
    )


def stage_manifest_assets(
    manifest: RemoteManifest,
    dest_dir: Path,
    *,
    copy: bool = True,
) -> List[Path]:
    """Copy (or link) manifest assets into ``dest_dir`` for Aseprite import."""
    dest_dir.mkdir(parents=True, exist_ok=True)
    staged: List[Path] = []
    for asset in manifest.assets:
        if not asset.path.is_file():
            raise FileNotFoundError(f"manifest asset missing: {asset.path}")
        target = dest_dir / asset.filename
        if copy:
            shutil.copy2(asset.path, target)
        else:
            if target.exists():
                target.unlink()
            target.symlink_to(asset.path)
        staged.append(target)
    # Keep a provenance copy of the manifest beside staged files.
    provenance = {
        "schema_version": manifest.schema_version,
        "job_id": manifest.job_id,
        "status": manifest.status,
        "prompt": manifest.prompt,
        "seed": manifest.seed,
        "tags": manifest.tags,
        "source_manifest": str(manifest.raw.get("_source") or ""),
        "assets": [
            {
                "filename": a.filename,
                "role": a.role,
                "width": a.width,
                "height": a.height,
                "sha256": a.sha256,
            }
            for a in manifest.assets
        ],
    }
    (dest_dir / "import-provenance.json").write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
    )
    return staged
