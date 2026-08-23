//! 手机端文件视图：路径栏 + 工具栏 + 单列条目表。
//!
//! 与桌面 SFTP 表格的差异：单击（而非双击）进入目录；条目动作由行尾 `⋮` 打开动作面板
//! 替代右键；不渲染修改时间/权限/属主等在窄屏放不下的列。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};
use kt_core::{SessionId, SftpEntry};

use super::action_sheet::PhoneSheet;
use crate::components::app_logic::ActiveSftpView;
use crate::components::icons::Icon;
use crate::components::sftp::{
    display_path, normalize_sftp_path_input, parent_path, request_directory,
};
use crate::components::sidebar::{format_sftp_size, SftpEntryContext};
use crate::i18n::texts;
use crate::state::AppState;
use crate::store::Store;

pub(super) struct FilesViewArgs {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) store: Arc<Store>,
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) active_sftp: Option<ActiveSftpView>,
    pub(super) phone_sheet: Signal<Option<PhoneSheet>>,
    pub(super) on_entry_open: Callback<SftpEntryContext>,
    pub(super) on_mkdir: Callback<(SessionId, String)>,
}

pub(super) fn render_files_view(args: FilesViewArgs) -> Element {
    let FilesViewArgs {
        state,
        store,
        settings,
        language,
        active_sftp,
        phone_sheet,
        on_entry_open,
        on_mkdir,
    } = args;

    let t = texts(language).sftp;

    let Some(sftp) = active_sftp else {
        return rsx! {
            div {
                class: "phone-view phone-view-files",
                div { class: "phone-empty", p { "{texts(language).phone.files_need_session}" } }
            }
        };
    };

    rsx! {
        div {
            class: "phone-view phone-view-files",

            PhoneSftpPathBar {
                key: "phone-sftp-path-{sftp.session_id.0}",
                session_id: sftp.session_id,
                connected: sftp.connected,
                path: sftp.path.clone(),
                language,
            }

            div {
                class: "phone-view-toolbar",

                button {
                    class: "phone-toolbar-button",
                    "aria-label": "{t.back}",
                    disabled: !sftp.connected || is_root(&sftp.path),
                    onclick: {
                        let state = state.clone();
                        let path = sftp.path.clone();
                        move |_| {
                            if let Err(error) = request_directory(
                                state.clone(),
                                sftp.session_id,
                                parent_path(&path),
                                language,
                            ) {
                                tracing::error!("SFTP 返回上级失败: {error}");
                            }
                        }
                    },
                    Icon { name: "back" }
                }
                button {
                    class: "phone-toolbar-button",
                    "aria-label": "{t.refresh}",
                    disabled: !sftp.connected,
                    onclick: {
                        let state = state.clone();
                        let path = sftp.path.clone();
                        move |_| {
                            if let Err(error) = request_directory(
                                state.clone(),
                                sftp.session_id,
                                path.clone(),
                                language,
                            ) {
                                tracing::error!("SFTP 刷新失败: {error}");
                            }
                        }
                    },
                    Icon { name: "refresh" }
                }
                // 桌面端「新建文件夹」在 SFTP 空白处右键；手机上提升为工具栏按钮。
                button {
                    class: "phone-toolbar-button",
                    "aria-label": "{t.new_folder}",
                    disabled: !sftp.connected,
                    onclick: {
                        let path = sftp.path.clone();
                        move |_| on_mkdir.call((sftp.session_id, path.clone()))
                    },
                    Icon { name: "add" }
                }

                label {
                    class: if sftp.auto_sync { "phone-auto-sync is-active" } else { "phone-auto-sync" },
                    input {
                        r#type: "checkbox",
                        checked: sftp.auto_sync,
                        disabled: !sftp.connected,
                        onchange: {
                            let state = state.clone();
                            let store = Arc::clone(&store);
                            let mut settings = settings;
                            move |evt: Event<FormData>| {
                                let enabled = evt.checked();
                                let applied = state
                                    .lock()
                                    .map(|mut app_state| {
                                        app_state.set_sftp_auto_sync(sftp.session_id, enabled)
                                    })
                                    .unwrap_or_else(|_| Err(t.state_unavailable.to_string()));
                                match applied {
                                    Ok(()) => {
                                        // 记住这次选择，之后新建的会话直接沿用。
                                        let mut next = settings.peek().clone();
                                        if next.sftp_auto_sync != enabled {
                                            next.sftp_auto_sync = enabled;
                                            match store.update_settings(next.clone()) {
                                                Ok(()) => settings.set(next),
                                                Err(error) => {
                                                    tracing::error!("保存自动同步设置失败: {error}")
                                                }
                                            }
                                        }
                                    }
                                    Err(message) => set_sftp_error(&state, sftp.session_id, message),
                                }
                            }
                        },
                    }
                    span { "{t.auto_sync}" }
                }
            }

            div {
                class: "phone-scroll",

                if sftp.loading {
                    div { class: "phone-empty", p { "{t.loading}" } }
                } else if let Some(error) = sftp.error.clone() {
                    div { class: "phone-empty is-error", p { "{t.error_prefix}: {error}" } }
                } else if sftp.entries.is_empty() {
                    div { class: "phone-empty", p { "0 {t.items}" } }
                } else {
                    for entry in sftp.entries.clone() {
                        div {
                            key: "{entry.name}",
                            class: if entry.is_dir { "phone-row phone-file-row is-dir" } else { "phone-row phone-file-row" },

                            button {
                                class: "phone-row-main",
                                // 单击进入目录；文件在手机上没有可用的外部编辑链路，点击不做动作。
                                disabled: !entry.is_dir,
                                onclick: {
                                    let entry = entry.clone();
                                    let base_path = sftp.path.clone();
                                    move |_| on_entry_open.call(SftpEntryContext {
                                        session_id: sftp.session_id,
                                        base_path: base_path.clone(),
                                        entry: entry.clone(),
                                    })
                                },
                                Icon { name: entry_icon(&entry) }
                                span {
                                    class: "phone-row-copy",
                                    strong { "{entry.name}" }
                                    small { "{phone_entry_detail(&entry)}" }
                                }
                            }

                            button {
                                class: "phone-row-more",
                                "aria-label": "{texts(language).phone.more_actions}",
                                onclick: {
                                    let entry = entry.clone();
                                    let base_path = sftp.path.clone();
                                    let mut phone_sheet = phone_sheet;
                                    move |evt: Event<MouseData>| {
                                        evt.stop_propagation();
                                        phone_sheet.set(Some(PhoneSheet::SftpEntry(SftpEntryContext {
                                            session_id: sftp.session_id,
                                            base_path: base_path.clone(),
                                            entry: entry.clone(),
                                        })));
                                    }
                                },
                                "⋮"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 路径输入框需要在远端目录变化时跟随，这部分状态是局部的，放进独立组件承接 hook。
#[component]
fn PhoneSftpPathBar(
    session_id: SessionId,
    connected: bool,
    path: String,
    language: AppLanguage,
) -> Element {
    let state = crate::components::app::get_state().clone();
    let t = texts(language).sftp;
    let mut path_input = use_signal(|| display_path(&path));

    use_effect(use_reactive((&path,), move |(path,)| {
        let display = display_path(&path);
        if *path_input.peek() != display {
            path_input.set(display);
        }
    }));

    rsx! {
        div {
            class: "phone-path-bar",
            Icon { name: "folder" }
            input {
                class: "phone-path-input",
                r#type: "text",
                value: "{path_input()}",
                disabled: !connected,
                placeholder: "{t.path}",
                autocapitalize: "off",
                autocorrect: "off",
                autocomplete: "off",
                spellcheck: false,
                oninput: move |evt| path_input.set(evt.value()),
                onkeydown: {
                    let state = state.clone();
                    let path = path.clone();
                    move |evt: Event<KeyboardData>| {
                        if evt.key() != Key::Enter {
                            return;
                        }
                        evt.stop_propagation();
                        evt.prevent_default();
                        match normalize_sftp_path_input(&path_input()) {
                            Some(next) => {
                                path_input.set(display_path(&next));
                                if let Err(error) =
                                    request_directory(state.clone(), session_id, next, language)
                                {
                                    tracing::error!("SFTP 路径跳转失败: {error}");
                                }
                            }
                            None => path_input.set(display_path(&path)),
                        }
                    }
                },
            }
        }
    }
}

fn set_sftp_error(state: &Arc<Mutex<AppState>>, session_id: SessionId, message: String) {
    if let Ok(mut app_state) = state.lock() {
        if let Some(session) = app_state.sessions.get_mut(&session_id) {
            session.sftp_error = Some(message);
        }
    }
}

fn is_root(path: &str) -> bool {
    path == "/" || path == "."
}

fn entry_icon(entry: &SftpEntry) -> &'static str {
    if entry.is_dir {
        "folder"
    } else {
        "file"
    }
}

/// 窄屏放不下修改时间/权限/属主，副标题只保留大小（目录不显示）。
fn phone_entry_detail(entry: &SftpEntry) -> String {
    format_sftp_size(entry.size, entry.is_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64) -> SftpEntry {
        SftpEntry {
            name: name.to_string(),
            is_dir,
            size,
            modified: None,
            permissions: None,
            user: None,
            group: None,
            uid: None,
            gid: None,
        }
    }

    #[test]
    fn root_directories_disable_the_back_button() {
        assert!(is_root("/"));
        assert!(is_root("."));
        assert!(!is_root("/var"));
        assert!(!is_root("./logs"));
    }

    #[test]
    fn entry_subtitle_shows_size_for_files_and_nothing_for_directories() {
        assert_eq!(phone_entry_detail(&entry("app.log", false, 2048)), "2.0 KB");
        assert_eq!(phone_entry_detail(&entry("logs", true, 4096)), "");
    }
}
