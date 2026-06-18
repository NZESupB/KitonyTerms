//! KitonyTerms GUI entry point.

mod app;
mod connect_dialog;
mod fonts;
mod input;
mod store;
mod terminal_view;
mod unlock_dialog;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // Logging: RUST_LOG=info cargo run -p kt-app
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,kt_app=info".into()),
        )
        .try_init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title("KitonyTerms"),
        ..Default::default()
    };

    eframe::run_native(
        "KitonyTerms",
        native_options,
        Box::new(|cc| Ok(Box::new(app::KitonyApp::new(cc)))),
    )
}
