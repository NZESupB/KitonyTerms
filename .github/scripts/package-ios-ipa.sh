#!/usr/bin/env bash

set -euo pipefail

readonly REQUIRED_DX_VERSION="0.7.9"

fail() {
  echo "::error::$*" >&2
  exit 1
}

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] || fail "缺少必需环境变量: ${name}"
}

for name in \
  MOBILE_BUNDLE_ID \
  MOBILE_VERSION_LABEL \
  MOBILE_MARKETING_VERSION \
  MOBILE_BUILD_NUMBER; do
  require_env "$name"
done

[[ "${MOBILE_BUNDLE_ID}" =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ ]] \
  || fail "MOBILE_BUNDLE_ID 不是合法的反向域名标识符: ${MOBILE_BUNDLE_ID}"
[[ "${MOBILE_MARKETING_VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || fail "MOBILE_MARKETING_VERSION 必须是三段数字版本号"
[[ "${MOBILE_BUILD_NUMBER}" =~ ^[1-9][0-9]*$ ]] \
  || fail "MOBILE_BUILD_NUMBER 必须是大于零且单调递增的整数"
[[ "${MOBILE_VERSION_LABEL}" =~ ^[A-Za-z0-9._-]+$ ]] \
  || fail "MOBILE_VERSION_LABEL 只能包含字母、数字、点、下划线和短横线"

command -v dx >/dev/null || fail "未安装 Dioxus CLI"
command -v codesign >/dev/null || fail "当前环境缺少 codesign"
command -v plutil >/dev/null || fail "当前环境缺少 plutil"
command -v ditto >/dev/null || fail "当前环境缺少 ditto"
command -v lipo >/dev/null || fail "当前环境缺少 lipo"
command -v file >/dev/null || fail "当前环境缺少 file"

dx_version="$(dx --version 2>&1)"
[[ "$dx_version" == *"${REQUIRED_DX_VERSION}"* ]] \
  || fail "Dioxus CLI 版本不匹配，要求 ${REQUIRED_DX_VERSION}，实际为: ${dx_version}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

work_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kitonyterms-ios.XXXXXX")"
ipa_root="$work_dir/ipa"
verify_root="$work_dir/verify"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

echo "使用 Dioxus ${REQUIRED_DX_VERSION} 构建未签名 iOS arm64 应用"
if [[ -d target/dx ]]; then
  find target/dx -type d -path '*/release/ios' -prune -exec rm -rf {} +
fi
CODE_SIGNING_ALLOWED=NO \
CODE_SIGNING_REQUIRED=NO \
dx build \
  --release \
  --platform ios \
  --target aarch64-apple-ios \
  --package kt-app

apps_file="$work_dir/apps.txt"
find target/dx -type d -path '*/release/ios/*' -name '*.app' -print > "$apps_file"
app_count="$(wc -l < "$apps_file" | tr -d '[:space:]')"
[[ "$app_count" == "1" ]] \
  || fail "期望 Dioxus 生成唯一一个 iOS .app，实际找到 ${app_count} 个"
built_app="$(sed -n '1p' "$apps_file")"

# 仅修改 IPA 暂存副本，保留 Dioxus 原始构建目录用于故障排查。
mkdir -p "$ipa_root/Payload" dist
staged_app="$ipa_root/Payload/$(basename "$built_app")"
ditto "$built_app" "$staged_app"

info_plist="$staged_app/Info.plist"
[[ -f "$info_plist" ]] || fail "iOS .app 缺少 Info.plist: ${staged_app}"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier ${MOBILE_BUNDLE_ID}" "$info_plist" \
  || /usr/libexec/PlistBuddy -c "Add :CFBundleIdentifier string ${MOBILE_BUNDLE_ID}" "$info_plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${MOBILE_MARKETING_VERSION}" "$info_plist" \
  || /usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string ${MOBILE_MARKETING_VERSION}" "$info_plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${MOBILE_BUILD_NUMBER}" "$info_plist" \
  || /usr/libexec/PlistBuddy -c "Add :CFBundleVersion string ${MOBILE_BUILD_NUMBER}" "$info_plist"
plutil -lint "$info_plist" >/dev/null

executable_name="$(plutil -extract CFBundleExecutable raw -o - "$info_plist")"
main_executable="$staged_app/$executable_name"
[[ -f "$main_executable" ]] || fail "Info.plist 指定的主程序不存在: ${main_executable}"
[[ -x "$main_executable" ]] || fail "Info.plist 指定的主程序不可执行: ${main_executable}"
architectures="$(lipo -archs "$main_executable")"
[[ " $architectures " == *" arm64 "* ]] || fail "iOS 主程序不包含 arm64: ${architectures}"
[[ " $architectures " != *" x86_64 "* ]] || fail "iOS 真机产物意外包含 x86_64: ${architectures}"

# Xcode/Dioxus 可能生成 ad-hoc 签名；重签前必须移除全部 Mach-O 签名与描述文件。
while IFS= read -r candidate; do
  description="$(file -b "$candidate")"
  if [[ "$description" == Mach-O* ]]; then
    codesign --remove-signature "$candidate" >/dev/null 2>&1 || true
  fi
done < <(find "$staged_app" -type f -print)

find "$staged_app" -type d -name '_CodeSignature' -prune -exec rm -rf {} +
find "$staged_app" -name 'CodeResources' -exec rm -rf {} +
find "$staged_app" -type f \( \
  -name 'embedded.mobileprovision' -o \
  -name '*.mobileprovision' -o \
  -name '*.provisionprofile' \
\) -delete

ipa_path="$repo_root/dist/kitonyterms-${MOBILE_VERSION_LABEL}-ios-aarch64-unsigned.ipa"
rm -f "$ipa_path"
(
  cd "$ipa_root"
  ditto -c -k --keepParent Payload "$ipa_path"
)

mkdir -p "$verify_root"
ditto -x -k "$ipa_path" "$verify_root"
if find "$verify_root" -mindepth 1 -maxdepth 1 ! -name 'Payload' -print -quit | grep -q .; then
  fail "IPA 顶层只能包含 Payload 目录"
fi
verified_apps_file="$work_dir/verified-apps.txt"
find "$verify_root/Payload" -mindepth 1 -maxdepth 1 -type d -name '*.app' -print > "$verified_apps_file"
verified_app_count="$(wc -l < "$verified_apps_file" | tr -d '[:space:]')"
[[ "$verified_app_count" == "1" ]] || fail "IPA 中必须且只能包含一个 Payload/*.app"
verified_app="$(sed -n '1p' "$verified_apps_file")"

verified_info="$verified_app/Info.plist"
plutil -lint "$verified_info" >/dev/null || fail "IPA 中的 Info.plist 无效"
[[ "$(plutil -extract CFBundleIdentifier raw -o - "$verified_info")" == "$MOBILE_BUNDLE_ID" ]] \
  || fail "IPA 的 CFBundleIdentifier 校验失败"
[[ "$(plutil -extract CFBundleShortVersionString raw -o - "$verified_info")" == "$MOBILE_MARKETING_VERSION" ]] \
  || fail "IPA 的 CFBundleShortVersionString 校验失败"
[[ "$(plutil -extract CFBundleVersion raw -o - "$verified_info")" == "$MOBILE_BUILD_NUMBER" ]] \
  || fail "IPA 的 CFBundleVersion 校验失败"

verified_executable_name="$(plutil -extract CFBundleExecutable raw -o - "$verified_info")"
verified_executable="$verified_app/$verified_executable_name"
[[ -f "$verified_executable" && -x "$verified_executable" ]] \
  || fail "IPA 主程序不存在或不可执行: ${verified_executable}"
verified_architectures="$(lipo -archs "$verified_executable")"
[[ " $verified_architectures " == *" arm64 "* ]] \
  || fail "IPA 主程序不包含 arm64: ${verified_architectures}"
[[ " $verified_architectures " != *" x86_64 "* ]] \
  || fail "IPA 主程序意外包含 x86_64: ${verified_architectures}"

if find "$verified_app" \( \
  -name '_CodeSignature' -o \
  -name 'CodeResources' -o \
  -name 'embedded.mobileprovision' -o \
  -name '*.mobileprovision' -o \
  -name '*.provisionprofile' \
\) -print -quit | grep -q .; then
  fail "IPA 中仍残留代码签名资源或 provisioning profile"
fi

while IFS= read -r candidate; do
  description="$(file -b "$candidate")"
  if [[ "$description" == Mach-O* ]] && codesign -d "$candidate" >/dev/null 2>&1; then
    fail "IPA 中仍存在代码签名: ${candidate}"
  fi
done < <(find "$verified_app" -type f -print)

echo "未签名 iOS IPA 已生成并通过结构校验: ${ipa_path}"
