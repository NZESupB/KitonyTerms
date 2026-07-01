//! 终端渲染组件（核心性能组件）

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::term::{CursorShape, GridSnapshot, SnapshotCell};
use kt_core::{SessionId, ToCore};

use crate::components::icons::Icon;
use crate::i18n::texts;
use crate::state::AppState;

/// 单行 gutter 元数据：内容签名 + 首次出现该内容时的时钟时间戳。
#[derive(Clone, Default)]
struct RowMeta {
    signature: u64,
    time: String,
    /// 该行是上一行 wrap 的续行（上一行行尾带 WRAPLINE），行号位置渲染 `-`。
    is_continuation: bool,
    /// live 视图下光标下方的未使用行：不渲染时间戳与行号。滚动历史视图下恒为 false。
    unused: bool,
}

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
    show_line_numbers: bool,
    show_timestamps: bool,
    language: AppLanguage,
) -> Element {
    let snapshot = &snapshot.0;
    let rows = snapshot.rows;
    let cols = snapshot.cols;

    let state = crate::components::app::get_state().clone();
    let state_for_resize = state.clone();
    let state_for_input = state.clone();
    let state_for_scroll = state.clone();
    let state_for_paste = state.clone();
    let terminal_id = format!("terminal-{}-{}", session_id.0, pane_id);
    let terminal_screen_id = terminal_screen_id(&terminal_id);
    let mut terminal_context_menu = use_signal(|| None::<TerminalContextMenuState>);
    let t = texts(language).app;

    // 行号/时间戳 gutter：用非响应式内部状态跨帧保存每行内容签名与首见时刻，
    // 避免在 render 中写响应式信号造成重渲染循环。时间戳为尽力而为的近似值。
    let show_gutter = show_line_numbers || show_timestamps;
    let row_meta_store = use_hook(|| Rc::new(RefCell::new(Vec::<RowMeta>::new())));
    let gutter_rows = if show_gutter {
        compute_gutter_rows(
            &row_meta_store,
            snapshot,
            show_line_numbers,
            show_timestamps,
        )
    } else {
        row_meta_store.borrow_mut().clear();
        Vec::new()
    };
    let surface_class = if show_gutter {
        "terminal-surface has-gutter"
    } else {
        "terminal-surface"
    };
    let surface_style = gutter_surface_style(show_line_numbers, show_timestamps);

    use_effect({
        let terminal_id = terminal_id.clone();
        let pane_id = pane_id.clone();
        move || {
            let value = state_for_resize.clone();
            let terminal_id = terminal_id.clone();
            let pane_id = pane_id.clone();
            spawn(async move {
                if pane_id != "primary" {
                    return;
                }

                let terminal_id_js = format!("{terminal_id:?}");
                let script = format!(
                    r#"
                const terminalId = {terminal_id_js};
                const element = document.getElementById(terminalId);
                if (!element) {{
                    return;
                }}

                const cleanupKey = "__ktResizeCleanup";
                if (element[cleanupKey]) {{
                    element[cleanupKey]();
                }}

                const measure = () => {{
                    const rect = element.getBoundingClientRect();
                    const style = window.getComputedStyle(element);
                    const probe = document.createElement("span");
                    probe.textContent = "W";
                    probe.style.position = "absolute";
                    probe.style.visibility = "hidden";
                    probe.style.whiteSpace = "pre";
                    probe.style.font = style.font;
                    element.appendChild(probe);
                    const probeRect = probe.getBoundingClientRect();
                    probe.remove();

                    const charWidth = probeRect.width || 9;
                    const lineHeight = parseFloat(style.lineHeight) || probeRect.height || 18;
                    const paddingX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
                    const paddingY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
                    const cols = Math.floor(Math.max(0, rect.width - paddingX) / charWidth);
                    const rows = Math.floor(Math.max(0, rect.height - paddingY) / lineHeight);
                    dioxus.send([cols, rows]);
                }};

                const observer = new ResizeObserver(measure);
                observer.observe(element);
                window.addEventListener("resize", measure);
                element[cleanupKey] = () => {{
                    observer.disconnect();
                    window.removeEventListener("resize", measure);
                    delete element[cleanupKey];
                }};
                measure();
                await new Promise(() => {{}});
                "#
                );

                let mut eval = dioxus::document::eval(&script);
                let mut last_size = (0u16, 0u16);
                while let Ok(payload) = eval.recv::<Vec<f64>>().await {
                    let Some((new_cols, new_rows)) = resize_payload_to_pty(&payload) else {
                        continue;
                    };
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
        }
    });

    use_effect({
        let menu = terminal_context_menu;
        move || {
            if let Some(menu) = menu() {
                let menu_x = menu.x;
                let menu_y = menu.y;
                let script = format!(
                    r#"
                    requestAnimationFrame(() => {{
                        const menu = document.querySelector('[data-kt-terminal-context-menu="active"]');
                        if (!menu) return;
                        const margin = 8;
                        menu.style.left = '{menu_x}px';
                        menu.style.top = '{menu_y}px';
                        menu.style.right = 'auto';
                        menu.style.bottom = 'auto';
                        const rect = menu.getBoundingClientRect();
                        if (rect.right > window.innerWidth - margin) {{
                            menu.style.left = `${{Math.max(margin, window.innerWidth - rect.width - margin)}}px`;
                        }}
                        if (rect.bottom > window.innerHeight - margin) {{
                            menu.style.top = 'auto';
                            menu.style.bottom = `${{Math.max(margin, window.innerHeight - {menu_y})}}px`;
                        }}
                    }});
                    "#
                );
                dioxus::document::eval(&script);
            }
        }
    });

    rsx! {
        div {
            id: "{terminal_id}",
            class: "{surface_class}",
            style: "{surface_style}",
            tabindex: "0",
            autofocus: true,

            // 点击时获得焦点（暂时注释掉，Dioxus 会自动处理）
            onclick: move |_| {
                terminal_context_menu.set(None);
                // 尝试让终端获得焦点
            },

            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                terminal_context_menu.set(Some(TerminalContextMenuState {
                    x: evt.client_coordinates().x,
                    y: evt.client_coordinates().y,
                }));
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
                terminal_context_menu.set(None);
                tracing::debug!("键盘事件: key={:?}, code={:?}", evt.key(), evt.code());

                let data = terminal_input_for_key(&evt.key(), evt.modifiers().ctrl());

                if !data.is_empty() {
                    evt.prevent_default();
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
            if show_gutter {
                div {
                    class: "terminal-gutter",
                    for (idx, meta) in gutter_rows.iter().enumerate() {
                        div {
                            key: "gutter-{idx}",
                            class: "terminal-gutter-row",
                            // 光标下方未使用的行不显示时间戳与行号。
                            if show_timestamps && !meta.unused {
                                span { class: "terminal-gutter-time", "{meta.time}" }
                            }
                            if show_line_numbers && !meta.unused {
                                if meta.is_continuation {
                                    // wrap 续行：用 `-` 占位，物理行号在此跳过。
                                    span { class: "terminal-gutter-lineno is-continuation", "-" }
                                } else {
                                    span { class: "terminal-gutter-lineno", "{idx + 1}" }
                                }
                            }
                        }
                    }
                }
            }

            div {
                id: "{terminal_screen_id}",
                class: "terminal-screen",

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
                                    rsx! {
                                        span {
                                            key: "{idx}",
                                            style: "{terminal_spacer_style()}"
                                        }
                                    }
                                } else {
                                    render_cell(
                                        cell,
                                        idx,
                                        terminal_cursor_class(
                                            row == snapshot.cursor.line && col == snapshot.cursor.column,
                                            snapshot.cursor.shape,
                                        ),
                                    )
                                }
                            }
                        }
                    }
                }
            }

            if let Some(menu) = terminal_context_menu() {
                div {
                    class: "context-menu terminal-context-menu",
                    "data-kt-terminal-context-menu": "active",
                    style: "{terminal_context_menu_style(menu)}",
                    onclick: move |evt| evt.stop_propagation(),
                    oncontextmenu: move |evt| {
                        evt.prevent_default();
                        evt.stop_propagation();
                    },

                    button {
                        onclick: {
                            let terminal_id = terminal_id.clone();
                            move |_| {
                                terminal_context_menu.set(None);
                                copy_selected_terminal_text(&terminal_id);
                            }
                        },
                        Icon { name: "file" }
                        span { "{t.copy}" }
                        small { "Cmd/Ctrl+C" }
                    }
                    button {
                        onclick: {
                            let state = state_for_paste.clone();
                            move |_| {
                                terminal_context_menu.set(None);
                                paste_clipboard_to_terminal(state.clone(), session_id);
                            }
                        },
                        Icon { name: "download" }
                        span { "{t.paste}" }
                        small { "Cmd/Ctrl+V" }
                    }
                    button {
                        onclick: {
                            let terminal_screen_id = terminal_screen_id.clone();
                            move |_| {
                                terminal_context_menu.set(None);
                                select_terminal_contents(&terminal_screen_id);
                            }
                        },
                        Icon { name: "list" }
                        span { "{t.select_all}" }
                        small { "Cmd/Ctrl+A" }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalContextMenuState {
    x: f64,
    y: f64,
}

fn terminal_screen_id(terminal_id: &str) -> String {
    format!("{terminal_id}-screen")
}

/// gutter 宽度（以 ch 为单位）：行号约 5ch，时间戳 `[HH:MM:SS] ` 约 11ch。
/// 通过给 surface 增加等宽的左内边距实现，resize 脚本会自动扣除该内边距，
/// 从而保证 PTY 列数计算不受 gutter 影响。
fn gutter_width_ch(show_line_numbers: bool, show_timestamps: bool) -> u32 {
    let mut width = 0;
    if show_line_numbers {
        width += 6;
    }
    if show_timestamps {
        width += 11;
    }
    width
}

fn gutter_surface_style(show_line_numbers: bool, show_timestamps: bool) -> String {
    let width = gutter_width_ch(show_line_numbers, show_timestamps);
    if width == 0 {
        String::new()
    } else {
        // 覆盖左内边距为 gutter 宽度 + 原有 8px 视觉留白。
        format!("padding-left: calc({width}ch + 8px);")
    }
}

/// 计算每行 gutter 元数据。内容未变化的行沿用旧时间戳；变化或新出现的行
/// （含空行）记录当前时钟时间。wrap 续行标记 `is_continuation` 供行号位渲染 `-`。
fn compute_gutter_rows(
    store: &Rc<RefCell<Vec<RowMeta>>>,
    snapshot: &GridSnapshot,
    _show_line_numbers: bool,
    show_timestamps: bool,
) -> Vec<RowMeta> {
    let rows = snapshot.rows;
    let cols = snapshot.cols;
    let mut prev = store.borrow_mut();
    let now_label = if show_timestamps {
        current_clock_label()
    } else {
        String::new()
    };

    let mut next = Vec::with_capacity(rows);
    for row in 0..rows {
        let signature = row_signature(snapshot, row, cols);
        // 续行判定：上一行行尾带 WRAPLINE 表示其内容溢出到本行。
        let is_continuation = row > 0 && snapshot.wrapped.get(row - 1).copied().unwrap_or(false);
        // live 视图下，光标下方为尚未使用的空白区，不显示时间戳与行号；
        // 滚动历史视图下所有可见行都是已使用内容，全部显示。
        let unused = snapshot.display_offset == 0 && row > snapshot.cursor.line;
        // 空行同样显示时间戳：未变化时沿用旧值，变化或新行用当前时刻。
        let time = match prev.get(row) {
            Some(existing) if existing.signature == signature => existing.time.clone(),
            _ => now_label.clone(),
        };
        next.push(RowMeta {
            signature,
            time,
            is_continuation,
            unused,
        });
    }

    *prev = next.clone();
    next
}

/// 计算某可见行的内容签名，用于判断行内容是否变化。
fn row_signature(snapshot: &GridSnapshot, row: usize, cols: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let start = row * cols;
    let end = (start + cols).min(snapshot.cells.len());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in &snapshot.cells[start..end] {
        cell.c.hash(&mut hasher);
    }
    hasher.finish()
}

fn current_clock_label() -> String {
    let now: chrono::DateTime<chrono::Local> = chrono::Local::now();
    now.format("[%H:%M:%S]").to_string()
}

fn terminal_context_menu_style(menu: TerminalContextMenuState) -> String {
    format!("left: {:.0}px; top: {:.0}px;", menu.x, menu.y)
}

fn copy_selected_terminal_text(terminal_id: &str) {
    let terminal_id = format!("{terminal_id:?}");
    let script = format!(
        r#"
        (() => {{
            const root = document.getElementById({terminal_id});
            const selection = window.getSelection ? window.getSelection() : null;
            if (!root || !selection || selection.rangeCount === 0) {{
                return;
            }}

            const range = selection.getRangeAt(0);
            const node = range.commonAncestorContainer;
            const element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
            if (!element || !root.contains(element)) {{
                return;
            }}

            const text = selection.toString().replace(/\u00a0/g, " ");
            if (!text) {{
                return;
            }}

            const fallbackCopy = (value) => {{
                const el = document.createElement("textarea");
                el.value = value;
                el.style.position = "fixed";
                el.style.opacity = "0";
                document.body.appendChild(el);
                el.select();
                document.execCommand("copy");
                document.body.removeChild(el);
            }};

            if (navigator.clipboard && navigator.clipboard.writeText) {{
                navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
            }} else {{
                fallbackCopy(text);
            }}
            root.focus();
        }})();
        "#
    );
    dioxus::document::eval(&script);
}

fn paste_clipboard_to_terminal(state: Arc<Mutex<AppState>>, session_id: SessionId) {
    let mut eval = dioxus::document::eval(
        r#"
        (async () => {
            try {
                if (navigator.clipboard && navigator.clipboard.readText) {
                    dioxus.send(await navigator.clipboard.readText());
                    return;
                }
            } catch (error) {
                console.warn("读取剪贴板失败", error);
            }
            dioxus.send("");
        })();
        "#,
    );

    spawn(async move {
        match eval.recv::<String>().await {
            Ok(text) => send_terminal_text(state, session_id, &text),
            Err(error) => tracing::warn!("终端粘贴读取失败: {}", error),
        }
    });
}

fn send_terminal_text(state: Arc<Mutex<AppState>>, session_id: SessionId, text: &str) {
    let data = terminal_paste_input(text);
    if data.is_empty() {
        return;
    }

    if let Ok(app_state) = state.lock() {
        app_state.manager.send(ToCore::Input {
            id: session_id,
            data,
        });
    }
}

fn terminal_paste_input(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").replace('\r', "\n").into_bytes()
}

fn select_terminal_contents(terminal_screen_id: &str) {
    let terminal_screen_id = format!("{terminal_screen_id:?}");
    let script = format!(
        r#"
        (() => {{
            const root = document.getElementById({terminal_screen_id});
            if (!root) {{
                return;
            }}
            const selection = window.getSelection ? window.getSelection() : null;
            if (!selection) {{
                return;
            }}
            const range = document.createRange();
            range.selectNodeContents(root);
            selection.removeAllRanges();
            selection.addRange(range);
            root.parentElement?.focus();
        }})();
        "#
    );
    dioxus::document::eval(&script);
}

fn terminal_input_for_key(key: &Key, ctrl: bool) -> Vec<u8> {
    match key {
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::ArrowUp => csi_final(b'A'),
        Key::ArrowDown => csi_final(b'B'),
        Key::ArrowRight => csi_final(b'C'),
        Key::ArrowLeft => csi_final(b'D'),
        Key::Home => csi_final(b'H'),
        Key::End => csi_final(b'F'),
        Key::Insert => csi_numbered(2),
        Key::Delete => csi_numbered(3),
        Key::PageUp => csi_numbered(5),
        Key::PageDown => csi_numbered(6),
        Key::F1 => ss3_final(b'P'),
        Key::F2 => ss3_final(b'Q'),
        Key::F3 => ss3_final(b'R'),
        Key::F4 => ss3_final(b'S'),
        Key::F5 => csi_numbered(15),
        Key::F6 => csi_numbered(17),
        Key::F7 => csi_numbered(18),
        Key::F8 => csi_numbered(19),
        Key::F9 => csi_numbered(20),
        Key::F10 => csi_numbered(21),
        Key::F11 => csi_numbered(23),
        Key::F12 => csi_numbered(24),
        Key::Character(c) => character_input(c, ctrl),
        _ => Vec::new(),
    }
}

fn character_input(value: &str, ctrl: bool) -> Vec<u8> {
    if ctrl {
        if let Some(ch) = value.chars().next() {
            if ch.is_ascii_alphabetic() {
                return vec![(ch.to_ascii_lowercase() as u8) - b'a' + 1];
            }
        }
    }
    value.bytes().collect()
}

fn csi_final(final_byte: u8) -> Vec<u8> {
    vec![0x1b, b'[', final_byte]
}

fn csi_numbered(number: u8) -> Vec<u8> {
    format!("\x1b[{number}~").into_bytes()
}

fn ss3_final(final_byte: u8) -> Vec<u8> {
    vec![0x1b, b'O', final_byte]
}

fn resize_payload_to_pty(payload: &[f64]) -> Option<(u16, u16)> {
    let cols = payload.first().copied()?;
    let rows = payload.get(1).copied()?;
    Some((
        clamp_pty_dimension(cols, 20, 500),
        clamp_pty_dimension(rows, 5, 200),
    ))
}

fn clamp_pty_dimension(value: f64, min: u16, max: u16) -> u16 {
    if value.is_finite() {
        (value.round() as i32).clamp(min as i32, max as i32) as u16
    } else {
        min
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

fn render_cell(cell: &SnapshotCell, idx: usize, cursor_class: &'static str) -> Element {
    let style = terminal_cell_style(cell);
    let char_to_display = terminal_cell_text(cell);

    rsx! {
        span {
            key: "{idx}",
            style: "{style}",
            class: "{cursor_class}",
            "{char_to_display}"
        }
    }
}

fn terminal_cursor_class(is_cursor: bool, shape: CursorShape) -> &'static str {
    if !is_cursor || shape == CursorShape::Hidden {
        return "";
    }

    match shape {
        CursorShape::Block => "terminal-cursor terminal-cursor-block",
        CursorShape::Bar => "terminal-cursor terminal-cursor-bar",
        CursorShape::Underline => "terminal-cursor terminal-cursor-underline",
        CursorShape::Hidden => "",
    }
}

fn terminal_cell_style(cell: &SnapshotCell) -> String {
    let fg_color = format!("rgb({}, {}, {})", cell.fg.r, cell.fg.g, cell.fg.b);
    let bg_color = format!("rgb({}, {}, {})", cell.bg.r, cell.bg.g, cell.bg.b);
    let bg_value = if cell_has_visible_background(cell) {
        bg_color.as_str()
    } else {
        "transparent"
    };
    let font_weight = if cell.attrs.bold { "700" } else { "400" };
    let font_style = if cell.attrs.italic {
        "italic"
    } else {
        "normal"
    };
    let text_decoration = terminal_text_decoration(cell);
    let opacity = if cell.attrs.dim { "0.7" } else { "1" };

    format!(
        "color: {fg_color}; background: {bg_value}; display: inline-block; position: relative; width: 1ch; font-weight: {font_weight}; font-style: {font_style}; text-decoration: {text_decoration}; opacity: {opacity};"
    )
}

fn terminal_spacer_style() -> &'static str {
    "background: transparent; display: inline-block; position: relative; width: 0; font-weight: 400; font-style: normal; text-decoration: none; opacity: 1;"
}

fn terminal_text_decoration(cell: &SnapshotCell) -> &'static str {
    if is_blank_cell(cell) {
        return "none";
    }

    match (cell.attrs.underline, cell.attrs.strikeout) {
        (true, true) => "underline line-through",
        (true, false) => "underline",
        (false, true) => "line-through",
        (false, false) => "none",
    }
}

fn cell_has_visible_background(cell: &SnapshotCell) -> bool {
    cell.bg.r != kt_core::term::color::DEFAULT_BG.r
        || cell.bg.g != kt_core::term::color::DEFAULT_BG.g
        || cell.bg.b != kt_core::term::color::DEFAULT_BG.b
}

fn is_blank_cell(cell: &SnapshotCell) -> bool {
    cell.c == '\0' || cell.c == ' '
}

fn terminal_cell_text(cell: &SnapshotCell) -> String {
    if is_blank_cell(cell) {
        "\u{00A0}".to_string()
    } else {
        cell.c.to_string()
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

    #[test]
    fn function_keys_emit_xterm_sequences() {
        assert_eq!(terminal_input_for_key(&Key::F1, false), b"\x1bOP");
        assert_eq!(terminal_input_for_key(&Key::F2, false), b"\x1bOQ");
        assert_eq!(terminal_input_for_key(&Key::F12, false), b"\x1b[24~");
    }

    #[test]
    fn navigation_keys_emit_terminal_sequences() {
        assert_eq!(terminal_input_for_key(&Key::Home, false), b"\x1b[H");
        assert_eq!(terminal_input_for_key(&Key::End, false), b"\x1b[F");
        assert_eq!(terminal_input_for_key(&Key::Delete, false), b"\x1b[3~");
        assert_eq!(terminal_input_for_key(&Key::PageUp, false), b"\x1b[5~");
    }

    #[test]
    fn resize_payload_is_clamped_for_pty() {
        assert_eq!(resize_payload_to_pty(&[120.0, 40.0]), Some((120, 40)));
        assert_eq!(resize_payload_to_pty(&[1.0, 1.0]), Some((20, 5)));
        assert_eq!(resize_payload_to_pty(&[900.0, 300.0]), Some((500, 200)));
        assert_eq!(resize_payload_to_pty(&[f64::NAN, 10.0]), Some((20, 10)));
        assert_eq!(resize_payload_to_pty(&[80.0]), None);
    }

    #[test]
    fn default_background_cells_do_not_paint_full_row_background() {
        let cell = SnapshotCell {
            c: ' ',
            attrs: kt_core::term::snapshot::CellAttrs {
                underline: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let style = terminal_cell_style(&cell);

        assert!(!style.contains("background: rgb"));
        assert!(style.contains("background: transparent"));
        assert!(!style.contains("text-decoration: underline"));
        assert!(style.contains("text-decoration: none"));
        assert!(style.contains("width: 1ch"));
        assert!(style.contains("position: relative"));
    }

    #[test]
    fn terminal_style_uses_snapshot_colors_without_second_inverse_pass() {
        let cell = SnapshotCell {
            c: 'X',
            fg: kt_core::term::color::DEFAULT_BG,
            bg: kt_core::term::color::DEFAULT_FG,
            attrs: kt_core::term::snapshot::CellAttrs {
                inverse: true,
                ..Default::default()
            },
        };

        let style = terminal_cell_style(&cell);

        assert!(style.contains("color: rgb(26, 27, 38)"));
        assert!(style.contains("background: rgb(208, 208, 208)"));
    }

    #[test]
    fn terminal_default_style_explicitly_resets_mutable_properties() {
        let cell = SnapshotCell::default();

        let style = terminal_cell_style(&cell);

        assert!(style.contains("background: transparent"));
        assert!(style.contains("font-weight: 400"));
        assert!(style.contains("font-style: normal"));
        assert!(style.contains("text-decoration: none"));
        assert!(style.contains("opacity: 1"));
    }

    #[test]
    fn terminal_text_decoration_can_reset_or_combine_flags() {
        let cell = SnapshotCell {
            c: 'X',
            attrs: kt_core::term::snapshot::CellAttrs {
                underline: true,
                strikeout: true,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(terminal_text_decoration(&cell), "underline line-through");
        assert_eq!(terminal_text_decoration(&SnapshotCell::default()), "none");
    }

    #[test]
    fn terminal_cell_text_preserves_html_sensitive_characters() {
        let cell = SnapshotCell {
            c: '<',
            ..Default::default()
        };

        assert_eq!(terminal_cell_text(&cell), "<");
    }

    #[test]
    fn cursor_class_tracks_shape_without_repainting_cell_style() {
        assert_eq!(
            terminal_cursor_class(true, CursorShape::Block),
            "terminal-cursor terminal-cursor-block"
        );
        assert_eq!(
            terminal_cursor_class(true, CursorShape::Bar),
            "terminal-cursor terminal-cursor-bar"
        );
        assert_eq!(
            terminal_cursor_class(true, CursorShape::Underline),
            "terminal-cursor terminal-cursor-underline"
        );
        assert_eq!(terminal_cursor_class(true, CursorShape::Hidden), "");
        assert_eq!(terminal_cursor_class(false, CursorShape::Block), "");
    }

    #[test]
    fn terminal_context_menu_ids_and_style_are_stable() {
        assert_eq!(
            terminal_screen_id("terminal-1-primary"),
            "terminal-1-primary-screen"
        );
        assert_eq!(
            terminal_context_menu_style(TerminalContextMenuState { x: 12.4, y: 98.6 }),
            "left: 12px; top: 99px;"
        );
    }

    #[test]
    fn terminal_paste_input_normalizes_line_endings() {
        assert_eq!(
            terminal_paste_input("one\r\ntwo\rthree"),
            b"one\ntwo\nthree"
        );
        assert!(terminal_paste_input("").is_empty());
    }

    #[test]
    fn gutter_width_and_style_track_enabled_columns() {
        assert_eq!(gutter_width_ch(false, false), 0);
        assert_eq!(gutter_width_ch(true, false), 6);
        assert_eq!(gutter_width_ch(false, true), 11);
        assert_eq!(gutter_width_ch(true, true), 17);
        assert!(gutter_surface_style(false, false).is_empty());
        assert!(gutter_surface_style(true, true).contains("17ch"));
    }

    #[test]
    fn gutter_timestamps_persist_for_unchanged_rows() {
        use kt_core::term::snapshot::{Cursor, CursorShape};
        let store = Rc::new(RefCell::new(Vec::<RowMeta>::new()));
        let mut cells = vec![SnapshotCell::default(); 2];
        cells[0].c = 'a';
        let snapshot = GridSnapshot {
            rows: 2,
            cols: 1,
            cells,
            cursor: Cursor {
                line: 1,
                column: 0,
                shape: CursorShape::Block,
            },
            revision: 1,
            display_offset: 0,
            wrapped: vec![false, false],
        };

        let first = compute_gutter_rows(&store, &snapshot, true, true);
        // 光标在底部（line 1），两行都已使用，均显示时间戳。
        assert!(!first[0].unused);
        assert!(!first[1].unused);
        assert!(!first[0].time.is_empty());
        assert!(!first[1].time.is_empty());

        // 内容未变化时，时间戳保持不变（不刷新为当前时刻）。
        let first_time = first[0].time.clone();
        let first_blank_time = first[1].time.clone();
        let second = compute_gutter_rows(&store, &snapshot, true, true);
        assert_eq!(second[0].time, first_time);
        assert_eq!(second[0].signature, first[0].signature);
        assert_eq!(second[1].time, first_blank_time);
    }

    #[test]
    fn gutter_marks_wrap_continuation_rows() {
        use kt_core::term::snapshot::{Cursor, CursorShape};
        let store = Rc::new(RefCell::new(Vec::<RowMeta>::new()));
        // 两行非空内容，第 0 行 wrap 到第 1 行；光标在底部使两行均显示。
        let mut cells = vec![SnapshotCell::default(); 2];
        cells[0].c = 'a';
        cells[1].c = 'b';
        let snapshot = GridSnapshot {
            rows: 2,
            cols: 1,
            cells,
            cursor: Cursor {
                line: 1,
                column: 0,
                shape: CursorShape::Block,
            },
            revision: 1,
            display_offset: 0,
            wrapped: vec![true, false],
        };

        let rows = compute_gutter_rows(&store, &snapshot, true, true);
        // 第 0 行是逻辑行首，第 1 行是 wrap 续行；两行均已使用。
        assert!(!rows[0].is_continuation);
        assert!(rows[1].is_continuation);
        assert!(!rows[0].unused);
        assert!(!rows[1].unused);
    }

    #[test]
    fn gutter_hides_unused_rows_below_cursor_in_live_view() {
        use kt_core::term::snapshot::{Cursor, CursorShape};
        let store = Rc::new(RefCell::new(Vec::<RowMeta>::new()));
        // 3 行，光标在 line 0（刚启动/清屏后），line 1、2 为未使用区。
        let mut cells = vec![SnapshotCell::default(); 3];
        cells[0].c = '$';
        let snapshot = GridSnapshot {
            rows: 3,
            cols: 1,
            cells,
            cursor: Cursor {
                line: 0,
                column: 0,
                shape: CursorShape::Block,
            },
            revision: 1,
            display_offset: 0,
            wrapped: vec![false, false, false],
        };

        let rows = compute_gutter_rows(&store, &snapshot, true, true);
        assert!(!rows[0].unused);
        assert!(rows[1].unused);
        assert!(rows[2].unused);
    }

    #[test]
    fn gutter_shows_all_rows_when_scrolling_history() {
        use kt_core::term::snapshot::{Cursor, CursorShape};
        let store = Rc::new(RefCell::new(Vec::<RowMeta>::new()));
        // 滚动历史视图（display_offset > 0）：即使光标在顶部，所有可见行都显示。
        let mut cells = vec![SnapshotCell::default(); 2];
        cells[0].c = 'a';
        cells[1].c = 'b';
        let snapshot = GridSnapshot {
            rows: 2,
            cols: 1,
            cells,
            cursor: Cursor {
                line: 0,
                column: 0,
                shape: CursorShape::Block,
            },
            revision: 1,
            display_offset: 5,
            wrapped: vec![false, false],
        };

        let rows = compute_gutter_rows(&store, &snapshot, true, true);
        assert!(!rows[0].unused);
        assert!(!rows[1].unused);
    }
}
