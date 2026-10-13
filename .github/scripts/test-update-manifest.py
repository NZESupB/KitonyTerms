"""更新清单与桌面发布契约的回归测试。"""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("generate-update-manifest.py")
SPEC = importlib.util.spec_from_file_location("update_manifest", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

DESKTOP_TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
}
NODE24_ACTIONS = {
    "actions/checkout@v5",
    "actions/upload-artifact@v6",
    "actions/download-artifact@v8",
    "actions/setup-java@v5",
    "android-actions/setup-android@v4",
    "Swatinem/rust-cache@v2",
    "softprops/action-gh-release@v3",
}


class ManifestTests(unittest.TestCase):
    def fixtures(self, dist):
        record = dict(schema=1, channel="alpha", version="0.1.2", commit="a" * 40, build=123, attempt=2, label="alpha")
        for target, platform in MODULE.TARGETS.items():
            (dist / ("build-" + target + ".json")).write_text(json.dumps(dict(record, target=target)), encoding="utf-8")
            base = "kitonyterms-alpha-" + platform
            suffix = ".tar.gz" if "linux" in target else ".dmg" if "apple" in target else ".zip"
            (dist / (base + suffix)).write_bytes(b"package")
            if "windows" in target:
                (dist / (base + "-setup.exe")).write_bytes(b"installer")

    def test_complete_and_missing_packages(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            self.fixtures(dist)
            manifest = MODULE.generate(dist)
            self.assertEqual(set(MODULE.TARGETS), DESKTOP_TARGETS)
            self.assertEqual(set(manifest["targets"]), DESKTOP_TARGETS)
            macos = manifest["targets"]["aarch64-apple-darwin"]
            self.assertEqual(macos["archive"]["kind"], "dmg")
            self.assertEqual(macos["archive"]["name"], "kitonyterms-alpha-macos-aarch64.dmg")
            self.assertIsNone(macos["installer"])
            self.assertEqual(manifest["targets"]["x86_64-pc-windows-msvc"]["archive"]["kind"], "zip")
            self.assertEqual(manifest["targets"]["x86_64-pc-windows-msvc"]["installer"]["kind"], "installer")
            self.assertEqual(manifest["targets"]["aarch64-unknown-linux-gnu"]["archive"]["size"], 7)
            (dist / "kitonyterms-alpha-windows-x64-setup.exe").unlink()
            with self.assertRaises(ValueError):
                MODULE.generate(dist)

    def test_macos_requires_dmg_instead_of_zip(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            self.fixtures(dist)
            (dist / "kitonyterms-alpha-macos-aarch64.dmg").rename(dist / "kitonyterms-alpha-macos-aarch64.zip")
            with self.assertRaises(ValueError):
                MODULE.generate(dist)

    def test_mixed_build_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            self.fixtures(dist)
            path = dist / "build-aarch64-apple-darwin.json"
            record = json.loads(path.read_text(encoding="utf-8"))
            record["build"] += 1
            path.write_text(json.dumps(record), encoding="utf-8")
            with self.assertRaises(ValueError):
                MODULE.generate(dist)

    def test_tag_version_and_development(self):
        with tempfile.TemporaryDirectory() as temporary:
            cargo = Path(temporary) / "Cargo.toml"
            cargo.write_text('[workspace.package]\nversion = "0.1.2"\n', encoding="utf-8")
            environment = dict(GITHUB_REF="refs/tags/v0.1.2", GITHUB_REF_NAME="v0.1.2", GITHUB_SHA="a" * 40, GITHUB_RUN_ID="12", GITHUB_RUN_ATTEMPT="1")
            self.assertEqual(MODULE.identity("auto", environment, cargo)["channel"], "stable")
            environment["GITHUB_REF"] = "refs/tags/v0.1.1"
            with self.assertRaises(ValueError):
                MODULE.identity("auto", environment, cargo)
            environment["GITHUB_REF"] = "refs/heads/main"
            self.assertEqual(MODULE.identity("auto", environment, cargo)["channel"], "development")
            self.assertEqual(MODULE.identity("alpha", environment, cargo)["label"], "alpha")

    def test_utf8_input_with_windows_default_encoding(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo = root / "Cargo.toml"
            cargo.write_text('# 发布构建中文注释\n[workspace.package]\nversion = "0.1.2"\n', encoding="utf-8")
            environment = dict(GITHUB_REF="refs/heads/dev", GITHUB_REF_NAME="dev", GITHUB_SHA="a" * 40, GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="2")
            self.fixtures(root)
            for target in MODULE.TARGETS:
                path = root / ("build-" + target + ".json")
                record = json.loads(path.read_text(encoding="utf-8"))
                record["note"] = "发布构建"
                path.write_text(json.dumps(record, ensure_ascii=False), encoding="utf-8")
            original_read_text = Path.read_text

            def windows_read_text(path, encoding=None, errors=None, **kwargs):
                if encoding in (None, "locale"):
                    encoding = "cp1252"
                return original_read_text(path, encoding=encoding, errors=errors, **kwargs)

            with mock.patch.object(Path, "read_text", windows_read_text):
                with self.assertRaises(UnicodeDecodeError):
                    cargo.read_text()
                self.assertEqual(MODULE.identity("auto", environment, cargo)["version"], "0.1.2")
                self.assertEqual(set(MODULE.generate(root)["targets"]), DESKTOP_TARGETS)

    def test_metadata_command_for_supported_targets(self):
        for target in MODULE.TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "Cargo.toml").write_text('# 发布构建中文注释\n[workspace.package]\nversion = "0.1.2"\n', encoding="utf-8")
                github_env = root / "github-env"
                prefix = "EXISTING=保留配置\n".encode("utf-8")
                github_env.write_bytes(prefix)
                expected = dict(schema=1, channel="development", version="0.1.2", commit="a" * 40, build=123, attempt=2, label="dev", target=target)
                environment = dict(os.environ, GITHUB_REF="refs/heads/dev", GITHUB_REF_NAME="dev", GITHUB_SHA=expected["commit"], GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="2")
                appended = "".join("KT_RELEASE_" + key.upper() + "=" + str(expected[key]) + "\n" for key in ("channel", "commit", "build", "attempt", "version")).encode("utf-8")
                for count in (1, 2):
                    subprocess.run([sys.executable, "-B", str(SCRIPT.resolve()), "metadata", "--channel", "auto", "--target", target, "--dist", "dist", "--github-env", str(github_env)], cwd=root, env=environment, check=True, capture_output=True, encoding="utf-8", timeout=15)
                    record = json.loads((root / "dist" / ("build-" + target + ".json")).read_text(encoding="utf-8"))
                    self.assertEqual(record, expected)
                    contents = github_env.read_bytes()
                    self.assertEqual(contents, prefix + appended * count)
                    self.assertNotIn(b"\r", contents)
                    self.assertEqual(contents.decode("utf-8"), (prefix + appended * count).decode("utf-8"))

    def test_removed_targets_are_rejected(self):
        for target in ("x86_64-apple-darwin", "aarch64-pc-windows-msvc"):
            with self.subTest(target=target):
                result = subprocess.run([sys.executable, "-B", str(SCRIPT), "metadata", "--channel", "auto", "--target", target], capture_output=True, encoding="utf-8", timeout=15)
                self.assertEqual(result.returncode, 2)
                self.assertIn("invalid choice", result.stderr)

    def test_workflow_contract(self):
        for name, channel in (("release", "auto"), ("alpha", "alpha")):
            workflow = (SCRIPT.parent.parent / "workflows" / (name + ".yml")).read_text(encoding="utf-8")
            self.assertIn("metadata --channel " + channel, workflow)
            self.assertIn("python3 -B .github/scripts/test-update-manifest.py", workflow)
            self.assertLess(workflow.index("python3 -B .github/scripts/test-update-manifest.py"), workflow.index("metadata --channel " + channel))
            self.assertIn("update_manifest:", workflow)
            self.assertIn("update-manifest.json", workflow)
            self.assertIn("AppId=KitonyTerms", workflow)
            self.assertIn("CloseApplications=no", workflow)
            self.assertIn("kitonyterms-installation.txt", workflow)
            matrix = workflow.split("      matrix:\n", 1)[1].split("    steps:\n", 1)[0]
            self.assertEqual(set(re.findall(r"target: (\S+)", matrix)), DESKTOP_TARGETS)
            self.assertEqual(len(re.findall(r"target: (\S+)", matrix)), 4)
            self.assertNotIn("macos-x64", workflow)
            self.assertNotIn("windows-aarch64", workflow)
            macos = workflow.split("- name: Package (macOS)", 1)[1].split("\n      # Windows:", 1)[0]
            self.assertIn('hdiutil create -volname "KitonyTerms"', macos)
            self.assertIn('"dist/${base}.dmg"', macos)
            self.assertNotIn(".zip", macos)
            self.assertNotIn("ditto", macos)
            windows = workflow.split("- name: Package (Windows)", 1)[1].split("- name: Upload build artifacts", 1)[0]
            self.assertIn('Compress-Archive -Path "staging\\*" -DestinationPath "dist\\$base.zip"', windows)
            self.assertIn("OutputBaseFilename=$base-setup", windows)
            self.assertLess(windows.index("Compress-Archive"), windows.index("kitonyterms-installation.txt"))
            actions = set(re.findall(r"uses: (\S+)", workflow))
            self.assertEqual(actions, NODE24_ACTIONS | {"dtolnay/rust-toolchain@stable"})


if __name__ == "__main__":
    unittest.main()
