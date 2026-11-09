//! The async side of RustCord. Runs on its own Tokio runtime thread so the
//! UI thread never blocks on the network.

use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::api::Api;
use crate::gateway::{self, GatewayConfig, Session, SessionEnd};
use crate::members;
use crate::profile::Profile;
use crate::types::{ToBackend, ToUi};

/// Options read from the config at login.
#[derive(Clone, Copy, Default)]
pub struct Options {
    /// Bots: ask for the Server Members + Presence intents.
    pub bot_member_intents: bool,
}

pub fn spawn(
    raw_token: String,
    demo: bool,
    options: Options,
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
            rt.block_on(run(raw_token, options, tx, rx, egui_ctx));
        }
    });
}

async fn run(
    raw_token: String,
    options: Options,
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

    // Ops for the gateway (op 14 / op 8) from UI requests.
    let (gw_tx, mut gw_rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();

    // Gateway task: reconnects with backoff until a fatal error.
    {
        let tx = tx.clone();
        let ctx = egui_ctx.clone();
        tokio::spawn(async move {
            let cfg = GatewayConfig {
                token,
                is_bot,
                member_intents: is_bot && options.bot_member_intents,
            };
            let repaint = || ctx.request_repaint();
            // Kept across connections so we can RESUME instead of re-identifying.
            let mut session = Session::default();
            let mut backoff = 1u64;
            loop {
                match gateway::run_session(&cfg, &mut session, &tx, &mut gw_rx, &repaint).await {
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
        // Gateway ops are just forwarded (sent once the session is ready).
        let op = match &req {
            // Op 14 is for user accounts only.
            ToBackend::SubscribeMembers { .. } if is_bot => continue,
            ToBackend::SubscribeMembers {
                guild_id,
                channel_id,
                ranges,
            } => Some(members::subscribe_op(guild_id, channel_id, ranges)),
            ToBackend::RequestMembers { guild_id, user_ids } => {
                Some(members::request_members_op(guild_id, user_ids, is_bot))
            }
            ToBackend::RequestMemberList { guild_id } => {
                if !(is_bot && options.bot_member_intents) {
                    continue;
                }
                Some(members::request_member_list_op(guild_id))
            }
            _ => None,
        };
        if let Some(op) = op {
            let _ = gw_tx.send(op);
            continue;
        }
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
                // Bots have no read state on Discord's side: keep it local.
                ToBackend::Ack { .. } | ToBackend::MarkUnread { .. } | ToBackend::AckBulk(_)
                    if is_bot =>
                {
                    Ok(None)
                }
                ToBackend::Ack {
                    channel_id,
                    message_id,
                } => api.ack(&channel_id, &message_id).await.map(|_| None),
                ToBackend::MarkUnread {
                    channel_id,
                    message_id,
                    mention_count,
                } => api
                    .mark_unread(&channel_id, &message_id, mention_count)
                    .await
                    .map(|_| None),
                ToBackend::AckBulk(acks) => {
                    let mut result = Ok(None);
                    for chunk in acks.chunks(100) {
                        if let Err(e) = api.ack_bulk(chunk).await {
                            result = Err(e);
                            break;
                        }
                    }
                    result
                }
                ToBackend::SubscribeMembers { .. }
                | ToBackend::RequestMembers { .. }
                | ToBackend::RequestMemberList { .. } => Ok(None),
                // Profiles and notes are user-account endpoints.
                ToBackend::LoadProfile { user_id, .. } if is_bot => {
                    Ok(Some(ToUi::ProfileUnavailable(user_id)))
                }
                ToBackend::LoadNote { .. } | ToBackend::SetNote { .. } if is_bot => Ok(None),
                ToBackend::LoadProfile { user_id, guild_id } => {
                    // Failures just mean "show what we have".
                    match api.user_profile(&user_id, guild_id.as_deref()).await {
                        Ok(v) => Ok(Some(ToUi::Profile(Box::new(Profile::from_json(
                            &v, guild_id,
                        ))))),
                        Err(_) => Ok(Some(ToUi::ProfileUnavailable(user_id))),
                    }
                }
                ToBackend::LoadNote { user_id } => {
                    let note = api.note(&user_id).await.unwrap_or_default();
                    Ok(Some(ToUi::Note { user_id, note }))
                }
                ToBackend::SetNote { user_id, note } => api
                    .set_note(&user_id, &note)
                    .await
                    .map(|_| Some(ToUi::Note { user_id, note })),
                ToBackend::Notify { title, body } => {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::notify::show_desktop(&title, &body)
                    })
                    .await;
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
