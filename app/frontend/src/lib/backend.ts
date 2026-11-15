import { create } from "zustand";
import { Events } from "@wailsio/runtime";
import * as Backend from "../../bindings/kitonyterms/internal/ipc/backend";
import type {
  AppInfo,
  ConfigSnapshot,
  SessionSnapshot,
} from "../../bindings/kitonyterms/internal/model/models";
import { newerRevision } from "./protocol";
import type { InstallStage } from "./update-installation";

export { Backend };
export type * from "../../bindings/kitonyterms/internal/model/models";
export const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

type State = {
  config: ConfigSnapshot | null;
  sessions: SessionSnapshot[];
  active: string | null;
  info: AppInfo | null;
  error: string;
  dirtyEditors: Record<string, boolean>;
  quitting: boolean;
  // 安装准备/提交期间的独立标记；不复用 quitting，避免退出保护查询被永久停止。
  installing: boolean;
  installStage: InstallStage;
  backendEpoch: number;
  // 通用“本地重启安全”登记表：仅记录 owner -> 是否仍在阻塞安装/退出的布尔值，
  // 不保存文件内容、历史、凭据或用户文本。命名弹窗、SFTP 选择器、设置草稿等
  // 组件在动作开始前登记 true、结束后登记 false，避免只靠 useEffect 回写造成同帧竞态。
  protectedOwners: Record<string, boolean>;
  apply: (snapshot: ConfigSnapshot) => void;
  select: (id: string | null) => void;
  openSession: (session: SessionSnapshot) => void;
  report: (error: unknown) => void;
  setProtected: (owner: string, active: boolean) => void;
};
export const useApp = create<State>((set) => ({
  config: null,
  sessions: [],
  active: null,
  info: null,
  error: "",
  dirtyEditors: {},
  quitting: false,
  installing: false,
  installStage: "idle",
  backendEpoch: 0,
  protectedOwners: {},
  apply: (config) =>
    set((state) =>
      newerRevision(state.config?.revision, config.revision) ? { config } : {},
    ),
  select: (active) => set({ active }),
  openSession: (session) =>
    set((state) => ({
      // 列表刷新可能已带回更新的认证/连接状态，不用 Connect 的快照覆盖它。
      // 始终产生新引用，让在途列表请求识别出这次本地写入。
      sessions: state.sessions.some((value) => value.id === session.id)
        ? [...state.sessions]
        : [...state.sessions, session],
      active: session.id,
    })),
  report: (error) =>
    set((state) =>
      state.quitting || state.installing ? {} : { error: errorText(error) },
    ),
  setProtected: (owner, active) =>
    set((state) => {
      if (!active) {
        if (!(owner in state.protectedOwners)) return {};
        const protectedOwners = { ...state.protectedOwners };
        delete protectedOwners[owner];
        return { protectedOwners };
      }
      return { protectedOwners: { ...state.protectedOwners, [owner]: true } };
    }),
}));

// 本地重启安全判断：任意隐藏会话的未保存编辑器，或任一已登记 owner 仍处于
// busy/dirty，都应阻止提交安装。只读取布尔聚合，不读取受保护内容本身。
export function hasLocalInstallProtection(): boolean {
  const state = useApp.getState();
  return (
    Object.values(state.dirtyEditors).some(Boolean) ||
    Object.values(state.protectedOwners).some(Boolean)
  );
}

export function connectBackend(onReady?: () => void): () => void {
  let disposed = false;
  let refreshing = false;
  let refreshAgain = false;
  const pending = new Set<{ cancel: () => Promise<unknown> }>();
  const loaded = { info: false, config: false, sessions: false };
  let announced = false;
  const announceReady = () => {
    if (
      !disposed &&
      !announced &&
      !useApp.getState().quitting &&
      !useApp.getState().installing &&
      loaded.info &&
      loaded.config &&
      loaded.sessions
    ) {
      announced = true;
      onReady?.();
    }
  };
  const report = (error: unknown) => {
    if (!disposed) useApp.getState().report(error);
  };
  const refreshSessions = async () => {
    if (disposed || useApp.getState().quitting || useApp.getState().installing)
      return;
    if (refreshing) {
      refreshAgain = true;
      return;
    }
    refreshing = true;
    refreshAgain = false;
    const sessionsAtStart = useApp.getState().sessions;
    const request = Backend.GetSessions();
    pending.add(request);
    try {
      const sessions = (await request) ?? [];
      if (!disposed) {
        // Connect 返回后已插入新会话；旧列表不能移除它并回退选中项。
        if (useApp.getState().sessions !== sessionsAtStart) {
          refreshAgain = true;
          return;
        }
        useApp.setState((state) => ({
          sessions,
          active: sessions.some((s) => s.id === state.active)
            ? state.active
            : (sessions[0]?.id ?? null),
        }));
        loaded.sessions = true;
        announceReady();
      }
    } catch (error) {
      report(error);
    } finally {
      refreshing = false;
      pending.delete(request);
      // 请求期间的 sessions:changed（包括认证挑战）必须在结束后补刷。
      if (refreshAgain && !disposed) void refreshSessions();
    }
  };
  const offConfig = Events.On("config:changed", (event) => {
    if (!disposed) useApp.getState().apply(event.data);
  });
  const offSessions = Events.On("sessions:changed", () => {
    void refreshSessions();
  });
  const refresh = () => {
    if (useApp.getState().quitting || useApp.getState().installing) return;
    const request = Backend.GetConfig();
    pending.add(request);
    void request
      .then((config) => {
        if (!disposed) {
          useApp.getState().apply(config);
          loaded.config = true;
          announceReady();
        }
      }, report)
      .finally(() => pending.delete(request));
    void refreshSessions();
  };
  const infoRequest = Backend.Info();
  pending.add(infoRequest);
  void infoRequest
    .then((info) => {
      if (!disposed) {
        useApp.setState({ info });
        loaded.info = true;
        announceReady();
      }
    }, report)
    .finally(() => pending.delete(infoRequest));
  refresh();
  const timer = window.setInterval(() => {
    void refreshSessions();
  }, 1000);
  const visible = () => {
    if (document.visibilityState === "visible") refresh();
  };
  document.addEventListener("visibilitychange", visible);
  return () => {
    disposed = true;
    offConfig();
    offSessions();
    clearInterval(timer);
    document.removeEventListener("visibilitychange", visible);
    for (const request of pending) void request.cancel();
  };
}
