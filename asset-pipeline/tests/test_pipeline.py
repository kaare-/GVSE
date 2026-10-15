#!/usr/bin/env python3
"""Unit + optional Aseprite integration tests for gvse_art."""

from __future__ import annotations

import json
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from gvse_art import ase_writer, pngutil  # noqa: E402
from gvse_art.commands import (  # noqa: E402
    create_from_spec,
    export_source,
    import_png,
    import_remote_manifest,
    validate,
    which_aseprite,
)
from gvse_art.find_aseprite import find_aseprite  # noqa: E402
from gvse_art.remote_manifest import load_manifest  # noqa: E402
from gvse_art.spec import load_spec  # noqa: E402


ASE = find_aseprite()
HAS_ASEPRITE = ASE is not None


class SpecTests(unittest.TestCase):
    def test_load_fungal_spec(self) -> None:
        spec = load_spec(ROOT / "specs/fungal-organism.json")
        self.assertEqual(spec.width, 16)
        self.assertEqual(spec.height, 16)
        self.assertEqual(len(spec.frames), 4)
        self.assertTrue(spec.transparent_background)
        frames = spec.raster_frames()
        self.assertEqual(len(frames[0]), 16 * 16)
        # At least one opaque pixel in frame 0.
        self.assertTrue(any(p[3] > 0 for p in frames[0]))


class PngAndAseTests(unittest.TestCase):
    def test_png_roundtrip(self) -> None:
        pixels = [(255, 0, 0, 255), (0, 255, 0, 128), (0, 0, 255, 0), (1, 2, 3, 4)]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "t.png"
            pngutil.write_rgba_png(path, 2, 2, pixels)
            w, h, got = pngutil.read_rgba_png(path)
            self.assertEqual((w, h), (2, 2))
            self.assertEqual(got, pixels)

    def test_ase_header(self) -> None:
        pixels = [(10, 20, 30, 255)] * 4
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "t.aseprite"
            ase_writer.write_aseprite(path, 2, 2, [pixels, pixels], [100, 100])
            data = path.read_bytes()
            self.assertEqual(struct.unpack_from("<H", data, 4)[0], 0xA5E0)
            self.assertEqual(struct.unpack_from("<H", data, 6)[0], 2)
            self.assertEqual(struct.unpack_from("<H", data, 8)[0], 2)
            self.assertEqual(struct.unpack_from("<H", data, 10)[0], 2)


class CliCommandTests(unittest.TestCase):
    def test_which(self) -> None:
        info = which_aseprite()
        self.assertIn("available", info)
        self.assertEqual(info["available"], HAS_ASEPRITE)

    def test_create_offline_or_aseprite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            spec = {
                "name": "tiny",
                "width": 4,
                "height": 4,
                "transparent_background": True,
                "palette": {"a": "#112233"},
                "paths": {
                    "source_aseprite": str(tmp_path / "tiny.aseprite"),
                    "export_sheet": str(tmp_path / "tiny-sheet.png"),
                    "export_data": str(tmp_path / "tiny-sheet.json"),
                },
                "frames": [
                    {
                        "duration_ms": 80,
                        "pixels": [{"x": 1, "y": 1, "color": "#112233"}],
                    },
                    {
                        "duration_ms": 80,
                        "pixels": [{"x": 2, "y": 2, "color": "#112233"}],
                    },
                ],
            }
            spec_path = tmp_path / "tiny.json"
            spec_path.write_text(json.dumps(spec), encoding="utf-8")
            meta = create_from_spec(spec_path, force=True)
            self.assertTrue(Path(meta["source"]).is_file())
            self.assertTrue(Path(meta["sheet"]).is_file())
            report = validate(
                source=Path(meta["source"]),
                data=Path(meta["data"]),
                expect_width=4,
                expect_height=4,
                expect_min_frames=2,
            )
            self.assertTrue(report["ok"], report)

    def test_import_png_offline_or_aseprite(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            png = tmp_path / "in.png"
            pngutil.write_rgba_png(
                png, 3, 2, [(9, 8, 7, 255)] * 6
            )
            out = tmp_path / "out.aseprite"
            meta = import_png(png, out, force=True)
            self.assertTrue(out.is_file())
            self.assertIn("aseprite_available", meta)

    def test_remote_manifest_fixture(self) -> None:
        manifest_path = ROOT / "fixtures/remote-art-job/manifest.json"
        manifest = load_manifest(manifest_path)
        self.assertEqual(manifest.job_id, "local-fixture-fungal-001")
        self.assertIsNotNone(manifest.primary_png)
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            meta = import_remote_manifest(
                manifest_path,
                tmp_path / "stage",
                tmp_path / "from-remote.aseprite",
                force=True,
            )
            self.assertTrue(Path(meta["source"]).is_file())
            self.assertTrue((tmp_path / "stage" / "import-provenance.json").is_file())


class ExampleAssetTests(unittest.TestCase):
    def test_checked_in_example(self) -> None:
        source = ROOT / "examples/fungal-organism/source/fungal-organism.aseprite"
        data = ROOT / "examples/fungal-organism/export/fungal-organism-sheet.json"
        sheet = ROOT / "examples/fungal-organism/export/fungal-organism-sheet.png"
        self.assertTrue(source.is_file())
        self.assertTrue(data.is_file())
        self.assertTrue(sheet.is_file())
        report = validate(
            source=source,
            data=data,
            expect_width=16,
            expect_height=16,
            expect_min_frames=4,
        )
        self.assertTrue(report["ok"], report)


@unittest.skipUnless(HAS_ASEPRITE, "Aseprite not installed — CLI path untested")
class AsepriteIntegrationTests(unittest.TestCase):
    def test_export_via_cli(self) -> None:
        source = ROOT / "examples/fungal-organism/source/fungal-organism.aseprite"
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            sheet = tmp_path / "sheet.png"
            data = tmp_path / "sheet.json"
            # Prefer regenerating source with Lua when possible.
            spec = ROOT / "specs/fungal-organism.json"
            # Work on a copy so we do not mutate the tree mid-test.
            work_spec = tmp_path / "spec.json"
            payload = json.loads(spec.read_text(encoding="utf-8"))
            payload["paths"] = {
                "source_aseprite": str(tmp_path / "org.aseprite"),
                "export_sheet": str(sheet),
                "export_data": str(data),
            }
            work_spec.write_text(json.dumps(payload), encoding="utf-8")
            meta = create_from_spec(work_spec, force=True)
            self.assertTrue(meta["aseprite_available"])
            self.assertEqual(meta["mode"], "aseprite-lua")
            self.assertTrue(sheet.is_file())
            self.assertTrue(data.is_file())
            # Re-export explicitly.
            sheet2 = tmp_path / "sheet2.png"
            data2 = tmp_path / "sheet2.json"
            export_source(Path(meta["source"]), sheet2, data2)
            self.assertTrue(sheet2.is_file())
            report = validate(
                source=Path(meta["source"]),
                data=data2,
                expect_width=16,
                expect_height=16,
                expect_min_frames=4,
            )
            self.assertTrue(report["ok"], report)
            self.assertTrue(report["reports"][0]["aseprite_tested"])

    def test_import_png_lua(self) -> None:
        fixture = ROOT / "fixtures/remote-art-job/fungal-remote.png"
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "imp.aseprite"
            meta = import_png(fixture, out, force=True)
            self.assertTrue(meta["aseprite_available"])
            self.assertTrue(out.is_file())


def main() -> int:
    print(
        "Aseprite:",
        f"{ASE.path} ({ASE.version})" if ASE else "NOT INSTALLED — integration skipped",
    )
    suite = unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
