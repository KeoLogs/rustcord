//! Discord's synced user settings (`PreloadedUserSettings` protobuf).
//!
//! User accounts store their account-wide settings (status, custom status,
//! server folders, DM safety, compact mode, theme...) as one protobuf message.
//! It arrives base64-encoded in two places:
//!
//! - READY: `d.user_settings_proto` (full message), or
//!   `GET users/@me/settings-proto/1` -> `{"settings": "<b64>"}`.
//! - `USER_SETTINGS_PROTO_UPDATE`: `d.settings.type` (1 = preloaded settings),
//!   `d.settings.proto` (b64) and `d.partial` (true = only the top-level
//!   sub-messages that changed are included; each replaces the old one).
//!
//! Changes go back with `PATCH users/@me/settings-proto/1` and the body
//! `{"settings": "<b64 PreloadedUserSettings>"}` holding only the top-level
//! sub-messages that changed. The server replaces each sent top-level
//! sub-message as a whole, so a patch must carry the *entire* sub-message.
//! We only model a handful of fields, so patches are built by splicing the
//! changed field into the original raw bytes of that sub-message: fields we
//! don't understand are sent back untouched instead of being reset.
//!
//! Only ever build a patch from an explicit user action (no self-bot
//! behaviour; see CLAUDE.md).
//!
//! Field numbers relied on (from the public `discord-protos`
//! `PreloadedUserSettings.proto`, cross-checked against Discord Messenger's
//! `SettingsManager.hpp` where it uses them; "DM" = also used there):
//!
//! ```text
//! PreloadedUserSettings
//!   6  text_and_images: TextAndImagesSettings                       (DM)
//!        17 message_display_compact: BoolValue                      (DM)
//!        19 explicit_content_filter: UInt32Value 0 off/1 non-friends/2 all (DM)
//!   8  privacy: PrivacySettings                                     (DM)
//!        3  restricted_guild_ids: repeated fixed64 (packed)         (DM)
//!        4  default_guilds_restricted: bool                         (DM)
//!   11 status: StatusSettings                                       (DM)
//!        1  status: StringValue "online"/"idle"/"dnd"/"invisible"   (DM)
//!        2  custom_status: CustomStatus                             (DM)
//!             1 text: string                                        (DM)
//!             2 emoji_id: fixed64                                   (protos only)
//!             3 emoji_name: string                                  (DM)
//!             4 expires_at_ms: fixed64                              (DM)
//!             5 created_at_ms: fixed64                              (protos only)
//!   13 appearance: AppearanceSettings                               (protos only)
//!        1  theme: enum 0 unset/1 dark/2 light/3 darker/4 midnight
//!   14 guild_folders: GuildFolders                                  (DM)
//!        1  folders: repeated GuildFolder                           (DM)
//!             1 guild_ids: repeated fixed64 (packed)                (DM)
//!             2 id: Int64Value (absent = loose server)              (DM)
//!             3 name: StringValue                                   (DM)
//!             4 color: UInt64Value (0xRRGGBB)                       (protos only)
//!        2  guild_positions: repeated fixed64                       (protos only)
//! ```
//!
//! Each top-level sub-message is decoded on its own, so if Discord ever
//! changes one of these fields' wire type only that section is lost (and
//! noted in [`UserSettings::warnings`]); the rest still decodes.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use prost::Message as _;
use serde_json::Value;

/// `d.settings.type` of a `USER_SETTINGS_PROTO_UPDATE` that carries
/// `PreloadedUserSettings` (type 2 is frecency, 3 test settings).
pub const PRELOADED_SETTINGS_TYPE: u64 = 1;

// Top-level field numbers of PreloadedUserSettings.
const TEXT_AND_IMAGES: u32 = 6;
const PRIVACY: u32 = 8;
const STATUS: u32 = 11;
const APPEARANCE: u32 = 13;
const GUILD_FOLDERS: u32 = 14;

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// Online status as stored in the settings (what the status picker shows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Online,
    Idle,
    Dnd,
    Invisible,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Online => "online",
            Status::Idle => "idle",
            Status::Dnd => "dnd",
            Status::Invisible => "invisible",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "online" => Some(Status::Online),
            "idle" => Some(Status::Idle),
            "dnd" => Some(Status::Dnd),
            "invisible" => Some(Status::Invisible),
            _ => None,
        }
    }
}

/// Custom status ("What's happening?").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomStatus {
    pub text: String,
    /// Unicode emoji, or the name of a custom emoji.
    pub emoji_name: Option<String>,
    /// Custom (server) emoji id; `None` for unicode emoji.
    pub emoji_id: Option<u64>,
    /// Unix ms when it clears itself; `None` = never.
    pub expires_at_ms: Option<u64>,
    /// Unix ms when it was set (filled in with "now" when patching if `None`).
    pub created_at_ms: Option<u64>,
}

impl CustomStatus {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|t| t <= now_ms)
    }
}

/// "Keep me safe" / explicit image filter for DMs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplicitFilter {
    /// Don't scan direct messages.
    Off,
    /// Scan DMs from people who aren't friends.
    NonFriends,
    /// Scan all DMs.
    Everyone,
    /// A value we don't know about (kept so we don't misreport it).
    Other(u32),
}

impl ExplicitFilter {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => ExplicitFilter::Off,
            1 => ExplicitFilter::NonFriends,
            2 => ExplicitFilter::Everyone,
            n => ExplicitFilter::Other(n),
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            ExplicitFilter::Off => 0,
            ExplicitFilter::NonFriends => 1,
            ExplicitFilter::Everyone => 2,
            ExplicitFilter::Other(n) => n,
        }
    }
}

/// Discord client theme (read-only here; RustCord has its own palettes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
    Darker,
    Midnight,
    Other(i32),
}

impl Theme {
    fn from_i32(v: i32) -> Option<Theme> {
        match v {
            0 => None,
            1 => Some(Theme::Dark),
            2 => Some(Theme::Light),
            3 => Some(Theme::Darker),
            4 => Some(Theme::Midnight),
            n => Some(Theme::Other(n)),
        }
    }
}

/// One entry of the server rail as stored by Discord. Entries without an `id`
/// are loose servers (normally exactly one guild id each).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildFolder {
    pub id: Option<i64>,
    pub name: Option<String>,
    /// 0xRRGGBB
    pub color: Option<u64>,
    pub guild_ids: Vec<u64>,
}

/// What the server rail should draw, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RailItem {
    Guild(String),
    Folder {
        id: i64,
        name: Option<String>,
        /// 0xRRGGBB
        color: Option<u32>,
        guild_ids: Vec<String>,
    },
}

/// The settings we understand, plus the raw message they came from.
#[derive(Clone, Debug, PartialEq)]
pub struct UserSettings {
    /// `None` when never set (Discord treats that as online).
    pub status: Option<Status>,
    pub custom_status: Option<CustomStatus>,
    /// Server rail entries, in order (folders and loose servers).
    pub folders: Vec<GuildFolder>,
    /// Older flat server order; only used when `folders` is empty.
    pub guild_positions: Vec<u64>,
    /// `None` = never set (Discord picks a default based on account age).
    pub explicit_content_filter: Option<ExplicitFilter>,
    /// "Allow direct messages from server members" default for new servers.
    pub allow_dms_from_guild_members: bool,
    /// Servers where DMs from members are blocked (per-server overrides).
    pub restricted_guild_ids: Vec<u64>,
    /// `None` = never set (cozy).
    pub compact_mode: Option<bool>,
    pub theme: Option<Theme>,
    /// Sections that failed to decode (their fields are left at defaults).
    pub warnings: Vec<&'static str>,
    raw: Vec<u8>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            status: None,
            custom_status: None,
            folders: Vec::new(),
            guild_positions: Vec::new(),
            explicit_content_filter: None,
            allow_dms_from_guild_members: true,
            restricted_guild_ids: Vec::new(),
            compact_mode: None,
            theme: None,
            warnings: Vec::new(),
            raw: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

impl UserSettings {
    /// Decode a full base64 `PreloadedUserSettings`.
    pub fn from_base64(b64: &str) -> Result<Self> {
        Self::decode(decode_b64(b64)?)
    }

    /// Decode a full `PreloadedUserSettings` message.
    pub fn decode(raw: Vec<u8>) -> Result<Self> {
        // Validate the top-level framing first; a bad message is an error,
        // a bad section is only a warning.
        fields(&raw).context("settings proto is malformed")?;
        let mut s = Self {
            raw,
            ..Self::default()
        };
        s.refresh();
        Ok(s)
    }

    /// From a READY payload's `d` (`user_settings_proto`).
    pub fn from_ready(ready: &Value) -> Result<Self> {
        let Some(b64) = ready.get("user_settings_proto").and_then(Value::as_str) else {
            bail!("READY has no user_settings_proto");
        };
        Self::from_base64(b64)
    }

    /// From the JSON returned by `GET`/`PATCH users/@me/settings-proto/1`
    /// (`{"settings": "<b64>"}`, always the full message).
    pub fn from_rest(resp: &Value) -> Result<Self> {
        let Some(b64) = resp.get("settings").and_then(Value::as_str) else {
            bail!("settings-proto response has no settings");
        };
        Self::from_base64(b64)
    }

    /// Apply a `USER_SETTINGS_PROTO_UPDATE` payload's `d`. Returns `Ok(false)`
    /// for other settings types (frecency etc.), which are ignored.
    pub fn apply_update_event(&mut self, d: &Value) -> Result<bool> {
        let settings = &d["settings"];
        if settings["type"].as_u64() != Some(PRELOADED_SETTINGS_TYPE) {
            return Ok(false);
        }
        let Some(b64) = settings["proto"].as_str() else {
            bail!("USER_SETTINGS_PROTO_UPDATE has no proto");
        };
        let partial = d["partial"].as_bool().unwrap_or(false);
        self.merge_base64(b64, partial)?;
        Ok(true)
    }

    /// Merge an encoded message: `partial` replaces only the top-level
    /// sub-messages it contains; otherwise it replaces everything.
    pub fn merge_base64(&mut self, b64: &str, partial: bool) -> Result<()> {
        let update = decode_b64(b64)?;
        let update_fields = fields(&update).context("settings update is malformed")?;
        if !partial {
            self.raw = update;
        } else {
            let tags: Vec<u32> = update_fields.iter().map(|f| f.tag).collect();
            let mut raw = without_tags(&self.raw, &tags);
            raw.extend_from_slice(&update);
            self.raw = raw;
        }
        self.refresh();
        Ok(())
    }

    /// Optimistically apply one of our own patches locally (the server's
    /// response / USER_SETTINGS_PROTO_UPDATE is still the source of truth).
    pub fn apply_patch(&mut self, patch_b64: &str) -> Result<()> {
        self.merge_base64(patch_b64, true)
    }

    /// The raw `PreloadedUserSettings` bytes everything above came from.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Re-derive the typed fields from `raw`.
    fn refresh(&mut self) {
        let raw = std::mem::take(&mut self.raw);
        *self = Self {
            raw: Vec::new(),
            ..Self::default()
        };
        let mut warnings = Vec::new();

        if let Some(t) = section::<PbStatusSettings>(&raw, STATUS, "status", &mut warnings) {
            self.status = t.status.and_then(|s| Status::parse(&s.value));
            self.custom_status = t.custom_status.map(CustomStatus::from);
        }
        if let Some(t) =
            section::<PbTextAndImages>(&raw, TEXT_AND_IMAGES, "text_and_images", &mut warnings)
        {
            self.compact_mode = t.message_display_compact.map(|v| v.value);
            self.explicit_content_filter = t
                .explicit_content_filter
                .map(|v| ExplicitFilter::from_u32(v.value));
        }
        if let Some(t) = section::<PbPrivacy>(&raw, PRIVACY, "privacy", &mut warnings) {
            self.allow_dms_from_guild_members = !t.default_guilds_restricted;
            self.restricted_guild_ids = t.restricted_guild_ids;
        }
        if let Some(t) = section::<PbAppearance>(&raw, APPEARANCE, "appearance", &mut warnings) {
            self.theme = Theme::from_i32(t.theme);
        }
        if let Some(t) =
            section::<PbGuildFolders>(&raw, GUILD_FOLDERS, "guild_folders", &mut warnings)
        {
            self.folders = t.folders.into_iter().map(GuildFolder::from).collect();
            self.guild_positions = t.guild_positions;
        }

        self.warnings = warnings;
        self.raw = raw;
    }
}

fn decode_b64(b64: &str) -> Result<Vec<u8>> {
    B64.decode(b64.trim())
        .context("settings proto is not valid base64")
}

/// Decode every occurrence of top-level field `tag` as one message
/// (protobuf merge semantics: concatenated encodings merge).
fn section<M: prost::Message + Default>(
    raw: &[u8],
    tag: u32,
    name: &'static str,
    warnings: &mut Vec<&'static str>,
) -> Option<M> {
    let bytes = top_level_payload(raw, tag)?;
    match M::decode(bytes.as_slice()) {
        Ok(m) => Some(m),
        Err(_) => {
            warnings.push(name);
            None
        }
    }
}

/// Concatenated payloads of all length-delimited top-level fields `tag`.
fn top_level_payload(raw: &[u8], tag: u32) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    for f in fields(raw).ok()? {
        if f.tag == tag
            && let Some(p) = f.payload
        {
            out.get_or_insert_with(Vec::new).extend_from_slice(&raw[p]);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Encoding partial updates
// ---------------------------------------------------------------------------

impl UserSettings {
    /// Patch: set my status.
    pub fn patch_status(&self, status: Status) -> String {
        let field = PbStatusSettings {
            status: Some(StringValue {
                value: status.as_str().to_string(),
            }),
            ..Default::default()
        };
        self.patch(STATUS, &[1], &field.encode_to_vec())
    }

    /// Patch: set (`Some`) or clear (`None`) my custom status. An empty text
    /// with no emoji also clears it, like Discord's client.
    pub fn patch_custom_status(&self, custom: Option<&CustomStatus>) -> String {
        let custom = custom.filter(|c| !c.text.is_empty() || c.emoji_name.is_some());
        let field = PbStatusSettings {
            custom_status: custom.map(|c| PbCustomStatus {
                text: c.text.clone(),
                emoji_id: c.emoji_id.unwrap_or(0),
                emoji_name: c.emoji_name.clone().unwrap_or_default(),
                expires_at_ms: c.expires_at_ms.unwrap_or(0),
                created_at_ms: c.created_at_ms.unwrap_or_else(now_ms),
            }),
            ..Default::default()
        };
        self.patch(STATUS, &[2], &field.encode_to_vec())
    }

    /// Patch: set the server rail (folders + loose servers) in this order.
    /// Folder ids must be unique; see [`new_folder_id`].
    pub fn patch_guild_folders(&self, rail: &[RailItem]) -> String {
        let mut folders = Vec::with_capacity(rail.len());
        let mut positions = Vec::new();
        for item in rail {
            match item {
                RailItem::Guild(id) => {
                    let Ok(id) = id.parse::<u64>() else { continue };
                    positions.push(id);
                    folders.push(PbGuildFolder {
                        guild_ids: vec![id],
                        ..Default::default()
                    });
                }
                RailItem::Folder {
                    id,
                    name,
                    color,
                    guild_ids,
                } => {
                    let ids: Vec<u64> = guild_ids.iter().filter_map(|g| g.parse().ok()).collect();
                    if ids.is_empty() {
                        continue; // Discord drops empty folders too.
                    }
                    positions.extend_from_slice(&ids);
                    folders.push(PbGuildFolder {
                        guild_ids: ids,
                        id: Some(Int64Value { value: *id }),
                        name: name.clone().map(|value| StringValue { value }),
                        color: color.map(|c| UInt64Value { value: c as u64 }),
                    });
                }
            }
        }
        let field = PbGuildFolders {
            folders,
            guild_positions: positions,
        };
        self.patch(GUILD_FOLDERS, &[1, 2], &field.encode_to_vec())
    }

    /// Patch: set the DM explicit content filter.
    pub fn patch_explicit_filter(&self, filter: ExplicitFilter) -> String {
        let field = PbTextAndImages {
            explicit_content_filter: Some(UInt32Value {
                value: filter.as_u32(),
            }),
            ..Default::default()
        };
        self.patch(TEXT_AND_IMAGES, &[19], &field.encode_to_vec())
    }

    /// Patch: compact (`true`) or cozy (`false`) message display.
    pub fn patch_compact(&self, compact: bool) -> String {
        let field = PbTextAndImages {
            message_display_compact: Some(BoolValue { value: compact }),
            ..Default::default()
        };
        self.patch(TEXT_AND_IMAGES, &[17], &field.encode_to_vec())
    }

    /// Patch: default for "Allow direct messages from server members".
    pub fn patch_allow_dms_from_guild_members(&self, allow: bool) -> String {
        let field = PbPrivacy {
            default_guilds_restricted: !allow,
            ..Default::default()
        };
        self.patch(PRIVACY, &[4], &field.encode_to_vec())
    }

    /// A `PreloadedUserSettings` holding only top-level field `top`: its
    /// current bytes with `replace` fields removed and `new_fields` appended.
    fn patch(&self, top: u32, replace: &[u32], new_fields: &[u8]) -> String {
        let current = top_level_payload(&self.raw, top).unwrap_or_default();
        let mut sub = without_tags(&current, replace);
        sub.extend_from_slice(new_fields);
        let mut msg = Vec::with_capacity(sub.len() + 8);
        put_varint(&mut msg, (u64::from(top) << 3) | 2);
        put_varint(&mut msg, sub.len() as u64);
        msg.extend_from_slice(&sub);
        B64.encode(msg)
    }
}

/// A folder id not used by any existing folder (Discord uses random
/// positive 31-bit ids).
pub fn new_folder_id(settings: &UserSettings) -> i64 {
    let mut id = (now_ms() as i64 ^ 0x5DEE_CE66) & 0x7FFF_FFFF;
    while id == 0 || settings.folders.iter().any(|f| f.id == Some(id)) {
        id = (id.wrapping_mul(48_271) + 1) & 0x7FFF_FFFF;
    }
    id
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Server rail order
// ---------------------------------------------------------------------------

/// The server rail in Discord's order: servers the settings don't mention
/// (newly joined) first, then folders and loose servers as stored. Servers
/// I'm no longer in are dropped, as are folders left empty.
pub fn ordered_guilds<'a>(
    settings: &UserSettings,
    all_guild_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<RailItem> {
    use std::collections::HashSet;

    let all: Vec<&str> = all_guild_ids.into_iter().collect();
    let known: HashSet<u64> = all.iter().filter_map(|g| g.parse().ok()).collect();
    let mut placed: HashSet<u64> = HashSet::new();
    let mut take = |id: u64| known.contains(&id) && placed.insert(id);

    let mut stored = Vec::new();
    if !settings.folders.is_empty() {
        for f in &settings.folders {
            let ids: Vec<u64> = f.guild_ids.iter().copied().filter(|&g| take(g)).collect();
            match f.id {
                Some(id) if !ids.is_empty() => stored.push(RailItem::Folder {
                    id,
                    name: f.name.clone().filter(|n| !n.is_empty()),
                    color: f.color.map(|c| (c & 0xFF_FFFF) as u32),
                    guild_ids: ids.iter().map(u64::to_string).collect(),
                }),
                Some(_) => {}
                None => stored.extend(ids.into_iter().map(|g| RailItem::Guild(g.to_string()))),
            }
        }
    } else {
        for &g in &settings.guild_positions {
            if take(g) {
                stored.push(RailItem::Guild(g.to_string()));
            }
        }
    }

    let mut out: Vec<RailItem> = all
        .iter()
        .filter(|g| g.parse::<u64>().map_or(true, |id| !placed.contains(&id)))
        .map(|g| RailItem::Guild(g.to_string()))
        .collect();
    // Unparseable or duplicate ids in the input: keep one of each.
    let mut seen = HashSet::new();
    out.retain(|i| matches!(i, RailItem::Guild(g) if seen.insert(g.clone())));
    out.extend(stored);
    out
}

// ---------------------------------------------------------------------------
// Protobuf structs (just the fields above; prost skips the rest)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct StringValue {
    #[prost(string, tag = "1")]
    value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct BoolValue {
    #[prost(bool, tag = "1")]
    value: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
struct UInt32Value {
    #[prost(uint32, tag = "1")]
    value: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Int64Value {
    #[prost(int64, tag = "1")]
    value: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct UInt64Value {
    #[prost(uint64, tag = "1")]
    value: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbStatusSettings {
    #[prost(message, optional, tag = "1")]
    status: Option<StringValue>,
    #[prost(message, optional, tag = "2")]
    custom_status: Option<PbCustomStatus>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbCustomStatus {
    #[prost(string, tag = "1")]
    text: String,
    #[prost(fixed64, tag = "2")]
    emoji_id: u64,
    #[prost(string, tag = "3")]
    emoji_name: String,
    #[prost(fixed64, tag = "4")]
    expires_at_ms: u64,
    #[prost(fixed64, tag = "5")]
    created_at_ms: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbTextAndImages {
    #[prost(message, optional, tag = "17")]
    message_display_compact: Option<BoolValue>,
    #[prost(message, optional, tag = "19")]
    explicit_content_filter: Option<UInt32Value>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbPrivacy {
    #[prost(fixed64, repeated, tag = "3")]
    restricted_guild_ids: Vec<u64>,
    #[prost(bool, tag = "4")]
    default_guilds_restricted: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbAppearance {
    #[prost(int32, tag = "1")]
    theme: i32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbGuildFolders {
    #[prost(message, repeated, tag = "1")]
    folders: Vec<PbGuildFolder>,
    #[prost(fixed64, repeated, tag = "2")]
    guild_positions: Vec<u64>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbGuildFolder {
    #[prost(fixed64, repeated, tag = "1")]
    guild_ids: Vec<u64>,
    #[prost(message, optional, tag = "2")]
    id: Option<Int64Value>,
    #[prost(message, optional, tag = "3")]
    name: Option<StringValue>,
    #[prost(message, optional, tag = "4")]
    color: Option<UInt64Value>,
}

impl From<PbCustomStatus> for CustomStatus {
    fn from(c: PbCustomStatus) -> Self {
        let nz = |v: u64| (v != 0).then_some(v);
        Self {
            text: c.text,
            emoji_name: (!c.emoji_name.is_empty()).then_some(c.emoji_name),
            emoji_id: nz(c.emoji_id),
            expires_at_ms: nz(c.expires_at_ms),
            created_at_ms: nz(c.created_at_ms),
        }
    }
}

impl From<PbGuildFolder> for GuildFolder {
    fn from(f: PbGuildFolder) -> Self {
        Self {
            id: f.id.map(|v| v.value),
            name: f.name.map(|v| v.value),
            color: f.color.map(|v| v.value),
            guild_ids: f.guild_ids,
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal wire-format walker (to splice fields without losing unknown ones)
// ---------------------------------------------------------------------------

struct Field {
    tag: u32,
    /// Whole field: key + value.
    span: std::ops::Range<usize>,
    /// Payload of a length-delimited field.
    payload: Option<std::ops::Range<usize>>,
}

fn get_varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v: u64 = 0;
    for shift in (0..64).step_by(7) {
        let Some(&b) = buf.get(*pos) else {
            bail!("truncated varint");
        };
        *pos += 1;
        v |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("varint too long")
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Split a message into its fields (not recursive).
fn fields(buf: &[u8]) -> Result<Vec<Field>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let start = pos;
        let key = get_varint(buf, &mut pos)?;
        let tag = u32::try_from(key >> 3).context("field number too large")?;
        if tag == 0 {
            bail!("field number 0");
        }
        let mut payload = None;
        let end = match key & 7 {
            0 => {
                get_varint(buf, &mut pos)?;
                pos
            }
            1 => pos + 8,
            2 => {
                let len = usize::try_from(get_varint(buf, &mut pos)?)?;
                let end = pos.checked_add(len).context("length overflow")?;
                payload = Some(pos..end);
                end
            }
            5 => pos + 4,
            w => bail!("unsupported wire type {w}"),
        };
        if end > buf.len() {
            bail!("field runs past end of message");
        }
        pos = end;
        out.push(Field {
            tag,
            span: start..end,
            payload,
        });
    }
    Ok(out)
}

/// `buf` with every field numbered in `tags` removed. Malformed input yields
/// an empty message (the caller then sends only the new fields).
fn without_tags(buf: &[u8], tags: &[u32]) -> Vec<u8> {
    let Ok(fs) = fields(buf) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(buf.len());
    for f in fs.iter().filter(|f| !tags.contains(&f.tag)) {
        out.extend_from_slice(&buf[f.span.clone()]);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Hand-rolled encoders, independent of the prost structs above.
    fn key(tag: u32, wire: u64) -> Vec<u8> {
        let mut v = Vec::new();
        put_varint(&mut v, (u64::from(tag) << 3) | wire);
        v
    }
    fn len_field(tag: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = key(tag, 2);
        put_varint(&mut v, payload.len() as u64);
        v.extend_from_slice(payload);
        v
    }
    fn varint_field(tag: u32, value: u64) -> Vec<u8> {
        let mut v = key(tag, 0);
        put_varint(&mut v, value);
        v
    }
    fn fixed64_field(tag: u32, value: u64) -> Vec<u8> {
        let mut v = key(tag, 1);
        v.extend_from_slice(&value.to_le_bytes());
        v
    }
    fn packed_fixed64(tag: u32, values: &[u64]) -> Vec<u8> {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        len_field(tag, &bytes)
    }
    fn wrap_str(tag: u32, s: &str) -> Vec<u8> {
        len_field(tag, &len_field(1, s.as_bytes()))
    }
    fn wrap_varint(tag: u32, v: u64) -> Vec<u8> {
        len_field(tag, &varint_field(1, v))
    }

    const G1: u64 = 111_111_111_111_111_111;
    const G2: u64 = 222_222_222_222_222_222;
    const G3: u64 = 333_333_333_333_333_333;
    const G4: u64 = 444_444_444_444_444_444;

    /// A realistic message with fields we don't model sprinkled in.
    fn fixture() -> Vec<u8> {
        let custom = [
            len_field(1, b"brb"),
            fixed64_field(2, 42),
            len_field(3, b"party"),
            fixed64_field(4, 1_800_000_000_000),
            fixed64_field(5, 1_700_000_000_000),
        ]
        .concat();
        let status = [
            wrap_str(1, "dnd"),
            len_field(2, &custom),
            wrap_varint(3, 1), // show_current_game (unknown to us)
        ]
        .concat();
        let text = [
            wrap_varint(11, 0), // gif_auto_play (unknown)
            wrap_varint(17, 1),
            wrap_varint(19, 2),
        ]
        .concat();
        let privacy = [packed_fixed64(3, &[G4]), varint_field(4, 1)].concat();
        let folder = [
            packed_fixed64(1, &[G2, G3]),
            wrap_varint(2, 12345),
            wrap_str(3, "Games"),
            wrap_varint(4, 0x5865F2),
        ]
        .concat();
        let loose = packed_fixed64(1, &[G1]);
        let folders = [len_field(1, &loose), len_field(1, &folder)].concat();
        [
            len_field(1, &varint_field(1, 7)), // versions (unknown)
            len_field(6, &text),
            len_field(8, &privacy),
            len_field(11, &status),
            len_field(13, &varint_field(1, 4)),
            len_field(14, &folders),
            len_field(99, b"future"), // unknown top-level field
        ]
        .concat()
    }

    fn sub_fields(patch_b64: &str, top: u32) -> Vec<u8> {
        let bytes = B64.decode(patch_b64).unwrap();
        let fs = fields(&bytes).unwrap();
        assert_eq!(fs.len(), 1, "patch must hold exactly one top-level field");
        assert_eq!(fs[0].tag, top);
        bytes[fs[0].payload.clone().unwrap()].to_vec()
    }

    #[test]
    fn decodes_fixture() {
        let s = UserSettings::from_base64(&B64.encode(fixture())).unwrap();
        assert!(s.warnings.is_empty());
        assert_eq!(s.status, Some(Status::Dnd));
        assert_eq!(
            s.custom_status,
            Some(CustomStatus {
                text: "brb".into(),
                emoji_name: Some("party".into()),
                emoji_id: Some(42),
                expires_at_ms: Some(1_800_000_000_000),
                created_at_ms: Some(1_700_000_000_000),
            })
        );
        assert_eq!(s.compact_mode, Some(true));
        assert_eq!(s.explicit_content_filter, Some(ExplicitFilter::Everyone));
        assert!(!s.allow_dms_from_guild_members);
        assert_eq!(s.restricted_guild_ids, vec![G4]);
        assert_eq!(s.theme, Some(Theme::Midnight));
        assert_eq!(s.folders.len(), 2);
        assert_eq!(s.folders[0].id, None);
        assert_eq!(s.folders[1].id, Some(12345));
        assert_eq!(s.folders[1].name.as_deref(), Some("Games"));
        assert_eq!(s.folders[1].color, Some(0x5865F2));
        assert_eq!(s.raw(), fixture().as_slice());
    }

    #[test]
    fn decodes_literal_status_only() {
        // 5a 0a | 0a 08 | 0a 06 "online": status { status { value: "online" } }
        let s = UserSettings::from_base64("WgoKCAoGb25saW5l").unwrap();
        assert_eq!(s.status, Some(Status::Online));
        assert_eq!(s.custom_status, None);
        assert!(s.allow_dms_from_guild_members);
    }

    #[test]
    fn missing_fields_are_defaults() {
        let s = UserSettings::from_base64("").unwrap();
        assert_eq!(s, UserSettings::default());
        // Sections present but empty.
        let raw = [len_field(6, &[]), len_field(11, &[]), len_field(14, &[])].concat();
        let s = UserSettings::decode(raw).unwrap();
        assert_eq!(s.status, None);
        assert_eq!(s.compact_mode, None);
        assert_eq!(s.explicit_content_filter, None);
        assert!(s.folders.is_empty());
        assert!(s.warnings.is_empty());
        // Wrapper present with the default value is not "missing".
        let s = UserSettings::decode(len_field(6, &len_field(17, &[]))).unwrap();
        assert_eq!(s.compact_mode, Some(false));
    }

    #[test]
    fn garbage_does_not_panic() {
        for b64 in ["!!!not base64!!!", "AAAA", "/////w==", "Wg==", "WgoKCAoG"] {
            let _ = UserSettings::from_base64(b64);
        }
        assert!(UserSettings::from_base64("%%%").is_err());
        assert!(UserSettings::from_base64("Wg==").is_err()); // truncated length
        assert!(UserSettings::decode(vec![0xFF; 12]).is_err()); // varint too long
        assert!(UserSettings::decode(vec![0x00]).is_err()); // field 0

        let mut s = UserSettings::default();
        assert!(s.merge_base64("@@", true).is_err());
        assert!(
            s.apply_update_event(&json!({"settings": {"type": 1}}))
                .is_err()
        );
        assert!(UserSettings::from_ready(&json!({})).is_err());
        assert_eq!(s, UserSettings::default());
    }

    #[test]
    fn bad_section_only_loses_that_section() {
        // status.status as a varint instead of a StringValue message.
        let raw = [
            len_field(11, &varint_field(1, 5)),
            len_field(6, &wrap_varint(17, 1)),
        ]
        .concat();
        let s = UserSettings::decode(raw).unwrap();
        assert_eq!(s.warnings, vec!["status"]);
        assert_eq!(s.status, None);
        assert_eq!(s.compact_mode, Some(true));
    }

    #[test]
    fn patches_round_trip() {
        let mut s = UserSettings::default();
        s.apply_patch(&s.patch_status(Status::Idle)).unwrap();
        s.apply_patch(&s.patch_compact(true)).unwrap();
        s.apply_patch(&s.patch_explicit_filter(ExplicitFilter::Off))
            .unwrap();
        s.apply_patch(&s.patch_allow_dms_from_guild_members(false))
            .unwrap();
        let custom = CustomStatus {
            text: "coding".into(),
            emoji_name: Some("🦀".into()),
            emoji_id: None,
            expires_at_ms: Some(99),
            created_at_ms: Some(1),
        };
        s.apply_patch(&s.patch_custom_status(Some(&custom)))
            .unwrap();

        let decoded = UserSettings::decode(s.raw().to_vec()).unwrap();
        assert_eq!(decoded, s);
        assert_eq!(s.status, Some(Status::Idle));
        assert_eq!(s.compact_mode, Some(true));
        assert_eq!(s.explicit_content_filter, Some(ExplicitFilter::Off));
        assert!(!s.allow_dms_from_guild_members);
        assert_eq!(s.custom_status, Some(custom.clone()));

        // Changing status keeps the custom status, and vice versa.
        s.apply_patch(&s.patch_status(Status::Invisible)).unwrap();
        assert!(s.custom_status.is_some());
        s.apply_patch(&s.patch_custom_status(None)).unwrap();
        assert_eq!(s.custom_status, None);
        assert_eq!(s.status, Some(Status::Invisible));

        // Empty text + no emoji clears too.
        s.apply_patch(&s.patch_custom_status(Some(&custom)))
            .unwrap();
        s.apply_patch(&s.patch_custom_status(Some(&CustomStatus::default())))
            .unwrap();
        assert_eq!(s.custom_status, None);
    }

    #[test]
    fn patches_keep_unknown_fields_of_the_sub_message() {
        let s = UserSettings::decode(fixture()).unwrap();

        let status = sub_fields(&s.patch_status(Status::Online), STATUS);
        let tags: Vec<u32> = fields(&status).unwrap().iter().map(|f| f.tag).collect();
        assert!(tags.contains(&3), "show_current_game must survive");
        assert!(tags.contains(&2), "custom status must survive");
        assert_eq!(tags.iter().filter(|&&t| t == 1).count(), 1);

        let text = sub_fields(&s.patch_compact(false), TEXT_AND_IMAGES);
        let tags: Vec<u32> = fields(&text).unwrap().iter().map(|f| f.tag).collect();
        assert!(tags.contains(&11) && tags.contains(&19) && tags.contains(&17));

        // Applying it changes only that field; other sections untouched.
        let mut s2 = s.clone();
        s2.apply_patch(&s.patch_compact(false)).unwrap();
        assert_eq!(s2.compact_mode, Some(false));
        assert_eq!(s2.explicit_content_filter, s.explicit_content_filter);
        assert_eq!(s2.status, s.status);
        assert_eq!(s2.folders, s.folders);
        assert!(top_level_payload(s2.raw(), 99).is_some());
    }

    #[test]
    fn update_events() {
        let mut s = UserSettings::decode(fixture()).unwrap();
        let patch = UserSettings::default().patch_status(Status::Online);

        // Partial: replaces only the status sub-message.
        let ev = json!({"settings": {"type": 1, "proto": patch}, "partial": true});
        assert!(s.apply_update_event(&ev).unwrap());
        assert_eq!(s.status, Some(Status::Online));
        assert_eq!(s.custom_status, None); // the whole sub-message was replaced
        assert_eq!(s.compact_mode, Some(true));
        assert_eq!(s.folders.len(), 2);

        // Other settings types are ignored.
        let ev = json!({"settings": {"type": 2, "proto": "AAAA"}, "partial": false});
        assert!(!s.apply_update_event(&ev).unwrap());
        assert_eq!(s.compact_mode, Some(true));

        // Full: replaces everything.
        let ev = json!({"settings": {"type": 1, "proto": patch}, "partial": false});
        assert!(s.apply_update_event(&ev).unwrap());
        assert_eq!(s.compact_mode, None);
        assert!(s.folders.is_empty());

        let ready = json!({"user_settings_proto": B64.encode(fixture())});
        assert_eq!(
            UserSettings::from_ready(&ready).unwrap().theme,
            Some(Theme::Midnight)
        );
        let rest = json!({"settings": B64.encode(fixture())});
        assert_eq!(
            UserSettings::from_rest(&rest).unwrap().status,
            Some(Status::Dnd)
        );
    }

    #[test]
    fn ordering_with_folders() {
        let s = UserSettings::decode(fixture()).unwrap();
        let ids = [G1, G2, G3, G4, 555].map(|g| g.to_string());
        let rail = ordered_guilds(&s, ids.iter().map(String::as_str));
        assert_eq!(
            rail,
            vec![
                // Not in the settings: on top, input order.
                RailItem::Guild(G4.to_string()),
                RailItem::Guild("555".into()),
                RailItem::Guild(G1.to_string()),
                RailItem::Folder {
                    id: 12345,
                    name: Some("Games".into()),
                    color: Some(0x5865F2),
                    guild_ids: vec![G2.to_string(), G3.to_string()],
                },
            ]
        );

        // Left G2 and G3: the folder disappears; unknown ids are ignored.
        let rail = ordered_guilds(&s, [G1.to_string()].iter().map(String::as_str));
        assert_eq!(rail, vec![RailItem::Guild(G1.to_string())]);
    }

    #[test]
    fn ordering_without_folders_uses_positions() {
        let mut s = UserSettings::default();
        assert_eq!(
            ordered_guilds(&s, ["1", "2"]),
            vec![RailItem::Guild("1".into()), RailItem::Guild("2".into())]
        );
        s.guild_positions = vec![3, 1, 99];
        assert_eq!(
            ordered_guilds(&s, ["1", "2", "3", "2"]),
            vec![
                RailItem::Guild("2".into()),
                RailItem::Guild("3".into()),
                RailItem::Guild("1".into()),
            ]
        );
    }

    #[test]
    fn folder_patch_round_trips_through_ordering() {
        let s = UserSettings::decode(fixture()).unwrap();
        let rail = vec![
            RailItem::Folder {
                id: 7,
                name: None,
                color: None,
                guild_ids: vec![G3.to_string(), G1.to_string()],
            },
            RailItem::Guild(G2.to_string()),
            RailItem::Folder {
                id: 8,
                name: Some("empty".into()),
                color: None,
                guild_ids: vec![],
            },
        ];
        let mut s2 = s.clone();
        s2.apply_patch(&s.patch_guild_folders(&rail)).unwrap();
        assert_eq!(s2.guild_positions, vec![G3, G1, G2]);
        let ids = [G1, G2, G3].map(|g| g.to_string());
        let back = ordered_guilds(&s2, ids.iter().map(String::as_str));
        assert_eq!(back, rail[..2].to_vec());
        // Other sections untouched.
        assert_eq!(s2.custom_status, s.custom_status);

        let id = new_folder_id(&s2);
        assert!(id > 0 && id != 7 && id != 8);
    }
}
