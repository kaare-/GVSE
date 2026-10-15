"""Optional HTTP client for a future remote ComfyUI art service.

Not imported by the game. Disabled unless ``GVSE_ART_BASE_URL`` (or an
explicit base_url argument) is provided. No credentials are hardcoded.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Optional


class RemoteClientError(RuntimeError):
    pass


@dataclass
class RemoteClient:
    base_url: str
    token: Optional[str] = None
    timeout_s: float = 60.0

    @classmethod
    def from_env(cls) -> Optional["RemoteClient"]:
        base = os.environ.get("GVSE_ART_BASE_URL", "").strip().rstrip("/")
        if not base:
            return None
        token = os.environ.get("GVSE_ART_TOKEN", "").strip() or None
        return cls(base_url=base, token=token)

    def _headers(self) -> Dict[str, str]:
        headers = {"Accept": "application/json", "Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        return headers

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        url = f"{self.base_url}{path}"
        data = None if body is None else json.dumps(body).encode("utf-8")
        req = urllib.request.Request(url, data=data, headers=self._headers(), method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_s) as resp:
                payload = resp.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise RemoteClientError(f"{method} {url} -> HTTP {exc.code}: {detail}") from exc
        except urllib.error.URLError as exc:
            raise RemoteClientError(f"{method} {url} failed: {exc}") from exc
        if not payload.strip():
            return {}
        return json.loads(payload)

    def submit_job(self, spec: Dict[str, Any]) -> Dict[str, Any]:
        return self._request("POST", "/v1/art/jobs", spec)

    def get_job(self, job_id: str) -> Dict[str, Any]:
        return self._request("GET", f"/v1/art/jobs/{job_id}")

    def download_asset(self, job_id: str, filename: str, dest: Path) -> Path:
        url = f"{self.base_url}/v1/art/jobs/{job_id}/assets/{filename}"
        headers = self._headers()
        headers.pop("Content-Type", None)
        req = urllib.request.Request(url, headers=headers, method="GET")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_s) as resp:
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(resp.read())
        except urllib.error.URLError as exc:
            raise RemoteClientError(f"GET {url} failed: {exc}") from exc
        return dest
