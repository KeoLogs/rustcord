//! RustCord — a native, customisable Discord client written in Rust.

mod api;
mod app;
mod backend;
mod demo;
mod gateway;
mod layout;
mod types;

use std::sync::mpsc;

fn main() -> eframe::Result {
    let demo = std::env::var("RUSTCORD_DEMO").is_ok_and(|v| v == "1");
    let token = match std::env::var("DISCORD_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ if demo => String::new(),
        _ => {
            eprintln!(
                "DISCORD_TOKEN is not set.\n\
                 For a bot (recommended for testing):  DISCORD_TOKEN=\"Bot <token>\" cargo run\n\
                 For a user account:                   DISCORD_TOKEN=\"<token>\" cargo run\n\
                 Offline demo (no token):              RUSTCORD_DEMO=1 cargo run"
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

            // Images (avatars, server icons, attachments) load straight from
            // Discord's CDN.
            egui_extras::install_image_loaders(&cc.egui_ctx);

            backend::spawn(token, demo, ui_tx, be_rx, cc.egui_ctx.clone());

            Ok(Box::new(app::RustCordApp::new(cc, ui_rx, be_tx)))
        }),
    )
}
