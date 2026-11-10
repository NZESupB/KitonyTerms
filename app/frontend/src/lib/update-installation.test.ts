import assert from "node:assert/strict";
import test from "node:test";
import {
  UpdateInstaller,
  InstallError,
  type InstallPort,
  type InstallQuit,
  type InstallStage,
} from "./update-installation";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
const idle = (): InstallQuit => ({
  requestId: "",
  intent: "",
  committing: false,
});
const install = (requestId = "ours", committing = false): InstallQuit => ({
  requestId,
  intent: "install",
  committing,
});
function fixture() {
  let state = idle();
  const prepared = deferred<string>();
  const stages: InstallStage[] = [];
  const aborts: string[] = [],
    commits: string[] = [];
  let prepares = 0;
  const port: InstallPort = {
    read: async () => ({ ...state }),
    prepare: () => {
      prepares++;
      state = install();
      return prepared.promise;
    },
    commit: async (id) => {
      commits.push(id);
      state = install(id, true);
    },
    abort: async (id) => {
      aborts.push(id);
      state = idle();
    },
    changed: (stage) => stages.push(stage),
  };
  const controller = new UpdateInstaller(port);
  return {
    controller,
    port,
    prepared,
    stages,
    aborts,
    commits,
    prepares: () => prepares,
    state: () => state,
    setState: (next: InstallQuit) => {
      state = next;
    },
  };
}
const problem = (kind: string) => (error: unknown) =>
  error instanceof InstallError && error.kind === kind;

test("安装同帧双击只准备和提交一次，提交后取消不回收", async () => {
  const f = fixture();
  const first = f.controller.start(() => true);
  assert.equal(await f.controller.start(() => true), false);
  await tick();
  f.prepared.resolve("ours");
  assert.equal(await first, true);
  await f.controller.cancel();
  assert.equal(f.prepares(), 1);
  assert.deepEqual(f.commits, ["ours"]);
  assert.deepEqual(f.aborts, []);
  assert.equal(f.controller.stage, "committed");
});

test("初次或握手后不安全均不提交，后者精确回收再恢复", async () => {
  const f = fixture();
  await assert.rejects(
    f.controller.start(() => false),
    problem("blocked"),
  );
  assert.equal(f.prepares(), 0);
  let safe = true;
  const first = f.controller.start(() => safe);
  const rejected = assert.rejects(first, problem("blocked"));
  await tick();
  safe = false;
  f.prepared.resolve("ours");
  await rejected;
  assert.deepEqual(f.aborts, ["ours"]);
  assert.deepEqual(f.commits, []);
  assert.equal(f.controller.stage, "idle");
});

test("提前取消等待迟到的准备 ID 和真实回收，期间不能重复准备", async () => {
  const f = fixture();
  const cleaned = deferred<void>();
  f.port.abort = async (id) => {
    f.aborts.push(id);
    await cleaned.promise;
    f.setState(idle());
  };
  const first = f.controller.start(() => true);
  await tick();
  let done = false;
  const cancelled = f.controller.cancel().then(() => {
    done = true;
  });
  await tick();
  assert.equal(done, false);
  assert.equal(f.controller.stage, "cancelling");
  assert.equal(await f.controller.start(() => true), false);
  f.prepared.resolve("ours");
  await tick();
  assert.equal(done, false);
  assert.deepEqual(f.aborts, ["ours"]);
  assert.equal(f.controller.active, true);
  cleaned.resolve();
  await cancelled;
  assert.equal(await first, false);
  assert.equal(f.controller.stage, "idle");
  assert.deepEqual(f.commits, []);
});

test("预检返回前取消不创建服务端请求", async () => {
  const f = fixture();
  const read = deferred<InstallQuit>();
  f.port.read = () => read.promise;
  const started = f.controller.start(() => true);
  const cancelled = f.controller.cancel();
  read.resolve(idle());
  await cancelled;
  assert.equal(await started, false);
  assert.equal(f.prepares(), 0);
  assert.equal(f.controller.stage, "idle");
});

test("已有外来意图不取消，未知 ID 的失败也不猜测所有权", async () => {
  const f = fixture();
  f.setState(install("foreign"));
  await assert.rejects(
    f.controller.start(() => true),
    problem("busy"),
  );
  assert.deepEqual(f.aborts, []);
  f.setState(idle());
  const rejected = assert.rejects(
    f.controller.start(() => true),
    problem("uncertain"),
  );
  await tick();
  f.setState(install("foreign"));
  f.prepared.reject(new Error("响应丢失"));
  await rejected;
  assert.deepEqual(f.aborts, []);
  assert.equal(f.controller.stage, "uncertain");
  await f.controller.reconcile();
  assert.equal(f.controller.stage, "uncertain");
});

test("回收失败保持锁定，显式重试成功后才释放", async () => {
  const f = fixture();
  f.port.abort = async (id) => {
    f.aborts.push(id);
    throw new Error("传输失败");
  };
  let safe = true;
  const rejected = assert.rejects(
    f.controller.start(() => safe),
    problem("cleanup"),
  );
  await tick();
  safe = false;
  f.prepared.resolve("ours");
  await rejected;
  assert.equal(f.controller.stage, "uncertain");
  assert.deepEqual(f.aborts, ["ours"]);
  f.port.abort = async (id) => {
    f.aborts.push(id);
    f.setState(idle());
  };
  await f.controller.cancel();
  assert.equal(f.controller.stage, "idle");
  assert.deepEqual(f.aborts, ["ours", "ours"]);
});

test("提交响应丢失时不取消、不重复提交，也不提早恢复业务", async () => {
  const f = fixture();
  f.port.commit = async (id) => {
    f.commits.push(id);
    f.setState(install(id, true));
    throw new Error("提交响应丢失");
  };
  const rejected = assert.rejects(
    f.controller.start(() => true),
    problem("uncertain"),
  );
  await tick();
  f.prepared.resolve("ours");
  await rejected;
  await f.controller.cancel();
  await f.controller.reconcile();
  assert.equal(f.controller.stage, "uncertain");
  assert.equal(await f.controller.start(() => true), false);
  assert.deepEqual(f.aborts, []);
  assert.deepEqual(f.commits, ["ours"]);
  f.setState(idle());
  await f.controller.reconcile();
  assert.equal(f.controller.stage, "idle");
});

test("提交失败且后端完成恢复后释放本地门禁", async () => {
  const f = fixture();
  f.port.commit = async () => {
    f.setState(idle());
    throw new Error("安全检查失败");
  };
  const rejected = assert.rejects(
    f.controller.start(() => true),
    problem("failed"),
  );
  await tick();
  f.prepared.resolve("ours");
  await rejected;
  assert.equal(f.controller.stage, "idle");
  assert.deepEqual(f.aborts, []);
});

test("刷新恢复精确取消未提交意图，提交中意图只能等待", async () => {
  const f = fixture();
  f.setState(install("orphan"));
  await f.controller.recover(f.state());
  assert.deepEqual(f.aborts, ["orphan"]);
  assert.equal(f.controller.stage, "idle");
  f.setState(install("committing", true));
  await f.controller.recover(f.state());
  await f.controller.cancel();
  assert.equal(f.controller.stage, "committing");
  assert.deepEqual(f.aborts, ["orphan"]);
  assert.deepEqual(f.commits, []);
  f.setState(idle());
  await f.controller.reconcile();
  assert.equal(f.controller.stage, "idle");
});

test("旧请求迟到清理不影响新意图，持有准备期间不接管刷新快照", async () => {
  const f = fixture();
  let safe = true;
  const rejected = assert.rejects(
    f.controller.start(() => safe),
    problem("blocked"),
  );
  await tick();
  await f.controller.recover(install("foreign"));
  assert.deepEqual(f.aborts, []);
  f.setState(install("new-owner"));
  safe = false;
  f.prepared.resolve("ours");
  await rejected;
  assert.equal(f.state().requestId, "new-owner");
  assert.deepEqual(f.aborts, []);
  assert.equal(f.controller.stage, "idle");
});
