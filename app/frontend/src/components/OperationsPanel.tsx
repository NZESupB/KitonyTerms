import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import {
  Backend,
  errorText,
  type OperationChallenge,
  type OperationReply,
  type OperationsCapabilities,
  type SessionSnapshot,
} from "../lib/backend";
import {
  listOperation,
  operationErrorLabels,
  operationRequest,
  operationSupported,
  type OperationSpec,
} from "../lib/operations";
import { formatBytes } from "../lib/transfers";
import { Modal } from "./Dialogs";
import {
  ContainerTerminalPanel,
  type ContainerTerminalMemory,
} from "./ContainerTerminalPanel";
import {
  dockerCollectionMemory,
  type DockerCollectionMemory,
} from "../lib/docker-collection";
import { useDockerCollection } from "./useDockerCollection";

import { useText } from "../lib/useText";
import type { TextKey } from "../lib/i18n";
const actionLabels: Record<string, TextKey> = {
  start: "启动",
  stop: "停止",
  restart: "重启",
  enable: "启用",
  disable: "禁用",
  remove: "删除",
  terminate: "终止",
  kill: "强杀",
};
const actionText = (t: ReturnType<typeof useText>, action: string) =>
  actionLabels[action] ? t(actionLabels[action]) : action;
type Memory = {
  resource: string;
  filter: string;
  page: number;
  scroll: number;
  capabilities?: OperationsCapabilities;
  lists: Record<string, OperationReply>;
  detail?: OperationReply;
  containerTerminal?: ContainerTerminalMemory;
  collection?: DockerCollectionMemory;
};
export type OperationsCache = Map<string, Memory>;
type Row = {
  id: string;
  title: string;
  subtitle: string;
  meta: string;
  details?: OperationSpec;
  container?: string;
  target?: string;
  metrics?: string;
  actions: { label: string; spec: OperationSpec; confirm?: boolean }[];
};
const resources = [
  { id: "compose", label: "Compose" },
  { id: "containers", label: "容器" },
  { id: "images", label: "镜像" },
  { id: "volumes", label: "卷" },
  { id: "networks", label: "网络" },
] as const;

function rowsFor(
  result: OperationReply | undefined,
  kind: string,
  resource: string,
  collection: DockerCollectionMemory,
  t: ReturnType<typeof useText>,
): Row[] {
  if (!result) return [];
  if (kind === "services")
    return (result.services ?? []).map((s) => ({
      id: s.name,
      title: s.name,
      subtitle: s.description,
      meta: `${s.loadState} · ${s.activeState} / ${s.subState}`,
      details: { kind: "service_details", target: s.name },
      actions: [
        { label: t("日志"), spec: { kind: "service_logs", target: s.name } },
        ...["start", "stop", "restart", "enable", "disable"].map((action) => ({
          label: actionText(t, action),
          spec: { kind: "service_action", target: s.name, action },
          confirm: true,
        })),
      ],
    }));
  if (kind === "processes")
    return (result.processes ?? []).map((p) => ({
      id: String(p.pid),
      title: `${p.pid} · ${p.command}`,
      subtitle: `PPID ${p.ppid} · UID ${p.uid} · ${p.state} · ${p.elapsed}`,
      meta: `CPU ${p.cpuPercent}% · ${t("内存")} ${p.memoryPercent}% · RSS ${formatBytes(String(BigInt(p.rssKiB) * 1024n))}`,
      details: { kind: "process_details", pid: p.pid },
      actions: [
        {
          label: t("终止"),
          spec: { kind: "process_action", pid: p.pid, action: "terminate" },
          confirm: true,
        },
        {
          label: t("强杀"),
          spec: { kind: "process_action", pid: p.pid, action: "kill" },
          confirm: true,
        },
      ],
    }));
  if (kind === "network")
    return (result.network ?? []).map((n, i) => ({
      id: `${i}:${n.protocol}:${n.local}`,
      title: `${n.protocol} · ${n.state}`,
      subtitle: `${n.local} → ${n.peer}`,
      meta: n.ownerAvailable ? n.owner : t("进程归属不可用"),
      actions: [],
    }));
  if (kind !== "docker") return [];
  return (result.docker ?? []).map((d) => {
    const target = d.id;
    const base = { target, resource };
    const actions: Row["actions"] = [];
    if (resource === "containers")
      actions.push(
        { label: t("日志"), spec: { kind: "docker_logs", ...base } },
        { label: t("资源"), spec: { kind: "docker_stats", ...base } },
        { label: t("网络"), spec: { kind: "docker_network", ...base } },
      );
    const verbs =
      resource === "containers" || resource === "compose"
        ? ["start", "stop", "restart", "remove"]
        : ["remove"];
    for (const action of verbs)
      actions.push({
        label: actionText(t, action),
        spec: { kind: "docker_action", ...base, action },
        confirm: true,
      });
    const runtime = collection.runtime[target];
    const storage = collection.storage[target];
    const image =
      resource === "images"
        ? collection.images[target]
        : storage?.value.imageId
          ? collection.images[storage.value.imageId]
          : undefined;
    const stale = [runtime, storage, image].some(
      (entry) => entry && entry.at + entry.ttl < Date.now(),
    );
    const metrics =
      resource === "containers"
        ? `CPU ${runtime?.value.cpuPercent == null ? "--" : `${runtime.value.cpuPercent}%`} · ${t("内存")} ${formatBytes(runtime?.value.memoryUsedBytes ?? "")} / ${formatBytes(runtime?.value.memoryLimitBytes ?? "")} · ${t("可写层")} ${formatBytes(storage?.value.sizeRW ?? "")} · rootfs ${formatBytes(storage?.value.sizeRootFS ?? "")} · ${t("镜像逻辑大小")} ${formatBytes(image?.value.size ?? "")}`
        : resource === "images"
          ? `${t("镜像逻辑大小")} ${formatBytes(image?.value.size ?? "")}`
          : "";
    return {
      id: `${d.id}:${d.repository}:${d.tag}`,
      target,
      metrics: metrics
        ? `${metrics}${stale ? t("（保留过期快照）") : ""}`
        : undefined,
      container: resource === "containers" ? target : undefined,
      title:
        resource === "images"
          ? `${d.repository || "<none>"}:${d.tag || "<none>"}`
          : d.name,
      subtitle: d.image || d.configFiles || d.id,
      meta: [d.status || d.state, d.size, d.driver, d.scope, d.created]
        .filter(Boolean)
        .join(" · "),
      details:
        resource === "compose"
          ? undefined
          : { kind: "docker_details", ...base },
      actions,
    };
  });
}

function SudoDialog({
  challenge,
  session,
  onClose,
  onError,
}: {
  challenge: OperationChallenge;
  session: SessionSnapshot;
  onClose: () => void;
  onError: (message: string) => void;
}) {
  const t = useText();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const pending = useRef<ReturnType<typeof Backend.AnswerOperation> | null>(
    null,
  );
  useEffect(
    () => () => {
      void pending.current?.cancel().catch(() => {});
    },
    [],
  );
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (pending.current) return;
    const request = Backend.AnswerOperation({
      sessionId: session.id,
      generation: session.generation,
      operationId: challenge.operationId,
      challengeId: challenge.id,
      password,
      cancel: false,
    });
    pending.current = request;
    setPassword("");
    setBusy(true);
    try {
      await request;
      onClose();
    } catch (error) {
      onError(errorText(error));
    } finally {
      pending.current = null;
      setBusy(false);
    }
  }
  function cancel() {
    setPassword("");
    void Backend.CancelOperation(
      session.id,
      session.generation,
      challenge.operationId,
    ).catch((error: unknown) => onError(errorText(error)));
    onClose();
  }
  return (
    <Modal
      open
      title={t("sudo 认证")}
      description={challenge.message}
      onOpenChange={(open) => {
        if (!open) cancel();
      }}
    >
      <form onSubmit={(event) => void submit(event)}>
        <label>
          {t("sudo 密码")}
          <input
            autoFocus
            type="password"
            autoComplete="off"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            disabled={busy}
          />
        </label>
        <footer>
          <button type="button" onClick={cancel}>
            {t("取消操作")}
          </button>
          <button className="primary" disabled={busy} type="submit">
            {t(busy ? "提交中…" : "确认")}
          </button>
        </footer>
      </form>
    </Modal>
  );
}

export function OperationsPanel({
  session,
  kind,
  cache,
}: {
  session: SessionSnapshot;
  kind: string;
  cache: OperationsCache;
}) {
  const t = useText();
  const text = useRef(t);
  text.current = t;
  const key = `${session.id}:${session.generation}:${kind}`;
  const [view, setView] = useState<Memory>(
    () =>
      cache.get(key) ?? {
        resource: "containers",
        filter: "",
        page: 0,
        scroll: 0,
        lists: {},
      },
  );
  const latest = useRef(view);
  const [collection] = useState(
    () => view.collection ?? dockerCollectionMemory(),
  );
  const [recoveringContainer, setRecoveringContainer] = useState(
    kind === "docker" &&
      !view.containerTerminal &&
      session.status === "connected",
  );
  latest.current = view;
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  function closeContainer(memory: ContainerTerminalMemory) {
    const saved = cache.get(key);
    if (saved?.containerTerminal === memory)
      cache.set(key, { ...saved, containerTerminal: undefined });
    // 关闭完成可能晚于抽屉卸载；只清理所属代次和 ExecID。
    if (mounted.current)
      setView((state) =>
        state.containerTerminal === memory
          ? { ...state, containerTerminal: undefined }
          : state,
      );
  }
  function openContainer(container: string) {
    if (
      recoveringContainer ||
      cache.get(key)?.containerTerminal ||
      session.status !== "connected"
    )
      return;
    const memory: ContainerTerminalMemory = {
      snapshot: {
        execId: crypto.randomUUID(),
        container,
        status: "starting",
        error: "",
        inputSequence: "0",
      },
      error: "",
    };
    const next = {
      ...latest.current,
      detail: undefined,
      containerTerminal: memory,
    };
    // 先保存标识再发起请求，组件卸载后仍然能够定位并回收。
    cache.set(key, next);
    latest.current = next;
    setView(next);
    memory.starting = Backend.StartContainerTerminal(
      session.id,
      session.generation,
      {
        execId: memory.snapshot.execId,
        container,
        cols: 80,
        rows: 24,
      },
    )
      .then((snapshot) => {
        memory.snapshot = snapshot;
      })
      .catch((error: unknown) => {
        memory.error = errorText(error);
        memory.snapshot = { ...memory.snapshot, status: "closed" };
      })
      .finally(() => {
        memory.starting = undefined;
        if (mounted.current && cache.get(key)?.containerTerminal === memory)
          setView((state) => ({ ...state }));
      });
  }
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [failed, setFailed] = useState(false);
  const [challenge, setChallenge] = useState<OperationChallenge | null>(null);
  const [confirm, setConfirm] = useState<{
    title: string;
    spec: OperationSpec;
  } | null>(null);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (
      kind !== "docker" ||
      view.containerTerminal ||
      session.status !== "connected"
    ) {
      setRecoveringContainer(false);
      return;
    }
    let stopped = false;
    setRecoveringContainer(true);
    const query = Backend.GetContainerTerminal(
      session.id,
      session.generation,
      "",
    );
    void query
      .then((snapshot) => {
        if (stopped || !snapshot.execId || latest.current.containerTerminal)
          return;
        const next = {
          ...latest.current,
          containerTerminal: { snapshot, error: "" },
        };
        latest.current = next;
        cache.set(key, next);
        setView(next);
      })
      .catch((error: unknown) => {
        if (!stopped) {
          setMessage(errorText(error));
          setFailed(true);
        }
      })
      .finally(() => {
        if (!stopped) setRecoveringContainer(false);
      });
    return () => {
      stopped = true;
      void query.cancel().catch(() => {});
    };
  }, [
    cache,
    key,
    kind,
    session.id,
    session.generation,
    session.status,
    view.containerTerminal,
  ]);
  const epoch = useRef(0);
  const dismissedChallenges = useRef(new Set<string>());
  const pending = useRef<{
    id: string;
    request: ReturnType<typeof Backend.ExecuteOperation>;
    cancelled: boolean;
    poll: ReturnType<typeof Backend.GetOperationChallenges> | null;
  } | null>(null);
  const resource = view.resource;
  useEffect(() => {
    cache.set(key, {
      ...view,
      collection,
      containerTerminal: view.containerTerminal?.released
        ? undefined
        : view.containerTerminal,
    });
  }, [cache, key, view, collection]);
  const rows = rowsFor(
    view.lists[resource],
    kind,
    resource,
    collection,
    t,
  ).filter((row) =>
    `${row.title} ${row.subtitle} ${row.meta}`
      .toLocaleLowerCase()
      .includes(view.filter.toLocaleLowerCase()),
  );
  const page = Math.min(
    view.page,
    Math.max(0, Math.ceil(rows.length / 100) - 1),
  );
  const {
    pause: pauseCollection,
    resume: resumeCollection,
    retry: retryCollection,
    recordList: recordCollectionList,
  } = useDockerCollection({
    id: session.id,
    generation: session.generation,
    memory: collection,
    enabled:
      kind === "docker" &&
      session.status === "connected" &&
      !busy &&
      !confirm &&
      !view.detail &&
      !(resource === "containers" && !!view.containerTerminal) &&
      operationSupported(view.capabilities, kind, resource),
    resource,
    context: () => ({
      resource: latest.current.resource,
      visibleIds: rows
        .slice(page * 100, page * 100 + 100)
        .map((row) => row.target ?? "")
        .filter(Boolean),
      containers: (latest.current.lists.containers?.docker ?? []).map(
        (item) => item.id,
      ),
      images: (latest.current.lists.images?.docker ?? []).map(
        (item) => item.id,
      ),
    }),
    onList: (targetResource, reply) => {
      const next = {
        ...latest.current,
        lists: { ...latest.current.lists, [targetResource]: reply },
      };
      latest.current = next;
      setView(next);
    },
    onChange: () => setView((state) => ({ ...state })),
  });
  const execute = useCallback(
    async (
      spec: OperationSpec,
      slot: "list" | "detail" | "capabilities" | "action",
      targetResource: string,
    ): Promise<OperationReply | undefined> => {
      if (pending.current || session.status !== "connected") return;
      const owner = epoch.current;
      const input = operationRequest(spec);
      const request = Backend.ExecuteOperation(
        session.id,
        session.generation,
        input,
      );
      const task = {
        id: input.id,
        request,
        cancelled: false,
        poll: null as ReturnType<typeof Backend.GetOperationChallenges> | null,
      };
      pending.current = task;
      dismissedChallenges.current.clear();
      setBusy(true);
      setMessage("");
      setFailed(false);
      let pollTimer: ReturnType<typeof setTimeout> | undefined;
      const current = () => epoch.current === owner && pending.current === task;
      async function poll() {
        if (!current() || task.cancelled) return;
        const query = Backend.GetOperationChallenges(
          session.id,
          session.generation,
        );
        task.poll = query;
        try {
          const values = (await query) ?? [];
          if (current() && !task.cancelled)
            setChallenge(
              values.find(
                (value) =>
                  value.operationId === input.id &&
                  !dismissedChallenges.current.has(value.id),
              ) ?? null,
            );
        } catch (error) {
          if (current() && !task.cancelled) {
            setMessage(errorText(error));
            setFailed(true);
          }
        } finally {
          if (task.poll === query) task.poll = null;
          if (current() && !task.cancelled)
            pollTimer = setTimeout(() => void poll(), 400);
        }
      }
      if (slot === "action") pollTimer = setTimeout(() => void poll(), 200);
      try {
        const result = await request;
        if (!current() || result.requestId !== input.id) return;
        if (task.cancelled) {
          setMessage(text.current("运维请求已取消"));
          return;
        }
        if (result.error) {
          setMessage(
            `${text.current(operationErrorLabels[result.error.kind] ?? "请求失败")}：${result.error.message}`,
          );
          setFailed(result.error.kind !== "cancelled");
          return;
        }
        if (slot === "capabilities")
          setView((state) => ({
            ...state,
            capabilities: result.capabilities ?? undefined,
          }));
        if (slot === "list" && kind === "docker")
          recordCollectionList(targetResource);
        if (slot === "list")
          setView((state) => ({
            ...state,
            lists: { ...state.lists, [targetResource]: result },
          }));
        if (slot === "detail")
          setView((state) => ({ ...state, detail: result }));
        if (slot === "action")
          setMessage(result.output || text.current("操作已完成"));
        return result;
      } catch (error) {
        if (current()) {
          setMessage(
            task.cancelled ? text.current("运维请求已取消") : errorText(error),
          );
          setFailed(!task.cancelled);
        }
      } finally {
        clearTimeout(pollTimer);
        void task.poll?.cancel().catch(() => {});
        if (pending.current === task) pending.current = null;
        if (epoch.current === owner) {
          setBusy(false);
          setChallenge(null);
        }
      }
    },
    [
      session.id,
      session.generation,
      session.status,
      kind,
      recordCollectionList,
    ],
  );
  const foreground = useRef(false);
  const run = useCallback(
    async (...args: Parameters<typeof execute>) => {
      if (foreground.current) return;
      foreground.current = true;
      const owner = epoch.current;
      try {
        await pauseCollection();
        if (owner !== epoch.current) return;
        return await execute(...args);
      } finally {
        foreground.current = false;
        resumeCollection();
      }
    },
    [execute, pauseCollection, resumeCollection],
  );
  const refresh = useCallback(
    async (probe = false) => {
      const owner = epoch.current;
      retryCollection();
      let capabilities = latest.current.capabilities;
      if (probe || !capabilities) {
        const result = await run(
          { kind: "capabilities" },
          "capabilities",
          resource,
        );
        if (epoch.current !== owner) return;
        if (!result?.capabilities) return;
        capabilities = result.capabilities;
      }
      if (!operationSupported(capabilities, kind, resource)) {
        setMessage(
          text.current("此服务器尚未提供该功能的可用后端，可重新探测。"),
        );
        setFailed(false);
        return;
      }
      await run(listOperation(kind, resource), "list", resource);
    },
    [kind, resource, run, retryCollection],
  );
  useEffect(() => {
    const owner = ++epoch.current;
    const previous = pending.current?.request;
    void Promise.resolve(previous)
      .catch(() => {})
      .then(() => {
        if (epoch.current === owner) {
          setBusy(false);
          setChallenge(null);
          if (session.status === "connected") void refresh();
        }
      });
    return () => {
      epoch.current = owner + 1;
      const task = pending.current;
      if (task) {
        task.cancelled = true;
        void task.request.cancel().catch(() => {});
        void task.poll?.cancel().catch(() => {});
      }
    };
  }, [refresh, session.status]);
  const supported = operationSupported(view.capabilities, kind, resource);
  const disabled = busy || session.status !== "connected";
  useEffect(() => {
    if (!view.detail && list.current)
      list.current.scrollTop = latest.current.scroll;
  }, [view.detail, resource, page]);
  async function cancel() {
    const task = pending.current;
    if (!task || task.cancelled) return;
    task.cancelled = true;
    setMessage(t("正在取消…"));
    setChallenge(null);
    try {
      await Backend.CancelOperation(session.id, session.generation, task.id);
      await task.request.cancel();
    } catch (error) {
      if (pending.current === task) {
        setMessage(errorText(error));
        setFailed(true);
      }
    }
  }
  async function action(spec: OperationSpec) {
    const owner = epoch.current;
    const result = await run(spec, "action", resource);
    if (result && epoch.current === owner)
      await run(listOperation(kind, resource), "list", resource);
  }
  const detail = view.detail;
  return (
    <section className="operations">
      <div className="toolbar operations-toolbar">
        <button disabled={disabled} onClick={() => void refresh()}>
          {t(busy ? "查询中…" : "刷新")}
        </button>
        <button disabled={disabled} onClick={() => void refresh(true)}>
          {t("重新探测")}
        </button>
        {busy && <button onClick={() => void cancel()}>{t("取消")}</button>}
      </div>
      {kind === "docker" && (
        <nav className="operations-resources" aria-label={t("Docker 资源")}>
          {resources.map((item) => (
            <button
              key={item.id}
              aria-pressed={resource === item.id}
              disabled={busy}
              onClick={() =>
                setView((state) => ({
                  ...state,
                  resource: item.id,
                  detail: undefined,
                  page: 0,
                  scroll: 0,
                }))
              }
            >
              {item.label === "Compose" ? item.label : t(item.label)}
            </button>
          ))}
        </nav>
      )}
      {kind === "docker" &&
        Object.values(collection.gates).some((gate) => gate.error) && (
          <p className="operations-message muted" role="status">
            {t("部分采集失败，保留上次结果；点击刷新重试。")}
          </p>
        )}
      {message && (
        <p
          className={
            failed ? "operations-message error" : "operations-message muted"
          }
          role={failed ? "alert" : "status"}
        >
          {message}
        </p>
      )}
      {kind === "docker" &&
      resource === "containers" &&
      view.containerTerminal ? (
        <ContainerTerminalPanel
          key={view.containerTerminal.snapshot.execId}
          session={session}
          memory={view.containerTerminal}
          onClosed={closeContainer}
        />
      ) : detail ? (
        <div className="operations-detail">
          <div className="toolbar">
            <button
              disabled={busy}
              onClick={() =>
                setView((state) => ({ ...state, detail: undefined }))
              }
            >
              {t("返回列表")}
            </button>
          </div>
          <h3>{detail.title || t("资源用量")}</h3>
          {detail.runtime?.length ? (
            detail.runtime.map((item) => (
              <dl key={item.id}>
                <dt>{t("容器")}</dt>
                <dd>{item.id}</dd>
                <dt>CPU</dt>
                <dd>
                  {item.cpuPercent == null ? "--" : `${item.cpuPercent}%`}
                </dd>
                <dt>{t("内存")}</dt>
                <dd>
                  {item.memoryUsedBytes
                    ? formatBytes(item.memoryUsedBytes)
                    : "--"}{" "}
                  /{" "}
                  {item.memoryLimitBytes
                    ? formatBytes(item.memoryLimitBytes)
                    : "--"}
                </dd>
              </dl>
            ))
          ) : (
            <pre>{detail.output || t("暂无结果。")}</pre>
          )}
        </div>
      ) : (
        <>
          {kind !== "status" && (
            <div className="operations-filter">
              <input
                aria-label={t("筛选运维结果")}
                placeholder={t("筛选")}
                value={view.filter}
                onChange={(event) =>
                  setView((state) => ({
                    ...state,
                    filter: event.target.value,
                    page: 0,
                    scroll: 0,
                  }))
                }
              />
              <span className="muted">
                {t("{count} 项", { count: rows.length })}
              </span>
            </div>
          )}
          <div
            className="operations-list"
            ref={list}
            onScroll={(event) => {
              const scroll = event.currentTarget.scrollTop;
              setView((state) =>
                state.scroll === scroll ? state : { ...state, scroll },
              );
            }}
          >
            {kind === "status" ? (
              <pre>
                {view.lists[resource]?.output || t("查询后显示系统概览。")}
              </pre>
            ) : (
              rows.slice(page * 100, page * 100 + 100).map((row) => (
                <article className="operation-row" key={row.id}>
                  <button
                    className="operation-title"
                    title={row.title}
                    disabled={disabled || !row.details || !supported}
                    onClick={() => {
                      if (row.details)
                        void run(row.details, "detail", resource);
                    }}
                  >
                    {row.title}
                  </button>
                  <p title={row.subtitle}>{row.subtitle}</p>
                  <p className="muted">{row.meta}</p>
                  {row.metrics && (
                    <p className="muted operation-metrics">{row.metrics}</p>
                  )}
                  <div className="operation-actions">
                    {row.container && (
                      <button
                        disabled={disabled || !supported || recoveringContainer}
                        onClick={() => openContainer(row.container!)}
                      >
                        {t("终端")}
                      </button>
                    )}
                    {row.actions.map((item) => (
                      <button
                        key={item.label}
                        disabled={disabled || !supported}
                        onClick={() => {
                          if (item.confirm)
                            setConfirm({ title: row.title, spec: item.spec });
                          else void run(item.spec, "detail", resource);
                        }}
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>
                </article>
              ))
            )}
            {kind !== "status" && !rows.length && (
              <p className="empty">
                {t(
                  busy
                    ? "正在查询…"
                    : view.lists[resource]
                      ? "没有匹配结果。"
                      : "暂无查询结果。",
                )}
              </p>
            )}
          </div>
          {rows.length > 100 && (
            <div className="toolbar operations-pagination">
              <button
                disabled={page === 0}
                onClick={() =>
                  setView((state) => ({ ...state, page: page - 1, scroll: 0 }))
                }
              >
                {t("上一页")}
              </button>
              <span>
                {page + 1} / {Math.ceil(rows.length / 100)}
              </span>
              <button
                disabled={(page + 1) * 100 >= rows.length}
                onClick={() =>
                  setView((state) => ({ ...state, page: page + 1, scroll: 0 }))
                }
              >
                {t("下一页")}
              </button>
            </div>
          )}
        </>
      )}
      {confirm && (
        <Modal
          open
          title={t("确认运维操作")}
          description={`${actionText(t, confirm.spec.action ?? "")}：${confirm.title}`}
          onOpenChange={(open) => {
            if (!open) setConfirm(null);
          }}
        >
          <p>{t("此操作会改变服务器状态。")}</p>
          <footer>
            <button onClick={() => setConfirm(null)}>{t("取消")}</button>
            <button
              className="primary"
              disabled={disabled}
              onClick={() => {
                const spec = confirm.spec;
                setConfirm(null);
                void action(spec);
              }}
            >
              {t("确认执行")}
            </button>
          </footer>
        </Modal>
      )}
      {challenge && (
        <SudoDialog
          key={challenge.id}
          challenge={challenge}
          session={session}
          onClose={() => {
            dismissedChallenges.current.add(challenge.id);
            setChallenge(null);
          }}
          onError={(text) => {
            setMessage(text);
            setFailed(true);
          }}
        />
      )}
    </section>
  );
}
