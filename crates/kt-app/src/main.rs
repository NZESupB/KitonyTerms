#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! KitonyTerms 桌面应用入口

mod icon;

use kt_ui::App;

fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let mut desktop_config = dioxus::desktop::Config::new().with_window(
        dioxus::desktop::WindowBuilder::new()
            .with_title("KitonyTerms")
            .with_inner_size(dioxus::desktop::LogicalSize::new(1200.0, 800.0))
            .with_resizable(true),
    );
    if let Some(window_icon) = icon::kitony_window_icon() {
        desktop_config = desktop_config.with_icon(window_icon);
    }
    desktop_config = icon::with_platform_icon_hooks(desktop_config);

    // 启动应用
    dioxus::LaunchBuilder::desktop()
        .with_cfg(desktop_config)
        .launch(App);
}
