//! 终端选区的 WebView 桥。请求顺序与核心一致，生命周期结束后禁止迟到复制。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dioxus::prelude::*;
use kt_core::term::selection::{SelectionAction, SelectionCommand, SelectionReply};
use kt_core::{ExecId, SessionId, ToCore};
use tokio::sync::oneshot;

static NEXT_BRIDGE: AtomicU64 = AtomicU64::new(1);

fn use_selection_owner(terminal_id: &str) -> String {
    let cached = use_hook(|| Rc::new(RefCell::new(None::<(String, String)>)));
    let mut cached = cached.borrow_mut();
    if let Some((target, owner)) = cached.as_ref() {
        if target == terminal_id {
            return owner.clone();
        }
    }
    let nonce = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
    let owner = format!("{terminal_id}:{nonce}");
    *cached = Some((terminal_id.to_owned(), owner.clone()));
    owner
}

pub(super) fn use_terminal_selection(
    terminal_id: &str,
    session_id: SessionId,
    exec_id: Option<ExecId>,
) -> String {
    let bridge_key = use_hook(|| NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed));
    let owner = use_selection_owner(terminal_id);
    let generation = use_hook(|| Rc::new(Cell::new(0u64)));
    let latest = use_hook(|| Rc::new(RefCell::new(None)));
    *latest.borrow_mut() = Some((session_id, exec_id, owner.clone()));
    let sender = crate::components::app::get_state()
        .lock()
        .expect("应用状态锁已损坏")
        .manager
        .raw_sender();

    use_effect(use_reactive(
        (&terminal_id.to_owned(), &owner, &session_id, &exec_id),
        {
            let generation = generation.clone();
            let sender = sender.clone();
            move |(terminal_id, owner, session_id, exec_id)| {
                let current = generation.get().wrapping_add(1);
                generation.set(current);
                let generation = generation.clone();
                let sender = sender.clone();
                let script = format!(
                    "const terminalId = {terminal_id:?}; const bridgeKey = {bridge_key};\n{}",
                    include_str!("../../assets/terminal_selection.js"),
                );
                let mut eval = dioxus::document::eval(&script);
                spawn(async move {
                    while let Ok(Some(action)) = eval.recv::<Option<SelectionAction>>().await {
                        if generation.get() != current {
                            break;
                        }
                        let (reply, receiver) = oneshot::channel();
                        let request = ToCore::TerminalSelection {
                            id: session_id,
                            exec_id,
                            command: SelectionCommand {
                                owner: owner.clone(),
                                action,
                                reply,
                            },
                        };
                        let result = tokio::time::timeout(Duration::from_secs(5), async {
                            sender
                                .send(request)
                                .await
                                .map_err(|_| "终端命令通道已关闭".to_string())?;
                            receiver
                                .await
                                .map_err(|_| "终端选区请求已取消".to_string())?
                        })
                        .await
                        .unwrap_or_else(|_| Err("终端选区请求超时".into()));
                        if generation.get() != current {
                            break;
                        }
                        let result = result.map(|mut reply: SelectionReply| {
                            if let Some(text) = reply.text.as_ref().filter(|text| !text.is_empty())
                            {
                                match crate::clipboard::write_text(text) {
                                    Ok(()) => reply.text = None,
                                    Err(error) => tracing::debug!(
                                        "原生复制不可用，交由 WebView 写入: {error}"
                                    ),
                                }
                            }
                            reply
                        });
                        if let Err(error) = &result {
                            tracing::warn!("终端选区操作失败: {error}");
                        }
                        if eval.send(result).is_err() {
                            break;
                        }
                    }
                    // 旧实例只能清理自己的选区，不会清除后来建立的其他 pane 选区。
                    let (reply, _) = oneshot::channel();
                    let _ = sender
                        .send(ToCore::TerminalSelection {
                            id: session_id,
                            exec_id,
                            command: SelectionCommand {
                                owner,
                                action: SelectionAction::Clear,
                                reply,
                            },
                        })
                        .await;
                });
            }
        },
    ));
    use_drop(move || {
        generation.set(generation.get().wrapping_add(1));
        dioxus::document::eval(&format!(
            "window.__ktTerminalSelectionBridges?.get({bridge_key})?.cleanup();"
        ));
        if let Some((id, exec_id, owner)) = latest.borrow_mut().take() {
            let (reply, _receiver) = oneshot::channel();
            if sender
                .try_send(ToCore::TerminalSelection {
                    id,
                    exec_id,
                    command: SelectionCommand {
                        owner,
                        action: SelectionAction::Clear,
                        reply,
                    },
                })
                .is_err()
            {
                tracing::debug!("终端选区清理通道已关闭或繁忙");
            }
        }
    });
    owner
}

#[cfg(test)]
mod tests {
    use super::*;

    type ProbeState = (Rc<RefCell<String>>, Rc<RefCell<String>>);

    fn owner_probe((target, result): ProbeState) -> Element {
        let target = target.borrow().clone();
        *result.borrow_mut() = use_selection_owner(&target);
        rsx! { div {} }
    }

    #[test]
    fn owner_survives_rerenders_but_changes_when_returning_to_a_previous_target() {
        let target = Rc::new(RefCell::new("terminal-a".to_string()));
        let result = Rc::new(RefCell::new(String::new()));
        let mut dom = VirtualDom::new_with_props(owner_probe, (target.clone(), result.clone()));
        dom.rebuild_in_place();
        let first = result.borrow().clone();
        let app = ScopeId::APP;
        dom.mark_dirty(app);
        let _ = dom.render_immediate_to_vec();
        assert_eq!(*result.borrow(), first);

        *target.borrow_mut() = "terminal-b".into();
        dom.mark_dirty(app);
        let _ = dom.render_immediate_to_vec();
        let second = result.borrow().clone();
        assert_ne!(second, first);
        assert!(second.starts_with("terminal-b:"));

        *target.borrow_mut() = "terminal-a".into();
        dom.mark_dirty(app);
        let _ = dom.render_immediate_to_vec();
        assert_ne!(*result.borrow(), first);
        assert!(result.borrow().starts_with("terminal-a:"));
    }
}
