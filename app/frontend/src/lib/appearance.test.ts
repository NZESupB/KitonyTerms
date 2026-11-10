import assert from "node:assert/strict";
import test from "node:test";
import { accentForeground, uiFontFamily } from "./appearance";

test("界面字体保留完整单个名称并追加平台回退", () => {
  assert.equal(uiFontFamily(undefined), undefined);
  assert.equal(uiFontFamily("   "), undefined);
  for (const name of [
    "Fira Sans (UI)",
    "字体 SC",
    "123 Font",
    "Font, Regular",
    "A;:{}",
  ])
    assert.equal(
      uiFontFamily(` ${name} `),
      `"${name}", var(--ui-font-default)`,
    );
  assert.equal(uiFontFamily("monospace"), "monospace, var(--ui-font-default)");
  assert.equal(uiFontFamily("SYSTEM-UI"), "SYSTEM-UI, var(--ui-font-default)");
});
test("界面字体转义引号、反斜杠与控制字符且不丢名称", () => {
  assert.equal(
    uiFontFamily('A"; color: red; /*\\Font\nB\0C'),
    '"A\\"; color: red; /*\\\\Font\\a B\ufffdC", var(--ui-font-default)',
  );
  assert.equal(uiFontFamily("A\x1fa"), '"A\\1f a", var(--ui-font-default)');
});
test("强调色前景选取较高对比度且拒绝无效颜色", () => {
  assert.equal(accentForeground("#000000"), "#ffffff");
  assert.equal(accentForeground("#FFFFFF"), "#000000");
  assert.equal(accentForeground("#ff0000"), "#000000");
  assert.throws(() => accentForeground("red; color:black"));
});
