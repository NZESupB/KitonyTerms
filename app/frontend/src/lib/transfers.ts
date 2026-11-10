import type { TransferProgress } from "./backend";

export function formatBytes(value: string): string {
  if (!/^\d+$/.test(value)) return "--";
  const bytes = BigInt(value);
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  let unit = 0;
  let divisor = 1n;
  while (unit < units.length - 1 && bytes >= divisor * 1024n) {
    divisor *= 1024n;
    unit++;
  }
  if (!unit) return `${bytes} B`;
  const tenths = (bytes * 10n) / divisor;
  return `${tenths / 10n}.${tenths % 10n} ${units[unit]}`;
}
export const transferFinished = (status: string) =>
  ["succeeded", "failed", "cancelled"].includes(status);

export function transferPercent(
  value: Pick<TransferProgress, "status" | "bytes" | "totalBytes">,
): number | undefined {
  if (value.status === "succeeded") return 100;
  if (!/^\d+$/.test(value.totalBytes) || !/^\d+$/.test(value.bytes))
    return undefined;
  const total = BigInt(value.totalBytes);
  if (total === 0n) return 0;
  return Math.min(99, Number((BigInt(value.bytes) * 100n) / total));
}
