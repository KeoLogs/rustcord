//! Plain data types shared between the Discord backend and the UI.
//!
//! These are parsed by hand from `serde_json::Value` rather than with strict
//! `#[derive(Deserialize)]` structs, because Discord's payloads differ between
//! bot and user accounts (and change over time). Lenient parsing means a
//! missing field gives an empty value instead of crashing the client.

use std::path::PathBuf;

use serde_json::Value;

/// Discord channel types we care about.
pub const CHANNEL_TEXT: u64 = 0;
pub const CHANNEL_DM: u64 = 1;
pub const CHANNEL_GROUP_DM: u64 = 3;
pub const CHANNEL_CATEGORY: u64 = 4;
pub const CHANNEL_ANNOUNCEMENT: u64 = 5;

/// Pseudo guild id used for the DMs "server".
pub const DM_GUILD_ID: &str = "@me";

const CDN: &str = "https://cdn.discordapp.com";

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn snowflake(v: &Value, key: &str) -> Option<u64> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: String,
    /// Display name (global_name), falling back to username.
    pub name: String,
    /// The unique @username.
    pub username: String,
    pub avatar_url: String,
    pub bot: bool,
}

impl User {
    pub fn from_json(v: &Value) -> Self {
        let id = str_of(v, "id");
        let name = v
            .get("global_name")
            .and_then(Value::as_str)
            .or_else(|| v.get("username").and_then(Value::as_str))
            .unwrap_or("unknown")
            .to_string();
        let avatar_url = avatar_url(&id, v.get("avatar").and_then(Value::as_str));
        Self {
            id,
            name,
            username: str_of(v, "username"),
            avatar_url,
            bot: v.get("bot").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

/// CDN URL for a user's avatar, or Discord's default avatar if they have none.
pub fn avatar_url(user_id: &str, hash: Option<&str>) -> String {
    match hash {
        Some(h) => {
            let ext = if h.starts_with("a_") { "gif" } else { "png" };
            format!("{CDN}/avatars/{user_id}/{h}.{ext}?size=64")
        }
        None => {
            let index = user_id.parse::<u64>().map(|id| (id >> 22) % 6).unwrap_or(0);
            format!("{CDN}/embed/avatars/{index}.png")
        }
    }
}

// ---------------------------------------------------------------------------
// Guilds and channels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: u64,
    pub position: i64,
    pub parent_id: Option<String>,
    /// For DMs: the other people in the conversation.
    pub recipients: Vec<User>,
    /// Snowflake of the newest message, used to sort DMs by recency.
    pub last_message_id: u64,
}

/// A category header (if any) and the text channels under it.
pub struct ChannelGroup<'a> {
    pub category: Option<&'a Channel>,
    pub channels: Vec<&'a Channel>,
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

        let id = str_of(v, "id");
        let icon_url = v
            .get("icon")
            .and_then(Value::as_str)
            .or_else(|| v.pointer("/properties/icon").and_then(Value::as_str))
            .map(|hash| format!("{CDN}/icons/{id}/{hash}.png?size=64"));

        let mut guild = Self {
            id,
            name,
            icon_url,
            channels: v
                .get("channels")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().map(Channel::from_json).collect())
                .unwrap_or_default(),
        };
        guild.sort_channels();
        guild
    }

    /// The pseudo-guild that holds DMs and group DMs.
    pub fn dms(channels: Vec<Channel>) -> Self {
        let mut guild = Self {
            id: DM_GUILD_ID.into(),
            name: "Direct Messages".into(),
            icon_url: None,
            channels,
        };
        guild.sort_channels();
        guild
    }

    pub fn is_dms(&self) -> bool {
        self.id == DM_GUILD_ID
    }

    pub fn sort_channels(&mut self) {
        if self.is_dms() {
            // Most recent conversation first.
            self.channels
                .sort_by_key(|c| std::cmp::Reverse(c.last_message_id));
        } else {
            self.channels
                .sort_by_key(|c| (c.position, c.id.parse::<u64>().unwrap_or(0)));
        }
    }

    pub fn upsert_channel(&mut self, channel: Channel) {
        match self.channels.iter_mut().find(|c| c.id == channel.id) {
            Some(existing) => *existing = channel,
            None => self.channels.push(channel),
        }
        self.sort_channels();
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Text channels grouped the way Discord shows them: uncategorised
    /// channels first, then each category in order.
    pub fn channel_groups(&self) -> Vec<ChannelGroup<'_>> {
        if self.is_dms() {
            return vec![ChannelGroup {
                category: None,
                channels: self.channels.iter().filter(|c| c.is_text()).collect(),
            }];
        }

        let mut groups = vec![ChannelGroup {
            category: None,
            channels: self
                .channels
                .iter()
                .filter(|c| c.is_text() && c.parent_id.is_none())
                .collect(),
        }];
        for cat in self.channels.iter().filter(|c| c.kind == CHANNEL_CATEGORY) {
            let channels: Vec<&Channel> = self
                .channels
                .iter()
                .filter(|c| c.is_text() && c.parent_id.as_deref() == Some(cat.id.as_str()))
                .collect();
            if !channels.is_empty() {
                groups.push(ChannelGroup {
                    category: Some(cat),
                    channels,
                });
            }
        }
        groups.retain(|g| !g.channels.is_empty());
        groups
    }
}

impl Channel {
    pub fn from_json(v: &Value) -> Self {
        let recipients: Vec<User> = v
            .get("recipients")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(User::from_json).collect())
            .unwrap_or_default();
        let kind = v.get("type").and_then(Value::as_u64).unwrap_or(u64::MAX);

        // DMs have no name; group DMs may or may not.
        let mut name = str_of(v, "name");
        if name.is_empty() && (kind == CHANNEL_DM || kind == CHANNEL_GROUP_DM) {
            name = recipients
                .iter()
                .map(|u| u.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if name.is_empty() {
                name = "Direct message".into();
            }
        }

        Self {
            id: str_of(v, "id"),
            name,
            kind,
            position: v.get("position").and_then(Value::as_i64).unwrap_or(0),
            parent_id: v
                .get("parent_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            recipients,
            last_message_id: snowflake(v, "last_message_id").unwrap_or(0),
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(
            self.kind,
            CHANNEL_TEXT | CHANNEL_ANNOUNCEMENT | CHANNEL_DM | CHANNEL_GROUP_DM
        )
    }

    pub fn is_dm(&self) -> bool {
        self.kind == CHANNEL_DM || self.kind == CHANNEL_GROUP_DM
    }

    /// Label shown on the channel tab.
    pub fn label(&self) -> String {
        match self.kind {
            CHANNEL_DM => format!("@ {}", self.name),
            CHANNEL_GROUP_DM => format!("👥 {}", self.name),
            _ => format!("# {}", self.name),
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Attachment {
    pub url: String,
    pub proxy_url: String,
    pub filename: String,
    pub content_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: u64,
}

impl Attachment {
    pub fn from_json(v: &Value) -> Self {
        Self {
            url: str_of(v, "url"),
            proxy_url: str_of(v, "proxy_url"),
            filename: str_of(v, "filename"),
            content_type: str_of(v, "content_type"),
            width: v.get("width").and_then(Value::as_u64).map(|w| w as u32),
            height: v.get("height").and_then(Value::as_u64).map(|h| h as u32),
            size: v.get("size").and_then(Value::as_u64).unwrap_or(0),
        }
    }

    pub fn is_image(&self) -> bool {
        if self.content_type.starts_with("image/") {
            return true;
        }
        let name = self.filename.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp"]
            .iter()
            .any(|ext| name.ends_with(ext))
    }

    /// A resized copy from Discord's media proxy so we don't download
    /// full-size images just to show a thumbnail.
    pub fn thumbnail_url(&self, max_w: u32, max_h: u32) -> String {
        let base = if self.proxy_url.is_empty() {
            &self.url
        } else {
            &self.proxy_url
        };
        let (Some(w), Some(h)) = (self.width, self.height) else {
            return base.clone();
        };
        let scale = (max_w as f32 / w as f32)
            .min(max_h as f32 / h as f32)
            .min(1.0);
        let (tw, th) = (
            (w as f32 * scale).round().max(1.0),
            (h as f32 * scale).round().max(1.0),
        );
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}width={tw}&height={th}")
    }

    /// Size the image will be shown at, keeping aspect ratio.
    pub fn display_size(&self, max_w: f32, max_h: f32) -> Option<(f32, f32)> {
        let (w, h) = (self.width? as f32, self.height? as f32);
        let scale = (max_w / w).min(max_h / h).min(1.0);
        Some((w * scale, h * scale))
    }

    pub fn human_size(&self) -> String {
        let b = self.size as f64;
        if b >= 1_048_576.0 {
            format!("{:.1} MB", b / 1_048_576.0)
        } else if b >= 1024.0 {
            format!("{:.0} KB", b / 1024.0)
        } else {
            format!("{b} B")
        }
    }
}

/// Short preview of the message being replied to.
#[derive(Debug, Clone)]
pub struct ReplyPreview {
    pub author: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    pub content: String,
    pub timestamp: String,
    pub edited: bool,
    pub attachments: Vec<Attachment>,
    pub reply: Option<ReplyPreview>,
}

impl Message {
    pub fn from_json(v: &Value) -> Self {
        let reply = v
            .get("referenced_message")
            .filter(|r| r.is_object())
            .map(|r| {
                let author = User::from_json(&r["author"]).name;
                let mut content = str_of(r, "content").replace('\n', " ");
                if content.chars().count() > 80 {
                    content = content.chars().take(80).collect::<String>() + "…";
                }
                if content.is_empty() {
                    content = "(attachment)".into();
                }
                ReplyPreview { author, content }
            });

        Self {
            id: str_of(v, "id"),
            channel_id: str_of(v, "channel_id"),
            author: User::from_json(&v["author"]),
            content: str_of(v, "content"),
            timestamp: str_of(v, "timestamp"),
            edited: v.get("edited_timestamp").is_some_and(|t| !t.is_null()),
            attachments: parse_attachments(v).unwrap_or_default(),
            reply,
        }
    }

    /// "HH:MM" from Discord's ISO-8601 timestamp (UTC).
    pub fn short_time(&self) -> &str {
        self.timestamp.get(11..16).unwrap_or("")
    }

    /// Apply a MESSAGE_UPDATE. Discord sends partial objects, so only
    /// overwrite fields that are present.
    pub fn apply_patch(&mut self, patch: &MessagePatch) {
        if let Some(content) = &patch.content {
            self.content = content.clone();
        }
        if let Some(attachments) = &patch.attachments {
            self.attachments = attachments.clone();
        }
        if patch.edited {
            self.edited = true;
        }
    }
}

fn parse_attachments(v: &Value) -> Option<Vec<Attachment>> {
    v.get("attachments")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(Attachment::from_json).collect())
}

/// Partial message from MESSAGE_UPDATE.
#[derive(Debug, Clone)]
pub struct MessagePatch {
    pub id: String,
    pub channel_id: String,
    pub content: Option<String>,
    pub attachments: Option<Vec<Attachment>>,
    pub edited: bool,
}

impl MessagePatch {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            channel_id: str_of(v, "channel_id"),
            content: v.get("content").and_then(Value::as_str).map(str::to_string),
            attachments: parse_attachments(v),
            edited: v.get("edited_timestamp").is_some_and(|t| !t.is_null()),
        }
    }
}

// ---------------------------------------------------------------------------
// UI <-> backend messages
// ---------------------------------------------------------------------------

/// Events the backend sends to the UI.
#[derive(Debug)]
pub enum ToUi {
    Status(String),
    /// The session can't continue (e.g. invalid token). The UI logs out.
    Fatal(String),
    Ready {
        me: User,
        guilds: Vec<Guild>,
        dms: Vec<Channel>,
    },
    GuildAdded(Guild),
    /// A channel was created or changed. `guild_id` is None for DMs.
    ChannelUpserted {
        guild_id: Option<String>,
        channel: Channel,
    },
    History {
        channel_id: String,
        messages: Vec<Message>,
    },
    /// Files chosen in the "+" file picker.
    FilesPicked(Vec<PathBuf>),
    NewMessage(Message),
    MessageUpdated(MessagePatch),
    MessageDeleted {
        channel_id: String,
        message_id: String,
    },
    Typing {
        channel_id: String,
        user_id: String,
        /// Present in servers (from the member object); None in DMs.
        name: Option<String>,
    },
}

/// Requests the UI sends to the backend.
#[derive(Debug)]
pub enum ToBackend {
    LoadHistory {
        channel_id: String,
    },
    Send {
        channel_id: String,
        content: String,
        reply_to: Option<String>,
        /// Files to upload with the message.
        files: Vec<PathBuf>,
    },
    /// Open the system file picker (runs off the UI thread).
    PickFiles,
    Edit {
        channel_id: String,
        message_id: String,
        content: String,
    },
    Delete {
        channel_id: String,
        message_id: String,
    },
    /// Show "<you> is typing…" to others. Only sent while the user types.
    Typing {
        channel_id: String,
    },
}
