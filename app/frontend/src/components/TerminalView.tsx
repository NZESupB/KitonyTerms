import { useEffect, useRef, useState } from "react";
import * as ContextMenu from "@radix-ui/react-context-menu";
import { Copy, ClipboardPaste, History, MousePointer2 } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import {
  Backend,
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
import { translate } from "../lib/i18n";
export function TerminalView({
  session,
  active,
  settings,
  execId,
  onToggleHistory,
}: {
  session: SessionSnapshot;
  active: boolean;
  settings: Settings;
  execId?: string;
  onToggleHistory?: () => void;
}) {
  const t = useText();
  const surface = useRef<HTMLDivElement>(null);
  const gutter = useRef<HTMLDivElement>(null);
  const highlights = useRef<HTMLDivElement>(null);
  const annotations = useRef<ReturnType<typeof attachTerminalAnnotations>>();
  const resizeTerminal = useRef<(focus?: boolean) => void>();
  const container = useRef<HTMLDivElement>(null);
  const [hasSelection, setHasSelection] = useState(false);
  const terminal = useRef<Terminal>();
  const currentStatus = useRef(session.status);
  currentStatus.current = session.status;
  const currentActive = useRef(active);
  currentActive.current = active;
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
    let timer: ReturnType<typeof setTimeout>;
    let after = "0";
    let pollRequest: ReturnType<typeof Backend.PollTerminal> | undefined;
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
    let lastSize = "";
    let fontLoad: { key: string; promise: Promise<FontFace[]> } | undefined;
    const sendSize = () => {
      if (!term.element || currentStatus.current !== "connected") {
        lastSize = "";
        return;
      }
      const key = `${term.cols}:${term.rows}`;
      if (lastSize === key) return;
      lastSize = key;
      void (
        execId
          ? Backend.ResizeContainerTerminal(
              session.id,
              session.generation,
              execId,
              term.cols,
              term.rows,
            )
          : Backend.Resize(session.id, session.generation, term.cols, term.rows)
      ).catch((error: unknown) => {
        if (lastSize === key) lastSize = "";
        report(error);
      });
    };
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
      sendSize();
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
    const input = new OrderedInput(
      async (sequence, data) => {
        const request = {
          sessionId: session.id,
          generation: session.generation,
          sequence,
          data: bytesToBase64(data),
        };
        await (execId
          ? Backend.SendContainerInput({ ...request, execId })
          : Backend.SendInput(request));
      },
      (execId
        ? Backend.GetContainerTerminal(session.id, session.generation, execId)
        : Backend.GetTerminalState(session.id, session.generation)
      ).then((state) => state.inputSequence),
    );
    const onData = term.onData((data) => {
      term.scrollToBottom();
      if (currentStatus.current === "connected")
        void input.enqueue(new TextEncoder().encode(data)).catch(report);
    });
    const onBinary = term.onBinary((data) => {
      term.scrollToBottom();
      if (currentStatus.current === "connected")
        void input
          .enqueue(Uint8Array.from(data, (char) => char.charCodeAt(0)))
          .catch(report);
    });
    const onResize = term.onResize(sendSize);
    const observer = new ResizeObserver(() => resize());
    observer.observe(container.current);
    if (surface.current) observer.observe(surface.current);
    const poll = async () => {
      try {
        pollRequest = execId
          ? Backend.PollContainerTerminal(
              session.id,
              session.generation,
              execId,
              after,
            )
          : Backend.PollTerminal(session.id, session.generation, after);
        const batch = await pollRequest;
        if (stopped) return;
        if (batch.reset) {
          term.reset();
          display.reset();
          term.writeln(
            translate(
              useApp.getState().config?.config.settings.language ?? "chinese",
              "[已恢复最近的终端历史，更早输出已超出缓冲区]",
            ),
          );
        }
        const chunks = batch.chunks ?? [];
        for (const chunk of chunks) {
          if (stopped) return;
          await new Promise<void>((resolve) =>
            term.write(base64ToBytes(chunk.data), resolve),
          );
          after = chunk.sequence;
        }
        // 再发一次 after 让后端确认尾块；关闭后的已消费数据不会丢失。
        if (batch.closed && chunks.length === 0) return;
      } catch (error) {
        if (!stopped) report(error);
        return;
      }
      if (!stopped)
        timer = setTimeout(() => {
          void poll();
        }, 16);
    };
    void poll();
    return () => {
      stopped = true;
      layoutVersion++;
      cancelAnimationFrame(frame);
      clearTimeout(timer);
      if (pollRequest) void pollRequest.cancel();
      observer.disconnect();
      appearance.disconnect();
      onData.dispose();
      onBinary.dispose();
      onResize.dispose();
      display.dispose();
      annotations.current = undefined;
      resizeTerminal.current = undefined;
      term.dispose();
      terminal.current = undefined;
    };
  }, [session.id, session.generation, execId]);
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
      currentStatus.current === "connected" &&
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
