//! 文件六列布局与原生 WebView 拖动桥；只有完成调整时才持久化。

use crate::i18n::texts;
use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};
use kt_core::{SessionId, SftpEntry};

pub const COLUMN_MIN: [u16; 6] = [96, 110, 64, 64, 78, 90];
pub const COLUMN_MAX: u16 = 2048;
const COLUMN_WEIGHT: [f64; 6] = [1.4, 1.3, 0.7, 0.7, 0.9, 1.0];

pub fn normalize_widths(widths: [u16; 6]) -> [u16; 6] {
    std::array::from_fn(|i| widths[i].clamp(COLUMN_MIN[i], COLUMN_MAX))
}

fn column_style(widths: Option<[u16; 6]>) -> String {
    let (columns, min) = match widths.map(normalize_widths) {
        Some(widths) => (
            widths
                .iter()
                .map(|w| format!("{w}px"))
                .collect::<Vec<_>>()
                .join(" "),
            widths.iter().map(|w| u32::from(*w)).sum::<u32>(),
        ),
        None => (
            COLUMN_MIN
                .iter()
                .zip(COLUMN_WEIGHT)
                .map(|(min, weight)| format!("minmax({min}px, {weight}fr)"))
                .collect::<Vec<_>>()
                .join(" "),
            COLUMN_MIN.iter().map(|w| u32::from(*w)).sum::<u32>(),
        ),
    };
    format!("--sftp-cols: {columns}; --sftp-table-min: {}px;", min + 32)
}

pub fn file_type(entry: &SftpEntry, language: AppLanguage) -> &'static str {
    let t = texts(language).sftp;
    if entry
        .permissions
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        t.symlink
    } else if entry.is_dir {
        t.directory
    } else {
        t.file
    }
}

#[component]
pub fn SftpColumnHeader(language: AppLanguage) -> Element {
    let t = texts(language).sftp;
    rsx! {
        div { class: "sftp-table-head-row",
            for (index, label) in [t.name, t.modified, t.file_type, t.size, t.permissions, t.owner_group].into_iter().enumerate() {
                div { key: "{index}", class: "sftp-column",
                    span { title: "{label}", "{label}" }
                    div {
                        class: "sftp-column-resizer", role: "separator", tabindex: "0",
                        aria_orientation: "vertical", aria_label: "{label}", title: "{t.resize_column}",
                        aria_valuemin: "{COLUMN_MIN[index]}", aria_valuemax: "{COLUMN_MAX}",
                        "data-column-resize": "{index}",
                    }
                }
            }
        }
    }
}

#[component]
pub fn SftpColumns(
    session_id: SessionId,
    mut settings: Signal<AppSettings>,
    language: AppLanguage,
    children: Element,
) -> Element {
    let mut save_failed = use_signal(|| false);
    let mut bridge_version = use_signal(|| 0u64);
    let widths = settings().sftp_column_widths.map(normalize_widths);
    let style = column_style(widths);
    let id = format!("sftp-columns-{}", session_id.0);
    let minima = COLUMN_MIN.map(|w| w.to_string()).join(",");
    use_effect(use_reactive(
        (&session_id, &widths, &bridge_version()),
        move |(session_id, _, _)| {
            let script = format!(
                "const columnId = 'sftp-columns-{}';\n{}",
                session_id.0,
                include_str!("../assets/sftp_columns.js")
            );
            let mut eval = dioxus::document::eval(&script);
            spawn(async move {
                while let Ok(message) = eval.recv::<Vec<u16>>().await {
                    let next = if message.is_empty() {
                        None
                    } else {
                        let Ok(array) = <[u16; 6]>::try_from(message) else {
                            continue;
                        };
                        Some(normalize_widths(array))
                    };
                    match crate::components::app::get_store().update_sftp_column_widths(next) {
                        Ok(()) => {
                            settings.with_mut(|settings| settings.sftp_column_widths = next);
                            save_failed.set(false);
                            bridge_version += 1;
                        }
                        Err(_) => save_failed.set(true),
                    }
                }
            });
        },
    ));
    use_drop(move || {
        dioxus::document::eval(&format!(
            "window.__ktSftpColumnsCleanup?.get('sftp-columns-{}')?.();",
            session_id.0
        ));
    });
    let t = texts(language).sftp;
    rsx! {
        if save_failed() {
            p { class: "sftp-columns-error", role: "status", "{t.columns_save_failed}" }
        }
        div { id, class: "sftp-table-grid", style, "data-column-min": minima,
            {children}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn columns_keep_minima_and_bound_persisted_values() {
        assert_eq!(
            normalize_widths([0, 1, 2, 3, 4, u16::MAX]),
            [96, 110, 64, 64, 78, 2048]
        );
        assert!(column_style(None).contains("534px"));
        assert!(!column_style(Some([100; 6])).contains("minmax"));
    }
    #[test]
    fn file_type_uses_mode_before_followed_directory_flag() {
        let entry = |is_dir, mode| SftpEntry {
            name: "entry".into(),
            is_dir,
            size: 0,
            modified: None,
            permissions: mode,
            uid: None,
            gid: None,
            user: None,
            group: None,
        };
        assert_eq!(
            file_type(&entry(true, Some(0o120777)), AppLanguage::Chinese),
            "符号链接"
        );
        assert_eq!(
            file_type(&entry(true, None), AppLanguage::English),
            "Folder"
        );
        assert_eq!(file_type(&entry(false, None), AppLanguage::English), "File");
    }
}
