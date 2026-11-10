import { useEffect, useReducer, useRef } from "react";
import {
  Backend,
  errorText,
  useApp,
  type ContainerTerminalSnapshot,
  type SessionSnapshot,
} from "../lib/backend";
import { TerminalView } from "./TerminalView";
import "./operations-content.css";
import { useText } from "../lib/useText";

// 生命周期保存在运维缓存中；抽屉卸载不会取消启动或回收 PTY。
export type ContainerTerminalMemory = {
  snapshot: ContainerTerminalSnapshot;
  starting?: Promise<void>;
  closing?: Promise<void>;
  error: string;
  released?: boolean;
};

export function ContainerTerminalPanel({
  session,
  memory,
  onClosed,
}: {
  session: SessionSnapshot;
  memory: ContainerTerminalMemory;
  onClosed: (memory: ContainerTerminalMemory) => void;
}) {
  const t = useText();
  const settings = useApp((state) => state.config?.config.settings);
  const [, redraw] = useReducer((value: number) => value + 1, 0);
  const mounted = useRef(false);
  const execId = memory.snapshot.execId;
  const closeCallback = useRef(onClosed);
  closeCallback.current = onClosed;
  useEffect(() => {
    mounted.current = true;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    let query: ReturnType<typeof Backend.GetContainerTerminal> | undefined;
    async function poll() {
      await memory.starting;
      if (stopped) return;
      if (memory.released) {
        closeCallback.current(memory);
        return;
      }
      if (session.status !== "connected") {
        memory.snapshot = { ...memory.snapshot, status: "closed" };
        redraw();
        return;
      }
      try {
        query = Backend.GetContainerTerminal(
          session.id,
          session.generation,
          execId,
        );
        const snapshot = await query;
        if (stopped || snapshot.execId !== execId) return;
        memory.snapshot = snapshot;
        redraw();
      } catch (error) {
        if (!stopped) {
          memory.error = errorText(error);
          redraw();
        }
      }
      if (!stopped) timer = setTimeout(() => void poll(), 500);
    }
    void poll();
    return () => {
      stopped = true;
      mounted.current = false;
      clearTimeout(timer);
      void query?.cancel().catch(() => {});
    };
  }, [session.id, session.generation, session.status, execId, memory]);

  function close() {
    if (memory.closing) return;
    memory.error = "";
    memory.closing = (async () => {
      // 先等启动请求返回，确保关闭对应的已接受 PTY。
      await memory.starting;
      if (session.status === "connected") {
        const current = await Backend.GetContainerTerminal(
          session.id,
          session.generation,
          "",
        );
        if (current.execId === execId)
          await Backend.CloseContainerTerminal(
            session.id,
            session.generation,
            execId,
          );
      }
      memory.released = true;
      onClosed(memory);
    })()
      .catch((error: unknown) => {
        memory.error = errorText(error);
      })
      .finally(() => {
        memory.closing = undefined;
        if (mounted.current) redraw();
      });
    redraw();
  }

  const snapshot = memory.snapshot;
  const running = snapshot.status === "running" && !memory.closing;
  return (
    <section
      className="operations-container-terminal"
      aria-label={t("容器终端")}
    >
      <header className="toolbar">
        <strong title={snapshot.container}>
          {snapshot.container} · {t("终端")}
        </strong>
        <button disabled={!!memory.closing} onClick={close}>
          {t(memory.closing ? "关闭中…" : "关闭终端")}
        </button>
      </header>
      <p className="operations-message muted" role="status">
        {t(
          memory.closing
            ? "正在回收终端…"
            : memory.starting || snapshot.status === "starting"
              ? "正在连接容器…"
              : running
                ? "已连接"
                : "终端已结束",
        )}
      </p>
      {(memory.error || snapshot.error) && (
        <p className="operations-message error" role="alert">
          {memory.error || snapshot.error}
        </p>
      )}
      <div className="operations-container-terminal-body">
        {settings && !memory.starting && (
          <TerminalView
            key={execId}
            execId={execId}
            session={{
              ...session,
              title: snapshot.container,
              status: running ? "connected" : "closed",
            }}
            settings={settings}
            active
          />
        )}
      </div>
    </section>
  );
}
