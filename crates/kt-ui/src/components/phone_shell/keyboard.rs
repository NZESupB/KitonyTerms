//! 手机端终端输入：软键盘桥 + 键位工具条。
//!
//! 桌面端终端是 `div[tabindex] + onkeydown`，在 WebView 里聚焦 `div` 不会唤起软键盘，
//! 因此手机端必须挂一个真实的可聚焦输入框。这里用一个视觉上不可见的 `textarea`
//! 承接软键盘，并通过常驻 eval 把事件回传 Rust：
//!
//! - `input` 事件取值后立刻清空。**不能**用 Dioxus 受控 `value`：受控值与 IME 组合
//!   过程会互相打断，中文/日文上屏会丢字。
//! - `keydown` 只拦 Enter/Backspace/方向键这类 GBoard 与 iOS 键盘会真实派发的键，
//!   字符一律走 `input`（GBoard 对字符只给 `keyCode 229`）。
//! - `visualViewport` 的高度差直接在 JS 侧写入 `--kt-keyboard-inset`，让终端在键盘
//!   弹出时收缩；终端自己的 `ResizeObserver` 会因此重算 PTY 行数，无需额外往返。
//!
//! 字节序列一律复用 [`crate::components::terminal`] 中已有且已测的映射，不在这里
//! 重新实现一套 escape 序列。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::SessionId;

use crate::components::terminal::{terminal_input_for_key_name, terminal_input_for_text};
use crate::i18n::texts;
use crate::state::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneKeyKind {
    /// 直接按 Web key 名发送字节序列。
    Send(&'static str),
    /// 粘滞 Ctrl：点亮后作用于下一个按键或软键盘字符，随后自动熄灭。
    StickyCtrl,
    /// 粘滞 Alt，语义同上。
    StickyAlt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhoneKeyDef {
    pub label: &'static str,
    pub kind: PhoneKeyKind,
}

const fn send(label: &'static str, name: &'static str) -> PhoneKeyDef {
    PhoneKeyDef {
        label,
        kind: PhoneKeyKind::Send(name),
    }
}

/// 键位条按使用频率排序，单行横向滚动，避免占用终端的竖直空间。
pub const PHONE_KEYBAR: &[PhoneKeyDef] = &[
    send("Esc", "Escape"),
    send("Tab", "Tab"),
    PhoneKeyDef {
        label: "Ctrl",
        kind: PhoneKeyKind::StickyCtrl,
    },
    PhoneKeyDef {
        label: "Alt",
        kind: PhoneKeyKind::StickyAlt,
    },
    send("↑", "ArrowUp"),
    send("↓", "ArrowDown"),
    send("←", "ArrowLeft"),
    send("→", "ArrowRight"),
    send("|", "|"),
    send("-", "-"),
    send("/", "/"),
    send("~", "~"),
    send("Home", "Home"),
    send("End", "End"),
    send("PgUp", "PageUp"),
    send("PgDn", "PageDown"),
    send("Del", "Delete"),
];

/// 软键盘承接输入框的 DOM id。按会话区分，避免会话切换时新旧节点抢焦点。
pub fn phone_keyboard_input_id(session_id: SessionId) -> String {
    format!("kt-phone-keyboard-{}", session_id.0)
}

/// 让软键盘承接输入框获得焦点，唤起系统键盘。
pub fn focus_phone_keyboard(session_id: SessionId) {
    let input_id = format!("{:?}", phone_keyboard_input_id(session_id));
    dioxus::document::eval(&format!(
        r#"document.getElementById({input_id})?.focus({{ preventScroll: true }});"#
    ));
}

/// 收起系统键盘。
pub fn blur_phone_keyboard(session_id: SessionId) {
    let input_id = format!("{:?}", phone_keyboard_input_id(session_id));
    dioxus::document::eval(&format!(r#"document.getElementById({input_id})?.blur();"#));
}

#[component]
pub fn PhoneKeyboard(session_id: SessionId, language: AppLanguage) -> Element {
    let state = crate::components::app::get_state().clone();
    let t = texts(language).phone;
    let mut ctrl_armed = use_signal(|| false);
    let mut alt_armed = use_signal(|| false);
    let input_id = phone_keyboard_input_id(session_id);

    use_effect({
        let state = state.clone();
        let input_id = input_id.clone();
        move || {
            let state = state.clone();
            let input_id = input_id.clone();
            spawn(async move {
                let mut eval = dioxus::document::eval(&keyboard_bridge_script(&input_id));
                while let Ok(payload) = eval.recv::<Vec<String>>().await {
                    let Some(data) =
                        bridge_payload_to_input(&payload, *ctrl_armed.peek(), *alt_armed.peek())
                    else {
                        continue;
                    };
                    // 粘滞修饰键是「一次性」的：无论这次是键位条按键还是软键盘字符，
                    // 消费后立刻熄灭，与物理键盘上松开 Ctrl 的语义一致。
                    if *ctrl_armed.peek() {
                        ctrl_armed.set(false);
                    }
                    if *alt_armed.peek() {
                        alt_armed.set(false);
                    }
                    send_input(&state, session_id, data);
                }
            });
        }
    });

    rsx! {
        // 视觉上不可见但必须可聚焦：`display: none` / `visibility: hidden` 都无法获得焦点。
        textarea {
            id: "{input_id}",
            class: "phone-keyboard-input",
            rows: "1",
            autocapitalize: "off",
            autocorrect: "off",
            autocomplete: "off",
            spellcheck: false,
            inputmode: "text",
            "aria-label": "{t.keyboard}",
        }

        div {
            class: "phone-keybar",

            for key in PHONE_KEYBAR {
                button {
                    key: "{key.label}",
                    class: phone_key_class(key.kind, ctrl_armed(), alt_armed()),
                    // 保持真实 textarea 的焦点，避免按钮点击收起软键盘。
                    onmousedown: move |evt| evt.prevent_default(),
                    onclick: {
                        let state = state.clone();
                        move |_| match key.kind {
                            PhoneKeyKind::StickyCtrl => {
                                let next = !ctrl_armed();
                                ctrl_armed.set(next);
                                alt_armed.set(false);
                                focus_phone_keyboard(session_id);
                            }
                            PhoneKeyKind::StickyAlt => {
                                let next = !alt_armed();
                                alt_armed.set(next);
                                ctrl_armed.set(false);
                                focus_phone_keyboard(session_id);
                            }
                            PhoneKeyKind::Send(name) => {
                                if let Some(data) = terminal_input_for_key_name(
                                    name,
                                    ctrl_armed(),
                                    alt_armed(),
                                ) {
                                    ctrl_armed.set(false);
                                    alt_armed.set(false);
                                    send_input(&state, session_id, data);
                                }
                            }
                        }
                    },
                    "{key.label}"
                }
            }
        }

        if ctrl_armed() || alt_armed() {
            div {
                class: "phone-sticky-hint",
                if ctrl_armed() { "{t.sticky_ctrl}" } else { "{t.sticky_alt}" }
            }
        }
    }
}

fn send_input(state: &Arc<Mutex<AppState>>, session_id: SessionId, data: Vec<u8>) {
    if let Ok(mut app_state) = state.lock() {
        app_state.send_terminal_input(session_id, data);
    }
}

fn phone_key_class(kind: PhoneKeyKind, ctrl_armed: bool, alt_armed: bool) -> &'static str {
    let armed = match kind {
        PhoneKeyKind::StickyCtrl => ctrl_armed,
        PhoneKeyKind::StickyAlt => alt_armed,
        PhoneKeyKind::Send(_) => false,
    };
    if armed {
        "phone-key is-armed"
    } else {
        "phone-key"
    }
}

/// 把 JS 桥的一条消息翻译成要写给 PTY 的字节。未知消息返回 `None`。
fn bridge_payload_to_input(payload: &[String], ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    let kind = payload.first()?.as_str();
    let value = payload.get(1)?;
    match kind {
        "text" => {
            let data = terminal_input_for_text(value, ctrl, alt);
            (!data.is_empty()).then_some(data)
        }
        "key" => terminal_input_for_key_name(value, ctrl, alt),
        _ => None,
    }
}

fn keyboard_bridge_script(input_id: &str) -> String {
    let input_id = format!("{input_id:?}");
    format!(
        r#"
        const element = document.getElementById({input_id});
        if (!element) {{
            return;
        }}

        const cleanupKey = "__ktPhoneKeyboardCleanup";
        if (element[cleanupKey]) {{
            element[cleanupKey]();
        }}

        // 字符输入一律走 input 事件：GBoard 对字符只派发 keyCode 229，keydown 里拿不到内容。
        // 组合（IME 上屏、拼写建议）期间不能取值：此时字段内容还在被输入法改写，
        // 边取边清会让组合状态错乱、中文丢字。等 compositionend 一次性发出。
        let composing = false;
        const flush = () => {{
            const value = element.value;
            element.value = "";
            if (value) {{
                dioxus.send(["text", value]);
            }}
        }};
        const onCompositionStart = () => {{
            composing = true;
        }};
        // compositionend 之后浏览器还会补发一次 input；那次 flush 读到的是空串，
        // 因此不会重复发送。
        const onCompositionEnd = () => {{
            composing = false;
            flush();
        }};
        const onInput = () => {{
            if (!composing) {{
                flush();
            }}
        }};

        // 只拦软键盘会真实派发的功能键，其余交给 input 事件。
        const forwarded = new Set([
            "Enter", "Backspace", "Tab", "Escape", "Delete",
            "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
            "Home", "End", "PageUp", "PageDown",
        ]);
        const onKeyDown = (event) => {{
            if (composing || !forwarded.has(event.key)) {{
                return;
            }}
            event.preventDefault();
            dioxus.send(["key", event.key]);
        }};

        // 键盘遮挡量不在这里处理：`crate::device` 的常驻 eval 是 `--kt-keyboard-inset`
        // 的唯一所有者，内嵌编辑器打开时本桥并不挂载，放在这里会漏。

        element.addEventListener("compositionstart", onCompositionStart);
        element.addEventListener("compositionend", onCompositionEnd);
        element.addEventListener("input", onInput);
        element.addEventListener("keydown", onKeyDown);

        element[cleanupKey] = () => {{
            element.removeEventListener("compositionstart", onCompositionStart);
            element.removeEventListener("compositionend", onCompositionEnd);
            element.removeEventListener("input", onInput);
            element.removeEventListener("keydown", onKeyDown);
            delete element[cleanupKey];
        }};

        await new Promise(() => {{}});
        "#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_keyboard_text_is_forwarded_verbatim_without_modifiers() {
        assert_eq!(
            bridge_payload_to_input(&["text".into(), "ls -la".into()], false, false),
            Some(b"ls -la".to_vec())
        );
        // IME 上屏的多字节字符不得被拆改。
        assert_eq!(
            bridge_payload_to_input(&["text".into(), "目录".into()], false, false),
            Some("目录".as_bytes().to_vec())
        );
    }

    #[test]
    fn armed_ctrl_turns_the_next_soft_keyboard_character_into_a_control_code() {
        assert_eq!(
            bridge_payload_to_input(&["text".into(), "c".into()], true, false),
            Some(vec![0x03])
        );
        assert_eq!(
            bridge_payload_to_input(&["key".into(), "ArrowUp".into()], false, false),
            Some(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn unknown_and_incomplete_bridge_messages_are_ignored() {
        assert_eq!(bridge_payload_to_input(&[], false, false), None);
        assert_eq!(
            bridge_payload_to_input(&["text".into()], false, false),
            None
        );
        assert_eq!(
            bridge_payload_to_input(&["text".into(), String::new()], false, false),
            None
        );
        assert_eq!(
            bridge_payload_to_input(&["wat".into(), "x".into()], false, false),
            None
        );
        // 修饰键本身不是可输入的键，不产生字节。
        assert_eq!(
            bridge_payload_to_input(&["key".into(), "Shift".into()], false, false),
            None
        );
    }

    #[test]
    fn sticky_keys_light_up_only_their_own_button() {
        assert_eq!(
            phone_key_class(PhoneKeyKind::StickyCtrl, true, false),
            "phone-key is-armed"
        );
        assert_eq!(
            phone_key_class(PhoneKeyKind::StickyAlt, true, false),
            "phone-key"
        );
        assert_eq!(
            phone_key_class(PhoneKeyKind::Send("Escape"), true, true),
            "phone-key"
        );
    }

    #[test]
    fn keybar_covers_the_keys_a_touch_keyboard_cannot_produce() {
        let labels: Vec<&str> = PHONE_KEYBAR.iter().map(|key| key.label).collect();
        for required in ["Esc", "Tab", "Ctrl", "↑", "↓", "←", "→", "|", "~"] {
            assert!(labels.contains(&required), "键位条缺少 {required}");
        }

        // 每个非粘滞键都必须能映射出真实字节，否则按下去毫无反应。
        for key in PHONE_KEYBAR {
            if let PhoneKeyKind::Send(name) = key.kind {
                assert!(
                    terminal_input_for_key_name(name, false, false).is_some(),
                    "键位 {} 映射不到终端字节",
                    key.label
                );
            }
        }
    }

    #[test]
    fn keyboard_input_ids_are_scoped_per_session() {
        assert_eq!(phone_keyboard_input_id(SessionId(3)), "kt-phone-keyboard-3");
        assert_ne!(
            phone_keyboard_input_id(SessionId(1)),
            phone_keyboard_input_id(SessionId(2))
        );
    }

    #[test]
    fn bridge_script_targets_the_session_scoped_input() {
        let script = keyboard_bridge_script("kt-phone-keyboard-7");
        assert!(script.contains(r#"getElementById("kt-phone-keyboard-7")"#));
        // 键盘遮挡量由 `crate::device` 的常驻 eval 统一负责，本桥不得再写一份，
        // 否则内嵌编辑器打开、本桥卸载时会把 inset 清成 0。
        assert!(!script.contains("setProperty"));
    }

    #[test]
    fn bridge_script_defers_to_composition_end_for_ime_input() {
        // 组合期间取值会打断输入法、造成中文丢字，必须等 compositionend。
        let script = keyboard_bridge_script("kt-phone-keyboard-1");
        assert!(script.contains("compositionstart"));
        assert!(script.contains("compositionend"));
        assert!(script.contains("if (!composing)"));
        // 每个监听都必须能被清理，避免会话切换后旧监听继续往新会话发字节。
        for event in ["compositionstart", "compositionend", "input", "keydown"] {
            assert!(
                script.contains(&format!("removeEventListener(\"{event}\"")),
                "{event} 监听没有被清理"
            );
        }
    }
}
