//! RustCord — a native, customisable Discord client written in Rust.

mod api;
mod app;
mod backend;
mod gateway;
mod types;

use std::sync::mpsc;

fn main() -> eframe::Result {
    let token = match std::env::var("DISCORD_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!(
                "DISCORD_TOKEN is not set.\n\
                 For a bot (recommended for testing):  DISCORD_TOKEN=\"Bot <token>\" cargo run\n\
                 For a user account:                   DISCORD_TOKEN=\"<token>\" cargo run"
            );
            std::process::exit(1);
        }
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RustCord")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RustCord",
        options,
        Box::new(move |cc| {
            // backend -> UI (polled every frame)
            let (ui_tx, ui_rx) = mpsc::channel();
            // UI -> backend (async)
            let (be_tx, be_rx) = tokio::sync::mpsc::unbounded_channel();

            backend::spawn(token, ui_tx, be_rx, cc.egui_ctx.clone());

            Ok(Box::new(app::RustCordApp::new(ui_rx, be_tx)))
        }),
    )
}
