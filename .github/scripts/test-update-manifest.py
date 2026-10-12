"""更新清单与桌面发布契约的回归测试。"""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("generate-update-manifest.py")
SPEC = importlib.util.spec_from_file_location("update_manifest", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ManifestTests(unittest.TestCase):
    def fixtures(self, dist):
        record = dict(schema=1, channel="alpha", version="0.1.2", commit="a" * 40, build=123, attempt=2, label="alpha")
        for target, platform in MODULE.TARGETS.items():
            (dist / ("build-" + target + ".json")).write_text(json.dumps(dict(record, target=target)))
            base = "kitonyterms-alpha-" + platform
            (dist / (base + (".tar.gz" if "linux" in target else ".zip"))).write_bytes(b"package")
            if "windows" in target:
                (dist / (base + "-setup.exe")).write_bytes(b"installer")

    def test_complete_and_missing_packages(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            self.fixtures(dist)
            manifest = MODULE.generate(dist)
            self.assertEqual(len(manifest["targets"]), 6)
            self.assertEqual(manifest["targets"]["x86_64-pc-windows-msvc"]["installer"]["kind"], "installer")
            self.assertEqual(manifest["targets"]["aarch64-unknown-linux-gnu"]["archive"]["size"], 7)
            (dist / "kitonyterms-alpha-windows-x64-setup.exe").unlink()
            with self.assertRaises(ValueError):
                MODULE.generate(dist)

    def test_mixed_build_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            self.fixtures(dist)
            path = dist / "build-aarch64-apple-darwin.json"
            record = json.loads(path.read_text())
            record["build"] += 1
            path.write_text(json.dumps(record))
            with self.assertRaises(ValueError):
                MODULE.generate(dist)

    def test_tag_version_and_development(self):
        with tempfile.TemporaryDirectory() as temporary:
            cargo = Path(temporary) / "Cargo.toml"
            cargo.write_text('[workspace.package]\nversion = "0.1.2"\n')
            environment = dict(GITHUB_REF="refs/tags/v0.1.2", GITHUB_REF_NAME="v0.1.2", GITHUB_SHA="a" * 40, GITHUB_RUN_ID="12", GITHUB_RUN_ATTEMPT="1")
            self.assertEqual(MODULE.identity("auto", environment, cargo)["channel"], "stable")
            environment["GITHUB_REF"] = "refs/tags/v0.1.1"
            with self.assertRaises(ValueError):
                MODULE.identity("auto", environment, cargo)
            environment["GITHUB_REF"] = "refs/heads/main"
            self.assertEqual(MODULE.identity("auto", environment, cargo)["channel"], "development")
            self.assertEqual(MODULE.identity("alpha", environment, cargo)["label"], "alpha")

    def test_workflow_contract(self):
        for name, channel in (("release", "auto"), ("alpha", "alpha")):
            workflow = (SCRIPT.parent.parent / "workflows" / (name + ".yml")).read_text()
            self.assertIn("metadata --channel " + channel, workflow)
            self.assertIn("update_manifest:", workflow)
            self.assertIn("update-manifest.json", workflow)
            self.assertIn("AppId=KitonyTerms", workflow)
            self.assertIn("CloseApplications=no", workflow)
            self.assertIn("kitonyterms-installation.txt", workflow)
            for target in MODULE.TARGETS:
                self.assertIn("target: " + target, workflow)


if __name__ == "__main__":
    unittest.main()
