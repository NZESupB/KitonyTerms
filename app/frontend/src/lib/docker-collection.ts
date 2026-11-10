import type { DockerRuntime, DockerStorage, OperationReply } from "./backend";
import type { OperationSpec } from "./operations";

type Timed<T> = { value: T; at: number; ttl: number };
type Gate = { due: number; failures: number; error: string; blocked: boolean };
export type DockerCollectionMemory = {
  gates: Record<string, Gate>;
  runtime: Record<string, Timed<DockerRuntime>>;
  storage: Record<string, Timed<DockerStorage>>;
  images: Record<string, Timed<DockerStorage>>;
};
export function dockerCollectionMemory(): DockerCollectionMemory {
  return {
    gates: Object.create(null),
    runtime: Object.create(null),
    storage: Object.create(null),
    images: Object.create(null),
  };
}
export type CollectionWork = {
  lane: "fast" | "slow";
  key: string;
  spec: OperationSpec;
};
export type CollectionContext = {
  resource: string;
  visibleIds: string[];
  containers: string[];
  images: string[];
};
const retryable = new Set(["timeout", "busy", "command_failed"]);
const emptyStorage = (id: string): DockerStorage => ({
  id,
  imageId: "",
  sizeRW: "",
  sizeRootFS: "",
  size: "",
});

// 调度规则与时钟分离，页面隐藏和前台抢占由宿主 hook 管理。
export class DockerCollection {
  private active = new Set<string>();
  constructor(readonly memory: DockerCollectionMemory) {}
  private gate(key: string): Gate {
    return (this.memory.gates[key] ??= {
      due: 0,
      failures: 0,
      error: "",
      blocked: false,
    });
  }
  recordList(resource: string, now: number) {
    Object.assign(this.gate(`list:${resource}`), {
      due: now + 10000,
      failures: 0,
      error: "",
      blocked: false,
    });
  }
  retry() {
    for (const gate of Object.values(this.memory.gates)) {
      gate.blocked = false;
      gate.failures = 0;
      gate.error = "";
      gate.due = 0;
    }
  }
  private ready(key: string, now: number) {
    const gate = this.gate(key);
    return !gate.blocked && gate.due <= now;
  }
  plan(context: CollectionContext, now: number): CollectionWork[] {
    const work: CollectionWork[] = [];
    if (!this.active.has("fast")) {
      const key = `list:${context.resource}`;
      if (this.ready(key, now))
        work.push({
          lane: "fast",
          key,
          spec: { kind: "docker_list", resource: context.resource },
        });
      else if (
        context.resource === "containers" &&
        context.visibleIds.length &&
        this.ready("runtime", now)
      )
        work.push({
          lane: "fast",
          key: "runtime",
          spec: { kind: "docker_stats", resource: "containers" },
        });
    }
    if (!this.active.has("slow") && this.ready("slow", now)) {
      const due = (
        cache: Record<string, Timed<DockerStorage>>,
        ids: string[],
      ) =>
        [...new Set(ids)]
          .filter(
            (id) => id && (!cache[id] || cache[id].at + cache[id].ttl <= now),
          )
          .slice(0, 64);
      const containers =
        context.resource === "containers"
          ? due(this.memory.storage, context.visibleIds)
          : [];
      const images =
        context.resource === "images"
          ? context.visibleIds
          : context.resource === "containers"
            ? context.visibleIds.map(
                (id) => this.memory.storage[id]?.value.imageId ?? "",
              )
            : [];
      const imageIds = due(this.memory.images, images);
      if (containers.length)
        work.push({
          lane: "slow",
          key: "slow",
          spec: { kind: "docker_storage", targets: containers },
        });
      else if (imageIds.length)
        work.push({
          lane: "slow",
          key: "slow",
          spec: { kind: "docker_image_sizes", targets: imageIds },
        });
    }
    for (const item of work) this.active.add(item.lane);
    return work;
  }
  abandon(work: CollectionWork) {
    this.active.delete(work.lane);
  }
  complete(
    work: CollectionWork,
    result: OperationReply,
    now: number,
    duration: number,
    context: CollectionContext,
  ): boolean {
    this.abandon(work);
    const gate = this.gate(work.key);
    if (result.error) {
      gate.failures++;
      gate.error = result.error.message;
      gate.blocked = !retryable.has(result.error.kind);
      gate.due =
        now + Math.min(30000, 2000 * 2 ** Math.min(4, gate.failures - 1));
      return false;
    }
    gate.failures = 0;
    gate.error = "";
    gate.blocked = false;
    gate.due = now + (work.key === "runtime" ? 2000 : 0);
    if (work.spec.kind === "docker_list")
      this.recordList(work.spec.resource ?? "", now);
    const currentContainers = new Set(context.containers);
    const currentImages = new Set([
      ...context.images,
      ...Object.values(this.memory.storage).map((v) => v.value.imageId),
    ]);
    if (work.spec.kind === "docker_stats") {
      for (const value of result.runtime ?? [])
        if (currentContainers.has(value.id))
          this.memory.runtime[value.id] = { value, at: now, ttl: 2000 };
    }
    if (
      work.spec.kind === "docker_storage" ||
      work.spec.kind === "docker_image_sizes"
    ) {
      const container = work.spec.kind === "docker_storage";
      const cache = container ? this.memory.storage : this.memory.images;
      const allowed = container ? currentContainers : currentImages;
      const values = new Map((result.storage ?? []).map((v) => [v.id, v]));
      const ttl = Math.min(300000, Math.max(60000, duration * 4));
      for (const id of work.spec.targets ?? [])
        if (allowed.has(id))
          cache[id] = {
            value: values.get(id) ?? emptyStorage(id),
            at: now,
            ttl,
          };
    }
    return true;
  }
  prune(context: CollectionContext) {
    const containers = new Set(context.containers);
    for (const cache of [this.memory.runtime, this.memory.storage])
      for (const id of Object.keys(cache))
        if (!containers.has(id)) delete cache[id];
    const images = new Set([
      ...context.images,
      ...Object.values(this.memory.storage).map((v) => v.value.imageId),
    ]);
    for (const id of Object.keys(this.memory.images))
      if (!images.has(id)) delete this.memory.images[id];
  }
}
