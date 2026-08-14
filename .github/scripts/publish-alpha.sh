#!/usr/bin/env bash

set -euo pipefail

readonly ALPHA_REF="tags/alpha"

fail() {
  echo "::error::$*" >&2
  exit 1
}

for command in gh jq find sort diff; do
  command -v "$command" >/dev/null 2>&1 || fail "缺少命令: ${command}"
done

for name in \
  GH_TOKEN \
  GITHUB_REPOSITORY \
  GITHUB_SHA \
  GITHUB_RUN_ID \
  GITHUB_RUN_ATTEMPT \
  STAGING_RELEASE_ID \
  STAGING_TAG; do
  [[ -n "${!name:-}" ]] || fail "缺少环境变量: ${name}"
done

readonly BACKUP_TAG="alpha-backup-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
work_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kitonyterms-alpha.XXXXXX")"
old_release_id=""
old_tag_sha=""
transaction_started=false
committed=false

api_optional() {
  local output_path="$1"
  shift
  local error_path="$work_dir/api-error.txt"
  if gh api "$@" >"$output_path" 2>"$error_path"; then
    return 0
  fi
  if grep -q 'HTTP 404' "$error_path"; then
    : >"$output_path"
    return 1
  fi
  cat "$error_path" >&2
  fail "GitHub API 请求失败: $*"
}

rollback() {
  echo "::warning::Alpha 切换失败，开始恢复旧 tag 与 Release"

  # API 超时可能发生在服务端已完成变更之后，因此回滚始终按服务端当前状态执行。
  gh api --method PATCH \
    "repos/${GITHUB_REPOSITORY}/releases/${STAGING_RELEASE_ID}" \
    -f tag_name="$STAGING_TAG" \
    -F draft=true \
    -F prerelease=true \
    -f make_latest=false >/dev/null 2>&1 \
    || echo "::error::无法把 staging Release 恢复为草稿"

  if [[ -n "$old_tag_sha" ]]; then
    gh api --method PATCH \
      "repos/${GITHUB_REPOSITORY}/git/refs/${ALPHA_REF}" \
      -f sha="$old_tag_sha" \
      -F force=true >/dev/null 2>&1 \
      || echo "::error::无法恢复旧 alpha tag"
  else
    gh api --method DELETE \
      "repos/${GITHUB_REPOSITORY}/git/refs/${ALPHA_REF}" >/dev/null 2>&1 \
      || echo "::warning::删除新建 alpha tag 失败或 tag 已不存在"
  fi

  if [[ -n "$old_release_id" ]]; then
    gh api --method PATCH \
      "repos/${GITHUB_REPOSITORY}/releases/${old_release_id}" \
      -f tag_name=alpha \
      -F draft=false \
      -F prerelease=true \
      -f make_latest=false >/dev/null 2>&1 \
      || echo "::error::无法恢复旧 Alpha Release"
  fi

  gh api --method DELETE \
    "repos/${GITHUB_REPOSITORY}/git/refs/tags/${STAGING_TAG}" >/dev/null 2>&1 || true
}

on_exit() {
  local status="$?"
  trap - EXIT INT TERM
  if ((status != 0)) && [[ "$transaction_started" == true && "$committed" != true ]]; then
    rollback
  fi
  rm -rf "$work_dir"
  exit "$status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

staging_json="$(gh api "repos/${GITHUB_REPOSITORY}/releases/${STAGING_RELEASE_ID}")"
[[ "$(jq -r '.draft' <<<"$staging_json")" == true ]] \
  || fail "staging Alpha Release 必须保持 draft=true"
[[ "$(jq -r '.tag_name' <<<"$staging_json")" == "$STAGING_TAG" ]] \
  || fail "staging Alpha Release tag 不一致"

find dist -maxdepth 1 -type f -exec basename {} \; | sort >"$work_dir/local-assets.txt"
jq -r '.assets[].name' <<<"$staging_json" | sort >"$work_dir/remote-assets.txt"
[[ -s "$work_dir/local-assets.txt" ]] || fail "没有可发布的 Alpha 资产"
diff -u "$work_dir/local-assets.txt" "$work_dir/remote-assets.txt" \
  || fail "staging Alpha Release 的资产列表与本地产物不一致"
if jq -e '.assets[] | select(.size <= 0)' <<<"$staging_json" >/dev/null; then
  fail "staging Alpha Release 包含空资产"
fi

old_release_path="$work_dir/old-release.json"
if api_optional "$old_release_path" \
  "repos/${GITHUB_REPOSITORY}/releases/tags/alpha"; then
  old_release_id="$(jq -er '.id' "$old_release_path")"
fi

old_ref_path="$work_dir/old-ref.json"
if api_optional "$old_ref_path" \
  "repos/${GITHUB_REPOSITORY}/git/ref/${ALPHA_REF}"; then
  old_tag_sha="$(jq -er '.object.sha' "$old_ref_path")"
fi

transaction_started=true

if [[ -n "$old_release_id" ]]; then
  gh api --method PATCH \
    "repos/${GITHUB_REPOSITORY}/releases/${old_release_id}" \
    -f tag_name="$BACKUP_TAG" \
    -F draft=true \
    -F prerelease=true \
    -f make_latest=false >/dev/null
fi

if [[ -n "$old_tag_sha" ]]; then
  gh api --method PATCH \
    "repos/${GITHUB_REPOSITORY}/git/refs/${ALPHA_REF}" \
    -f sha="$GITHUB_SHA" \
    -F force=true >/dev/null
else
  gh api --method POST \
    "repos/${GITHUB_REPOSITORY}/git/refs" \
    -f ref=refs/tags/alpha \
    -f sha="$GITHUB_SHA" >/dev/null
fi
gh api --method PATCH \
  "repos/${GITHUB_REPOSITORY}/releases/${STAGING_RELEASE_ID}" \
  -f tag_name=alpha \
  -F draft=false \
  -F prerelease=true \
  -f make_latest=false >/dev/null
verified=false
for _ in {1..6}; do
  current_tag_sha="$(gh api \
    "repos/${GITHUB_REPOSITORY}/git/ref/${ALPHA_REF}" --jq '.object.sha')"
  current_release="$(gh api "repos/${GITHUB_REPOSITORY}/releases/tags/alpha")"
  if [[ "$current_tag_sha" == "$GITHUB_SHA" ]] \
    && [[ "$(jq -r '.id' <<<"$current_release")" == "$STAGING_RELEASE_ID" ]] \
    && [[ "$(jq -r '.draft' <<<"$current_release")" == false ]] \
    && [[ "$(jq -r '.prerelease' <<<"$current_release")" == true ]]; then
    verified=true
    break
  fi
  sleep 2
done
[[ "$verified" == true ]] || fail "Alpha tag 与新 Release 切换后复验失败"
committed=true

if [[ -n "$old_release_id" ]]; then
  gh api --method DELETE \
    "repos/${GITHUB_REPOSITORY}/releases/${old_release_id}" >/dev/null 2>&1 \
    || echo "::warning::新 Alpha 已发布，但旧 backup 草稿清理失败"
fi
for obsolete_tag in "$BACKUP_TAG" "$STAGING_TAG"; do
  gh api --method DELETE \
    "repos/${GITHUB_REPOSITORY}/git/refs/tags/${obsolete_tag}" >/dev/null 2>&1 || true
done

echo "Alpha Release 已切换到 ${GITHUB_SHA}，固定 tag 与完整资产一致"
