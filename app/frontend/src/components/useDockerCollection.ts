import { useCallback, useEffect, useRef } from "react";
import { Backend, type OperationReply } from "../lib/backend";
import { operationRequest } from "../lib/operations";
import {
  DockerCollection,
  type CollectionContext,
  type CollectionWork,
  type DockerCollectionMemory,
} from "../lib/docker-collection";

type Options = {
  id: string;
  generation: string;
  memory: DockerCollectionMemory;
  enabled: boolean;
  resource: string;
  context: () => CollectionContext;
  onList: (resource: string, reply: OperationReply) => void;
  onChange: () => void;
};
type Controls = {
  pause: () => Promise<void>;
  resume: () => void;
  retry: () => void;
  recordList: (resource: string) => void;
  wake: () => void;
};

export function useDockerCollection(options: Options) {
  const latest = useRef(options);
  latest.current = options;
  const controls = useRef<Controls>();
  const { id, generation, memory } = options;
  useEffect(() => {
    const planner = new DockerCollection(memory);
    let stopped = false;
    let holds = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    type Task = {
      work: CollectionWork;
      resource: string;
      id: string;
      request: ReturnType<typeof Backend.ExecuteOperation>;
      done: Promise<void>;
      cancelled: boolean;
      stopping?: Promise<void>;
    };
    const tasks = new Set<Task>();
    const visible = () =>
      !stopped &&
      !holds &&
      latest.current.enabled &&
      document.visibilityState === "visible";
    function stopTask(task: Task) {
      if (task.stopping) return task.stopping;
      task.cancelled = true;
      task.stopping = (async () => {
        try {
          await Backend.CancelOperation(id, generation, task.id);
        } finally {
          await task.request.cancel().catch(() => {});
          await task.done;
        }
      })();
      return task.stopping;
    }
    async function cancelAll() {
      clearTimeout(timer);
      await Promise.allSettled([...tasks].map(stopTask));
    }
    function tick() {
      clearTimeout(timer);
      if (!visible()) return;
      const context = latest.current.context();
      planner.prune(context);
      for (const work of planner.plan(context, Date.now())) {
        const input = operationRequest(work.spec);
        const started = Date.now();
        const request = Backend.ExecuteOperation(id, generation, input);
        const task: Task = {
          work,
          resource: context.resource,
          id: input.id,
          request,
          cancelled: false,
          done: Promise.resolve(),
        };
        tasks.add(task);
        task.done = (async () => {
          try {
            const reply = await request;
            if (
              task.cancelled ||
              !visible() ||
              latest.current.resource !== task.resource ||
              reply.requestId !== task.id
            )
              return;
            if (
              planner.complete(
                work,
                reply,
                Date.now(),
                Date.now() - started,
                latest.current.context(),
              )
            ) {
              if (work.spec.kind === "docker_list")
                latest.current.onList(task.resource, reply);
            }
            latest.current.onChange();
          } catch {
            if (!task.cancelled && visible()) {
              planner.complete(
                work,
                {
                  error: {
                    kind: "command_failed",
                    message: "采集请求失败，请刷新重试",
                  },
                } as OperationReply,
                Date.now(),
                Date.now() - started,
                latest.current.context(),
              );
              latest.current.onChange();
            }
          } finally {
            planner.abandon(work);
            tasks.delete(task);
          }
        })();
      }
      timer = setTimeout(tick, 250);
    }
    const owner: Controls = {
      pause: async () => {
        holds++;
        await cancelAll();
      },
      resume: () => {
        holds = Math.max(0, holds - 1);
        tick();
      },
      retry: () => {
        planner.retry();
        latest.current.onChange();
      },
      recordList: (resource) => planner.recordList(resource, Date.now()),
      wake: () => {
        void cancelAll().then(tick);
      },
    };
    controls.current = owner;
    const visibility = () => owner.wake();
    document.addEventListener("visibilitychange", visibility);
    tick();
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
      void cancelAll();
      if (controls.current === owner) controls.current = undefined;
    };
  }, [id, generation, memory]);
  useEffect(() => {
    controls.current?.wake();
  }, [options.enabled, options.resource]);
  const pause = useCallback(async () => {
    await controls.current?.pause();
  }, []);
  const resume = useCallback(() => controls.current?.resume(), []);
  const retry = useCallback(() => controls.current?.retry(), []);
  const recordList = useCallback(
    (resource: string) => controls.current?.recordList(resource),
    [],
  );
  return { pause, resume, retry, recordList };
}
