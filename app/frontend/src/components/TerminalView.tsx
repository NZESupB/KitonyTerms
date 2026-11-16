import { useEffect, useRef, useState } from "react";
import * as ContextMenu from "@radix-ui/react-context-menu";
import {
  Copy,
  ClipboardPaste,
  History,
  LoaderCircle,
  MousePointer2,
} from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import {
  Backend,
  errorText,
  useApp,
  type SessionSnapshot,
  type Settings,
} from "../lib/backend";
import { base64ToBytes, bytesToBase64, OrderedInput } from "../lib/protocol";
import { readClipboard, writeClipboard } from "../lib/clipboard";
import { terminalFontFamily } from "../lib/appearance";
import {
  attachTerminalAnnotations,
  terminalColumns,
} from "../lib/terminal-display";
import { useText } from "../lib/useText";
import { translate, type TextKey } from "../lib/i18n";

// 返回普通缓冲区并解除远端遗留的交互模式，不清除滚动历史。
const disconnectModes =
  "\x1b[!p\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?25h\x1b[0m";
function terminalText(value: string): string {
  return Array.from(value.slice(0, 8192).replace(/\r\n?/g, "\n"))
    .filter((char) => {
      const code = char.codePointAt(0)!;
      return char === "\n" || (code >= 32 && (code < 127 || code > 159));
    })
    .join("")
    .replace(/\n/g, "\r\n");
}

export function TerminalView({
  session,
  active,
  settings,
  execId,
  onToggleHistory,
  onReconnect,
  reconnectDisabled = false,
  profileAvailable = true,
}: {
  session: SessionSnapshot;
  active: boolean;
  settings: Settings;
  execId?: string;
  onToggleHistory?: () => void;
  onReconnect?: () => Promise<void>;
  reconnectDisabled?: boolean;
  profileAvailable?: boolean;
}) {
  const t = useText();
  const surface = useRef<HTMLDivElement>(null);
  const gutter = useRef<HTMLDivElement>(null);
  const highlights = useRef<HTMLDivElement>(null);
  const annotations = useRef<ReturnType<typeof attachTerminalAnnotations>>();
  const resizeTerminal = useRef<(focus?: boolean) => void>();
  const sendTerminalSize = useRef<() => void>();
  const refreshStatus = useRef<() => void>();
  const container = useRef<HTMLDivElement>(null);
  const [hasSelection, setHasSelection] = useState(false);
  const [pendingReconnect, setPendingReconnect] = useState<string | null>(null);
  const connectionOwner = `${session.id}:${session.generation}`;
  const showConnecting =
    active &&
    !execId &&
    !!onReconnect &&
    (session.status === "connecting" ||
      (session.status === "closed" && pendingReconnect === connectionOwner));
  const terminal = useRef<Terminal>();
  const currentSession = useRef(session);
  currentSession.current = session;
  const currentActive = useRef(active);
  currentActive.current = active;
  const reconnectOptions = useRef({
    onReconnect,
    reconnectDisabled,
    profileAvailable,
  });
  reconnectOptions.current = {
    onReconnect,
    reconnectDisabled,
    profileAvailable,
  };
  const pasteVersion = useRef(0);
  useEffect(() => {
    const invalidate = () => {
      pasteVersion.current++;
    };
    document.addEventListener("visibilitychange", invalidate);
    return () => {
      invalidate();
      document.removeEventListener("visibilitychange", invalidate);
    };
  }, [active, session.id, session.generation, session.status, execId]);
  const initialSettings = useRef(settings);
  initialSettings.current = settings;
  useEffect(() => {
    if (!container.current || !gutter.current || !highlights.current) return;
    let stopped = false;
    const report = (error: unknown) => {
      if (!stopped) useApp.getState().report(error);
    };
    const options = initialSettings.current;
    const term = new Terminal({
      cursorBlink: true,
      fontFamily: terminalFontFamily(options.font_family),
      fontSize: options.font_size,
      scrollback: options.scrollback_lines,
      allowProposedApi: false,
    });
    const addon = new FitAddon();
    term.loadAddon(addon);
    const applyTheme = () => {
      const style = getComputedStyle(document.documentElement);
      term.options.theme = {
        background: style.getPropertyValue("--terminal-bg").trim(),
        foreground: style.getPropertyValue("--terminal-fg").trim(),
        cursor: style.getPropertyValue("--terminal-fg").trim(),
      };
    };
    applyTheme();
    const appearance = new MutationObserver(applyTheme);
    appearance.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "style"],
    });
    terminal.current = term;
    const display = attachTerminalAnnotations(
      term,
      gutter.current,
      highlights.current,
      () => initialSettings.current,
    );
    annotations.current = display;
    let frame = 0;
    let layoutVersion = 0;
    let focusRequested = false;
    let fontLoad: { key: string; promise: Promise<FontFace[]> } | undefined;
    const measurable = () =>
      !!container.current?.clientWidth && !!container.current?.clientHeight;
    const applyLayout = async (version: number) => {
      if (stopped || !measurable()) return;
      const current = initialSettings.current;
      const family = terminalFontFamily(current.font_family);
      const key = `${current.font_size}px ${family}`;
      if (fontLoad?.key !== key)
        fontLoad = { key, promise: document.fonts.load(key) };
      await fontLoad.promise;
      if (stopped || version !== layoutVersion || !measurable()) return;
      term.options.fontFamily = family;
      term.options.fontSize = current.font_size;
      // 隐藏容器的 DOM 字宽为零，必须等布局恢复后再打开渲染器。
      if (!term.element) term.open(container.current!);
      const size = addon.proposeDimensions();
      if (size)
        term.resize(
          terminalColumns(
            size.cols,
            current.terminal_wrap,
            matchMedia("(max-width: 599px), (max-height: 599px)").matches,
          ),
          size.rows,
        );
      display.refresh();
      sendTerminalSize.current?.();
      if (
        focusRequested &&
        currentActive.current &&
        !document.querySelector('[role="dialog"]') &&
        (!document.activeElement?.matches("input, textarea, select") ||
          term.element?.contains(document.activeElement))
      )
        term.focus();
      focusRequested = false;
    };
    const resize = (focus = false) => {
      focusRequested ||= focus;
      layoutVersion++;
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          void applyLayout(layoutVersion).catch(report);
        });
    };
    resizeTerminal.current = resize;
    const observer = new ResizeObserver(() => resize());
    observer.observe(container.current);
    if (surface.current) observer.observe(surface.current);
    const onResize = term.onResize(() => sendTerminalSize.current?.());
    return () => {
      stopped = true;
      layoutVersion++;
      cancelAnimationFrame(frame);
      observer.disconnect();
      appearance.disconnect();
      onResize.dispose();
      display.dispose();
      annotations.current = undefined;
      resizeTerminal.current = undefined;
      term.dispose();
      terminal.current = undefined;
    };
  }, [session.id, execId]);

  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    let after = "0";
    let drained = false;
    let disconnected = false;
    let reconnecting = false;
    let failure = "";
    let available: boolean | undefined;
    const pending = new Set<{ cancel: () => Promise<unknown> }>();
    const isCurrent = () =>
      !stopped &&
      currentSession.current.id === session.id &&
      currentSession.current.generation === session.generation;
    const text = (key: TextKey) =>
      translate(initialSettings.current.language, key);
    const report = (error: unknown) => {
      if (isCurrent() && currentSession.current.status === "connected")
        useApp.getState().report(error);
    };
    const track = async <T,>(
      request: Promise<T> & { cancel: () => Promise<unknown> },
    ): Promise<T> => {
      pending.add(request);
      try {
        return await request;
      } finally {
        pending.delete(request);
      }
    };
    const hint = () =>
      text(
        reconnectOptions.current.profileAvailable
          ? "按 Enter 重新连接。"
          : "此会话的连接配置已删除或改名，请在连接管理中选择或新建连接。",
      );
    const showStatus = () => {
      if (!isCurrent() || execId || !reconnectOptions.current.onReconnect)
        return;
      const current = currentSession.current;
      if (current.status !== "closed" || !drained) return;
      if (!disconnected) {
        disconnected = true;
        const reason = current.error || failure;
        if (term.buffer.active.type === "alternate") term.write("\x1b[?1049l");
        term.write(disconnectModes);
        term.writeln(`\r\n${text("[连接已断开]")}`);
        if (reason)
          term.writeln(`${text("断开原因")}：${terminalText(reason)}`);
        term.writeln(hint());
        term.scrollToBottom();
        available = reconnectOptions.current.profileAvailable;
      } else if (available !== reconnectOptions.current.profileAvailable) {
        available = reconnectOptions.current.profileAvailable;
        term.writeln(`\r\n${hint()}`);
        term.scrollToBottom();
      }
    };
    refreshStatus.current = showStatus;
    let lastSize = "";
    const sendSize = () => {
      if (
        !isCurrent() ||
        !term.element ||
        currentSession.current.status !== "connected"
      ) {
        lastSize = "";
        return;
      }
      const key = `${term.cols}:${term.rows}`;
      if (lastSize === key) return;
      lastSize = key;
      void track(
        execId
          ? Backend.ResizeContainerTerminal(
              session.id,
              session.generation,
              execId,
              term.cols,
              term.rows,
            )
          : Backend.Resize(
              session.id,
              session.generation,
              term.cols,
              term.rows,
            ),
      ).catch((error: unknown) => {
        if (lastSize === key) lastSize = "";
        report(error);
      });
    };
    sendTerminalSize.current = sendSize;
    const input = new OrderedInput(
      async (sequence, data) => {
        if (!isCurrent() || currentSession.current.status !== "connected")
          throw new Error("终端输入所属连接已失效");
        const request = {
          sessionId: session.id,
          generation: session.generation,
          sequence,
          data: bytesToBase64(data),
        };
        await track(
          execId
            ? Backend.SendContainerInput({ ...request, execId })
            : Backend.SendInput(request),
        );
      },
      track(
        execId
          ? Backend.GetContainerTerminal(session.id, session.generation, execId)
          : Backend.GetTerminalState(session.id, session.generation),
      ).then((state) => state.inputSequence),
    );
    const onData = term.onData((data) => {
      if (!isCurrent() || currentSession.current.status !== "connected") return;
      term.scrollToBottom();
      void input.enqueue(new TextEncoder().encode(data)).catch(report);
    });
    const onBinary = term.onBinary((data) => {
      if (!isCurrent() || currentSession.current.status !== "connected") return;
      term.scrollToBottom();
      void input
        .enqueue(Uint8Array.from(data, (char) => char.charCodeAt(0)))
        .catch(report);
    });
    term.attachCustomKeyEventHandler((event) => {
      if (
        event.key !== "Enter" ||
        execId ||
        !reconnectOptions.current.onReconnect ||
        currentSession.current.status === "connected"
      )
        return true;
      if (
        event.type !== "keydown" ||
        event.repeat ||
        event.isComposing ||
        event.keyCode === 229 ||
        event.ctrlKey ||
        event.altKey ||
        event.metaKey ||
        event.shiftKey
      )
        return false;
      const options = reconnectOptions.current;
      const state = useApp.getState();
      if (
        !isCurrent() ||
        !currentActive.current ||
        !term.element?.contains(document.activeElement) ||
        document.visibilityState !== "visible" ||
        document.querySelector('[role="dialog"]') ||
        state.quitting ||
        state.installing ||
        options.reconnectDisabled ||
        !options.profileAvailable ||
        !drained ||
        !disconnected ||
        reconnecting ||
        currentSession.current.status !== "closed"
      )
        return false;
      reconnecting = true;
      setPendingReconnect(`${session.id}:${session.generation}`);
      void options.onReconnect!().catch((error: unknown) => {
        if (!isCurrent()) return;
        reconnecting = false;
        setPendingReconnect(null);
        if (useApp.getState().quitting || useApp.getState().installing) return;
        term.writeln(
          `${text("[重新连接失败]")} ${terminalText(errorText(error))}`,
        );
        term.writeln(hint());
        term.scrollToBottom();
      });
      return false;
    });
    const poll = async () => {
      try {
        const batch = await track(
          execId
            ? Backend.PollContainerTerminal(
                session.id,
                session.generation,
                execId,
                after,
              )
            : Backend.PollTerminal(session.id, session.generation, after),
        );
        if (!isCurrent()) return;
        if (batch.reset) {
          term.reset();
          annotations.current?.reset();
          term.writeln(text("[已恢复最近的终端历史，更早输出已超出缓冲区]"));
        }
        const chunks = batch.chunks ?? [];
        for (const chunk of chunks) {
          if (!isCurrent()) return;
          await new Promise<void>((resolve) =>
            term.write(base64ToBytes(chunk.data), resolve),
          );
          after = chunk.sequence;
        }
        // 再发一次 after 确认尾块，再追加本地断线提示。
        if (batch.closed && chunks.length === 0) {
          drained = true;
          failure = batch.error;
          showStatus();
          return;
        }
      } catch (error) {
        if (!isCurrent()) return;
        drained = true;
        failure = errorText(error);
        if (execId) report(error);
        else {
          showStatus();
          if (!disconnected)
            term.writeln(
              `\r\n${text("连接错误详情")}：${terminalText(failure)}`,
            );
        }
        return;
      }
      if (isCurrent()) timer = setTimeout(() => void poll(), 16);
    };
    showStatus();
    sendSize();
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      for (const request of pending) void request.cancel().catch(() => {});
      onData.dispose();
      onBinary.dispose();
      term.attachCustomKeyEventHandler(() => true);
      refreshStatus.current = undefined;
      sendTerminalSize.current = undefined;
    };
  }, [session.id, session.generation, execId]);

  useEffect(() => {
    refreshStatus.current?.();
  }, [session.status, session.error, profileAvailable]);
  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.scrollback = settings.scrollback_lines;
    term.options.cursorStyle =
      settings.cursor_style === "bar"
        ? "bar"
        : settings.cursor_style === "underline"
          ? "underline"
          : "block";
    resizeTerminal.current?.(active);
    annotations.current?.refresh();
  }, [
    active,
    settings.font_family,
    settings.font_size,
    settings.scrollback_lines,
    settings.cursor_style,
    session.status,
    session.id,
    session.generation,
    execId,
    settings.terminal_wrap,
    settings.show_line_numbers,
    settings.show_timestamps,
    settings.trigger_highlights,
  ]);

  async function paste() {
    const target = terminal.current;
    const version = ++pasteVersion.current;
    const isCurrent = () =>
      !!target &&
      target === terminal.current &&
      version === pasteVersion.current &&
      currentActive.current &&
      currentSession.current.status === "connected" &&
      document.visibilityState === "visible" &&
      !useApp.getState().quitting;
    if (!isCurrent()) return;
    try {
      const text = await readClipboard();
      if (!isCurrent()) return;
      if (
        text.length > 1 << 20 ||
        new TextEncoder().encode(text).byteLength > 1 << 20
      )
        throw new Error(t("剪贴板内容超过 1 MiB，已取消粘贴"));
      target?.paste(text);
    } catch (error) {
      if (isCurrent()) useApp.getState().report(error);
    }
  }
  return (
    <ContextMenu.Root
      onOpenChange={(open) => {
        if (open) setHasSelection(terminal.current?.hasSelection() ?? false);
      }}
    >
      <ContextMenu.Trigger asChild>
        <div
          ref={surface}
          className={`terminal-instance ${active ? "is-active" : ""} ${settings.terminal_wrap ? "" : "no-wrap"}`}
          aria-label={t("{name} 终端", { name: session.title })}
        >
          {showConnecting && (
            <div
              className="terminal-connection-status"
              role="status"
              aria-live="polite"
            >
              <LoaderCircle
                className="terminal-connection-spinner"
                size={14}
                aria-hidden="true"
              />
              <span>{t("连接中")}</span>
            </div>
          )}
          <div
            ref={gutter}
            className="terminal-gutter"
            aria-hidden="true"
            hidden={!settings.show_line_numbers && !settings.show_timestamps}
          />
          <div className="terminal-scroll">
            <div ref={container} className="terminal-xterm" />
            <div
              ref={highlights}
              className="terminal-highlights"
              aria-hidden="true"
            />
          </div>
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className="context-menu">
          <ContextMenu.Item
            disabled={!hasSelection}
            onSelect={() =>
              void writeClipboard(terminal.current?.getSelection() ?? "").catch(
                useApp.getState().report,
              )
            }
          >
            <Copy size={14} />
            {t("复制")}
          </ContextMenu.Item>
          <ContextMenu.Item
            disabled={session.status !== "connected"}
            onSelect={() => void paste()}
          >
            <ClipboardPaste size={14} />
            {t("粘贴")}
          </ContextMenu.Item>
          <ContextMenu.Item onSelect={() => terminal.current?.selectAll()}>
            <MousePointer2 size={14} />
            {t("全选")}
          </ContextMenu.Item>
          {!execId && onToggleHistory && (
            <ContextMenu.Item onSelect={onToggleHistory}>
              <History size={14} />
              {t("命令历史")}
            </ContextMenu.Item>
          )}
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
