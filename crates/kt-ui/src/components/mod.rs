//! UI 组件模块

pub mod app;
pub mod app_logic;
pub mod app_runtime;
pub mod command_history;
pub mod dialog;
pub(crate) mod docker_usage;
pub mod external_edit;
pub(crate) mod fonts;
pub mod icons;
pub mod inline_editor;
pub mod main_shell;
pub(crate) mod metrics_format;
pub mod monitor;
pub mod operations;
mod password_field;
pub mod phone_shell;
pub mod qr;
pub mod scanner;
pub mod security_dialogs;
pub mod settings;
pub mod sftp;
pub mod sftp_columns;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) mod sftp_download;
pub mod sidebar;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub mod software_update;
pub mod state_controller;
pub mod terminal;
pub(crate) mod theme;
pub mod workbench;
