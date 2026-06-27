//! UI 多语言文案集中入口。

use kt_config::AppLanguage;

mod en;
mod zh_cn;

pub fn texts(language: AppLanguage) -> &'static Texts {
    match language {
        AppLanguage::Chinese => &zh_cn::TEXTS,
        AppLanguage::English => &en::TEXTS,
    }
}

pub fn sftp_timeout_message(language: AppLanguage, path: &str, seconds: u64) -> String {
    match language {
        AppLanguage::Chinese => zh_cn::sftp_timeout_message(path, seconds),
        AppLanguage::English => en::sftp_timeout_message(path, seconds),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    pub app: AppText,
    pub dialog: DialogText,
    pub sftp: SftpText,
    pub monitor: MonitorText,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AppText {
    pub connect: &'static str,
    pub sessions: &'static str,
    pub monitor: &'static str,
    pub settings: &'static str,
    pub no_session: &'static str,
    pub search: &'static str,
    pub notifications: &'static str,
    pub account: &'static str,
    pub product_subtitle: &'static str,
    pub explorer: &'static str,
    pub new_connection: &'static str,
    pub search_hosts: &'static str,
    pub my_connections: &'static str,
    pub more: &'static str,
    pub no_saved_connections: &'static str,
    pub saved_connections_hint: &'static str,
    pub connection_settings: &'static str,
    pub resize_explorer: &'static str,
    pub close_session: &'static str,
    pub disconnected: &'static str,
    pub split: &'static str,
    pub split_horizontal: &'static str,
    pub split_vertical: &'static str,
    pub clear: &'static str,
    pub resize_sftp: &'static str,
    pub connected: &'static str,
    pub connecting: &'static str,
    pub ssh_connection: &'static str,
    pub ready: &'static str,
    pub ready_hint: &'static str,
    pub language: &'static str,
    pub language_hint: &'static str,
    pub chinese: &'static str,
    pub english: &'static str,
    pub close: &'static str,
    pub terminal_waiting: &'static str,
    pub terminal_connecting: &'static str,
    pub session_label: &'static str,
    pub empty_title: &'static str,
    pub empty_hint: &'static str,
    pub system_monitor: &'static str,
    pub memory: &'static str,
    pub load: &'static str,
    pub network: &'static str,
    pub filter: &'static str,
    pub edit: &'static str,
    pub delete: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DialogText {
    pub new_title: &'static str,
    pub edit_title: &'static str,
    pub name: &'static str,
    pub name_placeholder: &'static str,
    pub host: &'static str,
    pub host_placeholder: &'static str,
    pub port: &'static str,
    pub user: &'static str,
    pub password: &'static str,
    pub password_placeholder: &'static str,
    pub cancel: &'static str,
    pub save: &'static str,
    pub required_warning: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SftpText {
    pub close: &'static str,
    pub back: &'static str,
    pub refresh: &'static str,
    pub more: &'static str,
    pub name: &'static str,
    pub size: &'static str,
    pub modified: &'static str,
    pub loading: &'static str,
    pub error_prefix: &'static str,
    pub items: &'static str,
    pub upload: &'static str,
    pub download: &'static str,
    pub list_view: &'static str,
    pub delete: &'static str,
    pub session_missing: &'static str,
    pub state_unavailable: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MonitorText {
    pub system_monitor: &'static str,
    pub close: &'static str,
    pub cpu_cores: &'static str,
    pub memory: &'static str,
    pub load: &'static str,
    pub network: &'static str,
    pub waiting: &'static str,
    pub trend: &'static str,
}
