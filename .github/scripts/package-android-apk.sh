#!/usr/bin/env bash

set -Eeuo pipefail

readonly DIOXUS_CLI_VERSION="0.7.9"
readonly ANDROID_TARGET="aarch64-linux-android"
readonly ANDROID_ABI="arm64-v8a"
readonly ANDROID_BUILD_TOOLS_VERSION="35.0.0"
readonly ANDROID_ICON_SOURCE="crates/kt-app/assets/android/res"

fail() {
  echo "::error::$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "缺少命令: $1"
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    fail "缺少环境变量: $name"
  fi
}

for name in \
  MOBILE_BUNDLE_ID \
  MOBILE_VERSION_LABEL \
  MOBILE_MARKETING_VERSION \
  MOBILE_BUILD_NUMBER \
  ANDROID_KEYSTORE_BASE64 \
  ANDROID_KEYSTORE_PASSWORD \
  ANDROID_KEY_ALIAS \
  ANDROID_CERT_SHA256; do
  require_env "$name"
done

if [[ -z "${ANDROID_KEY_PASSWORD:-}" ]]; then
  ANDROID_KEY_PASSWORD="$ANDROID_KEYSTORE_PASSWORD"
fi
export ANDROID_KEY_PASSWORD

[[ "$MOBILE_BUNDLE_ID" =~ ^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$ ]] ||
  fail "MOBILE_BUNDLE_ID 不是有效的 Android applicationId: $MOBILE_BUNDLE_ID"
[[ "$MOBILE_VERSION_LABEL" =~ ^[0-9A-Za-z][0-9A-Za-z._+-]*$ ]] ||
  fail "MOBILE_VERSION_LABEL 只能包含字母、数字、点、下划线、加号和短横线"
[[ "$MOBILE_MARKETING_VERSION" =~ ^[0-9]+(\.[0-9]+){2}([-+][0-9A-Za-z.-]+)?$ ]] ||
  fail "MOBILE_MARKETING_VERSION 必须是三段式版本号，可带 prerelease/build 后缀"
[[ "$MOBILE_BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]] ||
  fail "MOBILE_BUILD_NUMBER 必须是正整数"
((MOBILE_BUILD_NUMBER <= 2100000000)) ||
  fail "MOBILE_BUILD_NUMBER 超出 Android versionCode 上限"

expected_cert_sha256="$(printf '%s' "$ANDROID_CERT_SHA256" | tr -d '[:space:]:' | tr '[:upper:]' '[:lower:]')"
[[ "$expected_cert_sha256" =~ ^[0-9a-f]{64}$ ]] ||
  fail "ANDROID_CERT_SHA256 必须是 64 位十六进制 SHA-256 摘要"

require_command dx
require_command perl
require_command unzip
require_command find

dx_version="$(dx --version 2>&1)"
[[ "$dx_version" =~ (^|[^0-9])${DIOXUS_CLI_VERSION//./\.}([^0-9]|$) ]] ||
  fail "Dioxus CLI 版本不一致，期望 $DIOXUS_CLI_VERSION，实际: $dx_version"

[[ -n "${ANDROID_HOME:-}" ]] || fail "缺少环境变量: ANDROID_HOME"
readonly BUILD_TOOLS_DIR="$ANDROID_HOME/build-tools/$ANDROID_BUILD_TOOLS_VERSION"
readonly ZIPALIGN="$BUILD_TOOLS_DIR/zipalign"
readonly APKSIGNER="$BUILD_TOOLS_DIR/apksigner"
readonly AAPT="$BUILD_TOOLS_DIR/aapt"
for tool in "$ZIPALIGN" "$APKSIGNER" "$AAPT"; do
  [[ -x "$tool" ]] || fail "缺少 Android Build Tools 可执行文件: $tool"
done

[[ -d "$ANDROID_ICON_SOURCE" ]] || fail "缺少仓库 Android 图标目录: $ANDROID_ICON_SOURCE"
icon_count="$(find "$ANDROID_ICON_SOURCE" -type f -path '*/mipmap-*/ic_launcher.png' | wc -l | tr -d '[:space:]')"
((icon_count > 0)) || fail "仓库 Android 图标目录中没有 mipmap-*/ic_launcher.png"

temp_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
work_dir="$(mktemp -d "$temp_root/kitonyterms-android.XXXXXX")"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

keystore_path="$work_dir/kitonyterms-android-signing.p12"
if ! printf '%s' "$ANDROID_KEYSTORE_BASE64" | base64 --decode >"$keystore_path" 2>/dev/null; then
  printf '%s' "$ANDROID_KEYSTORE_BASE64" | base64 -d >"$keystore_path" 2>/dev/null ||
    fail "ANDROID_KEYSTORE_BASE64 无法解码"
fi
[[ -s "$keystore_path" ]] || fail "解码后的 Android keystore 为空"
chmod 600 "$keystore_path"

rm -rf target/dx/kitonyterms/release/android
ANDROID_PLATFORM="${ANDROID_PLATFORM:-android-35}" dx bundle \
  --release \
  --platform android \
  --target "$ANDROID_TARGET" \
  --package-types apk \
  --package kt-app

gradlew_path="$(find target/dx -path '*/release/android/app/gradlew' -print -quit)"
[[ -n "$gradlew_path" ]] || fail "未找到 Dioxus Android Gradle 工程"
android_project="$(dirname "$gradlew_path")"

gradle_file=""
for candidate in "$android_project/app/build.gradle.kts" "$android_project/app/build.gradle"; do
  if [[ -f "$candidate" ]]; then
    gradle_file="$candidate"
    break
  fi
done
[[ -n "$gradle_file" ]] || fail "未找到 Android app 模块 Gradle 配置"

MOBILE_BUNDLE_ID="$MOBILE_BUNDLE_ID" \
MOBILE_MARKETING_VERSION="$MOBILE_MARKETING_VERSION" \
MOBILE_BUILD_NUMBER="$MOBILE_BUILD_NUMBER" \
perl -0pi -e '
  my $application_id = s/^(\s*applicationId\s*=\s*)"[^"]*"/$1"$ENV{MOBILE_BUNDLE_ID}"/m;
  my $version_code = s/^(\s*versionCode\s*=\s*)\d+/$1$ENV{MOBILE_BUILD_NUMBER}/m;
  my $version_name = s/^(\s*versionName\s*=\s*)"[^"]*"/$1"$ENV{MOBILE_MARKETING_VERSION}"/m;
  die "Gradle 配置中的 applicationId 替换次数不是 1: $application_id\n" if $application_id != 1;
  die "Gradle 配置中的 versionCode 替换次数不是 1: $version_code\n" if $version_code != 1;
  die "Gradle 配置中的 versionName 替换次数不是 1: $version_name\n" if $version_name != 1;
' "$gradle_file"

res_dir="$android_project/app/src/main/res"
[[ -d "$res_dir" ]] || fail "未找到 Android res 目录: $res_dir"
find "$res_dir" -type f -name 'ic_launcher*' -delete
cp -R "$ANDROID_ICON_SOURCE/." "$res_dir/"
chmod +x "$gradlew_path"

(
  cd "$android_project"
  ./gradlew assembleRelease --no-daemon
)

unsigned_apk="$(find "$android_project/app/build/outputs/apk/release" -name '*release-unsigned.apk' -print -quit)"
[[ -n "$unsigned_apk" && -f "$unsigned_apk" ]] || fail "Android release unsigned APK 产物不存在"

aligned_apk="$work_dir/kitonyterms-aligned.apk"
signed_apk="$work_dir/kitonyterms-signed.apk"
"$ZIPALIGN" -p -f 4 "$unsigned_apk" "$aligned_apk"
"$ZIPALIGN" -c -p 4 "$aligned_apk"
"$APKSIGNER" sign \
  --ks "$keystore_path" \
  --ks-type PKCS12 \
  --ks-pass env:ANDROID_KEYSTORE_PASSWORD \
  --ks-key-alias "$ANDROID_KEY_ALIAS" \
  --key-pass env:ANDROID_KEY_PASSWORD \
  --out "$signed_apk" \
  "$aligned_apk"

cert_file="$work_dir/certificate.txt"
"$APKSIGNER" verify --verbose --print-certs "$signed_apk" >"$cert_file"
cat "$cert_file"
actual_cert_sha256="$(awk -F': ' '/Signer #1 certificate SHA-256 digest/ {print $2; exit}' "$cert_file" | tr -d '[:space:]:' | tr '[:upper:]' '[:lower:]')"
[[ -n "$actual_cert_sha256" ]] || fail "无法读取 Android APK 签名证书 SHA-256 摘要"
[[ "$actual_cert_sha256" == "$expected_cert_sha256" ]] ||
  fail "Android APK 签名证书 SHA-256 不一致，实际为 $actual_cert_sha256"

apk_listing="$work_dir/apk-listing.txt"
unzip -Z1 "$signed_apk" >"$apk_listing"
grep -Fxq "lib/$ANDROID_ABI/libmain.so" "$apk_listing" ||
  fail "Android APK 缺少 $ANDROID_ABI/libmain.so"
grep -Eq '^res/mipmap-[^/]+/ic_launcher\.png$' "$apk_listing" ||
  fail "Android APK 缺少 PNG launcher 图标"

badging_file="$work_dir/badging.txt"
"$AAPT" dump badging "$signed_apk" >"$badging_file"
cat "$badging_file"
package_line="$(grep -m1 '^package:' "$badging_file" || true)"
actual_bundle_id="$(printf '%s\n' "$package_line" | sed -n "s/.* name='\([^']*\)'.*/\1/p")"
actual_version_code="$(printf '%s\n' "$package_line" | sed -n "s/.* versionCode='\([^']*\)'.*/\1/p")"
actual_version_name="$(printf '%s\n' "$package_line" | sed -n "s/.* versionName='\([^']*\)'.*/\1/p")"
[[ "$actual_bundle_id" == "$MOBILE_BUNDLE_ID" ]] ||
  fail "Android APK 包名不一致，期望 $MOBILE_BUNDLE_ID，实际 $actual_bundle_id"
[[ "$actual_version_code" == "$MOBILE_BUILD_NUMBER" ]] ||
  fail "Android APK versionCode 不一致，期望 $MOBILE_BUILD_NUMBER，实际 $actual_version_code"
[[ "$actual_version_name" == "$MOBILE_MARKETING_VERSION" ]] ||
  fail "Android APK versionName 不一致，期望 $MOBILE_MARKETING_VERSION，实际 $actual_version_name"
grep -q "native-code:.*'$ANDROID_ABI'" "$badging_file" ||
  fail "Android APK badging 未声明 $ANDROID_ABI native-code"
grep -Eq "^application-icon-[0-9]+:'.*ic_launcher.*\.png'$" "$badging_file" ||
  fail "Android APK 未使用 PNG launcher 图标"

mkdir -p dist
output="dist/kitonyterms-${MOBILE_VERSION_LABEL}-android-aarch64.apk"
cp "$signed_apk" "$output"
echo "Android APK 已生成: $output"
