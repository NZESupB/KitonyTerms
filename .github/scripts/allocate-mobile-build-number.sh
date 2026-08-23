#!/usr/bin/env bash

set -Eeuo pipefail

# 旧实现把计数器写入 Git ref；上级策略可使 Actions integration token 保持只读，
# 即使 workflow 声明 contents:write 仍会收到 403。因此分配器不再写 GitHub API，
# Alpha 与 Release 通过同一个 workflow concurrency group 串行执行这段临界区。
readonly CUTOVER_FLOOR=1787238032
readonly MAX_BUILD_NUMBER=2100000000
readonly MAX_WAIT_SECONDS=30
readonly MAX_CLOCK_VALUE=9223372036854775807

fail() {
  echo "::error::$*" >&2
  exit 1
}

clock_now() {
  date -u +%s
}

sleep_seconds() {
  sleep "$1"
}

require_timestamp() {
  local value="$1"
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "无法生成有效的 UTC Unix 秒构建号: ${value:-空值}"
  if ((${#value} > ${#MAX_CLOCK_VALUE})) ||
    { ((${#value} == ${#MAX_CLOCK_VALUE})) && [[ "$value" > "$MAX_CLOCK_VALUE" ]]; }; then
    fail "UTC Unix 秒时钟值超出 Bash 可计算范围: $value"
  fi
}

require_build_number_limit() {
  local value="$1"
  require_timestamp "$value"
  if ((${#value} > ${#MAX_BUILD_NUMBER})) ||
    { ((${#value} == ${#MAX_BUILD_NUMBER})) && [[ "$value" > "$MAX_BUILD_NUMBER" ]]; }; then
    fail "UTC Unix 秒构建号 $value 超出 Android versionCode 上限 $MAX_BUILD_NUMBER"
  fi
}

calculate_candidate() {
  local now="$1"

  if ((now <= CUTOVER_FLOOR)); then
    printf '%s\n' "$((CUTOVER_FLOOR + 1))"
  else
    printf '%s\n' "$now"
  fi
}

wait_for_slot() {
  local candidate="$1"
  local previous_now="$2"
  local waited=0
  local now

  while :; do
    now="$(clock_now)"
    require_timestamp "$now"
    if ((now < previous_now)); then
      fail "系统 UTC 时钟发生回拨，拒绝分配移动构建号"
    fi
    previous_now="$now"
    if ((now > candidate)); then
      return 0
    fi
    if ((waited >= MAX_WAIT_SECONDS)); then
      fail "等待移动构建号时间槽超时（时钟未越过 ${candidate}）"
    fi
    sleep_seconds 1
    waited=$((waited + 1))
  done
}

main() {
  local channel="${1:-}"
  local now candidate

  [[ "$#" -eq 1 ]] || fail "用法: $0 <alpha|release>"
  [[ -n "${GITHUB_OUTPUT:-}" ]] || fail "缺少环境变量: GITHUB_OUTPUT"
  [[ "$channel" == alpha || "$channel" == release ]] \
    || fail "未知移动构建通道: ${channel}（必须是 alpha 或 release）"

  now="$(clock_now)"
  require_build_number_limit "$now"
  candidate="$(calculate_candidate "$now")"
  require_build_number_limit "$candidate"

  # 保持共享 workflow concurrency 锁直到候选秒结束，使下一次分配进入更晚时间槽。
  wait_for_slot "$candidate" "$now"

  printf 'build_number=%s\n' "$candidate" >>"$GITHUB_OUTPUT" \
    || fail "无法写入 GitHub Actions 输出文件"
  echo "已分配移动构建号: ${candidate}（${channel}）"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
