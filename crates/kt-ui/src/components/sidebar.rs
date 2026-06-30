//! 侧边栏 SFTP 树与右键菜单组件。

use dioxus::prelude::*;
use kt_config::{AppLanguage, SessionProfile};
use kt_core::{SessionId, SftpEntry};

use crate::components::app::get_state;
use crate::components::app_logic::DEFAULT_GROUP_NAME;
use crate::components::icons::Icon;
use crate::components::sftp::{
    display_path, join_path, normalize_sftp_path_input, parent_path, request_directory,
};
use crate::i18n::texts;

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

#[component]
pub fn SidebarSftpTree(
    session_id: SessionId,
    connected: bool,
    path: String,
    entries: Vec<SftpEntry>,
    loading: bool,
    error: Option<String>,
    language: AppLanguage,
    on_context_menu: EventHandler<ContextMenuState>,
    on_entry_open: EventHandler<SftpEntryContext>,
    on_entry_external_edit: EventHandler<SftpEntryContext>,
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

    use_effect({
        let path = path.clone();
        move || {
            let display = display_path(&path);
            if *path_input.peek() != display {
                path_input.set(display);
            }
        }
    });

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
            }

            div {
                class: "sftp-tree-list",
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
                                on_entry_external_edit,
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
    on_entry_external_edit: EventHandler<SftpEntryContext>,
) -> Element {
    let icon = if entry.is_dir { "folder" } else { "file" };
    let row_class = if entry.is_dir {
        "sftp-table-row is-dir"
    } else {
        "sftp-table-row is-file"
    };
    let name = entry.name.clone();
    let modified = format_sftp_time(entry.modified);
    let size = format_sftp_size(entry.size, entry.is_dir);
    let permissions = format_sftp_permissions(entry.permissions, entry.is_dir);
    let owner = format_sftp_owner(&entry);
    let full_path = join_path(&base_path, &name);
    let double_open_base_path = base_path.clone();
    let double_open_entry = entry.clone();

    rsx! {
        div {
            class: "{row_class}",
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
                    SftpEntryOpenAction::ExternalEdit => on_entry_external_edit.call(ctx),
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
    on_sftp_external_edit: EventHandler<SftpEntryContext>,
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
                                    move |_| on_sftp_external_edit.call(ctx.clone())
                                },
                                Icon { name: "edit" }
                                span { "{sftp_t.edit_external}" }
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
}
