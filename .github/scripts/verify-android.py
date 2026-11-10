#!/usr/bin/env python3
"""回读正式签名、ARM64 ABI、版本及16KiB ELF对齐后才收集Android产物。"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import ssl
import zipfile

ROOT = Path(__file__).resolve().parents[2]
label = os.environ["MOBILE_VERSION_LABEL"]
if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", label):
    raise ValueError("移动产物标签无效")
expected = re.sub(r"[\s:]", "", os.environ["ANDROID_CERT_SHA256"]).lower()
if not re.fullmatch(r"[a-f0-9]{64}", expected):
    raise ValueError("发布证书摘要格式无效")
text = Path(os.environ["ANDROID_SIGNATURE_RECORD"]).read_text(encoding="utf-8")
certificates = re.findall(r"Signer #\d+ certificate SHA-256 digest: ([a-fA-F0-9]+)", text)
if [value.lower() for value in certificates] != [expected] or "Verified using v2 scheme (APK Signature Scheme v2): true" not in text:
    raise ValueError("APK签名证书或v2签名不符")
apk = ROOT / "app/build/android/app/build/outputs/apk/release/app-release.apk"
aab = ROOT / "app/build/android/app/build/outputs/bundle/release/app-release.aab"
with zipfile.ZipFile(apk) as archive:
    libs = [name for name in archive.namelist() if name.startswith("lib/") and name.endswith(".so")]
    if libs != ["lib/arm64-v8a/libwails.so"]:
        raise ValueError("APK必须仅包含ARM64共享库")
    library = archive.read(libs[0])
    if library[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", library, 18)[0] != 183:
        raise ValueError("Android共享库架构无效")
    offset = struct.unpack_from("<Q", library, 32)[0]
    size, count = struct.unpack_from("<HH", library, 54)
    loads = [struct.unpack_from("<IIQQQQQQ", library, offset+i*size) for i in range(count)]
    if not any(p[0] == 1 for p in loads) or any(p[0] == 1 and (p[7] < 16384 or (p[2]-p[3]) % 16384) for p in loads):
        raise ValueError("Android ELF未满足16KiB页对齐")
with zipfile.ZipFile(aab) as archive:
    libs = [name for name in archive.namelist() if name.endswith(".so")]
    if libs != ["base/lib/arm64-v8a/libwails.so"] or archive.read(libs[0]) != library:
        raise ValueError("AAB共享库不匹配")
    signers = [name for name in archive.namelist() if re.fullmatch(r"META-INF/[^/]+\.(RSA|DSA|EC)", name)]
    if len(signers) != 1:
        raise ValueError("AAB必须包含单一签名者")
verification = subprocess.run(["jarsigner", "-J-Duser.language=en", "-J-Duser.country=US", "-verify", "-strict", str(aab)], capture_output=True, text=True, timeout=60)
if verification.returncode not in (0, 4) or "jar verified" not in verification.stdout.lower():
    raise ValueError("AAB JAR签名完整性校验失败")
certificate_text = subprocess.run(["keytool", "-J-Duser.language=en", "-printcert", "-jarfile", str(aab), "-rfc"], check=True, capture_output=True, text=True, timeout=30).stdout
chain = re.findall(r"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----", certificate_text, re.S)
if not chain or hashlib.sha256(ssl.PEM_cert_to_DER_cert(chain[0])).hexdigest() != expected:
    raise ValueError("AAB签名证书与APK不一致")
aapt = Path(os.environ["ANDROID_HOME"]) / "build-tools/36.0.0/aapt"
badging = subprocess.run([str(aapt), "dump", "badging", str(apk)], check=True, capture_output=True, text=True, timeout=30).stdout
version = json.loads((ROOT / "app/frontend/package.json").read_text(encoding="utf-8"))["version"]
if f"name='com.kitonyterms.app'" not in badging or f"versionCode='{os.environ['MOBILE_BUILD_NUMBER']}'" not in badging or f"versionName='{version}'" not in badging:
    raise ValueError("APK身份或版本不匹配")
dist = ROOT / "dist"
dist.mkdir(exist_ok=True)
files = {}
for source, suffix in ((apk, ".apk"), (aab, ".aab")):
    destination = dist / ("kitonyterms-"+label+"-android-aarch64"+suffix)
    if destination.exists():
        raise ValueError("拒绝覆盖已有Android产物")
    shutil.copyfile(source, destination)
    files[destination.name] = dict(size=destination.stat().st_size, sha256=hashlib.sha256(destination.read_bytes()).hexdigest())
record = dict(version=version, buildNumber=int(os.environ["MOBILE_BUILD_NUMBER"]), commit=os.environ["GITHUB_SHA"], build=int(os.environ["GITHUB_RUN_ID"]), attempt=int(os.environ["GITHUB_RUN_ATTEMPT"]), abi="arm64-v8a", certificateSHA256=expected, signatureV2Verified=True, aabSignatureVerified=True, pageAlignment=16384, deviceExecutionVerified=False, files=files)
(dist / "verification-android.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
print("Android正式证书、版本、ARM64 APK/AAB与16KiB页对齐校验通过")
