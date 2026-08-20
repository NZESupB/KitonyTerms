//! 手机端终端视图：全屏终端 + 软键盘输入桥 + 键位条。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};

use super::keyboard::{focus_phone_keyboard, PhoneKeyboard};
use crate::components::app_logic::ActiveTerminalView;
use crate::components::main_shell::SplitMode;
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::components::workbench::{EmptyWorkbench, TerminalPlaceholder};
use crate::state::AppState;

pub(super) struct TerminalViewArgs {
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) active_terminal: Option<ActiveTerminalView>,
    /// 仅为满足 `Terminal` 的签名；手机端从不设置分屏。
    pub(super) split_mode: Signal<Option<SplitMode>>,
}

pub(super) fn render_terminal_view(args: TerminalViewArgs) -> Element {
    let TerminalViewArgs {
        settings,
        language,
        active_terminal,
        split_mode,
    } = args;

    let Some(session) = active_terminal else {
        return rsx! {
            div { class: "phone-view phone-view-terminal", EmptyWorkbench { language } }
        };
    };

    let session_id = session.id;

    rsx! {
        div {
            class: "phone-view phone-view-terminal",

            div {
                class: "phone-terminal-body",
                // 点击终端任意处唤起软键盘：手机上没有别的方式把焦点交给输入框。
                onclick: move |_| focus_phone_keyboard(session_id),

                if let Some(snapshot) = session.snapshot.clone() {
                    Terminal {
                        snapshot: SnapshotWrapper(snapshot),
                        session_id,
                        pane_id: "primary".to_string(),
                        trigger_highlights: settings().trigger_highlights,
                        show_line_numbers: settings().show_line_numbers,
                        show_timestamps: settings().show_timestamps,
                        language,
                        split_mode,
                        // 手机屏幕放不下两个终端，不提供分屏入口。
                        allow_split: false,
                    }
                } else {
                    TerminalPlaceholder {
                        status: session.status,
                        title: session.title.clone(),
                        error: session.connection_error.clone(),
                        language,
                    }
                }
            }

            PhoneKeyboard {
                key: "phone-keyboard-{session_id.0}",
                session_id,
                language,
            }
        }
    }
}

/// 终端右键菜单在手机上由长按触发，但复制/粘贴/全选也要能从顶栏动作面板进入。
pub(super) fn terminal_pane_id(session_id: kt_core::SessionId) -> String {
    format!("terminal-{}-primary", session_id.0)
}

/// 把当前 SFTP 浏览目录同步到终端。失败原因写回会话的 SFTP 错误位，与桌面端一致。
pub(super) fn sync_terminal_to_sftp_path(
    state: &Arc<Mutex<AppState>>,
    session_id: kt_core::SessionId,
    language: AppLanguage,
) {
    let Ok(mut app_state) = state.lock() else {
        return;
    };
    let Some(path) = app_state
        .sessions
        .get(&session_id)
        .map(|session| session.sftp_path.clone())
    else {
        return;
    };
    if let Err(error) = app_state.send_terminal_cd(session_id, &path) {
        let t = crate::i18n::texts(language).sftp;
        let message = match error {
            crate::state::TerminalCdBlocked::Unavailable => t.session_missing,
            crate::state::TerminalCdBlocked::AltScreen => t.sync_blocked_alt_screen,
            crate::state::TerminalCdBlocked::SendFailed => t.sync_send_failed,
        };
        if let Some(session) = app_state.sessions.get_mut(&session_id) {
            session.sftp_error = Some(message.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_core::SessionId;

    #[test]
    fn terminal_pane_id_matches_the_id_the_terminal_component_renders() {
        // 顶栏动作面板的复制/全选靠这个 id 找到 DOM 节点。
        assert_eq!(terminal_pane_id(SessionId(5)), "terminal-5-primary");
    }
}
