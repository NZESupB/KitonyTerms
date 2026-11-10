import { useEffect, useRef, useState } from "react";
import { Events } from "@wailsio/runtime";
import { Backend, useApp, type QuitState } from "../lib/backend";
import { Modal } from "./Dialogs";
import { useText } from "../lib/useText";
import { reportInstall, updateInstaller } from "../lib/update-install";

export function QuitProtection() {
  const t = useText();
  const dirty = useApp(
    (state) => Object.values(state.dirtyEditors).filter(Boolean).length,
  );
  const [state, setState] = useState<QuitState | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const mounted = useRef(false);
  const epoch = useRef(0);
  const refreshState = useRef<(() => void) | undefined>();
  useEffect(() => {
    mounted.current = true;
    let stopped = false;
    let querying = false;
    const queries = new Set<ReturnType<typeof Backend.GetQuitState>>();
    function refresh() {
      if (pending.current || querying || document.visibilityState !== "visible")
        return;
      querying = true;
      const owner = ++epoch.current;
      const query = Backend.GetQuitState();
      queries.add(query);
      void query
        .then((value) => {
          if (stopped || epoch.current !== owner) return;
          if (value.intent === "install") {
            setState(null);
            // 本地仍持有准备调用时不接管；新页面才取消未提交的遗留请求。
            void updateInstaller.recover(value).catch(reportInstall);
          } else {
            setState(value.requestId ? value : null);
            if (value.preparing) useApp.setState({ quitting: true });
          }
          void updateInstaller.reconcile().catch(() => undefined);
        })
        .catch((error: unknown) => {
          if (!stopped && epoch.current === owner)
            useApp.getState().report(error);
        })
        .finally(() => {
          querying = false;
          queries.delete(query);
        });
    }
    refreshState.current = refresh;
    const off = Events.On("app:close-requested", refresh);
    const timer = setInterval(refresh, 1000);
    const visible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", visible);
    refresh();
    return () => {
      mounted.current = false;
      refreshState.current = undefined;
      stopped = true;
      off();
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visible);
      for (const query of queries) void query.cancel().catch(() => undefined);
    };
  }, []);
  async function answer(confirm: boolean) {
    if (
      pending.current ||
      !state ||
      state.preparing ||
      (confirm && state.protectedOperations > 0)
    )
      return;
    pending.current = true;
    epoch.current++;
    setBusy(true);
    const id = state.requestId;
    if (confirm) useApp.setState({ quitting: true, error: "" });
    try {
      if (confirm) {
        await Backend.ConfirmQuit(id);
        if (mounted.current)
          setState((value) =>
            value?.requestId === id ? { ...value, preparing: true } : value,
          );
      } else {
        await Backend.CancelQuit(id);
        if (mounted.current)
          setState((value) => (value?.requestId === id ? null : value));
      }
    } catch (error) {
      if (confirm) useApp.setState({ quitting: false });
      if (mounted.current) useApp.getState().report(error);
    } finally {
      pending.current = false;
      if (mounted.current) {
        setBusy(false);
        refreshState.current?.();
      }
    }
  }
  if (!state) return null;
  const locked = busy || state.preparing;
  const activity = state.activity;
  return (
    <Modal
      open
      title={t("退出 KitonyTerms")}
      description={t("退出将关闭连接并停止正在进行的任务。")}
      onOpenChange={(open) => {
        if (!open && !locked) void answer(false);
      }}
    >
      {dirty > 0 && (
        <p className="error" role="alert">
          {t("{count} 个远端文件尚未保存，退出将丢弃未保存内容。", {
            count: dirty,
          })}
        </p>
      )}
      <ul>
        <li>
          {t("连接")}：{activity.sessions}
        </li>
        <li>
          {t("文件传输：{count}，取消后保留已完成文件", {
            count: activity.transfers,
          })}
        </li>
        <li>{t("运维请求：{count}", { count: activity.operations })}</li>
        <li>
          {t("容器终端：{count}", { count: activity.containerTerminals })}
        </li>
        {state.externalEdits > 0 && (
          <li>
            {t("外部编辑：{count}，停止监听并保留未上传副本", {
              count: state.externalEdits,
            })}
          </li>
        )}
        {state.sharing && <li>{t("本机配置分享将停止")}</li>}
      </ul>
      {state.protectedOperations > 0 && (
        <p role="status">
          {t("在途文件选择、保存或同步：{count}，请等待完成", {
            count: state.protectedOperations,
          })}
        </p>
      )}
      {state.preparing && <p role="status">{t("正在停止任务并回收连接…")}</p>}
      <footer>
        <button disabled={locked} onClick={() => void answer(false)}>
          {t("继续使用")}
        </button>
        <button
          className="danger"
          disabled={locked || state.protectedOperations > 0}
          onClick={() => void answer(true)}
        >
          {t(locked ? "退出中…" : "退出")}
        </button>
      </footer>
    </Modal>
  );
}
