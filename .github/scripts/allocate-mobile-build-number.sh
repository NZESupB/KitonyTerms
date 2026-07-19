#!/usr/bin/env bash

set -Eeuo pipefail

readonly COUNTER_REF="ci/mobile-build-number"
readonly COUNTER_FULL_REF="refs/${COUNTER_REF}"
readonly MAX_BUILD_NUMBER=2100000000
readonly MAX_ATTEMPTS=12

fail() {
  echo "::error::$*" >&2
  exit 1
}

for command in gh jq date; do
  command -v "$command" >/dev/null 2>&1 || fail "缺少命令: ${command}"
done

for name in \
  GH_TOKEN \
  GITHUB_REPOSITORY \
  GITHUB_SHA \
  GITHUB_RUN_ID \
  GITHUB_RUN_ATTEMPT \
  GITHUB_OUTPUT; do
  [[ -n "${!name:-}" ]] || fail "缺少环境变量: ${name}"
done

for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  ref_exists=false
  ref_error="$(mktemp)"
  if ref_json="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/${COUNTER_REF}" 2>"$ref_error")"; then
    ref_exists=true
    parent_sha="$(jq -er '.object.sha' <<<"$ref_json")" \
      || fail "移动构建号引用缺少 object.sha"
    parent_commit="$(gh api "repos/${GITHUB_REPOSITORY}/git/commits/${parent_sha}")"
    parent_message="$(jq -er '.message' <<<"$parent_commit")" \
      || fail "无法读取移动构建号提交消息"
    tree_sha="$(jq -er '.tree.sha' <<<"$parent_commit")" \
      || fail "无法读取移动构建号提交 tree"
    parent_first_line="${parent_message%%$'\n'*}"
    if [[ "$parent_first_line" =~ ^mobile-build-number:\ ([1-9][0-9]*)$ ]]; then
      last_build_number="${BASH_REMATCH[1]}"
    else
      fail "移动构建号提交消息格式错误: ${parent_first_line}"
    fi
  else
    if ! grep -q 'HTTP 404' "$ref_error"; then
      cat "$ref_error" >&2
      fail "读取移动构建号引用失败"
    fi
    # Contents commits endpoint 会把 annotated tag 等 ref 剥离为实际 commit。
    base_commit="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${GITHUB_SHA}")"
    parent_sha="$(jq -er '.sha' <<<"$base_commit")" \
      || fail "无法解析当前 ref 对应的 commit SHA"
    tree_sha="$(jq -er '.commit.tree.sha' <<<"$base_commit")" \
      || fail "无法读取当前提交 tree"
    last_build_number=0
  fi
  rm -f "$ref_error"

  now="$(date -u +%s)"
  [[ "$now" =~ ^[1-9][0-9]*$ ]] || fail "无法生成 UTC Unix 秒构建号"
  candidate="$now"
  if ((candidate <= last_build_number)); then
    candidate=$((last_build_number + 1))
  fi
  ((candidate <= MAX_BUILD_NUMBER)) \
    || fail "移动构建号 ${candidate} 超出 Android versionCode 上限 ${MAX_BUILD_NUMBER}"

  commit_message="$(printf \
    'mobile-build-number: %s\nallocator: %s.%s.%s' \
    "$candidate" \
    "$GITHUB_RUN_ID" \
    "$GITHUB_RUN_ATTEMPT" \
    "$attempt")"
  new_commit="$(gh api --method POST \
    "repos/${GITHUB_REPOSITORY}/git/commits" \
    -f message="$commit_message" \
    -f tree="$tree_sha" \
    -f "parents[]=${parent_sha}")"
  new_commit_sha="$(jq -er '.sha' <<<"$new_commit")" \
    || fail "GitHub 未返回新的移动构建号提交 SHA"

  if [[ "$ref_exists" == true ]]; then
    if gh api --method PATCH \
      "repos/${GITHUB_REPOSITORY}/git/refs/${COUNTER_REF}" \
      -f sha="$new_commit_sha" \
      -F force=false >/dev/null 2>&1; then
      printf 'build_number=%s\n' "$candidate" >>"$GITHUB_OUTPUT"
      echo "已分配移动构建号: ${candidate}"
      exit 0
    fi
  elif gh api --method POST \
    "repos/${GITHUB_REPOSITORY}/git/refs" \
    -f ref="$COUNTER_FULL_REF" \
    -f sha="$new_commit_sha" >/dev/null 2>&1; then
    printf 'build_number=%s\n' "$candidate" >>"$GITHUB_OUTPUT"
    echo "已初始化并分配移动构建号: ${candidate}"
    exit 0
  fi

  echo "移动构建号发生并发竞争，第 ${attempt}/${MAX_ATTEMPTS} 次重试"
  sleep "$attempt"
done

fail "移动构建号并发分配重试耗尽"
