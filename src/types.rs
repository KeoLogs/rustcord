//! Plain data types shared between the Discord backend and the UI.
//!
//! These are parsed by hand from `serde_json::Value` rather than with strict
//! `#[derive(Deserialize)]` structs, because Discord's payloads differ between
//! bot and user accounts (and change over time). Lenient parsing means a
//! missing field gives an empty value instead of crashing the client.

use serde_json::Value;

/// Discord channel types we care about for now.
pub const CHANNEL_TEXT: u64 = 0;
#[allow(dead_code)] // for grouping channels by category later
pub const CHANNEL_CATEGORY: u64 = 4;
pub const CHANNEL_ANNOUNCEMENT: u64 = 5;

#[derive(Debug, Clone)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: u64,
    pub position: i64,
    #[allow(dead_code)] // category this channel belongs to; for grouping later
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: String,
    pub content: String,
    pub timestamp: String,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Guild {
    pub fn from_json(v: &Value) -> Self {
        // Bot accounts get `name` at the top level; newer user payloads may
        // nest it under `properties`.
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/properties/name").and_then(Value::as_str))
            .unwrap_or("Unknown server")
            .to_string();

        let mut channels: Vec<Channel> = v
            .get("channels")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(Channel::from_json).collect())
            .unwrap_or_default();
        channels.sort_by_key(|c| c.position);

        Self {
            id: str_of(v, "id"),
            name,
            channels,
        }
    }

    /// Channels that can hold chat messages, in display order.
    pub fn text_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(|c| c.is_text())
    }
}

impl Channel {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            name: str_of(v, "name"),
            kind: v.get("type").and_then(Value::as_u64).unwrap_or(u64::MAX),
            position: v.get("position").and_then(Value::as_i64).unwrap_or(0),
            parent_id: v
                .get("parent_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    pub fn is_text(&self) -> bool {
        self.kind == CHANNEL_TEXT || self.kind == CHANNEL_ANNOUNCEMENT
    }
}

impl Message {
    pub fn from_json(v: &Value) -> Self {
        // Prefer display name (global_name) over the raw username.
        let author = v
            .pointer("/author/global_name")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/author/username").and_then(Value::as_str))
            .unwrap_or("unknown")
            .to_string();

        Self {
            id: str_of(v, "id"),
            channel_id: str_of(v, "channel_id"),
            author,
            content: str_of(v, "content"),
            timestamp: str_of(v, "timestamp"),
        }
    }

    /// "HH:MM" from Discord's ISO-8601 timestamp (UTC).
    pub fn short_time(&self) -> &str {
        self.timestamp.get(11..16).unwrap_or("")
    }
}

/// Events the backend sends to the UI.
#[derive(Debug)]
pub enum ToUi {
    Status(String),
    Ready {
        username: String,
        guilds: Vec<Guild>,
    },
    GuildAdded(Guild),
    History {
        channel_id: String,
        messages: Vec<Message>,
    },
    NewMessage(Message),
}

/// Requests the UI sends to the backend.
#[derive(Debug)]
pub enum ToBackend {
    LoadHistory { channel_id: String },
    Send { channel_id: String, content: String },
}
