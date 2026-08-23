//! 手机端底部动作面板。
//!
//! 触屏没有右键，桌面 `ContextMenu` 的全部入口在手机上改由这里承接：条目行尾的
//! `⋮` 按钮或顶栏 `⋮` 打开一张底部面板，动作以大号可点区域纵向排列。

use dioxus::prelude::*;
use kt_config::{AppLanguage, SessionProfile};
use kt_core::SessionId;

use crate::components::app_logic::{SessionTabView, DEFAULT_GROUP_NAME};
use crate::components::icons::Icon;
use crate::components::sidebar::SftpEntryContext;
use crate::i18n::texts;

/// 当前打开的动作面板。`None` 表示没有面板。
#[derive(Clone, Debug, PartialEq)]
pub enum PhoneSheet {
    /// 服务器卡片的动作。
    Profile(SessionProfile),
    /// 分组标题的动作。
    Group(String),
    /// SFTP 条目的动作。
    SftpEntry(SftpEntryContext),
    /// 终端视图顶栏的动作。
    Terminal(SessionId),
    /// 会话切换器。
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetActionId {
    Connect,
    Edit,
    Duplicate,
    Delete,
    GroupRename,
    GroupDelete,
    Open,
    InlineEdit,
    Rename,
    CopyPath,
    CopyName,
    Copy,
    Paste,
    SelectAll,
    SyncToTerminal,
    Reconnect,
    Disconnect,
    NewConnection,
    SelectSession(SessionId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetAction {
    pub id: SheetActionId,
    pub icon: &'static str,
    pub label: String,
    /// 破坏性动作，用红色强调。
    pub danger: bool,
    /// 当前状态下不可用（如未连接时不能同步目录）。
    pub disabled: bool,
}

impl SheetAction {
    fn new(id: SheetActionId, icon: &'static str, label: &str) -> Self {
        SheetAction {
            id,
            icon,
            label: label.to_string(),
            danger: false,
            disabled: false,
        }
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn disabled_when(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl SheetActionId {
    fn key(self) -> String {
        format!("{self:?}")
    }
}

/// 面板标题与动作列表。纯逻辑，便于单测覆盖「手机端不提供外部编辑」这类约束。
///
/// `sessions` 只在 [`PhoneSheet::Sessions`] 下使用；`connected` 表示当前会话是否
/// 已连接，用于禁用需要活动连接的动作。
pub fn phone_sheet_actions(
    sheet: &PhoneSheet,
    sessions: &[SessionTabView],
    connected: bool,
    language: AppLanguage,
) -> (String, Vec<SheetAction>) {
    let t = texts(language);
    match sheet {
        PhoneSheet::Profile(profile) => (
            profile.name.clone(),
            vec![
                SheetAction::new(SheetActionId::Connect, "connect", t.app.connect),
                SheetAction::new(SheetActionId::Edit, "edit", t.app.edit),
                SheetAction::new(SheetActionId::Duplicate, "file", t.app.copy),
                SheetAction::new(SheetActionId::Delete, "trash", t.app.delete).danger(),
            ],
        ),
        PhoneSheet::Group(name) => {
            let mut actions = vec![SheetAction::new(
                SheetActionId::GroupRename,
                "edit",
                t.app.rename_group,
            )];
            if name != DEFAULT_GROUP_NAME {
                actions.push(
                    SheetAction::new(SheetActionId::GroupDelete, "trash", t.app.delete_group)
                        .danger(),
                );
            }
            (name.clone(), actions)
        }
        PhoneSheet::SftpEntry(ctx) => {
            let mut actions = Vec::new();
            // 目录才有「打开」；文件走内嵌编辑器。手机上没有可用的外部编辑器链路
            // （`open_with_system_default` 在 Android/iOS 会调用 `xdg-open`），
            // 因此只提供内嵌编辑，不提供外部编辑与打开方式。
            if ctx.entry.is_dir {
                actions.push(SheetAction::new(SheetActionId::Open, "folder", t.sftp.open));
            } else {
                actions.push(SheetAction::new(
                    SheetActionId::InlineEdit,
                    "edit",
                    t.sftp.edit_inline,
                ));
            }
            actions.push(SheetAction::new(
                SheetActionId::Rename,
                "edit",
                t.sftp.rename,
            ));
            actions.push(SheetAction::new(
                SheetActionId::CopyPath,
                "file",
                t.sftp.copy_path,
            ));
            actions.push(SheetAction::new(
                SheetActionId::CopyName,
                "file",
                t.sftp.copy_name,
            ));
            actions.push(SheetAction::new(SheetActionId::Delete, "trash", t.sftp.delete).danger());
            (ctx.entry.name.clone(), actions)
        }
        PhoneSheet::Terminal(_) => (
            t.phone.tab_terminal.to_string(),
            vec![
                SheetAction::new(SheetActionId::Copy, "file", t.app.copy),
                SheetAction::new(SheetActionId::Paste, "download", t.app.paste),
                SheetAction::new(SheetActionId::SelectAll, "list", t.app.select_all),
                SheetAction::new(
                    SheetActionId::SyncToTerminal,
                    "split-vertical",
                    t.sftp.sync_to_terminal,
                )
                .disabled_when(!connected),
                SheetAction::new(SheetActionId::Reconnect, "refresh", t.app.reconnect)
                    .disabled_when(connected),
                SheetAction::new(SheetActionId::Disconnect, "close", t.phone.disconnect).danger(),
            ],
        ),
        PhoneSheet::Sessions => {
            let mut actions: Vec<SheetAction> = sessions
                .iter()
                .map(|session| {
                    SheetAction::new(
                        SheetActionId::SelectSession(session.id),
                        "connect",
                        &session.title,
                    )
                })
                .collect();
            actions.push(SheetAction::new(
                SheetActionId::NewConnection,
                "add",
                t.app.new_connection,
            ));
            (t.phone.switch_session.to_string(), actions)
        }
    }
}

#[component]
pub fn PhoneActionSheet(
    title: String,
    actions: Vec<SheetAction>,
    on_action: EventHandler<SheetActionId>,
    on_dismiss: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "phone-sheet-overlay",
            onclick: move |_| on_dismiss.call(()),

            section {
                class: "phone-sheet",
                onclick: move |evt| evt.stop_propagation(),

                div { class: "phone-sheet-grip" }
                header { class: "phone-sheet-title", "{title}" }

                for action in actions {
                    button {
                        key: "{action.id.key()}",
                        class: if action.danger { "phone-sheet-action is-danger" } else { "phone-sheet-action" },
                        disabled: action.disabled,
                        onclick: {
                            let id = action.id;
                            move |_| on_action.call(id)
                        },
                        Icon { name: action.icon }
                        span { "{action.label}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::ConnectParams;
    use kt_core::SftpEntry;

    fn entry(name: &str, is_dir: bool) -> SftpEntry {
        SftpEntry {
            name: name.to_string(),
            is_dir,
            size: 0,
            modified: None,
            permissions: None,
            user: None,
            group: None,
            uid: None,
            gid: None,
        }
    }

    fn sftp_sheet(is_dir: bool) -> PhoneSheet {
        PhoneSheet::SftpEntry(SftpEntryContext {
            session_id: SessionId(1),
            base_path: "/var".to_string(),
            entry: entry("log", is_dir),
        })
    }

    #[test]
    fn files_do_not_offer_external_editing_on_phones() {
        // Android/iOS 没有可用的「外部编辑器 + 回传」链路，动作面板不得暴露该入口；
        // 文件的编辑动作只能是内嵌编辑器。
        let (title, actions) =
            phone_sheet_actions(&sftp_sheet(false), &[], true, AppLanguage::Chinese);
        assert_eq!(title, "log");
        let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
        let t = texts(AppLanguage::Chinese).sftp;
        assert!(!labels.contains(&t.edit_external));
        assert!(!labels.contains(&t.open_with));
        assert!(actions.iter().any(|a| a.id == SheetActionId::InlineEdit));
        // 文件没有「打开」，目录才有。
        assert!(!actions.iter().any(|a| a.id == SheetActionId::Open));
    }

    #[test]
    fn directories_can_be_opened_and_never_offer_editing() {
        let (_, actions) = phone_sheet_actions(&sftp_sheet(true), &[], true, AppLanguage::Chinese);
        assert!(actions.iter().any(|a| a.id == SheetActionId::Open));
        assert!(!actions.iter().any(|a| a.id == SheetActionId::InlineEdit));
    }

    #[test]
    fn deletion_is_always_marked_destructive() {
        for sheet in [
            sftp_sheet(false),
            PhoneSheet::Group("Web".to_string()),
            PhoneSheet::Profile(SessionProfile {
                name: "prod".to_string(),
                group: None,
                params: ConnectParams::new("10.0.0.1", "root"),
            }),
        ] {
            let (_, actions) = phone_sheet_actions(&sheet, &[], true, AppLanguage::Chinese);
            let destructive = actions
                .iter()
                .find(|a| matches!(a.id, SheetActionId::Delete | SheetActionId::GroupDelete))
                .expect("每张面板都应有删除动作");
            assert!(destructive.danger);
        }
    }

    #[test]
    fn default_group_does_not_offer_delete() {
        let (_, actions) = phone_sheet_actions(
            &PhoneSheet::Group(DEFAULT_GROUP_NAME.to_string()),
            &[],
            true,
            AppLanguage::Chinese,
        );
        assert!(!actions
            .iter()
            .any(|action| action.id == SheetActionId::GroupDelete));

        let (_, actions) = phone_sheet_actions(
            &PhoneSheet::Group("Web".to_string()),
            &[],
            true,
            AppLanguage::Chinese,
        );
        assert!(actions
            .iter()
            .any(|action| action.id == SheetActionId::GroupDelete));
    }

    #[test]
    fn terminal_sheet_disables_actions_that_need_a_live_connection() {
        let connected = phone_sheet_actions(
            &PhoneSheet::Terminal(SessionId(1)),
            &[],
            true,
            AppLanguage::English,
        )
        .1;
        let sync = connected
            .iter()
            .find(|a| a.id == SheetActionId::SyncToTerminal)
            .unwrap();
        let reconnect = connected
            .iter()
            .find(|a| a.id == SheetActionId::Reconnect)
            .unwrap();
        assert!(!sync.disabled);
        // 已连接时不该提供重连。
        assert!(reconnect.disabled);

        let offline = phone_sheet_actions(
            &PhoneSheet::Terminal(SessionId(1)),
            &[],
            false,
            AppLanguage::English,
        )
        .1;
        assert!(
            offline
                .iter()
                .find(|a| a.id == SheetActionId::SyncToTerminal)
                .unwrap()
                .disabled
        );
        assert!(
            !offline
                .iter()
                .find(|a| a.id == SheetActionId::Reconnect)
                .unwrap()
                .disabled
        );
    }

    #[test]
    fn session_picker_lists_every_session_and_keeps_a_new_connection_entry() {
        use crate::components::app_logic::SessionConnectionStatus;
        let sessions = vec![
            SessionTabView {
                id: SessionId(1),
                title: "prod".to_string(),
                status: SessionConnectionStatus::Connected,
            },
            SessionTabView {
                id: SessionId(2),
                title: "staging".to_string(),
                status: SessionConnectionStatus::Connecting,
            },
        ];
        let (_, actions) =
            phone_sheet_actions(&PhoneSheet::Sessions, &sessions, true, AppLanguage::Chinese);

        assert_eq!(actions[0].id, SheetActionId::SelectSession(SessionId(1)));
        assert_eq!(actions[0].label, "prod");
        assert_eq!(actions[1].id, SheetActionId::SelectSession(SessionId(2)));
        assert_eq!(actions.last().unwrap().id, SheetActionId::NewConnection);
    }

    #[test]
    fn session_action_keys_include_the_session_identity() {
        assert_ne!(
            SheetActionId::SelectSession(SessionId(1)).key(),
            SheetActionId::SelectSession(SessionId(2)).key()
        );
        assert_ne!(SheetActionId::Connect.key(), SheetActionId::Edit.key());
    }
}
