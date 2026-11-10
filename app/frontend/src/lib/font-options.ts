// 未识别的历史配置仍须作为完整值保留，不能把逗号解释成多项。
export function fontOptions(
  current: string,
  families: readonly string[],
): string[] {
  return current.trim() && !families.includes(current)
    ? [current, ...families]
    : [...families];
}
