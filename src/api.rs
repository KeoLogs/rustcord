//! Minimal Discord REST client.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::types::Message;

const API_BASE: &str = "https://discord.com/api/v10";

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    /// Value for the Authorization header ("Bot xxx" for bots, raw token for users).
    auth: String,
}

impl Api {
    pub fn new(auth: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("RustCord/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, auth })
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

    /// Last `limit` messages in a channel, oldest first.
    pub async fn channel_history(&self, channel_id: &str, limit: u8) -> Result<Vec<Message>> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages?limit={limit}");
        let resp = self
            .http
            .get(&url)
            .header("Authorization", &self.auth)
            .send()
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
        files: &[PathBuf],
    ) -> Result<()> {
        let mut body = json!({ "content": content });
        if let Some(id) = reply_to {
            body["message_reference"] = json!({ "message_id": id, "fail_if_not_exists": false });
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
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("file{i}"));
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

        let resp = request.send().await.context("send request failed")?;
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
            .http
            .patch(format!(
                "{API_BASE}/channels/{channel_id}/messages/{message_id}"
            ))
            .header("Authorization", &self.auth)
            .json(&json!({ "content": content }))
            .send()
            .await
            .context("edit request failed")?;
        Self::check(resp, "Editing message").await?;
        Ok(())
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!(
                "{API_BASE}/channels/{channel_id}/messages/{message_id}"
            ))
            .header("Authorization", &self.auth)
            .send()
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
            .http
            .post(format!(
                "{API_BASE}/channels/{channel_id}/messages/{message_id}/ack"
            ))
            .header("Authorization", &self.auth)
            .json(&body)
            .send()
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
            .http
            .post(format!("{API_BASE}/read-states/ack-bulk"))
            .header("Authorization", &self.auth)
            .json(&json!({ "read_states": read_states }))
            .send()
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
            .http
            .get(&url)
            .header("Authorization", &self.auth)
            .send()
            .await
            .context("profile request failed")?;
        Ok(Self::check(resp, "Loading profile").await?.json().await?)
    }

    /// My note on a user. 404 means there's no note.
    pub async fn note(&self, user_id: &str) -> Result<String> {
        let resp = self
            .http
            .get(format!("{API_BASE}/users/@me/notes/{user_id}"))
            .header("Authorization", &self.auth)
            .send()
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
            .http
            .put(format!("{API_BASE}/users/@me/notes/{user_id}"))
            .header("Authorization", &self.auth)
            .json(&json!({ "note": note }))
            .send()
            .await
            .context("note request failed")?;
        Self::check(resp, "Saving note").await?;
        Ok(())
    }

    // -- Relationships (friends) and DMs. User accounts only; every call is
    // made from an explicit user click (see `friends::execute`). --

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
            .http
            .put(format!("{API_BASE}/users/@me/relationships/{user_id}"))
            .header("Authorization", &self.auth)
            .json(&body)
            .send()
            .await
            .context("relationship request failed")?;
        Self::check(resp, what).await?;
        Ok(())
    }

    /// POST users/@me/relationships: send a friend request by @username.
    pub async fn send_friend_request(&self, username: &str) -> Result<()> {
        let resp = self
            .http
            .post(format!("{API_BASE}/users/@me/relationships"))
            .header("Authorization", &self.auth)
            .json(&json!({ "username": username, "discriminator": null }))
            .send()
            .await
            .context("friend request failed")?;
        Self::check(resp, "Sending friend request").await?;
        Ok(())
    }

    /// DELETE users/@me/relationships/{id}: remove a friend, cancel an
    /// outgoing request, ignore an incoming one, or unblock.
    pub async fn remove_relationship(&self, user_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{API_BASE}/users/@me/relationships/{user_id}"))
            .header("Authorization", &self.auth)
            .send()
            .await
            .context("relationship request failed")?;
        Self::check(resp, "Updating relationship").await?;
        Ok(())
    }

    /// POST users/@me/channels: open (or create) a DM. Returns the channel
    /// object; Discord also sends CHANNEL_CREATE if it's new.
    pub async fn open_dm(&self, user_id: &str) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{API_BASE}/users/@me/channels"))
            .header("Authorization", &self.auth)
            .json(&json!({ "recipients": [user_id] }))
            .send()
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
            .http
            .put(format!("{API_BASE}/channels/{channel_id}/recipients/@me"))
            .header("Authorization", &self.auth)
            .json(&json!({ "consent_status": 2 }))
            .send()
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
            .http
            .delete(format!("{API_BASE}/channels/{channel_id}/recipients/@me"))
            .header("Authorization", &self.auth)
            .send()
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

    /// DELETE channels/{id}: close a DM (hides it until a new message).
    pub async fn close_dm(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{API_BASE}/channels/{channel_id}"))
            .header("Authorization", &self.auth)
            .send()
            .await
            .context("close DM request failed")?;
        Self::check(resp, "Closing DM").await?;
        Ok(())
    }

    /// Show the typing indicator to others for ~10 seconds.
    pub async fn trigger_typing(&self, channel_id: &str) -> Result<()> {
        let resp = self
            .http
            .post(format!("{API_BASE}/channels/{channel_id}/typing"))
            .header("Authorization", &self.auth)
            .header("Content-Length", "0")
            .send()
            .await
            .context("typing request failed")?;
        Self::check(resp, "Typing indicator").await?;
        Ok(())
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
