import { useEffect, useRef, useState } from "react";
import { Copy, X } from "lucide-react";
import {
  Backend,
  errorText,
  useApp,
  type CommandEntry,
  type SessionSnapshot,
} from "../lib/backend";
import { writeClipboard } from "../lib/clipboard";

import { useText } from "../lib/useText";
export function CommandHistory({
  session,
  onClose,
}: {
  session: SessionSnapshot;
  onClose: () => void;
}) {
  const t = useText();
  const [entries, setEntries] = useState<CommandEntry[]>([]);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [clearing, setClearing] = useState(false);
  const clearingNow = useRef(false);
  const mounted = useRef(false);
  const version = useRef(0);
  const pending = useRef<ReturnType<typeof Backend.GetCommandHistory>>();
  const refresh = useRef<() => void>();
  const quitting = useApp((state) => state.quitting);
  useEffect(() => {
    mounted.current = true;
    if (quitting)
      return () => {
        mounted.current = false;
      };
    let stopped = false;
    let running = false;
    let timer: ReturnType<typeof setTimeout>;
    let request: ReturnType<typeof Backend.GetCommandHistory> | undefined;
    const visible = () => document.visibilityState === "visible";
    const poll = async () => {
      if (stopped || running || clearingNow.current || !visible()) return;
      running = true;
      const revision = version.current;
      const query = Backend.GetCommandHistory(session.id, session.generation);
      request = query;
      pending.current = query;
      try {
        const value = (await query) ?? [];
        if (!stopped && revision === version.current) {
          setEntries(value);
          setError("");
        }
      } catch (error) {
        if (!stopped && revision === version.current)
          setError(errorText(error));
      } finally {
        if (pending.current === query) pending.current = undefined;
        running = false;
        request = undefined;
        if (!stopped && visible() && session.status === "connected")
          timer = setTimeout(() => void poll(), 1000);
      }
    };
    refresh.current = () => {
      clearTimeout(timer);
      void poll();
    };
    const visibility = () => {
      clearTimeout(timer);
      if (visible()) void poll();
      else {
        version.current++;
        if (request) void request.cancel().catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      stopped = true;
      mounted.current = false;
      refresh.current = undefined;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
      if (request) void request.cancel().catch(() => {});
    };
  }, [session.id, session.generation, session.status, quitting]);

  async function clear() {
    if (clearingNow.current) return;
    clearingNow.current = true;
    version.current++;
    setClearing(true);
    setError("");
    setMessage("");
    if (pending.current) void pending.current.cancel().catch(() => {});
    try {
      await Backend.ClearCommandHistory(session.id, session.generation);
      if (mounted.current) setEntries([]);
    } catch (error) {
      if (mounted.current) setError(errorText(error));
    } finally {
      clearingNow.current = false;
      if (mounted.current) {
        setClearing(false);
        refresh.current?.();
      }
    }
  }
  async function copy(command: string) {
    try {
      await writeClipboard(command);
      if (mounted.current) setMessage(t("已复制命令"));
    } catch (error) {
      if (mounted.current) setMessage(errorText(error));
    }
  }
  return (
    <section
      id="command-history-panel"
      className="command-history-panel"
      aria-label={t("命令历史")}
    >
      <header className="command-history-head">
        <strong>{t("命令历史")}</strong>
        <span className="command-history-count">{entries.length} / 200</span>
        <button
          disabled={clearing || !entries.length}
          onClick={() => void clear()}
        >
          {t("清空")}
        </button>
        <button aria-label={t("关闭命令历史")} onClick={onClose}>
          <X size={15} />
        </button>
      </header>
      <div className="command-history-list">
        {!entries.length && (
          <p className="muted">{t("当前会话暂无命令历史")}</p>
        )}
        {[...entries].reverse().map((entry) => (
          <div className="command-history-row" key={entry.sequence}>
            <time>{entry.time}</time>
            <code>{entry.command}</code>
            <button
              aria-label={t("复制命令")}
              title={t("复制")}
              onClick={() => void copy(entry.command)}
            >
              <Copy size={14} />
            </button>
          </div>
        ))}
      </div>
      {error && (
        <p className="error command-history-feedback" role="alert">
          {error}
        </p>
      )}
      {message && (
        <p className="command-history-feedback" role="status">
          {message}
        </p>
      )}
    </section>
  );
}
