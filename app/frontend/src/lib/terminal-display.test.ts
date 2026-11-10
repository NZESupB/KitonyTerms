import assert from "node:assert/strict";
import test from "node:test";
import {
  rowMatchesTrigger,
  terminalColumns,
  TerminalAnnotations,
} from "./terminal-display";
import type { IBuffer, Terminal } from "@xterm/xterm";
test("关键词使用ASCII忽略大小写的字面子串，不解释正则", () => {
  assert.equal(rowMatchesTrigger("ERROR [a.b] 中文", [" error "]), true);
  assert.equal(rowMatchesTrigger("ERROR [a.b] 中文", ["a.b", "中文"]), true);
  assert.equal(rowMatchesTrigger("axby", ["a.b", "  "]), false);
  assert.equal(rowMatchesTrigger("Ä", ["ä"]), false);
  assert.equal(terminalColumns(42, false, false), 160);
  assert.equal(terminalColumns(42, false, true), 80);
  assert.equal(terminalColumns(10, true, false), 20);
  assert.equal(terminalColumns(200, false, false), 200);
});
test("行辅助栏保留未变时间戳，续行不占逻辑行号，空白与备用屏隔离", () => {
  const lines = [
    { text: "first", wrap: false },
    { text: "next", wrap: true },
    { text: "", wrap: false },
    { text: "unused", wrap: false },
  ];
  const buffer = {
    type: "normal",
    baseY: 0,
    cursorY: 2,
    viewportY: 0,
    length: 4,
    getLine: (index: number) => ({
      isWrapped: lines[index].wrap,
      translateToString: () => lines[index].text,
    }),
  } as unknown as IBuffer;
  const term = {
    buffer: { active: buffer },
    cols: 80,
    rows: 4,
    registerMarker: () => undefined,
  } as unknown as Terminal;
  const metadata = new TerminalAnnotations();
  const first = metadata.visible(term, "01:02:03");
  assert.deepEqual(
    first.map((row) => row?.number),
    [1, 1, 2, undefined],
  );
  assert.equal(first[1]?.wrapped, true);
  assert.equal(first[2]?.time, "01:02:03");
  lines[0].text = "changed";
  const second = metadata.visible(term, "02:03:04");
  assert.equal(second[0]?.time, "02:03:04");
  assert.equal(second[1]?.time, "01:02:03");
  const alt = { ...buffer, type: "alternate" } as IBuffer;
  const alternate = metadata.visible(
    { ...term, buffer: { active: alt } } as unknown as Terminal,
    "03:04:05",
  );
  assert.equal(alternate[0]?.time, "03:04:05");
  assert.equal(alternate[3]?.number, 3);
  assert.equal(metadata.visible(term, "04:05:06")[0]?.time, "02:03:04");
  assert.equal(
    metadata.visible(
      { ...term, cols: 100 } as unknown as Terminal,
      "05:06:07",
    )[0]?.time,
    "02:03:04",
  );
});

test("缓冲区反复裁剪后编号连续且元数据受缓冲行数限制", () => {
  let first = 1;
  const markers: { line: number; isDisposed: boolean; dispose: () => void }[] =
    [];
  const buffer = {
    type: "normal",
    baseY: 1,
    cursorY: 2,
    viewportY: 0,
    length: 4,
    getLine: (index: number) => ({
      isWrapped: false,
      translateToString: () => `row-${first + index}`,
    }),
  } as unknown as IBuffer;
  const term = {
    buffer: { active: buffer },
    cols: 80,
    rows: 4,
    registerMarker: () => {
      const marker = {
        line: 3,
        isDisposed: false,
        dispose() {
          this.isDisposed = true;
        },
      };
      markers.push(marker);
      return marker;
    },
  } as unknown as Terminal;
  const metadata = new TerminalAnnotations();
  metadata.visible(term, "00:00:00");
  for (let count = 0; count < 50; count++) {
    first++;
    for (const marker of markers) if (!marker.isDisposed) marker.line--;
    const rows = metadata.visible(term, "00:00:01");
    assert.deepEqual(
      rows.map((row) => row?.number),
      [first, first + 1, first + 2, first + 3],
    );
    assert.equal(metadata.sync(term).rows.size, 4);
  }
  metadata.reset();
  assert.equal(
    markers.every((marker) => marker.isDisposed),
    true,
  );
});
