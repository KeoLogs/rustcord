//! The async side of RustCord. Runs on its own Tokio runtime thread so the
//! UI thread never blocks on the network.

use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::api::Api;
use crate::gateway::{self, GatewayConfig, SessionEnd};
use crate::types::{ToBackend, ToUi};

pub fn spawn(
    raw_token: String,
    demo: bool,
    tx: Sender<ToUi>,
    rx: UnboundedReceiver<ToBackend>,
    egui_ctx: eframe::egui::Context,
) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        if demo {
            let ctx = egui_ctx.clone();
            rt.block_on(crate::demo::run(tx, rx, move || ctx.request_repaint()));
        } else {
            rt.block_on(run(raw_token, tx, rx, egui_ctx));
        }
    });
}

async fn run(
    raw_token: String,
    tx: Sender<ToUi>,
    mut rx: UnboundedReceiver<ToBackend>,
    egui_ctx: eframe::egui::Context,
) {
    // "Bot xxxx" -> bot account; anything else is treated as a user token.
    let (is_bot, token) = match raw_token.strip_prefix("Bot ") {
        Some(t) => (true, t.trim().to_string()),
        None => (false, raw_token.trim().to_string()),
    };
    let auth = if is_bot {
        format!("Bot {token}")
    } else {
        token.clone()
    };

    let api = match Api::new(auth) {
        Ok(api) => api,
        Err(e) => {
            let _ = tx.send(ToUi::Status(format!("HTTP client error: {e}")));
            return;
        }
    };

    // Gateway task: reconnects with backoff until a fatal error.
    {
        let tx = tx.clone();
        let ctx = egui_ctx.clone();
        tokio::spawn(async move {
            let cfg = GatewayConfig { token, is_bot };
            let repaint = || ctx.request_repaint();
            let mut backoff = 1u64;
            loop {
                match gateway::run_session(&cfg, &tx, &repaint).await {
                    Ok(SessionEnd::Fatal(reason)) => {
                        let _ = tx.send(ToUi::Fatal(reason));
                        repaint();
                        return;
                    }
                    Ok(SessionEnd::Retry) => backoff = 1,
                    Err(e) => {
                        let _ = tx.send(ToUi::Status(format!("Disconnected: {e}")));
                        repaint();
                    }
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        });
    }

    // Handle requests from the UI.
    while let Some(req) = rx.recv().await {
        let api = api.clone();
        let tx = tx.clone();
        let ctx = egui_ctx.clone();
        tokio::spawn(async move {
            let result = match req {
                ToBackend::LoadHistory { channel_id } => {
                    api.channel_history(&channel_id, 50).await.map(|messages| {
                        Some(ToUi::History {
                            channel_id,
                            messages,
                        })
                    })
                }
                ToBackend::Send {
                    channel_id,
                    content,
                    reply_to,
                    files,
                } => api
                    .send_message(&channel_id, &content, reply_to.as_deref(), &files)
                    .await
                    .map(|_| None),
                ToBackend::PickFiles => {
                    let picked = rfd::AsyncFileDialog::new()
                        .set_title("Upload files")
                        .pick_files()
                        .await
                        .unwrap_or_default();
                    let paths = picked.iter().map(|f| f.path().to_path_buf()).collect();
                    Ok(Some(ToUi::FilesPicked(paths)))
                }
                ToBackend::Edit {
                    channel_id,
                    message_id,
                    content,
                } => api
                    .edit_message(&channel_id, &message_id, &content)
                    .await
                    .map(|_| None),
                ToBackend::Delete {
                    channel_id,
                    message_id,
                } => api
                    .delete_message(&channel_id, &message_id)
                    .await
                    .map(|_| None),
                ToBackend::Typing { channel_id } => {
                    // Typing failures aren't worth bothering the user with.
                    let _ = api.trigger_typing(&channel_id).await;
                    Ok(None)
                }
            };
            match result {
                Ok(Some(ev)) => {
                    let _ = tx.send(ev);
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = tx.send(ToUi::Status(format!("Error: {e}")));
                }
            }
            ctx.request_repaint();
        });
    }
}
