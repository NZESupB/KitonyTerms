//! SFTP 外部编辑状态机与本地文件工具。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{SessionId, SftpOp};

use crate::components::icons::Icon;
use crate::i18n::texts;
use crate::state::{AppState, SessionState, SftpProgressState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalEditStatus {
    Downloading,
    Watching,
    PromptPending,
    UploadingOnce,
    UploadingAuto,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalEditSyncMode {
    Ask,
    AutoUpload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalEdit {
    pub id: u64,
    pub session_id: SessionId,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub file_name: String,
    pub after_revision: u64,
    pub status: ExternalEditStatus,
    pub sync_mode: ExternalEditSyncMode,
    pub last_seen_modified: Option<SystemTime>,
    pub pending_modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalEditAction {
    OpenLocal {
        edit_id: u64,
        path: PathBuf,
        file_name: String,
    },
    Upload {
        session_id: SessionId,
        local_path: PathBuf,
        remote_path: String,
        file_name: String,
    },
    DeleteLocal(PathBuf),
    UploadCompleted {
        file_name: String,
    },
}

pub fn sync_external_edits(
    edits: Vec<ExternalEdit>,
    sessions: &HashMap<SessionId, SessionState>,
) -> (Vec<ExternalEdit>, Vec<ExternalEditAction>) {
    let mut next = Vec::with_capacity(edits.len());
    let mut actions = Vec::new();

    for mut edit in edits {
        let completion = sessions
            .get(&edit.session_id)
            .and_then(|session| session.sftp_last_done.as_ref())
            .filter(|completion| {
                completion.revision > edit.after_revision && completion.path == edit.remote_path
            });
        let completion_op = completion.map(|completion| completion.op);
        let completion_revision = completion
            .map(|completion| completion.revision)
            .unwrap_or(edit.after_revision);

        match (&edit.status, completion_op) {
            (ExternalEditStatus::Downloading, Some(SftpOp::Download)) => {
                edit.after_revision = completion_revision;
                edit.status = ExternalEditStatus::Watching;
                edit.last_seen_modified = local_file_modified(&edit.local_path);
                edit.pending_modified = None;
                actions.push(ExternalEditAction::OpenLocal {
                    edit_id: edit.id,
                    path: edit.local_path.clone(),
                    file_name: edit.file_name.clone(),
                });
                next.push(edit);
            }
            (ExternalEditStatus::UploadingOnce, Some(SftpOp::Upload)) => {
                actions.push(ExternalEditAction::UploadCompleted {
                    file_name: edit.file_name.clone(),
                });
                actions.push(ExternalEditAction::DeleteLocal(edit.local_path.clone()));
            }
            (ExternalEditStatus::UploadingAuto, Some(SftpOp::Upload)) => {
                edit.after_revision = completion_revision;
                edit.status = ExternalEditStatus::Watching;
                edit.last_seen_modified = local_file_modified(&edit.local_path)
                    .or(edit.pending_modified)
                    .or(edit.last_seen_modified);
                edit.pending_modified = None;
                actions.push(ExternalEditAction::UploadCompleted {
                    file_name: edit.file_name.clone(),
                });
                next.push(edit);
            }
            (ExternalEditStatus::Watching, _) => {
                let modified = local_file_modified(&edit.local_path);
                if is_newer_modified(modified, edit.last_seen_modified) {
                    edit.pending_modified = modified;
                    match edit.sync_mode {
                        ExternalEditSyncMode::Ask => {
                            edit.status = ExternalEditStatus::PromptPending;
                        }
                        ExternalEditSyncMode::AutoUpload => {
                            edit.status = ExternalEditStatus::UploadingAuto;
                            edit.after_revision = latest_sftp_completion_revision_from_sessions(
                                sessions,
                                edit.session_id,
                            );
                            edit.last_seen_modified = modified;
                            edit.pending_modified = None;
                            actions.push(ExternalEditAction::Upload {
                                session_id: edit.session_id,
                                local_path: edit.local_path.clone(),
                                remote_path: edit.remote_path.clone(),
                                file_name: edit.file_name.clone(),
                            });
                        }
                    }
                }
                next.push(edit);
            }
            _ => next.push(edit),
        }
    }

    (next, actions)
}

pub fn local_file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn is_newer_modified(current: Option<SystemTime>, previous: Option<SystemTime>) -> bool {
    match (current, previous) {
        (Some(current), Some(previous)) => current
            .duration_since(previous)
            .map(|duration| duration > Duration::from_millis(0))
            .unwrap_or(false),
        (Some(_), None) => true,
        _ => false,
    }
}

fn latest_sftp_completion_revision_from_sessions(
    sessions: &HashMap<SessionId, SessionState>,
    session_id: SessionId,
) -> u64 {
    sessions
        .get(&session_id)
        .and_then(|session| session.sftp_last_done.as_ref())
        .map(|completion| completion.revision)
        .unwrap_or(0)
}

pub fn latest_sftp_completion_revision(state: Arc<Mutex<AppState>>, session_id: SessionId) -> u64 {
    state
        .lock()
        .ok()
        .and_then(|app_state| {
            app_state
                .sessions
                .get(&session_id)
                .and_then(|session| session.sftp_last_done.as_ref())
                .map(|completion| completion.revision)
        })
        .unwrap_or(0)
}

pub fn external_edit_local_path(session_id: SessionId, remote_path: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("kitonyterms")
        .join("sftp-edit")
        .join(format!("session-{}", session_id.0))
        .join(format!(
            "{}-{}",
            stamp,
            sanitize_local_file_name(&remote_file_name(remote_path))
        ))
}

pub fn remote_file_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("remote-file")
        .to_string()
}

pub fn sanitize_local_file_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "remote-file".to_string()
    } else {
        sanitized
    }
}

pub fn open_local_file(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command.arg(&path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.arg("/C").arg("start").arg("").arg(&path);
        command
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(&path);
        command
    };

    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

pub fn external_edit_status_text(
    edits: &[ExternalEdit],
    session: Option<&SessionState>,
    notice: Option<&str>,
    language: AppLanguage,
) -> Option<String> {
    let t = texts(language).sftp;

    if let Some(edit) = edits
        .iter()
        .find(|edit| edit.status == ExternalEditStatus::Downloading)
    {
        return Some(format!("{} {}", t.edit_status_downloading, edit.file_name));
    }

    if let Some(edit) = edits
        .iter()
        .find(|edit| edit.status == ExternalEditStatus::PromptPending)
    {
        return Some(format!("{} {}", t.edit_status_saved, edit.file_name));
    }

    if let Some(edit) = edits.iter().find(|edit| {
        matches!(
            edit.status,
            ExternalEditStatus::UploadingOnce | ExternalEditStatus::UploadingAuto
        )
    }) {
        let progress = session
            .and_then(|session| session.sftp_progress.as_ref())
            .filter(|progress| progress.name == edit.file_name)
            .and_then(format_sftp_progress_percent)
            .map(|percent| format!(" {percent}%"))
            .unwrap_or_default();
        return Some(format!(
            "{} {}{}",
            t.edit_status_uploading, edit.file_name, progress
        ));
    }

    notice.map(str::to_string)
}

fn format_sftp_progress_percent(progress: &SftpProgressState) -> Option<u64> {
    if progress.total == 0 {
        return None;
    }
    Some(((progress.transferred.saturating_mul(100)) / progress.total).min(100))
}

#[component]
pub fn ExternalEditSaveDialog(
    edit: ExternalEdit,
    language: AppLanguage,
    on_upload_once: EventHandler<u64>,
    on_auto_upload: EventHandler<u64>,
    on_ignore: EventHandler<u64>,
) -> Element {
    let t = texts(language).sftp;
    let dialog_t = texts(language).dialog;
    let edit_id = edit.id;

    rsx! {
        div {
            class: "settings-overlay",

            section {
                class: "settings-panel external-edit-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{t.edit_save_prompt_title}" }
                    button {
                        class: "icon-button slim",
                        title: "{dialog_t.cancel}",
                        onclick: move |_| on_ignore.call(edit_id),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "external-edit-dialog-body",
                    Icon { name: "edit" }
                    div {
                        strong { "{edit.file_name}" }
                        p { "{t.edit_save_prompt_body}" }
                        code { "{edit.remote_path}" }
                    }
                }

                div {
                    class: "external-edit-dialog-actions",
                    button {
                        class: "primary",
                        onclick: move |_| on_upload_once.call(edit_id),
                        "{t.edit_save_once}"
                    }
                    button {
                        onclick: move |_| on_auto_upload.call(edit_id),
                        "{t.edit_save_auto}"
                    }
                    button {
                        onclick: move |_| on_ignore.call(edit_id),
                        "{t.edit_save_ignore}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_core::SftpEntry;

    fn session_state(id: SessionId) -> SessionState {
        SessionState {
            id,
            title: "demo".to_string(),
            snapshot: None,
            connected: true,
            connection_error: None,
            auth_challenge: None,
            sftp_path: ".".to_string(),
            sftp_entries: Vec::<SftpEntry>::new(),
            sftp_loading: false,
            sftp_error: None,
            sftp_last_done: None,
            sftp_progress: None,
            monitor: None,
            monitor_loading: false,
            monitor_error: None,
        }
    }

    fn temp_edit_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kitonyterms-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            name
        ))
    }

    #[test]
    fn external_edit_temp_names_are_sanitized() {
        assert_eq!(remote_file_name("/root/.bashrc"), ".bashrc");
        assert_eq!(sanitize_local_file_name("../a b.txt"), ".._a_b.txt");
        let path = external_edit_local_path(SessionId(9), "/root/a b.txt");
        assert!(path.to_string_lossy().contains("session-9"));
        assert!(path.to_string_lossy().contains("a_b.txt"));
    }

    #[test]
    fn external_edit_download_completion_opens_local_file() {
        let mut session = session_state(SessionId(1));
        session.sftp_last_done = Some(crate::state::SftpCompletion {
            op: SftpOp::Download,
            path: "/root/demo.txt".to_string(),
            revision: 2,
        });
        let sessions = HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 1,
            session_id: SessionId(1),
            remote_path: "/root/demo.txt".to_string(),
            local_path: PathBuf::from("/tmp/demo.txt"),
            file_name: "demo.txt".to_string(),
            after_revision: 1,
            status: ExternalEditStatus::Downloading,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: None,
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::Watching);
        assert_eq!(edits[0].after_revision, 2);
        assert_eq!(
            actions,
            vec![ExternalEditAction::OpenLocal {
                edit_id: 1,
                path: PathBuf::from("/tmp/demo.txt"),
                file_name: "demo.txt".to_string(),
            }]
        );
    }

    #[test]
    fn external_edit_upload_once_completion_removes_pending_item() {
        let mut session = session_state(SessionId(1));
        session.sftp_last_done = Some(crate::state::SftpCompletion {
            op: SftpOp::Upload,
            path: "/root/demo.txt".to_string(),
            revision: 5,
        });
        let sessions = HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 1,
            session_id: SessionId(1),
            remote_path: "/root/demo.txt".to_string(),
            local_path: PathBuf::from("/tmp/demo.txt"),
            file_name: "demo.txt".to_string(),
            after_revision: 4,
            status: ExternalEditStatus::UploadingOnce,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: None,
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert!(edits.is_empty());
        assert_eq!(
            actions,
            vec![
                ExternalEditAction::UploadCompleted {
                    file_name: "demo.txt".to_string(),
                },
                ExternalEditAction::DeleteLocal(PathBuf::from("/tmp/demo.txt")),
            ]
        );
    }

    #[test]
    fn external_edit_saved_file_prompts_before_upload() {
        let path = temp_edit_path("prompt.txt");
        std::fs::write(&path, b"changed").unwrap();
        let sessions = HashMap::new();
        let edit = ExternalEdit {
            id: 2,
            session_id: SessionId(1),
            remote_path: "/root/prompt.txt".to_string(),
            local_path: path.clone(),
            file_name: "prompt.txt".to_string(),
            after_revision: 0,
            status: ExternalEditStatus::Watching,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: Some(UNIX_EPOCH),
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::PromptPending);
        assert!(edits[0].pending_modified.is_some());
        assert!(actions.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn external_edit_auto_uploads_when_saved() {
        let path = temp_edit_path("auto.txt");
        std::fs::write(&path, b"changed").unwrap();
        let session = session_state(SessionId(1));
        let sessions = HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 3,
            session_id: SessionId(1),
            remote_path: "/root/auto.txt".to_string(),
            local_path: path.clone(),
            file_name: "auto.txt".to_string(),
            after_revision: 0,
            status: ExternalEditStatus::Watching,
            sync_mode: ExternalEditSyncMode::AutoUpload,
            last_seen_modified: Some(UNIX_EPOCH),
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::UploadingAuto);
        assert_eq!(
            actions,
            vec![ExternalEditAction::Upload {
                session_id: SessionId(1),
                local_path: path.clone(),
                remote_path: "/root/auto.txt".to_string(),
                file_name: "auto.txt".to_string(),
            }]
        );
        let _ = std::fs::remove_file(path);
    }
}
