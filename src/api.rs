//! Minimal Discord REST client.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::emoji_picker::EmojiRef;
use crate::forum::ForumQuery;
use crate::ratelimit::{self, Limiter, RateLimited};
use crate::search::{SearchQuery, SearchResponse, SearchScope};
use crate::threads::{CHANNEL_PRIVATE_THREAD, CHANNEL_PUBLIC_THREAD, Thread, ThreadPage};
use crate::types::{Message, User};

const API_BASE: &str = "https://discord.com/api/v10";

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    /// Value for the Authorization header ("Bot xxx" for bots, raw token for users).
    auth: String,
    /// Shared 429 state (per-route buckets + global lock).
    limiter: Arc<Limiter>,
}

impl Api {
    pub fn new(auth: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("RustCord/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            auth,
            limiter: Arc::default(),
        })
    }

    /// Every Discord API request goes through here (the shared 429 layer).
    ///
    /// GETs wait for a known reset and retry once after a 429; writes are
    /// never retried and fail with a typed [`RateLimited`] instead.
    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let req = rb.build()?;
        let is_get = req.method() == reqwest::Method::GET;
        let (route, major) = ratelimit::route_key(req.method().as_str(), req.url().path());
        let max_wait = if is_get {
            ratelimit::MAX_GET_WAIT
        } else {
            ratelimit::MAX_WRITE_WAIT
        };
        let mut retried = false;
        let mut next = Some(req);
        while let Some(req) = next.take() {
            if let Some(wait) = self.limiter.wait_for(&route, Instant::now()) {
                if wait > max_wait {
                    return Err(RateLimited {
                        retry_after: wait,
                        global: false,
                    }
                    .into());
                }
                tokio::time::sleep(wait).await;
            }
            // Keep a copy for the single GET retry (GETs have no body).
            let spare = if is_get && !retried {
                req.try_clone()
            } else {
                None
            };
            let resp = self.http.execute(req).await?;
            let headers = ratelimit::Headers::from_map(resp.headers());
            if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
                self.limiter
                    .update(&route, &major, &headers, Instant::now());
                return Ok(resp);
            }
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let rl = self.limiter.on_429(
                &route,
                &major,
                &headers,
                body["retry_after"].as_f64(),
                body["global"].as_bool().unwrap_or(false),
                Instant::now(),
            );
            match spare {
                Some(again) if rl.retry_after <= ratelimit::MAX_GET_WAIT => {
                    retried = true;
                    tokio::time::sleep(rl.retry_after).await;
                    next = Some(again);
                }
                _ => return Err(rl.into()),
            }
        }
        bail!("request could not be sent")
    }

    async fn check(resp: reqwest::Response, what: &str) -> Result<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // Pull out Discord's human-readable message if there is one.
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or(body);
        bail!("{what} failed: HTTP {status} - {msg}")
    }

    /// Up to 50 messages around `message_id` (oldest first), for jumping to
    /// a message that isn't loaded.
    pub async fn history_around(&self, channel_id: &str, message_id: &str) -> Result<Vec<Message>> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages?around={message_id}&limit=50");
        let resp = self
            .send(self.http.get(&url).header("Authorization", &self.auth))
            .await
            .context("history request failed")?;
        let body: Value = Self::check(resp, "Loading messages").await?.json().await?;
        let mut messages: Vec<Message> = body
            .as_array()
            .map(|arr| arr.iter().map(Message::from_json).collect())
            .unwrap_or_default();
        messages.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));
        Ok(messages)
    }

    /// Close a DM (`DELETE channels/{id}`, what Discord's × on a DM does; the
    /// conversation comes back when either side writes). Only from the
    /// user's click.
    pub async fn close_dm(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!("{API_BASE}/channels/{channel_id}"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("close DM request failed")?;
        Self::check(resp, "Closing DM").await?;
        Ok(())
    }

    /// Discord's GIF search (Tenor): trending (`None`) or search results.
    /// Undocumented endpoints; the response is parsed leniently by
    /// `gif_picker::parse`.
    pub async fn gifs(&self, query: Option<&str>) -> Result<Value> {
        let common = "provider=tenor&locale=en-US&media_format=gif";
        let url = match query {
            Some(q) => format!(
                "{API_BASE}/gifs/search?q={}&{common}",
                crate::gif_picker::url_encode(q)
            ),
            None => format!("{API_BASE}/gifs/trending?{common}"),
        };
        let resp = self
            .send(self.http.get(url).header("Authorization", &self.auth))
            .await
            .context("GIF request failed")?;
        Ok(Self::check(resp, "Loading GIFs").await?.json().await?)
    }

    /// Leave a server (`DELETE users/@me/guilds/{id}`, like Discord Messenger
    /// DI:1647). Only called after the user confirmed.
    pub async fn leave_guild(&self, guild_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!("{API_BASE}/users/@me/guilds/{guild_id}"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "lurking": false })),
            )
            .await
            .context("leave request failed")?;
        Self::check(resp, "Leaving server").await?;
        Ok(())
    }

    /// Download a file from Discord's CDN (no auth header: CDN links are
    /// signed and the token must never go to other hosts).
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().await.context("download failed")?;
        Ok(Self::check(resp, "Downloading")
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// Last `limit` messages in a channel, oldest first.
    pub async fn channel_history(&self, channel_id: &str, limit: u8) -> Result<Vec<Message>> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages?limit={limit}");
        let resp = self
            .send(self.http.get(&url).header("Authorization", &self.auth))
            .await
            .context("history request failed")?;
        let body: Value = Self::check(resp, "Loading messages").await?.json().await?;

        let mut messages: Vec<Message> = body
            .as_array()
            .map(|arr| arr.iter().map(Message::from_json).collect())
            .unwrap_or_default();
        // Discord returns newest first; the UI wants oldest first.
        messages.reverse();
        Ok(messages)
    }

    /// Send a message, optionally as a reply and/or with files. The gateway
    /// echoes it back as MESSAGE_CREATE, so we don't need the response.
    pub async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
        reply_ping: bool,
        files: &[PathBuf],
        spoilers: &[bool],
    ) -> Result<()> {
        let mut body = json!({ "content": content });
        if let Some(id) = reply_to {
            body["message_reference"] = json!({ "message_id": id, "fail_if_not_exists": false });
            // Discord's default parse list plus the reply ping switch.
            body["allowed_mentions"] = json!({
                "parse": ["users", "roles", "everyone"],
                "replied_user": reply_ping,
            });
        }
        let request = self
            .http
            .post(format!("{API_BASE}/channels/{channel_id}/messages"))
            .header("Authorization", &self.auth);

        let request = if files.is_empty() {
            request.json(&body)
        } else {
            // Multipart: JSON goes in `payload_json`, each file in `files[n]`.
            let mut parts = Vec::new();
            let mut meta = Vec::new();
            for (i, path) in files.iter().enumerate() {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("couldn't read {}", path.display()))?;
                let mut name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("file{i}"));
                // Discord marks spoiler attachments by their file name.
                if spoilers.get(i).copied().unwrap_or(false) && !name.starts_with("SPOILER_") {
                    name = format!("SPOILER_{name}");
                }
                meta.push(json!({ "id": i, "filename": name }));
                let mime = mime_for(&name);
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(name)
                    .mime_str(mime)?;
                parts.push((format!("files[{i}]"), part));
            }
            body["attachments"] = json!(meta);
            let mut form = reqwest::multipart::Form::new().text("payload_json", body.to_string());
            for (name, part) in parts {
                form = form.part(name, part);
            }
            request.multipart(form)
        };

        let resp = self.send(request).await.context("send request failed")?;
        Self::check(resp, "Sending message").await?;
        Ok(())
    }

    pub async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .patch(format!(
                        "{API_BASE}/channels/{channel_id}/messages/{message_id}"
                    ))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "content": content })),
            )
            .await
            .context("edit request failed")?;
        Self::check(resp, "Editing message").await?;
        Ok(())
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!(
                        "{API_BASE}/channels/{channel_id}/messages/{message_id}"
                    ))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("delete request failed")?;
        Self::check(resp, "Deleting message").await?;
        Ok(())
    }

    /// Mark a channel read up to `message_id` (DM: `RequestAcknowledgeChannel`).
    /// User accounts only.
    pub async fn ack(&self, channel_id: &str, message_id: &str) -> Result<()> {
        self.post_ack(channel_id, message_id, json!({ "token": null }))
            .await
    }

    /// "Mark unread": a manual ack of the message *before* the first unread
    /// one (DM: `RequestAcknowledgeMessages(.., manual = true)`).
    pub async fn mark_unread(
        &self,
        channel_id: &str,
        message_id: &str,
        mention_count: u32,
    ) -> Result<()> {
        self.post_ack(
            channel_id,
            message_id,
            json!({ "manual": true, "mention_count": mention_count }),
        )
        .await
    }

    async fn post_ack(&self, channel_id: &str, message_id: &str, body: Value) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .post(format!(
                        "{API_BASE}/channels/{channel_id}/messages/{message_id}/ack"
                    ))
                    .header("Authorization", &self.auth)
                    .json(&body),
            )
            .await
            .context("ack request failed")?;
        Self::check(resp, "Marking as read").await?;
        Ok(())
    }

    /// Mark several channels read at once (DM: `RequestAcknowledgeGuild`).
    pub async fn ack_bulk(&self, acks: &[(String, String)]) -> Result<()> {
        let read_states: Vec<Value> = acks
            .iter()
            .map(|(c, m)| json!({ "channel_id": c, "message_id": m, "read_state_type": 0 }))
            .collect();
        let resp = self
            .send(
                self.http
                    .post(format!("{API_BASE}/read-states/ack-bulk"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "read_states": read_states })),
            )
            .await
            .context("ack-bulk request failed")?;
        Self::check(resp, "Marking server as read").await?;
        Ok(())
    }

    /// A user's profile (user accounts only). `guild_id` adds their server
    /// member data (nick, roles, joined_at).
    pub async fn user_profile(&self, user_id: &str, guild_id: Option<&str>) -> Result<Value> {
        let mut url = format!(
            "{API_BASE}/users/{user_id}/profile?with_mutual_guilds=false\
             &with_mutual_friends=false&with_mutual_friends_count=false"
        );
        if let Some(g) = guild_id {
            url.push_str(&format!("&guild_id={g}"));
        }
        let resp = self
            .send(self.http.get(&url).header("Authorization", &self.auth))
            .await
            .context("profile request failed")?;
        Ok(Self::check(resp, "Loading profile").await?.json().await?)
    }

    /// My note on a user. 404 means there's no note.
    pub async fn note(&self, user_id: &str) -> Result<String> {
        let resp = self
            .send(
                self.http
                    .get(format!("{API_BASE}/users/@me/notes/{user_id}"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("note request failed")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(String::new());
        }
        let body: Value = Self::check(resp, "Loading note").await?.json().await?;
        Ok(body["note"].as_str().unwrap_or_default().to_string())
    }

    /// Set my note on a user (empty clears it, like Discord's client).
    pub async fn set_note(&self, user_id: &str, note: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(format!("{API_BASE}/users/@me/notes/{user_id}"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "note": note })),
            )
            .await
            .context("note request failed")?;
        Self::check(resp, "Saving note").await?;
        Ok(())
    }

    /// Pinned messages in a channel, newest pin first (raw message objects;
    /// parse with `pins::PinnedMessage::from_json`).
    ///
    /// Uses the classic `GET channels/{id}/pins` (a plain array, as Discord
    /// Messenger does) but also accepts the newer paged shape
    /// `{ items: [{ pinned_at, message }] }`, copying `pinned_at` into each
    /// message.
    pub async fn get_pins(&self, channel_id: &str) -> Result<Vec<Value>> {
        let resp = self
            .send(
                self.http
                    .get(format!("{API_BASE}/channels/{channel_id}/pins"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("pins request failed")?;
        let body: Value = Self::check(resp, "Loading pinned messages")
            .await?
            .json()
            .await?;
        Ok(pins_from_body(body))
    }

    /// Pin a message. Only called when the user picks "Pin".
    pub async fn pin_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(format!(
                        "{API_BASE}/channels/{channel_id}/messages/pins/{message_id}"
                    ))
                    .header("Authorization", &self.auth)
                    .header("Content-Length", "0"),
            )
            .await
            .context("pin request failed")?;
        Self::check(resp, "Pinning message").await?;
        Ok(())
    }

    /// Unpin a message. Only called when the user picks "Unpin".
    pub async fn unpin_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!(
                        "{API_BASE}/channels/{channel_id}/messages/pins/{message_id}"
                    ))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("unpin request failed")?;
        Self::check(resp, "Unpinning message").await?;
        Ok(())
    }

    /// My synced settings (`{"settings": "<b64 PreloadedUserSettings>"}`).
    /// User accounts only; see `settings_proto::UserSettings::from_rest`.
    pub async fn settings_proto(&self) -> Result<Value> {
        let resp = self
            .send(
                self.http
                    .get(format!("{API_BASE}/users/@me/settings-proto/1"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("settings request failed")?;
        Ok(Self::check(resp, "Loading settings").await?.json().await?)
    }

    /// Change synced settings. `settings_b64` holds only the changed
    /// top-level sub-messages (built by `settings_proto::UserSettings::patch_*`
    /// from a user action). Returns `{"settings": "<full b64>"}`.
    pub async fn patch_settings_proto(&self, settings_b64: &str) -> Result<Value> {
        let resp = self
            .send(
                self.http
                    .patch(format!("{API_BASE}/users/@me/settings-proto/1"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "settings": settings_b64 })),
            )
            .await
            .context("settings request failed")?;
        Ok(Self::check(resp, "Saving settings").await?.json().await?)
    }

    /// Show the typing indicator to others for ~10 seconds.
    pub async fn trigger_typing(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .post(format!("{API_BASE}/channels/{channel_id}/typing"))
                    .header("Authorization", &self.auth)
                    .header("Content-Length", "0"),
            )
            .await
            .context("typing request failed")?;
        Self::check(resp, "Typing indicator").await?;
        Ok(())
    }

    // --- search ---

    /// One page (25) of message search results in a server or DM
    /// (user accounts only). HTTP 202 (index not built yet) comes back as
    /// `SearchResponse::Indexing`; see `search.rs` for the protocol. Only
    /// sent for a user's search, never polled.
    pub async fn search(
        &self,
        scope: &SearchScope,
        query: &SearchQuery,
        offset: u32,
    ) -> Result<SearchResponse> {
        let url = crate::search::search_url(API_BASE, scope, query, offset);
        let resp = self
            .send(self.http.get(&url).header("Authorization", &self.auth))
            .await
            .context("search request failed")?;
        let status = resp.status().as_u16();
        let body: Value = Self::check(resp, "Searching").await?.json().await?;
        Ok(SearchResponse::from_json(status, &body))
    }

    // --- friends ---
    // Relationships (friends) and DMs. User accounts only; every call is
    // made from an explicit user click (see `friends::execute`).

    /// PUT users/@me/relationships/{id} with `{}`: accept an incoming friend
    /// request, or send one to a user id.
    pub async fn add_friend(&self, user_id: &str) -> Result<()> {
        self.put_relationship(user_id, json!({}), "Adding friend")
            .await
    }

    /// PUT users/@me/relationships/{id} with `{"type": 2}`: block a user.
    pub async fn block_user(&self, user_id: &str) -> Result<()> {
        self.put_relationship(user_id, json!({ "type": 2 }), "Blocking user")
            .await
    }

    async fn put_relationship(&self, user_id: &str, body: Value, what: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(format!("{API_BASE}/users/@me/relationships/{user_id}"))
                    .header("Authorization", &self.auth)
                    .json(&body),
            )
            .await
            .context("relationship request failed")?;
        Self::check(resp, what).await?;
        Ok(())
    }

    /// POST users/@me/relationships: send a friend request by @username.
    pub async fn send_friend_request(&self, username: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .post(format!("{API_BASE}/users/@me/relationships"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "username": username, "discriminator": null })),
            )
            .await
            .context("friend request failed")?;
        Self::check(resp, "Sending friend request").await?;
        Ok(())
    }

    /// DELETE users/@me/relationships/{id}: remove a friend, cancel an
    /// outgoing request, ignore an incoming one, or unblock.
    pub async fn remove_relationship(&self, user_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!("{API_BASE}/users/@me/relationships/{user_id}"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("relationship request failed")?;
        Self::check(resp, "Updating relationship").await?;
        Ok(())
    }

    /// POST users/@me/channels: open (or create) a DM. Returns the channel
    /// object; Discord also sends CHANNEL_CREATE if it's new.
    pub async fn open_dm(&self, user_id: &str) -> Result<Value> {
        let resp = self
            .send(
                self.http
                    .post(format!("{API_BASE}/users/@me/channels"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "recipients": [user_id] })),
            )
            .await
            .context("open DM request failed")?;
        Ok(Self::check(resp, "Opening DM").await?.json().await?)
    }

    /// Accept a message request.
    ///
    /// Unofficial (user API): PUT channels/{id}/recipients/@me with
    /// `{"consent_status": 2}` (2 = accepted), as documented by the
    /// community user-API docs. Kept in one place in case Discord changes it.
    pub async fn accept_message_request(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(format!("{API_BASE}/channels/{channel_id}/recipients/@me"))
                    .header("Authorization", &self.auth)
                    .json(&json!({ "consent_status": 2 })),
            )
            .await
            .context("message request failed")?;
        Self::check(resp, "Accepting message request").await?;
        Ok(())
    }

    /// Ignore (reject) a message request.
    ///
    /// Unofficial: DELETE channels/{id}/recipients/@me. If Discord answers
    /// 404/405 to that, fall back to closing the DM (DELETE channels/{id}),
    /// which at least hides it.
    pub async fn reject_message_request(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!("{API_BASE}/channels/{channel_id}/recipients/@me"))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("message request failed")?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return self.close_dm(channel_id).await;
        }
        Self::check(resp, "Ignoring message request").await?;
        Ok(())
    }

    // --- reactions ---

    /// Add my (normal) reaction. Only ever called because the user clicked.
    /// The gateway echoes it as MESSAGE_REACTION_ADD.
    pub async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &EmojiRef,
    ) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(reaction_url(channel_id, message_id, emoji, None, true))
                    .header("Authorization", &self.auth)
                    .header("Content-Length", "0"),
            )
            .await
            .context("reaction request failed")?;
        Self::check(resp, "Adding reaction").await?;
        Ok(())
    }

    /// Remove my reaction. `burst` removes my Super Reaction (user accounts;
    /// the `/{type}/@me` path Discord's own client uses).
    pub async fn remove_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &EmojiRef,
        burst: bool,
    ) -> Result<()> {
        let kind = burst.then_some(1);
        let resp = self
            .send(
                self.http
                    .delete(reaction_url(channel_id, message_id, emoji, kind, true))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("reaction request failed")?;
        Self::check(resp, "Removing reaction").await?;
        Ok(())
    }

    /// Who reacted with `emoji` (first `limit`, 1-100), for the tooltip.
    pub async fn reactors(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &EmojiRef,
        burst: bool,
        limit: u8,
    ) -> Result<Vec<User>> {
        let url = format!(
            "{}?limit={}&type={}",
            reaction_url(channel_id, message_id, emoji, None, false),
            limit.clamp(1, 100),
            u8::from(burst)
        );
        let resp = self
            .send(self.http.get(&url).header("Authorization", &self.auth))
            .await
            .context("reactions request failed")?;
        let body: Value = Self::check(resp, "Loading reactions").await?.json().await?;
        Ok(body
            .as_array()
            .map(|arr| arr.iter().map(User::from_json).collect())
            .unwrap_or_default())
    }
}

// --- reactions ---

/// `channels/{c}/messages/{m}/reactions/{emoji}[/{type}][/@me]`, with the
/// emoji URL-encoded (Unicode) or as `name:id` (custom).
fn reaction_url(
    channel_id: &str,
    message_id: &str,
    emoji: &EmojiRef,
    kind: Option<u8>,
    me: bool,
) -> String {
    let mut url = format!(
        "{API_BASE}/channels/{channel_id}/messages/{message_id}/reactions/{}",
        emoji.url_segment()
    );
    if let Some(k) = kind {
        url.push_str(&format!("/{k}"));
    }
    if me {
        url.push_str("/@me");
    }
    url
}

/// Message objects from either pins response shape (see `Api::get_pins`).
fn pins_from_body(body: Value) -> Vec<Value> {
    match body {
        Value::Array(messages) => messages.into_iter().filter(Value::is_object).collect(),
        Value::Object(mut obj) => match obj.remove("items") {
            Some(Value::Array(items)) => items
                .into_iter()
                .filter_map(|mut item| {
                    let pinned_at = item.get("pinned_at").cloned();
                    let mut msg = item.get_mut("message")?.take();
                    if let (Some(at), Some(m)) = (pinned_at, msg.as_object_mut()) {
                        m.insert("pinned_at".into(), at);
                    }
                    msg.is_object().then_some(msg)
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Content type from the file extension (Discord uses it to decide whether to
/// show an image preview).
fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "txt" | "log" | "md" | "rs" | "toml" | "json" => "text/plain",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

// --- threads ---
// Threads and forums (see `threads.rs` / `forum.rs`). Join / leave / create
// are only ever called because the user clicked something; nothing here is
// retried or polled.
impl Api {
    async fn get_json(&self, url: &str, what: &str) -> Result<Value> {
        let resp = self
            .send(self.http.get(url).header("Authorization", &self.auth))
            .await
            .with_context(|| format!("{what} request failed"))?;
        Ok(Self::check(resp, what).await?.json().await?)
    }

    async fn post_json(&self, url: &str, body: &Value, what: &str) -> Result<Value> {
        let resp = self
            .send(
                self.http
                    .post(url)
                    .header("Authorization", &self.auth)
                    .json(body),
            )
            .await
            .with_context(|| format!("{what} request failed"))?;
        Ok(Self::check(resp, what).await?.json().await?)
    }

    /// All active threads in a server: `GET /guilds/{id}/threads/active`.
    /// Documented for bots; Discord's own client doesn't use it, so user
    /// accounts may get an error here (use `forum_threads` per channel).
    pub async fn active_guild_threads(&self, guild_id: &str) -> Result<ThreadPage> {
        let v = self
            .get_json(
                &format!("{API_BASE}/guilds/{guild_id}/threads/active"),
                "Loading threads",
            )
            .await?;
        Ok(ThreadPage::from_json(&v, Some(guild_id)))
    }

    /// One page of a forum's posts, the way Discord's client asks for them
    /// (user accounts): `GET /channels/{forum}/threads/search?…`.
    /// Undocumented endpoint; see the notes in `forum.rs`.
    pub async fn forum_threads(&self, forum_id: &str, query: &ForumQuery) -> Result<ThreadPage> {
        let v = self
            .get_json(
                &format!(
                    "{API_BASE}/channels/{forum_id}/threads/search?{}",
                    query.search_params()
                ),
                "Loading posts",
            )
            .await?;
        Ok(ThreadPage::from_json(&v, None))
    }

    /// Public archived threads of a channel, newest archived first:
    /// `GET /channels/{id}/threads/archived/public`. `before` is an ISO-8601
    /// `archive_timestamp` (from `ThreadStore::oldest_archived_iso`).
    pub async fn archived_public_threads(
        &self,
        channel_id: &str,
        before: Option<&str>,
        limit: u8,
    ) -> Result<ThreadPage> {
        let mut url = format!(
            "{API_BASE}/channels/{channel_id}/threads/archived/public?limit={}",
            limit.clamp(2, 100)
        );
        if let Some(b) = before {
            // '+' in "+00:00" would be read as a space.
            url.push_str("&before=");
            url.push_str(&b.replace('+', "%2B"));
        }
        let v = self.get_json(&url, "Loading archived threads").await?;
        Ok(ThreadPage::from_json(&v, None))
    }

    /// Join a thread: `PUT /channels/{thread}/thread-members/@me`.
    pub async fn join_thread(&self, thread_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .put(format!(
                        "{API_BASE}/channels/{thread_id}/thread-members/@me"
                    ))
                    .header("Authorization", &self.auth)
                    .header("Content-Length", "0"),
            )
            .await
            .context("join thread request failed")?;
        Self::check(resp, "Joining thread").await?;
        Ok(())
    }

    /// Leave a thread: `DELETE /channels/{thread}/thread-members/@me`.
    pub async fn leave_thread(&self, thread_id: &str) -> Result<()> {
        let resp = self
            .send(
                self.http
                    .delete(format!(
                        "{API_BASE}/channels/{thread_id}/thread-members/@me"
                    ))
                    .header("Authorization", &self.auth),
            )
            .await
            .context("leave thread request failed")?;
        Self::check(resp, "Leaving thread").await?;
        Ok(())
    }

    /// Start a thread from a message:
    /// `POST /channels/{channel}/messages/{message}/threads`.
    pub async fn create_thread_from_message(
        &self,
        channel_id: &str,
        message_id: &str,
        name: &str,
        auto_archive_minutes: u32,
    ) -> Result<Thread> {
        let v = self
            .post_json(
                &format!("{API_BASE}/channels/{channel_id}/messages/{message_id}/threads"),
                &json!({ "name": name, "auto_archive_duration": auto_archive_minutes }),
                "Creating thread",
            )
            .await?;
        Ok(Thread::from_json(&v, None))
    }

    /// Start a thread without a message in a text / announcement channel:
    /// `POST /channels/{channel}/threads` (type 11 public, 12 private).
    pub async fn create_thread(
        &self,
        channel_id: &str,
        name: &str,
        private: bool,
        auto_archive_minutes: u32,
    ) -> Result<Thread> {
        let kind = if private {
            CHANNEL_PRIVATE_THREAD
        } else {
            CHANNEL_PUBLIC_THREAD
        };
        let v = self
            .post_json(
                &format!("{API_BASE}/channels/{channel_id}/threads"),
                &json!({
                    "name": name,
                    "type": kind,
                    "auto_archive_duration": auto_archive_minutes,
                }),
                "Creating thread",
            )
            .await?;
        Ok(Thread::from_json(&v, None))
    }

    /// New forum / media post: `POST /channels/{forum}/threads` with the
    /// first message. The answer is the thread (with the message under
    /// `message` on current API versions).
    pub async fn create_forum_post(
        &self,
        forum_id: &str,
        name: &str,
        content: &str,
        applied_tags: &[String],
    ) -> Result<Thread> {
        let v = self
            .post_json(
                &format!("{API_BASE}/channels/{forum_id}/threads"),
                &json!({
                    "name": name,
                    "applied_tags": applied_tags,
                    "message": { "content": content },
                }),
                "Creating post",
            )
            .await?;
        Ok(Thread::from_json(&v, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_both_shapes() {
        let classic = json!([{ "id": "1" }, "junk", { "id": "2" }]);
        let ids: Vec<_> = pins_from_body(classic)
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["1", "2"]);

        let paged = json!({
            "items": [
                { "pinned_at": "2026-11-01T10:00:00+00:00", "message": { "id": "3" } },
                { "pinned_at": "x" },
                { "message": "junk" }
            ],
            "has_more": false
        });
        let pins = pins_from_body(paged);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0]["id"], "3");
        assert_eq!(pins[0]["pinned_at"], "2026-11-01T10:00:00+00:00");

        assert!(pins_from_body(json!({ "message": "401: Unauthorized" })).is_empty());
        assert!(pins_from_body(Value::Null).is_empty());
    }
    #[test]
    fn reaction_urls() {
        let base = format!("{API_BASE}/channels/1/messages/2/reactions");
        assert_eq!(
            reaction_url("1", "2", &EmojiRef::unicode("👍"), None, true),
            format!("{base}/%F0%9F%91%8D/@me")
        );
        assert_eq!(
            reaction_url(
                "1",
                "2",
                &EmojiRef::custom("9", "pepe", true),
                Some(1),
                true
            ),
            format!("{base}/pepe:9/1/@me")
        );
        assert_eq!(
            reaction_url("1", "2", &EmojiRef::unicode("🔥"), None, false),
            format!("{base}/%F0%9F%94%A5")
        );
    }
}
