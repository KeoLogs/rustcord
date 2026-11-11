//! The async side of RustCord. Runs on its own Tokio runtime thread so the
//! UI thread never blocks on the network.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::api::Api;
use crate::gateway::{self, GatewayConfig, Session, SessionEnd};
use crate::members;
use crate::profile::Profile;
use crate::types::{ToBackend, ToUi};

/// "Save image": native save dialog, then download into the chosen file.
async fn save_file(api: &Api, url: &str, filename: &str) -> anyhow::Result<Option<ToUi>> {
    let Some(target) = rfd::AsyncFileDialog::new()
        .set_title("Save image")
        .set_file_name(filename)
        .save_file()
        .await
    else {
        return Ok(None); // Cancelled.
    };
    let bytes = api.download(url).await?;
    tokio::fs::write(target.path(), &bytes).await?;
    Ok(Some(ToUi::Notice(format!(
        "Saved {}",
        target.path().display()
    ))))
}

// --- threads ---
/// A 403 / 404 (typed HTTP status): the endpoint isn't available to this
/// account here.
fn not_available(e: &anyhow::Error) -> bool {
    crate::http_error::is_unavailable(e)
}

/// Run one thread / forum request. Writes come only from clicks and are
/// never retried; a 429 also sends `ToUi::RateLimited`.
async fn thread_request(
    api: &Api,
    tx: &Sender<ToUi>,
    action: crate::threads::ThreadAction,
) -> Option<ToUi> {
    use crate::threads::ThreadAction as A;
    let fail = |e: anyhow::Error| {
        if let Some(rl) = crate::ratelimit::rate_limited(&e) {
            let _ = tx.send(ToUi::RateLimited(rl));
        }
        ToUi::ThreadFailed {
            error: crate::ratelimit::user_message(&e),
        }
    };
    Some(match action {
        // Navigation only; handled in the UI.
        A::OpenThread { .. } => return None,
        A::JoinThread { thread_id } => match api.join_thread(&thread_id).await {
            Ok(()) => ToUi::ThreadJoined {
                thread_id,
                joined: true,
            },
            Err(e) => fail(e),
        },
        A::LeaveThread { thread_id } => match api.leave_thread(&thread_id).await {
            Ok(()) => ToUi::ThreadJoined {
                thread_id,
                joined: false,
            },
            Err(e) => fail(e),
        },
        A::CreateThread {
            channel_id,
            message_id,
            name,
            private,
            auto_archive_minutes,
        } => {
            let r = match message_id {
                Some(m) => {
                    api.create_thread_from_message(&channel_id, &m, &name, auto_archive_minutes)
                        .await
                }
                None => {
                    api.create_thread(&channel_id, &name, private, auto_archive_minutes)
                        .await
                }
            };
            r.map_or_else(fail, ToUi::ThreadCreated)
        }
        A::CreatePost {
            forum_id,
            name,
            content,
            applied_tags,
        } => api
            .create_forum_post(&forum_id, &name, &content, &applied_tags)
            .await
            .map_or_else(fail, ToUi::ThreadCreated),
        A::LoadMore { forum_id, query } => match api.forum_threads(&forum_id, &query).await {
            Ok(page) => ToUi::ForumPage {
                forum_id,
                query,
                page,
            },
            Err(e) => ToUi::ForumPageFailed {
                forum_id,
                error: if not_available(&e) {
                    "Not available here".into()
                } else {
                    crate::ratelimit::user_message(&e)
                },
            },
        },
        A::LoadArchived { channel_id, before } => {
            match api
                .archived_public_threads(&channel_id, before.as_deref(), 25)
                .await
            {
                Ok(page) => ToUi::ArchivedThreads { channel_id, page },
                Err(e) if not_available(&e) => ToUi::ThreadFailed {
                    error: "Archived threads: not available here".into(),
                },
                Err(e) => fail(e),
            }
        }
    })
}

/// Options read from the config at login.
#[derive(Clone, Default)]
pub struct Options {
    /// Developer options: custom API base URL (`None` = Discord's).
    pub api_base: Option<String>,
    /// Bots: ask for the Server Members + Presence intents.
    pub bot_member_intents: bool,
    /// "Show as" (IDENTIFY properties; user accounts).
    pub identity: crate::config::ClientIdentity,
}

/// Start a backend thread for one session. `viewport` is the window that
/// shows it (the main window, or an account window): events repaint only
/// that window. The thread ends when the UI drops its `ToBackend` sender.
pub fn spawn(
    raw_token: String,
    demo: bool,
    options: Options,
    tx: Sender<ToUi>,
    rx: UnboundedReceiver<ToBackend>,
    egui_ctx: eframe::egui::Context,
    viewport: eframe::egui::ViewportId,
) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        let ctx = Repaint {
            ctx: egui_ctx,
            viewport,
        };
        if demo {
            rt.block_on(crate::demo::run(tx, rx, move || ctx.request_repaint()));
        } else {
            rt.block_on(run(raw_token, options, tx, rx, ctx));
        }
    });
}

/// Repaints the window a session belongs to (from any thread).
#[derive(Clone)]
struct Repaint {
    ctx: eframe::egui::Context,
    viewport: eframe::egui::ViewportId,
}

impl Repaint {
    fn request_repaint(&self) {
        self.ctx.request_repaint_of(self.viewport);
    }
}

async fn run(
    raw_token: String,
    options: Options,
    tx: Sender<ToUi>,
    mut rx: UnboundedReceiver<ToBackend>,
    egui_ctx: Repaint,
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
        Ok(api) => api.with_base(options.api_base.clone()),
        Err(e) => {
            let _ = tx.send(ToUi::Status(format!("HTTP client error: {e}")));
            return;
        }
    };

    // Ops for the gateway (op 14 / op 8) from UI requests.
    let (gw_tx, mut gw_rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();

    // "Reconnect" in the connection status wakes the gateway task.
    let reconnect = std::sync::Arc::new(tokio::sync::Notify::new());

    // Gateway task: reconnects with backoff until a fatal error, or until
    // the UI goes away (then it closes the connection cleanly).
    let gateway = {
        let tx = tx.clone();
        let reconnect = reconnect.clone();
        let ctx = egui_ctx.clone();
        tokio::spawn(async move {
            let cfg = GatewayConfig {
                token,
                is_bot,
                member_intents: is_bot && options.bot_member_intents,
                identity: options.identity,
            };
            let repaint = || ctx.request_repaint();
            // Kept across connections so we can RESUME instead of re-identifying.
            let mut session = Session::default();
            let mut backoff = 1u64;
            loop {
                // A manual Reconnect drops this connection (no close frame,
                // so the session can still be resumed).
                let ended = tokio::select! {
                    r = gateway::run_session(&cfg, &mut session, &tx, &mut gw_rx, &repaint) => r,
                    _ = reconnect.notified() => {
                        backoff = 0;
                        Ok(SessionEnd::Retry)
                    }
                };
                match ended {
                    Ok(SessionEnd::Fatal(reason)) => {
                        let _ = tx.send(ToUi::Fatal(reason));
                        repaint();
                        return;
                    }
                    Ok(SessionEnd::Shutdown) => return,
                    Ok(SessionEnd::Retry) => backoff = backoff.min(1),
                    Err(e) => {
                        let _ = tx.send(ToUi::Status(format!("Disconnected: {e}")));
                        repaint();
                    }
                }
                // Stop waiting if the UI goes away meanwhile.
                if gw_rx.is_closed() {
                    return;
                }
                // Reconnect cuts the wait short.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    _ = reconnect.notified() => {}
                }
                backoff = (backoff * 2).clamp(1, 60);
            }
        })
    };

    // Uploads in flight, by nonce: "Cancel" drops the request.
    let cancels = Cancels::default();

    // Handle requests from the UI.
    while let Some(req) = rx.recv().await {
        if let ToBackend::CancelSend { nonce } = &req {
            if let Some(cancel) = lock(&cancels).remove(nonce) {
                let _ = cancel.send(());
            }
            continue;
        }
        // RUSTCORD_TEST_ACCOUNT (memprobe live runs): reads only.
        if crate::config::live::on(&crate::config::live::READ_ONLY) && !read_only_ok(&req) {
            eprintln!("read-only test run: dropped a write request");
            continue;
        }
        // Gateway ops are just forwarded (sent once the session is ready).
        if matches!(req, ToBackend::Reconnect) {
            reconnect.notify_one();
            continue;
        }
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
            // Op 3: show the status the user just picked straight away.
            ToBackend::SetPresence { status, custom } => {
                Some(crate::status::presence_op(status, custom.as_ref()))
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
        let cancels = cancels.clone();
        tokio::spawn(async move {
            let result = match req {
                ToBackend::LoadHistory { channel_id } => {
                    match api.channel_history(&channel_id, 50).await {
                        Ok(messages) => Ok(Some(ToUi::History {
                            channel_id,
                            messages,
                        })),
                        // A rate limit takes the usual RateLimited path.
                        Err(e) if crate::ratelimit::rate_limited(&e).is_some() => Err(e),
                        Err(e) => Ok(Some(ToUi::HistoryFailed {
                            no_access: crate::http_error::is_no_access(&e),
                            error: crate::ratelimit::user_message(&e),
                            channel_id,
                        })),
                    }
                }
                ToBackend::LoadPage { channel_id, before } => {
                    match api
                        .history_before(&channel_id, before.as_deref(), PAGE_LIMIT)
                        .await
                    {
                        Ok(messages) => Ok(Some(ToUi::Page {
                            channel_id,
                            before,
                            messages,
                        })),
                        Err(e) => {
                            if let Some(rl) = crate::ratelimit::rate_limited(&e) {
                                let _ = tx.send(ToUi::RateLimited(rl));
                            }
                            Ok(Some(ToUi::PageFailed {
                                channel_id,
                                error: crate::ratelimit::user_message(&e),
                            }))
                        }
                    }
                }
                // Handled above (never reaches here).
                ToBackend::Reconnect => Ok(None),
                ToBackend::CopyImage { url } => {
                    // Discord's CDN only: the token never goes anywhere.
                    match api.download(&url).await {
                        Ok(bytes) => {
                            let decoded = tokio::task::spawn_blocking(move || decode_image(&bytes))
                                .await
                                .ok()
                                .flatten();
                            Ok(Some(match decoded {
                                Some(img) => ToUi::CopyImage(Box::new(img)),
                                None => ToUi::Notice("Couldn't copy that image".into()),
                            }))
                        }
                        Err(e) => Ok(Some(ToUi::Notice(format!(
                            "Couldn't copy the image: {}",
                            crate::ratelimit::user_message(&e)
                        )))),
                    }
                }
                ToBackend::Send {
                    channel_id,
                    nonce,
                    content,
                    reply_to,
                    reply_ping,
                    files,
                    spoilers,
                } => match send_cancellable(
                    &cancels,
                    &nonce,
                    !files.is_empty(),
                    api.send_message(
                        &channel_id,
                        &nonce,
                        &content,
                        reply_to.as_deref(),
                        reply_ping,
                        &files,
                        &spoilers,
                        (!files.is_empty())
                            .then(|| upload_progress(&tx, &ctx, &channel_id, &nonce)),
                    ),
                )
                .await
                {
                    Ok(message) => Ok(Some(ToUi::Sent {
                        channel_id,
                        nonce,
                        message: Box::new(message),
                    })),
                    // Shown red under the message with Retry / Delete;
                    // never retried on its own.
                    Err(e) => {
                        if let Some(rl) = crate::ratelimit::rate_limited(&e) {
                            let _ = tx.send(ToUi::RateLimited(rl));
                        }
                        Ok(Some(ToUi::SendFailed {
                            channel_id,
                            nonce,
                            error: crate::ratelimit::user_message(&e),
                        }))
                    }
                },
                ToBackend::CloseDm { channel_id } => api.close_dm(&channel_id).await.map(|_| None),
                ToBackend::LoadGifs { .. } if is_bot => Ok(Some(ToUi::GifsFailed {
                    error: "GIF search needs a user account".into(),
                })),
                ToBackend::LoadGifs { query } => Ok(Some(match api.gifs(query.as_deref()).await {
                    Ok(v) => ToUi::Gifs {
                        results: crate::gif_picker::parse(&v),
                        query,
                    },
                    Err(e) => ToUi::GifsFailed {
                        error: crate::ratelimit::user_message(&e),
                    },
                })),
                ToBackend::PickFiles => {
                    let picked = rfd::AsyncFileDialog::new()
                        .set_title("Upload files")
                        .pick_files()
                        .await
                        .unwrap_or_default();
                    let paths = picked.iter().map(|f| f.path().to_path_buf()).collect();
                    Ok(Some(ToUi::FilesPicked(paths)))
                }
                ToBackend::SavePastedImage(img) => {
                    let saved = tokio::task::spawn_blocking(move || {
                        crate::paste::save_png(&img, &crate::paste::paste_dir())
                    })
                    .await;
                    Ok(Some(match saved {
                        Ok(Ok(path)) => ToUi::FilesPicked(vec![path]),
                        Ok(Err(e)) => ToUi::Notice(format!("Couldn't paste the image: {e}")),
                        Err(e) => ToUi::Notice(format!("Couldn't paste the image: {e}")),
                    }))
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
                | ToBackend::RequestMemberList { .. }
                | ToBackend::SetPresence { .. } => Ok(None),
                ToBackend::LoadAround {
                    channel_id,
                    message_id,
                } => api
                    .history_around(&channel_id, &message_id)
                    .await
                    .map(|messages| {
                        Some(ToUi::HistoryAround {
                            channel_id,
                            message_id,
                            messages,
                        })
                    }),
                ToBackend::LoadPins { channel_id } => {
                    Ok(Some(match api.get_pins(&channel_id).await {
                        Ok(pins) => ToUi::Pins { channel_id, pins },
                        Err(e) => ToUi::PinsFailed {
                            channel_id,
                            error: crate::ratelimit::user_message(&e),
                        },
                    }))
                }
                ToBackend::SetPinned {
                    channel_id,
                    message_id,
                    pinned,
                } => {
                    let r = if pinned {
                        api.pin_message(&channel_id, &message_id).await
                    } else {
                        api.unpin_message(&channel_id, &message_id).await
                    };
                    Ok(Some(match r {
                        Ok(()) => ToUi::PinChanged {
                            channel_id,
                            message_id,
                            pinned,
                        },
                        Err(e) => ToUi::PinFailed {
                            channel_id,
                            message_id,
                            error: crate::ratelimit::user_message(&e),
                        },
                    }))
                }
                // Synced settings only exist for user accounts.
                ToBackend::LoadUserSettings | ToBackend::PatchUserSettings { .. } if is_bot => {
                    Ok(None)
                }
                ToBackend::LoadUserSettings => api.settings_proto().await.and_then(|v| {
                    Ok(Some(ToUi::UserSettings(Box::new(
                        crate::settings_proto::UserSettings::from_rest(&v)?,
                    ))))
                }),
                ToBackend::PatchUserSettings { settings_b64 } => {
                    api.patch_settings_proto(&settings_b64).await.and_then(|v| {
                        Ok(Some(ToUi::UserSettings(Box::new(
                            crate::settings_proto::UserSettings::from_rest(&v)?,
                        ))))
                    })
                }
                ToBackend::LeaveGuild { guild_id } => api
                    .leave_guild(&guild_id)
                    .await
                    .map(|_| Some(ToUi::GuildRemoved(guild_id))),
                ToBackend::SaveFile { url, filename } => save_file(&api, &url, &filename).await,
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
                // --- themes ---
                ToBackend::PickTheme { folder } => Ok(Some(ToUi::ThemePicked(
                    crate::themes::pick_import_source(folder).await,
                ))),
                // --- presets --- (local files only)
                ToBackend::PickPreset => {
                    Ok(crate::presets::pick_file().await.map(ToUi::PresetFile))
                }
                ToBackend::ExportPreset { file_name, text } => {
                    Ok(crate::presets::export_file(&file_name, text)
                        .await
                        .map(ToUi::PresetFile))
                }
                // --- onboarding --- (user accounts; writes only from clicks)
                ToBackend::JoinInvite { code } if is_bot => Ok(Some(ToUi::InviteJoined {
                    code,
                    outcome: crate::invites::JoinOutcome::Failed(
                        "Bots can't join servers through invites".into(),
                    ),
                })),
                ToBackend::ResolveInvite { code } => Ok(Some(ToUi::InviteResolved {
                    result: crate::invites::fetch(&api, &code).await,
                    code,
                })),
                ToBackend::JoinInvite { code } => Ok(Some(ToUi::InviteJoined {
                    outcome: crate::invites::join(&api, &code).await,
                    code,
                })),
                ToBackend::LoadScreening { guild_id } => Ok(Some(ToUi::ScreeningLoaded {
                    result: crate::screening::load(&api, &guild_id).await,
                    guild_id,
                })),
                ToBackend::SubmitScreening { guild_id, body } => {
                    Ok(Some(ToUi::ScreeningSubmitted {
                        result: crate::screening::submit(&api, &guild_id, &body).await,
                        guild_id,
                    }))
                }
                ToBackend::LoadOnboarding { guild_id, page } => Ok(Some(ToUi::OnboardingLoaded {
                    result: crate::onboarding::load(&api, &guild_id).await,
                    guild_id,
                    page,
                })),
                ToBackend::SubmitOnboarding { guild_id, body } => Ok(Some(ToUi::OnboardingSaved {
                    result: crate::onboarding::submit(&api, &guild_id, &body).await,
                    guild_id,
                })),
                // --- threads ---
                ToBackend::LoadActiveThreads { guild_id } => {
                    Ok(Some(match api.active_guild_threads(&guild_id).await {
                        Ok(page) => ToUi::ThreadPage { page },
                        Err(e) if not_available(&e) => ToUi::ThreadsUnavailable { guild_id },
                        Err(e) => ToUi::ThreadFailed {
                            error: crate::ratelimit::user_message(&e),
                        },
                    }))
                }
                ToBackend::Threads(action) => Ok(thread_request(&api, &tx, action).await),
                // --- friends ---
                ToBackend::Friends(_) | ToBackend::MessageRequest(_) if is_bot => {
                    Ok(Some(ToUi::Notice("Friends need a user account".into())))
                }
                ToBackend::Friends(action) => {
                    let r = crate::friends::execute(&api, &action).await;
                    if let Err(e) = &r
                        && let Some(rl) = crate::ratelimit::rate_limited(e)
                    {
                        let _ = tx.send(ToUi::RateLimited(rl));
                    }
                    Ok(Some(ToUi::FriendResult {
                        action,
                        result: r.map_err(|e| crate::ratelimit::user_message(&e)),
                    }))
                }
                ToBackend::MessageRequest(action) => {
                    let r = crate::friends::execute_request(&api, &action).await;
                    if let Err(e) = &r
                        && let Some(rl) = crate::ratelimit::rate_limited(e)
                    {
                        let _ = tx.send(ToUi::RateLimited(rl));
                    }
                    Ok(Some(ToUi::RequestResult {
                        action,
                        result: r.map_err(|e| crate::ratelimit::user_message(&e)),
                    }))
                }
                // --- search ---
                ToBackend::Search { seq, .. } if is_bot => Ok(Some(ToUi::SearchFailed {
                    seq,
                    error: "Search needs a user account (bots can't use it)".into(),
                })),
                ToBackend::Search {
                    seq,
                    scope,
                    query,
                    offset,
                } => Ok(Some(match api.search(&scope, &query, offset).await {
                    Ok(crate::search::SearchResponse::Page {
                        total,
                        messages,
                        partial,
                    }) => ToUi::SearchResults {
                        seq,
                        total,
                        messages: messages
                            .iter()
                            .map(crate::types::Message::from_json)
                            .collect(),
                        partial,
                    },
                    Ok(crate::search::SearchResponse::Indexing {
                        retry_after,
                        documents_indexed,
                    }) => ToUi::SearchIndexing {
                        seq,
                        retry_after,
                        documents_indexed,
                    },
                    Err(e) => ToUi::SearchFailed {
                        seq,
                        error: crate::ratelimit::user_message(&e),
                    },
                })),
                // --- reactions ---
                ToBackend::React {
                    channel_id,
                    message_id,
                    emoji,
                    add,
                    burst,
                } => {
                    let r = if add {
                        api.add_reaction(&channel_id, &message_id, &emoji).await
                    } else {
                        api.remove_reaction(&channel_id, &message_id, &emoji, burst)
                            .await
                    };
                    match r {
                        Ok(()) => Ok(None),
                        Err(e) => {
                            // Undo the optimistic change; a 429 also gets the
                            // quiet "Slow down" notice (never retried).
                            if let Some(rl) = crate::ratelimit::rate_limited(&e) {
                                let _ = tx.send(ToUi::RateLimited(rl));
                            }
                            Ok(Some(ToUi::ReactionFailed {
                                channel_id,
                                message_id,
                                emoji,
                                add,
                                burst,
                                error: crate::ratelimit::user_message(&e),
                            }))
                        }
                    }
                }
                ToBackend::LoadReactors {
                    channel_id,
                    message_id,
                    emoji,
                    burst,
                } => {
                    let users = api
                        .reactors(
                            &channel_id,
                            &message_id,
                            &emoji,
                            burst,
                            crate::reactions::REACTORS_LIMIT,
                        )
                        .await
                        .unwrap_or_default();
                    Ok(Some(ToUi::Reactors {
                        message_id,
                        emoji,
                        burst,
                        names: users.into_iter().map(|u| u.name).collect(),
                    }))
                }
                ToBackend::Notify { title, body } => {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::notify::show_desktop(&title, &body)
                    })
                    .await;
                    Ok(None)
                }
                // QR login runs before any session, in remote_auth.rs.
                ToBackend::StartQrLogin | ToBackend::CancelQrLogin => Ok(None),
                // Handled before the spawn.
                ToBackend::CancelSend { .. } => Ok(None),
                ToBackend::OpenCrashFolder(report) => {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::crash_report::open_folder(&report)
                    })
                    .await;
                    Ok(None)
                }
                // Bots have no notification settings.
                ToBackend::PatchGuildSettings { .. } if is_bot => Ok(None),
                ToBackend::PatchGuildSettings { guild_id, body } => {
                    api.patch_guild_settings(&guild_id, &body).await.map(|v| {
                        Some(ToUi::GuildSettingsUpdated(
                            crate::notif_settings::parse_response(&v, &guild_id),
                        ))
                    })
                }
                ToBackend::PlaySound { custom, volume } => {
                    let played = tokio::task::spawn_blocking(move || {
                        crate::sound::play(custom.as_deref(), volume)
                    })
                    .await;
                    // A custom file that can't be used: say why (the ping
                    // played instead).
                    Ok(played.ok().and_then(Result::err).map(ToUi::Notice))
                }
                ToBackend::PickSound => {
                    Ok(Some(ToUi::SoundPicked(crate::sound::pick_file().await)))
                }
                // GitHub only, with its own client (no Discord token).
                ToBackend::CheckUpdate => Ok(crate::update_check::fetch_latest()
                    .await
                    .map(ToUi::LatestRelease)),
            };
            match result {
                Ok(Some(ev)) => {
                    let _ = tx.send(ev);
                }
                Ok(None) => {}
                Err(e) => {
                    let ev = match crate::ratelimit::rate_limited(&e) {
                        Some(rl) => ToUi::RateLimited(rl),
                        None => {
                            ToUi::Status(format!("Error: {}", crate::ratelimit::user_message(&e)))
                        }
                    };
                    let _ = tx.send(ev);
                }
            }
            ctx.request_repaint();
        });
    }

    // The UI is gone (log out, or a restart for "Show as"): let the gateway
    // send its close frame so Discord ends this session now instead of
    // keeping it (and its platform) around until it times out.
    drop(gw_tx);
    let _ = tokio::time::timeout(Duration::from_secs(3), gateway).await;
}

/// Uploads that can still be cancelled, by nonce.
type Cancels = std::sync::Arc<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>;

fn lock(
    cancels: &Cancels,
) -> std::sync::MutexGuard<'_, HashMap<String, tokio::sync::oneshot::Sender<()>>> {
    cancels.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run a send; with `cancellable` (it has attachments) a
/// `ToBackend::CancelSend` for its nonce drops the request and the send
/// fails with "Upload cancelled".
async fn send_cancellable<T>(
    cancels: &Cancels,
    nonce: &str,
    cancellable: bool,
    send: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    if !cancellable {
        return send.await;
    }
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    lock(cancels).insert(nonce.to_owned(), cancel_tx);
    let result = tokio::select! {
        r = send => r,
        Ok(()) = cancel_rx => Err(anyhow::anyhow!(crate::upload::CANCELLED)),
    };
    lock(cancels).remove(nonce);
    result
}

/// The upload stream's progress callback: throttled `ToUi::UploadProgress`.
fn upload_progress(
    tx: &Sender<ToUi>,
    ctx: &Repaint,
    channel_id: &str,
    nonce: &str,
) -> crate::upload::OnProgress {
    let (tx, ctx) = (tx.clone(), ctx.clone());
    let (channel_id, nonce) = (channel_id.to_owned(), nonce.to_owned());
    let throttles = std::sync::Mutex::new(Vec::<crate::upload::Throttle>::new());
    std::sync::Arc::new(move |file, sent, total| {
        let report = {
            let mut t = throttles.lock().unwrap_or_else(|e| e.into_inner());
            if t.len() <= file {
                t.resize_with(file + 1, Default::default);
            }
            t[file].should_report(sent, total)
        };
        if report {
            let _ = tx.send(ToUi::UploadProgress {
                channel_id: channel_id.clone(),
                nonce: nonce.clone(),
                file,
                sent,
                total,
            });
            ctx.request_repaint();
        }
    })
}

/// Messages per history page (Discord's maximum).
const PAGE_LIMIT: u8 = crate::history::PAGE_SIZE as u8;

/// Decode a downloaded picture for the clipboard ("Copy Image").
fn decode_image(bytes: &[u8]) -> Option<eframe::egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied(
        size,
        img.as_raw(),
    ))
}

/// What a read-only test-account run (`RUSTCORD_TEST_ACCOUNT`) may still
/// do: load things and gateway reads. Everything that would send, ack,
/// type, react, join, edit or change settings is dropped.
fn read_only_ok(req: &ToBackend) -> bool {
    matches!(
        req,
        ToBackend::LoadHistory { .. }
            | ToBackend::LoadPage { .. }
            | ToBackend::LoadAround { .. }
            | ToBackend::CopyImage { .. }
            | ToBackend::Reconnect
            | ToBackend::CheckUpdate
            | ToBackend::PlaySound { .. }
            | ToBackend::PickSound
            | ToBackend::OpenCrashFolder(_)
            | ToBackend::LoadPins { .. }
            | ToBackend::LoadGifs { .. }
            | ToBackend::LoadProfile { .. }
            | ToBackend::LoadNote { .. }
            | ToBackend::LoadUserSettings
            | ToBackend::LoadActiveThreads { .. }
            | ToBackend::LoadReactors { .. }
            | ToBackend::ResolveInvite { .. }
            | ToBackend::Search { .. }
            | ToBackend::SubscribeMembers { .. }
            | ToBackend::RequestMembers { .. }
            | ToBackend::RequestMemberList { .. }
            | ToBackend::Notify { .. }
    )
}

#[cfg(test)]
mod read_only_tests {
    use super::*;

    #[test]
    fn writes_are_not_read_only() {
        assert!(read_only_ok(&ToBackend::LoadHistory {
            channel_id: "1".into()
        }));
        assert!(!read_only_ok(&ToBackend::Typing {
            channel_id: "1".into()
        }));
        assert!(!read_only_ok(&ToBackend::AckBulk(Vec::new())));
        assert!(read_only_ok(&ToBackend::LoadUserSettings));
        assert!(!read_only_ok(&ToBackend::PatchUserSettings {
            settings_b64: String::new()
        }));
    }
}
