#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  cat <<'HELP'
用法：bash scripts/compile.sh [GOOS] [GOARCH] [desktop|server] [OUTPUT]
默认编译宿主桌面，输出到 bin/compile/<系统>-<架构>-<模式>/kitonyterms[.exe]。
支持 darwin/windows/linux 的 amd64/arm64；移动端沿用 android/ios task。
server 必须显式选择，只编译无 GUI 的 HTTP 服务，不代表原生桌面验收。
需要已有 Go/npm 依赖和 Wails CLI 3.0.0，不自动安装依赖或工具。
HELP
}

if [ "${1:-}" = '--help' ] || [ "${1:-}" = '-h' ]; then
  usage
  exit 0
fi
if [ "$#" -gt 4 ]; then
  usage >&2
  exit 2
fi
command -v go >/dev/null || { echo '缺少 Go 工具链' >&2; exit 1; }
host_os=$(go env GOHOSTOS)
host_arch=$(go env GOHOSTARCH)
target_os=${1:-${GOOS:-$host_os}}
target_arch=${2:-${GOARCH:-$host_arch}}
mode=${3:-desktop}
case "$target_os/$target_arch" in
  darwin/amd64|darwin/arm64|windows/amd64|windows/arm64|linux/amd64|linux/arm64) ;;
  *) echo '不支持此桌面编译目标；移动端请使用 android/ios task' >&2; exit 2 ;;
esac
case "$mode" in
  desktop|server) ;;
  *) echo '编译模式必须为 desktop 或 server' >&2; exit 2 ;;
esac

cgo=0
tags=production
ldflags='-w -s'
if [ "$mode" = server ]; then
  tags=production,server
elif [ "$target_os" = darwin ]; then
  if [ "$host_os" != darwin ]; then
    echo 'macOS 桌面编译需要 macOS 主机和 SDK；跨平台构建请使用 darwin:build' >&2
    exit 2
  fi
  command -v xcrun >/dev/null || { echo '缺少 macOS SDK 工具 xcrun' >&2; exit 1; }
  xcrun --sdk macosx --show-sdk-path >/dev/null
  cgo=1
  export MACOSX_DEPLOYMENT_TARGET=15.0
  export CGO_CFLAGS="${CGO_CFLAGS:+$CGO_CFLAGS }-mmacosx-version-min=15.0"
  export CGO_LDFLAGS="${CGO_LDFLAGS:+$CGO_LDFLAGS }-mmacosx-version-min=15.0"
elif [ "$target_os" = linux ]; then
  if [ "$host_os" != linux ] || [ "$target_arch" != "$host_arch" ]; then
    echo 'Linux 桌面编译需要对应架构的 Linux CGO 环境；请使用 linux:build，或显式选择 server 仅验证无 GUI 编译' >&2
    exit 2
  fi
  command -v pkg-config >/dev/null || { echo 'Linux 桌面编译缺少 pkg-config 与原生开发依赖' >&2; exit 1; }
  cgo=1
else
  ldflags="$ldflags -H windowsgui"
fi

for tool in node npm wails3; do
  command -v "$tool" >/dev/null || { printf '缺少构建工具：%s\n' "$tool" >&2; exit 1; }
done
if [ "$(wails3 version 2>&1)" != 'v3.0.0' ]; then
  echo 'Wails CLI 必须固定为 v3.0.0' >&2
  exit 1
fi
if [ "$(go list -mod=readonly -m -f '{{.Version}}' github.com/wailsapp/wails/v3)" != 'v3.0.0' ]; then
  echo 'Wails Go 模块必须固定为 v3.0.0' >&2
  exit 1
fi
if [ ! -d frontend/node_modules ]; then
  echo '缺少前端依赖，请先执行 npm --prefix frontend ci' >&2
  exit 1
fi
node --input-type=module <<'JS'
import fs from 'node:fs';
const frontend = JSON.parse(fs.readFileSync('frontend/package.json', 'utf8'));
const installed = JSON.parse(fs.readFileSync('frontend/node_modules/@wailsio/runtime/package.json', 'utf8'));
if (frontend.dependencies['@wailsio/runtime'] !== '3.0.0' || installed.version !== '3.0.0') {
  throw Error('前端 Wails runtime 必须固定为 3.0.0');
}
JS

extension=''
if [ "$target_os" = windows ]; then extension='.exe'; fi
output=${4:-bin/compile/$target_os-$target_arch-$mode/kitonyterms$extension}
if [ -d "$output" ]; then
  echo 'OUTPUT 必须为可执行文件路径，不能是目录' >&2
  exit 2
fi
export GOOS="$target_os" GOARCH="$target_arch" CGO_ENABLED="$cgo"
printf '开始编译：%s/%s，模式 %s，CGO=%s\n' "$GOOS" "$GOARCH" "$mode" "$CGO_ENABLED"
wails3 generate bindings -f "-mod=readonly -tags $tags" -ts -i
npm --prefix frontend run build
mkdir -p -- "$(dirname "$output")"
go build -mod=readonly -tags "$tags" -trimpath -buildvcs=false \
  -ldflags="$ldflags" -o "$output" .
printf '编译通过：%s\n' "$output"
if [ "$mode" = server ]; then
  echo '产物为无 GUI 的 HTTP 服务；隔离运行时请设置 WAILS_SERVER_HOST=127.0.0.1。'
fi
