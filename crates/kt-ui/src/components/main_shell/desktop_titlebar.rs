//! 桌面端应用内标题栏。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::SessionId;

use crate::components::app_logic::{
    session_dot_class_for_status, SessionConnectionStatus, SessionTabView,
};
use crate::components::icons::Icon;
use crate::i18n::texts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesktopTitlebarLayout {
    Macos,
    Standard,
}

fn titlebar_layout_for_platform(is_macos: bool) -> DesktopTitlebarLayout {
    if is_macos {
        DesktopTitlebarLayout::Macos
    } else {
        DesktopTitlebarLayout::Standard
    }
}

#[component]
pub(super) fn DesktopTitlebar(
    language: AppLanguage,
    session_tabs: Vec<SessionTabView>,
    active_session_id: Signal<Option<SessionId>>,
    sidebar_collapsed: bool,
    on_sidebar_toggle: Callback<()>,
    on_settings_open: Callback<()>,
    on_session_close: Callback<SessionId>,
    on_session_reconnect: Callback<SessionId>,
    on_new_connection: Callback<()>,
) -> Element {
    let desktop = dioxus::desktop::use_window();
    let t = texts(language).app;
    let layout = titlebar_layout_for_platform(cfg!(target_os = "macos"));
    let is_macos = layout == DesktopTitlebarLayout::Macos;
    let mut active_session_id = active_session_id;
    let active_tab_id = active_session_id();
    let sidebar_tooltip = if sidebar_collapsed {
        t.expand_sidebar
    } else {
        t.collapse_sidebar
    };
    let sidebar_icon = if sidebar_collapsed {
        "panel-left-open"
    } else {
        "panel-left-close"
    };

    rsx! {
        header { class: "desktop-titlebar",
            if is_macos {
                WindowControls { language, layout }
            }

            button {
                class: "desktop-topbar-icon tooltip-trigger",
                "data-tooltip": "{sidebar_tooltip}",
                aria_label: "{sidebar_tooltip}",
                aria_expanded: (!sidebar_collapsed).to_string(),
                aria_controls: "resource-sidebar",
                onclick: move |_| on_sidebar_toggle.call(()),
                Icon { name: sidebar_icon }
            }

            div {
                class: "desktop-session-tabs",
                onmousedown: move |event| event.stop_propagation(),
                for sess in session_tabs {
                    div {
                        key: "top-tab-{sess.id.0}",
                        class: if active_tab_id == Some(sess.id) { "desktop-session-tab is-active" } else { "desktop-session-tab" },
                        onclick: {
                            let id = sess.id;
                            move |_| active_session_id.set(Some(id))
                        },

                        span { class: session_dot_class_for_status(sess.status) }
                        span { class: "desktop-tab-title", "{sess.title}" }
                        if active_tab_id == Some(sess.id) {
                            div {
                                class: "desktop-tab-actions",
                                onclick: move |event| event.stop_propagation(),
                                if matches!(sess.status, SessionConnectionStatus::Disconnected) {
                                    button {
                                        class: "desktop-tab-action tooltip-trigger",
                                        "data-tooltip": "{t.reconnect}",
                                        aria_label: "{t.reconnect}",
                                        onclick: {
                                            let id = sess.id;
                                            move |_| on_session_reconnect.call(id)
                                        },
                                        Icon { name: "refresh" }
                                    }
                                }
                            }
                        }
                        button {
                            class: "desktop-tab-close tooltip-trigger",
                            "data-tooltip": "{t.close_session}",
                            aria_label: "{t.close_session}",
                            onclick: {
                                let id = sess.id;
                                move |event| {
                                    event.stop_propagation();
                                    on_session_close.call(id);
                                }
                            },
                            Icon { name: "close" }
                        }
                    }
                }
                button {
                    class: "desktop-new-session tooltip-trigger",
                    "data-tooltip": "{t.new_connection}",
                    aria_label: "{t.new_connection}",
                    onclick: move |_| on_new_connection.call(()),
                    Icon { name: "add" }
                }
            }

            div {
                class: "desktop-titlebar-drag",
                onmousedown: {
                    let desktop = desktop.clone();
                    move |event| {
                        event.stop_propagation();
                        desktop.drag();
                    }
                },
                ondoubleclick: {
                    let desktop = desktop.clone();
                    move |event| {
                        event.stop_propagation();
                        desktop.toggle_maximized();
                    }
                },
            }

            div { class: "desktop-titlebar-controls desktop-titlebar-settings",
                button {
                    class: "desktop-settings-button",
                    onclick: move |_| on_settings_open.call(()),
                    Icon { name: "sliders" }
                    span { "{t.settings}" }
                }
            }

            if !is_macos {
                WindowControls { language, layout }
            }
        }
    }
}

#[component]
fn WindowControls(language: AppLanguage, layout: DesktopTitlebarLayout) -> Element {
    let desktop = dioxus::desktop::use_window();
    let t = texts(language).app;
    let controls_class = if layout == DesktopTitlebarLayout::Macos {
        "desktop-titlebar-controls desktop-titlebar-window-controls is-macos"
    } else {
        "desktop-titlebar-controls desktop-titlebar-window-controls"
    };

    rsx! {
        div { class: "{controls_class}",
            if layout == DesktopTitlebarLayout::Macos {
                button {
                    class: "desktop-titlebar-control is-close tooltip-trigger",
                    "data-tooltip": "{t.close}",
                    aria_label: "{t.close}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.close()
                    },
                    Icon { name: "close" }
                }
                button {
                    class: "desktop-titlebar-control is-minimize tooltip-trigger",
                    "data-tooltip": "{t.minimize}",
                    aria_label: "{t.minimize}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.window.set_minimized(true)
                    },
                    Icon { name: "minimize" }
                }
                button {
                    class: "desktop-titlebar-control is-maximize tooltip-trigger",
                    "data-tooltip": "{t.maximize}",
                    aria_label: "{t.maximize}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.toggle_maximized()
                    },
                    Icon { name: "maximize" }
                }
            } else {
                button {
                    class: "desktop-titlebar-control is-minimize tooltip-trigger",
                    "data-tooltip": "{t.minimize}",
                    aria_label: "{t.minimize}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.window.set_minimized(true)
                    },
                    Icon { name: "minimize" }
                }
                button {
                    class: "desktop-titlebar-control is-maximize tooltip-trigger",
                    "data-tooltip": "{t.maximize}",
                    aria_label: "{t.maximize}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.toggle_maximized()
                    },
                    Icon { name: "maximize" }
                }
                button {
                    class: "desktop-titlebar-control is-close tooltip-trigger",
                    "data-tooltip": "{t.close}",
                    aria_label: "{t.close}",
                    onclick: {
                        let desktop = desktop.clone();
                        move |_| desktop.close()
                    },
                    Icon { name: "close" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_titlebar_layout_puts_window_controls_before_drag_area() {
        assert_eq!(
            titlebar_layout_for_platform(true),
            DesktopTitlebarLayout::Macos
        );
        assert_eq!(
            titlebar_layout_for_platform(false),
            DesktopTitlebarLayout::Standard
        );
    }
}
