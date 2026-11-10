import { strict as assert } from "node:assert";
import { test } from "node:test";
import {
  base64ToBytes,
  bytesToBase64,
  newerRevision,
  OrderedInput,
} from "./protocol";

test("版本比较不受 JavaScript 整数精度影响", () => {
  assert.equal(
    newerRevision("1793347200000000002", "1793347200000000001"),
    false,
  );
  assert.equal(newerRevision(undefined, "1793347200000000001"), true);
  assert.throws(() => newerRevision("1", "NaN"));
});
test("任意终端字节与多字节 UTF-8 完整往返", () => {
  const bytes = new Uint8Array([
    ...Array.from({ length: 256 }, (_, i) => i),
    ...new TextEncoder().encode("中文🙂"),
  ]);
  assert.deepEqual(base64ToBytes(bytesToBase64(bytes)), bytes);
});
test("恢复输入序号并串行等待前一块完成", async () => {
  const calls: string[] = [];
  let finish: () => void = () => undefined;
  const gate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const queue = new OrderedInput(async (sequence) => {
    calls.push(sequence);
    await gate;
  }, Promise.resolve("42"));
  const one = queue.enqueue(new Uint8Array([1]));
  const two = queue.enqueue(new Uint8Array([2]));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, ["43"]);
  finish();
  await Promise.all([one, two]);
  assert.deepEqual(calls, ["43", "44"]);
});
test("输入失败阻断后续块，不跳序写入", async () => {
  const calls: string[] = [];
  const queue = new OrderedInput(async (sequence) => {
    calls.push(sequence);
    throw new Error("write failed");
  });
  const results = await Promise.allSettled([
    queue.enqueue(new Uint8Array([1])),
    queue.enqueue(new Uint8Array([2])),
  ]);
  assert.equal(
    results.every((result) => result.status === "rejected"),
    true,
  );
  assert.deepEqual(calls, ["1"]);
});
test("队列饱和拒绝新增字节，不影响已排队数据", async () => {
  let finish: () => void = () => undefined;
  const gate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const queue = new OrderedInput(async () => {
    await gate;
  });
  const pending = queue.enqueue(new Uint8Array(65536));
  await assert.rejects(queue.enqueue(new Uint8Array([1])), /输入队列已满/);
  finish();
  await pending;
  await queue.enqueue(new Uint8Array([2]));
});
