//! 主应用组件 —— 整合会话列表、终端、SFTP、资源监控的主界面。
//!
//! 布局参考 FinalShell + WindTerm:
//! - 顶部:已打开会话的标签页
//! - 左侧:已保存连接列表(新建/编辑/删除)
//! - 中央:终端区域 + 底部可切换的 SFTP 抽屉
//! - 右侧:资源监控面板(可折叠)
//! - 底部:状态栏

use std::sync::{Arc, Mutex, OnceLock};

use dioxus::prelude::*;
use kt_config::{ConnectParams, SessionProfile};
use kt_core::ssh::HostKeyDecision;
use kt_core::{AuthProvider, HostKeyVerifier, PtySize, SessionId, SessionManager, ToCore};

use crate::components::dialog::ConnectionDialog;
use crate::components::monitor::MonitorPanel;
use crate::components::sftp::SftpPanel;
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::state::{AppState, SessionState};
use crate::store::Store;

/// 全局 Store(只初始化一次)。
static GLOBAL_STORE: OnceLock<Arc<Store>> = OnceLock::new();

/// 全局 AppState(只初始化一次)。
static GLOBAL_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

/// 获取全局 state(供其他模块使用)。
pub fn get_state() -> &'static Arc<Mutex<AppState>> {
    GLOBAL_STATE.get_or_init(|| {
        tracing::info!("初始化 SessionManager");

        let store = get_store();
        let manager = SessionManager::spawn(
            Arc::new(AcceptAllVerifier),
            Arc::new(StoreAuthFactory {
                store: store.clone(),
            }),
        )
        .expect("无法启动 SessionManager");

        Arc::new(Mutex::new(AppState::new(manager)))
    })
}

fn get_store() -> &'static Arc<Store> {
    GLOBAL_STORE.get_or_init(|| Arc::new(Store::load().expect("无法加载配置")))
}

/// AuthProvider 实现(从 Store 读取密码)。
struct StoreAuthProvider {
    store: Arc<Store>,
    vault_id: String,
}

impl AuthProvider for StoreAuthProvider {
    fn password(&mut self, _user: &str, _host: &str) -> Option<String> {
        self.store.get_secret(&self.vault_id)
    }
    fn key_passphrase(&mut self, _key_path: &str) -> Option<String> {
        None
    }
    fn keyboard_interactive(
        &mut self,
        _name: &str,
        _instructions: &str,
        _prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }
}

/// AuthProviderFactory 实现。
struct StoreAuthFactory {
    store: Arc<Store>,
}

impl kt_core::session::AuthProviderFactory for StoreAuthFactory {
    fn create(&self, _id: kt_core::SessionId, params: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(StoreAuthProvider {
            store: self.store.clone(),
            vault_id: params.effective_vault_id(),
        })
    }
}

/// 简单的 HostKeyVerifier(接受所有主机密钥, TOFU)。
struct AcceptAllVerifier;

impl HostKeyVerifier for AcceptAllVerifier {
    fn verify(
        &self,
        _host: &str,
        _port: u16,
        _key: &russh::keys::PublicKey,
        _fingerprint: &str,
    ) -> HostKeyDecision {
        HostKeyDecision::Accept
    }
}

/// 中央区域的视图模式:终端 or SFTP。
#[derive(Clone, Copy, PartialEq)]
enum CenterView {
    Terminal,
    Sftp,
}

#[component]
pub fn App() -> Element {
    let store = get_store();
    let state = get_state();

    // 对话框状态
    let mut show_dialog = use_signal(|| false);
    let mut dialog_mode = use_signal(|| "new".to_string());
    let mut edit_name = use_signal(String::new);
    let mut edit_host = use_signal(String::new);
    let mut edit_port = use_signal(|| String::from("22"));
    let mut edit_user = use_signal(String::new);
    let mut edit_password = use_signal(String::new);

    // 当前活动会话 ID
    let mut active_session_id = use_signal(|| None::<SessionId>);

    // 所有会话列表(定时刷新)
    let mut all_sessions = use_signal(Vec::<SessionState>::new);

    // 中央视图模式(终端/SFTP)
    let mut center_view = use_signal(|| CenterView::Terminal);

    // 右侧监控面板是否展开
    let mut show_monitor = use_signal(|| false);

    // 触发重建保存连接列表(删除/新建后)
    let mut saved_tick = use_signal(|| 0u64);

    // 定时泵送事件(每 16ms = ~60fps)
    use_future(move || async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
            if let Ok(mut app_state) = state.lock() {
                app_state.pump_events();
            }
        }
    });

    // 定期更新会话列表(100ms)
    use_effect(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                if let Ok(s) = state.lock() {
                    let sessions: Vec<SessionState> = s.sessions.values().cloned().collect();
                    all_sessions.set(sessions.clone());

                    // 如果没有活动会话,选择第一个
                    if active_session_id().is_none() && !sessions.is_empty() {
                        active_session_id.set(Some(sessions[0].id));
                    }
                }
            }
        });
    });

    // 获取当前活动会话
    let active_session = move || {
        if let Some(id) = active_session_id() {
            all_sessions().into_iter().find(|s| s.id == id)
        } else {
            None
        }
    };

    rsx! {
        // 全局样式(光标闪烁动画)
        style { {include_str!("../assets/app.css")} }

        div {
            style: "display: flex; flex-direction: column; height: 100vh; width: 100vw; background: #f0f1f4; font-family: -apple-system, 'Segoe UI', sans-serif;",

            // ===== 顶部标签栏 =====
            header {
                style: "height: 40px; background: #f0f1f4; border-bottom: 1px solid #d3d7de; display: flex; align-items: center; padding: 0 12px; gap: 4px;",

                for sess in all_sessions() {
                    div {
                        key: "tab-{sess.id.0}",
                        style: if active_session_id() == Some(sess.id) {
                            "padding: 6px 12px; background: #ffffff; color: #1f2937; border: 1px solid #d3d7de; border-bottom: none; border-radius: 6px 6px 0 0; cursor: pointer; font-size: 13px; font-weight: 600; display: flex; align-items: center; gap: 6px;"
                        } else {
                            "padding: 6px 12px; background: #e5e7eb; color: #6b7280; border: 1px solid transparent; border-radius: 6px 6px 0 0; cursor: pointer; font-size: 13px; display: flex; align-items: center; gap: 6px;"
                        },
                        onclick: {
                            let id = sess.id;
                            move |_| active_session_id.set(Some(id))
                        },
                        if sess.connected {
                            span { style: "color: #16a34a; font-size: 10px;", "●" }
                        } else {
                            span { style: "color: #dc4e5a; font-size: 10px;", "○" }
                        }
                        span { "{sess.title}" }
                        span {
                            style: "margin-left: 4px; cursor: pointer; opacity: 0.6; font-size: 14px;",
                            onclick: {
                                let id = sess.id;
                                move |evt| {
                                    evt.stop_propagation();
                                    if let Ok(mut app_state) = state.lock() {
                                        app_state.manager.send(ToCore::Disconnect { id });
                                        app_state.sessions.remove(&id);
                                        if active_session_id() == Some(id) {
                                            let next = app_state.sessions.keys().next().copied();
                                            active_session_id.set(next);
                                        }
                                    }
                                }
                            },
                            "✕"
                        }
                    }
                }

                if all_sessions().is_empty() {
                    span { style: "color: #9ca3af; font-size: 13px;", "从左侧选择连接 →" }
                }
            }

            // ===== 主内容区 =====
            main {
                style: "flex: 1; display: flex; overflow: hidden;",

                // ----- 左侧:已保存连接 -----
                aside {
                    style: "width: 240px; background: #f0f1f4; border-right: 1px solid #d3d7de; display: flex; flex-direction: column;",

                    div {
                        style: "display: flex; justify-content: space-between; align-items: center; padding: 12px;",
                        span { style: "font-size: 14px; font-weight: 600; color: #374151;", "已保存的连接" }
                        button {
                            style: "padding: 4px 10px; font-size: 12px; background: #10b981; color: white; border: none; border-radius: 4px; cursor: pointer;",
                            onclick: move |_| {
                                dialog_mode.set("new".to_string());
                                edit_name.set(String::new());
                                edit_host.set(String::new());
                                edit_port.set("22".to_string());
                                edit_user.set(String::new());
                                edit_password.set(String::new());
                                show_dialog.set(true);
                            },
                            "+ 新建"
                        }
                    }

                    div {
                        style: "flex: 1; overflow-y: auto; padding: 0 8px 8px; display: flex; flex-direction: column; gap: 6px;",

                        {
                            let _ = saved_tick(); // 订阅刷新信号
                            rsx! {
                                for profile in store.saved_sessions() {
                                    ConnectionCard {
                                        key: "saved-{profile.name}",
                                        profile: profile.clone(),
                                        on_connect: {
                                            let profile = profile.clone();
                                            move |_| {
                                                if let Ok(mut app_state) = state.lock() {
                                                    let id = app_state.next_session_id();
                                                    app_state.sessions.insert(id, SessionState {
                                                        id,
                                                        title: profile.name.clone(),
                                                        snapshot: None,
                                                        connected: false,
                                                        sftp_path: "/".to_string(),
                                                        sftp_entries: Vec::new(),
                                                        sftp_loading: false,
                                                        sftp_error: None,
                                                        monitor: None,
                                                    });
                                                    app_state.manager.send(ToCore::Connect {
                                                        id,
                                                        params: Box::new(profile.params.clone()),
                                                        pty: PtySize { cols: 80, rows: 24 },
                                                    });
                                                    active_session_id.set(Some(id));
                                                }
                                            }
                                        },
                                        on_edit: {
                                            let profile = profile.clone();
                                            move |_| {
                                                dialog_mode.set("edit".to_string());
                                                edit_name.set(profile.name.clone());
                                                edit_host.set(profile.params.host.clone());
                                                edit_port.set(profile.params.port.to_string());
                                                edit_user.set(profile.params.user.clone());
                                                edit_password.set(String::new());
                                                show_dialog.set(true);
                                            }
                                        },
                                        on_delete: {
                                            let name = profile.name.clone();
                                            move |_| {
                                                if let Err(e) = store.delete_session(&name) {
                                                    tracing::error!("删除失败: {}", e);
                                                } else {
                                                    saved_tick.set(saved_tick() + 1);
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                }

                // ----- 中央:终端 / SFTP -----
                section {
                    style: "flex: 1; display: flex; flex-direction: column; overflow: hidden; background: #1a1b26;",

                    if let Some(sess) = active_session() {
                        // 视图切换栏
                        div {
                            style: "height: 32px; background: #16161e; display: flex; align-items: center; padding: 0 8px; gap: 4px; border-bottom: 1px solid #2a2b36;",
                            button {
                                style: if center_view() == CenterView::Terminal {
                                    "padding: 4px 12px; background: #2b7de9; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                } else {
                                    "padding: 4px 12px; background: transparent; color: #9ca3af; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                },
                                onclick: move |_| center_view.set(CenterView::Terminal),
                                "终端"
                            }
                            button {
                                style: if center_view() == CenterView::Sftp {
                                    "padding: 4px 12px; background: #2b7de9; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                } else {
                                    "padding: 4px 12px; background: transparent; color: #9ca3af; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                },
                                onclick: move |_| center_view.set(CenterView::Sftp),
                                "文件 (SFTP)"
                            }
                            div { style: "flex: 1;" }
                            // 监控面板开关
                            button {
                                style: if show_monitor() {
                                    "padding: 4px 12px; background: #6366f1; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                } else {
                                    "padding: 4px 12px; background: transparent; color: #9ca3af; border: none; border-radius: 4px; cursor: pointer; font-size: 12px;"
                                },
                                onclick: move |_| show_monitor.set(!show_monitor()),
                                "📊 监控"
                            }
                        }

                        // 内容区
                        div {
                            style: "flex: 1; overflow: hidden;",
                            match center_view() {
                                CenterView::Terminal => rsx! {
                                    if let Some(snapshot) = sess.snapshot.clone() {
                                        Terminal {
                                            snapshot: SnapshotWrapper(snapshot),
                                            session_id: sess.id,
                                        }
                                    } else {
                                        div {
                                            style: "color: #6b7280; padding: 16px; font-size: 14px;",
                                            "等待终端数据..."
                                        }
                                    }
                                },
                                CenterView::Sftp => rsx! {
                                    SftpPanel { session_id: sess.id }
                                },
                            }
                        }
                    } else {
                        div {
                            style: "flex: 1; display: flex; align-items: center; justify-content: center; color: #6b7280; font-size: 14px;",
                            "点击左侧连接以开始"
                        }
                    }
                }

                // ----- 右侧:资源监控(可折叠) -----
                if show_monitor() {
                    if let Some(sess) = active_session() {
                        aside {
                            style: "width: 320px; border-left: 1px solid #d3d7de; overflow: hidden;",
                            MonitorPanel { session_id: sess.id }
                        }
                    }
                }
            }

            // ===== 底部状态栏 =====
            footer {
                style: "height: 26px; background: #f0f1f4; border-top: 1px solid #d3d7de; display: flex; align-items: center; padding: 0 12px; font-size: 12px; color: #6b7280; gap: 16px;",
                if let Some(sess) = active_session() {
                    span {
                        if sess.connected { "● 已连接" } else { "○ 连接中..." }
                    }
                    span { "{sess.title}" }
                } else {
                    span { "就绪" }
                }
            }
        }

        // 连接编辑对话框
        ConnectionDialog {
            show: show_dialog,
            mode: dialog_mode,
            name: edit_name,
            host: edit_host,
            port: edit_port,
            user: edit_user,
            password: edit_password,
            on_save: move |profile: SessionProfile| {
                if let Err(e) = store.save_session(profile.clone()) {
                    tracing::error!("保存连接失败: {}", e);
                } else {
                    let pwd = edit_password();
                    if !pwd.is_empty() {
                        let vault_id = profile.params.effective_vault_id();
                        if let Err(e) = store.set_secret(&vault_id, &pwd) {
                            tracing::error!("保存密码失败: {}", e);
                        }
                    }
                    saved_tick.set(saved_tick() + 1);
                }
            },
        }
    }
}

/// 左侧的单个连接卡片。
#[component]
fn ConnectionCard(
    profile: SessionProfile,
    on_connect: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    let subtitle = format!(
        "{}@{}:{}",
        profile.params.user, profile.params.host, profile.params.port
    );

    rsx! {
        div {
            style: "background: #ffffff; border: 1px solid #d3d7de; border-radius: 6px; padding: 10px;",

            div {
                style: "cursor: pointer;",
                onclick: move |_| on_connect.call(()),
                div {
                    style: "font-size: 14px; font-weight: 600; color: #1f2937; margin-bottom: 2px;",
                    "{profile.name}"
                }
                div {
                    style: "font-size: 12px; color: #6b7280;",
                    "{subtitle}"
                }
            }

            div {
                style: "margin-top: 8px; display: flex; gap: 6px;",
                button {
                    style: "flex: 1; padding: 4px 8px; font-size: 11px; background: #e5e7eb; color: #374151; border: none; border-radius: 3px; cursor: pointer;",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_edit.call(());
                    },
                    "✏️ 编辑"
                }
                button {
                    style: "flex: 1; padding: 4px 8px; font-size: 11px; background: #fee2e2; color: #dc2626; border: none; border-radius: 3px; cursor: pointer;",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_delete.call(());
                    },
                    "🗑️ 删除"
                }
            }
        }
    }
}
