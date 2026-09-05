//! 侧边栏 SFTP 树与右键菜单组件。

use dioxus::prelude::dioxus_elements::HasFileData;
use dioxus::prelude::*;
use kt_config::{AppLanguage, SessionProfile};
use kt_core::{SessionId, SftpBatchEntry, SftpEntry, SftpRequest, SftpRequestId};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::components::app::get_state;
use crate::components::app_logic::DEFAULT_GROUP_NAME;
use crate::components::icons::Icon;
use crate::components::sftp::{
    display_path, join_path, normalize_sftp_path_input, parent_path, request_directory,
};
use crate::i18n::texts;
use crate::state::{SftpProgressState, TerminalCdBlocked};

#[derive(Clone, Debug, PartialEq)]
pub enum ContextMenuTarget {
    Profile(String),
    Group(String),
    SftpEntry(SftpEntryContext),
    SftpBlank { session_id: SessionId, path: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuState {
    pub target: ContextMenuTarget,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SftpEntryContext {
    pub session_id: SessionId,
    pub base_path: String,
    pub entry: SftpEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpEntryOpenAction {
    OpenDirectory,
    ExternalEdit,
}

pub fn sftp_entry_open_action(entry: &SftpEntry) -> SftpEntryOpenAction {
    if entry.is_dir {
        SftpEntryOpenAction::OpenDirectory
    } else {
        SftpEntryOpenAction::ExternalEdit
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSubmenuSide {
    Left,
    Right,
}

/// Choose the side with enough viewport room for a fly-out menu.
pub fn context_submenu_side(
    menu_x: f64,
    menu_width: f64,
    submenu_width: f64,
    viewport_width: f64,
) -> ContextSubmenuSide {
    if menu_x + menu_width + submenu_width > viewport_width - 8.0 && menu_x >= submenu_width + 8.0 {
        ContextSubmenuSide::Left
    } else {
        ContextSubmenuSide::Right
    }
}

/// 向指定会话的终端发送 `cd` 命令，把终端切换到 SFTP 当前浏览目录。
fn send_cd_to_terminal(
    state: &std::sync::Arc<std::sync::Mutex<crate::state::AppState>>,
    session_id: SessionId,
    sftp_path: &str,
    language: AppLanguage,
) -> Result<(), String> {
    let Ok(mut app_state) = state.lock() else {
        return Err(texts(language).sftp.state_unavailable.to_string());
    };
    app_state
        .send_terminal_cd(session_id, sftp_path)
        .map_err(|error| terminal_cd_blocked_text(error, language))
}

/// 把终端目录同步被拒绝的原因渲染成用户可见文案。
fn terminal_cd_blocked_text(error: TerminalCdBlocked, language: AppLanguage) -> String {
    let t = texts(language).sftp;
    match error {
        TerminalCdBlocked::Unavailable => t.session_missing.to_string(),
        TerminalCdBlocked::AltScreen => t.sync_blocked_alt_screen.to_string(),
        TerminalCdBlocked::SendFailed => t.sync_send_failed.to_string(),
    }
}

fn set_sftp_sync_error(
    state: &std::sync::Arc<std::sync::Mutex<crate::state::AppState>>,
    session_id: SessionId,
    message: String,
) {
    if let Ok(mut app_state) = state.lock() {
        if let Some(session) = app_state.sessions.get_mut(&session_id) {
            session.sftp_error = Some(message);
        }
    }
}

fn set_sftp_auto_sync(
    state: &std::sync::Arc<std::sync::Mutex<crate::state::AppState>>,
    session_id: SessionId,
    enabled: bool,
) -> Result<(), String> {
    let Ok(mut app_state) = state.lock() else {
        return Err("无法访问应用状态，自动同步设置未生效".to_string());
    };
    app_state.set_sftp_auto_sync(session_id, enabled)
}

pub fn format_sftp_size(size: u64, is_dir: bool) -> String {
    if is_dir {
        String::new()
    } else {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;
        const GB: u64 = 1024 * MB;

        if size >= GB {
            format!("{:.1} GB", size as f64 / GB as f64)
        } else if size >= MB {
            format!("{:.1} MB", size as f64 / MB as f64)
        } else if size >= KB {
            format!("{:.1} KB", size as f64 / KB as f64)
        } else {
            format!("{} B", size)
        }
    }
}

fn format_sftp_time(timestamp: Option<u32>) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    timestamp
        .map(|timestamp| {
            let time = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
            let datetime: chrono::DateTime<chrono::Local> = time.into();
            datetime.format("%Y/%m/%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

pub fn format_sftp_permissions(permissions: Option<u32>, is_dir: bool) -> String {
    let Some(permissions) = permissions else {
        return String::new();
    };

    let file_type = if is_dir {
        'd'
    } else if permissions & 0o120000 == 0o120000 {
        'l'
    } else {
        '-'
    };
    let mut out = String::with_capacity(10);
    out.push(file_type);
    for bit in [
        0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
    ] {
        out.push(match bit {
            0o400 | 0o040 | 0o004 => {
                if permissions & bit != 0 {
                    'r'
                } else {
                    '-'
                }
            }
            0o200 | 0o020 | 0o002 => {
                if permissions & bit != 0 {
                    'w'
                } else {
                    '-'
                }
            }
            _ => {
                if permissions & bit != 0 {
                    'x'
                } else {
                    '-'
                }
            }
        });
    }
    out
}

pub fn format_sftp_owner(entry: &SftpEntry) -> String {
    let user = entry
        .user
        .clone()
        .or_else(|| entry.uid.map(|uid| uid.to_string()))
        .unwrap_or_else(|| "-".to_string());
    let group = entry
        .group
        .clone()
        .or_else(|| entry.gid.map(|gid| gid.to_string()))
        .unwrap_or_else(|| "-".to_string());
    format!("{user}/{group}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingUpload {
    entries: Vec<SftpBatchEntry>,
    target_dir: String,
    conflicts: Vec<String>,
    confirm_all_files: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UploadBatchView {
    request_id: SftpRequestId,
    session_id: SessionId,
    target_dir: String,
    item_count: usize,
    progress: Option<SftpProgressState>,
}

/// 把原生拖放得到的本地文件/目录展开为有序批次；目录先于子项，空目录也保留。
pub fn collect_upload_entries(
    paths: &[PathBuf],
    target_dir: &str,
) -> Result<Vec<SftpBatchEntry>, String> {
    let mut result = Vec::new();
    for path in upload_roots(paths) {
        let name = local_file_name(&path)?;
        collect_upload_path(&path, &join_path(target_dir, &name), &mut result)?;
    }
    Ok(result)
}

fn upload_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = paths.to_vec();
    roots.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    roots.dedup();
    let mut filtered = Vec::with_capacity(roots.len());
    for path in roots {
        if filtered
            .iter()
            .any(|parent: &PathBuf| path == *parent || path.strip_prefix(parent).is_ok())
        {
            continue;
        }
        filtered.push(path);
    }
    filtered
}

fn local_file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("无法读取本地条目名称：{}", path.display()))
}

fn collect_upload_path(
    local: &Path,
    remote: &str,
    result: &mut Vec<SftpBatchEntry>,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(local)
        .map_err(|error| format!("读取本地条目 {} 失败：{error}", local.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(format!("不支持上传符号链接：{}", local.display()));
    }
    if metadata.is_dir() {
        result.push(SftpBatchEntry {
            local: local.to_path_buf(),
            remote: remote.to_string(),
            is_dir: true,
        });
        let mut children = std::fs::read_dir(local)
            .map_err(|error| format!("读取本地目录 {} 失败：{error}", local.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取本地目录 {} 失败：{error}", local.display()))?;
        children.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        for child in children {
            let child_name = local_file_name(&child)?;
            collect_upload_path(&child, &join_path(remote, &child_name), result)?;
        }
    } else if metadata.is_file() {
        result.push(SftpBatchEntry {
            local: local.to_path_buf(),
            remote: remote.to_string(),
            is_dir: false,
        });
    } else {
        return Err(format!("不支持上传特殊文件：{}", local.display()));
    }
    Ok(())
}

fn prepare_upload(
    paths: &[PathBuf],
    target_dir: &str,
    remote_entries: &[SftpEntry],
) -> Result<PendingUpload, String> {
    let entries = collect_upload_entries(paths, target_dir)?;
    let mut conflicts = Vec::new();
    let mut confirm_all_files = false;
    let mut seen_roots = HashSet::new();
    for root in upload_roots(paths) {
        let name = local_file_name(&root)?;
        if !seen_roots.insert(name.to_string()) {
            return Err(format!("多个本地条目将上传到同一个远端名称：{name}"));
        }
        let local_is_dir = std::fs::symlink_metadata(&root)
            .map_err(|error| format!("读取本地条目 {} 失败：{error}", root.display()))?
            .is_dir();
        if local_is_dir {
            confirm_all_files = true;
        }
        if let Some(existing) = remote_entries.iter().find(|entry| entry.name == name) {
            if existing.is_dir != local_is_dir {
                return Err(format!("本地条目 {name} 与远端同名条目类型不同，无法合并"));
            }
            conflicts.push(name.to_string());
        }
    }
    if confirm_all_files && conflicts.is_empty() {
        conflicts.extend(
            upload_roots(paths)
                .iter()
                .filter_map(|root| local_file_name(root).ok()),
        );
    }
    Ok(PendingUpload {
        entries,
        target_dir: target_dir.to_string(),
        conflicts,
        confirm_all_files,
    })
}

fn start_upload_batch(
    state: &std::sync::Arc<std::sync::Mutex<crate::state::AppState>>,
    session_id: SessionId,
    prepared: PendingUpload,
    overwrite: bool,
    mut upload_batch: Signal<Option<UploadBatchView>>,
    language: AppLanguage,
) {
    let target_dir = prepared.target_dir.clone();
    let item_count = prepared.entries.len();
    let overwrite_paths = if overwrite && prepared.confirm_all_files {
        prepared
            .entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.remote.clone())
            .collect()
    } else if overwrite {
        prepared
            .conflicts
            .iter()
            .map(|name| join_path(&target_dir, name))
            .collect()
    } else {
        Vec::new()
    };
    let request = SftpRequest::UploadBatch {
        entries: prepared.entries,
        target_dir: target_dir.clone(),
        overwrite_paths,
    };
    let result = state
        .lock()
        .map_err(|_| texts(language).sftp.state_unavailable.to_string())
        .and_then(|mut app_state| app_state.send_sftp_request(session_id, request));
    match result {
        Ok(request_id) => upload_batch.set(Some(UploadBatchView {
            request_id,
            session_id,
            target_dir,
            item_count,
            progress: None,
        })),
        Err(message) => set_sftp_sync_error(state, session_id, message),
    }
}

#[component]
pub fn SidebarSftpTree(
    session_id: SessionId,
    connected: bool,
    path: String,
    entries: Vec<SftpEntry>,
    loading: bool,
    error: Option<String>,
    auto_sync: bool,
    language: AppLanguage,
    on_context_menu: EventHandler<ContextMenuState>,
    on_entry_open: EventHandler<SftpEntryContext>,
    on_entry_inline_edit: EventHandler<SftpEntryContext>,
    on_auto_sync_change: EventHandler<bool>,
) -> Element {
    let state = get_state().clone();
    let t = texts(language).sftp;
    let mut path_input = use_signal(|| display_path(&path));
    let item_count = entries.len();
    let total_size = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.size)
        .sum::<u64>();
    let auto_sync_class = match (connected, auto_sync) {
        (false, _) => "sftp-auto-sync is-disabled",
        (true, true) => "sftp-auto-sync is-active",
        (true, false) => "sftp-auto-sync",
    };
    let mut drop_target = use_signal(|| None::<String>);
    let mut pending_upload = use_signal(|| None::<PendingUpload>);
    let mut upload_batch = use_signal(|| None::<UploadBatchView>);

    use_effect(use_reactive((&path,), move |(path,)| {
        let display = display_path(&path);
        if *path_input.peek() != display {
            path_input.set(display);
        }
    }));

    use_effect(use_reactive((&connected,), move |(connected,)| {
        if !connected {
            drop_target.set(None);
            pending_upload.set(None);
            upload_batch.set(None);
        }
    }));

    // 批次请求沿用会话级 SFTP 事件队列；这里只观察自己的 request ID，断线时立即收敛。
    let state_for_upload = state.clone();
    use_effect(move || {
        let state_for_upload = state_for_upload.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                let Some(batch) = upload_batch.peek().clone() else {
                    continue;
                };
                let mut clear_batch = false;
                let mut failure = None;
                if let Ok(app_state) = state_for_upload.lock() {
                    if batch.session_id != session_id {
                        upload_batch.set(None);
                        continue;
                    }
                    let Some(session) = app_state.sessions.get(&session_id) else {
                        drop_target.set(None);
                        upload_batch.set(None);
                        continue;
                    };
                    if !session.connected {
                        clear_batch = true;
                        drop_target.set(None);
                    } else if let Some(progress) = session
                        .sftp_progress
                        .as_ref()
                        .filter(|progress| progress.request_id == batch.request_id)
                    {
                        let mut next = batch.clone();
                        next.progress = Some(progress.clone());
                        upload_batch.set(Some(next));
                    } else if !session.sftp_pending_requests.contains(&batch.request_id) {
                        if let Some(item) = session
                            .sftp_failures
                            .iter()
                            .find(|item| item.request_id == batch.request_id)
                        {
                            failure = Some(item.message.clone());
                        } else if session
                            .sftp_completions
                            .iter()
                            .any(|item| item.request_id == batch.request_id)
                        {
                            clear_batch = true;
                        } else {
                            // SFTP 子任务停止时不会为未完成请求补发终态，避免 UI 永久显示上传中。
                            clear_batch = true;
                        }
                    }
                } else {
                    clear_batch = true;
                }
                if let Some(message) = failure {
                    set_sftp_sync_error(&state_for_upload, session_id, message);
                    clear_batch = true;
                }
                if clear_batch {
                    upload_batch.set(None);
                }
            }
        });
    });

    let drop_path_for_over = path.clone();
    let drop_path_for_leave = path.clone();

    rsx! {
        div {
            class: "sidebar-sftp-tree",

            div {
                class: "sftp-path-line",
                Icon { name: "folder" }
                input {
                    class: "sftp-path-input",
                    r#type: "text",
                    value: "{path_input()}",
                    disabled: !connected,
                    placeholder: "{t.path}",
                    oninput: move |evt| {
                        path_input.set(evt.value());
                    },
                    onkeydown: {
                        let state = state.clone();
                        let path = path.clone();
                        move |evt| {
                            if evt.key() == Key::Enter {
                                evt.stop_propagation();
                                evt.prevent_default();
                                match normalize_sftp_path_input(&path_input()) {
                                    Some(next) => {
                                        path_input.set(display_path(&next));
                                        if let Err(e) = request_directory(
                                            state.clone(),
                                            session_id,
                                            next,
                                            language,
                                        ) {
                                            tracing::error!("SFTP 路径跳转失败: {}", e);
                                        }
                                    }
                                    None => {
                                        path_input.set(display_path(&path));
                                    }
                                }
                            }
                        }
                    },
                }
            }

            div {
                class: "sftp-tree-actions",
                button {
                    title: "{t.back}",
                    disabled: !connected || path == "/" || path == ".",
                    onclick: {
                        let path = path.clone();
                        let state = state.clone();
                        move |evt| {
                            evt.stop_propagation();
                            if path != "/" && path != "." {
                                let parent = parent_path(&path);
                                if let Err(e) = request_directory(state.clone(), session_id, parent, language) {
                                    tracing::error!("SFTP 返回上级失败: {}", e);
                                }
                            }
                        }
                    },
                    Icon { name: "back" }
                }
                button {
                    title: "{t.refresh}",
                    disabled: !connected,
                    onclick: {
                        let path = path.clone();
                        let state = state.clone();
                        move |evt| {
                            evt.stop_propagation();
                            if let Err(e) = request_directory(state.clone(), session_id, path.clone(), language) {
                                tracing::error!("SFTP 刷新失败: {}", e);
                            }
                        }
                    },
                    Icon { name: "refresh" }
                }
                // SFTP → 终端：把当前终端切换到文件管理所在目录。
                button {
                    title: "{t.sync_to_terminal}",
                    disabled: !connected,
                    onclick: {
                        let path = path.clone();
                        let state = state.clone();
                        move |evt| {
                            evt.stop_propagation();
                            if let Err(message) = send_cd_to_terminal(&state, session_id, &path, language) {
                                set_sftp_sync_error(&state, session_id, message);
                            }
                        }
                    },
                    Icon { name: "split-vertical" }
                }
                label {
                    class: auto_sync_class,
                    title: "{t.auto_sync}",
                    input {
                        r#type: "checkbox",
                        checked: auto_sync,
                        disabled: !connected,
                        onchange: {
                            let state = state.clone();
                            move |evt: Event<FormData>| {
                                let enabled = evt.checked();
                                match set_sftp_auto_sync(&state, session_id, enabled) {
                                    // 记住这次选择，之后新建的会话直接沿用。
                                    Ok(()) => on_auto_sync_change.call(enabled),
                                    Err(message) => {
                                        set_sftp_sync_error(&state, session_id, message)
                                    }
                                }
                            }
                        },
                    }
                    span { "{t.auto_sync}" }
                }
            }

            div {
                class: if drop_target().as_deref() == Some(path.as_str()) {
                    "sftp-tree-list is-drop-target"
                } else {
                    "sftp-tree-list"
                },
                ondragover: move |evt| {
                    if !connected {
                        return;
                    }
                    evt.prevent_default();
                    evt.stop_propagation();
                    drop_target.set(Some(drop_path_for_over.clone()));
                },
                ondragleave: move |evt| {
                    evt.stop_propagation();
                    if drop_target.peek().as_deref() == Some(drop_path_for_leave.as_str()) {
                        drop_target.set(None);
                    }
                },
                ondrop: {
                    let path = path.clone();
                    let remote_entries = entries.clone();
                    move |evt| {
                        if !connected {
                            return;
                        }
                        evt.prevent_default();
                        evt.stop_propagation();
                        drop_target.set(None);
                        let paths = evt
                            .data()
                            .files()
                            .into_iter()
                            .map(|file| file.path())
                            .collect::<Vec<_>>();
                        if paths.is_empty() {
                            return;
                        }
                        if upload_batch.peek().is_some() {
                            set_sftp_sync_error(
                                &state,
                                session_id,
                                t.upload_in_progress.to_string(),
                            );
                            return;
                        }
                        match prepare_upload(&paths, &path, &remote_entries) {
                            Ok(prepared)
                                if prepared.conflicts.is_empty() && !prepared.confirm_all_files =>
                            {
                                start_upload_batch(
                                    &state,
                                    session_id,
                                    prepared,
                                    false,
                                    upload_batch,
                                    language,
                                );
                            }
                            Ok(prepared) => pending_upload.set(Some(prepared)),
                            Err(message) => set_sftp_sync_error(&state, session_id, message),
                        }
                    }
                },
                oncontextmenu: {
                    let path = path.clone();
                    move |evt| {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_context_menu.call(ContextMenuState {
                            target: ContextMenuTarget::SftpBlank {
                                session_id,
                                path: path.clone(),
                            },
                            x: evt.client_coordinates().x,
                            y: evt.client_coordinates().y,
                        });
                    }
                },

                div {
                    class: "sftp-table-grid",

                    div {
                        class: "sftp-table-head-row",
                        span { class: "sftp-name-col", "{t.name}" }
                        span { "{t.modified}" }
                        span { "{t.size}" }
                        span { "{t.permissions}" }
                        span { "{t.owner_group}" }
                    }

                    if loading {
                        div { class: "sftp-table-message", "{t.loading}" }
                    } else if let Some(error) = error.clone() {
                        div { class: "sftp-table-message error", "{t.error_prefix}: {error}" }
                    } else if entries.is_empty() {
                        div { class: "sftp-table-message", "0 {t.items}" }
                    } else {
                        for entry in entries.clone() {
                            SidebarSftpEntry {
                                key: "{entry.name}",
                                session_id,
                                base_path: path.clone(),
                                entry,
                                on_context_menu,
                                on_entry_open,
                                on_entry_inline_edit,
                                drop_target,
                                on_file_drop: {
                                    let state = state.clone();
                                    move |(drop_path, paths): (String, Vec<PathBuf>)| {
                                        if upload_batch.peek().is_some() {
                                            set_sftp_sync_error(
                                                &state,
                                                session_id,
                                                t.upload_in_progress.to_string(),
                                            );
                                            return;
                                        }
                                        // 目录内容在当前列表之外，先确认一次，再由 core
                                        // 按精确批次路径覆盖文件并合并目录。
                                        match prepare_upload(&paths, &drop_path, &[]) {
                                            Ok(mut prepared) => {
                                                prepared.confirm_all_files = true;
                                                if prepared.conflicts.is_empty() {
                                                    prepared.conflicts.push(drop_path.clone());
                                                }
                                                pending_upload.set(Some(prepared));
                                            }
                                            Err(message) => set_sftp_sync_error(&state, session_id, message),
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
            }

            div {
                class: "sftp-table-status",
                span { "{item_count} {t.items}" }
                span { "{format_sftp_size(total_size, false)}" }
            }

            if let Some(batch) = upload_batch() {
                div {
                    class: "sftp-upload-status",
                    title: "{batch.target_dir}",
                    Icon { name: "upload" }
                    span {
                        if let Some(progress) = batch.progress {
                            "{t.uploading} {progress.name} ({progress.transferred}/{progress.total})"
                        } else {
                            "{t.uploading} {batch.item_count} {t.items}"
                        }
                    }
                }
            }

            if let Some(conflict) = pending_upload() {
                div {
                    class: "settings-overlay upload-conflict-overlay",
                    onclick: move |_| pending_upload.set(None),
                    section {
                        class: "settings-panel group-dialog upload-conflict-dialog",
                        onclick: move |evt| {
                            evt.stop_propagation();
                            evt.prevent_default();
                        },
                        div {
                            class: "settings-head",
                            h2 { "{t.upload_conflict_title}" }
                            button {
                                class: "icon-button slim",
                                title: "{t.close}",
                                onclick: move |_| pending_upload.set(None),
                                Icon { name: "close" }
                            }
                        }
                        div {
                            class: "group-form",
                            p { "{t.upload_conflict_body}" }
                            for name in conflict.conflicts.iter() {
                                code { "{name}" }
                            }
                        }
                        div {
                            class: "group-actions",
                            button {
                                onclick: move |_| pending_upload.set(None),
                                "{t.upload_cancel}"
                            }
                            button {
                                class: "primary",
                                onclick: {
                                    let state = state.clone();
                                    move |_| {
                                        let Some(prepared) = pending_upload.peek().clone() else {
                                            return;
                                        };
                                        pending_upload.set(None);
                                        start_upload_batch(
                                            &state,
                                            session_id,
                                            prepared,
                                            true,
                                            upload_batch,
                                            language,
                                        );
                                    }
                                },
                                "{t.upload_overwrite}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn SidebarSftpEntry(
    session_id: SessionId,
    base_path: String,
    entry: SftpEntry,
    on_context_menu: EventHandler<ContextMenuState>,
    on_entry_open: EventHandler<SftpEntryContext>,
    on_entry_inline_edit: EventHandler<SftpEntryContext>,
    drop_target: Signal<Option<String>>,
    on_file_drop: EventHandler<(String, Vec<PathBuf>)>,
) -> Element {
    let icon = if entry.is_dir { "folder" } else { "file" };
    let row_class = if entry.is_dir {
        "sftp-table-row is-dir"
    } else {
        "sftp-table-row is-file"
    };
    let name = entry.name.clone();
    let is_dir = entry.is_dir;
    let modified = format_sftp_time(entry.modified);
    let size = format_sftp_size(entry.size, entry.is_dir);
    let permissions = format_sftp_permissions(entry.permissions, entry.is_dir);
    let owner = format_sftp_owner(&entry);
    let full_path = join_path(&base_path, &name);
    let double_open_base_path = base_path.clone();
    let double_open_entry = entry.clone();
    let drop_path = full_path.clone();
    let drop_path_for_leave = full_path.clone();
    let drop_path_for_drop = full_path.clone();
    let row_class = row_class.to_string();
    let row_class_with_drop = format!("{row_class} is-drop-target");

    rsx! {
        div {
            class: if drop_target().as_deref() == Some(full_path.as_str()) {
                "{row_class_with_drop}"
            } else {
                "{row_class}"
            },
            title: "{full_path}",
            onclick: move |evt| {
                evt.stop_propagation();
            },
            ondoubleclick: move |evt| {
                evt.stop_propagation();
                let ctx = SftpEntryContext {
                    session_id,
                    base_path: double_open_base_path.clone(),
                    entry: double_open_entry.clone(),
                };
                match sftp_entry_open_action(&ctx.entry) {
                    SftpEntryOpenAction::OpenDirectory => on_entry_open.call(ctx),
                    // 内置编辑器是普通文件的默认入口；外部编辑仍可从右键菜单选择。
                    SftpEntryOpenAction::ExternalEdit => on_entry_inline_edit.call(ctx),
                }
            },
            ondragover: move |evt| {
                if !is_dir {
                    return;
                }
                evt.prevent_default();
                evt.stop_propagation();
                drop_target.set(Some(drop_path.clone()));
            },
            ondragleave: move |evt| {
                evt.stop_propagation();
                if drop_target.peek().as_deref() == Some(drop_path_for_leave.as_str()) {
                    drop_target.set(None);
                }
            },
            ondrop: move |evt| {
                if !is_dir {
                    return;
                }
                evt.prevent_default();
                evt.stop_propagation();
                drop_target.set(None);
                let paths = evt
                    .data()
                    .files()
                    .into_iter()
                    .map(|file| file.path())
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    on_file_drop.call((drop_path_for_drop.clone(), paths));
                }
            },
            oncontextmenu: {
                let base_path = base_path.clone();
                let entry = entry.clone();
                move |evt| {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_context_menu.call(ContextMenuState {
                        target: ContextMenuTarget::SftpEntry(SftpEntryContext {
                            session_id,
                            base_path: base_path.clone(),
                            entry: entry.clone(),
                        }),
                        x: evt.client_coordinates().x,
                        y: evt.client_coordinates().y,
                    });
                }
            },

            span {
                class: "sftp-name-cell",
                Icon { name: icon }
                span { "{name}" }
            }
            span { "{modified}" }
            span { "{size}" }
            span { "{permissions}" }
            span { "{owner}" }
        }
    }
}

#[component]
pub fn ContextMenu(
    menu: ContextMenuState,
    language: AppLanguage,
    editors: Vec<kt_config::EditorEntry>,
    on_profile_edit: EventHandler<String>,
    on_profile_delete: EventHandler<String>,
    on_profile_copy: EventHandler<String>,
    on_group_new: EventHandler<()>,
    on_group_rename: EventHandler<String>,
    on_group_delete: EventHandler<String>,
    on_sftp_open: EventHandler<SftpEntryContext>,
    on_sftp_refresh: EventHandler<(SessionId, String)>,
    on_sftp_mkdir: EventHandler<(SessionId, String)>,
    on_sftp_rename: EventHandler<SftpEntryContext>,
    on_sftp_delete: EventHandler<SftpEntryContext>,
    on_sftp_inline_edit: EventHandler<SftpEntryContext>,
    on_sftp_external_edit: EventHandler<SftpEntryContext>,
    on_sftp_open_with: EventHandler<(SftpEntryContext, Option<String>)>,
    on_copy_text: EventHandler<String>,
) -> Element {
    let t = texts(language).app;
    let sftp_t = texts(language).sftp;
    let style = format!("left: {:.0}px; top: {:.0}px;", menu.x, menu.y);
    let menu_x = menu.x;
    let menu_y = menu.y;

    use_effect(move || {
        let script = format!(
            r#"
            requestAnimationFrame(() => {{
                const menu = document.querySelector('[data-kt-context-menu="active"]');
                if (!menu) return;
                const margin = 8;
                menu.style.left = '{menu_x}px';
                menu.style.top = '{menu_y}px';
                menu.style.right = 'auto';
                menu.style.bottom = 'auto';
                const rect = menu.getBoundingClientRect();
                if (rect.right > window.innerWidth - margin) {{
                    menu.style.left = `${{Math.max(margin, window.innerWidth - rect.width - margin)}}px`;
                }}
                if (rect.bottom > window.innerHeight - margin) {{
                    menu.style.top = 'auto';
                    menu.style.bottom = `${{Math.max(margin, window.innerHeight - {menu_y})}}px`;
                }}

                const placeSubmenus = () => {{
                    menu.querySelectorAll('.context-submenu').forEach((item) => {{
                        const panel = item.querySelector('.context-submenu-panel');
                        if (!panel) return;
                        panel.style.left = '100%';
                        panel.style.right = 'auto';
                        panel.style.marginLeft = '0';
                        const itemRect = item.getBoundingClientRect();
                        const panelRect = panel.getBoundingClientRect();
                        if (itemRect.right + panelRect.width > window.innerWidth - margin) {{
                            panel.style.left = 'auto';
                            panel.style.right = '100%';
                        }}
                    }});
                }};
                if (!menu.__ktSubmenuPlacement) {{
                    menu.addEventListener('pointerover', placeSubmenus);
                    menu.addEventListener('focusin', placeSubmenus);
                    menu.__ktSubmenuPlacement = true;
                }}
                placeSubmenus();
            }});
            "#
        );
        dioxus::document::eval(&script);
    });

    rsx! {
        div {
            class: "context-menu",
            "data-kt-context-menu": "active",
            style: "{style}",
            onclick: move |evt| evt.stop_propagation(),
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
            },

            match menu.target.clone() {
                ContextMenuTarget::Profile(name) => rsx! {
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_edit.call(name.clone())
                        },
                        Icon { name: "edit" }
                        span { "{t.edit}" }
                    }
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_copy.call(name.clone())
                        },
                        Icon { name: "file" }
                        span { "{t.copy}" }
                    }
                    button {
                        class: "danger",
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_delete.call(name.clone())
                        },
                        Icon { name: "trash" }
                        span { "{t.delete}" }
                    }
                },
                ContextMenuTarget::Group(name) => rsx! {
                    button {
                        onclick: move |_| on_group_new.call(()),
                        Icon { name: "folder" }
                        span { "{t.new_group}" }
                    }
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_group_rename.call(name.clone())
                        },
                        Icon { name: "edit" }
                        span { "{t.rename_group}" }
                    }
                    if name != DEFAULT_GROUP_NAME {
                        button {
                            class: "danger",
                            onclick: {
                                let name = name.clone();
                                move |_| on_group_delete.call(name.clone())
                            },
                            Icon { name: "trash" }
                            span { "{t.delete_group}" }
                        }
                    }
                },
                ContextMenuTarget::SftpEntry(ctx) => {
                    let full_path = join_path(&ctx.base_path, &ctx.entry.name);
                    let is_dir = ctx.entry.is_dir;
                    rsx! {
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_refresh.call((ctx.session_id, ctx.base_path.clone()))
                            },
                            Icon { name: "refresh" }
                            span { "{sftp_t.refresh}" }
                            small { "F5" }
                        }
                        if is_dir {
                            button {
                                onclick: {
                                    let ctx = ctx.clone();
                                    move |_| on_sftp_open.call(ctx.clone())
                                },
                                Icon { name: "folder" }
                                span { "{sftp_t.open}" }
                                small { "Return" }
                            }
                        }
                        if !is_dir {
                            button {
                                onclick: {
                                    let ctx = ctx.clone();
                                    move |_| on_sftp_inline_edit.call(ctx.clone())
                                },
                                Icon { name: "edit" }
                                span { "{sftp_t.edit_inline}" }
                            }
                            button {
                                onclick: {
                                    let ctx = ctx.clone();
                                    move |_| on_sftp_external_edit.call(ctx.clone())
                                },
                                Icon { name: "edit" }
                                span { "{sftp_t.edit_external}" }
                            }
                            div {
                                class: "context-submenu",
                                button {
                                    class: "context-submenu-trigger",
                                    Icon { name: "edit" }
                                    span { "{sftp_t.open_with}" }
                                    small { "▸" }
                                }
                                div {
                                    class: "context-submenu-panel",
                                    button {
                                        onclick: {
                                            let ctx = ctx.clone();
                                            move |_| on_sftp_open_with.call((ctx.clone(), None))
                                        },
                                        Icon { name: "file" }
                                        span { "{sftp_t.open_with_system}" }
                                    }
                                    for editor in editors.clone() {
                                        button {
                                            onclick: {
                                                let ctx = ctx.clone();
                                                let command = editor.command.clone();
                                                move |_| on_sftp_open_with.call((ctx.clone(), Some(command.clone())))
                                            },
                                            Icon { name: "edit" }
                                            span { "{editor.name}" }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "context-separator" }
                        button {
                            onclick: {
                                let full_path = full_path.clone();
                                move |_| on_copy_text.call(full_path.clone())
                            },
                            Icon { name: "file" }
                            span { "{sftp_t.copy_path}" }
                        }
                        button {
                            onclick: {
                                let name = ctx.entry.name.clone();
                                move |_| on_copy_text.call(name.clone())
                            },
                            Icon { name: "file" }
                            span { "{sftp_t.copy_name}" }
                        }
                        div { class: "context-separator" }
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_mkdir.call((ctx.session_id, ctx.base_path.clone()))
                            },
                            Icon { name: "folder" }
                            span { "{sftp_t.new_folder}" }
                        }
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_rename.call(ctx.clone())
                            },
                            Icon { name: "edit" }
                            span { "{sftp_t.rename}" }
                            small { "F2" }
                        }
                        button {
                            class: "danger",
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_delete.call(ctx.clone())
                            },
                            Icon { name: "trash" }
                            span { "{sftp_t.delete}" }
                            small { "Del" }
                        }
                    }
                },
                ContextMenuTarget::SftpBlank { session_id, path } => rsx! {
                    button {
                        onclick: {
                            let path = path.clone();
                            move |_| on_sftp_refresh.call((session_id, path.clone()))
                        },
                        Icon { name: "refresh" }
                        span { "{sftp_t.refresh}" }
                        small { "F5" }
                    }
                    button {
                        onclick: {
                            let path = path.clone();
                            move |_| on_sftp_mkdir.call((session_id, path.clone()))
                        },
                        Icon { name: "folder" }
                        span { "{sftp_t.new_folder}" }
                    }
                },
            }
        }
    }
}

#[component]
pub fn ConnectionCard(
    profile: SessionProfile,
    active: bool,
    language: AppLanguage,
    on_connect: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_copy: EventHandler<()>,
    on_context_menu: EventHandler<(f64, f64)>,
) -> Element {
    let t = texts(language).app;

    rsx! {
        div {
            class: if active { "connection-card is-active" } else { "connection-card" },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                on_context_menu.call((evt.client_coordinates().x, evt.client_coordinates().y));
            },

            div {
                class: "connection-main",
                onclick: move |_| on_connect.call(()),

                span { class: "status-dot online" }
                div {
                    class: "connection-copy",
                    strong { "{profile.name}" }
                }
            }

            div {
                class: "connection-actions",
                button {
                    title: "{t.edit}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_edit.call(());
                    },
                    Icon { name: "edit" }
                }
                button {
                    title: "{t.copy}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_copy.call(());
                    },
                    Icon { name: "file" }
                }
                button {
                    class: "danger",
                    title: "{t.delete}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_delete.call(());
                    },
                    Icon { name: "trash" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_cd_blocked_reasons_are_localized_and_distinct() {
        // 命令构造本身由 kt-core 的 shell_integration 测试覆盖；这里只保证每个
        // 拒绝原因都有可见文案，且中英文都不落回另一种语言。
        for language in [AppLanguage::Chinese, AppLanguage::English] {
            let messages = [
                TerminalCdBlocked::Unavailable,
                TerminalCdBlocked::AltScreen,
                TerminalCdBlocked::SendFailed,
            ]
            .map(|error| terminal_cd_blocked_text(error, language));

            for message in &messages {
                assert!(!message.trim().is_empty(), "语言 {language:?} 缺少文案");
            }
            assert_ne!(messages[0], messages[1]);
            assert_ne!(messages[1], messages[2]);
        }

        assert_ne!(
            terminal_cd_blocked_text(TerminalCdBlocked::AltScreen, AppLanguage::Chinese),
            terminal_cd_blocked_text(TerminalCdBlocked::AltScreen, AppLanguage::English)
        );
    }

    #[test]
    fn sftp_permissions_are_rendered_like_unix_modes() {
        assert_eq!(format_sftp_permissions(Some(0o040755), true), "drwxr-xr-x");
        assert_eq!(format_sftp_permissions(Some(0o100600), false), "-rw-------");
        assert_eq!(format_sftp_permissions(None, false), "");
    }

    #[test]
    fn sftp_owner_prefers_names_and_falls_back_to_ids() {
        let named = SftpEntry {
            name: "demo".to_string(),
            is_dir: false,
            size: 1,
            modified: None,
            permissions: None,
            user: Some("root".to_string()),
            group: Some("wheel".to_string()),
            uid: Some(0),
            gid: Some(0),
        };
        assert_eq!(format_sftp_owner(&named), "root/wheel");

        let numeric = SftpEntry {
            user: None,
            group: None,
            ..named
        };
        assert_eq!(format_sftp_owner(&numeric), "0/0");
    }

    #[test]
    fn sftp_double_click_action_opens_dirs_and_edits_files() {
        let dir = SftpEntry {
            name: "logs".to_string(),
            is_dir: true,
            size: 0,
            modified: None,
            permissions: None,
            user: None,
            group: None,
            uid: None,
            gid: None,
        };
        let file = SftpEntry {
            name: "app.log".to_string(),
            is_dir: false,
            ..dir.clone()
        };

        assert_eq!(
            sftp_entry_open_action(&dir),
            SftpEntryOpenAction::OpenDirectory
        );
        assert_eq!(
            sftp_entry_open_action(&file),
            SftpEntryOpenAction::ExternalEdit
        );
    }

    #[test]
    fn open_with_submenu_flips_left_only_when_right_edge_is_occupied() {
        assert_eq!(
            context_submenu_side(700.0, 190.0, 180.0, 1024.0),
            ContextSubmenuSide::Left
        );
        assert_eq!(
            context_submenu_side(120.0, 190.0, 180.0, 1024.0),
            ContextSubmenuSide::Right
        );
        assert_eq!(
            context_submenu_side(2.0, 190.0, 180.0, 200.0),
            ContextSubmenuSide::Right
        );
    }

    #[test]
    fn open_with_submenu_uses_the_shared_theme_tokens() {
        let css = include_str!("../assets/app.css");
        let submenu = css
            .split_once(".context-submenu-panel {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(body, _)| body)
            .expect("缺少打开方式子菜单样式");
        assert!(submenu.contains("var(--menu-bg)"));
        assert!(submenu.contains("var(--menu-border)"));
        assert!(submenu.contains("var(--menu-shadow)"));
    }

    #[test]
    fn collect_upload_entries_preserves_nested_and_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bundle");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("readme.txt"), b"hello").unwrap();

        let entries = collect_upload_entries(std::slice::from_ref(&root), "/srv").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].remote, "/srv/bundle");
        assert!(entries[0].is_dir);
        assert!(entries
            .iter()
            .any(|entry| entry.remote == "/srv/bundle/nested" && entry.is_dir));
        assert!(entries
            .iter()
            .any(|entry| entry.remote == "/srv/bundle/readme.txt" && !entry.is_dir));
    }

    #[test]
    fn collect_upload_entries_rejects_symlinks_and_special_files() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("link");
        let target = temp.path().join("target");
        std::fs::write(&target, b"data").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let error = collect_upload_entries(&[link], ".").unwrap_err();
            assert!(error.contains("符号链接"));
        }

        #[cfg(not(unix))]
        {
            let _ = link;
        }
    }

    #[test]
    fn prepare_upload_reports_file_conflicts_but_merges_existing_directories() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("notes.txt");
        let folder = temp.path().join("folder");
        std::fs::write(&file, b"new").unwrap();
        std::fs::create_dir(&folder).unwrap();

        let remote_entries = vec![
            SftpEntry {
                name: "notes.txt".to_string(),
                is_dir: false,
                size: 1,
                modified: None,
                permissions: None,
                user: None,
                group: None,
                uid: None,
                gid: None,
            },
            SftpEntry {
                name: "folder".to_string(),
                is_dir: true,
                size: 0,
                modified: None,
                permissions: None,
                user: None,
                group: None,
                uid: None,
                gid: None,
            },
        ];
        let prepared = prepare_upload(&[file, folder], "/srv", &remote_entries).unwrap();
        assert_eq!(prepared.conflicts, vec!["folder", "notes.txt"]);
        assert!(prepared.confirm_all_files);
        assert_eq!(prepared.entries.len(), 2);
    }
}
