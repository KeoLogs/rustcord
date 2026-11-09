//! Plain data types shared between the Discord backend and the UI.
//!
//! These are parsed by hand from `serde_json::Value` rather than with strict
//! `#[derive(Deserialize)]` structs, because Discord's payloads differ between
//! bot and user accounts (and change over time). Lenient parsing means a
//! missing field gives an empty value instead of crashing the client.

use std::path::PathBuf;

use serde_json::Value;

use crate::members::{CachedMember, ListUpdate, Member, Role};
use crate::notify::GuildSettings;
use crate::presence::Presence;
use crate::profile::Profile;
use crate::read_state::ReadStateEntry;

/// Discord channel types we care about.
pub const CHANNEL_TEXT: u64 = 0;
pub const CHANNEL_DM: u64 = 1;
pub const CHANNEL_GROUP_DM: u64 = 3;
pub const CHANNEL_CATEGORY: u64 = 4;
pub const CHANNEL_ANNOUNCEMENT: u64 = 5;
pub const CHANNEL_VOICE: u64 = 2;
pub const CHANNEL_STAGE: u64 = 13;

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
    /// default_message_notifications: 0 all messages, 1 only mentions.
    pub default_notifications: u8,
    /// My role ids in this server (for role mentions). Filled in by the gateway.
    pub my_roles: Vec<String>,
    /// The server's roles, highest first (name colours, member list groups).
    pub roles: Vec<Role>,
    /// Server owner (has every permission).
    pub owner_id: String,
    /// Custom emoji (for the composer's `:` autocomplete).
    pub emojis: Vec<GuildEmoji>,
}

/// A server's custom emoji.
#[derive(Debug, Clone)]
pub struct GuildEmoji {
    pub id: String,
    pub name: String,
    pub animated: bool,
}

impl GuildEmoji {
    fn parse_list(v: &Value) -> Vec<Self> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|e| e["available"].as_bool() != Some(false))
                    .filter_map(|e| {
                        Some(Self {
                            id: e["id"].as_str()?.to_string(),
                            name: e["name"].as_str()?.to_string(),
                            animated: e["animated"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What goes into the message: `<:name:id>` / `<a:name:id>`.
    pub fn markup(&self) -> String {
        let a = if self.animated { "a" } else { "" };
        format!("<{a}:{}:{}>", self.name, self.id)
    }
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
    /// Channel topic (shown in the chat header). Empty if none.
    pub topic: String,
    /// Permission overwrites (servers only).
    pub overwrites: Vec<crate::perms::Overwrite>,
}

/// A category header (if any) and the text and voice channels under it.
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

        let default_notifications = v
            .get("default_message_notifications")
            .or_else(|| v.pointer("/properties/default_message_notifications"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(1) as u8;

        let mut guild = Self {
            id,
            name,
            icon_url,
            default_notifications,
            my_roles: Vec::new(),
            roles: Role::parse_list(
                v.get("roles")
                    .or_else(|| v.pointer("/properties/roles"))
                    .unwrap_or(&Value::Null),
            ),
            owner_id: v
                .get("owner_id")
                .or_else(|| v.pointer("/properties/owner_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            emojis: GuildEmoji::parse_list(
                v.get("emojis")
                    .or_else(|| v.pointer("/properties/emojis"))
                    .unwrap_or(&Value::Null),
            ),
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
            default_notifications: 0,
            my_roles: Vec::new(),
            roles: Vec::new(),
            owner_id: String::new(),
            emojis: Vec::new(),
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

    /// Text and voice channels grouped the way Discord shows them:
    /// uncategorised channels first, then each category in order. Within a
    /// group, voice channels come after text channels (like Discord).
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
                .filter(|c| c.is_listed() && c.parent_id.is_none())
                .collect(),
        }];
        for cat in self.channels.iter().filter(|c| c.kind == CHANNEL_CATEGORY) {
            let channels: Vec<&Channel> = self
                .channels
                .iter()
                .filter(|c| c.is_listed() && c.parent_id.as_deref() == Some(cat.id.as_str()))
                .collect();
            if !channels.is_empty() {
                groups.push(ChannelGroup {
                    category: Some(cat),
                    channels,
                });
            }
        }
        groups.retain(|g| !g.channels.is_empty());
        for g in &mut groups {
            // Stable sort: keeps position order within text and within voice.
            g.channels.sort_by_key(|c| c.is_voice());
        }
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
            topic: str_of(v, "topic").trim().to_string(),
            overwrites: crate::perms::Overwrite::parse_list(&v["permission_overwrites"]),
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

    /// Voice and stage channels (listed, but not joinable yet).
    pub fn is_voice(&self) -> bool {
        self.kind == CHANNEL_VOICE || self.kind == CHANNEL_STAGE
    }

    /// Shown in the channel list.
    fn is_listed(&self) -> bool {
        self.is_text() || self.is_voice()
    }

    /// Icon in front of the name in the channel list and chat header.
    pub fn icon(&self) -> &'static str {
        match self.kind {
            CHANNEL_DM => "@",
            CHANNEL_GROUP_DM => "👥",
            CHANNEL_ANNOUNCEMENT => "📢",
            CHANNEL_VOICE => "🔊",
            CHANNEL_STAGE => "📡",
            _ => "#",
        }
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
    pub id: String,
    pub author: String,
    pub author_id: String,
    pub avatar_url: String,
    /// Raw (markdown) content, cut to a few hundred characters; the UI turns
    /// it into one line of plain text. Empty: attachment / embed only.
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    pub content: String,
    /// When it was sent, unix seconds (0 if unknown). See `timefmt`.
    pub time: i64,
    pub edited: bool,
    pub attachments: Vec<Attachment>,
    pub reply: Option<ReplyPreview>,
    /// Server the message is in (gateway events only; None for DMs/history).
    pub guild_id: Option<String>,
    /// Ids of mentioned users.
    pub mentions: Vec<String>,
    /// (id, display name) of mentioned users, for showing `<@id>` mentions.
    pub mention_names: Vec<(String, String)>,
    pub mention_roles: Vec<String>,
    /// @everyone / @here.
    pub mention_everyone: bool,
    /// The author's server member info (MESSAGE_CREATE in servers). The UI
    /// moves it into the member cache.
    pub member: Option<Box<CachedMember>>,
    /// Link previews and bot embeds (see `embed.rs`).
    pub embeds: Vec<crate::embed::Embed>,
    pub pinned: bool,
}

impl Message {
    pub fn from_json(v: &Value) -> Self {
        let reply = v
            .get("referenced_message")
            .filter(|r| r.is_object())
            .map(|r| {
                let author = User::from_json(&r["author"]);
                let mut content = str_of(r, "content");
                if content.chars().count() > 300 {
                    content = content.chars().take(300).collect::<String>() + "…";
                }
                ReplyPreview {
                    id: str_of(r, "id"),
                    author: author.name,
                    author_id: author.id,
                    avatar_url: author.avatar_url,
                    content,
                }
            });

        Self {
            id: str_of(v, "id"),
            channel_id: str_of(v, "channel_id"),
            author: User::from_json(&v["author"]),
            content: str_of(v, "content"),
            time: crate::timefmt::parse_iso(v["timestamp"].as_str().unwrap_or_default()),
            edited: v.get("edited_timestamp").is_some_and(|t| !t.is_null()),
            attachments: parse_attachments(v).unwrap_or_default(),
            reply,
            guild_id: v
                .get("guild_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            mentions: v
                .get("mentions")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|u| u.get("id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            mention_names: v
                .get("mentions")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter(|u| u.is_object())
                        .map(|u| {
                            let u = User::from_json(u);
                            (u.id, u.name)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            mention_roles: v
                .get("mention_roles")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            mention_everyone: v
                .get("mention_everyone")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            member: v.get("member").filter(|m| m.is_object()).map(|m| {
                let m = Member::from_json(m);
                Box::new(CachedMember {
                    nick: m.nick,
                    roles: m.roles,
                    joined_at: m.joined_at,
                })
            }),
            embeds: crate::embed::parse_embeds(v),
            pinned: v.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    /// `https://discord.com/channels/<guild or @me>/<channel>/<message>`.
    pub fn link(&self, guild_id: Option<&str>) -> String {
        format!(
            "https://discord.com/channels/{}/{}/{}",
            guild_id.unwrap_or(DM_GUILD_ID),
            self.channel_id,
            self.id
        )
    }

    /// "HH:MM" in local time.
    pub fn short_time(&self) -> String {
        crate::timefmt::clock(self.time)
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
        if let Some(embeds) = &patch.embeds {
            self.embeds = embeds.clone();
        }
        if let Some(p) = patch.pinned {
            self.pinned = p;
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
    /// New embeds (link previews arrive this way a moment after the message).
    pub embeds: Option<Vec<crate::embed::Embed>>,
    pub pinned: Option<bool>,
}

impl MessagePatch {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            channel_id: str_of(v, "channel_id"),
            content: v.get("content").and_then(Value::as_str).map(str::to_string),
            attachments: parse_attachments(v),
            edited: v.get("edited_timestamp").is_some_and(|t| !t.is_null()),
            embeds: crate::embed::embeds_patch(v),
            pinned: v.get("pinned").and_then(Value::as_bool),
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
        /// User accounts only (bots get an empty list).
        read_state: Vec<ReadStateEntry>,
        /// Mute / notification settings (user accounts only).
        guild_settings: Vec<GuildSettings>,
        /// Users I blocked (READY `relationships` with type 2).
        blocked: Vec<String>,
        /// Synced settings if READY had them (`user_settings_proto`).
        user_settings: Option<Box<crate::settings_proto::UserSettings>>,
    },
    /// The gateway resumed the old session; missed events are replayed.
    Resumed,
    /// A channel was read (MESSAGE_ACK), e.g. on another device.
    MessageAck(ReadStateEntry),
    /// New last_message_id for channels (PASSIVE_UPDATE_V1).
    LastMessages(Vec<(String, u64)>),
    /// USER_GUILD_SETTINGS_UPDATE: mute / notification settings changed.
    GuildSettingsUpdated(GuildSettings),
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
    NewMessage(Box<Message>),
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
    /// Online status / activity changes (PRESENCE_UPDATE, READY_SUPPLEMENTAL,
    /// GUILD_MEMBERS_CHUNK presences).
    Presences(Vec<(String, Presence)>),
    /// GUILD_MEMBER_LIST_UPDATE (user accounts, after op 14).
    MemberList(ListUpdate),
    /// Server members (GUILD_MEMBERS_CHUNK, READY_SUPPLEMENTAL, GUILD_CREATE).
    /// `full_list`: answer to a bot's whole-list request (op 8 with query).
    Members {
        guild_id: String,
        members: Vec<Member>,
        full_list: bool,
    },
    RoleUpserted {
        guild_id: String,
        role: Role,
    },
    RoleDeleted {
        guild_id: String,
        role_id: String,
    },
    Profile(Box<Profile>),
    /// The profile couldn't be loaded (bot session, 403/404, network).
    ProfileUnavailable(String),
    /// A user's note (GET answer, USER_NOTE_UPDATE). Empty = no note.
    Note {
        user_id: String,
        note: String,
    },
    /// I left a server, or it went away (GUILD_DELETE without `unavailable`).
    GuildRemoved(String),
    /// Messages around `message_id` (jump to a pinned message).
    HistoryAround {
        channel_id: String,
        message_id: String,
        messages: Vec<Message>,
    },
    /// Pinned messages of a channel (raw message objects).
    Pins {
        channel_id: String,
        pins: Vec<serde_json::Value>,
    },
    PinsFailed {
        channel_id: String,
        error: String,
    },
    /// A pin / unpin went through.
    PinChanged {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },
    PinFailed {
        channel_id: String,
        message_id: String,
        error: String,
    },
    /// CHANNEL_PINS_UPDATE.
    PinsUpdated(String),
    /// My synced settings (READY, GET or PATCH answer): full message.
    UserSettings(Box<crate::settings_proto::UserSettings>),
    /// USER_SETTINGS_PROTO_UPDATE payload (`d`).
    UserSettingsUpdate(serde_json::Value),
    /// A short message for the user (shown briefly), e.g. "Saved …".
    Notice(String),
    /// RELATIONSHIP_ADD / RELATIONSHIP_REMOVE for blocked users.
    Blocked {
        user_id: String,
        blocked: bool,
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
        /// Ping the author of the replied-to message (allowed_mentions
        /// `replied_user`).
        reply_ping: bool,
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
    /// Mark a channel read up to `message_id` (user opened / is viewing it).
    Ack {
        channel_id: String,
        message_id: String,
    },
    /// "Mark unread": manual ack of the message before the one clicked.
    MarkUnread {
        channel_id: String,
        message_id: String,
        mention_count: u32,
    },
    /// "Mark as read" on a server: (channel_id, message_id) pairs.
    AckBulk(Vec<(String, String)>),
    /// Show a desktop notification (Notifications module).
    Notify {
        title: String,
        body: String,
    },
    /// Op 14: member list for a channel (user accounts; bots ignore it).
    SubscribeMembers {
        guild_id: String,
        channel_id: String,
        ranges: Vec<(u32, u32)>,
    },
    /// Op 8 with user ids: roles/nicks of these people (message authors).
    RequestMembers {
        guild_id: String,
        user_ids: Vec<String>,
    },
    /// Op 8 for a whole server (bots with the Server Members intent only).
    RequestMemberList {
        guild_id: String,
    },
    /// GET users/{id}/profile (user accounts).
    LoadProfile {
        user_id: String,
        guild_id: Option<String>,
    },
    /// GET users/@me/notes/{id}.
    LoadNote {
        user_id: String,
    },
    /// PUT users/@me/notes/{id}. Only sent when the user presses Save.
    SetNote {
        user_id: String,
        note: String,
    },
    /// GET messages around one (jump to a pin that isn't loaded).
    LoadAround {
        channel_id: String,
        message_id: String,
    },
    /// GET channels/{id}/pins.
    LoadPins {
        channel_id: String,
    },
    /// Pin (`true`) or unpin a message. Only from a confirmed menu choice.
    SetPinned {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },
    /// GET users/@me/settings-proto/1 (user accounts, after READY).
    LoadUserSettings,
    /// PATCH users/@me/settings-proto/1. Only from an explicit user action.
    PatchUserSettings {
        settings_b64: String,
    },
    /// Op 3 presence update (after the user picked a status).
    SetPresence {
        status: String,
        custom: Option<crate::settings_proto::CustomStatus>,
    },
    /// DELETE users/@me/guilds/{id}. Only after the confirmation dialog.
    LeaveGuild {
        guild_id: String,
    },
    /// "Save image": ask where, then download it there.
    SaveFile {
        url: String,
        filename: String,
    },
}
