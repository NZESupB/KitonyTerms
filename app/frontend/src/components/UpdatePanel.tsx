import { useEffect, useReducer, useRef } from "react";
import {
  Backend,
  useApp,
  type UpdatePreferences,
  type UpdateProxy,
} from "../lib/backend";
import { useText } from "../lib/useText";
import { formatBytes } from "../lib/transfers";
import {
  UpdateDraft,
  updateDate,
  updateFailures,
  updatePercent,
  updatePhases,
  updateDisabledReasons,
} from "../lib/update";
import type { TextKey } from "../lib/i18n";
import {
  reportInstall,
  startUpdateInstall,
  updateInstaller,
} from "../lib/update-install";

const formErrors: Record<string, TextKey> = {
  conflict: "更新偏好已被其他操作修改，请重新加载，或使用当前值覆盖。",
  save: "保存更新偏好失败，草稿已保留。",
  invalid: "更新代理的地址、端口或用户名无效。",
};

export function UpdatePanel({
  draft,
  canInstall,
}: {
  draft: UpdateDraft;
  canInstall: () => boolean;
}) {
  const t = useText();
  const language = useApp(
    (state) => state.config?.config.settings.language ?? "chinese",
  );
  const quitting = useApp((state) => state.quitting);
  const installing = useApp((state) => state.installing);
  const safe = useRef(canInstall);
  safe.current = canInstall;
  const [, render] = useReducer((value: number) => value + 1, 0);
  const mounted = useRef(true);
  const loadFailed = useRef(false);
  const actionFailed = useRef(false);
  const refresh = () => {
    if (mounted.current) render();
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      void updateInstaller.cancel().catch(reportInstall);
    };
  }, []);
  useEffect(() => {
    if (quitting) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let request: ReturnType<typeof Backend.GetUpdateState> | undefined;
    const poll = async () => {
      if (stopped || document.visibilityState !== "visible") return;
      const current = Backend.GetUpdateState();
      request = current;
      try {
        const snapshot = await current;
        if (!stopped && request === current) {
          draft.apply(snapshot);
          loadFailed.current = false;
          render();
        }
      } catch {
        if (!stopped && request === current) {
          loadFailed.current = true;
          render();
        }
      } finally {
        const owned = request === current;
        if (owned) request = undefined;
        if (owned && !stopped && document.visibilityState === "visible")
          timer = setTimeout(() => void poll(), 1000);
      }
    };
    const visibility = () => {
      clearTimeout(timer);
      const previous = request;
      request = undefined;
      if (previous) void previous.cancel().catch(() => undefined);
      if (document.visibilityState === "visible") void poll();
    };
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
      if (request) void request.cancel().catch(() => undefined);
    };
  }, [draft, quitting]);

  const snapshot = draft.snapshot;
  const preferences = draft.preferences;
  const locked =
    draft.saving || draft.action === "reload" || quitting || installing;
  const patch = (value: Partial<UpdatePreferences>) => {
    draft.edit(value);
    refresh();
  };
  const proxyPatch = (value: Partial<UpdateProxy>) => {
    if (preferences) patch({ proxy: { ...preferences.proxy, ...value } });
  };
  const save = async (overwrite = false) => {
    if (draft.action || quitting || useApp.getState().installing) return;
    if (overwrite) draft.rebase();
    const request = draft.beginSave();
    refresh();
    if (!request) return;
    try {
      draft.saved(await Backend.SaveUpdatePreferences(request));
    } catch (error) {
      draft.failed(error);
    }
    refresh();
  };
  const run = async (kind: "check" | "download" | "cancel" | "reload") => {
    if (
      draft.saving ||
      quitting ||
      useApp.getState().installing ||
      (draft.action && (kind !== "cancel" || draft.action === "cancel"))
    )
      return;
    if (
      kind === "reload" &&
      draft.dirty &&
      !window.confirm(t("重新加载会丢弃尚未保存的更新偏好，是否继续？"))
    )
      return;
    const token = ++draft.actionToken;
    draft.action = kind;
    actionFailed.current = false;
    refresh();
    try {
      const value = await (kind === "check"
        ? Backend.CheckUpdates()
        : kind === "download"
          ? Backend.DownloadUpdate()
          : kind === "cancel"
            ? Backend.CancelUpdate()
            : Backend.GetUpdateState());
      draft.apply(value);
      if (kind === "reload" && token === draft.actionToken) draft.reload();
    } catch {
      if (mounted.current && token === draft.actionToken)
        actionFailed.current = true;
    } finally {
      if (token === draft.actionToken) draft.action = "";
      refresh();
    }
  };
  const explicit =
    preferences && !["system", "direct"].includes(preferences.proxy.kind);
  const percent = snapshot ? updatePercent(snapshot) : undefined;
  const disabled = !snapshot || !!snapshot.disabledReason;
  return (
    <div className="settings-update">
      <section className="update-status" aria-label={t("更新状态")}>
        <strong role="status">
          {snapshot
            ? t(
                snapshot.phase === "idle" && !snapshot.lastCheck
                  ? "尚未检查更新"
                  : (updatePhases[snapshot.phase] ?? "更新状态不可用"),
              )
            : t("正在读取更新状态…")}
        </strong>
        {snapshot && (
          <>
            <p>
              {t("当前版本")}：{snapshot.current.version || "--"} ·{" "}
              {t(
                snapshot.current.channel === "stable"
                  ? "正式渠道"
                  : snapshot.current.channel === "alpha"
                    ? "Alpha 渠道"
                    : "开发构建",
              )}
            </p>
            <p className="muted">{snapshot.current.target || "--"}</p>
            {snapshot.disabledReason && (
              <p className="muted">
                {t(
                  updateDisabledReasons[snapshot.disabledReason] ??
                    "更新状态不可用",
                )}
              </p>
            )}
            {snapshot.phase === "downloaded" && (
              <>
                <p className="muted">
                  {t(
                    snapshot.installAvailable
                      ? "安装会断开当前 SSH 连接并重启应用。"
                      : "此构建暂不支持自动安装。",
                  )}
                </p>
                {snapshot.requiresElevation && (
                  <p className="muted">
                    {t("安装时 Windows 将请求管理员授权。")}
                  </p>
                )}
              </>
            )}
            <p className="muted">
              {t("上次检查")}：
              {snapshot.lastCheck
                ? updateDate(snapshot.lastCheck, language)
                : t("尚未检查更新")}
            </p>
            {snapshot.failure && (
              <p className="error" role="alert">
                {t(updateFailures[snapshot.failure.kind] ?? "更新任务失败")}
                {snapshot.failure.status
                  ? ` · HTTP ${snapshot.failure.status}`
                  : ""}
                {snapshot.failure.retryAt
                  ? ` · ${t("可重试时间")}：${updateDate(snapshot.failure.retryAt, language)}`
                  : ""}
              </p>
            )}
            {snapshot.notice === "invalid_proxy" && (
              <p className="error" role="alert">
                {t("已保存的更新代理无效，当前使用系统代理。")}
              </p>
            )}
            {snapshot.available && (
              <>
                <p>
                  {t("可用版本")}：{snapshot.available.info.version} ·{" "}
                  {formatBytes(snapshot.available.package.size)}
                </p>
                {["downloading", "cancelling", "downloaded"].includes(
                  snapshot.phase,
                ) && (
                  <div className="update-progress">
                    <progress
                      max={100}
                      value={percent}
                      aria-label={t("更新下载进度")}
                    />
                    <p>
                      {formatBytes(snapshot.received)} /{" "}
                      {formatBytes(snapshot.available.package.size)}
                      {snapshot.phase === "downloading"
                        ? ` · ${formatBytes(snapshot.speed)}/s`
                        : ""}
                    </p>
                  </div>
                )}
                <details className="update-notes">
                  <summary>{t("发行说明")}</summary>
                  <p>{snapshot.available.notes || t("该发布未提供说明。")}</p>
                  {snapshot.available.notesTruncated && (
                    <small className="muted">
                      {t("发行说明过长，已截断。")}
                    </small>
                  )}
                </details>
              </>
            )}
          </>
        )}
        {loadFailed.current && (
          <p className="error" role="alert">
            {t("读取更新状态失败，请重试。")}
          </p>
        )}
        {actionFailed.current && (
          <p className="error" role="alert">
            {t("更新操作未完成，请刷新状态后重试。")}
          </p>
        )}
        <div className="update-actions">
          <button
            type="button"
            disabled={disabled || locked || !!draft.action || snapshot?.busy}
            onClick={() => void run("check")}
          >
            {t("检查更新")}
          </button>
          {snapshot?.phase === "downloaded" && (
            <button
              type="button"
              className="primary"
              disabled={
                disabled ||
                locked ||
                !!draft.action ||
                draft.dirty ||
                !snapshot.installAvailable
              }
              onClick={() => {
                void startUpdateInstall(
                  () =>
                    mounted.current &&
                    safe.current() &&
                    !draft.saving &&
                    !draft.dirty &&
                    !draft.action,
                ).catch(reportInstall);
              }}
            >
              {t("重启并安装")}
            </button>
          )}
          {snapshot?.available && snapshot.phase !== "downloaded" && (
            <button
              type="button"
              disabled={disabled || locked || !!draft.action || snapshot.busy}
              onClick={() => void run("download")}
            >
              {t("下载更新包")}
            </button>
          )}
          {snapshot?.busy && !snapshot.phase.startsWith("install_") && (
            <button
              type="button"
              disabled={
                locked ||
                draft.action === "cancel" ||
                snapshot.phase === "cancelling"
              }
              onClick={() => void run("cancel")}
            >
              {t("取消更新任务")}
            </button>
          )}
          {!snapshot && (
            <button
              type="button"
              disabled={!!draft.action || quitting}
              onClick={() => void run("reload")}
            >
              {t("重新加载")}
            </button>
          )}
        </div>
      </section>
      {preferences && (
        <form
          className="update-preferences"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <h3>{t("本机更新偏好")}</h3>
          <fieldset disabled={locked}>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={preferences.auto_check}
                onChange={(event) =>
                  patch({ auto_check: event.target.checked })
                }
              />
              {t("自动检查更新")}
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={preferences.auto_download}
                onChange={(event) =>
                  patch({ auto_download: event.target.checked })
                }
              />
              {t("自动下载更新")}
            </label>
            <label>
              {t("更新代理")}
              <select
                value={preferences.proxy.kind}
                onChange={(event) =>
                  patch({
                    proxy: ["system", "direct"].includes(event.target.value)
                      ? { kind: event.target.value }
                      : {
                          kind: event.target.value,
                          host: "",
                          port: 8080,
                          username: "",
                        },
                  })
                }
              >
                <option value="system">{t("使用系统代理")}</option>
                <option value="direct">{t("更新强制直连")}</option>
                <option value="http">HTTP</option>
                <option value="https">HTTPS</option>
                <option value="socks5">SOCKS5</option>
              </select>
            </label>
            {explicit && (
              <>
                <label>
                  {t("更新代理主机")}
                  <input
                    autoComplete="off"
                    maxLength={255}
                    value={preferences.proxy.host ?? ""}
                    onChange={(event) =>
                      proxyPatch({ host: event.target.value })
                    }
                  />
                </label>
                <label>
                  {t("更新代理端口")}
                  <input
                    type="number"
                    min={1}
                    max={65535}
                    step={1}
                    value={preferences.proxy.port || ""}
                    onChange={(event) =>
                      proxyPatch({
                        port:
                          event.target.value === ""
                            ? 0
                            : Number(event.target.value),
                      })
                    }
                  />
                </label>
                <label>
                  {t("更新代理用户名")}
                  <input
                    autoComplete="off"
                    maxLength={128}
                    value={preferences.proxy.username ?? ""}
                    onChange={(event) =>
                      proxyPatch({ username: event.target.value })
                    }
                  />
                </label>
              </>
            )}
          </fieldset>
          <p className="muted">{t("仅保存在当前设备，不参与配置同步。")}</p>
          <p className="muted">
            {t(
              "显式代理只保存用户名；需要密码时请使用系统代理和代理环境变量。",
            )}
          </p>
          {draft.error && (
            <p className="error" role="alert">
              {t(formErrors[draft.error])}
            </p>
          )}
          <div className="update-actions">
            <p role="status" className="muted">
              {t(
                draft.saving
                  ? "保存中…"
                  : draft.dirty
                    ? "有尚未保存的更新偏好"
                    : "更新偏好已保存",
              )}
            </p>
            {draft.error === "conflict" && (
              <button
                type="button"
                disabled={locked || !!draft.action}
                onClick={() => void save(true)}
              >
                {t("使用当前值")}
              </button>
            )}
            <button
              type="button"
              disabled={locked || !!draft.action}
              onClick={() => void run("reload")}
            >
              {t("重新加载更新偏好")}
            </button>
            <button
              type="submit"
              className="primary"
              disabled={locked || !!draft.action || !draft.dirty}
            >
              {t("保存更新偏好")}
            </button>
          </div>
        </form>
      )}
    </div>
  );
}
