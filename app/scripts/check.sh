#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

command -v wails3 >/dev/null
[ "$(wails3 version 2>&1)" = 'v3.1.0' ]
[ "$(go list -m -f '{{.Version}}' github.com/wailsapp/wails/v3)" = 'v3.1.0' ]
node --input-type=module -e 'import fs from "node:fs"; const p=JSON.parse(fs.readFileSync("frontend/package.json","utf8")); if(p.dependencies["@wailsio/runtime"]!=="3.1.0") throw Error("Wails runtime 版本漂移");'
bash -n scripts/compile.sh
python3 -c 'from pathlib import Path; p=Path("scripts/ios_ipa.py"); compile(p.read_text(),str(p),"exec")'
if [ -n "$(gofmt -l main.go native_desktop.go native_mobile.go internal tools)" ]; then
  echo 'Go 格式不符合 gofmt'
  exit 1
fi
go vet ./...
# 更新领域包含真实签名 DMG、独立 helper 与崩溃恢复；各操作另有较短超时。
go test -race -count=1 -timeout=240s ./internal/... ./tools/...

check_tmp=$(mktemp -d "${PI_SCRATCH_DIR:-${TMPDIR:-/tmp}}/kitonyterms-check.XXXXXX")
trap 'rm -rf "$check_tmp"' EXIT
cp -R frontend/bindings "$check_tmp/bindings"
wails3 generate bindings -ts -i
diff -ru "$check_tmp/bindings" frontend/bindings
(
  cd frontend
  npm run format:check
  npm run typecheck
  npm run lint
  npm test
  npm run build
)
echo 'Go、绑定漂移与前端检查全部通过'
