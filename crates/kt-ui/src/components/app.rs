//! 主应用组件 —— 深色工作台。
//!
//! 这里保留全局状态、弹窗和跨模块副作用编排；主工作台布局由 `main_shell` 承接。

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, OnceLock},
};

use dioxus::prelude::*;
use kt_config::SessionProfile;
use kt_core::{AuthChallenge, AuthResponse, SessionId, SessionManager, SftpRequest, ToCore};

use crate::components::app_logic::{
    active_monitor_view, active_session, active_sftp_view, active_terminal_view,
    auth_challenge_view, clamp_dimension, duplicate_profile, session_tab_views,
    status_bar_session_view, DEFAULT_GROUP_NAME,
};
use crate::components::app_runtime::{KnownHostsVerifier, StoreAuthFactory};
use crate::components::desktop_menu::is_settings_menu_id;
use crate::components::dialog::{
    first_public_key_path, ConnectionDialog, GroupDialog, SftpNameDialog,
};
use crate::components::external_edit::{
    external_edit_local_path, external_edit_status_text, latest_sftp_completion_revision,
    local_file_modified, open_local_file, ExternalEdit, ExternalEditAction, ExternalEditSaveDialog,
    ExternalEditStatus, ExternalEditSyncMode,
};
use crate::components::main_shell::{
    render_main_shell, window_class, MainShellArgs, ResizeDrag, SplitMode, SFTP_MAX_HEIGHT,
    SFTP_MIN_HEIGHT, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
};
use crate::components::security_dialogs::{AuthChallengeDialog, HostKeyConfirmDialog};
use crate::components::sftp::{join_path, parent_path, request_directory};
use crate::components::sidebar::{ContextMenu, ContextMenuState, SftpEntryContext};
use crate::components::state_controller::use_state_controller;
use crate::components::workbench::SettingsPanel;
use crate::i18n::texts;
use crate::state::{AppState, SessionState};
use crate::store::PendingHostKey;
use crate::store::Store;

/// 全局 Store（只初始化一次）。
static GLOBAL_STORE: OnceLock<Arc<Store>> = OnceLock::new();

/// 全局 AppState（只初始化一次）。
static GLOBAL_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAuthSecret {
    session_id: SessionId,
    vault_id: String,
    password: String,
}

#[derive(Clone, Copy)]
struct SecretSaveSignals {
    status_notice: Signal<Option<String>>,
}

/// 获取全局 state，供其他模块共享会话运行时。
pub fn get_state() -> &'static Arc<Mutex<AppState>> {
    GLOBAL_STATE.get_or_init(|| {
        tracing::info!("初始化 SessionManager");

        let store = get_store();
        let manager = SessionManager::spawn(
            Arc::new(KnownHostsVerifier::new(Arc::clone(store))),
            Arc::new(StoreAuthFactory::new(Arc::clone(store))),
        )
        .expect("无法启动 SessionManager");

        Arc::new(Mutex::new(AppState::new(manager)))
    })
}

fn get_store() -> &'static Arc<Store> {
    GLOBAL_STORE.get_or_init(|| Arc::new(Store::load().expect("无法加载配置")))
}

#[component]
pub fn App() -> Element {
    let store = get_store();
    let state = get_state();

    let mut show_dialog = use_signal(|| false);
    let mut dialog_mode = use_signal(|| "new".to_string());
    let mut edit_original_name = use_signal(String::new);
    let mut edit_name = use_signal(String::new);
    let mut edit_host = use_signal(String::new);
    let mut edit_port = use_signal(|| String::from("22"));
    let mut edit_user = use_signal(String::new);
    let mut edit_group = use_signal(String::new);
    let mut edit_password = use_signal(String::new);
    let mut edit_key_path = use_signal(String::new);
    let mut edit_proxy_jump = use_signal(String::new);
    let mut edit_use_agent = use_signal(|| false);
    let mut edit_forward_agent = use_signal(|| false);

    let mut settings = use_signal(|| store.settings());
    let show_settings = use_signal(|| false);
    let mut show_group_dialog = use_signal(|| false);
    let mut group_dialog_mode = use_signal(|| "new".to_string());
    let mut group_dialog_name = use_signal(String::new);
    let mut group_dialog_original = use_signal(String::new);
    let mut show_sftp_name_dialog = use_signal(|| false);
    let mut sftp_name_dialog_mode = use_signal(String::new);
    let mut sftp_name_dialog_session = use_signal(|| None::<SessionId>);
    let mut sftp_name_dialog_base_path = use_signal(String::new);
    let mut sftp_name_dialog_target_path = use_signal(String::new);
    let mut sftp_name_dialog_is_dir = use_signal(|| false);
    let mut sftp_name_dialog_value = use_signal(String::new);
    let mut external_edits = use_signal(Vec::<ExternalEdit>::new);
    let mut external_edit_notice = use_signal(|| None::<String>);
    let next_external_edit_id = use_signal(|| 1u64);
    let mut host_key_prompt = use_signal(|| None::<PendingHostKey>);
    let mut host_key_error = use_signal(|| None::<String>);
    let mut pending_auth_secrets = use_signal(Vec::<PendingAuthSecret>::new);
    let mut status_notice = use_signal(|| store.vault_status_message());
    let active_session_id = use_signal(|| None::<SessionId>);
    let all_sessions = use_signal(Vec::<SessionState>::new);
    let mut saved_tick = use_signal(|| 0u64);
    let mut sidebar_width = use_signal(|| SIDEBAR_DEFAULT_WIDTH);
    let mut sftp_height = use_signal(|| None::<f64>);
    let mut active_resize = use_signal(|| None::<ResizeDrag>);
    let mut context_menu = use_signal(|| None::<ContextMenuState>);
    let collapsed_server_groups = use_signal(BTreeSet::<String>::new);
    let split_mode = use_signal(|| None::<SplitMode>);
    let secret_save_signals = SecretSaveSignals { status_notice };

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    dioxus::desktop::use_muda_event_handler({
        let mut show_settings = show_settings;
        move |event| {
            if is_settings_menu_id(event.id().as_ref()) {
                show_settings.set(true);
            }
        }
    });

    use_state_controller(
        state,
        Arc::clone(store),
        all_sessions,
        active_session_id,
        host_key_prompt,
        external_edits,
        Callback::new(move |action: ExternalEditAction| match action {
            ExternalEditAction::OpenLocal {
                edit_id,
                path,
                file_name,
            } => {
                if let Err(e) = open_local_file(&path) {
                    tracing::error!("打开外部编辑器失败: {}", e);
                    external_edit_notice.set(Some(format!(
                        "{} {}: {}",
                        texts(settings.peek().language).sftp.edit_status_open_failed,
                        file_name,
                        e
                    )));
                    let mut edits = external_edits.peek().clone();
                    edits.retain(|edit| edit.id != edit_id);
                    external_edits.set(edits);
                    let _ = std::fs::remove_file(path);
                } else {
                    external_edit_notice.set(Some(format!(
                        "{} {}",
                        texts(settings.peek().language).sftp.edit_status_opened,
                        file_name
                    )));
                }
            }
            ExternalEditAction::Upload {
                session_id,
                local_path,
                remote_path,
                file_name,
            } => {
                external_edit_notice.set(Some(format!(
                    "{} {}",
                    texts(settings.peek().language).sftp.edit_status_uploading,
                    file_name
                )));
                send_sftp_request(
                    Arc::clone(state),
                    session_id,
                    SftpRequest::Upload {
                        local: local_path,
                        remote: remote_path,
                    },
                );
            }
            ExternalEditAction::DeleteLocal(path) => {
                let _ = std::fs::remove_file(path);
            }
            ExternalEditAction::UploadCompleted { file_name } => {
                external_edit_notice.set(Some(format!(
                    "{} {}",
                    texts(settings.peek().language).sftp.edit_status_uploaded,
                    file_name
                )));
            }
        }),
    );

    use_effect({
        let store = Arc::clone(store);
        move || {
            let sessions = all_sessions();
            let pending = pending_auth_secrets();
            if pending.is_empty() {
                return;
            }

            let mut remaining = Vec::new();
            for secret in pending {
                let session = sessions
                    .iter()
                    .find(|session| session.id == secret.session_id);
                match session {
                    Some(session) if session.connected => {
                        save_pending_secret(
                            &store,
                            secret.vault_id,
                            secret.password,
                            secret_save_signals,
                            settings.peek().language,
                        );
                    }
                    Some(session) if session.connection_error.is_some() => {}
                    Some(_) => remaining.push(secret),
                    None => {}
                }
            }

            if *pending_auth_secrets.peek() != remaining {
                pending_auth_secrets.set(remaining);
            }
        }
    });

    let saved_profiles = {
        let _ = saved_tick();
        store.saved_sessions()
    };
    let saved_groups = {
        let _ = saved_tick();
        store.saved_groups()
    };

    let current_settings = settings();
    let language = current_settings.language;
    let theme_name = current_settings.normalized_theme();
    let window_class_name = window_class(active_resize(), theme_name);
    let sessions_snapshot = all_sessions();
    let active_session_ref = active_session(&sessions_snapshot, active_session_id());
    let active_status_session = active_session_ref.cloned();
    let active_auth_challenge = auth_challenge_view(&sessions_snapshot);
    let session_tabs = session_tab_views(&sessions_snapshot);
    let active_terminal = active_terminal_view(active_session_ref);
    let active_sftp = active_sftp_view(active_session_ref);
    let active_monitor = active_monitor_view(active_session_ref);
    let status_session = status_bar_session_view(active_session_ref);
    let external_edit_status = external_edit_status_text(
        &external_edits(),
        active_status_session.as_ref(),
        external_edit_notice().as_deref(),
        language,
    );
    let status_detail = match external_edit_status {
        Some(status) => Some(status),
        None => status_notice(),
    };

    rsx! {
        style { {include_str!("../assets/app.css")} }

        div {
            class: "{window_class_name}",
            "data-theme": "{theme_name}",
            onmousemove: move |evt| {
                match active_resize() {
                    Some(ResizeDrag::SidebarWidth { start_x, start_width }) => {
                        let delta = evt.client_coordinates().x - start_x;
                        sidebar_width.set(clamp_dimension(
                            start_width + delta,
                            SIDEBAR_MIN_WIDTH,
                            SIDEBAR_MAX_WIDTH,
                        ));
                    }
                    Some(ResizeDrag::SftpHeight { start_y, start_height }) => {
                        let delta = start_y - evt.client_coordinates().y;
                        sftp_height.set(Some(clamp_dimension(
                            start_height + delta,
                            SFTP_MIN_HEIGHT,
                            SFTP_MAX_HEIGHT,
                        )));
                    }
                    None => {}
                }
            },
            onmouseup: move |_| active_resize.set(None),
            onmouseleave: move |_| active_resize.set(None),
            onclick: move |_| context_menu.set(None),
            oncontextmenu: move |evt| {
                evt.prevent_default();
                context_menu.set(None);
            },

            {render_main_shell(MainShellArgs {
                state: Arc::clone(state),
                store: Arc::clone(store),
                settings,
                language,
                saved_profiles: saved_profiles.clone(),
                saved_groups: saved_groups.clone(),
                active_terminal,
                active_sftp,
                active_monitor,
                status_session,
                session_tabs,
                status_detail: status_detail.clone(),
                show_dialog,
                dialog_mode,
                edit_original_name,
                edit_name,
                edit_host,
                edit_port,
                edit_user,
                edit_group,
                edit_password,
                edit_key_path,
                edit_proxy_jump,
                edit_use_agent,
                edit_forward_agent,
                show_group_dialog,
                group_dialog_mode,
                group_dialog_name,
                group_dialog_original,
                active_session_id,
                saved_tick,
                sidebar_width,
                sftp_height,
                active_resize,
                context_menu,
                collapsed_server_groups,
                split_mode,
                on_sftp_entry_open: {
                    let state = Arc::clone(state);
                    Callback::new(move |ctx: SftpEntryContext| {
                        if let Err(e) = open_sftp_entry(state.clone(), ctx, language) {
                            tracing::error!("SFTP 打开目录失败: {}", e);
                        }
                    })
                },
                on_sftp_entry_external_edit: {
                    let state = Arc::clone(state);
                    Callback::new(move |ctx: SftpEntryContext| {
                        start_sftp_external_edit(
                            state.clone(),
                            ctx,
                            external_edits,
                            next_external_edit_id,
                        );
                    })
                },
            })}

            if let Some(menu) = context_menu() {
                ContextMenu {
                    menu,
                    language,
                    on_profile_edit: {
                        let saved_profiles = saved_profiles.clone();
                        move |name: String| {
                            if let Some(profile) = saved_profiles.iter().find(|profile| profile.name == name) {
                                dialog_mode.set("edit".to_string());
                                edit_original_name.set(profile.name.clone());
                                edit_name.set(profile.name.clone());
                                edit_host.set(profile.params.host.clone());
                                edit_port.set(profile.params.port.to_string());
                                edit_user.set(profile.params.user.clone());
                                edit_group.set(profile.group.clone().unwrap_or_default());
                                edit_password.set(String::new());
                                edit_key_path.set(first_public_key_path(&profile.params.auth));
                                edit_proxy_jump.set(profile.params.proxy_jump.clone().unwrap_or_default());
                                edit_use_agent.set(profile.params.auth.contains(&kt_config::AuthMethod::Agent));
                                edit_forward_agent.set(profile.params.forward_agent);
                                show_dialog.set(true);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_profile_delete: move |name: String| {
                        if let Err(e) = store.delete_session(&name) {
                            tracing::error!("删除失败: {}", e);
                        } else {
                            saved_tick.set(saved_tick() + 1);
                        }
                        context_menu.set(None);
                    },
                    on_profile_copy: {
                        let saved_profiles = saved_profiles.clone();
                        move |name: String| {
                            if let Some(profile) = saved_profiles.iter().find(|profile| profile.name == name) {
                                let duplicate = duplicate_profile(profile, &saved_profiles);
                                if let Err(e) = store.save_session(duplicate) {
                                    tracing::error!("复制连接失败: {}", e);
                                } else {
                                    saved_tick.set(saved_tick() + 1);
                                }
                            }
                            context_menu.set(None);
                        }
                    },
                    on_group_new: move |_| {
                        group_dialog_mode.set("new".to_string());
                        group_dialog_original.set(String::new());
                        group_dialog_name.set(String::new());
                        show_group_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_group_rename: move |name: String| {
                        group_dialog_mode.set("rename".to_string());
                        group_dialog_original.set(name.clone());
                        group_dialog_name.set(if name == DEFAULT_GROUP_NAME {
                            String::new()
                        } else {
                            name
                        });
                        show_group_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_group_delete: move |name: String| {
                        if let Err(e) = store.delete_group(&name) {
                            tracing::error!("删除分组失败: {}", e);
                        } else {
                            saved_tick.set(saved_tick() + 1);
                        }
                        context_menu.set(None);
                    },
                    on_sftp_open: {
                        let state = Arc::clone(state);
                        move |ctx: SftpEntryContext| {
                            if let Err(e) = open_sftp_entry(state.clone(), ctx, language) {
                                tracing::error!("SFTP 打开目录失败: {}", e);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_refresh: {
                        let state = Arc::clone(state);
                        move |(session_id, path): (SessionId, String)| {
                            if let Err(e) = request_directory(state.clone(), session_id, path, language) {
                                tracing::error!("SFTP 刷新失败: {}", e);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_mkdir: move |(session_id, path): (SessionId, String)| {
                        sftp_name_dialog_mode.set("mkdir".to_string());
                        sftp_name_dialog_session.set(Some(session_id));
                        sftp_name_dialog_base_path.set(path);
                        sftp_name_dialog_target_path.set(String::new());
                        sftp_name_dialog_is_dir.set(true);
                        sftp_name_dialog_value.set(String::new());
                        show_sftp_name_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_sftp_rename: move |ctx: SftpEntryContext| {
                        sftp_name_dialog_mode.set("rename".to_string());
                        sftp_name_dialog_session.set(Some(ctx.session_id));
                        sftp_name_dialog_base_path.set(ctx.base_path.clone());
                        sftp_name_dialog_target_path.set(join_path(&ctx.base_path, &ctx.entry.name));
                        sftp_name_dialog_is_dir.set(ctx.entry.is_dir);
                        sftp_name_dialog_value.set(ctx.entry.name.clone());
                        show_sftp_name_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_sftp_delete: {
                        let state = Arc::clone(state);
                        move |ctx: SftpEntryContext| {
                            let path = join_path(&ctx.base_path, &ctx.entry.name);
                            send_sftp_request(
                                state.clone(),
                                ctx.session_id,
                                SftpRequest::Remove {
                                    path,
                                    is_dir: ctx.entry.is_dir,
                                },
                            );
                            context_menu.set(None);
                        }
                    },
                    on_sftp_external_edit: {
                        let state = Arc::clone(state);
                        move |ctx: SftpEntryContext| {
                            start_sftp_external_edit(
                                state.clone(),
                                ctx,
                                external_edits,
                                next_external_edit_id,
                            );
                            context_menu.set(None);
                        }
                    },
                    on_copy_text: move |value: String| {
                        copy_to_clipboard(&value);
                        context_menu.set(None);
                    },
                }
            }

            if let Some(edit) = external_edits()
                .into_iter()
                .find(|edit| edit.status == ExternalEditStatus::PromptPending)
            {
                ExternalEditSaveDialog {
                    edit,
                    language,
                    on_upload_once: {
                        let state = Arc::clone(state);
                        move |edit_id: u64| {
                            let mut edits = external_edits.peek().clone();
                            if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id) {
                                edit.status = ExternalEditStatus::UploadingOnce;
                                edit.after_revision = latest_sftp_completion_revision(state.clone(), edit.session_id);
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                );
                            }
                            external_edits.set(edits);
                        }
                    },
                    on_auto_upload: {
                        let state = Arc::clone(state);
                        move |edit_id: u64| {
                            let mut edits = external_edits.peek().clone();
                            if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id) {
                                edit.status = ExternalEditStatus::UploadingAuto;
                                edit.sync_mode = ExternalEditSyncMode::AutoUpload;
                                edit.after_revision = latest_sftp_completion_revision(state.clone(), edit.session_id);
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                );
                            }
                            external_edits.set(edits);
                        }
                    },
                    on_ignore: move |edit_id: u64| {
                        let mut edits = external_edits.peek().clone();
                        if let Some(edit) = edits.iter().find(|edit| edit.id == edit_id) {
                            external_edit_notice.set(Some(format!(
                                "{} {}",
                                texts(language).sftp.edit_status_ignored,
                                edit.file_name
                            )));
                            let _ = std::fs::remove_file(&edit.local_path);
                        }
                        edits.retain(|edit| edit.id != edit_id);
                        external_edits.set(edits);
                    },
                }
            }

            if let Some((session_id, session_title, challenge)) = active_auth_challenge.clone() {
                AuthChallengeDialog {
                    session_title,
                    challenge: challenge.clone(),
                    language,
                    on_submit: {
                        let state = Arc::clone(state);
                        let challenge = challenge.clone();
                        move |answers: Vec<String>| {
                            if let Some(secret) =
                                pending_auth_secret(session_id, &challenge, &answers)
                            {
                                let mut next = pending_auth_secrets.peek().clone();
                                next.retain(|pending| {
                                    pending.session_id != secret.session_id
                                        || pending.vault_id != secret.vault_id
                                });
                                next.push(secret);
                                pending_auth_secrets.set(next);
                            }

                            if let Ok(mut app_state) = state.lock() {
                                if !app_state.manager.send(ToCore::AuthResponse {
                                    id: session_id,
                                    response: AuthResponse::Answers(answers),
                                }) {
                                    tracing::warn!("认证响应投递失败: {:?}", session_id);
                                }
                                if let Some(sess) = app_state.sessions.get_mut(&session_id) {
                                    sess.auth_challenge = None;
                                }
                            }
                        }
                    },
                    on_cancel: {
                        let state = Arc::clone(state);
                        move |_| {
                            if let Ok(mut app_state) = state.lock() {
                                if !app_state.manager.send(ToCore::AuthResponse {
                                    id: session_id,
                                    response: AuthResponse::Cancel,
                                }) {
                                    tracing::warn!("认证取消响应投递失败: {:?}", session_id);
                                }
                                if let Some(sess) = app_state.sessions.get_mut(&session_id) {
                                    sess.auth_challenge = None;
                                }
                            }
                        }
                    },
                }
            }

            if let Some(prompt) = host_key_prompt() {
                HostKeyConfirmDialog {
                    prompt,
                    language,
                    error: host_key_error(),
                    on_trust: {
                        let store = Arc::clone(store);
                        move |prompt: PendingHostKey| {
                            match store.trust_host_key(&prompt.host, prompt.port, &prompt.fingerprint) {
                                Ok(_) => {
                                    store.clear_pending_host_key();
                                    host_key_prompt.set(None);
                                    host_key_error.set(None);
                                    status_notice.set(Some(
                                        texts(language)
                                            .dialog
                                            .host_key_trusted_hint
                                            .to_string(),
                                    ));
                                }
                                Err(err) => {
                                    let message = format!(
                                        "{}: {}",
                                        texts(language).dialog.host_key_save_failed,
                                        err
                                    );
                                    tracing::error!("{}", message);
                                    host_key_error.set(Some(message));
                                }
                            }
                        }
                    },
                    on_allow_once: {
                        let store = Arc::clone(store);
                        move |prompt: PendingHostKey| {
                            store.allow_host_key_once(prompt);
                            store.clear_pending_host_key();
                            host_key_prompt.set(None);
                            host_key_error.set(None);
                            status_notice.set(Some(
                                texts(language)
                                    .dialog
                                    .host_key_allowed_once_hint
                                    .to_string(),
                            ));
                        }
                    },
                    on_cancel: {
                        let store = Arc::clone(store);
                        move |_| {
                            store.clear_pending_host_key();
                            host_key_prompt.set(None);
                            host_key_error.set(None);
                        }
                    },
                }
            }

            ConnectionDialog {
                show: show_dialog,
                mode: dialog_mode,
                name: edit_name,
                host: edit_host,
                port: edit_port,
                user: edit_user,
                group: edit_group,
                password: edit_password,
                key_path: edit_key_path,
                proxy_jump: edit_proxy_jump,
                use_agent: edit_use_agent,
                forward_agent: edit_forward_agent,
                groups: saved_groups.clone(),
                language,
                on_save: {
                    let store = Arc::clone(store);
                    move |profile: SessionProfile| {
                        let original_name = edit_original_name();
                        let is_rename = dialog_mode() == "edit"
                            && !original_name.is_empty()
                            && original_name != profile.name;

                        if let Err(e) = store.save_session(profile.clone()) {
                            tracing::error!("保存连接失败: {}", e);
                        } else {
                            if is_rename {
                                if let Err(e) = store.delete_session(&original_name) {
                                    tracing::error!("删除旧连接失败: {}", e);
                                }
                            }
                            let pwd = edit_password();
                            if !pwd.is_empty() {
                                let vault_id = profile.params.effective_vault_id();
                                save_pending_secret(
                                    &store,
                                    vault_id,
                                    pwd,
                                    secret_save_signals,
                                    language,
                                );
                            }
                            saved_tick.set(saved_tick() + 1);
                        }
                    }
                },
            }

            GroupDialog {
                show: show_group_dialog,
                mode: group_dialog_mode,
                name: group_dialog_name,
                language,
                on_save: {
                    let store = Arc::clone(store);
                    move |name: String| {
                        let original = group_dialog_original();
                        let result = if group_dialog_mode() == "rename" {
                            if original == DEFAULT_GROUP_NAME {
                                store.rename_default_group(&name)
                            } else {
                                store.rename_group(&original, &name)
                            }
                        } else {
                            store.add_group(&name)
                        };
                        match result {
                            Ok(()) => saved_tick.set(saved_tick() + 1),
                            Err(e) => tracing::error!("保存分组失败: {}", e),
                        }
                    }
                },
            }

            SftpNameDialog {
                show: show_sftp_name_dialog,
                mode: sftp_name_dialog_mode,
                value: sftp_name_dialog_value,
                language,
                on_save: {
                    let state = Arc::clone(state);
                    move |name: String| {
                        let Some(session_id) = sftp_name_dialog_session() else {
                            return;
                        };
                        let name = name.trim().to_string();
                        if name.is_empty() {
                            return;
                        }

                        match sftp_name_dialog_mode().as_str() {
                            "mkdir" => {
                                let path = join_path(&sftp_name_dialog_base_path(), &name);
                                send_sftp_request(state.clone(), session_id, SftpRequest::Mkdir { path });
                            }
                            "rename" => {
                                let from = sftp_name_dialog_target_path();
                                if from.is_empty() {
                                    return;
                                }
                                let to = join_path(&parent_path(&from), &name);
                                if from != to {
                                    send_sftp_request(state.clone(), session_id, SftpRequest::Rename { from, to });
                                }
                            }
                            _ => {}
                        }
                    }
                },
            }

            SettingsPanel {
                show: show_settings,
                language,
                theme: settings().theme.clone(),
                on_language_change: {
                    let store = Arc::clone(store);
                    move |language| {
                        let mut next = settings();
                        next.language = language;
                        match store.update_settings(next.clone()) {
                            Ok(()) => settings.set(next),
                            Err(e) => tracing::error!("保存设置失败: {}", e),
                        }
                    }
                },
                on_theme_change: {
                    let store = Arc::clone(store);
                    move |theme| {
                        let mut next = settings();
                        next.theme = theme;
                        match store.update_settings(next.clone()) {
                            Ok(()) => settings.set(next),
                            Err(e) => tracing::error!("保存主题失败: {}", e),
                        }
                    }
                },
            }
        }
    }
}

pub(crate) fn open_sftp_entry(
    state: Arc<Mutex<AppState>>,
    ctx: SftpEntryContext,
    language: kt_config::AppLanguage,
) -> Result<(), String> {
    if !ctx.entry.is_dir {
        return Ok(());
    }

    request_directory(
        state,
        ctx.session_id,
        join_path(&ctx.base_path, &ctx.entry.name),
        language,
    )
}

fn start_sftp_external_edit(
    state: Arc<Mutex<AppState>>,
    ctx: SftpEntryContext,
    mut external_edits: Signal<Vec<ExternalEdit>>,
    mut next_external_edit_id: Signal<u64>,
) {
    if ctx.entry.is_dir {
        return;
    }

    let remote_path = join_path(&ctx.base_path, &ctx.entry.name);
    let local_path = external_edit_local_path(ctx.session_id, &remote_path);
    if let Some(parent) = local_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("创建本地编辑目录失败: {}", e);
            return;
        }
    }

    let after_revision = latest_sftp_completion_revision(state.clone(), ctx.session_id);
    let edit = ExternalEdit {
        id: next_external_edit_id(),
        session_id: ctx.session_id,
        remote_path: remote_path.clone(),
        local_path: local_path.clone(),
        file_name: ctx.entry.name.clone(),
        after_revision,
        status: ExternalEditStatus::Downloading,
        sync_mode: ExternalEditSyncMode::Ask,
        last_seen_modified: None,
        pending_modified: None,
    };
    next_external_edit_id.set(next_external_edit_id() + 1);
    let mut edits = external_edits.peek().clone();
    edits.push(edit);
    external_edits.set(edits);
    send_sftp_request(
        state,
        ctx.session_id,
        SftpRequest::Download {
            remote: remote_path,
            local: local_path,
        },
    );
}

fn send_sftp_request(state: Arc<Mutex<AppState>>, session_id: SessionId, req: SftpRequest) {
    if let Ok(app_state) = state.lock() {
        app_state.manager.send(ToCore::Sftp {
            id: session_id,
            req,
        });
    }
}

fn auth_challenge_vault_id(challenge: &AuthChallenge) -> Option<String> {
    match challenge {
        AuthChallenge::Password { user, host, port } => Some(format!("{user}@{host}:{port}")),
        AuthChallenge::KeyPassphrase { key_path } => Some(format!("key:{key_path}")),
        AuthChallenge::KeyboardInteractive { .. } => None,
    }
}

fn auth_challenge_secret(challenge: &AuthChallenge, answers: &[String]) -> Option<String> {
    match challenge {
        AuthChallenge::Password { .. } | AuthChallenge::KeyPassphrase { .. } => answers
            .first()
            .map(|answer| answer.trim())
            .filter(|answer| !answer.is_empty())
            .map(str::to_string),
        AuthChallenge::KeyboardInteractive { .. } => None,
    }
}

fn pending_auth_secret(
    session_id: SessionId,
    challenge: &AuthChallenge,
    answers: &[String],
) -> Option<PendingAuthSecret> {
    Some(PendingAuthSecret {
        session_id,
        vault_id: auth_challenge_vault_id(challenge)?,
        password: auth_challenge_secret(challenge, answers)?,
    })
}

fn save_pending_secret(
    store: &Arc<Store>,
    vault_id: String,
    password: String,
    mut signals: SecretSaveSignals,
    language: kt_config::AppLanguage,
) {
    if vault_id.is_empty() || password.is_empty() {
        return;
    }

    match store.set_secret(&vault_id, &password) {
        Ok(()) => {
            signals.status_notice.set(Some(
                texts(language).dialog.vault_password_saved.to_string(),
            ));
        }
        Err(e) => {
            let message = format!("{}: {}", texts(language).dialog.vault_save_failed, e);
            tracing::error!("{}", message);
            signals.status_notice.set(Some(message));
        }
    }
}

fn copy_to_clipboard(value: &str) {
    let js_value = format!("{value:?}");
    let script = format!(
        r#"
        (() => {{
            const value = {js_value};
            if (navigator.clipboard && navigator.clipboard.writeText) {{
                navigator.clipboard.writeText(value);
                return;
            }}
            const el = document.createElement("textarea");
            el.value = value;
            el.style.position = "fixed";
            el.style.opacity = "0";
            document.body.appendChild(el);
            el.select();
            document.execCommand("copy");
            document.body.removeChild(el);
        }})();
        "#
    );
    dioxus::document::eval(&script);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_challenge_creates_pending_secret_for_session() {
        let challenge = AuthChallenge::Password {
            user: "root".to_string(),
            host: "example.com".to_string(),
            port: 2222,
        };
        let pending = pending_auth_secret(SessionId(7), &challenge, &[" secret ".to_string()])
            .expect("密码认证应生成待保存项");

        assert_eq!(pending.session_id, SessionId(7));
        assert_eq!(pending.vault_id, "root@example.com:2222");
        assert_eq!(pending.password, "secret");
    }

    #[test]
    fn keyboard_interactive_challenge_is_not_saved_as_password() {
        let challenge = AuthChallenge::KeyboardInteractive {
            name: "otp".to_string(),
            instructions: String::new(),
            prompts: vec![kt_core::AuthPrompt {
                text: "code".to_string(),
                echo: false,
            }],
        };

        assert!(pending_auth_secret(SessionId(1), &challenge, &["123456".to_string()]).is_none());
    }
}
