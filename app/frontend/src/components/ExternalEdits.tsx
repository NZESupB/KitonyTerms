import { useCallback, useEffect, useRef, useState } from "react";
import { Backend, useApp, type ExternalEdit } from "../lib/backend";
import { Modal } from "./Dialogs";
import { useText } from "../lib/useText";

export function useExternalEdits(enabled: boolean) {
  const [edits, setEdits] = useState<ExternalEdit[]>([]);
  const [wake, setWake] = useState(0);
  const [pending, setPending] = useState<string | null>(null);
  const pendingNow = useRef(false);
  const epoch = useRef(0);
  const mounted = useRef(false);
  const quitting = useApp((state) => state.quitting);
  useEffect(() => {
    mounted.current = true;
    if (!enabled || quitting)
      return () => {
        mounted.current = false;
      };
    let stopped = false;
    let running = false;
    let timer: ReturnType<typeof setTimeout>;
    let request: ReturnType<typeof Backend.GetExternalEdits> | undefined;
    const poll = async () => {
      if (stopped || running || document.visibilityState !== "visible") return;
      running = true;
      const version = epoch.current;
      request = Backend.GetExternalEdits();
      try {
        const value = (await request) ?? [];
        if (!stopped && version === epoch.current) setEdits(value);
      } catch (error) {
        if (!stopped && version === epoch.current)
          useApp.getState().report(error);
      } finally {
        running = false;
        request = undefined;
        if (!stopped && document.visibilityState === "visible")
          timer = setTimeout(() => void poll(), 750);
      }
    };
    const visibility = () => {
      clearTimeout(timer);
      if (document.visibilityState === "visible") void poll();
      else {
        epoch.current++;
        if (request) void request.cancel().catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      mounted.current = false;
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
      if (request) void request.cancel().catch(() => {});
    };
  }, [enabled, quitting, wake]);
  const act = useCallback(async (edit: ExternalEdit, action: string) => {
    if (pendingNow.current || useApp.getState().quitting) return;
    pendingNow.current = true;
    epoch.current++;
    setPending(edit.id);
    try {
      await Backend.ExternalEditAction(
        edit.sessionId,
        edit.generation,
        edit.id,
        action,
      );
    } catch (error) {
      if (mounted.current) useApp.getState().report(error);
    } finally {
      pendingNow.current = false;
      epoch.current++;
      if (mounted.current) {
        setPending(null);
        setWake((value) => value + 1);
      }
    }
  }, []);
  return { edits: enabled ? edits : [], act, pending };
}

export function ExternalEditPrompt({
  edits,
  pending,
  act,
}: {
  edits: ExternalEdit[];
  pending: string | null;
  act: (edit: ExternalEdit, action: string) => Promise<void>;
}) {
  const t = useText();
  const quitting = useApp((state) => state.quitting);
  const edit = edits.find(
    (value) =>
      value.status === "error" ||
      value.status === "retained" ||
      value.status === "failed" ||
      (value.status === "changed" && !value.autoUpload),
  );
  if (!edit || quitting) return null;
  const ended = edit.status === "retained" || edit.status === "failed";
  const busy = pending !== null;
  const stop = () => void act(edit, ended ? "dismiss" : "stop");
  return (
    <Modal
      open
      title={t(edit.status === "changed" ? "外部文件已保存" : "外部编辑")}
      description={t(
        edit.status === "failed"
          ? "未能创建本地编辑副本。"
          : ended
            ? "监听已结束。关闭记录会保留本地副本。"
            : "选择将本地修改上传一次，或在之后保存时自动上传。",
      )}
      onOpenChange={(open) => {
        if (!open && !busy) stop();
      }}
    >
      <p>
        <strong>{edit.remotePath}</strong>
      </p>
      {edit.localPath && (
        <p className="external-edit-path">
          {t("本地副本")}：<code>{edit.localPath}</code>
        </p>
      )}
      {edit.error && (
        <p className="error" role="alert">
          {edit.error}
        </p>
      )}
      <footer>
        {!ended && (
          <>
            <button
              className="primary"
              disabled={busy}
              onClick={() => void act(edit, "once")}
            >
              {t("上传一次")}
            </button>
            <button disabled={busy} onClick={() => void act(edit, "auto")}>
              {t("之后自动上传")}
            </button>
          </>
        )}
        <button disabled={busy} onClick={stop}>
          {t(ended ? "关闭记录" : "保留副本并结束")}
        </button>
      </footer>
    </Modal>
  );
}
