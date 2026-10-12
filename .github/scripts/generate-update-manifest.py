#!/usr/bin/env python3
"""生成桌面构建身份及更新清单；只使用 Python 标准库。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re

TARGETS = {
    "x86_64-unknown-linux-gnu": "linux-x64",
    "aarch64-unknown-linux-gnu": "linux-aarch64",
    "aarch64-apple-darwin": "macos-aarch64",
    "x86_64-apple-darwin": "macos-x64",
    "x86_64-pc-windows-msvc": "windows-x64",
    "aarch64-pc-windows-msvc": "windows-aarch64",
}
IDENTITY_KEYS = ("schema", "channel", "version", "commit", "build", "attempt", "label")
MAX_PACKAGE = 512 * 1024 * 1024


def identity(channel, environment=None, cargo=Path("Cargo.toml")):
    environment = os.environ if environment is None else environment
    match = re.search(r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"$', cargo.read_text(), re.M)
    if not match:
        raise ValueError("无法读取 workspace 版本")
    version = match.group(1)
    ref = environment.get("GITHUB_REF", "")
    label = environment.get("GITHUB_REF_NAME", "dev")
    if channel == "auto":
        channel = "stable" if ref.startswith("refs/tags/v") else "development"
    if channel == "stable" and (ref != "refs/tags/v" + version or label != "v" + version):
        raise ValueError("正式 tag 必须与 Cargo.toml 版本一致")
    if channel == "alpha":
        label = "alpha"
    commit = environment.get("GITHUB_SHA", "")
    if not re.fullmatch(r"[a-fA-F0-9]{40}", commit):
        raise ValueError("缺少完整 Git commit")
    build = int(environment.get("GITHUB_RUN_ID", "0"))
    attempt = int(environment.get("GITHUB_RUN_ATTEMPT", "0"))
    if build <= 0 or attempt <= 0:
        raise ValueError("缺少有效构建编号")
    if not re.fullmatch(r"[a-zA-Z0-9_.-]+", label):
        raise ValueError("产物版本名称无效")
    return dict(schema=1, channel=channel, version=version, commit=commit, build=build, attempt=attempt, label=label)


def package(dist, name, kind):
    path = dist / name
    if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= MAX_PACKAGE:
        raise ValueError("缺少或无效更新产物: " + name)
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            digest.update(block)
    return dict(name=name, size=path.stat().st_size, sha256=digest.hexdigest(), kind=kind)


def generate(dist):
    records = {}
    for target in TARGETS:
        path = dist / ("build-" + target + ".json")
        if path.is_symlink():
            raise ValueError("构建记录不能是链接")
        record = json.loads(path.read_text())
        if record.get("target") != target:
            raise ValueError("构建 target 不一致")
        records[target] = record
    first = next(iter(records.values()))
    for record in records.values():
        if any(record[key] != first[key] for key in IDENTITY_KEYS):
            raise ValueError("桌面构建身份不一致")
    if first["schema"] != 1 or first["channel"] not in ("stable", "alpha", "development"):
        raise ValueError("构建协议无效")
    if not re.fullmatch(r"[a-fA-F0-9]{40}", first["commit"]) or first["build"] <= 0 or first["attempt"] <= 0:
        raise ValueError("构建身份无效")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", first["version"]):
        raise ValueError("构建版本无效")
    if first["channel"] == "stable" and first["label"] != "v" + first["version"]:
        raise ValueError("正式版本名称不一致")
    if first["channel"] == "alpha" and first["label"] != "alpha":
        raise ValueError("Alpha 版本名称不一致")
    result = {key: first[key] for key in IDENTITY_KEYS if key != "label"}
    result["targets"] = {}
    for target, platform in TARGETS.items():
        base = "kitonyterms-" + first["label"] + "-" + platform
        linux = "linux" in target
        archive = package(dist, base + (".tar.gz" if linux else ".zip"), "tar_gz" if linux else "zip")
        installer = package(dist, base + "-setup.exe", "installer") if "windows" in target else None
        result["targets"][target] = dict(archive=archive, installer=installer)
    return result


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    meta = sub.add_parser("metadata")
    meta.add_argument("--channel", choices=("auto", "alpha", "stable", "development"), required=True)
    meta.add_argument("--target", choices=tuple(TARGETS), required=True)
    meta.add_argument("--dist", type=Path, default=Path("dist"))
    meta.add_argument("--github-env", type=Path)
    build = sub.add_parser("generate")
    build.add_argument("--dist", type=Path, default=Path("dist"))
    build.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    if args.command == "metadata":
        record = identity(args.channel)
        record["target"] = args.target
        args.dist.mkdir(parents=True, exist_ok=True)
        (args.dist / ("build-" + args.target + ".json")).write_text(json.dumps(record, indent=2) + "\n")
        if args.github_env:
            with args.github_env.open("a") as stream:
                for key in ("channel", "commit", "build", "attempt", "version"):
                    stream.write("KT_RELEASE_" + key.upper() + "=" + str(record[key]) + "\n")
    else:
        result = generate(args.dist)
        path = args.dist / "update-manifest.json"
        if args.verify:
            if json.loads(path.read_text()) != result:
                raise ValueError("更新清单与实际构建产物不一致")
        else:
            path.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
