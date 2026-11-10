#!/usr/bin/env python3
"""使用Go/Wails生产工程，在原生runner构建并验证桌面发布包。"""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / "app"
SPEC = importlib.util.spec_from_file_location("manifest", Path(__file__).with_name("generate-update-manifest.py"))
META = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(META)
HOSTS = {
    "aarch64-apple-darwin": ("Darwin", "arm64"),
    "x86_64-apple-darwin": ("Darwin", "amd64"),
    "x86_64-pc-windows-msvc": ("Windows", "amd64"),
    "aarch64-pc-windows-msvc": ("Windows", "arm64"),
    "x86_64-unknown-linux-gnu": ("Linux", "amd64"),
    "aarch64-unknown-linux-gnu": ("Linux", "arm64"),
}


def run(args, cwd=APP, env=None, timeout=600, capture=False):
    return subprocess.run(list(map(str, args)), cwd=cwd, env=env, check=True, timeout=timeout,
                          capture_output=capture, text=True)


def build(target):
    machine = {"x86_64": "amd64", "AMD64": "amd64", "aarch64": "arm64", "ARM64": "arm64", "arm64": "arm64"}.get(platform.machine())
    if HOSTS[target] != (platform.system(), machine):
        raise ValueError("桌面打包必须在对应架构的原生runner执行")
    record = dict(META.identity("stable"), target=target)
    info = {key: record[key] for key in ("channel", "version", "commit", "build", "attempt", "target")}
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    record_path = dist / f"build-{target}.json"
    if record_path.exists():
        raise ValueError("拒绝覆盖已有构建记录")
    record_path.write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
    work = Path(tempfile.mkdtemp(prefix="kitonyterms-release-", dir=os.environ.get("RUNNER_TEMP")))
    identity_path = work / "identity.json"
    identity_path.write_text(json.dumps(info)+"\n", encoding="utf-8")
    encoded = base64.urlsafe_b64encode(json.dumps(info, separators=(",", ":")).encode()).decode().rstrip("=")
    base = "kitonyterms-" + record["label"] + "-" + META.TARGETS[target]
    env = dict(os.environ)
    evidence = dict(identity=info, guiExecutionVerified=False, formalSigningVerified=False)
    if platform.system() == "Darwin":
        candidate = work / "candidate"
        run(["go", "run", "-mod=readonly", "./tools/macos-package", "build", "--project", APP, "--identity", identity_path, "--output", candidate], env=env)
        evidence.update(json.loads((candidate / "candidate-build.json").read_text(encoding="utf-8")))
        shutil.copyfile(candidate / "kitonyterms-candidate-macos-arm64.dmg", dist / (base+".dmg"))
    elif platform.system() == "Windows":
        env["CGO_ENABLED"] = "0"
        candidate, prepared = work / "candidate", work / "prepared"
        prefix = ["go", "run", "-mod=readonly", "./tools/windows-package"]
        run(prefix + ["build", "--project", APP, "--identity", identity_path, "--output", candidate], env=env)
        run(prefix + ["prepare", "--binary", candidate / "kitonyterms.exe", "--identity", identity_path, "--output", prepared], env=env)
        iscc = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Inno Setup 6/ISCC.exe"
        run(prefix + ["compile", "--prepared", prepared, "--iscc", iscc], env=env)
        for suffix in (".zip", "-setup.exe"):
            shutil.copyfile(prepared / (base+suffix), dist / (base+suffix))
        evidence.update(json.loads((candidate / "candidate-build.json").read_text(encoding="utf-8")))
        evidence["runtimeIdentityVerified"] = True
    else:
        env["CGO_ENABLED"] = "1"
        payload = work / base
        (payload / "bin").mkdir(parents=True)
        binary = payload / "kitonyterms"
        run(["go", "build", "-mod=readonly", "-tags=production", "-trimpath", "-buildvcs=false", "-ldflags=-w -s -X kitonyterms/internal/update.embeddedIdentity="+encoded, "-o", binary, "."], env=env)
        probe = work / "must-not-be-created"
        probe_env = dict(env, KITONYTERMS_DATA_DIR=str(probe))
        actual = json.loads(run([binary, "--update-info"], env=probe_env, capture=True, timeout=15).stdout)
        if actual != info or probe.exists():
            raise ValueError("Linux只读构建身份不符或创建了业务数据")
        shutil.copy2(binary, payload / "bin/kitonyterms")
        shutil.copytree(APP / "build/linux/release/share", payload / "share")
        with tarfile.open(dist / (base+".tar.gz"), "w:gz") as archive:
            archive.add(payload, arcname=base)
        evidence["runtimeIdentityVerified"] = True
    (dist / f"verification-{target}.json").write_text(json.dumps(evidence, indent=2)+"\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=HOSTS)
    try:
        build(parser.parse_args().target)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"桌面发布构建失败：{error}", file=sys.stderr)
        sys.exit(1)
