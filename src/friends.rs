//! Friends (relationships) and message requests: data, gateway events, the
//! REST calls behind each button, and the two Home pages (Friends, Message
//! Requests) that replace the chat when picked in the DM list.
//!
//! Protocol notes (user accounts only; bots have no relationships):
//! - READY `relationships[]`: `{ id, type, nickname?, since?, user? | user_id?,
//!   is_spam_request? }`. `id` is the other user's id. Without the
//!   user-dedup capability each entry carries a full `user`; with it there is
//!   only `user_id` and the user is in READY `users[]`. Both are handled.
//! - `type`: 1 friend, 2 blocked, 3 incoming request, 4 outgoing request,
//!   5 implicit (people you talk to a lot; not shown, like Discord's list).
//! - RELATIONSHIP_ADD carries a full relationship (with `user`).
//!   RELATIONSHIP_UPDATE is partial (`id`, `type`, `nickname`, …).
//!   RELATIONSHIP_REMOVE has `id` (and the old `type`). Discord sends
//!   REMOVE + ADD when the type changes (e.g. friend → blocked).
//! - Message requests are DM channels in READY `private_channels` (and
//!   CHANNEL_CREATE / CHANNEL_UPDATE) flagged `is_message_request: true`,
//!   with `is_message_request_timestamp` (ISO) and `is_spam`. These fields are
//!   undocumented: missing flags count as "not a request", and an update
//!   without the flag leaves the stored state alone. Accepting sends a
//!   CHANNEL_UPDATE with `is_message_request: false`.
//!
//! No self-bot behaviour: every `FriendsAction` / `RequestAction` comes from a
//! click (destructive ones after a confirm), and `execute*` only runs them.

use std::collections::HashMap;

use anyhow::Result;
use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::Value;

use crate::api::Api;
use crate::members::status_dot;
use crate::presence::{Presence, Status};
use crate::theme::{self, Bold, Palette};
use crate::types::{Channel, User, avatar_url};

// ---------------------------------------------------------------------------
// Relationships
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelKind {
    Friend,
    Blocked,
    /// Someone sent me a friend request.
    Incoming,
    /// I sent someone a friend request.
    Outgoing,
    /// Discord's "implicit" relationship (frequent contact). Not listed.
    Implicit,
    Unknown,
}

impl RelKind {
    pub fn from_code(n: u64) -> Self {
        match n {
            1 => RelKind::Friend,
            2 => RelKind::Blocked,
            3 => RelKind::Incoming,
            4 => RelKind::Outgoing,
            5 => RelKind::Implicit,
            _ => RelKind::Unknown,
        }
    }

    pub fn is_pending(self) -> bool {
        matches!(self, RelKind::Incoming | RelKind::Outgoing)
    }
}

#[derive(Clone, Debug)]
pub struct Relationship {
    pub user: User,
    pub kind: RelKind,
    /// My private nickname for this friend.
    pub nickname: Option<String>,
    /// When it started (friends since / request sent), unix seconds or 0.
    pub since: i64,
    /// Incoming request Discord thinks is spam.
    pub spam: bool,
}

fn nickname_of(v: &Value) -> Option<String> {
    v.get("nickname")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn unknown_user(id: &str) -> User {
    User {
        id: id.to_string(),
        name: "Unknown user".into(),
        username: String::new(),
        avatar_url: avatar_url(id, None),
        bot: false,
    }
}

impl Relationship {
    /// One relationship object. `users` resolves `user_id` (READY `users`).
    pub fn from_json(v: &Value, users: &HashMap<&str, &Value>) -> Option<Self> {
        let id = v
            .get("id")
            .or_else(|| v.get("user_id"))
            .or_else(|| v.pointer("/user/id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?;
        let user = match v.get("user").filter(|u| u.is_object()) {
            Some(u) => User::from_json(u),
            None => {
                let uid = v.get("user_id").and_then(Value::as_str).unwrap_or(id);
                users
                    .get(uid)
                    .map(|u| User::from_json(u))
                    .unwrap_or_else(|| unknown_user(id))
            }
        };
        let mut user = user;
        if user.id.is_empty() {
            user.id = id.to_string();
        }
        Some(Self {
            user,
            kind: RelKind::from_code(v.get("type").and_then(Value::as_u64).unwrap_or(0)),
            nickname: nickname_of(v),
            since: crate::timefmt::parse_iso(v.get("since").and_then(Value::as_str).unwrap_or("")),
            spam: v
                .get("is_spam_request")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Nickname, else display name, else username.
    pub fn display_name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.user.name)
    }

    /// Case-insensitive match on nickname / display name / username.
    /// `query` must already be lowercase; empty matches everyone.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                self.nickname.as_deref().unwrap_or(""),
                &self.user.name,
                &self.user.username,
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(query))
    }
}

/// READY `relationships`, resolving `user_id`s against READY `users`.
pub fn parse_ready(d: &Value) -> Vec<Relationship> {
    let users: HashMap<&str, &Value> = d["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| Some((u.get("id")?.as_str()?, u)))
        .collect();
    d["relationships"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Relationship::from_json(r, &users))
        .collect()
}

/// A relationship gateway event.
#[derive(Clone, Debug)]
pub enum RelationshipEvent {
    Add(Relationship),
    /// Partial update. `nickname: Some(None)` clears it; `user` is set when
    /// the payload had a full user object.
    Update {
        user_id: String,
        kind: Option<RelKind>,
        nickname: Option<Option<String>>,
        since: Option<i64>,
        user: Option<User>,
    },
    Remove {
        user_id: String,
    },
}

impl RelationshipEvent {
    /// RELATIONSHIP_ADD / _UPDATE / _REMOVE. Other events give None.
    pub fn parse(event: &str, d: &Value) -> Option<Self> {
        let id = || {
            d.get("id")
                .or_else(|| d.get("user_id"))
                .or_else(|| d.pointer("/user/id"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        match event {
            "RELATIONSHIP_ADD" => {
                Relationship::from_json(d, &HashMap::new()).map(RelationshipEvent::Add)
            }
            "RELATIONSHIP_UPDATE" => Some(RelationshipEvent::Update {
                user_id: id()?,
                kind: d
                    .get("type")
                    .and_then(Value::as_u64)
                    .map(RelKind::from_code),
                nickname: d.get("nickname").map(|_| nickname_of(d)),
                since: d
                    .get("since")
                    .and_then(Value::as_str)
                    .map(crate::timefmt::parse_iso),
                user: d.get("user").filter(|u| u.is_object()).map(User::from_json),
            }),
            "RELATIONSHIP_REMOVE" => Some(RelationshipEvent::Remove { user_id: id()? }),
            _ => None,
        }
    }
}

/// Everyone I have a relationship with, by user id.
#[derive(Default)]
pub struct RelationshipStore {
    map: HashMap<String, Relationship>,
}

impl RelationshipStore {
    /// Replace everything (READY).
    pub fn set_all(&mut self, list: Vec<Relationship>) {
        self.map = list.into_iter().map(|r| (r.user.id.clone(), r)).collect();
    }

    /// Apply a gateway event. Returns true if anything changed.
    pub fn apply(&mut self, ev: RelationshipEvent) -> bool {
        match ev {
            RelationshipEvent::Add(r) => {
                self.map.insert(r.user.id.clone(), r);
                true
            }
            RelationshipEvent::Update {
                user_id,
                kind,
                nickname,
                since,
                user,
            } => match self.map.get_mut(&user_id) {
                Some(r) => {
                    if let Some(k) = kind {
                        r.kind = k;
                    }
                    if let Some(n) = nickname {
                        r.nickname = n;
                    }
                    if let Some(s) = since {
                        r.since = s;
                    }
                    if let Some(u) = user {
                        r.user = u;
                    }
                    true
                }
                // An update for someone we don't know: only usable if it's
                // complete enough to list them.
                None => match (kind, user) {
                    (Some(kind), user) => {
                        let user = user.unwrap_or_else(|| unknown_user(&user_id));
                        self.map.insert(
                            user_id,
                            Relationship {
                                user,
                                kind,
                                nickname: nickname.flatten(),
                                since: since.unwrap_or(0),
                                spam: false,
                            },
                        );
                        true
                    }
                    _ => false,
                },
            },
            RelationshipEvent::Remove { user_id } => self.map.remove(&user_id).is_some(),
        }
    }

    /// Drop one (optimistically, after a successful REST call).
    pub fn remove(&mut self, user_id: &str) {
        self.map.remove(user_id);
    }

    pub fn get(&self, user_id: &str) -> Option<&Relationship> {
        self.map.get(user_id)
    }

    pub fn kind(&self, user_id: &str) -> Option<RelKind> {
        self.map.get(user_id).map(|r| r.kind)
    }

    pub fn is_friend(&self, user_id: &str) -> bool {
        self.kind(user_id) == Some(RelKind::Friend)
    }

    pub fn is_blocked(&self, user_id: &str) -> bool {
        self.kind(user_id) == Some(RelKind::Blocked)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Relationship> {
        self.map.values()
    }

    pub fn count(&self, kind: RelKind) -> usize {
        self.map.values().filter(|r| r.kind == kind).count()
    }

    /// Incoming friend requests that aren't flagged as spam (the badge).
    pub fn incoming_count(&self) -> usize {
        self.map
            .values()
            .filter(|r| r.kind == RelKind::Incoming && !r.spam)
            .count()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn clear(&mut self) {
        self.map = HashMap::new();
    }
}

/// Tabs of the Friends page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FriendsTab {
    #[default]
    Online,
    All,
    Pending,
    Blocked,
    AddFriend,
}

impl FriendsTab {
    pub fn label(self) -> &'static str {
        match self {
            FriendsTab::Online => "Online",
            FriendsTab::All => "All",
            FriendsTab::Pending => "Pending",
            FriendsTab::Blocked => "Blocked",
            FriendsTab::AddFriend => "Add Friend",
        }
    }
}

/// The people listed on a tab, filtered by `query` and sorted like Discord:
/// by name; Pending puts incoming before outgoing and spam last.
pub fn tab_rows<'a>(
    store: &'a RelationshipStore,
    tab: FriendsTab,
    query: &str,
    status: &dyn Fn(&str) -> Status,
) -> Vec<&'a Relationship> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<&Relationship> = store
        .iter()
        .filter(|r| match tab {
            FriendsTab::Online => {
                r.kind == RelKind::Friend && status(&r.user.id) != Status::Offline
            }
            FriendsTab::All => r.kind == RelKind::Friend,
            FriendsTab::Pending => r.kind.is_pending(),
            FriendsTab::Blocked => r.kind == RelKind::Blocked,
            FriendsTab::AddFriend => false,
        })
        .filter(|r| r.matches(&q))
        .collect();
    rows.sort_by_cached_key(|r| {
        (
            r.spam,
            r.kind != RelKind::Incoming,
            r.display_name().to_lowercase(),
            r.user.id.clone(),
        )
    });
    rows
}

/// Clean up what was typed into "Add Friend": trims, drops a leading `@`,
/// lowercases. Discord usernames are 2–32 of `a-z 0-9 _ .`.
pub fn normalize_username(input: &str) -> Result<String, &'static str> {
    let name = input.trim().trim_start_matches('@').to_lowercase();
    if name.is_empty() {
        return Err("Type a username first.");
    }
    if name.contains('#') {
        return Err("Discord usernames don't have a #tag any more. Use their new username.");
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if !(2..=32).contains(&name.chars().count()) || !valid {
        return Err("Hm, didn't work. Double check that the username is correct.");
    }
    Ok(name)
}

/// Discord's wording for a failed friend request, with the server's
/// message kept for anything unusual.
pub fn friendly_add_error(err: &str) -> String {
    let low = err.to_lowercase();
    if low.contains("captcha") {
        "Discord wants a captcha for this request. Send it from the official app this time.".into()
    } else if low.contains("unknown user") || low.contains("80004") || low.contains("404") {
        "Hm, didn't work. Double check that the username is correct.".into()
    } else {
        format!("Hm, didn't work. {err}")
    }
}

// ---------------------------------------------------------------------------
// Message requests
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestInfo {
    pub channel_id: String,
    /// Discord filed it under Spam.
    pub spam: bool,
    /// When it became a request (unix seconds, 0 if unknown).
    pub since: i64,
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// A DM channel object that is a message request (or spam request).
pub fn is_request(v: &Value) -> bool {
    flag(v, "is_message_request") == Some(true) || flag(v, "is_spam") == Some(true)
}

/// Request info for a channel object, if it is one.
pub fn request_info(v: &Value) -> Option<RequestInfo> {
    if !is_request(v) {
        return None;
    }
    Some(RequestInfo {
        channel_id: v.get("id")?.as_str()?.to_string(),
        spam: flag(v, "is_spam") == Some(true),
        since: crate::timefmt::parse_iso(
            v.get("is_message_request_timestamp")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
    })
}

/// CHANNEL_CREATE / CHANNEL_UPDATE: `Some((id, Some(info)))` if it's (still)
/// a request, `Some((id, None))` if the payload says it no longer is, None
/// if the payload doesn't carry the flags at all (leave state alone).
pub fn request_update(v: &Value) -> Option<(String, Option<RequestInfo>)> {
    let id = v.get("id")?.as_str()?.to_string();
    if v.get("is_message_request").is_none() && v.get("is_spam").is_none() {
        return None;
    }
    Some((id, request_info(v)))
}

/// READY `private_channels` that are message requests.
pub fn parse_ready_requests(d: &Value) -> Vec<RequestInfo> {
    d["private_channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(request_info)
        .collect()
}

/// DM channels that are message requests, by channel id.
#[derive(Default)]
pub struct RequestStore {
    map: HashMap<String, RequestInfo>,
}

impl RequestStore {
    pub fn set_all(&mut self, list: Vec<RequestInfo>) {
        self.map = list
            .into_iter()
            .map(|r| (r.channel_id.clone(), r))
            .collect();
    }

    /// Apply `request_update`'s result.
    pub fn apply(&mut self, channel_id: String, info: Option<RequestInfo>) {
        match info {
            Some(i) => {
                self.map.insert(channel_id, i);
            }
            None => {
                self.map.remove(&channel_id);
            }
        }
    }

    /// CHANNEL_DELETE, or after Accept / Ignore succeeded.
    pub fn remove(&mut self, channel_id: &str) {
        self.map.remove(channel_id);
    }

    pub fn is_request(&self, channel_id: &str) -> bool {
        self.map.contains_key(channel_id)
    }

    pub fn get(&self, channel_id: &str) -> Option<&RequestInfo> {
        self.map.get(channel_id)
    }

    /// Requests that aren't spam (the badge count).
    pub fn count(&self) -> usize {
        self.map.values().filter(|r| !r.spam).count()
    }

    pub fn spam_count(&self) -> usize {
        self.map.values().filter(|r| r.spam).count()
    }

    /// The normal DM list: everything that isn't a request.
    pub fn visible_dms<'a>(
        &'a self,
        dms: impl IntoIterator<Item = &'a Channel> + 'a,
    ) -> impl Iterator<Item = &'a Channel> + 'a {
        dms.into_iter().filter(|c| !self.is_request(&c.id))
    }

    /// Requests (or spam) with their channel, newest first.
    pub fn rows<'a>(
        &'a self,
        dms: &'a [Channel],
        spam: bool,
    ) -> Vec<(&'a Channel, &'a RequestInfo)> {
        let mut rows: Vec<_> = dms
            .iter()
            .filter_map(|c| Some((c, self.map.get(&c.id)?)))
            .filter(|(_, i)| i.spam == spam)
            .collect();
        rows.sort_by_key(|(c, i)| std::cmp::Reverse((i.since, c.last_message_id)));
        rows
    }

    pub fn clear(&mut self) {
        self.map = HashMap::new();
    }
}

// ---------------------------------------------------------------------------
// Actions (UI → backend)
// ---------------------------------------------------------------------------

/// What the user clicked on the Friends page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FriendsAction {
    /// Open (or create) the DM with this user.
    Message(String),
    /// Accept an incoming request.
    Accept(String),
    /// Ignore an incoming request.
    Ignore(String),
    /// Cancel an outgoing request.
    Cancel(String),
    /// Remove a friend (after confirm).
    Remove(String),
    /// Block (after confirm).
    Block(String),
    Unblock(String),
    /// "Send Friend Request" on the Add Friend tab (normalised username).
    SendRequest(String),
}

/// What the user clicked on the Message Requests page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestAction {
    /// View the request's messages (UI only; no network call, no ack).
    Open(String),
    Accept(String),
    Ignore(String),
}

/// Result of a successful `execute`.
#[derive(Debug)]
pub enum Outcome {
    Done,
    /// The DM channel object from POST users/@me/channels.
    DmOpened(Value),
    /// Friend request sent to this username.
    RequestSent(String),
}

/// Run a Friends action against the API (backend side).
pub async fn execute(api: &Api, action: &FriendsAction) -> Result<Outcome> {
    match action {
        FriendsAction::Message(id) => return Ok(Outcome::DmOpened(api.open_dm(id).await?)),
        FriendsAction::Accept(id) => api.add_friend(id).await?,
        FriendsAction::Ignore(id)
        | FriendsAction::Cancel(id)
        | FriendsAction::Remove(id)
        | FriendsAction::Unblock(id) => api.remove_relationship(id).await?,
        FriendsAction::Block(id) => api.block_user(id).await?,
        FriendsAction::SendRequest(name) => {
            api.send_friend_request(name).await?;
            return Ok(Outcome::RequestSent(name.clone()));
        }
    }
    Ok(Outcome::Done)
}

/// Run a Message Requests action (backend side). `Open` does nothing here.
pub async fn execute_request(api: &Api, action: &RequestAction) -> Result<()> {
    match action {
        RequestAction::Open(_) => Ok(()),
        RequestAction::Accept(id) => api.accept_message_request(id).await,
        RequestAction::Ignore(id) => api.reject_message_request(id).await,
    }
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

/// Paints a user's avatar into a rect (the integrator passes
/// `app::paint_avatar` with its Config so the Images module is respected).
pub type AvatarFn<'a> = &'a dyn Fn(&egui::Ui, egui::Rect, &User);

const HEADER_H: f32 = 48.0;
const ROW_H: f32 = 62.0;

/// Page header bar: icon + title, then whatever `tabs` adds.
fn header_bar<R>(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: &str,
    title: &str,
    tabs: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, pal.divider),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(16.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    child.label(RichText::new(icon).size(20.0).color(pal.muted));
    child.label(RichText::new(title).size(15.0).bold().color(pal.strong));
    let (sep, _) = child.allocate_exact_size(vec2(9.0, 24.0), egui::Sense::hover());
    child.painter().vline(
        sep.center().x,
        sep.y_range(),
        egui::Stroke::new(1.0, pal.divider),
    );
    tabs(&mut child)
}

/// A header tab: rounded highlight when selected / hovered, optional red
/// count badge.
fn tab(ui: &mut egui::Ui, pal: &Palette, label: &str, selected: bool, badge: usize) -> bool {
    let font = egui::FontId::proportional(15.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, pal.text);
    let badge_w = if badge > 0 {
        badge_width(badge) + 6.0
    } else {
        0.0
    };
    let size = vec2(galley.size().x + 16.0 + badge_w, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let (fill, color) = if selected {
        (pal.selected, pal.strong)
    } else if resp.hovered() {
        (pal.hover, pal.text)
    } else {
        (Color32::TRANSPARENT, pal.muted)
    };
    ui.painter().rect_filled(rect, 4, fill);
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if badge > 0 {
        count_badge(
            ui,
            egui::pos2(rect.right() - 8.0, rect.center().y),
            badge,
            pal,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn badge_width(n: usize) -> f32 {
    let digits = n.min(999).to_string().len() as f32;
    (8.0 + digits * 7.0).max(16.0)
}

/// Red rounded count pill, right-aligned at `right_center`.
pub fn count_badge(ui: &egui::Ui, right_center: egui::Pos2, n: usize, pal: &Palette) {
    let w = badge_width(n);
    let rect = egui::Rect::from_min_max(
        egui::pos2(right_center.x - w, right_center.y - 8.0),
        egui::pos2(right_center.x, right_center.y + 8.0),
    );
    ui.painter().rect_filled(rect, 8, pal.mention);
    let text = if n > 999 {
        "999+".into()
    } else {
        n.to_string()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        theme::bold_font(11.5),
        Color32::WHITE,
    );
}

/// Round icon button (Discord's row actions). `danger` / `good` tint the
/// icon on hover.
fn round_button(
    ui: &mut egui::Ui,
    pal: &Palette,
    center: egui::Pos2,
    id: egui::Id,
    icon: &str,
    hover_color: Color32,
    tip: &str,
) -> egui::Response {
    let rect = egui::Rect::from_center_size(center, vec2(36.0, 36.0));
    let resp = ui.interact(rect, id, egui::Sense::click());
    let fill = if resp.hovered() {
        pal.servers
    } else {
        pal.sidebar
    };
    ui.painter().circle_filled(center, 18.0, fill);
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(15.0),
        if resp.hovered() {
            hover_color
        } else {
            pal.muted
        },
    );
    resp.on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Search field with a 🔍 suffix. Escape clears it.
fn search_box(ui: &mut egui::Ui, pal: &Palette, text: &mut String, hint: &str, id: &str) {
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .id_salt(id)
                        .hint_text(RichText::new(hint).color(pal.muted))
                        .font(egui::FontId::proportional(14.5))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_width(ui.available_width() - 24.0),
                );
                if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    text.clear();
                }
                let icon = if text.is_empty() { "🔍" } else { "✕" };
                let btn = ui.add(
                    egui::Label::new(RichText::new(icon).color(pal.muted))
                        .selectable(false)
                        .sense(egui::Sense::click()),
                );
                if btn.clicked() {
                    text.clear();
                }
            });
        });
}

/// Centred grey message for empty lists.
fn empty_state(ui: &mut egui::Ui, pal: &Palette, icon: &str, text: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon).size(22.0).color(pal.muted));
        ui.add_space(8.0);
        ui.label(RichText::new(text).color(pal.muted));
    });
}

// ---------------------------------------------------------------------------
// Friends page
// ---------------------------------------------------------------------------

/// What the Friends page needs from the app each frame.
pub struct FriendsCtx<'a> {
    pub pal: &'a Palette,
    pub store: &'a RelationshipStore,
    /// Bot sessions have no friends; the page says so.
    pub is_bot: bool,
    /// Last known presence of a user (None = offline / unknown).
    pub presence: &'a dyn Fn(&str) -> Option<Presence>,
    pub avatar: AvatarFn<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AddStatus {
    Idle,
    Sending(String),
    Sent(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfirmKind {
    Remove,
    Block,
}

struct Confirm {
    kind: ConfirmKind,
    user_id: String,
    name: String,
}

/// The Friends page with its own UI state (tab, search, Add Friend input).
pub struct FriendsPage {
    pub tab: FriendsTab,
    search: String,
    add_input: String,
    add_status: AddStatus,
    confirm: Option<Confirm>,
}

impl Default for FriendsPage {
    fn default() -> Self {
        Self {
            tab: FriendsTab::Online,
            search: String::new(),
            add_input: String::new(),
            add_status: AddStatus::Idle,
            confirm: None,
        }
    }
}

impl FriendsPage {
    /// The backend's answer to `FriendsAction::SendRequest`:
    /// `Ok(username)` or `Err(error text)`.
    pub fn add_result(&mut self, result: Result<String, String>) {
        self.add_status = match result {
            Ok(name) => {
                self.add_input.clear();
                AddStatus::Sent(name)
            }
            Err(e) => AddStatus::Failed(friendly_add_error(&e)),
        };
    }

    pub fn show(&mut self, ui: &mut egui::Ui, cx: &FriendsCtx<'_>) -> Option<FriendsAction> {
        let pal = cx.pal;
        let mut action = None;
        let incoming = cx.store.incoming_count();
        let status = |id: &str| (cx.presence)(id).map(|p| p.status).unwrap_or_default();
        let counts = [
            tab_rows(cx.store, FriendsTab::Online, "", &status).len(),
            cx.store.count(RelKind::Friend),
            cx.store.count(RelKind::Incoming) + cx.store.count(RelKind::Outgoing),
            cx.store.count(RelKind::Blocked),
        ];

        ui.spacing_mut().item_spacing.y = 0.0;
        header_bar(ui, pal, "👥", "Friends", |ui| {
            let tabs = [
                FriendsTab::Online,
                FriendsTab::All,
                FriendsTab::Pending,
                FriendsTab::Blocked,
            ];
            for (t, n) in tabs.into_iter().zip(counts) {
                // Pending shows the red badge for incoming requests; the
                // others carry their count in the label.
                let (label, badge) = match t {
                    FriendsTab::Pending => (t.label().to_string(), incoming),
                    _ if n > 0 && !cx.is_bot => (format!("{} {n}", t.label()), 0),
                    _ => (t.label().to_string(), 0),
                };
                if tab(ui, pal, &label, self.tab == t, badge) {
                    self.tab = t;
                }
            }
            if add_friend_tab(ui, pal, self.tab == FriendsTab::AddFriend) {
                self.tab = FriendsTab::AddFriend;
            }
        });

        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 20,
                right: 20,
                top: 16,
                bottom: 8,
            })
            .show(ui, |ui| {
                if cx.is_bot {
                    empty_state(
                        ui,
                        pal,
                        "🤖",
                        "Friends aren't available for bot accounts. Log in with a user account to see them.",
                    );
                    return;
                }
                if self.tab == FriendsTab::AddFriend {
                    action = self.add_friend_ui(ui, pal);
                    return;
                }
                search_box(ui, pal, &mut self.search, "Search", "friends_search");
                ui.add_space(16.0);
                let rows = tab_rows(cx.store, self.tab, &self.search, &status);
                let label = match self.tab {
                    FriendsTab::Online => "Online",
                    FriendsTab::All => "All friends",
                    FriendsTab::Pending => "Pending",
                    _ => "Blocked",
                };
                ui.label(pal.section_label(&format!("{label} — {}", rows.len())));
                ui.add_space(8.0);
                if rows.is_empty() {
                    let text = if !self.search.trim().is_empty() {
                        "Wumpus looked, but couldn't find anyone with that name."
                    } else {
                        match self.tab {
                            FriendsTab::Online => "No one's around to play with Wumpus.",
                            FriendsTab::All => {
                                "Wumpus is waiting on friends. You don't have to though!"
                            }
                            FriendsTab::Pending => {
                                "There are no pending friend requests. Here's Wumpus for now."
                            }
                            _ => "You can't unblock the Wumpus.",
                        }
                    };
                    empty_state(ui, pal, "🌙", text);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("friends_list")
                    .auto_shrink(false)
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for r in &rows[range] {
                            if let Some(a) = self.friend_row(ui, r, cx) {
                                action = Some(a);
                            }
                        }
                    });
            });

        if let Some(a) = self.confirm_modal(ui, pal) {
            action = Some(a);
        }
        action
    }

    fn friend_row(
        &mut self,
        ui: &mut egui::Ui,
        r: &Relationship,
        cx: &FriendsCtx<'_>,
    ) -> Option<FriendsAction> {
        let pal = cx.pal;
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), egui::Sense::click());
        let id = &r.user.id;
        let menu_id = egui::Id::unique(("friend_menu", id));
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
        let inner = rect.shrink2(vec2(0.0, 1.0));
        if hovered {
            ui.painter().rect_filled(inner, 8, pal.hover);
        } else {
            ui.painter().hline(
                rect.shrink2(vec2(10.0, 0.0)).x_range(),
                rect.top() + 0.5,
                egui::Stroke::new(1.0, pal.divider),
            );
        }
        let presence = (cx.presence)(id).unwrap_or_default();

        // Avatar + status dot (friends only, like Discord).
        let av = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 10.0 + 16.0, rect.center().y),
            vec2(32.0, 32.0),
        );
        (cx.avatar)(ui, av, &r.user);
        if r.kind == RelKind::Friend {
            let ring = if hovered { pal.hover } else { pal.chat };
            status_dot(ui, av, presence.status, ring, pal);
        }

        // Name (+ @username on hover) and the status line.
        let x = av.right() + 12.0;
        let buttons_w = match r.kind {
            RelKind::Friend | RelKind::Incoming => 2.0 * 46.0,
            _ => 46.0,
        };
        let max_w = (rect.right() - 10.0 - buttons_w - x).max(20.0);
        let name = ui.painter().layout_no_wrap(
            r.display_name().to_string(),
            theme::bold_font(15.0),
            pal.strong,
        );
        let name_w = name.size().x.min(max_w);
        let top = rect.center().y - 18.0;
        ui.painter()
            .with_clip_rect(egui::Rect::from_min_size(
                egui::pos2(x, top),
                vec2(max_w, 20.0),
            ))
            .galley(egui::pos2(x, top), name, pal.strong);
        if hovered && !r.user.username.is_empty() {
            let at = ui.painter().layout_no_wrap(
                r.user.username.clone(),
                egui::FontId::proportional(14.5),
                pal.muted,
            );
            let ax = x + name_w + 6.0;
            if ax + 20.0 < x + max_w {
                ui.painter()
                    .with_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(ax, top),
                        egui::pos2(x + max_w, top + 20.0),
                    ))
                    .galley(egui::pos2(ax, top + 1.0), at, pal.muted);
            }
        }
        let sub = match r.kind {
            RelKind::Friend => presence
                .activity
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| presence.status.label().to_string()),
            RelKind::Incoming if r.spam => "Incoming Friend Request (possible spam)".into(),
            RelKind::Incoming => "Incoming Friend Request".into(),
            RelKind::Outgoing => "Outgoing Friend Request".into(),
            RelKind::Blocked => "Blocked".into(),
            _ => String::new(),
        };
        let sub = ui
            .painter()
            .layout_no_wrap(sub, egui::FontId::proportional(13.0), pal.muted);
        ui.painter()
            .with_clip_rect(egui::Rect::from_min_size(
                egui::pos2(x, top + 20.0),
                vec2(max_w, 18.0),
            ))
            .galley(egui::pos2(x, top + 21.0), sub, pal.muted);

        // Action buttons, right to left.
        let mut action = None;
        let mut cx_btn = rect.right() - 10.0 - 18.0;
        let cy = rect.center().y;
        let mut btn = |icon: &str, hover: Color32, tip: &str, key: &str| {
            let b = round_button(
                ui,
                pal,
                egui::pos2(cx_btn, cy),
                egui::Id::unique(("friend_btn", key, id)),
                icon,
                hover,
                tip,
            );
            cx_btn -= 46.0;
            b
        };
        match r.kind {
            RelKind::Friend => {
                let more = btn("⋯", pal.text, "More", "more");
                let msg = btn("💬", pal.text, "Message", "msg");
                if msg.clicked() {
                    action = Some(FriendsAction::Message(id.clone()));
                }
                egui::Popup::menu(&more).id(menu_id).show(|ui| {
                    ui.set_min_width(160.0);
                    if ui
                        .add(egui::Button::new(
                            RichText::new("Remove Friend").color(pal.danger),
                        ))
                        .clicked()
                    {
                        self.confirm = Some(Confirm {
                            kind: ConfirmKind::Remove,
                            user_id: id.clone(),
                            name: r.display_name().to_string(),
                        });
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Block").color(pal.danger)))
                        .clicked()
                    {
                        self.confirm = Some(Confirm {
                            kind: ConfirmKind::Block,
                            user_id: id.clone(),
                            name: r.display_name().to_string(),
                        });
                    }
                });
                if resp.clicked() {
                    action = Some(FriendsAction::Message(id.clone()));
                }
            }
            RelKind::Incoming => {
                if btn("✕", pal.danger, "Ignore", "ignore").clicked() {
                    action = Some(FriendsAction::Ignore(id.clone()));
                }
                if btn("✔", pal.online, "Accept", "accept").clicked() {
                    action = Some(FriendsAction::Accept(id.clone()));
                }
            }
            RelKind::Outgoing | RelKind::Blocked => {
                let (tip, undo) = if r.kind == RelKind::Outgoing {
                    ("Cancel", FriendsAction::Cancel(id.clone()))
                } else {
                    ("Unblock", FriendsAction::Unblock(id.clone()))
                };
                if btn("✕", pal.danger, tip, tip).clicked() {
                    action = Some(undo);
                }
            }
            _ => {}
        }
        action
    }

    fn add_friend_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<FriendsAction> {
        let mut action = None;
        ui.label(
            RichText::new("ADD FRIEND")
                .size(15.0)
                .bold()
                .color(pal.strong),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("You can add friends with their Discord username.").color(pal.muted),
        );
        ui.add_space(14.0);
        let sending = matches!(self.add_status, AddStatus::Sending(_));
        let border = match &self.add_status {
            AddStatus::Sent(_) => pal.online,
            AddStatus::Failed(_) => pal.danger,
            _ => pal.divider,
        };
        egui::Frame::new()
            .fill(pal.servers)
            .corner_radius(8)
            .stroke(egui::Stroke::new(1.0, border))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let btn_w = 170.0;
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.add_input)
                            .id_salt("add_friend_input")
                            .hint_text(
                                RichText::new("You can add friends with their Discord username.")
                                    .color(pal.muted),
                            )
                            .font(egui::FontId::proportional(15.0))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_width((ui.available_width() - btn_w).max(60.0)),
                    );
                    if resp.changed() && !sending {
                        self.add_status = AddStatus::Idle;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let can_send = !self.add_input.trim().is_empty() && !sending;
                    let clicked = ui
                        .add_enabled(can_send, pal.primary_button("Send Friend Request"))
                        .clicked();
                    if (clicked || enter) && can_send {
                        match normalize_username(&self.add_input) {
                            Ok(name) => {
                                self.add_status = AddStatus::Sending(name.clone());
                                action = Some(FriendsAction::SendRequest(name));
                            }
                            Err(e) => self.add_status = AddStatus::Failed(e.to_string()),
                        }
                    }
                });
            });
        ui.add_space(8.0);
        match &self.add_status {
            AddStatus::Idle => {}
            AddStatus::Sending(name) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(format!("Sending a request to {name}…")).color(pal.muted),
                    );
                });
            }
            AddStatus::Sent(name) => {
                ui.label(
                    RichText::new(format!("Success! Your friend request to {name} was sent."))
                        .color(pal.online),
                );
            }
            AddStatus::Failed(e) => {
                ui.label(RichText::new(e).color(pal.danger));
            }
        }
        action
    }

    fn confirm_modal(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<FriendsAction> {
        let c = self.confirm.as_ref()?;
        let mut action = None;
        let mut close = false;
        let (title, body, button) = match c.kind {
            ConfirmKind::Remove => (
                format!("Remove '{}'", c.name),
                format!(
                    "Are you sure you want to remove {} from your friends?",
                    c.name
                ),
                "Remove Friend",
            ),
            ConfirmKind::Block => (
                format!("Block {}?", c.name),
                format!(
                    "Are you sure you want to block {}? Blocking also removes them from your friends.",
                    c.name
                ),
                "Block",
            ),
        };
        let resp = egui::Modal::new(egui::Id::unique("friends_confirm")).show(ui.ctx(), |ui| {
            ui.set_max_width(380.0);
            ui.heading(title);
            ui.add_space(6.0);
            ui.label(body);
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new(button).color(Color32::WHITE).bold())
                            .fill(pal.danger)
                            .corner_radius(4),
                    )
                    .clicked()
                {
                    action = Some(match c.kind {
                        ConfirmKind::Remove => FriendsAction::Remove(c.user_id.clone()),
                        ConfirmKind::Block => FriendsAction::Block(c.user_id.clone()),
                    });
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || resp.should_close() {
            self.confirm = None;
        }
        action
    }
}

/// Discord's green "Add Friend" tab (outlined green text when selected).
fn add_friend_tab(ui: &mut egui::Ui, pal: &Palette, selected: bool) -> bool {
    let galley = ui.painter().layout_no_wrap(
        "Add Friend".to_string(),
        theme::bold_font(15.0),
        Color32::WHITE,
    );
    let (rect, resp) =
        ui.allocate_exact_size(vec2(galley.size().x + 16.0, 26.0), egui::Sense::click());
    let (fill, color) = if selected {
        (Color32::TRANSPARENT, pal.online)
    } else {
        (pal.online, Color32::WHITE)
    };
    ui.painter().rect_filled(rect, 4, fill);
    if selected && resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

// ---------------------------------------------------------------------------
// Message Requests page
// ---------------------------------------------------------------------------

pub struct RequestsCtx<'a> {
    pub pal: &'a Palette,
    pub store: &'a RequestStore,
    /// All DM channels (requests included).
    pub dms: &'a [Channel],
    pub is_bot: bool,
    /// Last message text of a channel, if loaded.
    pub preview: &'a dyn Fn(&str) -> Option<String>,
    pub avatar: AvatarFn<'a>,
}

#[derive(Default)]
pub struct MessageRequestsPage {
    spam: bool,
}

impl MessageRequestsPage {
    pub fn show(&mut self, ui: &mut egui::Ui, cx: &RequestsCtx<'_>) -> Option<RequestAction> {
        let pal = cx.pal;
        let spam_count = cx.store.spam_count();
        if spam_count == 0 {
            self.spam = false;
        }
        let mut action = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        header_bar(ui, pal, "✉", "Message Requests", |ui| {
            if tab(ui, pal, "Requests", !self.spam, cx.store.count()) {
                self.spam = false;
            }
            if spam_count > 0 && tab(ui, pal, &format!("Spam {spam_count}"), self.spam, 0) {
                self.spam = true;
            }
        });
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 20,
                right: 20,
                top: 16,
                bottom: 8,
            })
            .show(ui, |ui| {
                if cx.is_bot {
                    empty_state(
                        ui,
                        pal,
                        "🤖",
                        "Message requests aren't available for bot accounts.",
                    );
                    return;
                }
                let rows = cx.store.rows(cx.dms, self.spam);
                let label = if self.spam { "Spam" } else { "Message requests" };
                ui.label(pal.section_label(&format!("{label} — {}", rows.len())));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Messages from people you may not know. They won't know you've seen it until you accept.",
                    )
                    .small()
                    .color(pal.muted),
                );
                ui.add_space(8.0);
                if rows.is_empty() {
                    empty_state(ui, pal, "✉", "No pending requests. Nice!");
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("requests_list")
                    .auto_shrink(false)
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (c, info) in &rows[range] {
                            if let Some(a) = request_row(ui, c, info, cx) {
                                action = Some(a);
                            }
                        }
                    });
            });
        action
    }
}

fn request_row(
    ui: &mut egui::Ui,
    c: &Channel,
    info: &RequestInfo,
    cx: &RequestsCtx<'_>,
) -> Option<RequestAction> {
    let pal = cx.pal;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), egui::Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(0.0, 1.0)), 8, pal.hover);
    } else {
        ui.painter().hline(
            rect.shrink2(vec2(10.0, 0.0)).x_range(),
            rect.top() + 0.5,
            egui::Stroke::new(1.0, pal.divider),
        );
    }
    let av = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        vec2(32.0, 32.0),
    );
    match c.recipients.first() {
        Some(u) => (cx.avatar)(ui, av, u),
        None => {
            ui.painter().circle_filled(av.center(), 16.0, pal.accent);
        }
    }

    // Accept / Ignore buttons on the right.
    let mut action = None;
    let mut right = rect.right() - 10.0;
    for (label, primary) in [("Accept", true), ("Ignore", false)] {
        let galley =
            ui.painter()
                .layout_no_wrap(label.into(), theme::bold_font(13.0), Color32::WHITE);
        let w = galley.size().x + 24.0;
        let b = egui::Rect::from_min_max(
            egui::pos2(right - w, rect.center().y - 15.0),
            egui::pos2(right, rect.center().y + 15.0),
        );
        let r = ui.interact(
            b,
            egui::Id::unique(("req_btn", label, &c.id)),
            egui::Sense::click(),
        );
        let (fill, text) = if primary {
            let f = if r.hovered() {
                pal.accent.gamma_multiply(0.85)
            } else {
                pal.accent
            };
            (f, Color32::WHITE)
        } else {
            (if r.hovered() { pal.selected } else { pal.input }, pal.text)
        };
        ui.painter().rect_filled(b, 4, fill);
        ui.painter()
            .galley(b.center() - galley.size() / 2.0, galley, text);
        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Some(if primary {
                RequestAction::Accept(c.id.clone())
            } else {
                RequestAction::Ignore(c.id.clone())
            });
        }
        right = b.left() - 8.0;
    }

    let x = av.right() + 12.0;
    let max_w = (right - 8.0 - x).max(20.0);
    let top = rect.center().y - 18.0;
    let mut title = c.name.clone();
    if let Some(u) = c.recipients.first().filter(|u| !u.username.is_empty()) {
        title = format!("{title}  @{}", u.username);
    }
    let when = crate::timefmt::full(info.since);
    let name = ui
        .painter()
        .layout_no_wrap(c.name.clone(), theme::bold_font(15.0), pal.strong);
    let name_w = name.size().x;
    let clip = |y: f32, h: f32| egui::Rect::from_min_size(egui::pos2(x, y), vec2(max_w, h));
    ui.painter()
        .with_clip_rect(clip(top, 20.0))
        .galley(egui::pos2(x, top), name, pal.strong);
    if !when.is_empty() && name_w + 12.0 < max_w {
        ui.painter().with_clip_rect(clip(top, 20.0)).text(
            egui::pos2(x + name_w + 8.0, top + 3.0),
            egui::Align2::LEFT_TOP,
            when,
            egui::FontId::proportional(12.0),
            pal.muted,
        );
    }
    let preview = (cx.preview)(&c.id)
        .map(|p| p.replace('\n', " "))
        .unwrap_or_else(|| "Sent you a message".into());
    ui.painter().with_clip_rect(clip(top + 20.0, 18.0)).text(
        egui::pos2(x, top + 21.0),
        egui::Align2::LEFT_TOP,
        preview,
        egui::FontId::proportional(13.0),
        pal.muted,
    );

    if action.is_none() && resp.clicked() {
        action = Some(RequestAction::Open(c.id.clone()));
    }
    if hovered {
        resp.on_hover_text(title)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store(v: Value) -> RelationshipStore {
        let mut s = RelationshipStore::default();
        s.set_all(parse_ready(&v));
        s
    }

    #[test]
    fn ready_with_full_users_and_user_ids() {
        let s = store(json!({
            "relationships": [
                { "id": "1", "type": 1, "nickname": "Bestie", "since": "2024-01-01T00:00:00+00:00",
                  "user": { "id": "1", "username": "alice", "global_name": "Alice" } },
                { "id": "2", "type": 3, "user_id": "2", "is_spam_request": true },
                { "id": "3", "type": 4, "user_id": "3" },
                { "id": "4", "type": 2 },
                { "id": "5", "type": 5, "user_id": "5" },
                { "type": 1 },
            ],
            "users": [
                { "id": "2", "username": "bob" },
                { "id": "3", "username": "carol", "global_name": "Carol" },
            ],
        }));
        assert_eq!(s.len(), 5);
        let a = s.get("1").unwrap();
        assert_eq!(a.kind, RelKind::Friend);
        assert_eq!(a.display_name(), "Bestie");
        assert_eq!(a.user.username, "alice");
        assert!(a.since > 0);
        let b = s.get("2").unwrap();
        assert_eq!(
            (b.kind, b.spam, b.user.name.as_str()),
            (RelKind::Incoming, true, "bob")
        );
        assert_eq!(s.get("3").unwrap().display_name(), "Carol");
        // Not in `users`: still listed, by id.
        let d = s.get("4").unwrap();
        assert_eq!((d.kind, d.user.id.as_str()), (RelKind::Blocked, "4"));
        assert_eq!(s.kind("5"), Some(RelKind::Implicit));
        assert!(s.is_friend("1") && s.is_blocked("4"));
        // Spam requests don't count towards the badge.
        assert_eq!(s.incoming_count(), 0);
    }

    #[test]
    fn missing_fields_are_harmless() {
        assert!(parse_ready(&json!({})).is_empty());
        assert!(parse_ready(&json!({ "relationships": null })).is_empty());
        let s = store(json!({ "relationships": [{ "id": "9", "type": 77, "nickname": "  " }] }));
        let r = s.get("9").unwrap();
        assert_eq!(r.kind, RelKind::Unknown);
        assert_eq!(r.nickname, None);
        assert_eq!(r.display_name(), "Unknown user");
    }

    #[test]
    fn gateway_events() {
        let mut s = RelationshipStore::default();
        let add = RelationshipEvent::parse(
            "RELATIONSHIP_ADD",
            &json!({ "id": "7", "type": 3, "user": { "id": "7", "username": "dan" } }),
        )
        .unwrap();
        assert!(s.apply(add));
        assert_eq!(s.incoming_count(), 1);

        // Accepted elsewhere: partial update to type 1, plus a nickname.
        let upd = RelationshipEvent::parse(
            "RELATIONSHIP_UPDATE",
            &json!({ "id": "7", "type": 1, "nickname": "Danny" }),
        )
        .unwrap();
        assert!(s.apply(upd));
        let r = s.get("7").unwrap();
        assert_eq!((r.kind, r.display_name()), (RelKind::Friend, "Danny"));
        assert_eq!(r.user.username, "dan");

        // Null nickname clears; missing nickname keeps.
        s.apply(RelationshipEvent::parse("RELATIONSHIP_UPDATE", &json!({ "id": "7" })).unwrap());
        assert_eq!(s.get("7").unwrap().display_name(), "Danny");
        s.apply(
            RelationshipEvent::parse(
                "RELATIONSHIP_UPDATE",
                &json!({ "id": "7", "nickname": null }),
            )
            .unwrap(),
        );
        assert_eq!(s.get("7").unwrap().nickname, None);

        // Update for an unknown user without a type: ignored.
        assert!(!s.apply(
            RelationshipEvent::parse("RELATIONSHIP_UPDATE", &json!({ "id": "8" })).unwrap()
        ));
        // With a type: added.
        assert!(
            s.apply(
                RelationshipEvent::parse("RELATIONSHIP_UPDATE", &json!({ "id": "8", "type": 2 }))
                    .unwrap()
            )
        );
        assert!(s.is_blocked("8"));

        let rm = RelationshipEvent::parse("RELATIONSHIP_REMOVE", &json!({ "id": "7", "type": 1 }))
            .unwrap();
        assert!(s.apply(rm));
        assert!(s.get("7").is_none());
        assert!(!s.apply(RelationshipEvent::Remove {
            user_id: "7".into()
        }));

        assert!(RelationshipEvent::parse("MESSAGE_CREATE", &json!({ "id": "1" })).is_none());
        assert!(RelationshipEvent::parse("RELATIONSHIP_REMOVE", &json!({})).is_none());
    }

    #[test]
    fn tabs_filter_and_sort() {
        let s = store(json!({ "relationships": [
            { "id": "1", "type": 1, "user": { "id": "1", "username": "zed", "global_name": "Zed" } },
            { "id": "2", "type": 1, "user": { "id": "2", "username": "amy", "global_name": "Amy" } },
            { "id": "3", "type": 1, "nickname": "Boss", "user": { "id": "3", "username": "carl" } },
            { "id": "4", "type": 4, "user": { "id": "4", "username": "aaron" } },
            { "id": "5", "type": 3, "user": { "id": "5", "username": "yuri" } },
            { "id": "6", "type": 3, "is_spam_request": true, "user": { "id": "6", "username": "abc" } },
            { "id": "7", "type": 2, "user": { "id": "7", "username": "troll" } },
        ]}));
        let online = |id: &str| {
            if id == "1" {
                Status::Idle
            } else {
                Status::Offline
            }
        };
        let ids = |tab, q| -> Vec<String> {
            tab_rows(&s, tab, q, &online)
                .iter()
                .map(|r| r.user.id.clone())
                .collect()
        };
        assert_eq!(ids(FriendsTab::Online, ""), ["1"]);
        assert_eq!(ids(FriendsTab::All, ""), ["2", "3", "1"]);
        // Incoming first, then outgoing, spam last.
        assert_eq!(ids(FriendsTab::Pending, ""), ["5", "4", "6"]);
        assert_eq!(ids(FriendsTab::Blocked, ""), ["7"]);
        assert!(ids(FriendsTab::AddFriend, "").is_empty());
        // Search: nickname, display name, username; case-insensitive.
        assert_eq!(ids(FriendsTab::All, "boss"), ["3"]);
        assert_eq!(ids(FriendsTab::All, " CARL "), ["3"]);
        assert_eq!(ids(FriendsTab::All, "am"), ["2"]);
        assert!(ids(FriendsTab::All, "nobody").is_empty());
    }

    #[test]
    fn usernames() {
        assert_eq!(
            normalize_username("  @Leo.Dev_1 ").as_deref(),
            Ok("leo.dev_1")
        );
        assert!(normalize_username("   ").is_err());
        assert!(normalize_username("old#1234").is_err());
        assert!(normalize_username("a").is_err());
        assert!(normalize_username("has space").is_err());
        assert!(normalize_username(&"x".repeat(33)).is_err());
        assert!(friendly_add_error("HTTP 400 - captcha-required").contains("captcha"));
        assert!(friendly_add_error("HTTP 400 - Unknown User").contains("Double check"));
        assert!(friendly_add_error("HTTP 429 - slow down").contains("slow down"));
    }

    fn dm(id: &str, last: u64) -> Channel {
        Channel::from_json(&json!({
            "id": id, "type": 1, "last_message_id": last.to_string(),
            "recipients": [{ "id": format!("u{id}"), "username": format!("user{id}") }],
        }))
    }

    #[test]
    fn message_request_flags() {
        let req = json!({ "id": "10", "type": 1, "is_message_request": true,
            "is_message_request_timestamp": "2026-11-01T10:00:00+00:00", "is_spam": false });
        assert!(is_request(&req));
        let info = request_info(&req).unwrap();
        assert_eq!(info.channel_id, "10");
        assert!(!info.spam && info.since > 0);

        let spam = json!({ "id": "11", "type": 1, "is_spam": true });
        assert!(request_info(&spam).unwrap().spam);

        let normal = json!({ "id": "12", "type": 1 });
        assert!(!is_request(&normal));
        assert!(request_info(&normal).is_none());
        // No flags in an update: leave state alone.
        assert_eq!(request_update(&normal), None);
        // Accepted: flag present and false.
        let accepted = json!({ "id": "10", "is_message_request": false, "is_spam": false });
        assert_eq!(request_update(&accepted), Some(("10".into(), None)));
        assert_eq!(
            request_update(&req).map(|(id, i)| (id, i.is_some())),
            Some(("10".into(), true))
        );
    }

    #[test]
    fn request_store_filters_dm_list() {
        let ready = json!({ "private_channels": [
            { "id": "1", "type": 1 },
            { "id": "2", "type": 1, "is_message_request": true,
              "is_message_request_timestamp": "2026-11-01T10:00:00+00:00" },
            { "id": "3", "type": 1, "is_message_request": true,
              "is_message_request_timestamp": "2026-11-02T10:00:00+00:00" },
            { "id": "4", "type": 1, "is_spam": true },
        ]});
        let mut s = RequestStore::default();
        s.set_all(parse_ready_requests(&ready));
        assert_eq!((s.count(), s.spam_count()), (2, 1));
        let dms = vec![dm("1", 50), dm("2", 40), dm("3", 30), dm("4", 20)];
        let visible: Vec<&str> = s.visible_dms(&dms).map(|c| c.id.as_str()).collect();
        assert_eq!(visible, ["1"]);
        // Newest request first.
        let rows: Vec<&str> = s
            .rows(&dms, false)
            .iter()
            .map(|(c, _)| c.id.as_str())
            .collect();
        assert_eq!(rows, ["3", "2"]);
        assert_eq!(s.rows(&dms, true).len(), 1);

        // Accepted via CHANNEL_UPDATE, ignored via remove.
        let (id, info) =
            request_update(&json!({ "id": "2", "is_message_request": false })).unwrap();
        s.apply(id, info);
        s.remove("3");
        assert!(!s.is_request("2") && !s.is_request("3"));
        assert_eq!(s.count(), 0);
        // A new request arrives with CHANNEL_CREATE.
        let (id, info) =
            request_update(&json!({ "id": "5", "type": 1, "is_message_request": true })).unwrap();
        s.apply(id, info);
        assert!(s.is_request("5"));
    }
}
