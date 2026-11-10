import type {
  SaveUpdatePreferencesRequest,
  UpdatePreferences,
  UpdateProxy,
  UpdateSnapshot,
} from "./backend";
import { newerRevision } from "./protocol";
import { transferPercent } from "./transfers";
import type { TextKey } from "./i18n";

export const updatePhases: Record<string, TextKey> = {
  disabled: "此构建未启用更新",
  idle: "暂无可用更新",
  checking: "正在检查更新…",
  cancelled: "更新检查已取消",
  available: "发现可用更新",
  downloading: "正在下载并校验…",
  cancelling: "正在取消更新任务…",
  downloaded: "更新包已下载并校验",
  install_preparing: "正在准备更新安装…",
  install_ready: "正在准备更新安装…",
  install_committed: "安装已提交，正在退出…",
  error: "更新任务失败",
};
export const updateFailures: Record<string, TextKey> = {
  metadata: "更新发布信息不匹配",
  size: "更新响应或文件大小不匹配",
  digest: "更新文件摘要校验失败",
  rate_limited: "更新请求受限，请等待后重试",
  timeout: "更新请求超时",
  connection: "无法连接更新服务或代理",
  tls: "更新服务或代理的 TLS 身份验证失败",
  proxy_auth: "更新代理认证失败",
  redirect: "更新跳转已被拒绝",
  http_status: "更新服务返回错误状态",
  storage: "更新文件无法保存或读取",
  install_prepare: "更新安装准备或提交失败，应用继续运行。",
  install_cleanup: "存在待恢复的安装材料，请重启应用后再试。",
  install_failed: "上次更新安装失败，恢复材料已保留。",
  install_record: "安装失败记录无法读取，原始材料已保留。",
};

export const updateDisabledReasons: Record<string, TextKey> = {
  development: "开发构建暂未启用自动更新。",
  unsupported_platform: "此平台没有可用的桌面更新目标。",
  not_app_bundle: "请先将完整 KitonyTerms.app 安装到固定可写目录。",
  mounted_installation: "请先将应用从镜像复制到可写目录。",
  translocated_installation: "请先将完整 KitonyTerms.app 安装到固定可写目录。",
  installation_unavailable: "当前安装目录不可访问，请检查后重启应用。",
  readonly_installation: "应用文件为只读，请先移除只读属性。",
  installation_unwritable: "当前安装目录不可写，请将应用移到可写目录。",
  pending_installation: "存在待恢复的安装材料，请重启应用后再试。",
};
export function copyUpdatePreferences(
  value: UpdatePreferences,
): UpdatePreferences {
  const p = value.proxy;
  const proxy: UpdateProxy =
    p.kind === "system" || p.kind === "direct"
      ? { kind: p.kind }
      : {
          kind: p.kind,
          host: p.host ?? "",
          port: p.port ?? 0,
          username: p.username ?? "",
        };
  return {
    auto_check: value.auto_check,
    auto_download: value.auto_download,
    proxy,
  };
}
function containsControl(value: string): boolean {
  return Array.from(value).some((char) => {
    const code = char.codePointAt(0)!;
    return code < 32 || (code >= 127 && code <= 159);
  });
}
export function validUpdatePreferences(value: UpdatePreferences): boolean {
  const p = value.proxy;
  if (p.kind === "system" || p.kind === "direct") return true;
  return (
    ["http", "https", "socks5"].includes(p.kind) &&
    !!p.host &&
    new TextEncoder().encode(p.host).length <= 255 &&
    !containsControl(p.host) &&
    !/[\s/@?#\\]/u.test(p.host) &&
    Number.isInteger(p.port) &&
    (p.port ?? 0) > 0 &&
    (p.port ?? 0) <= 65535 &&
    new TextEncoder().encode(p.username ?? "").length <= 128 &&
    (p.username ?? "").trim() === (p.username ?? "") &&
    !containsControl(p.username ?? "")
  );
}

// 生命周期归设置面板，分类卸载或语言变化不会丢掉草稿和在途保存。
export class UpdateDraft {
  snapshot: UpdateSnapshot | null = null;
  preferences: UpdatePreferences | null = null;
  saving = false;
  action = "";
  actionToken = 0;
  error: "save" | "conflict" | "invalid" | null = null;
  private baseline: UpdatePreferences | null = null;
  private revision = "";

  get dirty(): boolean {
    return (
      this.preferences !== null &&
      JSON.stringify(copyUpdatePreferences(this.preferences)) !==
        JSON.stringify(this.baseline)
    );
  }
  apply(value: UpdateSnapshot): boolean {
    if (
      !/^\d+$/.test(value.sequence) ||
      !/^\d+$/.test(value.preferenceRevision)
    ) {
      throw new Error("更新快照版本无效");
    }
    if (
      this.snapshot &&
      (value.sequence === this.snapshot.sequence ||
        !newerRevision(this.snapshot.sequence, value.sequence))
    )
      return false;
    this.snapshot = value;
    if (!this.dirty && !this.saving) this.reload();
    else if (!this.saving && value.preferenceRevision !== this.revision)
      this.error = "conflict";
    return true;
  }
  edit(patch: Partial<UpdatePreferences>): void {
    if (!this.preferences || this.saving) return;
    this.preferences = copyUpdatePreferences({ ...this.preferences, ...patch });
    this.error = null;
  }
  reload(): void {
    if (!this.snapshot || this.saving) return;
    this.preferences = copyUpdatePreferences(this.snapshot.preferences);
    this.baseline = copyUpdatePreferences(this.snapshot.preferences);
    this.revision = this.snapshot.preferenceRevision;
    this.error = null;
  }
  rebase(): void {
    if (!this.snapshot || this.saving) return;
    this.baseline = copyUpdatePreferences(this.snapshot.preferences);
    this.revision = this.snapshot.preferenceRevision;
    this.error = null;
  }
  beginSave(): SaveUpdatePreferencesRequest | null {
    if (!this.preferences || this.saving || !this.dirty) return null;
    if (!validUpdatePreferences(this.preferences)) {
      this.error = "invalid";
      return null;
    }
    this.saving = true;
    this.error = null;
    return {
      expectedRevision: this.revision,
      preferences: copyUpdatePreferences(this.preferences),
    };
  }
  saved(value: UpdateSnapshot): void {
    this.apply(value);
    this.saving = false;
    this.reload();
  }
  failed(error: unknown): void {
    this.saving = false;
    const message = error instanceof Error ? error.message : String(error);
    this.error = message.includes("版本冲突")
      ? "conflict"
      : message.includes("更新代理") && !message.includes("保存更新偏好失败")
        ? "invalid"
        : "save";
  }
}

export function updatePercent(value: UpdateSnapshot): number | undefined {
  if (!value.available) return undefined;
  return transferPercent({
    status: value.phase === "downloaded" ? "succeeded" : "running",
    bytes: value.received,
    totalBytes: value.available.package.size,
  });
}
export function updateDate(value: string, language: string): string {
  if (!/^\d+$/.test(value)) return "--";
  const seconds = Number(value);
  const date = new Date(seconds * 1000);
  if (!Number.isSafeInteger(seconds) || !Number.isFinite(date.getTime()))
    return "--";
  return date.toLocaleString(language === "english" ? "en-US" : "zh-CN");
}
