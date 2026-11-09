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

    /// Up to 50 messages around `message_id` (oldest first), for jumping to
    /// a message that isn't loaded.
    pub async fn history_around(&self, channel_id: &str, message_id: &str) -> Result<Vec<Message>> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages?around={message_id}&limit=50");
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
        messages.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));
        Ok(messages)
    }

    /// Leave a server (`DELETE users/@me/guilds/{id}`, like Discord Messenger
    /// DI:1647). Only called after the user confirmed.
    pub async fn leave_guild(&self, guild_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{API_BASE}/users/@me/guilds/{guild_id}"))
            .header("Authorization", &self.auth)
            .json(&json!({ "lurking": false }))
            .send()
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
        reply_ping: bool,
        files: &[PathBuf],
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

    /// Pinned messages in a channel, newest pin first (raw message objects;
    /// parse with `pins::PinnedMessage::from_json`).
    ///
    /// Uses the classic `GET channels/{id}/pins` (a plain array, as Discord
    /// Messenger does) but also accepts the newer paged shape
    /// `{ items: [{ pinned_at, message }] }`, copying `pinned_at` into each
    /// message.
    pub async fn get_pins(&self, channel_id: &str) -> Result<Vec<Value>> {
        let resp = self
            .http
            .get(format!("{API_BASE}/channels/{channel_id}/pins"))
            .header("Authorization", &self.auth)
            .send()
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
            .http
            .put(format!(
                "{API_BASE}/channels/{channel_id}/messages/pins/{message_id}"
            ))
            .header("Authorization", &self.auth)
            .header("Content-Length", "0")
            .send()
            .await
            .context("pin request failed")?;
        Self::check(resp, "Pinning message").await?;
        Ok(())
    }

    /// Unpin a message. Only called when the user picks "Unpin".
    pub async fn unpin_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!(
                "{API_BASE}/channels/{channel_id}/messages/pins/{message_id}"
            ))
            .header("Authorization", &self.auth)
            .send()
            .await
            .context("unpin request failed")?;
        Self::check(resp, "Unpinning message").await?;
        Ok(())
    }

    /// My synced settings (`{"settings": "<b64 PreloadedUserSettings>"}`).
    /// User accounts only; see `settings_proto::UserSettings::from_rest`.
    pub async fn settings_proto(&self) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{API_BASE}/users/@me/settings-proto/1"))
            .header("Authorization", &self.auth)
            .send()
            .await
            .context("settings request failed")?;
        Ok(Self::check(resp, "Loading settings").await?.json().await?)
    }

    /// Change synced settings. `settings_b64` holds only the changed
    /// top-level sub-messages (built by `settings_proto::UserSettings::patch_*`
    /// from a user action). Returns `{"settings": "<full b64>"}`.
    pub async fn patch_settings_proto(&self, settings_b64: &str) -> Result<Value> {
        let resp = self
            .http
            .patch(format!("{API_BASE}/users/@me/settings-proto/1"))
            .header("Authorization", &self.auth)
            .json(&json!({ "settings": settings_b64 }))
            .send()
            .await
            .context("settings request failed")?;
        Ok(Self::check(resp, "Saving settings").await?.json().await?)
    }

    /// Generic REST call for the server-admin pages (`guild_admin::execute`).
    /// `path` is relative to the API base and may carry a query string;
    /// `audit_reason` must already be percent-encoded (it goes into the
    /// `X-Audit-Log-Reason` header). 204 / empty bodies give `Value::Null`.
    pub async fn admin_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        audit_reason: Option<&str>,
        what: &str,
    ) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{API_BASE}{path}"))
            .header("Authorization", &self.auth);
        if let Some(reason) = audit_reason.filter(|r| !r.is_empty()) {
            request = request.header("X-Audit-Log-Reason", reason);
        }
        request = match body {
            Some(b) => request.json(b),
            None => request.header("Content-Length", "0"),
        };
        let resp = request
            .send()
            .await
            .with_context(|| format!("{what} request failed"))?;
        let text = Self::check(resp, what).await?.text().await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
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
}
