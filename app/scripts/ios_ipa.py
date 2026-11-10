#!/usr/bin/env python3
"""构建设备版无签名IPA；打包不要求开发者账号，设备安装另需侧载签名。"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import shlex
import struct
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MAX_FILE = 256 << 20
MAX_TOTAL = 512 << 20


def require(condition, message):
    if not condition:
        raise ValueError(message)


def build_number(value):
    require(re.fullmatch(r"[1-9][0-9]{0,9}", value or "") is not None,
            "MOBILE_BUILD_NUMBER必须为正整数")
    require(int(value) <= 2100000000, "MOBILE_BUILD_NUMBER不能超过2100000000")
    return value


def run(arguments, *, env=None, capture=False):
    result = subprocess.run([str(a) for a in arguments], cwd=ROOT, env=env,
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.PIPE if capture else None,
                            timeout=600, check=False)
    require(result.returncode == 0, f"工具执行失败：{arguments[0]}（退出{result.returncode}）")
    return (result.stdout + result.stderr).decode("utf-8").strip() if capture else ""


def preflight(number):
    build_number(number)
    require(sys.platform == "darwin", "iOS设备构建需要macOS与完整Xcode")
    for tool in ("xcrun", "go", "wails3", "npm", "codesign"):
        require(shutil.which(tool), f"缺少构建工具：{tool}")
    # 在创建输出、生成代码或构建前确认SDK；不自动安装Xcode或申请账号。
    sdk_result = subprocess.run(["xcrun", "--sdk", "iphoneos", "--show-sdk-path"],
                                capture_output=True, text=True, timeout=20)
    require(sdk_result.returncode == 0 and Path(sdk_result.stdout.strip()).is_dir(),
            "缺少iPhoneOS SDK；请先配置完整Xcode。无签名IPA不要求开发者账号，但仍需SDK链接")
    for tool in ("clang", "actool", "ibtool"):
        run(["xcrun", "--sdk", "iphoneos", "--find", tool], capture=True)
    require(run(["wails3", "version"], capture=True) == "v3.0.0", "Wails CLI必须固定v3.0.0")
    require(run(["go", "list", "-mod=readonly", "-m", "-f", "{{.Version}}",
                 "github.com/wailsapp/wails/v3"], capture=True) == "v3.0.0",
            "Wails Go模块必须固定v3.0.0")
    declared = json.loads((ROOT / "frontend/package.json").read_text())
    installed = json.loads((ROOT / "frontend/node_modules/@wailsio/runtime/package.json").read_text())
    require(declared["dependencies"]["@wailsio/runtime"] == installed["version"] == "3.0.0",
            "前端Wails runtime必须固定3.0.0，且已安装依赖")
    return sdk_result.stdout.strip()


def device_binary(data, minimum):
    require(32 <= len(data) <= MAX_FILE, "设备程序体积无效")
    magic, cpu, _, kind, count, size, _, _ = struct.unpack_from("<8I", data)
    require(magic == 0xFEEDFACF and cpu == 0x0100000C and kind == 2,
            "IPA必须包含单架构arm64 Mach-O可执行文件")
    require(0 < count <= 1024 and 8 * count <= size <= (1 << 20) and 32 + size <= len(data),
            "Mach-O加载命令越界")
    offset, platforms = 32, []
    for _ in range(count):
        require(offset + 8 <= 32 + size, "Mach-O命令头越界")
        command, length = struct.unpack_from("<II", data, offset)
        require(length >= 8 and length % 8 == 0 and offset + length <= 32 + size,
                "Mach-O命令长度无效")
        if command == 0x32:
            require(length >= 24, "缺少设备平台声明")
            platform, version, sdk = struct.unpack_from("<III", data, offset + 8)
            platforms.append((platform, version, sdk))
        require(command not in (0x24, 0x25, 0x1D), "拒绝旧平台声明或残留代码签名")
        offset += length
    expected = minimum[0] << 16 | minimum[1] << 8 | minimum[2]
    require(offset == 32 + size and len(platforms) == 1 and platforms[0][0] == 2
            and platforms[0][1] == expected and platforms[0][2] >= expected,
            "必须为iPhoneOS真机程序，最低版本需与plist一致；不能打包模拟器或macOS程序")


def app_files(app, number):
    require(app.suffix == ".app" and app.is_dir() and not app.is_symlink(), "APP必须为真实.app目录")
    files, total = {}, 0
    for directory, directories, names in os.walk(app, followlinks=False):
        for name in directories + names:
            path = Path(directory) / name
            require(not path.is_symlink(), "无签名IPA不接受链接")
            require(name not in ("_CodeSignature", "embedded.mobileprovision"), "无签名IPA不能包含签名资源或profile")
            require(path.is_dir() or path.is_file(), "IPA包含非普通文件")
        for name in names:
            path = Path(directory) / name
            require(path.stat().st_size <= MAX_FILE, "IPA单文件超过限额")
            total += path.stat().st_size
            require(total <= MAX_TOTAL and len(files) < 5000, "IPA输入超过限额")
            files[path.relative_to(app).as_posix()] = path
    require("Info.plist" in files and files["Info.plist"].stat().st_size <= (1 << 20), "缺少或过大的Info.plist")
    info = plistlib.loads(files["Info.plist"].read_bytes())
    require(info.get("CFBundleIdentifier") == "com.kitonyterms.app" and info.get("CFBundlePackageType") == "APPL",
            "应用身份不符")
    require(info.get("CFBundleVersion") == number, "IPA构建号不符")
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", info.get("CFBundleShortVersionString", "")), "产品版本必须为三段数字")
    require(info.get("CFBundleSupportedPlatforms") == ["iPhoneOS"] and info.get("LSRequiresIPhoneOS") is True
            and info.get("UIDeviceFamily") == [1, 2], "缺少iPhoneOS设备声明")
    for key in ("NSCameraUsageDescription", "NSLocalNetworkUsageDescription"):
        require(isinstance(info.get(key), str) and info[key].strip(), "缺少移动权限用途说明")
    require((app / "LaunchScreen.storyboardc").is_dir()
            and any(name.startswith("LaunchScreen.storyboardc/") and path.stat().st_size > 0
                    for name, path in files.items())
            and "Assets.car" in files and files["Assets.car"].stat().st_size > 0,
            "缺少已编译的启动屏幕或图标资源")
    version = info.get("MinimumOSVersion", "").split(".")
    require(1 <= len(version) <= 3 and all(p.isdigit() for p in version), "最低iOS版本无效")
    minimum = tuple(map(int, version)) + (0,) * (3 - len(version))
    require(minimum >= (15, 0, 0) and minimum[0] <= 65535 and all(p <= 255 for p in minimum[1:]), "最低iOS版本无效")
    executable = info.get("CFBundleExecutable")
    require(executable == "kitonyterms" and executable in files and os.access(files[executable], os.X_OK), "缺少可执行主程序")
    device_binary(files[executable].read_bytes(), minimum)
    return files


def package(app, output, number):
    files = app_files(app, build_number(number))
    require(output.suffix == ".ipa" and output.parent.is_dir() and not output.exists(), "IPA输出必须是不存在的.ipa文件")
    require(app.resolve() not in output.resolve().parents, "IPA输出不能位于输入.app内")
    prefix = f"Payload/{app.name}/"
    with output.open("xb") as destination:
        os.chmod(output, 0o600)
        with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, path in sorted(files.items()):
                archive.write(path, prefix + name)
    with zipfile.ZipFile(output) as archive:
        require(set(archive.namelist()) == {prefix + name for name in files}, "IPA条目集合不符")
        for name, path in files.items():
            entry = archive.getinfo(prefix + name)
            require(entry.file_size == path.stat().st_size
                    and hashlib.sha256(archive.read(entry)).digest() == hashlib.sha256(path.read_bytes()).digest(), "IPA回读内容不符")
            if name == "kitonyterms":
                require(stat.S_IMODE(entry.external_attr >> 16) & 0o111, "IPA丢失执行权限")
    print(json.dumps({"ipa": str(output), "bytes": output.stat().st_size,
                      "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
                      "signing": "unsigned", "deviceExecutionVerified": False}, ensure_ascii=False))


def build(output, number):
    sdk = preflight(number)
    require(output.is_absolute() and not output.exists(), "OUTPUT必须为不存在的绝对目录")
    require(output.parent.is_dir(), "OUTPUT父目录必须存在")
    output.mkdir(mode=0o700)
    work = output / "build"
    work.mkdir(mode=0o700)
    run(["wails3", "generate", "bindings", "-f", "-mod=readonly", "-ts", "-i"])
    run(["npm", "--prefix", "frontend", "run", "build"])
    run(["wails3", "ios", "overlay:gen", "-out", work / "overlay.json", "-config", "build/config.yml"])
    run(["wails3", "ios", "xcode:gen", "-outdir", work / "xcode", "-config", "build/config.yml"])
    env = os.environ.copy()
    target_flags = ["-isysroot", sdk, "-target", "arm64-apple-ios15.0"]
    env.update(GOOS="ios", GOARCH="arm64", CGO_ENABLED="1", CC="/usr/bin/clang", CXX="/usr/bin/clang++",
               CGO_CFLAGS=shlex.join(target_flags + ["-miphoneos-version-min=15.0"]),
               CGO_CXXFLAGS=shlex.join(target_flags + ["-miphoneos-version-min=15.0"]),
               CGO_LDFLAGS=shlex.join(target_flags))
    run(["go", "build", "-mod=readonly", "-buildmode=c-archive", "-overlay", work / "overlay.json",
         "-tags=production,ios", "-trimpath", "-buildvcs=false", "-ldflags=-w -s", "-o", work / "kitonyterms.a", "."], env=env)
    app = output / "KitonyTerms.app"
    app.mkdir(mode=0o700)
    executable = app / "kitonyterms"
    arguments = ["xcrun", "--sdk", "iphoneos", "clang", "-target", "arm64-apple-ios15.0", "-isysroot", sdk]
    for framework in ("Foundation", "UIKit", "WebKit", "Security", "CoreFoundation", "UniformTypeIdentifiers",
                      "LocalAuthentication", "UserNotifications", "AVFoundation", "CoreLocation", "CoreMotion", "SystemConfiguration"):
        arguments += ["-framework", framework]
    run(arguments + ["-lresolv", "-o", executable, work / "xcode/main/main.m", "-Xlinker", "-force_load", "-Xlinker", work / "kitonyterms.a"])
    signature = subprocess.run(["codesign", "-d", str(executable)], capture_output=True, timeout=20)
    if signature.returncode == 0:
        run(["codesign", "--remove-signature", executable])
    info = plistlib.loads((ROOT / "build/ios/Info.plist").read_bytes())
    info.update(CFBundleVersion=number, CFBundleSupportedPlatforms=["iPhoneOS"], UIDeviceFamily=[1, 2])
    assets_plist = work / "assets.plist"
    run(["xcrun", "--sdk", "iphoneos", "actool", "--compile", app, "--app-icon", "AppIcon",
         "--platform", "iphoneos", "--minimum-deployment-target", "15.0", "--product-type", "com.apple.product-type.application",
         "--target-device", "iphone", "--target-device", "ipad", "--output-partial-info-plist", assets_plist,
         work / "xcode/main/Assets.xcassets"])
    info.update(plistlib.loads(assets_plist.read_bytes()))
    (app / "Info.plist").write_bytes(plistlib.dumps(info))
    run(["xcrun", "--sdk", "iphoneos", "ibtool", "--compile", app / "LaunchScreen.storyboardc",
         "--minimum-deployment-target", "15.0", work / "xcode/main/LaunchScreen.storyboard"])
    package(app, output / "kitonyterms-unsigned.ipa", number)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("preflight", "build", "pack"))
    parser.add_argument("--build-number", default=os.environ.get("MOBILE_BUILD_NUMBER", ""))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--app", type=Path)
    args = parser.parse_args()
    if args.command == "preflight":
        preflight(args.build_number)
        print("iOS设备构建前置条件通过；无需开发者账号")
    elif args.command == "build":
        number = build_number(args.build_number)
        output = args.output or Path(os.environ.get("KITONYTERMS_IOS_IPA_OUTPUT") or ROOT / "bin" / f"ios-device-{number}")
        build(output, number)
    else:
        require(args.app is not None and args.output is not None, "pack需要--app和--output")
        package(args.app, args.output, args.build_number)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError, zipfile.BadZipFile, plistlib.InvalidFileException) as error:
        print(f"iOS IPA失败：{error}", file=sys.stderr)
        sys.exit(1)
