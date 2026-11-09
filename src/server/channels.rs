//! Channels and categories: kinds, the editable [`ChannelDraft`] with its
//! create / edit payloads, reorder payloads for `PATCH guilds/{id}/channels`,
//! and the "Create Channel" / "Create Category" dialog.
//!
//! Reordering: Discord sorts a category's children by `(voice-ness,
//! position, id)` and categories by `(position, id)`. Positions only matter
//! between siblings, so a move renumbers just that sibling group 0..n and
//! sends the entries whose position changed. Moving into another category
//! also sends `parent_id` for the moved channel (null = no category).

use eframe::egui::{self, RichText, vec2};
use serde_json::{Map, Value, json};

use super::permissions as p;
use super::rest::ServerRequest;
use super::widgets::{self, Icon};
use crate::theme::{Bold, Palette};
use crate::types::{
    CHANNEL_ANNOUNCEMENT, CHANNEL_CATEGORY, CHANNEL_STAGE, CHANNEL_TEXT, CHANNEL_VOICE, Channel,
    Guild,
};

pub const CHANNEL_FORUM: u64 = 15;
pub const CHANNEL_MEDIA: u64 = 16;

/// Channel kinds the editors know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Text,
    Voice,
    Announcement,
    Forum,
    Media,
    Stage,
    Category,
    Other(u64),
}

impl ChannelKind {
    /// Kinds offered by "Create Channel".
    pub const CREATABLE: [ChannelKind; 4] = [
        ChannelKind::Text,
        ChannelKind::Voice,
        ChannelKind::Forum,
        ChannelKind::Announcement,
    ];

    pub fn from_type(t: u64) -> Self {
        match t {
            CHANNEL_TEXT => Self::Text,
            CHANNEL_VOICE => Self::Voice,
            CHANNEL_ANNOUNCEMENT => Self::Announcement,
            CHANNEL_FORUM => Self::Forum,
            CHANNEL_MEDIA => Self::Media,
            CHANNEL_STAGE => Self::Stage,
            CHANNEL_CATEGORY => Self::Category,
            other => Self::Other(other),
        }
    }

    pub fn type_id(self) -> u64 {
        match self {
            Self::Text => CHANNEL_TEXT,
            Self::Voice => CHANNEL_VOICE,
            Self::Announcement => CHANNEL_ANNOUNCEMENT,
            Self::Forum => CHANNEL_FORUM,
            Self::Media => CHANNEL_MEDIA,
            Self::Stage => CHANNEL_STAGE,
            Self::Category => CHANNEL_CATEGORY,
            Self::Other(t) => t,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Voice => "Voice",
            Self::Announcement => "Announcement",
            Self::Forum => "Forum",
            Self::Media => "Media",
            Self::Stage => "Stage",
            Self::Category => "Category",
            Self::Other(_) => "Channel",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Text => "Send messages, images, GIFs, emoji, opinions, and puns",
            Self::Voice => "Hang out together with voice, video, and screen share",
            Self::Forum => "Create a space for organized discussions",
            Self::Announcement => "Important updates for people in and out of the server",
            _ => "",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Voice => Icon::Speaker,
            Self::Announcement => Icon::Megaphone,
            Self::Forum | Self::Media => Icon::Forum,
            Self::Stage => Icon::Stage,
            Self::Category => Icon::ChevronDown,
            _ => Icon::Hash,
        }
    }

    pub fn is_voice_like(self) -> bool {
        matches!(self, Self::Voice | Self::Stage)
    }

    /// Text-style names: lowercase, dashes for spaces.
    pub fn slug_names(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Announcement | Self::Forum | Self::Media
        )
    }

    pub fn has_topic(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Announcement | Self::Forum | Self::Media | Self::Stage
        )
    }

    /// Longest topic Discord takes (forum guidelines are longer).
    pub fn topic_limit(self) -> usize {
        if matches!(self, Self::Forum | Self::Media) {
            4096
        } else {
            1024
        }
    }

    pub fn has_slowmode(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Forum | Self::Media | Self::Voice | Self::Stage
        )
    }

    pub fn has_nsfw(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Announcement | Self::Forum | Self::Media | Self::Voice | Self::Stage
        )
    }

    pub fn has_voice_settings(self) -> bool {
        self.is_voice_like()
    }
}

/// Slowmode choices (seconds), as in Discord's slider.
pub const SLOWMODE_OPTIONS: &[u32] = &[
    0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600,
];

pub fn slowmode_label(secs: u32) -> String {
    match secs {
        0 => "Off".into(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

/// Bitrate limits (bps) without boosts; boosted servers allow more.
pub const BITRATE_MIN: u32 = 8_000;
pub const BITRATE_MAX: u32 = 96_000;
pub const BITRATE_DEFAULT: u32 = 64_000;

/// Discord's text-channel name rule: lowercase, whitespace → '-', no
/// leading / repeated dashes. Other kinds keep what was typed (trimmed).
pub fn normalize_name(kind: ChannelKind, raw: &str) -> String {
    if !kind.slug_names() {
        return raw.trim().chars().take(100).collect();
    }
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.extend(c.to_lowercase());
    }
    out.chars().take(100).collect()
}

/// Editable channel fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDraft {
    pub kind: ChannelKind,
    pub name: String,
    pub topic: String,
    /// rate_limit_per_user, seconds.
    pub slowmode: u32,
    pub nsfw: bool,
    pub bitrate: u32,
    /// 0 = unlimited.
    pub user_limit: u32,
    pub parent_id: Option<String>,
}

impl ChannelDraft {
    pub fn new(kind: ChannelKind, parent_id: Option<String>) -> Self {
        Self {
            kind,
            name: String::new(),
            topic: String::new(),
            slowmode: 0,
            nsfw: false,
            bitrate: BITRATE_DEFAULT,
            user_limit: 0,
            parent_id,
        }
    }

    /// From the full channel object (`GET channels/{id}`, CHANNEL_UPDATE).
    pub fn from_json(v: &Value) -> Self {
        let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        Self {
            kind: ChannelKind::from_type(v.get("type").and_then(Value::as_u64).unwrap_or(0)),
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            topic: v
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            slowmode: u("rate_limit_per_user"),
            nsfw: v.get("nsfw").and_then(Value::as_bool).unwrap_or(false),
            bitrate: v
                .get("bitrate")
                .and_then(Value::as_u64)
                .map_or(BITRATE_DEFAULT, |b| b as u32),
            user_limit: u("user_limit"),
            parent_id: v
                .get("parent_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    /// From the cached channel (no nsfw / slowmode / voice fields there;
    /// replaced once `GET channels/{id}` answers).
    pub fn from_channel(c: &Channel) -> Self {
        Self {
            name: c.name.clone(),
            topic: c.topic.clone(),
            ..Self::new(ChannelKind::from_type(c.kind), c.parent_id.clone())
        }
    }

    /// Problem that blocks saving, if any.
    pub fn validate(&self) -> Result<(), String> {
        let name = normalize_name(self.kind, &self.name);
        if name.is_empty() {
            return Err("The channel needs a name.".into());
        }
        if self.kind.has_topic() && self.topic.chars().count() > self.kind.topic_limit() {
            return Err(format!(
                "The topic can be at most {} characters.",
                self.kind.topic_limit()
            ));
        }
        if self.user_limit > 99 && self.kind == ChannelKind::Voice {
            return Err("The user limit can be at most 99.".into());
        }
        Ok(())
    }

    /// `POST guilds/{id}/channels` body (only fields the kind uses).
    /// `overwrites` makes it private (see [`private_overwrites`]).
    pub fn create_body(&self, overwrites: &[Value]) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), json!(normalize_name(self.kind, &self.name)));
        m.insert("type".into(), json!(self.kind.type_id()));
        if self.kind != ChannelKind::Category
            && let Some(parent) = &self.parent_id
        {
            m.insert("parent_id".into(), json!(parent));
        }
        if self.kind.has_topic() && !self.topic.trim().is_empty() {
            m.insert("topic".into(), json!(self.topic.trim()));
        }
        if self.kind.has_slowmode() && self.slowmode > 0 {
            m.insert("rate_limit_per_user".into(), json!(self.slowmode));
        }
        if self.kind.has_nsfw() && self.nsfw {
            m.insert("nsfw".into(), json!(true));
        }
        if self.kind.has_voice_settings() {
            if self.bitrate != BITRATE_DEFAULT {
                m.insert("bitrate".into(), json!(self.bitrate));
            }
            if self.user_limit > 0 {
                m.insert("user_limit".into(), json!(self.user_limit));
            }
        }
        if !overwrites.is_empty() {
            m.insert("permission_overwrites".into(), json!(overwrites));
        }
        Value::Object(m)
    }

    /// `PATCH channels/{id}` body with only what changed from `original`
    /// (an empty object = nothing to save).
    pub fn patch_body(&self, original: &ChannelDraft) -> Value {
        let mut m = Map::new();
        let name = normalize_name(self.kind, &self.name);
        if name != original.name {
            m.insert("name".into(), json!(name));
        }
        if self.kind.has_topic() && self.topic.trim() != original.topic.trim() {
            m.insert("topic".into(), json!(self.topic.trim()));
        }
        if self.kind.has_slowmode() && self.slowmode != original.slowmode {
            m.insert("rate_limit_per_user".into(), json!(self.slowmode));
        }
        if self.kind.has_nsfw() && self.nsfw != original.nsfw {
            m.insert("nsfw".into(), json!(self.nsfw));
        }
        if self.kind.has_voice_settings() {
            if self.bitrate != original.bitrate {
                m.insert("bitrate".into(), json!(self.bitrate));
            }
            if self.user_limit != original.user_limit {
                m.insert("user_limit".into(), json!(self.user_limit));
            }
        }
        if self.kind != ChannelKind::Category && self.parent_id != original.parent_id {
            m.insert("parent_id".into(), json!(self.parent_id));
        }
        Value::Object(m)
    }

    pub fn is_dirty(&self, original: &ChannelDraft) -> bool {
        self.patch_body(original)
            .as_object()
            .is_some_and(|m| !m.is_empty())
    }
}

/// Overwrites for a private channel: @everyone can't see it, I can (so I
/// don't lock myself out; voice also gets Connect).
pub fn private_overwrites(guild_id: &str, my_id: &str, kind: ChannelKind) -> Vec<Value> {
    let mut mine = p::VIEW_CHANNEL;
    if kind.is_voice_like() || kind == ChannelKind::Category {
        mine |= p::CONNECT;
    }
    vec![
        json!({ "id": guild_id, "type": 0, "allow": "0", "deny": p::VIEW_CHANNEL.to_string() }),
        json!({ "id": my_id, "type": 1, "allow": mine.to_string(), "deny": "0" }),
    ]
}

// ---------------------------------------------------------------------------
// Reordering
// ---------------------------------------------------------------------------

/// One entry of the `PATCH guilds/{id}/channels` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionUpdate {
    pub id: String,
    pub position: i64,
    /// `Some(None)` = move out of any category (`parent_id: null`).
    pub parent: Option<Option<String>>,
    /// Sync permissions with the new category.
    pub lock_permissions: Option<bool>,
}

impl PositionUpdate {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(self.id));
        m.insert("position".into(), json!(self.position));
        if let Some(parent) = &self.parent {
            m.insert("parent_id".into(), json!(parent));
        }
        if let Some(lock) = self.lock_permissions {
            m.insert("lock_permissions".into(), json!(lock));
        }
        Value::Object(m)
    }
}

pub fn reorder_body(updates: &[PositionUpdate]) -> Value {
    Value::Array(updates.iter().map(PositionUpdate::to_json).collect())
}

/// Renumber `ids` (in their new order) 0..n and keep the ones whose
/// position changed. `old` gives each id's current position.
fn renumber(ids: &[&str], old: impl Fn(&str) -> i64) -> Vec<PositionUpdate> {
    ids.iter()
        .enumerate()
        .filter(|(i, id)| old(id) != *i as i64)
        .map(|(i, id)| PositionUpdate {
            id: id.to_string(),
            position: i as i64,
            parent: None,
            lock_permissions: None,
        })
        .collect()
}

/// Move `from` to `to` within one sibling group (`(id, position)` in display
/// order). Out-of-range or no-op moves give an empty list.
pub fn reorder(siblings: &[(&str, i64)], from: usize, to: usize) -> Vec<PositionUpdate> {
    if from >= siblings.len() || to >= siblings.len() || from == to {
        return Vec::new();
    }
    let mut ids: Vec<&str> = siblings.iter().map(|(id, _)| *id).collect();
    let moved = ids.remove(from);
    ids.insert(to, moved);
    renumber(&ids, |id| {
        siblings
            .iter()
            .find(|(s, _)| *s == id)
            .map_or(-1, |(_, p)| *p)
    })
}

/// Channels that sort together with `channel` (same parent and same
/// text/voice class; categories with all categories), in display order.
pub fn siblings<'a>(guild: &'a Guild, channel: &Channel) -> Vec<&'a Channel> {
    let kind = ChannelKind::from_type(channel.kind);
    let mut list: Vec<&Channel> = guild
        .channels
        .iter()
        .filter(|c| {
            let k = ChannelKind::from_type(c.kind);
            if kind == ChannelKind::Category {
                k == ChannelKind::Category
            } else {
                k != ChannelKind::Category
                    && c.parent_id == channel.parent_id
                    && k.is_voice_like() == kind.is_voice_like()
            }
        })
        .collect();
    list.sort_by_key(|c| (c.position, c.id.parse::<u64>().unwrap_or(0)));
    list
}

/// Move a channel one step up (`delta = -1`) or down (`+1`) among its
/// siblings.
pub fn move_step(guild: &Guild, channel_id: &str, delta: i32) -> Vec<PositionUpdate> {
    let Some(ch) = guild.channel(channel_id) else {
        return Vec::new();
    };
    let sibs = siblings(guild, ch);
    let Some(from) = sibs.iter().position(|c| c.id == channel_id) else {
        return Vec::new();
    };
    let to = from as i64 + delta as i64;
    if to < 0 {
        return Vec::new();
    }
    let pairs: Vec<(&str, i64)> = sibs.iter().map(|c| (c.id.as_str(), c.position)).collect();
    reorder(&pairs, from, to as usize)
}

/// Move a channel into `parent` (None = no category), at the end of that
/// group. `sync` locks its permissions to the new category.
pub fn move_to_category(
    guild: &Guild,
    channel_id: &str,
    parent: Option<&str>,
    sync: bool,
) -> Vec<PositionUpdate> {
    let Some(ch) = guild.channel(channel_id) else {
        return Vec::new();
    };
    if ch.kind == CHANNEL_CATEGORY || ch.parent_id.as_deref() == parent {
        return Vec::new();
    }
    let mut moved = ch.clone();
    moved.parent_id = parent.map(str::to_string);
    let mut ids: Vec<&str> = siblings(guild, &moved)
        .iter()
        .filter(|c| c.id != channel_id)
        .map(|c| c.id.as_str())
        .collect();
    ids.push(channel_id);
    let mut updates = renumber(&ids, |id| {
        if id == channel_id {
            -1
        } else {
            guild.channel(id).map_or(-1, |c| c.position)
        }
    });
    if let Some(u) = updates.iter_mut().find(|u| u.id == channel_id) {
        u.parent = Some(parent.map(str::to_string));
        u.lock_permissions = sync.then_some(true);
    }
    updates
}

// ---------------------------------------------------------------------------
// Create dialog
// ---------------------------------------------------------------------------

/// "Create Channel" / "Create Category" modal.
#[derive(Debug, Clone)]
pub struct CreateChannelDialog {
    pub guild_id: String,
    pub draft: ChannelDraft,
    pub private: bool,
    creating: bool,
    error: Option<String>,
}

impl CreateChannelDialog {
    pub fn new(guild_id: &str, category: bool, parent_id: Option<String>) -> Self {
        let kind = if category {
            ChannelKind::Category
        } else {
            ChannelKind::Text
        };
        Self {
            guild_id: guild_id.to_string(),
            draft: ChannelDraft::new(kind, if category { None } else { parent_id }),
            private: false,
            creating: false,
            error: None,
        }
    }

    pub fn on_failed(&mut self, error: String) {
        self.creating = false;
        self.error = Some(error);
    }

    /// Returns `(close, request)`.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        guild: &Guild,
        my_id: &str,
        pal: &Palette,
    ) -> (bool, Option<ServerRequest>) {
        let mut close = false;
        let mut req = None;
        let is_cat = self.draft.kind == ChannelKind::Category;
        let modal = egui::Modal::new(egui::Id::unique("rustcord_create_channel")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(if is_cat { "Create Category" } else { "Create Channel" })
                            .bold()
                            .size(20.0)
                            .color(pal.strong),
                    );
                    if let Some(cat) = self.draft.parent_id.as_deref().and_then(|id| guild.channel(id)) {
                        widgets::hint(ui, pal, &format!("in {}", cat.name));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 22.0, "Close", pal).clicked() {
                        close = true;
                    }
                });
            });

            if !is_cat {
                widgets::field_label(ui, pal, "Channel type");
                for kind in ChannelKind::CREATABLE {
                    let selected = self.draft.kind == kind;
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), egui::Sense::click());
                    let fill = if selected { pal.selected } else if resp.hovered() { pal.hover } else { pal.input };
                    ui.painter().rect_filled(rect, 4, fill);
                    widgets::paint_icon(
                        ui.painter(),
                        egui::Rect::from_center_size(rect.left_center() + vec2(22.0, 0.0), vec2(18.0, 18.0)),
                        kind.icon(),
                        pal.text,
                    );
                    ui.painter().text(
                        rect.left_top() + vec2(44.0, 9.0),
                        egui::Align2::LEFT_TOP,
                        kind.label(),
                        crate::theme::bold_font(14.5),
                        pal.strong,
                    );
                    ui.painter().text(
                        rect.left_bottom() + vec2(44.0, -9.0),
                        egui::Align2::LEFT_BOTTOM,
                        kind.blurb(),
                        egui::FontId::proportional(13.0),
                        pal.muted,
                    );
                    // Radio dot.
                    let c = rect.right_center() - vec2(20.0, 0.0);
                    ui.painter().circle_stroke(c, 8.0, egui::Stroke::new(2.0, if selected { pal.strong } else { pal.muted }));
                    if selected {
                        ui.painter().circle_filled(c, 4.0, pal.strong);
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.draft.kind = kind;
                    }
                    ui.add_space(4.0);
                }
            }

            widgets::field_label(ui, pal, if is_cat { "Category name" } else { "Channel name" });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.draft.name)
                    .hint_text(if is_cat { "New Category" } else { "new-channel" })
                    .desired_width(f32::INFINITY)
                    .char_limit(100),
            );
            if self.draft.kind.slug_names() {
                let shown = normalize_name(self.draft.kind, &self.draft.name);
                if !shown.is_empty() && shown != self.draft.name {
                    widgets::hint(ui, pal, &format!("Will be created as #{shown}"));
                }
            }
            if !is_cat {
                let cats: Vec<(Option<String>, String)> = std::iter::once((None, "No category".to_string()))
                    .chain(
                        guild
                            .channels
                            .iter()
                            .filter(|c| c.kind == CHANNEL_CATEGORY)
                            .map(|c| (Some(c.id.clone()), c.name.clone())),
                    )
                    .collect();
                widgets::field_label(ui, pal, "Category");
                widgets::combo(ui, "create_channel_parent", &mut self.draft.parent_id, &cats, 240.0);
            }
            ui.add_space(12.0);
            widgets::toggle_row(
                ui,
                if is_cat { "Private Category" } else { "Private Channel" },
                "Only you (and people with Administrator) can see it until you add roles or members in its permissions.",
                &mut self.private,
                pal,
            );
            widgets::error_line(ui, pal, &self.error);
            ui.add_space(14.0);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let valid = self.draft.validate().is_ok();
                let label = if self.creating {
                    "Creating…"
                } else if is_cat {
                    "Create Category"
                } else {
                    "Create Channel"
                };
                let clicked = ui.add_enabled(valid && !self.creating, pal.primary_button(label)).clicked();
                if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                    close = true;
                }
                if (clicked || (enter && valid)) && !self.creating {
                    let ow = if self.private {
                        private_overwrites(&guild.id, my_id, self.draft.kind)
                    } else {
                        Vec::new()
                    };
                    self.creating = true;
                    self.error = None;
                    req = Some(ServerRequest::CreateChannel {
                        guild_id: self.guild_id.clone(),
                        body: self.draft.create_body(&ow),
                    });
                }
            });
        });
        if modal.should_close() && !self.creating {
            close = true;
        }
        (close, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guild() -> Guild {
        Guild::from_json(&json!({
            "id": "1",
            "channels": [
                { "id": "100", "type": 4, "name": "Cat A", "position": 0 },
                { "id": "101", "type": 4, "name": "Cat B", "position": 1 },
                { "id": "10", "type": 0, "name": "a", "position": 0, "parent_id": "100" },
                { "id": "11", "type": 0, "name": "b", "position": 1, "parent_id": "100" },
                { "id": "12", "type": 0, "name": "c", "position": 2, "parent_id": "100" },
                { "id": "13", "type": 2, "name": "v", "position": 0, "parent_id": "100" },
                { "id": "14", "type": 0, "name": "d", "position": 0, "parent_id": "101" },
                { "id": "15", "type": 0, "name": "top", "position": 5 }
            ]
        }))
    }

    #[test]
    fn names() {
        assert_eq!(
            normalize_name(ChannelKind::Text, "  My Cool  Channel "),
            "my-cool-channel"
        );
        assert_eq!(normalize_name(ChannelKind::Text, "--Hi"), "hi");
        assert_eq!(
            normalize_name(ChannelKind::Voice, "  Gaming Room "),
            "Gaming Room"
        );
        assert_eq!(
            normalize_name(ChannelKind::Category, "Text Channels"),
            "Text Channels"
        );
        assert_eq!(
            normalize_name(ChannelKind::Text, &"x".repeat(150)).len(),
            100
        );
        assert_eq!(slowmode_label(0), "Off");
        assert_eq!(slowmode_label(30), "30s");
        assert_eq!(slowmode_label(300), "5m");
        assert_eq!(slowmode_label(21600), "6h");
    }

    #[test]
    fn create_bodies() {
        let mut d = ChannelDraft::new(ChannelKind::Text, Some("100".into()));
        d.name = "Hello World".into();
        d.topic = " hi ".into();
        d.slowmode = 10;
        d.nsfw = true;
        assert_eq!(
            d.create_body(&[]),
            json!({ "name": "hello-world", "type": 0, "parent_id": "100", "topic": "hi",
                    "rate_limit_per_user": 10, "nsfw": true })
        );
        let mut v = ChannelDraft::new(ChannelKind::Voice, None);
        v.name = "Lounge".into();
        v.user_limit = 5;
        v.topic = "ignored".into();
        assert_eq!(
            v.create_body(&[]),
            json!({ "name": "Lounge", "type": 2, "user_limit": 5 })
        );
        let mut c = ChannelDraft::new(ChannelKind::Category, Some("100".into()));
        c.name = "Stuff".into();
        let ow = private_overwrites("1", "me", ChannelKind::Category);
        let body = c.create_body(&ow);
        assert_eq!(body["type"], 4);
        assert!(body.get("parent_id").is_none());
        assert_eq!(
            body["permission_overwrites"][0]["deny"],
            p::VIEW_CHANNEL.to_string()
        );
        assert_eq!(body["permission_overwrites"][0]["type"], 0);
        assert_eq!(body["permission_overwrites"][1]["id"], "me");
        assert_eq!(body["permission_overwrites"][1]["type"], 1);
        assert_eq!(
            body["permission_overwrites"][1]["allow"],
            (p::VIEW_CHANNEL | p::CONNECT).to_string()
        );
        assert!(
            ChannelDraft::new(ChannelKind::Text, None)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn patch_bodies_only_changes() {
        let orig = ChannelDraft::from_json(&json!({
            "id": "10", "type": 0, "name": "general", "topic": "t", "nsfw": false,
            "rate_limit_per_user": 0, "parent_id": "100"
        }));
        let mut d = orig.clone();
        assert!(!d.is_dirty(&orig));
        assert_eq!(d.patch_body(&orig), json!({}));
        d.name = "General Chat".into();
        d.slowmode = 60;
        d.parent_id = None;
        assert_eq!(
            d.patch_body(&orig),
            json!({ "name": "general-chat", "rate_limit_per_user": 60, "parent_id": null })
        );
        // Voice fields are ignored for text channels.
        let mut d = orig.clone();
        d.bitrate = 96_000;
        assert!(!d.is_dirty(&orig));

        let vorig = ChannelDraft::from_json(
            &json!({ "type": 2, "name": "v", "bitrate": 64000, "user_limit": 0 }),
        );
        let mut v = vorig.clone();
        v.bitrate = 96_000;
        v.user_limit = 10;
        assert_eq!(
            v.patch_body(&vorig),
            json!({ "bitrate": 96000, "user_limit": 10 })
        );
    }

    #[test]
    fn reorder_payloads() {
        let sibs = [("a", 0), ("b", 1), ("c", 2)];
        // Move c to the top: everything shifts.
        let u = reorder(&sibs, 2, 0);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "c", "position": 0 }, { "id": "a", "position": 1 }, { "id": "b", "position": 2 }])
        );
        // Swap the first two: c is untouched.
        let u = reorder(&sibs, 0, 1);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "b", "position": 0 }, { "id": "a", "position": 1 }])
        );
        assert!(reorder(&sibs, 1, 1).is_empty());
        assert!(reorder(&sibs, 0, 3).is_empty());
        // Duplicate positions get distinct ones.
        let dup = [("x", 0), ("y", 0), ("z", 0)];
        let u = reorder(&dup, 2, 1);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "z", "position": 1 }, { "id": "y", "position": 2 }])
        );
    }

    #[test]
    fn sibling_moves() {
        let g = guild();
        // b down: swaps with c; the voice channel is its own group.
        let u = move_step(&g, "11", 1);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "12", "position": 1 }, { "id": "11", "position": 2 }])
        );
        assert!(move_step(&g, "10", -1).is_empty());
        assert!(move_step(&g, "13", 1).is_empty());
        // Categories move among categories.
        let u = move_step(&g, "101", -1);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "101", "position": 0 }, { "id": "100", "position": 1 }])
        );
        let ids: Vec<_> = siblings(&g, g.channel("15").unwrap())
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, ["15"]);
    }

    #[test]
    fn category_moves() {
        let g = guild();
        let u = move_to_category(&g, "10", Some("101"), true);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "10", "position": 1, "parent_id": "101", "lock_permissions": true }])
        );
        let u = move_to_category(&g, "14", None, false);
        assert_eq!(
            reorder_body(&u),
            json!([{ "id": "15", "position": 0 }, { "id": "14", "position": 1, "parent_id": null }])
        );
        assert!(move_to_category(&g, "10", Some("100"), false).is_empty());
        assert!(move_to_category(&g, "100", Some("101"), false).is_empty());
    }
}
