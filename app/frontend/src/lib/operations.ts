import type { OperationRequest, OperationsCapabilities } from "./backend";
import type { TextKey } from "./i18n";

export type OperationSpec = Partial<Omit<OperationRequest, "id">> & {
  kind: string;
};
export function operationRequest(spec: OperationSpec): OperationRequest {
  return {
    id: crypto.randomUUID(),
    resource: "",
    target: "",
    action: "",
    pid: 0,
    lines: 200,
    targets: [],
    ...spec,
  };
}
export function listOperation(kind: string, resource: string): OperationSpec {
  return kind === "docker" ? { kind: "docker_list", resource } : { kind };
}
export function operationSupported(
  c: OperationsCapabilities | undefined,
  kind: string,
  resource: string,
): boolean {
  if (!c?.linux) return false;
  switch (kind) {
    case "services":
      return c.systemd;
    case "processes":
      return c.ps;
    case "network":
      return c.ss;
    case "docker":
      return c.docker_daemon && (resource !== "compose" || c.compose);
    default:
      return kind === "status";
  }
}
export const operationErrorLabels: Record<string, TextKey> = {
  cancelled: "已取消",
  timeout: "超时",
  permission_denied: "权限不足",
  unsupported_backend: "后端不可用",
  command_missing: "缺少命令或资源",
  output_limit: "输出超限",
  parse_failed: "响应无效",
  disconnected: "连接已失效",
  busy: "请求繁忙",
  invalid_request: "请求无效",
  command_failed: "执行失败",
};
