#!/usr/bin/env python3
"""核对一次构建的完整资产集合、身份、移动包记录和更新清单。"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate-update-manifest.py")
SPEC = importlib.util.spec_from_file_location("meta", SCRIPT)
META = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(META)


def finalize(dist, verify=False):
    identity = META.identity("stable")
    manifest = META.generate(dist)
    for key in ("schema", "channel", "version", "commit", "build", "attempt"):
        if manifest[key] != identity[key]:
            raise ValueError("产物不属于当前提交/run/attempt")
    expected = set()
    for target, packages in manifest["targets"].items():
        expected.update((f"build-{target}.json", f"verification-{target}.json"))
        for package in packages.values():
            if package:
                expected.add(package["name"])
        record = json.loads((dist / f"verification-{target}.json").read_text(encoding="utf-8"))
        wanted = {key: identity[key] for key in ("channel", "version", "commit", "build", "attempt")}
        wanted["target"] = target
        if record.get("identity") != wanted or record.get("runtimeIdentityVerified") is not True:
            raise ValueError("桌面产物未确认实际运行身份：" + target)
        if "apple" in target and record.get("dmgReadbackVerified") is not True:
            raise ValueError("macOS镜像未实际回读")
        if "apple" in target and record.get("minimumSystem") != "15.0.0":
            raise ValueError("macOS最低系统不符")
    prefix = "kitonyterms-" + identity["label"]
    mobile_files = {
        "android": {prefix+"-android-aarch64.apk", prefix+"-android-aarch64.aab"},
        "ios": {prefix+"-ios-aarch64-unsigned.ipa"},
    }
    mobile_build = None
    for platform, names in mobile_files.items():
        name = "verification-"+platform+".json"
        expected.add(name)
        expected.update(names)
        record = json.loads((dist / name).read_text(encoding="utf-8"))
        for key in ("version", "commit", "build", "attempt"):
            if record[key] != identity[key]:
                raise ValueError("移动产物身份不匹配")
        number = record["buildNumber"]
        if type(number) is not int or not 1 <= number <= 2100000000 or mobile_build is not None and mobile_build != number:
            raise ValueError("移动构建号不一致")
        mobile_build = number
        if set(record["files"]) != names:
            raise ValueError("移动产物集合不符")
        for filename in names:
            actual = META.package(dist, filename, "mobile")
            if record["files"][filename] != {key: actual[key] for key in ("size", "sha256")}:
                raise ValueError("移动产物摘要或大小不匹配")
        if platform == "android" and (record.get("signatureV2Verified") is not True or record.get("aabSignatureVerified") is not True or record.get("abi") != "arm64-v8a" or record.get("pageAlignment") != 16384):
            raise ValueError("Android签名/ABI/对齐记录无效")
        if platform == "ios" and record.get("signing") != "unsigned":
            raise ValueError("iOS签名范围不符")
    actual = {path.name for path in dist.iterdir()}
    final_names = expected | {"update-manifest.json", "SHA256SUMS"}
    if actual != (final_names if verify else expected):
        raise ValueError("Release资产集合不符："+str(sorted(actual ^ (final_names if verify else expected))))
    manifest_text = json.dumps(manifest, indent=2)+"\n"
    if verify:
        if (dist / "update-manifest.json").read_text(encoding="utf-8") != manifest_text:
            raise ValueError("更新清单与实际产物不匹配")
    else:
        (dist / "update-manifest.json").write_text(manifest_text, encoding="utf-8")
    checksums = "".join(hashlib.sha256((dist / name).read_bytes()).hexdigest()+"  "+name+"\n" for name in sorted(expected | {"update-manifest.json"}))
    if verify:
        if (dist / "SHA256SUMS").read_text(encoding="utf-8") != checksums:
            raise ValueError("SHA256SUMS与实际产物不匹配")
    else:
        (dist / "SHA256SUMS").write_text(checksums, encoding="utf-8")
    print(f"完整{len(final_names)}项发布资产、身份、摘要与更新清单校验通过")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    finalize(args.dist, args.verify)
