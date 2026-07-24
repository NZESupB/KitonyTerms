#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! KitonyTerms 桌面与移动应用入口

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod icon;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod single_instance;

use std::ffi::OsString;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::path::Path;
use std::process::ExitCode;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use kt_config::{AppLanguage, Config, Paths};
use kt_ui::App;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use rfd::{MessageButtons, MessageDialog, MessageLevel};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::single_instance::SingleInstanceLock;

fn main() -> ExitCode {
    init_logging();

    match startup_command() {
        Ok(AppCommand::Gui) => match run_gui() {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                tracing::error!("{message}");
                show_startup_error(&message);
                ExitCode::FAILURE
            }
        },
        Ok(AppCommand::Help) => {
            attach_console_for_cli();
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Err(message) => {
            attach_console_for_cli();
            eprintln!("{message}\n\n{}", usage());
            ExitCode::from(2)
        }
    }
}

/// 解析启动命令。移动端的 Dioxus 胶水层通过 `dlsym("main")` 以无参函数指针
/// 调用入口，argc/argv 未初始化，读取 `std::env::args_os` 会触发 SIGSEGV
/// 闪退，因此移动端固定按无参数（GUI）处理。
fn startup_command() -> Result<AppCommand, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        AppCommand::parse(std::iter::empty())
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        AppCommand::parse(std::env::args_os().skip(1))
    }
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn run_gui() -> Result<(), String> {
    dioxus::LaunchBuilder::mobile().launch(App);
    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn run_gui() -> Result<(), String> {
    let paths = Paths::discover().map_err(|err| err.to_string())?;
    let Some(_instance_lock) = SingleInstanceLock::try_acquire(&paths.instance_lock_file())
        .map_err(|err| format!("single-instance lock: {err}"))?
    else {
        show_already_running();
        return Ok(());
    };
    let language = startup_language(&paths.config_file());

    dioxus::LaunchBuilder::desktop()
        .with_cfg(desktop_config(language))
        .launch(App);
    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn show_already_running() {
    let description = already_running_message(AppLanguage::system_default());
    let _ = MessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title("KitonyTerms")
        .set_description(description)
        .set_buttons(MessageButtons::Ok)
        .show();
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn already_running_message(language: AppLanguage) -> &'static str {
    match language {
        AppLanguage::Chinese => "KitonyTerms 已在运行。请先切换到现有窗口。",
        AppLanguage::English => {
            "KitonyTerms is already running. Please switch to the existing window."
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn show_startup_error(error: &str) {
    let description = match AppLanguage::system_default() {
        AppLanguage::Chinese => format!("KitonyTerms 启动失败：{error}"),
        AppLanguage::English => format!("KitonyTerms failed to start: {error}"),
    };
    let _ = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title("KitonyTerms")
        .set_description(description)
        .set_buttons(MessageButtons::Ok)
        .show();
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn show_startup_error(_error: &str) {}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn startup_language(config_file: &Path) -> AppLanguage {
    match Config::load_from(config_file) {
        Ok(config) => config.settings.language,
        Err(error) => {
            tracing::warn!(
                "读取界面语言失败，将使用系统语言: {}: {error}",
                config_file.display()
            );
            AppLanguage::system_default()
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn desktop_config(language: AppLanguage) -> dioxus::desktop::Config {
    let mut config = dioxus::desktop::Config::new().with_window(
        dioxus::desktop::WindowBuilder::new()
            .with_title("KitonyTerms")
            .with_inner_size(dioxus::desktop::LogicalSize::new(1200.0, 800.0))
            .with_resizable(true),
    );
    if let Some(window_icon) = icon::kitony_window_icon() {
        config = config.with_icon(window_icon);
    }
    if should_use_kitonyterms_desktop_menu(std::env::consts::OS) {
        config = with_kitonyterms_desktop_menu(config, language);
    }
    icon::with_platform_icon_hooks(config)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn with_kitonyterms_desktop_menu(
    config: dioxus::desktop::Config,
    language: AppLanguage,
) -> dioxus::desktop::Config {
    config.with_menu(kt_ui::components::desktop_menu::app_menu(language))
}

#[cfg(any(test, target_os = "macos", target_os = "windows", target_os = "linux"))]
fn should_use_kitonyterms_desktop_menu(target_os: &str) -> bool {
    matches!(target_os, "macos" | "windows" | "linux")
}

#[cfg(test)]
fn is_mobile_target_os(target_os: &str) -> bool {
    matches!(target_os, "android" | "ios")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    Gui,
    Help,
}

impl AppCommand {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        match args.as_slice() {
            [] => Ok(Self::Gui),
            [flag] if flag == "--gui" => Ok(Self::Gui),
            [flag] if flag == "--help" || flag == "-h" => Ok(Self::Help),
            [flag] if is_removed_entry_flag(flag) => Err(format!(
                "`{flag}` 入口当前未提供；请使用 `kitonyterms` 启动 GUI，或使用 `cargo run -p kt-core --example headless -- user@host` 调试核心 SSH 管线。"
            )),
            [flag, ..] if is_removed_entry_flag(flag) => Err(format!(
                "`{flag}` 入口当前未提供；阶段二已将入口能力收敛为 GUI-only。"
            )),
            [unknown, ..] => Err(format!("未知参数：{unknown}")),
        }
    }
}

fn is_removed_entry_flag(flag: &str) -> bool {
    matches!(flag, "--safe" | "--system-ssh" | "--show-log" | "--list")
}

fn usage() -> &'static str {
    "用法：kitonyterms [--gui]\n\n当前二进制只提供 Dioxus GUI 入口；核心 SSH 管线可通过 `cargo run -p kt-core --example headless -- user@host` 调试。"
}

#[cfg(windows)]
fn attach_console_for_cli() {
    use windows_sys::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let _ = AllocConsole();
        }
    }
}

#[cfg(not(windows))]
fn attach_console_for_cli() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<AppCommand, String> {
        AppCommand::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_args_launches_gui() {
        assert_eq!(parse(&[]).unwrap(), AppCommand::Gui);
    }

    #[test]
    fn explicit_gui_launches_gui() {
        assert_eq!(parse(&["--gui"]).unwrap(), AppCommand::Gui);
    }

    #[test]
    fn help_is_supported() {
        assert_eq!(parse(&["--help"]).unwrap(), AppCommand::Help);
        assert_eq!(parse(&["-h"]).unwrap(), AppCommand::Help);
    }

    #[test]
    fn custom_desktop_menu_is_used_on_desktop_release_platforms() {
        assert!(should_use_kitonyterms_desktop_menu("windows"));
        assert!(should_use_kitonyterms_desktop_menu("macos"));
        assert!(should_use_kitonyterms_desktop_menu("linux"));
        assert!(!should_use_kitonyterms_desktop_menu("android"));
    }

    #[test]
    fn android_and_ios_use_mobile_launcher() {
        assert!(is_mobile_target_os("android"));
        assert!(is_mobile_target_os("ios"));
        assert!(!is_mobile_target_os("macos"));
        assert!(!is_mobile_target_os("windows"));
        assert!(!is_mobile_target_os("linux"));
    }

    #[test]
    fn single_instance_notice_follows_system_language() {
        assert!(already_running_message(AppLanguage::Chinese).contains("已在运行"));
        assert!(already_running_message(AppLanguage::English).contains("already running"));
    }

    #[test]
    fn desktop_menu_language_uses_saved_setting() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("config.toml");
        let mut config = Config::default();
        config.settings.language = AppLanguage::Chinese;
        config.save_to(&config_file).unwrap();

        assert_eq!(startup_language(&config_file), AppLanguage::Chinese);
    }

    #[test]
    fn desktop_menu_language_falls_back_to_system_for_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("config.toml");
        std::fs::write(&config_file, "invalid = [").unwrap();

        assert_eq!(
            startup_language(&config_file),
            AppLanguage::system_default()
        );
    }

    #[test]
    fn removed_entry_flags_fail_clearly() {
        let error = parse(&["--safe", "demo"]).unwrap_err();
        assert!(error.contains("--safe"));
        assert!(error.contains("GUI-only"));
    }

    #[test]
    fn unknown_args_fail() {
        let error = parse(&["demo"]).unwrap_err();
        assert!(error.contains("未知参数"));
    }
}
