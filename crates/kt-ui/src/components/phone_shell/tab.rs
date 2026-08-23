//! 手机端底部标签页定义。
//!
//! 纯逻辑，无 Dioxus 依赖，便于单测覆盖「无会话时的落位」这类边界。

use crate::i18n::PhoneText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneTab {
    Servers,
    Terminal,
    Files,
    Monitor,
}

pub const PHONE_TABS: [PhoneTab; 4] = [
    PhoneTab::Servers,
    PhoneTab::Terminal,
    PhoneTab::Files,
    PhoneTab::Monitor,
];

impl PhoneTab {
    pub fn icon(self) -> &'static str {
        match self {
            PhoneTab::Servers => "sessions",
            PhoneTab::Terminal => "connect",
            PhoneTab::Files => "folder",
            PhoneTab::Monitor => "monitor",
        }
    }

    pub fn label(self, t: &PhoneText) -> &'static str {
        match self {
            PhoneTab::Servers => t.tab_servers,
            PhoneTab::Terminal => t.tab_terminal,
            PhoneTab::Files => t.tab_files,
            PhoneTab::Monitor => t.tab_monitor,
        }
    }

    /// 终端、文件与监控都以「当前会话」为前提，没有会话时无内容可渲染。
    pub fn requires_session(self) -> bool {
        !matches!(self, PhoneTab::Servers)
    }
}

/// 没有会话时把需要会话的标签落回服务器页，避免用户停在三个空白页上。
pub fn resolved_tab(tab: PhoneTab, has_session: bool) -> PhoneTab {
    if tab.requires_session() && !has_session {
        PhoneTab::Servers
    } else {
        tab
    }
}

/// 新建连接后应当直接进入终端，而不是让用户自己再点一次底部标签。
pub fn tab_after_connect() -> PhoneTab {
    PhoneTab::Terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_backed_tabs_fall_back_to_the_server_list_when_nothing_is_connected() {
        for tab in PHONE_TABS {
            assert_eq!(resolved_tab(tab, false), PhoneTab::Servers);
        }
    }

    #[test]
    fn every_tab_is_reachable_once_a_session_exists() {
        for tab in PHONE_TABS {
            assert_eq!(resolved_tab(tab, true), tab);
        }
    }

    #[test]
    fn only_the_server_list_works_without_a_session() {
        assert!(!PhoneTab::Servers.requires_session());
        assert!(PhoneTab::Terminal.requires_session());
        assert!(PhoneTab::Files.requires_session());
        assert!(PhoneTab::Monitor.requires_session());
    }

    #[test]
    fn connecting_lands_the_user_on_the_terminal() {
        assert_eq!(tab_after_connect(), PhoneTab::Terminal);
    }

    #[test]
    fn tabs_have_distinct_icons_and_labels() {
        let t = crate::i18n::texts(kt_config::AppLanguage::Chinese).phone;
        let mut icons: Vec<&str> = PHONE_TABS.iter().map(|tab| tab.icon()).collect();
        let mut labels: Vec<&str> = PHONE_TABS.iter().map(|tab| tab.label(&t)).collect();
        icons.sort_unstable();
        icons.dedup();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(icons.len(), PHONE_TABS.len());
        assert_eq!(labels.len(), PHONE_TABS.len());
    }
}
