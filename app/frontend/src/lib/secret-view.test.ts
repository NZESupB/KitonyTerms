import test from "node:test";
import assert from "node:assert/strict";
import { SecretView } from "./secret-view";
import type { SecretReveal } from "./backend";

function deferred() {
  let resolve!: (value: SecretReveal) => void;
  let cancelled = 0;
  const promise = Object.assign(
    new Promise<SecretReveal>((done) => {
      resolve = done;
    }),
    {
      cancel: async () => {
        cancelled++;
      },
    },
  );
  return {
    promise,
    resolve,
    get cancelled() {
      return cancelled;
    },
  };
}

test("凭据查看拒绝取消及目标切换往返后的迟到成功", async () => {
  const view = new SecretView(() => {});
  view.own("host-a");
  const first = deferred();
  const task = view.reveal(() => first.promise);
  view.own("host-b");
  view.own("host-a");
  first.resolve({ value: "synthetic-old", failure: "" });
  await task;
  assert.equal(first.cancelled, 1);
  assert.equal(view.state.value, null);
  assert.equal(view.state.busy, false);
  view.clear();
});

test("查看值与编辑值隔离，超时及清除后不再持有明文", async () => {
  let expire!: () => void;
  const view = new SecretView(
    () => {},
    (callback) => {
      expire = callback;
      return 0 as unknown as ReturnType<typeof setTimeout>;
    },
  );
  view.own("host");
  const request = deferred();
  const pending = view.reveal(() => request.promise);
  request.resolve({ value: "synthetic-view-only", failure: "" });
  await pending;
  assert.equal(view.state.value, "synthetic-view-only");
  expire();
  assert.equal(view.state.value, null);
  assert.equal(view.state.failure, "");
});

test("认证失败不显示返回值、不自动重试或重复发起", async () => {
  const view = new SecretView(() => {});
  view.own("host");
  const request = deferred();
  let calls = 0;
  const factory = () => {
    calls++;
    return request.promise;
  };
  const pending = view.reveal(factory);
  await view.reveal(factory);
  request.resolve({ value: "synthetic-must-discard", failure: "cancelled" });
  await pending;
  assert.equal(calls, 1);
  assert.equal(view.state.value, null);
  assert.equal(view.state.failure, "cancelled");
  view.clear();
});
