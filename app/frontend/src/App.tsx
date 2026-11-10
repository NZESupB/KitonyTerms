import { MonitorPanel } from "./components/MonitorPanel";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { Window } from "@wailsio/runtime";
import {
  Activity,
  Box,
  ChevronDown,
  Cpu,
  FolderOpen,
  History,
  Minus,
  Network,
  PanelLeftClose,
  Pencil,
  Plus,
  Server,
  Settings,
  Square,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react";
import {
  Backend,
  connectBackend,
  useApp,
  type Profile,
  type SessionSnapshot,
} from "./lib/backend";
import {
  ProfileDialog,
  ChallengeDialog,
  Modal,
  blankProfile,
} from "./components/Dialogs";
import { TerminalView } from "./components/TerminalView";
import { FileBrowser } from "./components/FileBrowser";
import { SettingsPanel } from "./components/SettingsPanel";
import { Splitter } from "./components/Splitter";
import {
  OperationsPanel,
  type OperationsCache,
} from "./components/OperationsPanel";
import { transferFinished } from "./lib/transfers";
import { QuitProtection } from "./components/QuitProtection";
import { CommandHistory } from "./components/CommandHistory";
import {
  ExternalEditPrompt,
  useExternalEdits,
} from "./components/ExternalEdits";
import { uiFontFamily, accentForeground } from "./lib/appearance";
import { useText } from "./lib/useText";
import type { TextKey } from "./lib/i18n";
import { useProtectedState } from "./lib/useProtectedState";
import { UpdateInstallStatus } from "./components/UpdateInstallStatus";
import "./style.css";

type Page = "connections" | "terminal" | "files" | "settings" | "operations";
const phoneQuery = "(max-width: 599px), (max-height: 599px)";
const statusLabel: Record<string, TextKey> = {
  connecting: "连接中",
  connected: "已连接",
  closing: "关闭中",
  closed: "已断开",
};
const pages = [
  { id: "connections", label: "连接", icon: Server },
  { id: "terminal", label: "终端", icon: SquareTerminal },
  { id: "files", label: "文件", icon: FolderOpen },
  { id: "settings", label: "设置", icon: Settings },
] as const;
const tools = [
  { id: "docker", label: "Docker", icon: Box },
  { id: "services", label: "服务", icon: Server },
  { id: "processes", label: "进程", icon: Cpu },
  { id: "network", label: "网络", icon: Network },
  { id: "status", label: "系统概览", icon: Activity },
];

export default function App() {
  const t = useText();
  const {
    config,
    sessions,
    active,
    error,
    info,
    quitting,
    installing,
    backendEpoch,
  } = useApp();
  const external = useExternalEdits(!!info && !info.mobile);
  const [page, setPage] = useState<Page>("connections");
  const settingsGuard = useRef<() => boolean>(() => true);
  const [phone, setPhone] = useState(() => matchMedia(phoneQuery).matches);
  const [editing, setEditing] = useProtectedState<Profile | null>(null);
  const [deleting, setDeleting] = useProtectedState<Profile | null>(null);
  const [busy, setBusy] = useProtectedState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(220);
  const [sftpHeight, setSftpHeight] = useState(320);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyHeight, setHistoryHeight] = useState(200);
  const [workspaceSize, setWorkspaceSize] = useState({
    width: innerWidth,
    height: innerHeight - 94,
  });
  const [operation, setOperation] = useState<string | null>(null);
  const [systemLight, setSystemLight] = useState(
    () => matchMedia("(prefers-color-scheme: light)").matches,
  );
  const workspace = useRef<HTMLDivElement>(null);
  const operationsDock = useRef<HTMLElement>(null);
  const operationsCache = useRef<OperationsCache>(new Map());
  useEffect(() => {
    for (const key of operationsCache.current.keys()) {
      if (
        !sessions.some((session) =>
          key.startsWith(`${session.id}:${session.generation}:`),
        )
      )
        operationsCache.current.delete(key);
    }
  }, [sessions]);
  const [dockWidth, setDockWidth] = useState(49);
  const [backendReady, setBackendReady] = useState(false);
  const startupSent = useRef(false);
  const configLoaded = config !== null;
  useEffect(() => {
    setBackendReady(false);
    return connectBackend(() => setBackendReady(true));
  }, [backendEpoch]);
  useEffect(() => {
    if (
      !backendReady ||
      !info ||
      info.mobile ||
      quitting ||
      installing ||
      startupSent.current
    )
      return;
    let active = true;
    let request: ReturnType<typeof Backend.ConfirmUpdateStartup> | undefined;
    // 页面提交渲染后才确认，StrictMode 清理会取消尚未发送的下一帧。
    const frame = requestAnimationFrame(() => {
      if (
        !active ||
        useApp.getState().quitting ||
        useApp.getState().installing ||
        startupSent.current
      )
        return;
      startupSent.current = true;
      request = Backend.ConfirmUpdateStartup();
      void request.catch((failure: unknown) => {
        if (active) useApp.getState().report(failure);
      });
    });
    return () => {
      active = false;
      cancelAnimationFrame(frame);
      if (request) void request.cancel();
    };
  }, [backendReady, info, quitting, installing]);
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: light)");
    const device = matchMedia(phoneQuery);
    const update = () => setSystemLight(media.matches);
    const resize = () => setPhone(device.matches);
    media.addEventListener("change", update);
    device.addEventListener("change", resize);
    return () => {
      media.removeEventListener("change", update);
      device.removeEventListener("change", resize);
    };
  }, []);
  useEffect(() => {
    if (!workspace.current) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (entry.target === workspace.current)
          setWorkspaceSize({
            width: entry.contentRect.width,
            height: entry.contentRect.height,
          });
        if (entry.target === operationsDock.current)
          setDockWidth(entry.target.getBoundingClientRect().width);
      }
    });
    observer.observe(workspace.current);
    if (operationsDock.current) observer.observe(operationsDock.current);
    return () => observer.disconnect();
  }, [configLoaded, phone]);
  const settings = config?.config.settings;
  const light =
    settings?.theme === "default-light" ||
    settings?.theme === "light" ||
    (settings?.theme === "system" && systemLight);
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = light ? "light" : "dark";
    root.lang = settings?.language === "english" ? "en" : "zh-CN";
    root.style.setProperty("--ui-scale", String(settings?.ui_font_scale ?? 1));
    const family = uiFontFamily(settings?.ui_font_family);
    if (family) root.style.setProperty("--ui-font", family);
    else root.style.removeProperty("--ui-font");
    const accent = settings?.accent_color ?? "#5aa7ff";
    root.style.setProperty("--accent", accent);
    root.style.setProperty("--accent-foreground", accentForeground(accent));
  }, [
    light,
    settings?.language,
    settings?.ui_font_family,
    settings?.ui_font_scale,
    settings?.accent_color,
  ]);
  const maxSidebar = Math.max(
    176,
    Math.min(720, workspaceSize.width - dockWidth - 2 - 420),
  );
  const actualSidebar = Math.min(sidebarWidth, maxSidebar);
  const maxSftp = Math.max(120, Math.min(420, workspaceSize.height - 120));
  const actualSftp = Math.min(sftpHeight, maxSftp);
  const maxHistory = Math.max(
    120,
    Math.min(420, Math.floor((workspaceSize.height - (phone ? 39 : 0)) * 0.45)),
  );
  const actualHistory = Math.min(historyHeight, maxHistory);
  async function connect(profile: Profile) {
    if (busy) return;
    setBusy(true);
    try {
      const session = await Backend.Connect(profile.name);
      useApp.getState().select(session.id);
      setPage("terminal");
    } catch (error) {
      useApp.getState().report(error);
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!deleting || !config) return;
    setBusy(true);
    try {
      useApp
        .getState()
        .apply(await Backend.DeleteProfile(deleting.name, config.revision));
      setDeleting(null);
    } catch (error) {
      useApp.getState().report(error);
    } finally {
      setBusy(false);
    }
  }
  async function closeSession(session: SessionSnapshot) {
    if (
      useApp.getState().dirtyEditors[`${session.id}:${session.generation}`] &&
      !window.confirm(t("此会话有未保存的远端文件，仍要关闭？"))
    )
      return;
    try {
      const transfers =
        (await Backend.GetTransfers(session.id, session.generation)) ?? [];
      if (
        transfers.some((value) => !transferFinished(value.status)) &&
        !window.confirm(t("此会话正在传输文件，关闭会取消传输，仍要关闭？"))
      )
        return;
      await Backend.CloseSession(session.id, session.generation);
    } catch (error) {
      useApp.getState().report(error);
    }
  }
  const selected = sessions.find((session) => session.id === active);
  const challenging = sessions.find((session) => session.challenge);
  const profiles = config?.config.sessions ?? [];
  const groups = [
    ...new Set([
      ...(config?.config.groups ?? []),
      ...profiles.map((profile) => profile.group ?? ""),
    ]),
  ];
  const style = {
    "--sidebar-width": `${actualSidebar}px`,
    "--sftp-height": `${actualSftp}px`,
  } as CSSProperties;
  const sessionTabs = (
    <div className="session-tabs" role="tablist" aria-label={t("终端会话")}>
      {sessions.map((session) => (
        <div
          className={`session-tab ${session.id === active ? "selected" : ""}`}
          key={session.id}
        >
          <button
            role="tab"
            aria-selected={session.id === active}
            onClick={() => {
              useApp.getState().select(session.id);
              setPage("terminal");
            }}
          >
            <span className={`status-dot ${session.status}`} />
            <span className="tab-title">{session.title}</span>
          </button>
          <button
            aria-label={t("关闭 {name}", { name: session.title })}
            onClick={() => void closeSession(session)}
          >
            <X size={13} />
          </button>
        </div>
      ))}
      <button
        className="new-session"
        title={t("新建连接")}
        aria-label={t("新建连接")}
        onClick={() => setEditing(blankProfile())}
      >
        <Plus size={18} />
      </button>
    </div>
  );
  const controls = (
    <div
      className={`window-controls ${info?.platform === "darwin" ? "is-macos" : ""}`}
    >
      <button
        aria-label={t("最小化")}
        onClick={() => void Window.Minimise().catch(useApp.getState().report)}
      >
        <Minus size={12} />
      </button>
      <button
        aria-label={t("最大化或还原")}
        onClick={() =>
          void Window.ToggleMaximise().catch(useApp.getState().report)
        }
      >
        <Square size={11} />
      </button>
      <button
        className="close-window"
        aria-label={t("关闭窗口")}
        onClick={() =>
          void Backend.RequestQuit().catch(useApp.getState().report)
        }
      >
        <X size={14} />
      </button>
    </div>
  );
  return (
    <div
      ref={(element) => {
        if (element) element.inert = installing;
      }}
      className={`app-shell ${phone ? "is-phone" : "is-desktop"} page-${page}`}
      style={style}
    >
      <header className="app-header">
        {!phone && info?.platform === "darwin" && controls}
        {!phone && (
          <button
            className="sidebar-toggle"
            aria-label={t(collapsed ? "展开侧栏" : "收起侧栏")}
            aria-expanded={!collapsed}
            aria-controls="resource-sidebar"
            onClick={() => setCollapsed(!collapsed)}
          >
            <PanelLeftClose size={18} />
          </button>
        )}
        {phone ? <strong>KitonyTerms</strong> : sessionTabs}
        <div
          className="titlebar-drag"
          onDoubleClick={() => {
            if (!phone)
              void Window.ToggleMaximise().catch(useApp.getState().report);
          }}
        />
        <button
          className="settings-button"
          aria-label={t("命令历史")}
          aria-expanded={historyOpen}
          aria-controls="command-history-panel"
          disabled={!selected}
          onClick={() => setHistoryOpen(!historyOpen)}
        >
          <History size={15} />
          <span>{t("命令历史")}</span>
        </button>
        <button className="settings-button" onClick={() => setPage("settings")}>
          <Settings size={15} />
          <span>{t("设置")}</span>
        </button>
        {!phone && info?.platform !== "darwin" && controls}
      </header>
      {error && (
        <div role="alert" className="global-error">
          <span>{error}</span>
          <button
            aria-label={t("关闭错误消息")}
            onClick={() => useApp.setState({ error: "" })}
          >
            <X size={16} />
          </button>
        </div>
      )}
      {!config ? (
        <main className="empty">
          <Server size={36} />
          <h1>{t("正在加载应用服务")}</h1>
          <button onClick={() => location.reload()}>{t("重新加载")}</button>
        </main>
      ) : (
        <div className="workspace" ref={workspace}>
          <aside
            id="resource-sidebar"
            className={`sidebar ${!phone && collapsed ? "is-collapsed" : ""}`}
            aria-label={t("资源管理器")}
            aria-hidden={!phone && collapsed}
            {...(!phone && collapsed ? { inert: "" } : {})}
          >
            <section className="profile-section">
              <header className="sidebar-section-head">
                <span>{t("连接管理")}</span>
                <button
                  aria-label={t("新建连接")}
                  onClick={() => setEditing(blankProfile())}
                >
                  <Plus size={16} />
                </button>
              </header>
              <div className="profile-list">
                {groups.map((group) => (
                  <details className="connection-group" key={group} open>
                    <summary>
                      <ChevronDown size={13} />
                      <FolderOpen size={14} />
                      {group || t("默认分组")}
                    </summary>
                    {profiles
                      .filter((profile) => (profile.group ?? "") === group)
                      .map((profile) => (
                        <div className="profile" key={profile.name}>
                          <button
                            className="profile-connect"
                            disabled={busy}
                            onClick={() => void connect(profile)}
                            title={`${profile.user}@${profile.host}:${profile.port}`}
                          >
                            <Server size={14} />
                            <span>
                              <strong>{profile.name}</strong>
                              <small>
                                {profile.user}@{profile.host}
                              </small>
                            </span>
                          </button>
                          <button
                            aria-label={t("编辑 {name}", {
                              name: profile.name,
                            })}
                            onClick={() => setEditing(profile)}
                          >
                            <Pencil size={13} />
                          </button>
                          <button
                            aria-label={t("删除 {name}", {
                              name: profile.name,
                            })}
                            onClick={() => setDeleting(profile)}
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
                      ))}
                  </details>
                ))}
                {!profiles.length && (
                  <p className="empty small">{t("添加第一个 SSH 连接。")}</p>
                )}
              </div>
            </section>
            {!phone && (
              <Splitter
                orientation="horizontal"
                value={actualSftp}
                min={120}
                max={maxSftp}
                onChange={setSftpHeight}
                label={t("调整文件区域高度")}
                controls="sftp-panel"
              />
            )}
            <section
              className="sftp-section"
              id="sftp-panel"
              aria-label={t("SFTP 文件管理")}
            >
              <header className="sidebar-section-head">
                <span>{t("SFTP 文件管理")}</span>
              </header>
              {sessions.map((session) => (
                <div
                  className="workspace-view"
                  key={`files:${session.id}:${session.generation}`}
                  hidden={session.id !== active}
                >
                  <FileBrowser
                    session={session}
                    externalEdits={external.edits}
                    onExternalAction={external.act}
                    externalPending={external.pending}
                    active={
                      session.id === active &&
                      (phone ? page === "files" : !collapsed)
                    }
                  />
                </div>
              ))}
              {!selected && (
                <p className="empty small">{t("连接后读取远端目录。")}</p>
              )}
            </section>
          </aside>
          {!phone && !collapsed && (
            <Splitter
              orientation="vertical"
              value={actualSidebar}
              min={176}
              max={maxSidebar}
              onChange={setSidebarWidth}
              label={t("调整侧栏宽度")}
              controls="resource-sidebar"
            />
          )}
          <main className="main-panel">
            {phone && sessionTabs}
            <div
              className="terminal-panel"
              hidden={phone && page !== "terminal"}
            >
              {sessions.map((session) => (
                <TerminalView
                  key={`${session.id}:${session.generation}`}
                  session={session}
                  active={
                    session.id === active && (!phone || page === "terminal")
                  }
                  settings={settings!}
                  onToggleHistory={() => setHistoryOpen((open) => !open)}
                />
              ))}
              {!selected && (
                <div className="empty">
                  <SquareTerminal size={40} />
                  <h2>{t("选择连接，开始工作")}</h2>
                  <p>{t("从左侧连接管理中打开 SSH 会话。")}</p>
                </div>
              )}
              {selected && selected.status !== "connected" && (
                <p className="connection-status" role="status">
                  {statusLabel[selected.status]
                    ? t(statusLabel[selected.status])
                    : selected.status}{" "}
                  {selected.error}
                </p>
              )}
            </div>
            {historyOpen && selected && (!phone || page === "terminal") && (
              <>
                <Splitter
                  orientation="horizontal"
                  value={actualHistory}
                  min={120}
                  max={maxHistory}
                  onChange={setHistoryHeight}
                  label={t("调整命令历史高度")}
                  controls="command-history-panel"
                />
                <div
                  className="command-history-container"
                  style={{ height: actualHistory }}
                >
                  <CommandHistory
                    key={`${selected.id}:${selected.generation}`}
                    session={selected}
                    onClose={() => setHistoryOpen(false)}
                  />
                </div>
              </>
            )}
            {phone && page === "connections" && (
              <section className="connections-panel">
                <header>
                  <h1>{t("连接")}</h1>
                  <button
                    className="primary"
                    onClick={() => setEditing(blankProfile())}
                  >
                    <Plus size={16} />
                    {t("新建连接")}
                  </button>
                </header>
                <div className="connection-grid">
                  {profiles.map((profile) => (
                    <article key={profile.name}>
                      <Server size={22} />
                      <h2>{profile.name}</h2>
                      <p className="muted">
                        {profile.user}@{profile.host}:{profile.port}
                      </p>
                      <footer>
                        <button onClick={() => setEditing(profile)}>
                          {t("编辑")}
                        </button>
                        <button
                          className="primary"
                          disabled={busy}
                          onClick={() => void connect(profile)}
                        >
                          {t("连接")}
                        </button>
                      </footer>
                    </article>
                  ))}
                </div>
              </section>
            )}
            {phone && page === "settings" && (
              <SettingsPanel settings={settings!} guard={settingsGuard} />
            )}
          </main>
          {!phone && (
            <aside
              ref={operationsDock}
              className="operations-dock"
              aria-label={t("运维工具")}
            >
              <nav className="operations-rail" aria-label={t("运维导航")}>
                {tools.map((tool) => (
                  <button
                    key={tool.id}
                    title={
                      tool.label === "Docker"
                        ? tool.label
                        : t(tool.label as TextKey)
                    }
                    aria-label={
                      tool.label === "Docker"
                        ? tool.label
                        : t(tool.label as TextKey)
                    }
                    aria-pressed={operation === tool.id}
                    disabled={!selected || selected.status !== "connected"}
                    onClick={() =>
                      setOperation(operation === tool.id ? null : tool.id)
                    }
                  >
                    <tool.icon size={20} />
                  </button>
                ))}
              </nav>
              {operation && (
                <section className="operations-drawer">
                  <header>
                    <h2>
                      {tools.find((tool) => tool.id === operation)?.label ===
                      "Docker"
                        ? "Docker"
                        : t(
                            tools.find((tool) => tool.id === operation)!
                              .label as TextKey,
                          )}
                    </h2>
                    <button
                      aria-label={t("关闭运维面板")}
                      onClick={() => setOperation(null)}
                    >
                      <X size={18} />
                    </button>
                  </header>
                  {selected ? (
                    <OperationsPanel
                      key={`${selected.id}:${selected.generation}:${operation}`}
                      session={selected}
                      kind={operation}
                      cache={operationsCache.current}
                    />
                  ) : (
                    <p className="empty">{t("请先建立 SSH 连接。")}</p>
                  )}
                </section>
              )}
            </aside>
          )}
        </div>
      )}
      {!phone && (
        <footer className="status-bar" aria-label={t("会话状态")}>
          {selected && <MonitorPanel session={selected} />}
        </footer>
      )}
      {phone && (
        <nav className="mobile-nav" aria-label={t("主导航")}>
          {pages.map((item) => (
            <button
              key={item.id}
              aria-current={page === item.id ? "page" : undefined}
              onClick={() => {
                if (page !== "settings" || settingsGuard.current())
                  setPage(item.id);
              }}
            >
              <item.icon size={20} />
              <span>{t(item.label)}</span>
            </button>
          ))}
        </nav>
      )}
      {!phone && page === "settings" && settings && (
        <Modal
          open
          protectInstall={false}
          title={t("设置")}
          description={t("应用与连接设置")}
          onOpenChange={(open) => {
            if (!open && settingsGuard.current()) setPage("terminal");
          }}
        >
          <SettingsPanel settings={settings} guard={settingsGuard} />
        </Modal>
      )}
      {editing && (
        <ProfileDialog profile={editing} onClose={() => setEditing(null)} />
      )}
      {deleting && (
        <Modal
          open
          title={t("删除连接配置")}
          description={t("将删除 {name} 的连接配置，已有连接保持运行。", {
            name: deleting.name,
          })}
          onOpenChange={(open) => {
            if (!open && !busy) setDeleting(null);
          }}
        >
          <footer>
            <button disabled={busy} onClick={() => setDeleting(null)}>
              {t("取消")}
            </button>
            <button
              className="danger"
              disabled={busy}
              onClick={() => void remove()}
            >
              {t("删除")}
            </button>
          </footer>
        </Modal>
      )}
      {challenging && (
        <ChallengeDialog
          key={challenging.challenge!.id}
          session={challenging}
        />
      )}
      {!info?.mobile && <QuitProtection />}
      <UpdateInstallStatus />
      {info && !info.mobile && <ExternalEditPrompt {...external} />}
    </div>
  );
}
