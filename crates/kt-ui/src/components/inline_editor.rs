//! SFTP 内嵌编辑器：在应用内直接编辑远端文本文件。
//!
//! 与「外部编辑」的分工：外部编辑把文件交给系统编辑器并监听本地保存
//! （[`crate::components::external_edit`]）；内嵌编辑器把文件读进应用内的编辑框，
//! 由用户显式点保存后回传。Android/iOS 上 `open_with_system_default` 会走到
//! `xdg-open`，没有可用的外部编辑器链路，内嵌编辑器是手机上唯一的编辑路径；
//! 桌面端两者并存。
//!
//! 下载与回传复用既有的 SFTP 请求链路，因此覆盖写仍走 `kt-core` 的
//! 「同目录唯一临时文件 + 提交」原子语义，本模块不直接碰远端文件。
//!
//! 同一时刻只允许一个内嵌编辑会话：编辑器是全屏模态，多开既没有入口也没有意义，
//! 因此状态是 `Option<InlineEdit>` 而不是 `Vec`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{SessionId, SftpOp, SftpRequestId};

use crate::components::icons::Icon;
use crate::components::sidebar::format_sftp_size;
use crate::i18n::{inline_edit_too_large_message, texts};
use crate::state::SessionState;

/// 可在应用内编辑的最大字节数。
///
/// 编辑框是一次性把全文读进内存的纯文本框，不设上限会让手机在误点 18MB 的
/// `access.log` 时直接卡死。
pub const INLINE_EDIT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineEditStatus {
    /// 正在从远端下载。
    Loading,
    /// 已载入，可编辑。
    Ready,
    /// 正在回传。
    Saving,
    /// 载入失败：还没有内容可编辑，只能关闭。
    LoadFailed(String),
    /// 回传失败：内容仍在编辑框里，不能因为失败就关掉丢数据。
    SaveFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineEdit {
    pub session_id: SessionId,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub file_name: String,
    /// 当前进行中的下载或上传请求；空闲时为 `None`。
    pub request_id: Option<SftpRequestId>,
    pub status: InlineEditStatus,
    /// 下载后读到的原文。用于初始化编辑框，以及判断是否有未保存改动。
    pub original: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineEditAction {
    /// 下载完成，去读本地临时文件并载入编辑框。
    Load { local_path: PathBuf },
    /// 回传完成。
    Saved { file_name: String },
    /// 清理本地临时文件。
    DeleteLocal(PathBuf),
}

/// 载入失败的原因。用类型而不是字符串表达，便于单测断言且不把 i18n 混进 IO 逻辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineEditLoadError {
    TooLarge { limit: u64 },
    NotUtf8,
    Read(String),
}

pub fn inline_edit_load_error_text(error: &InlineEditLoadError, language: AppLanguage) -> String {
    let t = texts(language).sftp;
    match error {
        InlineEditLoadError::TooLarge { limit } => {
            inline_edit_too_large_message(language, &format_sftp_size(*limit, false))
        }
        InlineEditLoadError::NotUtf8 => t.editor_not_text.to_string(),
        InlineEditLoadError::Read(message) => format!("{}: {message}", t.editor_read_failed),
    }
}

/// 目录列表里已经带了大小，超限的文件不必先下载再拒绝。
pub fn inline_edit_size_rejection(size: u64) -> Option<InlineEditLoadError> {
    (size > INLINE_EDIT_MAX_BYTES).then_some(InlineEditLoadError::TooLarge {
        limit: INLINE_EDIT_MAX_BYTES,
    })
}

/// 把下载到本地的临时文件读成可编辑文本。
///
/// 即使 `inline_edit_size_rejection` 已经放行，这里仍要复查大小：列表里的 size 可能
/// 是旧的，远端文件也可能在下载期间变大。
pub fn read_editable_text(path: &Path) -> Result<String, InlineEditLoadError> {
    let data = std::fs::read(path).map_err(|error| InlineEditLoadError::Read(error.to_string()))?;
    if data.len() as u64 > INLINE_EDIT_MAX_BYTES {
        return Err(InlineEditLoadError::TooLarge {
            limit: INLINE_EDIT_MAX_BYTES,
        });
    }
    String::from_utf8(data).map_err(|_| InlineEditLoadError::NotUtf8)
}

/// 把编辑框内容写回本地临时文件，随后由调用方发起上传。
pub fn write_editable_text(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 推进内嵌编辑状态机。纯逻辑，IO 由调用方按返回的动作执行。
///
/// 与 [`crate::components::external_edit::sync_external_edits`] 同构：按 `request_id`
/// 精确消费请求级完成/失败事件，迟到的同路径事件不会误伤当前编辑。
pub fn sync_inline_edit(
    edit: Option<InlineEdit>,
    sessions: &HashMap<SessionId, SessionState>,
    language: AppLanguage,
) -> (Option<InlineEdit>, Vec<InlineEditAction>) {
    let Some(mut edit) = edit else {
        return (None, Vec::new());
    };
    let mut actions = Vec::new();

    let session = sessions.get(&edit.session_id);
    let completion = edit.request_id.and_then(|request_id| {
        session.and_then(|session| {
            session
                .sftp_completions
                .iter()
                .find(|completion| completion.request_id == request_id)
        })
    });
    let failure = edit.request_id.and_then(|request_id| {
        session.and_then(|session| {
            session
                .sftp_failures
                .iter()
                .find(|failure| failure.request_id == request_id)
        })
    });
    let session_closed = session.is_none_or(|session| !session.connected);

    let in_flight = matches!(
        edit.status,
        InlineEditStatus::Loading | InlineEditStatus::Saving
    );
    if failure.is_some() || (completion.is_none() && session_closed && in_flight) {
        let message = failure
            .map(|failure| failure.message.clone())
            .or_else(|| session.and_then(|session| session.connection_error.clone()))
            .unwrap_or_else(|| texts(language).sftp.editor_session_closed.to_string());
        edit.request_id = None;
        match edit.status {
            InlineEditStatus::Loading => {
                // 还没有内容，临时文件也没用了。
                actions.push(InlineEditAction::DeleteLocal(edit.local_path.clone()));
                edit.status = InlineEditStatus::LoadFailed(message);
            }
            InlineEditStatus::Saving => {
                // 保存失败必须保留编辑器与临时文件，让用户能重试。
                edit.status = InlineEditStatus::SaveFailed(message);
            }
            _ => {}
        }
        return (Some(edit), actions);
    }

    match (&edit.status, completion.map(|completion| completion.op)) {
        (InlineEditStatus::Loading, Some(SftpOp::Download)) => {
            // 清空 request_id 后本次完成事件不会被重复匹配，Load 只会派发一次。
            edit.request_id = None;
            actions.push(InlineEditAction::Load {
                local_path: edit.local_path.clone(),
            });
            (Some(edit), actions)
        }
        (InlineEditStatus::Saving, Some(SftpOp::Upload)) => {
            // 保存成功即关闭编辑器：内容已经在远端，本地临时文件不再需要。
            actions.push(InlineEditAction::Saved {
                file_name: edit.file_name.clone(),
            });
            actions.push(InlineEditAction::DeleteLocal(edit.local_path.clone()));
            (None, actions)
        }
        _ => (Some(edit), actions),
    }
}

fn inline_editor_status_can_close(status: &InlineEditStatus) -> bool {
    !matches!(status, InlineEditStatus::Loading)
}

/// 载入中与载入失败的轻量状态卡片。此时没有内容可编辑，不渲染编辑框。
#[component]
pub fn InlineEditorStatus(
    edit: InlineEdit,
    language: AppLanguage,
    on_close: EventHandler<()>,
) -> Element {
    let t = texts(language).sftp;
    let message = match &edit.status {
        InlineEditStatus::LoadFailed(message) => message.clone(),
        _ => t.editor_loading.to_string(),
    };
    let failed = matches!(edit.status, InlineEditStatus::LoadFailed(_));
    let can_close = inline_editor_status_can_close(&edit.status);

    rsx! {
        div {
            class: "inline-editor-overlay",
            onclick: move |_| {
                if can_close {
                    on_close.call(())
                }
            },

            section {
                class: "inline-editor-status",
                onclick: move |evt| evt.stop_propagation(),

                strong { "{edit.file_name}" }
                p {
                    class: if failed { "inline-editor-status-message is-error" } else { "inline-editor-status-message" },
                    "{message}"
                }
                button {
                    class: "inline-editor-button",
                    disabled: !can_close,
                    onclick: move |_| on_close.call(()),
                    "{texts(language).dialog.cancel}"
                }
            }
        }
    }
}

/// 编辑框本体。仅在有内容可编辑（Ready / Saving / SaveFailed）时渲染。
#[component]
pub fn InlineEditorDialog(
    edit: InlineEdit,
    language: AppLanguage,
    on_save: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let t = texts(language).sftp;
    // 只在挂载时播种一次：Ready → Saving → SaveFailed 之间组件不会重建，
    // 编辑内容因此不会被回退成 original。
    let mut content = use_signal(|| edit.original.clone());
    let mut confirm_discard = use_signal(|| false);

    let saving = edit.status == InlineEditStatus::Saving;
    let dirty = content() != edit.original;
    let save_error = match &edit.status {
        InlineEditStatus::SaveFailed(message) => Some(message.clone()),
        _ => None,
    };

    rsx! {
        div {
            class: "inline-editor-overlay",

            section {
                class: "inline-editor-panel",
                onclick: move |evt| evt.stop_propagation(),

                header {
                    class: "inline-editor-head",
                    div {
                        class: "inline-editor-title",
                        strong { "{edit.file_name}" }
                        small { "{edit.remote_path}" }
                    }
                    if dirty {
                        span { class: "inline-editor-dirty", "{t.editor_unsaved}" }
                    }
                    button {
                        class: "inline-editor-close",
                        "aria-label": "{texts(language).app.close}",
                        disabled: saving,
                        onclick: move |_| {
                            // 有未保存改动时先确认，避免一次误触丢掉全部编辑。
                            if dirty {
                                confirm_discard.set(true);
                            } else {
                                on_close.call(());
                            }
                        },
                        Icon { name: "close" }
                    }
                }

                if let Some(message) = save_error {
                    div { class: "inline-editor-banner is-error", "{message}" }
                }

                textarea {
                    class: "inline-editor-text",
                    value: "{content()}",
                    readonly: saving,
                    autocapitalize: "off",
                    autocorrect: "off",
                    autocomplete: "off",
                    spellcheck: false,
                    oninput: move |evt| content.set(evt.value()),
                }

                if confirm_discard() {
                    div {
                        class: "inline-editor-footer is-confirm",
                        span { "{t.editor_discard_prompt}" }
                        button {
                            class: "inline-editor-button",
                            onclick: move |_| confirm_discard.set(false),
                            "{texts(language).dialog.cancel}"
                        }
                        button {
                            class: "inline-editor-button is-danger",
                            onclick: move |_| on_close.call(()),
                            "{t.editor_discard}"
                        }
                    }
                } else {
                    div {
                        class: "inline-editor-footer",
                        span {
                            class: "inline-editor-hint",
                            if saving { "{t.editor_saving}" }
                        }
                        button {
                            class: "inline-editor-button is-primary",
                            disabled: saving || !dirty,
                            onclick: move |_| on_save.call(content()),
                            "{t.editor_save}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::app_logic::session_state_from_profile;
    use crate::state::{SftpCompletion, SftpFailure};
    use kt_config::{ConnectParams, SessionProfile};

    fn edit(status: InlineEditStatus, request_id: Option<u64>) -> InlineEdit {
        InlineEdit {
            session_id: SessionId(1),
            remote_path: "/etc/nginx/nginx.conf".to_string(),
            local_path: PathBuf::from("/tmp/kt/nginx.conf"),
            file_name: "nginx.conf".to_string(),
            request_id: request_id.map(SftpRequestId),
            status,
            original: "worker_processes 1;".to_string(),
        }
    }

    fn sessions(
        connected: bool,
        mutate: impl FnOnce(&mut SessionState),
    ) -> HashMap<SessionId, SessionState> {
        let profile = SessionProfile {
            name: "web".to_string(),
            group: None,
            params: ConnectParams::new("10.0.0.1", "root"),
        };
        let mut session = session_state_from_profile(SessionId(1), &profile, false);
        session.connected = connected;
        mutate(&mut session);
        HashMap::from([(SessionId(1), session)])
    }

    #[test]
    fn oversized_files_are_rejected_before_they_are_downloaded() {
        assert_eq!(inline_edit_size_rejection(INLINE_EDIT_MAX_BYTES), None);
        assert_eq!(
            inline_edit_size_rejection(INLINE_EDIT_MAX_BYTES + 1),
            Some(InlineEditLoadError::TooLarge {
                limit: INLINE_EDIT_MAX_BYTES
            })
        );
    }

    #[test]
    fn binary_and_oversized_content_never_reaches_the_editor() {
        let dir = tempfile::tempdir().unwrap();

        let text = dir.path().join("ok.conf");
        std::fs::write(&text, "server {\n  listen 80;\n}\n").unwrap();
        assert_eq!(
            read_editable_text(&text).unwrap(),
            "server {\n  listen 80;\n}\n"
        );

        // 含 NUL 的非 UTF-8 内容按二进制拒绝，而不是渲染成乱码。
        let binary = dir.path().join("app.bin");
        std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert_eq!(
            read_editable_text(&binary),
            Err(InlineEditLoadError::NotUtf8)
        );

        // 列表 size 过期时仍要在读取阶段兜住。
        let big = dir.path().join("big.log");
        std::fs::write(&big, vec![b'a'; INLINE_EDIT_MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(
            read_editable_text(&big),
            Err(InlineEditLoadError::TooLarge {
                limit: INLINE_EDIT_MAX_BYTES
            })
        );

        assert!(matches!(
            read_editable_text(&dir.path().join("missing")),
            Err(InlineEditLoadError::Read(_))
        ));
    }

    #[test]
    fn written_text_round_trips_and_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.conf");
        write_editable_text(&path, "listen 8080;").unwrap();
        assert_eq!(read_editable_text(&path).unwrap(), "listen 8080;");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "临时文件不得对其他用户可读");
        }
    }

    #[test]
    fn download_completion_asks_the_caller_to_load_the_file_once() {
        let sessions = sessions(true, |session| {
            session.sftp_completions.push_back(SftpCompletion {
                request_id: SftpRequestId(7),
                op: SftpOp::Download,
                path: "/etc/nginx/nginx.conf".to_string(),
            });
        });

        let (next, actions) = sync_inline_edit(
            Some(edit(InlineEditStatus::Loading, Some(7))),
            &sessions,
            AppLanguage::Chinese,
        );
        let next = next.unwrap();
        assert_eq!(
            actions,
            vec![InlineEditAction::Load {
                local_path: PathBuf::from("/tmp/kt/nginx.conf")
            }]
        );
        // request_id 已清空，下一轮不会重复派发 Load。
        assert_eq!(next.request_id, None);
        let (_, again) = sync_inline_edit(Some(next), &sessions, AppLanguage::Chinese);
        assert!(again.is_empty());
    }

    #[test]
    fn successful_upload_closes_the_editor_and_removes_the_temp_file() {
        let sessions = sessions(true, |session| {
            session.sftp_completions.push_back(SftpCompletion {
                request_id: SftpRequestId(9),
                op: SftpOp::Upload,
                path: "/etc/nginx/nginx.conf".to_string(),
            });
        });

        let (next, actions) = sync_inline_edit(
            Some(edit(InlineEditStatus::Saving, Some(9))),
            &sessions,
            AppLanguage::Chinese,
        );
        assert!(next.is_none(), "保存成功后编辑器关闭");
        assert_eq!(
            actions,
            vec![
                InlineEditAction::Saved {
                    file_name: "nginx.conf".to_string()
                },
                InlineEditAction::DeleteLocal(PathBuf::from("/tmp/kt/nginx.conf")),
            ]
        );
    }

    #[test]
    fn failed_save_keeps_the_editor_and_the_temp_file_so_edits_are_not_lost() {
        let sessions = sessions(true, |session| {
            session.sftp_failures.push_back(SftpFailure {
                request_id: SftpRequestId(9),
                message: "权限不足".to_string(),
            });
        });

        let (next, actions) = sync_inline_edit(
            Some(edit(InlineEditStatus::Saving, Some(9))),
            &sessions,
            AppLanguage::Chinese,
        );
        let next = next.unwrap();
        assert_eq!(
            next.status,
            InlineEditStatus::SaveFailed("权限不足".to_string())
        );
        assert_eq!(next.request_id, None);
        // 保存失败绝不能删掉本地内容。
        assert!(actions.is_empty());
    }

    #[test]
    fn failed_download_reports_the_error_and_cleans_up() {
        let sessions = sessions(true, |session| {
            session.sftp_failures.push_back(SftpFailure {
                request_id: SftpRequestId(7),
                message: "没有权限".to_string(),
            });
        });

        let (next, actions) = sync_inline_edit(
            Some(edit(InlineEditStatus::Loading, Some(7))),
            &sessions,
            AppLanguage::Chinese,
        );
        assert_eq!(
            next.unwrap().status,
            InlineEditStatus::LoadFailed("没有权限".to_string())
        );
        assert_eq!(
            actions,
            vec![InlineEditAction::DeleteLocal(PathBuf::from(
                "/tmp/kt/nginx.conf"
            ))]
        );
    }

    #[test]
    fn closing_the_session_mid_flight_resolves_both_directions() {
        let closed = sessions(false, |session| {
            session.connection_error = Some("连接已断开".to_string());
        });

        let (loading, _) = sync_inline_edit(
            Some(edit(InlineEditStatus::Loading, Some(7))),
            &closed,
            AppLanguage::Chinese,
        );
        assert_eq!(
            loading.unwrap().status,
            InlineEditStatus::LoadFailed("连接已断开".to_string())
        );

        let (saving, actions) = sync_inline_edit(
            Some(edit(InlineEditStatus::Saving, Some(9))),
            &closed,
            AppLanguage::Chinese,
        );
        assert_eq!(
            saving.unwrap().status,
            InlineEditStatus::SaveFailed("连接已断开".to_string())
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn idle_states_are_untouched_by_unrelated_traffic() {
        let sessions = sessions(true, |session| {
            session.sftp_completions.push_back(SftpCompletion {
                request_id: SftpRequestId(42),
                op: SftpOp::Download,
                path: "/other/file".to_string(),
            });
        });

        let ready = edit(InlineEditStatus::Ready, None);
        let (next, actions) =
            sync_inline_edit(Some(ready.clone()), &sessions, AppLanguage::Chinese);
        assert_eq!(next, Some(ready));
        assert!(actions.is_empty());

        assert_eq!(
            sync_inline_edit(None, &sessions, AppLanguage::Chinese),
            (None, Vec::new())
        );
    }

    #[test]
    fn load_errors_are_localized_and_distinct() {
        for language in [AppLanguage::Chinese, AppLanguage::English] {
            let messages = [
                InlineEditLoadError::TooLarge {
                    limit: INLINE_EDIT_MAX_BYTES,
                },
                InlineEditLoadError::NotUtf8,
                InlineEditLoadError::Read("boom".to_string()),
            ]
            .map(|error| inline_edit_load_error_text(&error, language));

            for message in &messages {
                assert!(!message.trim().is_empty(), "语言 {language:?} 缺少文案");
            }
            assert_ne!(messages[0], messages[1]);
            assert_ne!(messages[1], messages[2]);
            // 上限要以可读大小出现在文案里，用户才知道边界在哪。
            assert!(messages[0].contains("1.0 MB"));
        }
    }

    #[test]
    fn loading_status_cannot_close_before_download_converges() {
        assert!(!inline_editor_status_can_close(&InlineEditStatus::Loading));
        assert!(inline_editor_status_can_close(
            &InlineEditStatus::LoadFailed("失败".to_string())
        ));
        assert!(inline_editor_status_can_close(&InlineEditStatus::Ready));
    }

    #[test]
    fn closed_session_fallback_is_localized() {
        let closed = sessions(false, |_| {});
        let (english, _) = sync_inline_edit(
            Some(edit(InlineEditStatus::Loading, Some(7))),
            &closed,
            AppLanguage::English,
        );
        assert_eq!(
            english.unwrap().status,
            InlineEditStatus::LoadFailed("The session was closed".to_string())
        );

        let (chinese, _) = sync_inline_edit(
            Some(edit(InlineEditStatus::Loading, Some(7))),
            &closed,
            AppLanguage::Chinese,
        );
        assert_eq!(
            chinese.unwrap().status,
            InlineEditStatus::LoadFailed("会话已关闭".to_string())
        );
    }
}
