import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import * as ContextMenu from "@radix-ui/react-context-menu";
import {
  ArrowUp,
  Download,
  FileText,
  Folder,
  FolderPlus,
  Pencil,
  RefreshCw,
  SquareTerminal,
  Trash2,
  Upload,
} from "lucide-react";
import {
  Backend,
  errorText,
  useApp,
  type Directory,
  type Editor,
  type ExternalEdit,
  type SessionSnapshot,
  type TextFile,
  type TransferProgress,
} from "../lib/backend";
import { useShellState } from "../lib/useShellState";
import { Modal } from "./Dialogs";
import { validFileName } from "../lib/files";
import {
  formatBytes,
  transferFinished,
  transferPercent,
} from "../lib/transfers";

import { useText } from "../lib/useText";
import type { TextKey } from "../lib/i18n";
import { useProtectedState } from "../lib/useProtectedState";
const transferLabels: Record<string, TextKey> = {
  scanning: "正在预检",
  running: "正在传输",
  exporting: "正在导出",
  cancelling: "正在取消",
  cancelled: "已取消",
  failed: "传输失败",
  succeeded: "传输完成",
};
const externalLabels: Record<string, TextKey> = {
  downloading: "正在下载",
  changed: "本地已保存",
  uploading: "正在上传",
  error: "需要处理",
};
type Request<T> = PromiseLike<T> & { cancel: () => Promise<unknown> };
type Action = {
  kind: "mkdir" | "rename" | "delete";
  paths: string[];
  name: string;
};

export function FileBrowser({
  session,
  active,
  externalEdits,
  onExternalAction,
  externalPending,
}: {
  session: SessionSnapshot;
  active: boolean;
  externalEdits: ExternalEdit[];
  onExternalAction: (edit: ExternalEdit, action: string) => Promise<void>;
  externalPending: string | null;
}) {
  const t = useText();
  const [directory, setDirectory] = useState<Directory | null>(null);
  const [path, setPath] = useState(".");
  const [requestBusy, setBusy] = useProtectedState(false);
  const [transfers, setTransfers] = useProtectedState<TransferProgress[]>(
    [],
    (values) => values.some((value) => !transferFinished(value.status)),
  );
  const [uploading, setUploading] = useProtectedState(false);
  const [uploadDirectory, setUploadDirectory] = useState(false);
  const [overwrite, setOverwrite] = useState(false);
  const autoSync = useApp(
    (state) => state.config?.config.settings.sftp_auto_sync ?? false,
  );
  const [syncPreferenceBusy, setSyncPreferenceBusy] = useProtectedState(false);
  const autoSyncNow = useRef(autoSync);
  autoSyncNow.current = autoSync;
  const followedRevision = useRef<string>();
  const completed = useRef(new Set<string>());
  const transferEpoch = useRef(0);
  const currentDirectory = directory?.path ?? ".";
  const latestTransfer = transfers[transfers.length - 1];
  const transferring =
    !!latestTransfer && !transferFinished(latestTransfer.status);
  const busy = requestBusy || transferring;
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editing, setEditing] = useProtectedState<TextFile | null>(null);
  const [draft, setDraft] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [action, setAction] = useProtectedState<Action | null>(null);
  const generation = useRef(0);
  const loaded = useRef(false);
  const pending = useRef(new Set<{ cancel: () => Promise<unknown> }>());
  const mobile = useApp((state) => state.info?.mobile ?? false);
  const mobileFiles = useApp(
    (state) => state.info?.capabilities?.includes("mobile_files") ?? false,
  );
  const fileTransfers = !mobile || mobileFiles;
  const editorSettings = useApp((state) => state.config?.config.settings);
  const [editors, setEditors] = useState<Editor[]>([]);
  useEffect(() => {
    if (mobile) return;
    let stopped = false;
    const request = Backend.GetEditors();
    void request.then(
      (value) => {
        if (!stopped) setEditors(value ?? []);
      },
      (error: unknown) => {
        if (!stopped) setError(errorText(error));
      },
    );
    return () => {
      stopped = true;
      void request.cancel().catch(() => {});
    };
  }, [mobile, editorSettings]);
  const columnWidths = useApp(
    (state) => state.config?.config.settings.sftp_column_widths,
  );
  const columns = {
    "--sftp-columns": [96, 110, 64, 64, 78, 90]
      .map(
        (minimum, index) =>
          `${Math.max(minimum, Math.min(2048, columnWidths?.[index] ?? minimum))}px`,
      )
      .join(" "),
  } as CSSProperties;
  const owner = `${session.id}:${session.generation}`;
  const connected = session.status === "connected";
  const editableSelection =
    selected.length === 1 &&
    directory?.entries?.some(
      (entry) => entry.path === selected[0] && !entry.isDir && !entry.isSymlink,
    );
  const shell = useShellState(session, active);
  const shellNow = useRef(shell);
  shellNow.current = shell;
  const track = useCallback(async <T,>(request: Request<T>): Promise<T> => {
    pending.current.add(request);
    try {
      return await request;
    } finally {
      pending.current.delete(request);
    }
  }, []);
  const invalidate = useCallback(() => {
    generation.current++;
  }, []);
  useEffect(() => {
    const requests = pending.current;
    return () => {
      invalidate();
      for (const request of requests) void request.cancel().catch(() => {});
      useApp.setState((state) => {
        const dirtyEditors = { ...state.dirtyEditors };
        delete dirtyEditors[owner];
        return { dirtyEditors };
      });
    };
  }, [owner, invalidate]);
  useEffect(() => {
    useApp.setState((state) => ({
      dirtyEditors: {
        ...state.dirtyEditors,
        [owner]: editing !== null && draft !== editing.content,
      },
    }));
  }, [owner, editing, draft]);

  const syncDirectory = useCallback(
    async (target: string, automatic: boolean) => {
      const request = generation.current;
      const previous = shellNow.current?.revision;
      const written = await track(
        Backend.ChangeDirectory(
          session.id,
          session.generation,
          target,
          automatic,
        ),
      );
      if (written) {
        const observed = await track(
          Backend.GetShellState(session.id, session.generation),
        );
        if (
          request === generation.current &&
          (observed.pendingDirectory === target ||
            observed.cwd === target ||
            observed.revision === previous)
        )
          followedRevision.current = observed.revision;
      }
    },
    [session.id, session.generation, track],
  );
  const load = useCallback(
    async (next: string, synchronize = false) => {
      const request = ++generation.current;
      setBusy(true);
      setError("");
      try {
        const value = await track(
          Backend.ReadDirectory(session.id, session.generation, next),
        );
        if (request === generation.current) {
          setDirectory(value);
          setPath(value.path);
          setSelected([]);
          loaded.current = true;
          if (synchronize && autoSyncNow.current)
            await syncDirectory(value.path, true);
        }
      } catch (error) {
        if (request === generation.current) setError(errorText(error));
      } finally {
        if (request === generation.current) setBusy(false);
      }
    },
    [session.id, session.generation, track, syncDirectory, setBusy],
  );
  useEffect(() => {
    if (active && connected && !loaded.current) void load(".");
  }, [active, connected, load]);
  useEffect(() => {
    if (
      !autoSync ||
      !active ||
      !connected ||
      busy ||
      editing ||
      !shell?.cwd ||
      shell.pendingDirectory ||
      shell.altScreen ||
      shell.revision === followedRevision.current
    )
      return;
    followedRevision.current = shell.revision;
    if (shell.cwd !== directory?.path) void load(shell.cwd);
  }, [
    autoSync,
    active,
    connected,
    busy,
    editing,
    shell?.cwd,
    shell?.revision,
    shell?.pendingDirectory,
    shell?.altScreen,
    directory?.path,
    load,
  ]);

  async function syncCurrentDirectory() {
    if (busy || !connected) return;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    try {
      await syncDirectory(currentDirectory, false);
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function changeAutoSync(enabled: boolean) {
    const config = useApp.getState().config;
    if (!config || syncPreferenceBusy) return;
    setSyncPreferenceBusy(true);
    setError("");
    try {
      const saved = await track(
        Backend.SaveSettings({
          expectedRevision: config.revision,
          settings: { ...config.config.settings, sftp_auto_sync: enabled },
        }),
      );
      useApp.getState().apply(saved);
      followedRevision.current = undefined;
    } catch (error) {
      setError(errorText(error));
    } finally {
      setSyncPreferenceBusy(false);
    }
  }
  useEffect(() => {
    if (!active || !connected) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let request: ReturnType<typeof Backend.GetTransfers> | undefined;
    const poll = async () => {
      try {
        const epoch = transferEpoch.current;
        request = Backend.GetTransfers(session.id, session.generation);
        const values = (await request) ?? [];
        if (disposed) return;
        if (epoch === transferEpoch.current) {
          setTransfers(values);
          const retained = new Set(values.map((value) => value.id));
          completed.current = new Set(
            [...completed.current].filter((id) => retained.has(id)),
          );
          let refresh = false;
          for (const value of values) {
            if (
              !transferFinished(value.status) ||
              completed.current.has(value.id)
            )
              continue;
            const uploaded =
              value.direction === "upload" && value.status === "succeeded";
            if (uploaded && (requestBusy || editing)) continue;
            completed.current.add(value.id);
            refresh ||= uploaded;
          }
          if (refresh) void load(currentDirectory);
        }
      } catch (error) {
        if (!disposed) setError(errorText(error));
      }
      if (!disposed) timer = setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
      if (request) void request.cancel().catch(() => {});
    };
  }, [
    active,
    connected,
    session.id,
    session.generation,
    requestBusy,
    editing,
    load,
    currentDirectory,
    setTransfers,
  ]);

  async function externalEdit(filePath: string, editor: string) {
    if (busy || editing || mobile) return;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    try {
      await track(
        Backend.OpenExternalEdit(
          session.id,
          session.generation,
          filePath,
          editor,
        ),
      );
      if (request === generation.current) setNotice(t("外部编辑任务已创建"));
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function edit(filePath: string) {
    if (busy || editing) return;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    try {
      const value = await track(
        Backend.ReadText(session.id, session.generation, filePath),
      );
      if (request === generation.current) {
        setEditing(value);
        setDraft(value.content);
      }
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function save() {
    if (!editing || busy) return;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    try {
      const value = await track(
        Backend.WriteText(
          session.id,
          session.generation,
          editing.path,
          draft,
          editing.revision,
        ),
      );
      if (request === generation.current) setEditing(value);
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function change(factory: () => Request<void>) {
    if (busy) return;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await track(factory());
      if (request === generation.current) {
        setAction(null);
        await load(directory?.path ?? ".");
      }
    } catch (error) {
      if (request === generation.current) {
        setError(errorText(error));
        // 删除可能已完成一部分，保留错误并让用户手动刷新确认。
      }
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function download(paths: string[]) {
    if (busy) return;
    const request = ++generation.current;
    transferEpoch.current++;
    setBusy(true);
    setError("");
    try {
      const value = await track(
        Backend.StartDownload(session.id, session.generation, paths),
      );
      if (request === generation.current && value.id) {
        transferEpoch.current++;
        setTransfers((old) => [
          ...old.filter((job) => job.id !== value.id),
          value,
        ]);
      }
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  async function upload() {
    if (busy) return;
    const request = ++generation.current;
    transferEpoch.current++;
    setBusy(true);
    setError("");
    try {
      const value = await track(
        Backend.StartUpload(
          session.id,
          session.generation,
          directory?.path ?? ".",
          uploadDirectory,
          overwrite,
        ),
      );
      if (request === generation.current) {
        setUploading(false);
        if (value.id) {
          transferEpoch.current++;
          setTransfers((old) => [
            ...old.filter((job) => job.id !== value.id),
            value,
          ]);
        }
      }
    } catch (error) {
      if (request === generation.current) setError(errorText(error));
    } finally {
      if (request === generation.current) setBusy(false);
    }
  }
  function submitAction() {
    if (!action || busy) return;
    if (action.kind !== "delete" && !validFileName(action.name)) {
      setError(t("请输入单个名称，不能包含路径分隔符、换行或 .、.."));
      return;
    }
    const parent = directory?.path ?? ".";
    if (action.kind === "mkdir")
      void change(() =>
        Backend.CreateDirectory(
          session.id,
          session.generation,
          `${parent}/${action.name}`,
        ),
      );
    else if (action.kind === "rename")
      void change(() =>
        Backend.RenamePath(
          session.id,
          session.generation,
          action.paths[0],
          `${parent}/${action.name}`,
        ),
      );
    else
      void change(() =>
        Backend.RemovePaths(session.id, session.generation, action.paths),
      );
  }
  function cancel() {
    if (latestTransfer && transferring)
      void Backend.CancelTransfer(
        session.id,
        session.generation,
        latestTransfer.id,
      ).catch(useApp.getState().report);
    for (const request of pending.current)
      void request.cancel().catch(useApp.getState().report);
  }
  function openAction(kind: Action["kind"]) {
    setError("");
    setAction({
      kind,
      paths: [...selected],
      name:
        kind === "rename" ? (selected[0].split("/").slice(-1)[0] ?? "") : "",
    });
  }

  return (
    <section className="file-browser">
      <header className="toolbar">
        <button
          aria-label={t("上级目录")}
          disabled={busy || !connected}
          onClick={() => void load((directory?.path ?? ".") + "/..", true)}
        >
          <ArrowUp size={16} />
        </button>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (!busy && connected) void load(path, true);
          }}
        >
          <input
            aria-label={t("远端路径")}
            value={path}
            onChange={(event) => setPath(event.target.value)}
          />
        </form>
        <button
          aria-label={t("刷新目录")}
          disabled={busy || !connected}
          onClick={() => void load(path)}
        >
          <RefreshCw size={16} />
        </button>
        {fileTransfers && (
          <button
            aria-label={t("上传文件或目录")}
            title={t("上传")}
            disabled={busy || !connected}
            onClick={() => {
              setOverwrite(false);
              setUploading(true);
            }}
          >
            <Upload size={16} />
          </button>
        )}
      </header>
      <div
        className="toolbar file-sync-actions"
        role="group"
        aria-label={t("目录同步")}
      >
        <button
          aria-label={t("同步目录到终端")}
          title={t("同步目录到终端")}
          disabled={busy || !connected || !directory}
          onClick={() => void syncCurrentDirectory()}
        >
          <SquareTerminal size={15} />
        </button>
        <label>
          <input
            type="checkbox"
            aria-label={t("自动同步目录")}
            checked={autoSync}
            disabled={!connected || syncPreferenceBusy}
            onChange={(event) => void changeAutoSync(event.target.checked)}
          />
          {t("自动同步")}
        </label>
      </div>
      <div
        className="toolbar file-actions"
        role="group"
        aria-label={t("文件操作")}
      >
        <button
          disabled={busy || !connected || !!editing}
          onClick={() => openAction("mkdir")}
        >
          <FolderPlus size={16} />
          {t("新建目录")}
        </button>
        <button
          disabled={busy || !connected || !!editing || selected.length !== 1}
          onClick={() => openAction("rename")}
        >
          <Pencil size={16} />
          {t("重命名")}
        </button>
        <button
          disabled={busy || !connected || !!editing || !selected.length}
          onClick={() => openAction("delete")}
        >
          <Trash2 size={16} />
          {t("删除")}
        </button>
        {fileTransfers && (
          <button
            disabled={busy || !connected || !selected.length}
            onClick={() => void download(selected)}
          >
            <Download size={16} />
            {t("下载所选")}
          </button>
        )}
        <span className="muted">
          {t("已选 {count} 项", { count: selected.length })}
        </span>
        {busy && <button onClick={cancel}>{t("取消操作")}</button>}
      </div>
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="muted">
          {notice}
        </p>
      )}
      {!mobile &&
        externalEdits
          .filter(
            (edit) =>
              edit.sessionId === session.id &&
              edit.generation === session.generation &&
              edit.status !== "retained" &&
              edit.status !== "failed",
          )
          .map((edit) => (
            <div className="transfer-status external-edit-row" key={edit.id}>
              <span title={edit.remotePath}>
                {t("外部编辑")} · {edit.remotePath.split("/").pop()} ·{" "}
                {edit.status === "watching"
                  ? t(edit.autoUpload ? "自动上传已开启" : "等待保存")
                  : externalLabels[edit.status]
                    ? t(externalLabels[edit.status])
                    : edit.status}
              </span>
              <button
                disabled={externalPending !== null}
                onClick={() => void onExternalAction(edit, "stop")}
              >
                {t("结束外部编辑")}
              </button>
            </div>
          ))}
      {busy && (
        <p role="status" className="muted">
          {t("正在处理…")}
        </p>
      )}
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <div
            className="file-list"
            style={columns}
            tabIndex={0}
            aria-label={t("远端文件，按 Shift+F10 打开菜单")}
          >
            <div className="file-table-head">
              <span>{t("名称")}</span>
              <span>{t("修改时间")}</span>
              <span>{t("类型")}</span>
              <span>{t("大小")}</span>
              <span>{t("权限")}</span>
              <span>{t("所有者组")}</span>
            </div>
            {(directory?.entries ?? []).map((entry) => (
              <div
                key={entry.path}
                className="file-row"
                onContextMenu={() => {
                  if (!selected.includes(entry.path)) setSelected([entry.path]);
                }}
              >
                <div className="file-name-cell">
                  <label className="file-select">
                    <input
                      type="checkbox"
                      aria-label={t("选择 {name}", { name: entry.name })}
                      disabled={busy || !connected || entry.isSymlink}
                      checked={selected.includes(entry.path)}
                      onChange={(event) =>
                        setSelected((previous) =>
                          event.target.checked
                            ? [...previous, entry.path]
                            : previous.filter((value) => value !== entry.path),
                        )
                      }
                    />
                  </label>
                  <button
                    className="file-name"
                    disabled={busy || !connected || entry.isSymlink}
                    onClick={() => {
                      if (entry.isDir) void load(entry.path, true);
                      else void edit(entry.path);
                    }}
                  >
                    {entry.isDir ? (
                      <Folder size={16} />
                    ) : (
                      <FileText size={16} />
                    )}
                    <span title={entry.name}>{entry.name}</span>
                  </button>
                </div>
                <span className="file-modified" title={entry.modified}>
                  {entry.modified.replace("T", " ").slice(0, 16)}
                </span>
                <span className="file-type">
                  {t(entry.isSymlink ? "链接" : entry.isDir ? "目录" : "文件")}
                </span>
                <span className="file-size">
                  {entry.isDir ? t("目录") : formatBytes(entry.size)}
                </span>
                <span className="file-mode">{entry.mode}</span>
                <span className="file-owner" title={entry.ownerGroup}>
                  {entry.ownerGroup}
                </span>
                {fileTransfers && (
                  <button
                    disabled={busy || !connected || entry.isSymlink}
                    aria-label={t("下载 {name}", { name: entry.name })}
                    onClick={() => void download([entry.path])}
                  >
                    <Download size={16} />
                  </button>
                )}
              </div>
            ))}
          </div>
        </ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenu.Content className="context-menu">
            <ContextMenu.Item
              disabled={busy || !connected || !editableSelection || !!editing}
              onSelect={() => void edit(selected[0])}
            >
              <FileText size={14} />
              {t("内置编辑器")}
            </ContextMenu.Item>
            {!mobile && (
              <>
                <ContextMenu.Item
                  disabled={
                    busy || !connected || !editableSelection || !!editing
                  }
                  onSelect={() => void externalEdit(selected[0], "")}
                >
                  <Pencil size={14} />
                  {t("外部编辑器")}
                </ContextMenu.Item>
                <ContextMenu.Sub>
                  <ContextMenu.SubTrigger
                    disabled={
                      busy || !connected || !editableSelection || !!editing
                    }
                  >
                    {t("打开方式")}
                  </ContextMenu.SubTrigger>
                  <ContextMenu.Portal>
                    <ContextMenu.SubContent className="context-menu">
                      {editors.map((editor) => (
                        <ContextMenu.Item
                          key={editor.command}
                          onSelect={() =>
                            void externalEdit(selected[0], editor.command)
                          }
                        >
                          {editor.command === "__system__"
                            ? t("系统默认程序")
                            : editor.name}
                        </ContextMenu.Item>
                      ))}
                    </ContextMenu.SubContent>
                  </ContextMenu.Portal>
                </ContextMenu.Sub>
              </>
            )}
            <ContextMenu.Separator />
            <ContextMenu.Item
              disabled={busy || !connected}
              onSelect={() => openAction("mkdir")}
            >
              <FolderPlus size={14} />
              {t("新建目录")}
            </ContextMenu.Item>
            <ContextMenu.Item
              disabled={busy || !connected || selected.length !== 1}
              onSelect={() => openAction("rename")}
            >
              <Pencil size={14} />
              {t("重命名")}
            </ContextMenu.Item>
            {fileTransfers && (
              <ContextMenu.Item
                disabled={busy || !connected || !selected.length}
                onSelect={() => void download(selected)}
              >
                <Download size={14} />
                {t("下载所选")}
              </ContextMenu.Item>
            )}
            {fileTransfers && (
              <ContextMenu.Item
                disabled={busy || !connected}
                onSelect={() => {
                  setOverwrite(false);
                  setUploading(true);
                }}
              >
                <Upload size={14} />
                {t("上传文件或目录")}
              </ContextMenu.Item>
            )}
            <ContextMenu.Separator />
            <ContextMenu.Item
              className="danger"
              disabled={busy || !connected || !selected.length}
              onSelect={() => openAction("delete")}
            >
              <Trash2 size={14} />
              {t("删除所选")}
            </ContextMenu.Item>
          </ContextMenu.Content>
        </ContextMenu.Portal>
      </ContextMenu.Root>
      {latestTransfer && (
        <div className="transfer-status" role="status">
          <span title={latestTransfer.current}>
            {transferLabels[latestTransfer.status]
              ? t(transferLabels[latestTransfer.status])
              : latestTransfer.status}{" "}
            · {latestTransfer.current}
          </span>
          <span>
            {formatBytes(latestTransfer.bytes)} /{" "}
            {formatBytes(latestTransfer.totalBytes)} · {latestTransfer.files}/
            {latestTransfer.totalFiles} {t("文件")}
          </span>
          <progress
            max={100}
            value={transferPercent(latestTransfer)}
            aria-label={t("批次传输进度")}
          />
          {latestTransfer.error && (
            <span className="error">{latestTransfer.error}</span>
          )}
          {transferring && <button onClick={cancel}>{t("取消传输")}</button>}
        </div>
      )}
      {uploading && (
        <Modal
          open={active}
          title={t("上传文件或目录")}
          description={t(
            mobile
              ? "系统选择文件或目录；单批最多 1GiB、50000 项、64 层。Android 11 起不能选择下载根目录或存储卷根目录。已上传文件在取消或失败后保留。"
              : "目录先预检；已完成文件在取消或失败后保留。",
          )}
          onOpenChange={(open) => {
            if (!requestBusy) setUploading(open);
          }}
        >
          <div className="form-grid">
            <label>
              {t("上传内容")}
              <select
                value={uploadDirectory ? "directory" : "file"}
                disabled={requestBusy}
                onChange={(event) =>
                  setUploadDirectory(event.target.value === "directory")
                }
              >
                <option value="file">{t("文件（可多选）")}</option>
                <option value="directory">{t("目录（包含空目录）")}</option>
              </select>
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={overwrite}
                disabled={requestBusy}
                onChange={(event) => setOverwrite(event.target.checked)}
              />
              {t("覆盖同名普通文件并合并目录")}
            </label>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <footer>
              <button
                disabled={requestBusy}
                onClick={() => setUploading(false)}
              >
                {t("返回")}
              </button>
              <button
                className="primary"
                disabled={busy || !connected}
                onClick={() => void upload()}
              >
                {t(requestBusy ? "选择中…" : "选择并上传")}
              </button>
            </footer>
          </div>
        </Modal>
      )}
      {!directory && !busy && (
        <p className="empty">{t("连接后读取远端目录。")}</p>
      )}
      {action && (
        <Modal
          open={active}
          title={t(
            action.kind === "mkdir"
              ? "新建目录"
              : action.kind === "rename"
                ? "重命名"
                : "删除所选项目",
          )}
          description={
            action.kind === "delete"
              ? t("将永久删除 {count} 项及其目录内容，无法恢复。", {
                  count: action.paths.length,
                })
              : t("输入单个名称，已有同名项目会拒绝覆盖。")
          }
          onOpenChange={(open) => {
            if (!open && !busy) setAction(null);
          }}
        >
          <form
            className="form-grid"
            onSubmit={(event) => {
              event.preventDefault();
              submitAction();
            }}
          >
            {action.kind === "delete" ? (
              <ul className="path-text">
                {action.paths.map((value) => (
                  <li key={value}>{value}</li>
                ))}
              </ul>
            ) : (
              <label>
                {t("名称")}
                <input
                  autoFocus
                  required
                  value={action.name}
                  disabled={busy}
                  onChange={(event) =>
                    setAction({ ...action, name: event.target.value })
                  }
                />
              </label>
            )}
            {error && (
              <p role="alert" className="error">
                {error}
              </p>
            )}
            <footer>
              <button
                type="button"
                disabled={busy}
                onClick={() => setAction(null)}
              >
                {t("返回")}
              </button>
              {busy && (
                <button type="button" onClick={cancel}>
                  {t("取消操作")}
                </button>
              )}
              <button
                className={action.kind === "delete" ? "danger" : "primary"}
                disabled={busy || !connected}
              >
                {t(
                  busy
                    ? "处理中…"
                    : action.kind === "delete"
                      ? "永久删除"
                      : "确认",
                )}
              </button>
            </footer>
          </form>
        </Modal>
      )}
      {editing && (
        <Modal
          open={active}
          title={t("编辑远端文本")}
          description={editing.path}
          onOpenChange={(open) => {
            if (
              !open &&
              !busy &&
              (draft === editing.content ||
                window.confirm(t("放弃尚未保存的更改？")))
            )
              setEditing(null);
          }}
        >
          <textarea
            className="text-editor"
            aria-label={t("文件内容")}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            spellCheck={false}
          />
          {error && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
          <footer>
            <span className="muted">
              {t("UTF-8 · 最大 1 MiB · 保存失败保留内容")}
            </span>
            {busy && <button onClick={cancel}>{t("取消操作")}</button>}
            <button
              className="primary"
              disabled={busy || !connected || draft === editing.content}
              onClick={() => void save()}
            >
              {t(busy ? "保存中…" : "保存到服务器")}
            </button>
          </footer>
        </Modal>
      )}
    </section>
  );
}
