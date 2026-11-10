import { formatBytes } from "./transfers";

export const clampPercent = (value: number | null | undefined): number =>
  value == null || !Number.isFinite(value)
    ? 0
    : Math.max(0, Math.min(100, value));

export function metricPercent(used: string, total: string): number | null {
  if (!/^\d+$/.test(used) || !/^\d+$/.test(total) || BigInt(total) === 0n)
    return null;
  return clampPercent(Number((BigInt(used) * 1000n) / BigInt(total)) / 10);
}
export const metricRate = (value: string): string =>
  value === "" ? "--" : `${formatBytes(value)}/s`;

export function metricUptime(value: string): string {
  if (!/^\d+$/.test(value)) return "--";
  const seconds = BigInt(value);
  if (seconds >= 86400n)
    return `${seconds / 86400n}d ${(seconds % 86400n) / 3600n}h`;
  if (seconds >= 3600n) return `${seconds / 3600n}h`;
  if (seconds >= 60n) return `${seconds / 60n}m`;
  return `${seconds}s`;
}
export function latencyLevel(value: string): string {
  if (!/^\d+$/.test(value) || BigInt(value) === 0n) return "unknown";
  const latency = BigInt(value);
  return latency >= 300n ? "danger" : latency >= 150n ? "warn" : "normal";
}
export function networkActivity(rx: string, tx: string): number | null {
  if (!/^\d+$/.test(rx) || !/^\d+$/.test(tx)) return null;
  const total = BigInt(rx) + BigInt(tx);
  const limits = [0n, 65536n, 1048576n, 10485760n, 104857600n];
  for (let index = 1; index < limits.length; index++) {
    if (total <= limits[index])
      return (
        (index - 1) * 25 +
        Number(
          ((total - limits[index - 1]) * 2500n) /
            (limits[index] - limits[index - 1]),
        ) /
          100
      );
  }
  return 100;
}
