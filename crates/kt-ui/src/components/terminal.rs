//! 终端渲染组件（核心性能组件）

use dioxus::prelude::*;
use kt_core::term::{GridSnapshot, SnapshotCell};
use kt_core::{SessionId, ToCore};

/// GridSnapshot 的包装器，实现 PartialEq
#[derive(Clone)]
pub struct SnapshotWrapper(pub GridSnapshot);

impl PartialEq for SnapshotWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.0.revision == other.0.revision
    }
}

#[component]
pub fn Terminal(
    snapshot: SnapshotWrapper,
    session_id: SessionId,
    pane_id: String,
    trigger_highlights: Vec<String>,
) -> Element {
    let snapshot = &snapshot.0;
    let rows = snapshot.rows;
    let cols = snapshot.cols;

    // 获取全局 state 用于发送输入
    let state = crate::components::app::get_state().clone();
    let state_for_resize = state.clone();
    let state_for_input = state.clone();
    let state_for_scroll = state.clone();

    // 监听窗口大小变化（简单实现：每秒检查一次）
    use_effect(move || {
        let value = state_for_resize.clone();
        spawn(async move {
            let mut last_size = (80u16, 24u16);
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

                // 简单计算：假设字符宽度约 9px，高度约 17px
                // 实际应该通过 JavaScript 获取，这里暂时使用固定比例
                // TODO: 通过 eval 获取实际的终端 div 尺寸并计算

                let new_cols = 80u16; // 暂时固定
                let new_rows = 24u16; // 暂时固定

                if (new_cols, new_rows) != last_size {
                    last_size = (new_cols, new_rows);
                    if let Ok(app_state) = value.lock() {
                        app_state.manager.send(ToCore::Resize {
                            id: session_id,
                            cols: new_cols,
                            rows: new_rows,
                        });
                        tracing::debug!("调整终端大小: {}x{}", new_cols, new_rows);
                    }
                }
            }
        });
    });

    rsx! {
        div {
            id: "terminal-{session_id.0}-{pane_id}",
            class: "terminal-surface",
            tabindex: "0",
            autofocus: true,

            // 点击时获得焦点（暂时注释掉，Dioxus 会自动处理）
            onclick: move |_| {
                // 尝试让终端获得焦点
            },

            // 滚轮事件（滚动查看历史）
            onwheel: move |evt| {
                let delta_y = evt.delta().strip_units().y;
                let scroll_lines = if delta_y > 0.0 { 3 } else { -3 }; // 每次滚动3行

                if let Ok(app_state) = state_for_scroll.lock() {
                    app_state.manager.send(ToCore::Scroll {
                        id: session_id,
                        delta: scroll_lines,
                    });
                }
            },
            onkeydown: move |evt| {
                tracing::debug!("键盘事件: key={:?}, code={:?}", evt.key(), evt.code());

                let data: Vec<u8> = match evt.key() {
                    Key::Enter => vec![b'\r'],
                    Key::Backspace => vec![0x7f],
                    Key::Tab => vec![b'\t'],
                    Key::Escape => vec![0x1b],
                    Key::ArrowUp => vec![0x1b, b'[', b'A'],
                    Key::ArrowDown => vec![0x1b, b'[', b'B'],
                    Key::ArrowRight => vec![0x1b, b'[', b'C'],
                    Key::ArrowLeft => vec![0x1b, b'[', b'D'],
                    Key::Character(ref c) => {
                        if evt.modifiers().ctrl() {
                            if let Some(ch) = c.chars().next() {
                                if ch.is_ascii_alphabetic() {
                                    let byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                                    vec![byte]
                                } else {
                                    c.bytes().collect()
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            c.bytes().collect()
                        }
                    }
                    _ => vec![],
                };

                if !data.is_empty() {
                    tracing::debug!("发送输入: {:?}", data);
                    if let Ok(app_state) = state_for_input.lock() {
                        app_state.manager.send(ToCore::Input {
                            id: session_id,
                            data,
                        });
                    }
                }
            },

            // 渲染每一行
            for row in 0..rows {
                div {
                    key: "{row}",
                    class: if row_matches_trigger(snapshot, row, &trigger_highlights) {
                        "terminal-row is-trigger-highlight"
                    } else {
                        "terminal-row"
                    },

                    for col in 0..cols {
                        {
                            let idx = row * cols + col;
                            let cell = &snapshot.cells[idx];

                            if cell.attrs.wide_spacer {
                                rsx! { span { key: "{idx}" } }
                            } else {
                                render_cell(cell, idx, row == snapshot.cursor.line && col == snapshot.cursor.column)
                            }
                        }
                    }
                }
            }
        }
    }
}

fn row_matches_trigger(snapshot: &GridSnapshot, row: usize, triggers: &[String]) -> bool {
    if triggers.is_empty() {
        return false;
    }
    let start = row * snapshot.cols;
    let end = start + snapshot.cols;
    let line = snapshot.cells[start..end]
        .iter()
        .map(|cell| cell.c)
        .collect::<String>();
    line_matches_trigger(&line, triggers)
}

fn line_matches_trigger(line: &str, triggers: &[String]) -> bool {
    let line = line.to_ascii_lowercase();
    triggers
        .iter()
        .map(|trigger| trigger.trim().to_ascii_lowercase())
        .filter(|trigger| !trigger.is_empty())
        .any(|trigger| line.contains(&trigger))
}

fn render_cell(cell: &SnapshotCell, idx: usize, is_cursor: bool) -> Element {
    let fg_color = format!("rgb({}, {}, {})", cell.fg.r, cell.fg.g, cell.fg.b);
    let bg_color = format!("rgb({}, {}, {})", cell.bg.r, cell.bg.g, cell.bg.b);

    let mut style = format!(
        "color: {}; background: {}; display: inline-block;",
        fg_color, bg_color
    );

    if cell.attrs.bold {
        style.push_str(" font-weight: bold;");
    }
    if cell.attrs.italic {
        style.push_str(" font-style: italic;");
    }
    if cell.attrs.underline {
        style.push_str(" text-decoration: underline;");
    }
    if cell.attrs.strikeout {
        style.push_str(" text-decoration: line-through;");
    }
    if cell.attrs.dim {
        style.push_str(" opacity: 0.7;");
    }
    if cell.attrs.inverse {
        style = format!(
            "color: {}; background: {}; display: inline-block;",
            bg_color, fg_color
        );
    }

    // 光标高亮（带闪烁动画）
    if is_cursor {
        style.push_str(" background: #c0caf5; color: #1a1b26;");
    }

    let char_to_display = if cell.c == '\0' || cell.c == ' ' {
        "\u{00A0}"
    } else {
        &cell.c.to_string()
    };

    rsx! {
        span {
            key: "{idx}",
            style: "{style}",
            class: if is_cursor { "terminal-cursor" } else { "" },
            dangerous_inner_html: "{char_to_display}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_trigger_matching_is_case_insensitive_and_ignores_empty_rules() {
        let triggers = vec![" error ".to_string(), String::new()];
        assert!(line_matches_trigger("Build ERROR: failed", &triggers));
        assert!(!line_matches_trigger("all good", &triggers));
    }
}
