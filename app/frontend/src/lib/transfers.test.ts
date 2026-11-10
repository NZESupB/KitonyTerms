import { test } from "node:test";
import assert from "node:assert/strict";
import { formatBytes, transferPercent } from "./transfers";

test("字节格式保留大整数精度与已知零", () => {
  assert.equal(formatBytes("0"), "0 B");
  assert.equal(formatBytes("1536"), "1.5 KiB");
  assert.equal(formatBytes("1152921504606846976"), "1.0 EiB");
  assert.equal(formatBytes(""), "--");
});
test("传输成功之前不显示百分之百，未知总量保持未知", () => {
  assert.equal(
    transferPercent({ status: "running", bytes: "10", totalBytes: "10" }),
    99,
  );
  assert.equal(
    transferPercent({ status: "succeeded", bytes: "0", totalBytes: "0" }),
    100,
  );
  assert.equal(
    transferPercent({ status: "scanning", bytes: "0", totalBytes: "" }),
    undefined,
  );
  assert.equal(
    transferPercent({
      status: "running",
      bytes: "9007199254740992",
      totalBytes: "18014398509481984",
    }),
    50,
  );
});
