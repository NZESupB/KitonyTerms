//! SFTP 文件管理面板组件。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{SessionId, SftpEntry, SftpRequest, ToCore};
use std::sync::{Arc, Mutex};

use crate::components::icons::Icon;
use crate::i18n::{sftp_timeout_message, texts};
use crate::state::AppState;

const UI_SFTP_TIMEOUT_SECS: u64 = 15;

#[component]
pub fn SftpPanel(session_id: SessionId, language: AppLanguage) -> Element {
    let state = crate::components::app::get_state().clone();
    let state_for_load = state.clone();
    let state_for_sync = state.clone();
    let state_for_back = state.clone();
    let state_for_refresh = state.clone();

    let mut current_path = use_signal(|| ".".to_string());
    let mut entries = use_signal(Vec::<SftpEntry>::new);
    let mut loading = use_signal(|| false);
    let mut error_message = use_signal(|| None::<String>);
    let mut did_initial_load = use_signal(|| false);
    let t = texts(language).sftp;

    use_effect(move || {
        if !did_initial_load() {
            did_initial_load.set(true);
            load_directory(
                state_for_load.clone(),
                session_id,
                ".".to_string(),
                loading,
                entries,
                error_message,
                language,
            );
        }
    });

    use_effect(move || {
        let value = state_for_sync.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                if let Ok(app_state) = value.lock() {
                    if let Some(sess) = app_state.sessions.get(&session_id) {
                        if *current_path.peek() != sess.sftp_path {
                            current_path.set(sess.sftp_path.clone());
                        }
                        if !entries_same(&entries.peek(), &sess.sftp_entries) {
                            entries.set(sess.sftp_entries.clone());
                        }
                        if *loading.peek() != sess.sftp_loading {
                            loading.set(sess.sftp_loading);
                        }
                        if *error_message.peek() != sess.sftp_error {
                            error_message.set(sess.sftp_error.clone());
                        }
                    }
                }
            }
        });
    });

    rsx! {
        div {
            class: "sftp-panel",

            div {
                class: "sftp-titlebar",
                strong {
                    Icon { name: "folder" }
                    "{t.title}"
                }
            }

            div {
                class: "sftp-toolbar",
                button {
                    class: "icon-button slim",
                    title: "{t.back}",
                    disabled: current_path() == "/" || current_path() == ".",
                    onclick: move |_| {
                        let path = current_path();
                        if path != "/" && path != "." {
                            let parent = parent_path(&path);
                            current_path.set(parent.clone());
                            load_directory(
                                state_for_back.clone(),
                                session_id,
                                parent,
                                loading,
                                entries,
                                error_message,
                                language,
                            );
                        }
                    },
                    Icon { name: "back" }
                }

                button {
                    class: "icon-button slim",
                    title: "{t.refresh}",
                    onclick: move |_| {
                        load_directory(
                            state_for_refresh.clone(),
                            session_id,
                            current_path(),
                            loading,
                            entries,
                            error_message,
                            language,
                        );
                    },
                    Icon { name: "refresh" }
                }

                div {
                    class: "sftp-path",
                    "{display_path(&current_path())}"
                }
            }

            div {
                class: "sftp-table",

                div {
                    class: "sftp-table-head",
                    span { "{t.name}" }
                    span { "{t.size}" }
                    span { "{t.modified}" }
                }

                if loading() {
                    div { class: "sftp-message", "{t.loading}" }
                } else if let Some(err) = error_message() {
                    div { class: "sftp-message error", "{t.error_prefix}: {err}" }
                } else {
                    div {
                        class: "sftp-row is-parent",
                        onclick: move |_| {
                            let path = current_path();
                            if path != "/" && path != "." {
                                let parent = parent_path(&path);
                                current_path.set(parent.clone());
                                load_directory(
                                    state.clone(),
                                    session_id,
                                    parent,
                                    loading,
                                    entries,
                                    error_message,
                                    language,
                                );
                            }
                        },
                        span {
                            class: "file-name",
                            span { class: "file-icon folder", Icon { name: "folder" } }
                            ".."
                        }
                        span {}
                        span {}
                    }

                    for entry in entries() {
                        SftpRow {
                            key: "{entry.name}",
                            name: entry.name.clone(),
                            is_dir: entry.is_dir,
                            size: entry.size,
                            modified: entry.modified,
                            current_path: current_path(),
                            on_open: {
                                let state = state.clone();
                                move |path: String| {
                                    current_path.set(path.clone());
                                    load_directory(
                                        state.clone(),
                                        session_id,
                                        path,
                                        loading,
                                        entries,
                                        error_message,
                                        language,
                                    );
                                }
                            },
                        }
                    }
                }
            }

            div {
                class: "sftp-footer",
                span { "{entries().len()} {t.items}" }
            }
        }
    }
}

#[component]
fn SftpRow(
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u32>,
    current_path: String,
    on_open: EventHandler<String>,
) -> Element {
    let file_size = if is_dir {
        String::new()
    } else {
        format_size(size)
    };
    let modified = modified.map(format_time).unwrap_or_default();
    let file_icon = if is_dir { "folder" } else { "file" };
    let file_icon_class = if is_dir {
        "file-icon folder"
    } else {
        "file-icon document"
    };

    rsx! {
        div {
            class: "sftp-row",
            onclick: move |_| {
                if is_dir {
                    on_open.call(join_path(&current_path, &name));
                }
            },

            span {
                class: "file-name",
                span {
                    class: file_icon_class,
                    Icon { name: file_icon }
                }
                "{name}"
            }
            span { "{file_size}" }
            span { "{modified}" }
        }
    }
}

fn load_directory(
    state: Arc<Mutex<AppState>>,
    session_id: SessionId,
    path: String,
    mut loading: Signal<bool>,
    mut entries: Signal<Vec<SftpEntry>>,
    mut error_message: Signal<Option<String>>,
    language: AppLanguage,
) {
    loading.set(true);
    entries.set(Vec::new());
    error_message.set(None);

    if let Err(message) = request_directory(state, session_id, path, language) {
        loading.set(false);
        error_message.set(Some(message));
    }
}

pub(crate) fn request_directory(
    state: Arc<Mutex<AppState>>,
    session_id: SessionId,
    path: String,
    language: AppLanguage,
) -> Result<(), String> {
    let requested_path = path.clone();

    {
        let Ok(mut app_state) = state.lock() else {
            return Err(texts(language).sftp.state_unavailable.to_string());
        };

        let Some(sess) = app_state.sessions.get_mut(&session_id) else {
            return Err(texts(language).sftp.session_missing.to_string());
        };

        if should_skip_duplicate_request(sess, &requested_path) {
            return Ok(());
        }

        sess.sftp_path = requested_path.clone();
        sess.sftp_loading = true;
        sess.sftp_error = None;
        sess.sftp_entries.clear();

        app_state.manager.send(ToCore::Sftp {
            id: session_id,
            req: SftpRequest::List { path },
        });
    }

    let state_for_timeout = state.clone();
    spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(UI_SFTP_TIMEOUT_SECS)).await;
        if let Ok(mut app_state) = state_for_timeout.lock() {
            if let Some(sess) = app_state.sessions.get_mut(&session_id) {
                if sess.sftp_loading && sess.sftp_path == requested_path {
                    sess.sftp_loading = false;
                    sess.sftp_error = Some(ui_timeout_message(&requested_path, language));
                }
            }
        }
    });

    Ok(())
}

pub(crate) fn parent_path(path: &str) -> String {
    if path == "/" || path == "." {
        return "/".to_string();
    }

    let trimmed = path.trim_end_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        "/".to_string()
    }
}

pub(crate) fn join_path(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{}", name)
    } else if base == "." {
        name.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

pub(crate) fn display_path(path: &str) -> String {
    if path == "." {
        "~".to_string()
    } else {
        path.to_string()
    }
}

pub(crate) fn normalize_sftp_path_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = if trimmed == "~" {
        ".".to_string()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            ".".to_string()
        } else {
            format!("./{rest}")
        }
    } else {
        trimmed.to_string()
    };

    Some(trim_sftp_trailing_slashes(&path))
}

fn trim_sftp_trailing_slashes(path: &str) -> String {
    if path.chars().all(|ch| ch == '/') {
        return "/".to_string();
    }

    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn ui_timeout_message(path: &str, language: AppLanguage) -> String {
    sftp_timeout_message(language, path, UI_SFTP_TIMEOUT_SECS)
}

fn should_skip_duplicate_request(sess: &crate::state::SessionState, path: &str) -> bool {
    sess.sftp_loading && sess.sftp_path == path
}

fn entries_same(left: &[SftpEntry], right: &[SftpEntry]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(a, b)| {
            a.name == b.name
                && a.is_dir == b.is_dir
                && a.size == b.size
                && a.modified == b.modified
                && a.permissions == b.permissions
                && a.user == b.user
                && a.group == b.group
                && a.uid == b.uid
                && a.gid == b.gid
        })
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_time(timestamp: u32) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let time = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_path_handles_root_and_nested_paths() {
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path("."), "/");
        assert_eq!(parent_path("/root"), "/");
        assert_eq!(parent_path("/root/.ssh"), "/root");
        assert_eq!(parent_path("/var/log/"), "/var");
    }

    #[test]
    fn join_path_keeps_posix_paths_stable() {
        assert_eq!(join_path("/", ".ssh"), "/.ssh");
        assert_eq!(join_path("/root", ".ssh"), "/root/.ssh");
        assert_eq!(join_path("/root/", ".ssh"), "/root/.ssh");
        assert_eq!(join_path(".", ".ssh"), ".ssh");
    }

    #[test]
    fn normalize_sftp_path_input_trims_and_maps_home_display() {
        assert_eq!(
            normalize_sftp_path_input("  /var/log/ "),
            Some("/var/log".to_string())
        );
        assert_eq!(normalize_sftp_path_input("////"), Some("/".to_string()));
        assert_eq!(normalize_sftp_path_input("~"), Some(".".to_string()));
        assert_eq!(
            normalize_sftp_path_input("~/logs/"),
            Some("./logs".to_string())
        );
        assert_eq!(normalize_sftp_path_input("   "), None);
    }

    #[test]
    fn format_size_uses_compact_units() {
        assert_eq!(format_size(18), "18 B");
        assert_eq!(format_size(2 * 1024 + 512), "2.5 KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn ui_timeout_message_includes_path_and_limit() {
        let message = ui_timeout_message("/root", AppLanguage::Chinese);
        assert!(message.contains("/root"));
        assert!(message.contains("15 秒"));
    }

    #[test]
    fn entries_same_compares_visible_file_fields() {
        let left = vec![SftpEntry {
            name: ".ssh".to_string(),
            is_dir: true,
            size: 0,
            modified: Some(100),
            permissions: Some(0o755),
            user: Some("root".to_string()),
            group: Some("root".to_string()),
            uid: Some(0),
            gid: Some(0),
        }];
        let mut right = left.clone();

        assert!(entries_same(&left, &right));

        right[0].size = 1;
        assert!(!entries_same(&left, &right));
    }

    #[test]
    fn duplicate_request_is_skipped_only_for_same_loading_path() {
        let mut sess = crate::state::SessionState {
            id: SessionId(1),
            title: "demo".to_string(),
            connect_params: kt_config::ConnectParams::new("example.com", "root"),
            pty: kt_core::PtySize {
                cols: 100,
                rows: 30,
            },
            snapshot: None,
            connected: true,
            connection_error: None,
            host_key_pending: false,
            auth_challenge: None,
            sftp_path: "/root".to_string(),
            sftp_entries: Vec::new(),
            sftp_loading: true,
            sftp_error: None,
            sftp_last_done: None,
            sftp_progress: None,
            terminal_cwd: None,
            monitor: None,
            monitor_loading: false,
            monitor_error: None,
        };

        assert!(should_skip_duplicate_request(&sess, "/root"));
        assert!(!should_skip_duplicate_request(&sess, "/tmp"));

        sess.sftp_loading = false;
        assert!(!should_skip_duplicate_request(&sess, "/root"));
    }
}
