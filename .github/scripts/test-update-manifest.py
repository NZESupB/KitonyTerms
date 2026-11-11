"""Go/Wails桌面身份、全平台发布集合与版本契约回归。"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name+".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


META = load("generate-update-manifest")
FINAL = load("finalize-release")
TARGETS = {"aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"}
ENV = dict(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/dev", GITHUB_REF_NAME="dev", GITHUB_SHA="a"*40, GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="2")


class ReleaseTests(unittest.TestCase):
    def fixtures(self, dist):
        record = META.identity("stable", ENV)
        for target, platform in META.TARGETS.items():
            (dist / ("build-"+target+".json")).write_text(json.dumps(dict(record, target=target)), encoding="utf-8")
            info = {key: record[key] for key in ("channel", "version", "commit", "build", "attempt")}
            evidence = dict(identity=dict(info, target=target), runtimeIdentityVerified=True, dmgReadbackVerified=True, minimumSystem="15.0.0")
            (dist / ("verification-"+target+".json")).write_text(json.dumps(evidence), encoding="utf-8")
            suffixes = [".tar.gz"] if "linux" in target else [".dmg"] if "apple" in target else [".zip", "-setup.exe"]
            for suffix in suffixes:
                (dist / ("kitonyterms-"+record["label"]+"-"+platform+suffix)).write_bytes(b"public fixture")
        for platform, suffixes in (("android", [".apk", ".aab"]), ("ios", ["-unsigned.ipa"])):
            files = {}
            for suffix in suffixes:
                name = "kitonyterms-"+record["label"]+"-"+platform+"-aarch64"+suffix
                (dist / name).write_bytes(b"mobile fixture")
                files[name] = dict(size=14, sha256=hashlib.sha256(b"mobile fixture").hexdigest())
            evidence = {key: record[key] for key in ("version", "commit", "build", "attempt")}
            evidence.update(buildNumber=12345, files=files, signing="unsigned", signatureV2Verified=True, aabSignatureVerified=True, abi="arm64-v8a", pageAlignment=16384)
            (dist / ("verification-"+platform+".json")).write_text(json.dumps(evidence), encoding="utf-8")
        return record

    def test_native_targets_and_complete_inventory(self):
        self.assertEqual(set(META.TARGETS), TARGETS)
        with tempfile.TemporaryDirectory() as temporary, mock.patch.dict(os.environ, ENV):
            dist = Path(temporary)
            self.fixtures(dist)
            FINAL.finalize(dist)
            FINAL.finalize(dist, True)
            self.assertEqual(len(list(dist.iterdir())), 27)
            (dist / "extra.txt").write_text("unexpected")
            with self.assertRaises(ValueError): FINAL.finalize(dist, True)

    def test_missing_package_mixed_identity_and_bad_digest(self):
        for mutation in ("missing", "identity", "digest", "runtime", "minimum", "mobile"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary, mock.patch.dict(os.environ, ENV):
                dist = Path(temporary)
                record = self.fixtures(dist)
                if mutation == "missing":
                    (dist / ("kitonyterms-"+record["label"]+"-windows-x64-setup.exe")).unlink()
                elif mutation == "digest":
                    FINAL.finalize(dist)
                    (dist / ("kitonyterms-"+record["label"]+"-linux-x64.tar.gz")).write_bytes(b"tampered")
                else:
                    name = "build-aarch64-apple-darwin.json" if mutation == "identity" else "verification-ios.json" if mutation == "mobile" else "verification-aarch64-apple-darwin.json"
                    path = dist / name
                    data = json.loads(path.read_text(encoding="utf-8"))
                    key, value = {"identity": ("attempt", 3), "mobile": ("buildNumber", 2), "runtime": ("runtimeIdentityVerified", False), "minimum": ("minimumSystem", "14.0.0")}[mutation]
                    data[key] = value
                    path.write_text(json.dumps(data), encoding="utf-8")
                with self.assertRaises((ValueError, FileNotFoundError)): FINAL.finalize(dist, mutation == "digest")

    def test_tag_manual_branch_and_utf8_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "package.json"
            path.write_text(json.dumps(dict(version="0.1.2", description="发布版本"), ensure_ascii=False), encoding="utf-8")
            env = dict(ENV, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/tags/v0.1.2", GITHUB_REF_NAME="v0.1.2")
            self.assertEqual(META.identity("auto", env, path)["channel"], "stable")
            with self.assertRaises(ValueError): META.identity("stable", dict(env, GITHUB_REF_NAME="v0.1.1"), path)
            self.assertEqual(META.identity("stable", ENV, path)["label"], "v0.1.2")
            for change in (dict(GITHUB_SHA="bad"), dict(GITHUB_RUN_ATTEMPT="0"), dict(GITHUB_REF="refs/heads/main")):
                with self.assertRaises(ValueError): META.identity("stable", dict(ENV, **change), path)
            path.write_text('{"version":"01.1.2"}', encoding="utf-8")
            with self.assertRaises(ValueError): META.identity("stable", ENV, path)

    def test_workflow_and_no_project_rust_requirement(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertEqual(set(re.findall(r"target: (\S+)", workflow)), TARGETS)
        for command in ("cargo ", "rustc ", "dx ", "rust-toolchain", "rust-cache"):
            self.assertNotIn(command, workflow)
        self.assertIn("needs: [validate, build, mobile_android, mobile_ios]", workflow)
        self.assertIn("--latest --draft", workflow)
        self.assertIn("--draft=false --latest", workflow)
        self.assertNotIn("--prerelease --latest=false", workflow)
        for name in ("Cargo.toml", "Cargo.lock", "Dioxus.toml", "rust-toolchain.toml", "crates", "app/tools/interop"):
            self.assertFalse((ROOT / name).exists(), name)
        version = json.loads((ROOT / "app/frontend/package.json").read_text(encoding="utf-8"))["version"]
        for name in ("app/build/config.yml", "app/build/ios/Info.plist", "app/build/android/app/build.gradle", "app/internal/update/metadata.go"):
            self.assertIn(version, (ROOT / name).read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
