//! RustCord — a native, customisable Discord client written in Rust.

mod api;
mod app;
mod autocomplete;
mod backend;
mod config;
mod demo;
mod embed;
mod emoji;
mod emoji_picker;
mod forum;
mod friends;
mod gateway;
mod gif_picker;
mod home;
mod http_error;
mod icons;
mod invites;
mod layout;
mod layout_presets;
mod layout_v3;
mod links;
mod login;
mod markdown;
mod members;
mod notify;
mod onboarding;
mod perms;
mod pins;
mod presence;
mod profile;
mod quick_switcher;
mod ratelimit;
mod reactions;
mod read_state;
mod responsive;
mod screening;
mod search;
mod settings;
mod settings_proto;
mod status;
mod storage;
mod sysmsg;
mod theme;
mod theme_file;
mod themes;
mod themes_page;
mod threads;
mod tile_fit;
mod timefmt;
mod types;
mod unicode_symbols;
mod widgets;

fn main() -> eframe::Result {
    // Two dependencies enable different rustls crypto backends (aws-lc-rs via
    // reqwest, ring via tokio-tungstenite/ehttp). With both present rustls
    // can't pick one automatically and panics on the first TLS connection,
    // so choose one explicitly before anything connects.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Optional: skip the login screen with DISCORD_TOKEN, or run offline with
    // RUSTCORD_DEMO=1.
    let demo = storage::demo_from_env();
    let env_token = std::env::var("DISCORD_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RustCord")
            // Demo runs save under their own app id (storage folder), so
            // they never read or write the real config, layout or window.
            .with_app_id(storage::app_id(demo))
            .with_inner_size([1200.0, 760.0])
            // Doc §3.2: drawers take over below 700 px.
            .with_min_inner_size(responsive::WINDOW_MIN),
        // OpenGL renderer: noticeably less RAM than wgpu for a 2D chat UI.
        renderer: eframe::Renderer::Glow,
        // Demo screenshot runs resize the window; don't let them overwrite
        // the user's saved window size or egui memory.
        persist_window: !(demo
            && (std::env::var("RUSTCORD_DEMO_OPEN").is_ok()
                || std::env::var("RUSTCORD_DEMO_SHOT").is_ok())),
        ..Default::default()
    };

    eframe::run_native(
        storage::app_id(demo),
        options,
        Box::new(move |cc| {
            // Images (avatars, server icons, attachments, emoji) load
            // straight from Discord's CDN / Twemoji.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            // Noto Sans (bundled) as the UI font; system fonts stay the
            // fallback for scripts it doesn't cover.
            theme::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::RustCordApp::new(cc, env_token, demo)))
        }),
    )
}
