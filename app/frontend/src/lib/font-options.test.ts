import assert from "node:assert/strict";
import test from "node:test";
import { fontOptions } from "./font-options";

test("未知字体完整保留，逗号和引号不拆分", () => {
  const current = ' 中文, Private "Font" ';
  assert.deepEqual(fontOptions(current, ["Menlo", "Arial"]), [
    current,
    "Menlo",
    "Arial",
  ]);
});
test("已发现字体不重复，原顺序与目录不变", () => {
  const families = Object.freeze(["Menlo", "Arial"]);
  const options = fontOptions("Menlo", families);
  assert.deepEqual(options, families);
  assert.notEqual(options, families);
});
test("空系统默认由界面独立呈现，目录不加入空值", () => {
  assert.deepEqual(fontOptions("", ["Menlo"]), ["Menlo"]);
  assert.deepEqual(fontOptions("   ", []), []);
});
