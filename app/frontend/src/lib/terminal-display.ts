import type { IBuffer, IMarker, Terminal } from "@xterm/xterm";
import type { Settings } from "./backend";

const asciiLower = (value: string) =>
  value.replace(/[A-Z]/g, (char) => char.toLowerCase());
export function rowMatchesTrigger(
  line: string,
  triggers: readonly string[],
): boolean {
  const text = asciiLower(line);
  return triggers.some((value) => {
    const trigger = asciiLower(value.trim());
    return !!trigger && text.includes(trigger);
  });
}
export function terminalColumns(
  visible: number,
  wrap: boolean,
  phone: boolean,
): number {
  return Math.max(wrap ? 20 : phone ? 80 : 160, Math.floor(visible));
}
type Row = { text: string; time: string; number: number; wrapped: boolean };
type BufferRows = {
  rows: Map<number, Row>;
  offset: number;
  anchor?: IMarker;
  anchorLine: number;
  cols: number;
  last: number;
  firstNumber: number;
};

// 行文本只在本终端内用于精确比较，不进入应用 store 或持久化。
export class TerminalAnnotations {
  private buffers = new Map<IBuffer, BufferRows>();
  clear(buffer: IBuffer): void {
    this.buffers.get(buffer)?.anchor?.dispose();
    this.buffers.delete(buffer);
  }
  reset(): void {
    for (const buffer of this.buffers.keys()) this.clear(buffer);
  }
  sync(term: Terminal): BufferRows {
    const buffer = term.buffer.active;
    let state = this.buffers.get(buffer);
    if (!state) {
      state = {
        rows: new Map(),
        offset: 0,
        anchorLine: 0,
        cols: term.cols,
        last: -1,
        firstNumber: 1,
      };
      this.buffers.set(buffer, state);
    }
    if (state.cols !== term.cols) {
      // reflow 会改变行索引；相同行位且内容未变时仍保留时间戳。
      state.anchor?.dispose();
      state.anchor = undefined;
      state.rows = new Map(
        [...state.rows].map(([key, value]) => [key - state!.offset, value]),
      );
      for (const key of state.rows.keys())
        if (key >= buffer.length) state.rows.delete(key);
      state.offset = 0;
      state.last = -1;
      state.firstNumber = 1;
      state.cols = term.cols;
    }
    if (state.anchor) {
      if (state.anchor.isDisposed) {
        state.rows.clear();
        state.offset = 0;
        state.last = -1;
        state.firstNumber = 1;
      } else {
        const removed = state.anchorLine - state.anchor.line;
        if (removed > 0) {
          state.offset += removed;
          state.firstNumber =
            state.rows.get(state.offset)?.number ?? state.firstNumber;
          for (const key of state.rows.keys()) {
            if (key < state.offset) state.rows.delete(key);
            else break;
          }
        }
      }
    }
    state.anchor?.dispose();
    state.anchor =
      buffer.type === "normal" ? term.registerMarker(0) : undefined;
    state.anchorLine = state.anchor?.line ?? 0;
    for (let key = state.last; key >= state.offset + buffer.length; key--)
      state.rows.delete(key);
    return state;
  }
  capture(
    term: Terminal,
    end: number,
    now: string,
    dirtyStart = term.buffer.active.baseY,
  ): void {
    const state = this.sync(term),
      buffer = term.buffer.active;
    end = Math.min(end, buffer.length - 1);
    const start = Math.max(
      0,
      Math.min(dirtyStart, state.last - state.offset + 1),
    );
    const head = state.rows.get(state.offset);
    let number =
      start > 0
        ? (state.rows.get(start + state.offset - 1)?.number ?? 0)
        : head
          ? head.number - (head.wrapped ? 0 : 1)
          : state.firstNumber - 1;
    for (let index = start; index <= end; index++) {
      const line = buffer.getLine(index);
      if (!line) continue;
      const key = index + state.offset,
        previous = state.rows.get(key);
      if (!line.isWrapped) number++;
      else if (!number) number = state.firstNumber;
      const text = line.translateToString(true);
      state.rows.set(key, {
        text,
        time: previous?.text === text ? previous.time : now,
        number,
        wrapped: line.isWrapped,
      });
    }
    state.last = Math.max(state.last, end + state.offset);
  }
  visible(term: Terminal, now: string): (Row | undefined)[] {
    const state = this.sync(term),
      buffer = term.buffer.active;
    const end =
      buffer.type === "normal"
        ? Math.max(
            buffer.baseY + buffer.cursorY,
            Math.min(buffer.length - 1, state.last - state.offset),
          )
        : buffer.length - 1;
    this.capture(term, end, now);
    return Array.from({ length: term.rows }, (_, row) => {
      const index = buffer.viewportY + row;
      return index <= end ? state.rows.get(index + state.offset) : undefined;
    });
  }
}

export function attachTerminalAnnotations(
  term: Terminal,
  gutter: HTMLElement,
  highlights: HTMLElement,
  settings: () => Settings,
) {
  const metadata = new TerminalAnnotations();
  let frame = 0,
    disposed = false;
  const formatter = new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
  const clock = () => formatter.format(new Date());
  function render() {
    frame = 0;
    if (disposed || !term.element?.isConnected) return;
    const options = settings(),
      rows = metadata.visible(term, clock()),
      buffer = term.buffer.active;
    // 只测量 xterm 的渲染表面，不读取 DOM 文字或修改 xterm 节点。
    const screen = term.element.querySelector<HTMLElement>(".xterm-screen");
    const height =
      (screen?.getBoundingClientRect().height ?? term.element.clientHeight) /
      term.rows;
    const showGutter = options.show_line_numbers || options.show_timestamps;
    gutter.hidden = !showGutter;
    gutter.style.fontFamily = options.font_family;
    gutter.style.fontSize = `${options.font_size}px`;
    for (const target of [gutter, highlights]) {
      while (target.children.length > rows.length)
        target.lastElementChild?.remove();
      while (target.children.length < rows.length)
        target.append(document.createElement("div"));
    }
    rows.forEach((row, index) => {
      const label = gutter.children[index] as HTMLElement,
        highlight = highlights.children[index] as HTMLElement;
      label.style.height = highlight.style.height = `${height}px`;
      label.style.lineHeight = `${height}px`;
      const text =
        row && showGutter
          ? [
              options.show_timestamps ? row.time : "",
              options.show_line_numbers
                ? row.wrapped
                  ? "-"
                  : String(row.number)
                : "",
            ]
              .filter(Boolean)
              .join("  ")
          : "";
      if (label.textContent !== text) label.textContent = text;
      const line = buffer.getLine(buffer.viewportY + index);
      highlight.className =
        row &&
        line &&
        rowMatchesTrigger(
          line.translateToString(false),
          options.trigger_highlights ?? [],
        )
          ? "terminal-highlight"
          : "";
    });
  }
  const schedule = () => {
    if (!frame && !disposed) frame = requestAnimationFrame(render);
  };
  const feed = term.onLineFeed(() => {
    const buffer = term.buffer.active,
      end = buffer.baseY + buffer.cursorY;
    metadata.capture(term, end, clock(), Math.max(buffer.baseY, end - 1));
  });
  const listeners = [
    feed,
    term.onWriteParsed(schedule),
    term.onRender(schedule),
    term.onScroll(schedule),
    term.onResize(schedule),
    term.parser.registerCsiHandler({ final: "J" }, (params) => {
      if (params[0] === 2 || params[0] === 3)
        metadata.clear(term.buffer.active);
      return false;
    }),
    term.parser.registerEscHandler({ final: "c" }, () => {
      metadata.reset();
      return false;
    }),
  ];
  schedule();
  return {
    refresh: schedule,
    reset: () => {
      metadata.reset();
      schedule();
    },
    dispose: () => {
      disposed = true;
      cancelAnimationFrame(frame);
      listeners.forEach((value) => value.dispose());
      metadata.reset();
    },
  };
}
