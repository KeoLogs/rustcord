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
