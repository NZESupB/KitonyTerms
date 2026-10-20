//! 桌面下载选择与进度；独立于当前选中的会话。
use super::{
    app::get_state, metrics_format::format_bytes, sftp::join_path, sidebar::SftpEntryContext,
};
use crate::{i18n::texts, state::SftpProgressState};
use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{SessionId, SftpBatchProgress, SftpRequest, SftpRequestId};
use std::path::{Path, PathBuf};

fn folder_target(parent: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) {
        return Err("无效的远端目录名称".into());
    }
    Ok(parent.join(name))
}

#[derive(Clone, PartialEq)]
enum DownloadNotice {
    Downloading {
        name: String,
        size: Option<u64>,
        batch: Option<SftpBatchProgress>,
    },
    Complete(String),
    Error(String),
}

impl DownloadNotice {
    fn percentage(&self) -> Option<u64> {
        match self {
            Self::Downloading { batch, .. } => {
                let batch = batch.as_ref()?;
                let total = batch.total?;
                // 文件落盘提交仍可能失败，成功事件到达后才展示 100%。
                Some(if total == 0 {
                    0
                } else {
                    ((u128::from(batch.transferred) * 100 / u128::from(total)).min(99)) as u64
                })
            }
            Self::Complete(_) => Some(100),
            Self::Error(_) => None,
        }
    }
}

#[component]
pub(crate) fn DownloadController(
    mut pending: Signal<Option<SftpEntryContext>>,
    language: AppLanguage,
) -> Element {
    let mut busy = use_signal(|| false);
    let mut notice = use_signal(|| None::<DownloadNotice>);
    use_effect(move || {
        let Some(ctx) = pending() else {
            return;
        };
        pending.set(None);
        if *busy.peek() {
            return;
        }
        busy.set(true);
        notice.set(None);
        spawn(async move {
            let t = texts(language).sftp;
            let result = async {
                let local = if ctx.entry.is_dir {
                    let Some(parent) = rfd::AsyncFileDialog::new()
                        .set_title(t.download_folder)
                        .pick_folder()
                        .await
                    else {
                        return Ok(None);
                    };
                    folder_target(parent.path(), &ctx.entry.name)?
                } else {
                    let Some(file) = rfd::AsyncFileDialog::new()
                        .set_title(t.download)
                        .set_file_name(&ctx.entry.name)
                        .save_file()
                        .await
                    else {
                        return Ok(None);
                    };
                    file.path().to_path_buf()
                };
                let overwrite = match std::fs::symlink_metadata(&local) {
                    Ok(_) => {
                        let answer = rfd::AsyncMessageDialog::new()
                            .set_title(t.download)
                            .set_description(format!(
                                "{}\n{}",
                                t.download_conflict,
                                local.display()
                            ))
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show()
                            .await;
                        if answer != rfd::MessageDialogResult::Ok {
                            return Ok(None);
                        }
                        true
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e.to_string()),
                };
                let state = get_state();
                let request_id = state
                    .lock()
                    .map_err(|_| t.state_unavailable.to_string())?
                    .send_sftp_request(
                        ctx.session_id,
                        SftpRequest::DownloadBatch {
                            remote: join_path(&ctx.base_path, &ctx.entry.name),
                            local: local.clone(),
                            overwrite,
                        },
                    )?;
                notice.set(Some(DownloadNotice::Downloading {
                    name: ctx.entry.name.clone(),
                    size: (!ctx.entry.is_dir).then_some(ctx.entry.size),
                    batch: None,
                }));
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    match download_status(ctx.session_id, request_id, language)? {
                        DownloadStatus::Done => return Ok(Some(local)),
                        DownloadStatus::Pending(Some(progress)) => {
                            notice.set(Some(DownloadNotice::Downloading {
                                name: progress.name,
                                size: Some(progress.total),
                                batch: progress.batch,
                            }));
                        }
                        DownloadStatus::Pending(None) => {}
                    }
                }
            }
            .await;
            match result {
                Ok(Some(local)) => notice.set(Some(DownloadNotice::Complete(format!(
                    "{}: {}",
                    t.downloaded,
                    local.display()
                )))),
                Ok(None) => notice.set(None),
                Err(error) => notice.set(Some(DownloadNotice::Error(format!(
                    "{}: {error}",
                    t.error_prefix
                )))),
            }
            busy.set(false);
        });
    });
    rsx! {
        if let Some(current) = notice() {
            DownloadNoticeView {
                notice: current,
                language,
                on_close: move |_| notice.set(None),
            }
        }
    }
}

#[component]
fn DownloadNoticeView(
    notice: DownloadNotice,
    language: AppLanguage,
    on_close: EventHandler<()>,
) -> Element {
    let t = texts(language).sftp;
    let downloading = matches!(&notice, DownloadNotice::Downloading { .. });
    let show_progress = !matches!(&notice, DownloadNotice::Error(_));
    let percentage = notice.percentage();
    rsx! {
        div { class: "sftp-download-notice", role: "status",
            div { class: "sftp-download-header",
                match &notice {
                    DownloadNotice::Downloading { name, size, .. } => rsx! {
                        span { class: "sftp-download-copy", title: "{name}",
                            "{t.downloading} {name.rsplit('/').next().unwrap_or(name)}"
                            if let Some(size) = size {
                                " "
                                span { class: "sftp-download-size", "{format_bytes(*size)}" }
                            }
                        }
                    },
                    DownloadNotice::Complete(message) | DownloadNotice::Error(message) => rsx! {
                        span { class: "sftp-download-copy", "{message}" }
                    },
                }
                if !downloading {
                    button { onclick: move |_| on_close.call(()), "{t.close}" }
                }
            }
            if show_progress {
                div { class: "sftp-download-progress",
                    div { class: "sftp-download-progress-meta",
                        span { "{t.download_progress}" }
                        if let Some(percent) = percentage {
                            span { "{percent}%" }
                        }
                    }
                    if let Some(percent) = percentage {
                        progress { max: 100, value: percent, aria_label: t.download_progress, aria_valuetext: "{percent}%" }
                    } else {
                        progress { max: 100, aria_label: t.download_progress }
                    }
                }
            }
        }
    }
}

enum DownloadStatus {
    Pending(Option<SftpProgressState>),
    Done,
}
fn download_status(
    id: SessionId,
    request_id: SftpRequestId,
    language: AppLanguage,
) -> Result<DownloadStatus, String> {
    let t = texts(language).sftp;
    let state = get_state()
        .lock()
        .map_err(|_| t.state_unavailable.to_string())?;
    let session = state.sessions.get(&id).ok_or(t.session_missing)?;
    if let Some(failure) = session
        .sftp_failures
        .iter()
        .find(|e| e.request_id == request_id)
    {
        return Err(failure.message.clone());
    }
    if session
        .sftp_completions
        .iter()
        .any(|e| e.request_id == request_id)
    {
        return Ok(DownloadStatus::Done);
    }
    if !session.connected || !session.sftp_pending_requests.contains(&request_id) {
        return Err(t.editor_session_closed.to_string());
    }
    Ok(DownloadStatus::Pending(
        session
            .sftp_progress
            .as_ref()
            .filter(|p| p.request_id == request_id)
            .cloned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_mapping_preserves_name_without_escape() {
        assert_eq!(
            folder_target(Path::new("/downloads"), "目录").unwrap(),
            PathBuf::from("/downloads/目录")
        );
        for name in ["..", "../escape", "/absolute", "C:escape", "a\\b"] {
            assert!(folder_target(Path::new("/downloads"), name).is_err());
        }
    }

    #[test]
    fn overall_percentage_uses_batch_bytes_and_waits_for_success() {
        let downloading = |transferred, total| DownloadNotice::Downloading {
            name: "folder/second.bin".into(),
            size: Some(80),
            batch: Some(SftpBatchProgress { transferred, total }),
        };
        assert_eq!(downloading(20, Some(100)).percentage(), Some(20));
        assert_eq!(downloading(60, Some(100)).percentage(), Some(60));
        assert_eq!(downloading(100, Some(100)).percentage(), Some(99));
        assert_eq!(downloading(150, Some(100)).percentage(), Some(99));
        assert_eq!(downloading(0, Some(0)).percentage(), Some(0));
        assert_eq!(downloading(40, None).percentage(), None);
        assert_eq!(
            downloading(u64::MAX / 2, Some(u64::MAX)).percentage(),
            Some(49)
        );
        assert_eq!(
            DownloadNotice::Complete("完成".into()).percentage(),
            Some(100)
        );
        assert_eq!(DownloadNotice::Error("断线".into()).percentage(), None);
    }
}
