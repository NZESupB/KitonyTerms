//! SFTP 文件管理面板组件

use dioxus::prelude::*;
use kt_core::{SessionId, SftpEntry, SftpRequest, ToCore};
use std::sync::{Arc, Mutex};

use crate::state::AppState;

#[component]
pub fn SftpPanel(session_id: SessionId) -> Element {
    let state = crate::components::app::get_state().clone();
    let state_for_load = state.clone();
    let state_for_back = state.clone();
    let state_for_refresh = state.clone();

    // SFTP 状态
    let mut current_path = use_signal(|| "/".to_string());
    let entries = use_signal(Vec::<SftpEntry>::new);
    let loading = use_signal(|| false);
    let error_message = use_signal(|| None::<String>);

    // 初始加载
    use_effect(move || {
        load_directory(state_for_load.clone(), session_id, current_path(), loading, entries);
    });

    rsx! {
        div {
            style: "display: flex; flex-direction: column; height: 100%; background: #ffffff;",

            // 工具栏
            div {
                style: "padding: 12px; border-bottom: 1px solid #d3d7de; display: flex; align-items: center; gap: 8px;",

                // 返回上级目录按钮
                button {
                    style: "padding: 6px 12px; background: #e5e7eb; border: none; border-radius: 4px; cursor: pointer;",
                    onclick: move |_| {
                        let path = current_path();
                        if path != "/" {
                            let parent = parent_path(&path);
                            current_path.set(parent.clone());
                            load_directory(state_for_back.clone(), session_id, parent, loading, entries);
                        }
                    },
                    disabled: current_path() == "/",
                    "⬆️ 上级"
                }

                // 当前路径
                div {
                    style: "flex: 1; padding: 6px 12px; background: #f0f1f4; border-radius: 4px; font-family: monospace; font-size: 13px;",
                    "{current_path()}"
                }

                // 刷新按钮
                button {
                    style: "padding: 6px 12px; background: #10b981; color: white; border: none; border-radius: 4px; cursor: pointer;",
                    onclick: move |_| {
                        load_directory(state_for_refresh.clone(), session_id, current_path(), loading, entries);
                    },
                    "🔄 刷新"
                }

                // 新建文件夹按钮
                button {
                    style: "padding: 6px 12px; background: #2b7de9; color: white; border: none; border-radius: 4px; cursor: pointer;",
                    onclick: move |_| {
                        // TODO: 弹出对话框输入文件夹名
                        tracing::info!("新建文件夹");
                    },
                    "📁 新建"
                }
            }

            // 文件列表
            div {
                style: "flex: 1; overflow-y: auto; padding: 8px;",

                if loading() {
                    div {
                        style: "padding: 20px; text-align: center; color: #6b7280;",
                        "加载中..."
                    }
                } else if let Some(err) = error_message() {
                    div {
                        style: "padding: 20px; color: #dc2626; background: #fee2e2; border-radius: 6px; margin: 8px;",
                        "错误: {err}"
                    }
                } else {
                    // 文件列表表格
                    div {
                        style: "display: flex; flex-direction: column; gap: 2px;",

                        for entry in entries() {
                            div {
                                key: "{entry.name}",
                                style: "padding: 8px 12px; background: #f9fafb; border: 1px solid #e5e7eb; border-radius: 4px; cursor: pointer; hover: background: #f3f4f6; display: flex; align-items: center; gap: 12px;",
                                onclick: {
                                    let entry = entry.clone();
                                    let state_clone = state.clone();
                                    move |_| {
                                        if entry.is_dir {
                                            let new_path = join_path(&current_path(), &entry.name);
                                            current_path.set(new_path.clone());
                                            load_directory(state_clone.clone(), session_id, new_path, loading, entries);
                                        } else {
                                            tracing::info!("选择文件: {}", entry.name);
                                            // TODO: 显示文件操作菜单（下载、删除、重命名等）
                                        }
                                    }
                                },

                                // 图标
                                div {
                                    style: "font-size: 20px;",
                                    if entry.is_dir { "📁" } else { "📄" }
                                }

                                // 文件名
                                div {
                                    style: "flex: 1; font-size: 14px; color: #1f2937;",
                                    "{entry.name}"
                                }

                                // 文件大小
                                if !entry.is_dir {
                                    div {
                                        style: "font-size: 12px; color: #6b7280;",
                                        "{format_size(entry.size)}"
                                    }
                                }

                                // 修改时间
                                if let Some(modified) = entry.modified {
                                    div {
                                        style: "font-size: 12px; color: #6b7280;",
                                        "{format_time(modified)}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 加载目录列表
fn load_directory(
    state: Arc<Mutex<AppState>>,
    session_id: SessionId,
    path: String,
    mut loading: Signal<bool>,
    mut entries: Signal<Vec<SftpEntry>>,
) {
    loading.set(true);
    entries.set(Vec::new());

    if let Ok(app_state) = state.lock() {
        app_state.manager.send(ToCore::Sftp {
            id: session_id,
            req: SftpRequest::List { path },
        });
    }
}

/// 获取父目录路径
fn parent_path(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            "/".to_string()
        } else {
            trimmed[..pos].to_string()
        }
    } else {
        "/".to_string()
    }
}

/// 拼接路径
fn join_path(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", base, name)
    }
}

/// 格式化文件大小
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 格式化时间戳
fn format_time(timestamp: u32) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let time = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M").to_string()
}
