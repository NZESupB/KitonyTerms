//! 主工作台底部状态栏渲染。

use dioxus::prelude::*;
use kt_config::AppLanguage;

use crate::components::app_logic::{
    session_status_label, session_status_pill_class, StatusBarSessionView,
};
use crate::i18n::texts;

pub(super) struct StatusBarArgs {
    pub(super) language: AppLanguage,
    pub(super) status_session: Option<StatusBarSessionView>,
    pub(super) status_detail: Option<String>,
}

pub(super) fn render_status_bar(args: StatusBarArgs) -> Element {
    let StatusBarArgs {
        language,
        status_session,
        status_detail,
    } = args;
    let t = texts(language).app;

    rsx! {
        footer {
            class: "status-bar",
            if let Some(sess) = status_session {
                {
                    let status_class = session_status_pill_class(sess.status);
                    let status_label = session_status_label(sess.status, &t);
                    rsx! {
                        span {
                            class: "{status_class}",
                            "{status_label}"
                        }
                    }
                }
                span { "{sess.title}" }
                if let Some(status) = status_detail.clone() {
                    span { class: "status-detail", "{status}" }
                }
            } else {
                span { class: "status-pill pending", "{t.ready}" }
                span { "{t.ready_hint}" }
                if let Some(status) = status_detail {
                    span { class: "status-detail", "{status}" }
                }
            }
        }
    }
}
