import assert from "node:assert/strict";
import test from "node:test";
import { english, translate } from "./i18n";
test("中英资源保留占位符且用户内容不被解释", () => {
  for (const [key, value] of Object.entries(english)) {
    assert.ok(value.trim());
    assert.deepEqual(
      key.match(/\{\w+\}/g) ?? [],
      value.match(/\{\w+\}/g) ?? [],
      key,
    );
  }
  assert.equal(
    translate("english", "关闭 {name}", { name: "远端 {name}<>&" }),
    "Close 远端 {name}<>&",
  );
  assert.equal(
    translate("chinese", "关闭 {name}", { name: "settings" }),
    "关闭 settings",
  );
  assert.equal(
    translate(
      "english",
      "命令中的 {file} 将替换为本地副本路径，不经过 shell。",
    ),
    "{file} is replaced with the local copy path. Commands run without a shell.",
  );
});
