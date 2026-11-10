#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
for name in MOBILE_BUILD_NUMBER MOBILE_VERSION_LABEL ANDROID_HOME ANDROID_KEYSTORE_BASE64 ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD ANDROID_CERT_SHA256; do
  [ -n "${!name:-}" ] || { printf '缺少发布参数：%s\n' "$name" >&2; exit 1; }
done
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kitonyterms-signing.XXXXXX")
trap 'rm -rf "$work"' EXIT
export ANDROID_KEYSTORE_FILE="$work/release.p12"
python3 -B - <<'PY'
import base64, os
from pathlib import Path
path = Path(os.environ['ANDROID_KEYSTORE_FILE'])
path.write_bytes(base64.b64decode(os.environ['ANDROID_KEYSTORE_BASE64'], validate=True))
os.chmod(path, 0o600)
PY
(
  cd app
  wails3 task android:build PRODUCTION=true ARCH=arm64
  cd build/android
  bash ./gradlew --no-daemon lintRelease testReleaseUnitTest assembleRelease bundleRelease
)
export APKSIGNER="$ANDROID_HOME/build-tools/36.0.0/apksigner"
"$APKSIGNER" verify --verbose --print-certs app/build/android/app/build/outputs/apk/release/app-release.apk > "$work/signature.txt"
"$ANDROID_HOME/build-tools/36.0.0/zipalign" -c -P 16 -v 4 app/build/android/app/build/outputs/apk/release/app-release.apk > "$work/alignment.txt"
export ANDROID_SIGNATURE_RECORD="$work/signature.txt"
python3 -B .github/scripts/verify-android.py
