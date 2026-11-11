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

/// Image viewer "Copy image": download, decode on a blocking thread (first
/// frame of a GIF), hand the pixels to the UI for the clipboard.
async fn copy_image(api: &Api, url: &str) -> anyhow::Result<Option<ToUi>> {
    let bytes = api.download(url).await?;
    let decoded = tokio::task::spawn_blocking(move || {
        image::load_from_memory(&bytes).map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            eframe::egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
        })
    })
    .await?;
    Ok(Some(match decoded {
        Ok(img) => ToUi::CopyImage(std::sync::Arc::new(img)),
        Err(_) => ToUi::Notice("Couldn't copy the image: unknown format".into()),
    }))
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
#[derive(Clone, Copy, Default)]
pub struct Options {
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

/// Saved-account recovery (`accounts::missing_accounts`): for each stored
/// `(user id, token)`, ask Discord who the token belongs to (`GET
/// users/@me`, read-only) on a short-lived backend thread, and send back
/// the account's metadata when it matches that id. Bad or expired tokens
/// and network errors are skipped quietly (nothing is logged; the token is
/// never formatted). The receiver disconnects when every lookup is done.
pub fn spawn_account_lookup(
    lookups: Vec<(String, crate::accounts::Token)>,
    egui_ctx: eframe::egui::Context,
) -> std::sync::mpsc::Receiver<crate::accounts::SavedAccount> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let ctx = egui_ctx.clone();
        rt.block_on(async move {
            for (id, token) in lookups.into_iter().take(crate::accounts::MAX_ACCOUNTS) {
                let Ok(api) = Api::new(token.expose().to_string()) else {
                    continue;
                };
                let Ok(Ok(me)) = tokio::time::timeout(Duration::from_secs(15), api.me()).await
                else {
                    continue;
                };
                if me.id == id
                    && tx
                        .send(crate::accounts::SavedAccount::from_user(&me))
                        .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
        });
        egui_ctx.request_repaint();
    });
    rx
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
        Ok(api) => api,
        Err(e) => {
            let _ = tx.send(ToUi::Status(format!("HTTP client error: {e}")));
            return;
        }
    };

    // Ops for the gateway (op 14 / op 8) from UI requests.
    let (gw_tx, mut gw_rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();

    // Gateway task: reconnects with backoff until a fatal error, or until
    // the UI goes away (then it closes the connection cleanly).
    let gateway = {
        let tx = tx.clone();
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
                match gateway::run_session(&cfg, &mut session, &tx, &mut gw_rx, &repaint).await {
                    Ok(SessionEnd::Fatal(reason)) => {
                        let _ = tx.send(ToUi::Fatal(reason));
                        repaint();
                        return;
                    }
                    Ok(SessionEnd::Shutdown) => return,
                    Ok(SessionEnd::Retry) => backoff = 1,
                    Err(e) => {
                        let _ = tx.send(ToUi::Status(format!("Disconnected: {e}")));
                        repaint();
                    }
                }
                // Stop waiting if the UI goes away meanwhile.
                if gw_rx.is_closed() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        })
    };

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
                ToBackend::Send {
                    channel_id,
                    content,
                    reply_to,
                    reply_ping,
                    files,
                    spoilers,
                } => api
                    .send_message(
                        &channel_id,
                        &content,
                        reply_to.as_deref(),
                        reply_ping,
                        &files,
                        &spoilers,
                    )
                    .await
                    .map(|_| None),
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
                ToBackend::CopyImage { url } => copy_image(&api, &url).await,
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
