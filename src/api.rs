//! Minimal Discord REST client: fetch channel history and send messages.

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

        if !resp.status().is_success() {
            bail!(
                "history: HTTP {} - {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }

        let body: Value = resp.json().await?;
        let mut messages: Vec<Message> = body
            .as_array()
            .map(|arr| arr.iter().map(Message::from_json).collect())
            .unwrap_or_default();
        // Discord returns newest first; the UI wants oldest first.
        messages.reverse();
        Ok(messages)
    }

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<()> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages");
        let resp = self
            .http
            .post(&url)
            .header("Authorization", &self.auth)
            .json(&json!({ "content": content }))
            .send()
            .await
            .context("send request failed")?;

        if !resp.status().is_success() {
            bail!(
                "send: HTTP {} - {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        // No need to push the message to the UI here: the gateway will echo it
        // back as a MESSAGE_CREATE event.
        Ok(())
    }
}
