import assert from "node:assert/strict";
import test from "node:test";
import { validFileName } from "./files";

test("文件动作仅接受单个名称，不把输入解释为路径", () => {
  for (const name of ["", " ", ".", "..", "a/b", "a\\b", "a\nb", "a\0b"])
    assert.equal(validFileName(name), false, name);
  for (const name of ["中文目录", "a b.txt", ".config", "name:remote"])
    assert.equal(validFileName(name), true, name);
});
