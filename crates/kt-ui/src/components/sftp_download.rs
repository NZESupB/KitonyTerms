//! 桌面下载选择与进度；独立于当前选中的会话。
use super::{app::get_state, sftp::join_path, sidebar::SftpEntryContext};
use crate::i18n::texts;
use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{SessionId, SftpRequest, SftpRequestId};
use std::path::{Path, PathBuf};

fn folder_target(parent: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':']) {
        return Err("无效的远端目录名称".into());
    }
    Ok(parent.join(name))
}

#[component]
pub(crate) fn DownloadController(
    mut pending: Signal<Option<SftpEntryContext>>,
    language: AppLanguage,
) -> Element {
    let mut busy = use_signal(|| false);
    let mut message = use_signal(|| None::<String>);
    use_effect(move || {
        let Some(ctx) = pending() else {
            return;
        };
        pending.set(None);
        if *busy.peek() {
            message.set(Some(texts(language).sftp.downloading.to_string()));
            return;
        }
        busy.set(true);
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
                message.set(Some(format!("{}: {}", t.downloading, ctx.entry.name)));
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let status = download_status(ctx.session_id, request_id, language)?;
                    match status {
                        DownloadStatus::Done => return Ok(Some(local)),
                        DownloadStatus::Pending(Some(progress)) => message.set(Some(progress)),
                        DownloadStatus::Pending(None) => {}
                    }
                }
            }
            .await;
            match result {
                Ok(Some(local)) => {
                    message.set(Some(format!("{}: {}", t.downloaded, local.display())))
                }
                Ok(None) => {}
                Err(error) => message.set(Some(format!("{}: {error}", t.error_prefix))),
            }
            busy.set(false);
        });
    });
    rsx! {
        if let Some(text) = message() {
            div { class: "sftp-download-notice", role: "status",
                span { "{text}" }
                if !busy() {
                    button { onclick: move |_| message.set(None), "{texts(language).sftp.close}" }
                }
            }
        }
    }
}

enum DownloadStatus {
    Pending(Option<String>),
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
            .map(|p| {
                format!(
                    "{}: {} ({}/{})",
                    t.downloading, p.name, p.transferred, p.total
                )
            }),
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
}
