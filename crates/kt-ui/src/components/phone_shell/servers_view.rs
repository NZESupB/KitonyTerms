//! 手机端服务器视图：分组连接列表，点按即连，行尾 `⋮` 打开动作面板。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings, SessionProfile};
use kt_core::{PtySize, SessionId};

use super::action_sheet::PhoneSheet;
use super::tab::{tab_after_connect, PhoneTab};
use crate::components::app_logic::{
    group_profiles, params_with_ssh_config, session_state_from_profile,
};
use crate::components::icons::Icon;
use crate::i18n::texts;
use crate::state::AppState;

pub(super) struct ServersViewArgs {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) saved_profiles: Vec<SessionProfile>,
    pub(super) saved_groups: Vec<String>,
    pub(super) active_profile_title: Option<String>,
    pub(super) active_session_id: Signal<Option<SessionId>>,
    pub(super) phone_tab: Signal<PhoneTab>,
    pub(super) phone_sheet: Signal<Option<PhoneSheet>>,
    pub(super) on_new_connection: Callback<()>,
    pub(super) on_new_group: Callback<()>,
}

pub(super) fn render_servers_view(args: ServersViewArgs) -> Element {
    let ServersViewArgs {
        state,
        settings,
        language,
        saved_profiles,
        saved_groups,
        active_profile_title,
        mut active_session_id,
        mut phone_tab,
        mut phone_sheet,
        on_new_connection,
        on_new_group,
    } = args;

    let t = texts(language).app;
    let grouped = group_profiles(&saved_profiles, &saved_groups);

    rsx! {
        div {
            class: "phone-view phone-view-servers",

            div {
                class: "phone-view-toolbar",
                span { class: "phone-view-toolbar-title", "{t.groups}" }
                button {
                    class: "phone-toolbar-button",
                    "aria-label": "{t.new_group}",
                    onclick: move |_| on_new_group.call(()),
                    Icon { name: "folder" }
                }
                button {
                    class: "phone-toolbar-button is-primary",
                    "aria-label": "{t.new_connection}",
                    onclick: move |_| on_new_connection.call(()),
                    Icon { name: "add" }
                }
            }

            div {
                class: "phone-scroll",

                if saved_profiles.is_empty() {
                    div {
                        class: "phone-empty",
                        strong { "{t.no_saved_connections}" }
                        p { "{t.saved_connections_hint}" }
                    }
                }

                for (group_name, profiles) in grouped {
                    div {
                        key: "group-{group_name}",
                        class: "phone-group",

                        div {
                            class: "phone-group-head",
                            span { class: "phone-group-name", "{group_name}" }
                            button {
                                class: "phone-row-more",
                                "aria-label": "{texts(language).phone.more_actions}",
                                onclick: {
                                    let group_name = group_name.clone();
                                    move |evt: Event<MouseData>| {
                                        evt.stop_propagation();
                                        phone_sheet.set(Some(PhoneSheet::Group(group_name.clone())));
                                    }
                                },
                                "⋮"
                            }
                        }

                        for profile in profiles {
                            div {
                                key: "profile-{profile.name}",
                                class: if active_profile_title.as_deref() == Some(profile.name.as_str()) {
                                    "phone-row phone-server-row is-active"
                                } else {
                                    "phone-row phone-server-row"
                                },

                                button {
                                    class: "phone-row-main",
                                    onclick: {
                                        let profile = profile.clone();
                                        let state = state.clone();
                                        move |_| {
                                            let current = settings.peek().clone();
                                            if let Some(id) = connect_profile(&state, &profile, &current) {
                                                active_session_id.set(Some(id));
                                                phone_tab.set(tab_after_connect());
                                            }
                                        }
                                    },
                                    span { class: "status-dot online" }
                                    span {
                                        class: "phone-row-copy",
                                        strong { "{profile.name}" }
                                        small { "{profile.params.user}@{profile.params.host}:{profile.params.port}" }
                                    }
                                }

                                button {
                                    class: "phone-row-more",
                                    "aria-label": "{texts(language).phone.more_actions}",
                                    onclick: {
                                        let profile = profile.clone();
                                        move |evt: Event<MouseData>| {
                                            evt.stop_propagation();
                                            phone_sheet.set(Some(PhoneSheet::Profile(profile.clone())));
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
}

/// 建立会话并返回新的 session id。与桌面侧栏的连接路径保持一致：合并 `~/.ssh/config`、
/// 沿用持久化的自动同步开关。
pub(super) fn connect_profile(
    state: &Arc<Mutex<AppState>>,
    profile: &SessionProfile,
    settings: &AppSettings,
) -> Option<SessionId> {
    let params = params_with_ssh_config(profile.params.clone(), settings.use_ssh_config);
    let Ok(mut app_state) = state.lock() else {
        return None;
    };

    let id = app_state.next_session_id();
    let mut session = session_state_from_profile(id, profile, settings.sftp_auto_sync);
    session.connect_params = params;
    session.pty = PtySize {
        cols: 100,
        rows: 30,
    };
    app_state.sessions.insert(id, session);
    if let Err(error) = app_state.connect_session(id) {
        tracing::error!("连接请求失败: {error}");
    }
    Some(id)
}
