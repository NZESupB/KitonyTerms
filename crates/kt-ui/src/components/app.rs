//! 主应用组件 —— 深色工作台。
//!
//! 这里保留全局状态、弹窗和跨模块副作用编排；主工作台布局由 `main_shell` 承接。

use std::{
    cell::Cell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use dioxus::prelude::*;
use kt_config::SessionProfile;
use kt_core::{AuthChallenge, AuthResponse, SessionId, SessionManager, SftpRequest, ToCore};
use kt_sync::{
    encode_share_payload, import_share, normalize_pairing_code, parse_share_payload, start_share,
    PutPrecondition, ShareHandle, SyncEnvelope, WebDavClient, WebDavEndpoint, DEFAULT_SHARE_TTL,
};

use crate::components::app_logic::{
    active_monitor_view, active_session, active_sftp_view, active_terminal_view,
    auth_challenge_view, clamp_dimension, duplicate_profile, session_tab_views, DEFAULT_GROUP_NAME,
};
use crate::components::app_runtime::{KnownHostsVerifier, StoreAuthFactory};
use crate::components::dialog::{
    first_public_key_path, saved_connection_refs, ConnectionDialog, GroupDialog, SftpNameDialog,
};
use crate::components::external_edit::{
    ensure_private_edit_dir, external_edit_local_path, external_edit_status_text,
    local_file_modified, open_local_file_with, ExternalEdit, ExternalEditAction,
    ExternalEditSaveDialog, ExternalEditStatus, ExternalEditSyncMode,
};
use crate::components::inline_editor::{
    inline_edit_load_error_text, inline_edit_size_rejection, read_editable_text,
    write_editable_text, InlineEdit, InlineEditAction, InlineEditStatus, InlineEditorDialog,
    InlineEditorStatus,
};
use crate::components::main_shell::{
    render_main_shell, window_class, ResizeDrag, ShellArgs, SplitMode, SFTP_MAX_HEIGHT,
    SFTP_MIN_HEIGHT, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
};
use crate::components::phone_shell::{render_phone_shell, PhoneExtras, PhoneSheet, PhoneTab};
use crate::components::scanner::{scan_supported, PairingScanner, ScanOutcome};
use crate::components::security_dialogs::{AuthChallengeDialog, HostKeyConfirmDialog};
use crate::components::settings::{ActiveShare, SettingsPanel, SyncAction};
use crate::components::sftp::{join_path, parent_path, request_directory};
use crate::components::sidebar::{ContextMenu, ContextMenuState, SftpEntryContext};
use crate::components::state_controller::{use_state_controller, EditSignals, StoreSignals};
use crate::device::use_device_class;
use crate::i18n::texts;
use crate::state::{AppState, SessionState};
use crate::store::PendingHostKey;
use crate::store::Store;

/// 全局 Store（只初始化一次）。
static GLOBAL_STORE: OnceLock<Arc<Store>> = OnceLock::new();

/// 全局 AppState（只初始化一次）。
static GLOBAL_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

#[derive(Clone, PartialEq, Eq)]
struct PendingAuthSecret {
    session_id: SessionId,
    vault_id: String,
    password: String,
}

impl std::fmt::Debug for PendingAuthSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAuthSecret")
            .field("session_id", &self.session_id)
            .field("vault_id", &self.vault_id)
            .field("password", &"<redacted>")
            .finish()
    }
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
    let mut edit_proxy_type = use_signal(|| String::from("direct"));
    let mut edit_proxy_host = use_signal(String::new);
    let mut edit_proxy_port = use_signal(String::new);
    let mut edit_proxy_username = use_signal(String::new);
    let mut edit_use_agent = use_signal(|| false);
    let mut edit_forward_agent = use_signal(|| false);

    let mut settings = use_signal(|| store.settings());
    let show_settings = use_signal(|| false);
    let mut sync_busy = use_signal(|| false);
    let mut sync_status = use_signal(|| None::<String>);
    let mut sync_share_handle = use_signal(|| None::<ShareHandle>);
    // 正在进行的分享：展示二维码与配对码，并允许主动停止。
    let mut active_share = use_signal(|| None::<ActiveShare>);
    let mut show_scanner = use_signal(|| false);
    // 扫码结果：由设置面板取用后填入地址与配对码输入框。
    let mut share_scan_result = use_signal(|| None::<(String, String)>);
    use_future(move || async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            let share_finished = sync_share_handle
                .peek()
                .as_ref()
                .is_some_and(ShareHandle::is_finished);
            if share_finished {
                sync_share_handle.take();
                active_share.set(None);
                let status_was_share_active = sync_status.peek().as_deref().is_some_and(|status| {
                    status == texts(kt_config::AppLanguage::Chinese).app.sync_share_active
                        || status == texts(kt_config::AppLanguage::English).app.sync_share_active
                });
                if status_was_share_active {
                    sync_status.set(None);
                }
            }
        }
    });
    // 仅保留最近一次成功下载/上传得到的远端 ETag，并绑定完整 URL，避免把
    // 一个资源的版本条件误用于另一个 WebDAV 地址。
    let mut sync_remote_revision = use_signal(|| None::<(String, String)>);
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
    // 内嵌编辑器同一时刻只有一个：它是全屏模态，多开既没有入口也没有意义。
    let mut inline_edit = use_signal(|| None::<InlineEdit>);
    let mut host_key_prompt = use_signal(|| None::<PendingHostKey>);
    let mut host_key_error = use_signal(|| None::<String>);
    let mut pending_auth_secrets = use_signal(Vec::<PendingAuthSecret>::new);
    let status_notice = use_signal(|| store.vault_status_message());
    let active_session_id = use_signal(|| None::<SessionId>);
    let all_sessions = use_signal(Vec::<SessionState>::new);
    let mut saved_tick = use_signal(|| 0u64);
    let mut sidebar_width = use_signal(|| SIDEBAR_DEFAULT_WIDTH);
    let mut sftp_height = use_signal(|| None::<f64>);
    let mut active_resize = use_signal(|| None::<ResizeDrag>);
    let mut context_menu = use_signal(|| None::<ContextMenuState>);
    let collapsed_server_groups = use_signal(BTreeSet::<String>::new);
    let sidebar_collapsed = use_signal(|| false);
    let split_mode = use_signal(|| None::<SplitMode>);
    // 手机 Shell 的局部状态。两个 Shell 在同一层条件渲染，hook 必须无条件创建，
    // 否则设备类型切换（旋转、折叠屏展开）会打乱 hook 顺序。桌面端只是两个空信号。
    let phone_tab = use_signal(|| PhoneTab::Servers);
    let mut phone_sheet = use_signal(|| None::<PhoneSheet>);
    let device_class = use_device_class();
    use_effect(move || {
        // 上下文菜单和动作面板分别属于桌面/手机 Shell，设备切换时不能把旧状态带过去。
        let is_phone = device_class().is_phone();
        context_menu.set(None);
        if !is_phone {
            phone_sheet.set(None);
        }
    });
    let secret_save_signals = SecretSaveSignals { status_notice };

    use_state_controller(
        state,
        Arc::clone(store),
        all_sessions,
        active_session_id,
        StoreSignals {
            host_key_prompt,
            status_notice,
        },
        EditSignals {
            settings,
            external_edits,
            on_external_edit_action: Callback::new(
                move |action: ExternalEditAction| match action {
                    ExternalEditAction::OpenLocal {
                        edit_id,
                        path,
                        file_name,
                        editor_command,
                    } => {
                        if let Err(e) = open_local_file_with(&path, editor_command.as_deref()) {
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
                        edit_id,
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
                        match send_sftp_request(
                            Arc::clone(state),
                            session_id,
                            SftpRequest::Upload {
                                local: local_path,
                                remote: remote_path,
                            },
                        ) {
                            Ok(request_id) => {
                                let mut edits = external_edits.peek().clone();
                                if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id)
                                {
                                    edit.request_id = Some(request_id);
                                }
                                external_edits.set(edits);
                            }
                            Err(message) => {
                                let mut edits = external_edits.peek().clone();
                                if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id)
                                {
                                    edit.status = ExternalEditStatus::PromptPending;
                                    edit.request_id = None;
                                }
                                external_edits.set(edits);
                                external_edit_notice.set(Some(format!(
                                    "{} {}: {}",
                                    texts(settings.peek().language).sftp.edit_status_failed,
                                    file_name,
                                    message
                                )));
                            }
                        }
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
                    ExternalEditAction::SyncFailed { file_name, message } => {
                        external_edit_notice.set(Some(format!(
                            "{} {}: {}",
                            texts(settings.peek().language).sftp.edit_status_failed,
                            file_name,
                            message
                        )));
                    }
                },
            ),
            inline_edit,
            on_inline_edit_action: Callback::new(move |action: InlineEditAction| {
                let language = settings.peek().language;
                match action {
                    InlineEditAction::Load { local_path } => {
                        let mut edit = inline_edit.peek().clone();
                        let Some(current) = edit.as_mut() else {
                            return;
                        };
                        match read_editable_text(&local_path) {
                            Ok(content) => {
                                current.original = content;
                                current.status = InlineEditStatus::Ready;
                            }
                            Err(error) => {
                                // 过大或二进制内容不进编辑框，临时文件立即清掉。
                                let _ = std::fs::remove_file(&local_path);
                                current.status = InlineEditStatus::LoadFailed(
                                    inline_edit_load_error_text(&error, language),
                                );
                            }
                        }
                        inline_edit.set(edit);
                    }
                    InlineEditAction::Saved { file_name } => {
                        external_edit_notice.set(Some(format!(
                            "{} {}",
                            texts(language).sftp.editor_saved,
                            file_name
                        )));
                    }
                    InlineEditAction::DeleteLocal(path) => {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }),
        },
    );

    // 外部编辑反馈属于短时通知；用代号避免旧的定时器清掉后续新通知。
    let external_edit_notice_generation = use_hook(|| Rc::new(Cell::new(0u64)));
    use_effect({
        let generation = external_edit_notice_generation.clone();
        move || {
            let Some(notice) = external_edit_notice() else {
                return;
            };
            let next_generation = generation.get().wrapping_add(1);
            generation.set(next_generation);
            let mut notice_signal = external_edit_notice;
            let generation = generation.clone();
            spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                if generation.get() == next_generation
                    && notice_signal.peek().as_deref() == Some(notice.as_str())
                {
                    notice_signal.set(None);
                }
            });
        }
    });

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
    let is_phone = device_class().is_phone();
    let window_class_name = window_class(active_resize(), theme_name, is_phone);
    let sessions_snapshot = all_sessions();
    let active_session_ref = active_session(&sessions_snapshot, active_session_id());
    let active_status_session = active_session_ref.cloned();
    let active_auth_challenge = auth_challenge_view(&sessions_snapshot);
    let session_tabs = session_tab_views(&sessions_snapshot);
    let active_terminal = active_terminal_view(active_session_ref);
    let active_sftp = active_sftp_view(active_session_ref);
    let active_monitor = active_monitor_view(active_session_ref);
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

    // SFTP 新建目录 / 重命名共用同一个命名对话框：桌面端由右键菜单打开，手机端由
    // 动作面板打开，因此提成具名回调而不是各写一份。
    let on_sftp_mkdir = Callback::new(move |(session_id, path): (SessionId, String)| {
        sftp_name_dialog_mode.set("mkdir".to_string());
        sftp_name_dialog_session.set(Some(session_id));
        sftp_name_dialog_base_path.set(path);
        sftp_name_dialog_target_path.set(String::new());
        sftp_name_dialog_is_dir.set(true);
        sftp_name_dialog_value.set(String::new());
        show_sftp_name_dialog.set(true);
    });
    let on_sftp_rename = Callback::new(move |ctx: SftpEntryContext| {
        sftp_name_dialog_mode.set("rename".to_string());
        sftp_name_dialog_session.set(Some(ctx.session_id));
        sftp_name_dialog_base_path.set(ctx.base_path.clone());
        sftp_name_dialog_target_path.set(join_path(&ctx.base_path, &ctx.entry.name));
        sftp_name_dialog_is_dir.set(ctx.entry.is_dir);
        sftp_name_dialog_value.set(ctx.entry.name.clone());
        show_sftp_name_dialog.set(true);
    });

    // 内嵌编辑器：手机端唯一的编辑路径，桌面端与外部编辑并存。
    let on_sftp_inline_edit = {
        let state = Arc::clone(state);
        Callback::new(move |ctx: SftpEntryContext| {
            let file_name = ctx.entry.name.clone();
            if let Err(message) = start_inline_edit(state.clone(), ctx, inline_edit, language) {
                external_edit_notice.set(Some(format!("{file_name}: {message}")));
            }
        })
    };
    let on_inline_edit_save = {
        let state = Arc::clone(state);
        Callback::new(move |content: String| {
            let t = texts(language).sftp;
            let mut next = inline_edit.peek().clone();
            let Some(current) = next.as_mut() else {
                return;
            };

            if let Err(message) = write_editable_text(&current.local_path, &content) {
                current.status =
                    InlineEditStatus::SaveFailed(format!("{}: {message}", t.editor_write_failed));
                inline_edit.set(next);
                return;
            }

            match send_sftp_request(
                state.clone(),
                current.session_id,
                SftpRequest::Upload {
                    local: current.local_path.clone(),
                    remote: current.remote_path.clone(),
                },
            ) {
                Ok(request_id) => {
                    current.request_id = Some(request_id);
                    current.status = InlineEditStatus::Saving;
                    // 不更新 original：保存失败时「有未保存改动」的判定必须仍然成立。
                }
                Err(message) => {
                    current.request_id = None;
                    current.status = InlineEditStatus::SaveFailed(message);
                }
            }
            inline_edit.set(next);
        })
    };
    let on_inline_edit_close = Callback::new(move |_| {
        if let Some(edit) = inline_edit.peek().as_ref() {
            let _ = std::fs::remove_file(&edit.local_path);
        }
        inline_edit.set(None);
    });

    let shell_args = ShellArgs {
        state: Arc::clone(state),
        store: Arc::clone(store),
        settings,
        language,
        saved_profiles: saved_profiles.clone(),
        saved_groups: saved_groups.clone(),
        active_terminal,
        active_sftp,
        active_monitor,
        session_tabs,
        status_detail: status_detail.clone(),
        on_status_dismiss: {
            let mut external_edit_notice = external_edit_notice;
            let mut status_notice = status_notice;
            Callback::new(move |_| {
                external_edit_notice.set(None);
                status_notice.set(None);
            })
        },
        on_settings_open: {
            let mut show_settings = show_settings;
            Callback::new(move |_| show_settings.set(true))
        },
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
        edit_proxy_type,
        edit_proxy_host,
        edit_proxy_port,
        edit_proxy_username,
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
        sidebar_collapsed,
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
                let file_name = ctx.entry.name.clone();
                let editor = settings.peek().default_editor.clone();
                if let Err(message) = start_sftp_external_edit(
                    state.clone(),
                    ctx,
                    editor,
                    external_edits,
                    next_external_edit_id,
                ) {
                    tracing::error!("外部编辑下载启动失败: {}", message);
                    external_edit_notice.set(Some(format!(
                        "{} {}: {}",
                        texts(language).sftp.edit_status_failed,
                        file_name,
                        message
                    )));
                }
            })
        },
    };

    // 手机与桌面/平板的主界面在这里分派。两个 render 函数都必须保持无 hook，
    // 否则旋转设备切换分支时会打乱 hook 顺序。
    let shell = if is_phone {
        render_phone_shell(
            shell_args,
            PhoneExtras {
                phone_tab,
                phone_sheet,
                on_sftp_mkdir,
                on_sftp_rename,
                on_sftp_inline_edit,
            },
        )
    } else {
        render_main_shell(shell_args)
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

            {shell}

            if !is_phone {
                if let Some(menu) = context_menu() {
                    ContextMenu {
                    menu,
                    language,
                    editors: current_settings.editors.clone(),
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
                                edit_proxy_type.set(crate::components::dialog::proxy_mode(&profile.params).to_string());
                                let (proxy_host_val, proxy_port_val, proxy_user_val) = crate::components::dialog::proxy_fields(&profile.params.proxy);
                                edit_proxy_host.set(proxy_host_val);
                                edit_proxy_port.set(proxy_port_val);
                                edit_proxy_username.set(proxy_user_val);
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
                    on_sftp_mkdir: move |args: (SessionId, String)| {
                        on_sftp_mkdir.call(args);
                        context_menu.set(None);
                    },
                    on_sftp_rename: move |ctx: SftpEntryContext| {
                        on_sftp_rename.call(ctx);
                        context_menu.set(None);
                    },
                    on_sftp_delete: {
                        let state = Arc::clone(state);
                        move |ctx: SftpEntryContext| {
                            let path = join_path(&ctx.base_path, &ctx.entry.name);
                            if let Err(message) = send_sftp_request(
                                state.clone(),
                                ctx.session_id,
                                SftpRequest::Remove {
                                    path,
                                    is_dir: ctx.entry.is_dir,
                                },
                            ) {
                                tracing::error!("SFTP 删除请求投递失败: {}", message);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_inline_edit: move |ctx: SftpEntryContext| {
                        on_sftp_inline_edit.call(ctx);
                        context_menu.set(None);
                    },
                    on_sftp_external_edit: {
                        let state = Arc::clone(state);
                        move |ctx: SftpEntryContext| {
                            let file_name = ctx.entry.name.clone();
                            let editor = settings.peek().default_editor.clone();
                            if let Err(message) = start_sftp_external_edit(
                                state.clone(),
                                ctx,
                                editor,
                                external_edits,
                                next_external_edit_id,
                            ) {
                                tracing::error!("外部编辑下载启动失败: {}", message);
                                external_edit_notice.set(Some(format!(
                                    "{} {}: {}",
                                    texts(language).sftp.edit_status_failed,
                                    file_name,
                                    message
                                )));
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_open_with: {
                        let state = Arc::clone(state);
                        move |(ctx, command): (SftpEntryContext, Option<String>)| {
                            let file_name = ctx.entry.name.clone();
                            if let Err(message) = start_sftp_external_edit(
                                state.clone(),
                                ctx,
                                command,
                                external_edits,
                                next_external_edit_id,
                            ) {
                                tracing::error!("外部编辑下载启动失败: {}", message);
                                external_edit_notice.set(Some(format!(
                                    "{} {}: {}",
                                    texts(language).sftp.edit_status_failed,
                                    file_name,
                                    message
                                )));
                            }
                            context_menu.set(None);
                        }
                    },
                    on_copy_text: move |value: String| {
                        copy_to_clipboard(&value);
                        context_menu.set(None);
                    },
                    }
                }
            }

            if let Some(edit) = inline_edit() {
                match &edit.status {
                    // 载入中与载入失败都还没有内容可编辑，只渲染状态卡片。
                    InlineEditStatus::Loading | InlineEditStatus::LoadFailed(_) => rsx! {
                        InlineEditorStatus {
                            edit: edit.clone(),
                            language,
                            on_close: on_inline_edit_close,
                        }
                    },
                    _ => rsx! {
                        InlineEditorDialog {
                            edit: edit.clone(),
                            language,
                            on_save: on_inline_edit_save,
                            on_close: on_inline_edit_close,
                        }
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
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                match send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                ) {
                                    Ok(request_id) => edit.request_id = Some(request_id),
                                    Err(message) => {
                                        edit.status = ExternalEditStatus::PromptPending;
                                        edit.request_id = None;
                                        external_edit_notice.set(Some(format!(
                                            "{} {}: {}",
                                            texts(language).sftp.edit_status_failed,
                                            edit.file_name,
                                            message
                                        )));
                                    }
                                }
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
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                match send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                ) {
                                    Ok(request_id) => edit.request_id = Some(request_id),
                                    Err(message) => {
                                        edit.status = ExternalEditStatus::PromptPending;
                                        edit.request_id = None;
                                        external_edit_notice.set(Some(format!(
                                            "{} {}: {}",
                                            texts(language).sftp.edit_status_failed,
                                            edit.file_name,
                                            message
                                        )));
                                    }
                                }
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

            if let Some((session_id, session_title, generation, challenge)) =
                active_auth_challenge.clone()
            {
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
                                    generation,
                                    response: AuthResponse::Answers(answers),
                                }) {
                                    tracing::warn!("认证响应投递失败: {:?}", session_id);
                                }
                                if let Some(sess) = app_state.sessions.get_mut(&session_id) {
                                    sess.auth_challenge = None;
                                    sess.auth_challenge_generation = None;
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
                                    generation,
                                    response: AuthResponse::Cancel,
                                }) {
                                    tracing::warn!("认证取消响应投递失败: {:?}", session_id);
                                }
                                if let Some(sess) = app_state.sessions.get_mut(&session_id) {
                                    sess.auth_challenge = None;
                                    sess.auth_challenge_generation = None;
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
                        let state = Arc::clone(state);
                        move |prompt: PendingHostKey| {
                            match store.trust_host_key(&prompt.host, prompt.port, &prompt.fingerprint) {
                                Ok(_) => {
                                    store.clear_pending_host_key(
                                        &prompt.host,
                                        prompt.port,
                                        &prompt.fingerprint,
                                    );
                                    reconnect_host_key_pending_sessions(
                                        Arc::clone(&state),
                                        &prompt.host,
                                        prompt.port,
                                    );
                                    host_key_prompt.set(None);
                                    host_key_error.set(None);
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
                        let state = Arc::clone(state);
                        move |prompt: PendingHostKey| {
                            store.allow_host_key_once(prompt.clone());
                            store.clear_pending_host_key(
                                &prompt.host,
                                prompt.port,
                                &prompt.fingerprint,
                            );
                            reconnect_host_key_pending_sessions(
                                Arc::clone(&state),
                                &prompt.host,
                                prompt.port,
                            );
                            host_key_prompt.set(None);
                            host_key_error.set(None);
                        }
                    },
                    on_cancel: {
                        let store = Arc::clone(store);
                        let state = Arc::clone(state);
                        move |_| {
                            if let Some(prompt) = host_key_prompt.peek().clone() {
                                store.clear_pending_host_key(
                                    &prompt.host,
                                    prompt.port,
                                    &prompt.fingerprint,
                                );
                                clear_host_key_pending_state(
                                    Arc::clone(&state),
                                    &prompt.host,
                                    prompt.port,
                                );
                            }
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
                proxy_type: edit_proxy_type,
                proxy_host: edit_proxy_host,
                proxy_port: edit_proxy_port,
                proxy_username: edit_proxy_username,
                use_agent: edit_use_agent,
                forward_agent: edit_forward_agent,
                groups: saved_groups.clone(),
                saved_connections: saved_connection_refs(&saved_profiles, &edit_original_name()),
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
                                if let Err(message) = send_sftp_request(
                                    state.clone(),
                                    session_id,
                                    SftpRequest::Mkdir { path },
                                ) {
                                    tracing::error!("SFTP 新建目录请求投递失败: {}", message);
                                }
                            }
                            "rename" => {
                                let from = sftp_name_dialog_target_path();
                                if from.is_empty() {
                                    return;
                                }
                                let to = join_path(&parent_path(&from), &name);
                                if from != to {
                                    if let Err(message) = send_sftp_request(
                                        state.clone(),
                                        session_id,
                                        SftpRequest::Rename { from, to },
                                    ) {
                                        tracing::error!("SFTP 重命名请求投递失败: {}", message);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                },
            }

            PairingScanner {
                show: show_scanner,
                language,
                on_result: move |outcome: ScanOutcome| {
                    show_scanner.set(false);
                    let t = texts(language).app;
                    match outcome {
                        ScanOutcome::Decoded(text) => match parse_share_payload(&text) {
                            Some((url, code)) => {
                                share_scan_result.set(Some((url, code)));
                                sync_status.set(None);
                            }
                            // 扫到的不是本应用的配对二维码。
                            None => sync_status.set(Some(t.sync_scan_unsupported.to_string())),
                        },
                        ScanOutcome::PermissionDenied => {
                            sync_status.set(Some(t.sync_scan_denied.to_string()))
                        }
                        ScanOutcome::Unsupported => {
                            sync_status.set(Some(t.sync_scan_unsupported.to_string()))
                        }
                        ScanOutcome::Cancelled => {
                            sync_status.set(Some(t.sync_scan_cancelled.to_string()))
                        }
                    }
                },
            }

            SettingsPanel {
                show: show_settings,
                language,
                settings: current_settings.clone(),
                sync_busy: sync_busy(),
                sync_status: sync_status(),
                is_phone,
                active_share: active_share(),
                scan_supported: scan_supported(),
                scan_result: share_scan_result,
                on_sync_action: {
                    let store = Arc::clone(store);
                    move |action: SyncAction| {
                        // 这几个动作是本地即时操作，不占用同步忙状态。
                        match &action {
                            SyncAction::ScanPairingCode => {
                                show_scanner.set(true);
                                return;
                            }
                            SyncAction::StopLanShare => {
                                if let Some(handle) = sync_share_handle.take() {
                                    handle.stop();
                                }
                                active_share.set(None);
                                sync_status.set(None);
                                return;
                            }
                            SyncAction::CopyText(text) => {
                                copy_to_clipboard(text);
                                sync_status.set(Some(
                                    texts(language).app.sync_copied.to_string(),
                                ));
                                return;
                            }
                            _ => {}
                        }
                        if sync_busy() {
                            return;
                        }
                        sync_busy.set(true);
                        sync_status.set(Some(match language {
                            kt_config::AppLanguage::Chinese => "正在同步…".to_string(),
                            kt_config::AppLanguage::English => "Synchronizing…".to_string(),
                        }));
                        let store = Arc::clone(&store);
                        spawn(async move {
                            let imports_config = matches!(
                                action,
                                SyncAction::WebDavDownload { .. }
                                    | SyncAction::ImportLanShare { .. }
                            );
                            let result: Result<Option<String>, String> = match action {
                                SyncAction::WebDavUpload { url, username, password } => async {
                                    let endpoint = WebDavEndpoint::parse(
                                        &url,
                                        Some(username),
                                        Some(password),
                                    )
                                    .map_err(|error| error.to_string())?;
                                    let client = WebDavClient::new().map_err(|error| error.to_string())?;
                                    let envelope = SyncEnvelope::new(store.config_snapshot());
                                    let endpoint_key = endpoint.url().to_string();
                                    let etag = sync_remote_revision
                                        .peek()
                                        .as_ref()
                                        .filter(|(known_url, _)| known_url == &endpoint_key)
                                        .map(|(_, revision)| revision.clone());
                                    let precondition = match etag.as_deref() {
                                        Some(etag) => PutPrecondition::IfMatch(etag),
                                        None => PutPrecondition::CreateOnly,
                                    };
                                    let revision = client
                                        .upload(
                                            &endpoint,
                                            &envelope,
                                            precondition,
                                            &tokio_util::sync::CancellationToken::new(),
                                        )
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    sync_remote_revision.set(
                                        revision.map(|revision| (endpoint_key, revision.0)),
                                    );
                                    Ok(Some(match language {
                                        kt_config::AppLanguage::Chinese => "WebDAV 配置上传完成".to_string(),
                                        kt_config::AppLanguage::English => "WebDAV configuration uploaded".to_string(),
                                    }))
                                }.await,
                                SyncAction::WebDavDownload { url, username, password } => async {
                                    let endpoint = WebDavEndpoint::parse(
                                        &url,
                                        Some(username),
                                        Some(password),
                                    )
                                    .map_err(|error| error.to_string())?;
                                    let client = WebDavClient::new().map_err(|error| error.to_string())?;
                                    let remote = client
                                        .download(&endpoint, &tokio_util::sync::CancellationToken::new())
                                        .await
                                        .map_err(|error| error.to_string())?;
                                    store
                                        .replace_config_snapshot(remote.envelope.config)
                                        .map_err(|error| error.to_string())?;
                                    sync_remote_revision.set(remote.revision.map(|revision| {
                                        (endpoint.url().to_string(), revision.0)
                                    }));
                                    Ok(Some(match language {
                                        kt_config::AppLanguage::Chinese => "WebDAV 配置已导入".to_string(),
                                        kt_config::AppLanguage::English => "WebDAV configuration imported".to_string(),
                                    }))
                                }.await,
                                SyncAction::StartLanShare => async {
                                    let (handle, info) = start_share(
                                        store.config_snapshot(),
                                        DEFAULT_SHARE_TTL,
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                    sync_share_handle.set(Some(handle));
                                    // 地址与配对码交给设置面板渲染二维码，不再塞进状态文本。
                                    active_share.set(Some(ActiveShare {
                                        payload: encode_share_payload(
                                            &info.url,
                                            &info.pairing_code,
                                        ),
                                        url: info.url,
                                        pairing_code: info.pairing_code,
                                    }));
                                    Ok(Some(
                                        texts(language).app.sync_share_active.to_string(),
                                    ))
                                }.await,
                                // 本地即时动作已在上面提前返回。
                                SyncAction::ScanPairingCode
                                | SyncAction::StopLanShare
                                | SyncAction::CopyText(_) => Ok(None),
                                SyncAction::ImportLanShare { url, pairing_code } => async {
                                    let pending = import_share(
                                        &url,
                                        &normalize_pairing_code(&pairing_code),
                                        &tokio_util::sync::CancellationToken::new(),
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                    store
                                        .replace_config_snapshot(pending.envelope().config.clone())
                                        .map_err(|error| error.to_string())?;
                                    let acknowledged = pending
                                        .acknowledge(&tokio_util::sync::CancellationToken::new())
                                        .await;
                                    Ok(Some(match (language, acknowledged) {
                                        (kt_config::AppLanguage::Chinese, Ok(())) => {
                                            "局域网配置已导入".to_string()
                                        }
                                        (kt_config::AppLanguage::English, Ok(())) => {
                                            "LAN configuration imported".to_string()
                                        }
                                        (kt_config::AppLanguage::Chinese, Err(error)) => format!(
                                            "局域网配置已导入，但发送接收确认失败，可重新导入以关闭分享：{error}"
                                        ),
                                        (kt_config::AppLanguage::English, Err(error)) => format!(
                                            "LAN configuration imported, but acknowledgement failed; import again to close the share: {error}"
                                        ),
                                    }))
                                }.await,
                            };
                            match result {
                                Ok(message) => {
                                    if imports_config {
                                        settings.set(store.settings());
                                        saved_tick += 1;
                                    }
                                    sync_status.set(message);
                                }
                                Err(error) => sync_status.set(Some(error)),
                            }
                            sync_busy.set(false);
                        });
                    }
                },
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
                on_settings_change: {
                    let store = Arc::clone(store);
                    move |next: kt_config::AppSettings| {
                        match store.update_settings(next.clone()) {
                            Ok(()) => settings.set(next),
                            Err(e) => tracing::error!("保存设置失败: {}", e),
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

/// 打开内嵌编辑器：下载到本机私有临时文件，读入编辑框。
///
/// 目录列表里已经带了文件大小，超限的文件在这里直接拒绝，不浪费一次下载；
/// 返回 `Err` 表示无法开始（含超限），由调用方展示为通知。
fn start_inline_edit(
    state: Arc<Mutex<AppState>>,
    ctx: SftpEntryContext,
    mut inline_edit: Signal<Option<InlineEdit>>,
    language: kt_config::AppLanguage,
) -> Result<(), String> {
    if ctx.entry.is_dir {
        return Ok(());
    }

    if let Some(error) = inline_edit_size_rejection(ctx.entry.size) {
        return Err(inline_edit_load_error_text(&error, language));
    }

    let remote_path = join_path(&ctx.base_path, &ctx.entry.name);
    let local_path = external_edit_local_path(ctx.session_id, &remote_path);
    if let Some(parent) = local_path.parent() {
        ensure_private_edit_dir(parent).map_err(|error| error.to_string())?;
    }

    let request_id = send_sftp_request(
        state,
        ctx.session_id,
        SftpRequest::Download {
            remote: remote_path.clone(),
            local: local_path.clone(),
        },
    )?;

    inline_edit.set(Some(InlineEdit {
        session_id: ctx.session_id,
        remote_path,
        local_path,
        file_name: ctx.entry.name.clone(),
        request_id: Some(request_id),
        status: InlineEditStatus::Loading,
        original: String::new(),
    }));
    Ok(())
}

fn start_sftp_external_edit(
    state: Arc<Mutex<AppState>>,
    ctx: SftpEntryContext,
    editor_command: Option<String>,
    mut external_edits: Signal<Vec<ExternalEdit>>,
    mut next_external_edit_id: Signal<u64>,
) -> Result<(), String> {
    if ctx.entry.is_dir {
        return Ok(());
    }

    let remote_path = join_path(&ctx.base_path, &ctx.entry.name);
    let local_path = external_edit_local_path(ctx.session_id, &remote_path);
    if let Some(parent) = local_path.parent() {
        ensure_private_edit_dir(parent).map_err(|error| error.to_string())?;
    }

    let request_id = send_sftp_request(
        state,
        ctx.session_id,
        SftpRequest::Download {
            remote: remote_path.clone(),
            local: local_path.clone(),
        },
    )?;
    let edit = ExternalEdit {
        id: next_external_edit_id(),
        session_id: ctx.session_id,
        remote_path: remote_path.clone(),
        local_path: local_path.clone(),
        file_name: ctx.entry.name.clone(),
        request_id: Some(request_id),
        status: ExternalEditStatus::Downloading,
        sync_mode: ExternalEditSyncMode::Ask,
        editor_command,
        last_seen_modified: None,
        pending_modified: None,
    };
    next_external_edit_id.set(next_external_edit_id() + 1);
    let mut edits = external_edits.peek().clone();
    edits.push(edit);
    external_edits.set(edits);
    Ok(())
}

fn send_sftp_request(
    state: Arc<Mutex<AppState>>,
    session_id: SessionId,
    req: SftpRequest,
) -> Result<kt_core::SftpRequestId, String> {
    let mut app_state = state
        .lock()
        .map_err(|_| "应用状态不可用，SFTP 请求未发送".to_string())?;
    app_state.send_sftp_request(session_id, req)
}

fn clear_host_key_pending_state(state: Arc<Mutex<AppState>>, host: &str, port: u16) {
    if let Ok(mut app_state) = state.lock() {
        app_state.clear_host_key_pending_for(host, port);
    }
}

fn reconnect_host_key_pending_sessions(state: Arc<Mutex<AppState>>, host: &str, port: u16) {
    if let Ok(mut app_state) = state.lock() {
        app_state.reconnect_host_key_pending_for(host, port);
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
        AuthChallenge::Password { .. } | AuthChallenge::KeyPassphrase { .. } => {
            answers.first().filter(|answer| !answer.is_empty()).cloned()
        }
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
        Ok(()) => {}
        Err(e) => {
            let message = format!("{}: {}", texts(language).dialog.vault_save_failed, e);
            tracing::error!("{}", message);
            signals.status_notice.set(Some(message));
        }
    }
}

pub(crate) fn copy_to_clipboard(value: &str) {
    // 桌面端优先原生剪贴板：WebView 的异步剪贴板在部分平台会弹系统确认。
    if crate::clipboard::write_text(value).is_ok() {
        return;
    }
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
        assert_eq!(pending.password, " secret ");
    }

    #[test]
    fn whitespace_only_password_is_preserved() {
        let challenge = AuthChallenge::Password {
            user: "root".to_string(),
            host: "example.com".to_string(),
            port: 22,
        };

        let pending = pending_auth_secret(SessionId(1), &challenge, &["   ".to_string()])
            .expect("全空格仍可能是合法密码");

        assert_eq!(pending.password, "   ");
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
