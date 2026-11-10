import assert from "node:assert/strict";
import test from "node:test";
import {
  DockerCollection,
  dockerCollectionMemory,
  type CollectionContext,
  type CollectionWork,
} from "./docker-collection";
import type { OperationReply } from "./backend";
const context = (ids = ["c1"]): CollectionContext => ({
  resource: "containers",
  visibleIds: ids,
  containers: ids,
  images: [],
});
const reply = (extra: Partial<OperationReply> = {}): OperationReply => ({
  requestId: "test",
  kind: "",
  services: [],
  processes: [],
  network: [],
  docker: [],
  runtime: [],
  storage: [],
  title: "",
  output: "",
  ...extra,
});
function take(
  planner: DockerCollection,
  kind: string,
  state: CollectionContext,
  now: number,
): CollectionWork {
  const batch = planner.plan(state, now);
  const work = batch.find((item) => item.spec.kind === kind);
  assert.ok(work, `${kind} 未调度`);
  for (const item of batch) if (item !== work) planner.abandon(item);
  return work;
}

test("Docker 快慢采集不重叠，清单10秒/runtime2秒/大小60秒及64项批次", () => {
  const memory = dockerCollectionMemory();
  const planner = new DockerCollection(memory);
  const state = context(Array.from({ length: 100 }, (_, i) => `c${i}`));
  const initial = planner.plan(state, 1000);
  assert.equal(initial.length, 2);
  const list = initial.find((v) => v.spec.kind === "docker_list")!;
  const slow = initial.find((v) => v.spec.kind === "docker_storage")!;
  assert.equal(slow.spec.targets?.length, 64);
  assert.deepEqual(planner.plan(state, 2000), []);
  planner.complete(list, reply(), 2000, 1000, state);
  const runtime = take(planner, "docker_stats", state, 2000);
  planner.complete(
    runtime,
    reply({
      runtime: [
        {
          id: "c0",
          cpuPercent: 234.5,
          memoryPercent: 1,
          memoryUsedBytes: "0",
          memoryLimitBytes: "9007199254740993",
        },
      ],
    }),
    2100,
    100,
    state,
  );
  assert.equal(memory.runtime.c0.value.cpuPercent, 234.5);
  assert.equal(memory.runtime.c0.value.memoryUsedBytes, "0");
  assert.deepEqual(planner.plan(state, 4099), []);
  const again = take(planner, "docker_stats", state, 4100);
  planner.abandon(again);
  planner.complete(slow, reply(), 5000, 4000, state);
  const rest = take(planner, "docker_storage", state, 5000);
  assert.equal(rest.spec.targets?.length, 36);
  assert.equal(rest.spec.targets?.[0], "c64");
  planner.complete(rest, reply(), 5100, 100, state);
  assert.equal(memory.storage.c0.value.sizeRW, "");
  assert.equal(memory.storage.c0.ttl, 60000);
  const next = planner.plan(state, 12000);
  assert.equal(
    next.some((work) => work.spec.kind === "docker_list"),
    true,
  );
  assert.equal(
    next.some((work) => work.lane === "slow"),
    false,
  );
});

test("Docker 失败2/4/8/16/30秒退避，权限/解析/取消等待显式重试并保留成功数据", () => {
  for (const kind of [
    "permission_denied",
    "parse_failed",
    "output_limit",
    "cancelled",
    "unsupported_backend",
  ]) {
    const memory = dockerCollectionMemory();
    const planner = new DockerCollection(memory);
    const state: CollectionContext = {
      resource: "volumes",
      visibleIds: [],
      containers: [],
      images: [],
    };
    const work = take(planner, "docker_list", state, 0);
    planner.complete(
      work,
      reply({ error: { kind, message: "failure" } }),
      100,
      100,
      state,
    );
    assert.deepEqual(planner.plan(state, 999999), []);
    planner.retry();
    assert.equal(planner.plan(state, 999999).length, 1);
  }
  const memory = dockerCollectionMemory();
  const planner = new DockerCollection(memory);
  const state: CollectionContext = {
    resource: "networks",
    visibleIds: [],
    containers: [],
    images: [],
  };
  let now = 0;
  for (const delay of [2000, 4000, 8000, 16000, 30000, 30000]) {
    const work = take(planner, "docker_list", state, now);
    planner.complete(
      work,
      reply({ error: { kind: "timeout", message: "timeout" } }),
      now,
      0,
      state,
    );
    assert.deepEqual(planner.plan(state, now + delay - 1), []);
    now += delay;
  }
});

test("Docker 大小缓存分离，慢查询延长TTL，删除及迟到结果不会恢复已删除资源", () => {
  const memory = dockerCollectionMemory();
  const planner = new DockerCollection(memory);
  const state = context();
  const slow = take(planner, "docker_storage", state, 0);
  planner.complete(
    slow,
    reply({
      storage: [
        {
          id: "c1",
          imageId: "sha256:i1",
          sizeRW: "0",
          sizeRootFS: "300",
          size: "",
        },
      ],
    }),
    45000,
    45000,
    state,
  );
  assert.equal(memory.storage.c1.ttl, 180000);
  const image = take(planner, "docker_image_sizes", state, 45000);
  planner.complete(
    image,
    reply({
      storage: [
        {
          id: "sha256:i1",
          imageId: "",
          size: "200",
          sizeRW: "",
          sizeRootFS: "",
        },
      ],
    }),
    45100,
    100,
    state,
  );
  assert.equal(memory.images["sha256:i1"].value.size, "200");
  assert.equal(memory.storage.c1.value.sizeRW, "0");
  const empty = context([]);
  planner.prune(empty);
  assert.deepEqual(Object.keys(memory.storage), []);
  assert.deepEqual(Object.keys(memory.images), []);
  planner.complete(
    slow,
    reply({
      storage: [
        {
          id: "c1",
          imageId: "sha256:i1",
          sizeRW: "99",
          sizeRootFS: "300",
          size: "",
        },
      ],
    }),
    50000,
    10,
    empty,
  );
  assert.deepEqual(Object.keys(memory.storage), []);
  assert.deepEqual(Object.keys(memory.images), []);
});

test("Docker 手动清单成功后不会立即重复自动查询，资源新鲜度独立", () => {
  const planner = new DockerCollection(dockerCollectionMemory());
  const state: CollectionContext = {
    resource: "volumes",
    visibleIds: [],
    containers: [],
    images: [],
  };
  planner.recordList("volumes", 5000);
  assert.deepEqual(planner.plan(state, 14999), []);
  assert.equal(planner.plan(state, 15000)[0].spec.kind, "docker_list");
  const other = new DockerCollection(dockerCollectionMemory());
  other.recordList("volumes", 5000);
  assert.equal(
    other.plan({ ...state, resource: "networks" }, 5000)[0].spec.resource,
    "networks",
  );
});
