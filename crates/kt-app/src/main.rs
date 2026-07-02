#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! KitonyTerms 桌面应用入口

mod icon;

use std::ffi::OsString;
use std::process::ExitCode;

use kt_ui::App;

fn main() -> ExitCode {
    init_logging();

    match AppCommand::parse(std::env::args_os().skip(1)) {
        Ok(AppCommand::Gui) => {
            run_gui();
            ExitCode::SUCCESS
        }
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

fn init_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
}

fn run_gui() {
    dioxus::LaunchBuilder::desktop()
        .with_cfg(desktop_config())
        .launch(App);
}

fn desktop_config() -> dioxus::desktop::Config {
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
        config = with_kitonyterms_desktop_menu(config);
    }
    icon::with_platform_icon_hooks(config)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn with_kitonyterms_desktop_menu(config: dioxus::desktop::Config) -> dioxus::desktop::Config {
    config.with_menu(kt_ui::components::desktop_menu::app_menu())
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn with_kitonyterms_desktop_menu(config: dioxus::desktop::Config) -> dioxus::desktop::Config {
    config
}

fn should_use_kitonyterms_desktop_menu(target_os: &str) -> bool {
    matches!(target_os, "macos" | "windows" | "linux")
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
