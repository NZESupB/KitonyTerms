//! KitonyTerms 桌面应用入口

use kt_ui::App;

fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!("启动 KitonyTerms");

    // 启动应用
    dioxus::LaunchBuilder::desktop()
        .with_cfg(dioxus::desktop::Config::new().with_window(
            dioxus::desktop::WindowBuilder::new()
                .with_title("KitonyTerms")
                .with_inner_size(dioxus::desktop::LogicalSize::new(1200.0, 800.0))
                .with_resizable(true),
        ))
        .launch(App);
}
