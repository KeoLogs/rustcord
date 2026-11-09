//! Server invites: finding invite links in messages, the invite card under a
//! message, the "Add a Server → Join a Server" dialog and the server rail's
//! "+" button.
//!
//! Data in, actions out: nothing here talks to the network from the UI.
//! Widgets read an [`InviteCache`] and return [`InviteAction`]s; the caller
//! turns those into backend requests and feeds the answers back with
//! [`InviteCache::set_fetched`] / [`InviteCache::set_join_result`]. The
//! backend side uses [`fetch`] and [`join`].
//!
//! Joining only ever happens because the user pressed a Join button.
//!
//! Protocol notes (from Discord's web client behaviour; not documented for
//! user accounts, so parsed leniently):
//! - Preview: `GET invites/{code}?with_counts=true&with_expiration=true`.
//!   `type` 0 = server, 1 = group DM, 2 = friend invite. Counts are
//!   `approximate_member_count` / `approximate_presence_count` (newer
//!   payloads may also carry them in `profile`). Unknown / expired invites
//!   answer 404 with code 10006.
//! - Join: `POST invites/{code}` with `{}`. The official client also sends
//!   `X-Context-Properties` / `X-Super-Properties`; without them Discord is
//!   more likely to ask for a captcha. A captcha answer is a 400 whose body
//!   has `captcha_key` / `captcha_sitekey`; we never try to solve it and
//!   point the user to the official app or browser instead.
//! - Joining a server you're already in is believed to answer 200 with
//!   `new_member: false` (shown as "Joined"). Banned: code 40007. Server
//!   limit: code 30001.
//! - A successful join answer may carry `show_verification_form` (rules
//!   screening, see `screening.rs`) and the guild's `features`
//!   (`GUILD_ONBOARDING`, see `onboarding.rs`). The guild itself arrives
//!   over the gateway as GUILD_CREATE.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use serde_json::Value;

use crate::api::Api;
use crate::theme::{self, Bold, Palette};

/// Invite cards shown under one message at most.
pub const MAX_CARDS_PER_MESSAGE: usize = 3;
/// Invite previews kept in memory.
pub const CACHE_CAP: usize = 64;
/// Longer "codes" are junk (vanity URLs are at most ~25 characters).
const MAX_CODE_LEN: usize = 40;
const CDN: &str = "https://cdn.discordapp.com";

const CARD_WIDTH: f32 = 432.0;
const CARD_ICON: f32 = 50.0;
const RAIL_SIZE: f32 = 48.0;

// ---------------------------------------------------------------------------
// Finding invite links
// ---------------------------------------------------------------------------

/// Hosts + paths that carry an invite code (lowercase).
const PREFIXES: [&str; 3] = [
    "discord.gg/",
    "discord.com/invite/",
    "discordapp.com/invite/",
];
/// Subdomains allowed in front of the host.
const SUBDOMAINS: [&str; 3] = ["www.", "ptb.", "canary."];

/// Invite codes linked in a message, in order, without duplicates, at most
/// [`MAX_CARDS_PER_MESSAGE`]. Links inside `code` / ```code blocks``` are
/// ignored. Recognises `discord.gg/<code>`, `discord.com/invite/<code>` and
/// `discordapp.com/invite/<code>`, with or without `http(s)://` / `www.`.
pub fn extract_invite_codes(content: &str) -> Vec<String> {
    let text = strip_code(content);
    let mut out = Vec::new();
    for code in scan(&text) {
        if !out.contains(&code) {
            out.push(code);
            if out.len() == MAX_CARDS_PER_MESSAGE {
                break;
            }
        }
    }
    out
}

/// Every invite code in `text`, in order (with duplicates).
fn scan(text: &str) -> Vec<String> {
    // ASCII lowercasing keeps byte offsets identical.
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lower.len() {
        let next = PREFIXES
            .iter()
            .filter_map(|p| lower[i..].find(p).map(|pos| (i + pos, p.len())))
            .min_by_key(|&(pos, _)| pos);
        let Some((pos, len)) = next else { break };
        i = pos + len;
        if !starts_link(&lower[..pos]) {
            continue;
        }
        let mut code = take_code(&text[i..]);
        // discord.gg/invite/<code> works too.
        if code.eq_ignore_ascii_case("invite") && text[i + code.len()..].starts_with('/') {
            let rest = &text[i + code.len() + 1..];
            code = take_code(rest);
        }
        if valid_code(code) {
            out.push(code.to_string());
        }
    }
    out
}

/// True if a host found right after `before` starts a link (not the end of
/// another word, domain or path like `notdiscord.gg` / `x.com/discord.gg`).
fn starts_link(before: &str) -> bool {
    let mut b = before;
    if let Some(sub) = SUBDOMAINS.iter().find(|s| b.ends_with(*s)) {
        b = &b[..b.len() - sub.len()];
    }
    if let Some(rest) = b.strip_suffix("://") {
        let Some(rest) = rest
            .strip_suffix("https")
            .or_else(|| rest.strip_suffix("http"))
        else {
            return false;
        };
        b = rest;
    }
    match b.chars().next_back() {
        None => true,
        Some(c) => !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '@' | ':')),
    }
}

fn take_code(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(s.len());
    &s[..end]
}

fn valid_code(code: &str) -> bool {
    (2..=MAX_CODE_LEN).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !code.starts_with('-')
}

/// `text` with inline code and code blocks blanked out.
fn strip_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let fence = if rest[start..].starts_with("```") {
            "```"
        } else {
            "`"
        };
        let body = &rest[start + fence.len()..];
        match body.find(fence) {
            Some(end) => {
                out.push(' ');
                rest = &body[end + fence.len()..];
            }
            None => {
                // Unclosed: it's plain text.
                out.push_str(fence);
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The invite code in what the user typed into "Join a Server": a bare code
/// (`hTKzmak`, `cool-people`) or any invite link.
pub fn parse_invite_input(input: &str) -> Option<String> {
    let s = input
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    if s.contains('/') || s.contains('.') {
        return scan(s).into_iter().next();
    }
    valid_code(s).then(|| s.to_string())
}

/// The web link for a code (for "open in browser").
pub fn invite_url(code: &str) -> String {
    format!("https://discord.gg/{code}")
}

// ---------------------------------------------------------------------------
// Invite data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteKind {
    Guild,
    GroupDm,
    Friend,
}

/// What an invite card shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub code: String,
    pub kind: InviteKind,
    /// Server id (server invites).
    pub guild_id: Option<String>,
    /// Channel the invite points at (group DM id for group DM invites).
    pub channel_id: Option<String>,
    /// Server name / group DM name / friend's display name.
    pub name: String,
    pub icon_url: Option<String>,
    pub description: Option<String>,
    pub inviter: Option<String>,
    pub members: Option<u64>,
    pub online: Option<u64>,
    /// ISO time the invite expires (None: never).
    pub expires_at: Option<String>,
    /// Server features (`VERIFIED`, `PARTNERED`, `GUILD_ONBOARDING`, …).
    pub features: Vec<String>,
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn u64_at(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn user_name(u: &Value) -> Option<String> {
    str_at(u, "global_name").or_else(|| str_at(u, "username"))
}

impl Invite {
    /// Parse a `GET invites/{code}` answer. None if it has no code.
    pub fn from_json(v: &Value) -> Option<Self> {
        let code = str_at(v, "code")?;
        let guild = &v["guild"];
        let channel = &v["channel"];
        let inviter = &v["inviter"];
        let profile = &v["profile"];
        let kind = match v.get("type").and_then(Value::as_u64) {
            Some(1) => InviteKind::GroupDm,
            Some(2) => InviteKind::Friend,
            Some(_) => InviteKind::Guild,
            None if guild.is_object() => InviteKind::Guild,
            None if inviter.is_object() && !channel.is_object() => InviteKind::Friend,
            None => InviteKind::GroupDm,
        };
        let guild_id = str_at(guild, "id").or_else(|| str_at(v, "guild_id"));
        let channel_id = str_at(channel, "id");
        let inviter_name = user_name(inviter);

        let (name, icon_url) = match kind {
            InviteKind::Guild => {
                let name = str_at(guild, "name")
                    .or_else(|| str_at(profile, "name"))
                    .unwrap_or_else(|| "Unknown server".into());
                let icon = match (&guild_id, str_at(guild, "icon")) {
                    (Some(id), Some(hash)) => Some(format!("{CDN}/icons/{id}/{hash}.png?size=64")),
                    _ => None,
                };
                (name, icon)
            }
            InviteKind::GroupDm => {
                let name = str_at(channel, "name").unwrap_or_else(|| {
                    let names: Vec<String> = channel["recipients"]
                        .as_array()
                        .map(|r| r.iter().filter_map(user_name).collect())
                        .unwrap_or_default();
                    if names.is_empty() {
                        "Group DM".into()
                    } else {
                        names.join(", ")
                    }
                });
                let icon = match (&channel_id, str_at(channel, "icon")) {
                    (Some(id), Some(hash)) => {
                        Some(format!("{CDN}/channel-icons/{id}/{hash}.png?size=64"))
                    }
                    _ => None,
                };
                (name, icon)
            }
            InviteKind::Friend => {
                let icon = str_at(inviter, "id").map(|id| {
                    crate::types::avatar_url(&id, inviter.get("avatar").and_then(Value::as_str))
                });
                (
                    inviter_name.clone().unwrap_or_else(|| "Someone".into()),
                    icon,
                )
            }
        };

        let members = u64_at(v, "approximate_member_count")
            .or_else(|| u64_at(profile, "member_count"))
            .or_else(|| {
                (kind == InviteKind::GroupDm)
                    .then(|| channel["recipients"].as_array().map(|r| r.len() as u64))
                    .flatten()
            });
        let online =
            u64_at(v, "approximate_presence_count").or_else(|| u64_at(profile, "online_count"));
        let features = guild["features"]
            .as_array()
            .or_else(|| profile["features"].as_array())
            .map(|f| {
                f.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            code,
            kind,
            guild_id,
            channel_id,
            name,
            icon_url,
            description: str_at(guild, "description").or_else(|| str_at(profile, "description")),
            inviter: inviter_name,
            members,
            online,
            expires_at: str_at(v, "expires_at"),
            features,
        })
    }

    /// The id that says "already joined": server id, or group DM id.
    pub fn target_id(&self) -> Option<&str> {
        match self.kind {
            InviteKind::Guild => self.guild_id.as_deref(),
            InviteKind::GroupDm => self.channel_id.as_deref(),
            InviteKind::Friend => None,
        }
    }

    pub fn verified(&self) -> bool {
        self.features
            .iter()
            .any(|f| f == "VERIFIED" || f == "PARTNERED")
    }
}

/// Discord's JSON error code (`{"code": 10006, "message": …}`).
fn error_code(body: &Value) -> Option<u64> {
    body.get("code").and_then(Value::as_u64)
}

fn error_message(status: u16, body: &Value) -> String {
    str_at(body, "message").unwrap_or_else(|| format!("HTTP {status}"))
}

/// Result of loading an invite preview.
#[derive(Debug, Clone, PartialEq)]
pub enum InviteFetch {
    Ok(Box<Invite>),
    /// Unknown, expired or revoked.
    Invalid,
    Failed(String),
}

/// Turn the status + body of `GET invites/{code}` into a result.
pub fn classify_fetch(status: u16, body: &Value) -> InviteFetch {
    if (200..300).contains(&status) {
        return match Invite::from_json(body) {
            Some(inv) => InviteFetch::Ok(Box::new(inv)),
            None => InviteFetch::Invalid,
        };
    }
    if status == 404 || error_code(body) == Some(10006) {
        return InviteFetch::Invalid;
    }
    InviteFetch::Failed(error_message(status, body))
}

/// Result of pressing Join.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinOutcome {
    Joined {
        guild_id: Option<String>,
        channel_id: Option<String>,
        /// False: was already a member.
        new_member: bool,
        /// Membership screening (rules) needs completing before talking.
        show_verification_form: bool,
        /// The server's features from the answer (look for
        /// `GUILD_ONBOARDING` / `MEMBER_VERIFICATION_GATE_ENABLED`).
        features: Vec<String>,
    },
    /// Discord wants a captcha; we never solve it.
    Captcha,
    /// Unknown / expired invite.
    Invalid,
    Banned,
    /// Too many servers.
    ServerLimit,
    Failed(String),
}

/// Turn the status + body of `POST invites/{code}` into an outcome.
pub fn classify_join(status: u16, body: &Value) -> JoinOutcome {
    if body.get("captcha_key").is_some() || body.get("captcha_sitekey").is_some() {
        return JoinOutcome::Captcha;
    }
    if (200..300).contains(&status) {
        let guild = &body["guild"];
        return JoinOutcome::Joined {
            guild_id: str_at(guild, "id").or_else(|| str_at(body, "guild_id")),
            channel_id: str_at(&body["channel"], "id"),
            new_member: body
                .get("new_member")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            show_verification_form: body
                .get("show_verification_form")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            features: guild["features"]
                .as_array()
                .map(|f| {
                    f.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        };
    }
    match error_code(body) {
        Some(10006) => return JoinOutcome::Invalid,
        Some(40007) => return JoinOutcome::Banned,
        Some(30001) => return JoinOutcome::ServerLimit,
        _ => {}
    }
    if status == 404 {
        return JoinOutcome::Invalid;
    }
    if status == 429 {
        let wait = body
            .get("retry_after")
            .and_then(Value::as_f64)
            .map(|s| format!(" Try again in {} s.", s.ceil() as u64))
            .unwrap_or_default();
        return JoinOutcome::Failed(format!("You're joining too fast.{wait}"));
    }
    JoinOutcome::Failed(error_message(status, body))
}

/// Backend helper: load an invite preview.
pub async fn fetch(api: &Api, code: &str) -> InviteFetch {
    match api.get_invite(code).await {
        Ok((status, body)) => classify_fetch(status, &body),
        Err(e) => InviteFetch::Failed(e.to_string()),
    }
}

/// Backend helper: join through an invite. Only call after the user pressed
/// Join.
pub async fn join(api: &Api, code: &str) -> JoinOutcome {
    match api.join_invite(code).await {
        Ok((status, body)) => classify_join(status, &body),
        Err(e) => JoinOutcome::Failed(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum InviteState {
    Loading,
    Ready(Box<Invite>),
    Invalid,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinState {
    Idle,
    Joining,
    Joined,
    /// Discord asked for a captcha.
    Captcha,
    Error(String),
}

#[derive(Debug, Clone)]
struct Entry {
    code: String,
    state: InviteState,
    join: JoinState,
}

/// Invite previews by code, oldest dropped first (capped).
#[derive(Debug, Clone)]
pub struct InviteCache {
    entries: VecDeque<Entry>,
    cap: usize,
}

impl Default for InviteCache {
    fn default() -> Self {
        Self::new(CACHE_CAP)
    }
}

static IDLE: JoinState = JoinState::Idle;

impl InviteCache {
    pub fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    fn entry(&self, code: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.code == code)
    }

    fn entry_mut(&mut self, code: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.code == code)
    }

    pub fn state(&self, code: &str) -> Option<&InviteState> {
        self.entry(code).map(|e| &e.state)
    }

    pub fn join_state(&self, code: &str) -> &JoinState {
        self.entry(code).map(|e| &e.join).unwrap_or(&IDLE)
    }

    pub fn invite(&self, code: &str) -> Option<&Invite> {
        match self.state(code)? {
            InviteState::Ready(inv) => Some(inv),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Call for `InviteAction::Fetch` / `Retry`. Returns true if a request
    /// should go out (the code is new, or failed before); marks it loading.
    pub fn begin_fetch(&mut self, code: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.code == code) {
            let mut e = self.entries.remove(pos).expect("position is valid");
            let send = matches!(e.state, InviteState::Failed(_));
            if send {
                e.state = InviteState::Loading;
            }
            self.entries.push_back(e);
            return send;
        }
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            code: code.to_string(),
            state: InviteState::Loading,
            join: JoinState::Idle,
        });
        true
    }

    /// The preview answer arrived.
    pub fn set_fetched(&mut self, code: &str, result: InviteFetch) {
        if let Some(e) = self.entry_mut(code) {
            e.state = match result {
                InviteFetch::Ok(inv) => InviteState::Ready(inv),
                InviteFetch::Invalid => InviteState::Invalid,
                InviteFetch::Failed(m) => InviteState::Failed(m),
            };
        }
    }

    /// Call for `InviteAction::Join`. Returns true if the join request
    /// should go out (preview loaded and not already joining / joined).
    pub fn begin_join(&mut self, code: &str) -> bool {
        match self.entry_mut(code) {
            Some(e)
                if matches!(e.state, InviteState::Ready(_))
                    && matches!(e.join, JoinState::Idle | JoinState::Error(_)) =>
            {
                e.join = JoinState::Joining;
                true
            }
            _ => false,
        }
    }

    /// The join answer arrived.
    pub fn set_join_result(&mut self, code: &str, outcome: &JoinOutcome) {
        let Some(e) = self.entry_mut(code) else {
            return;
        };
        e.join = match outcome {
            JoinOutcome::Joined { .. } => JoinState::Joined,
            JoinOutcome::Captcha => JoinState::Captcha,
            JoinOutcome::Invalid => {
                e.state = InviteState::Invalid;
                JoinState::Idle
            }
            JoinOutcome::Banned => JoinState::Error("You're banned from this server.".into()),
            JoinOutcome::ServerLimit => {
                JoinState::Error("You're in too many servers. Leave one to join another.".into())
            }
            JoinOutcome::Failed(m) => JoinState::Error(m.clone()),
        };
    }
}

// ---------------------------------------------------------------------------
// Invite card
// ---------------------------------------------------------------------------

/// What the user asked for. The caller does the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteAction {
    /// No preview yet: call `InviteCache::begin_fetch`, then load it.
    Fetch(String),
    /// Load a failed preview again (also via `begin_fetch`).
    Retry(String),
    /// The user pressed Join: `begin_join`, then `POST invites/{code}`.
    Join(String),
    /// Already a member: switch to this server / group DM.
    Open(String),
    /// Captcha: open the invite link (through `State::open_link`).
    OpenLink(String),
}

/// What an invite card needs to draw.
pub struct CardCtx<'a> {
    pub pal: &'a Palette,
    pub cache: &'a InviteCache,
    /// True for servers / group DMs I'm already in.
    pub is_member: &'a dyn Fn(&str) -> bool,
    /// User accounts only: bots can't join through invites.
    pub can_join: bool,
    /// The invite was sent by me ("You sent an invite to join a server").
    pub sent_by_me: bool,
    /// Draw the Join / Joined button (the join dialog has its own).
    pub button: bool,
}

/// An invite card (under a message, or as the dialog's preview). Emits
/// `Fetch` when the code isn't in the cache yet.
pub fn invite_card(ui: &mut egui::Ui, code: &str, cx: &CardCtx) -> Option<InviteAction> {
    let pal = cx.pal;
    let mut action = None;
    let state = match cx.cache.state(code) {
        Some(s) => s,
        None => {
            action = Some(InviteAction::Fetch(code.to_string()));
            &InviteState::Loading
        }
    };
    let width = CARD_WIDTH.min(ui.available_width());
    egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(width - 32.0);
            match state {
                InviteState::Loading => loading_card(ui, pal),
                InviteState::Invalid => invalid_card(ui, pal, cx.sent_by_me),
                InviteState::Failed(err) => {
                    ui.label(pal.section_label("Couldn't load invite"));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(err).color(pal.danger));
                        if ui.button("Retry").clicked() {
                            action = Some(InviteAction::Retry(code.to_string()));
                        }
                    });
                }
                InviteState::Ready(inv) => {
                    if let Some(a) = ready_card(ui, inv, cx) {
                        action = Some(a);
                    }
                }
            }
        });
    action
}

fn loading_card(ui: &mut egui::Ui, pal: &Palette) {
    ui.label(pal.section_label("Resolving invite"));
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CARD_ICON), Sense::hover());
    let p = ui.painter();
    let icon = egui::Rect::from_min_size(rect.min, vec2(CARD_ICON, CARD_ICON));
    p.rect_filled(icon, 16, pal.input);
    let x = icon.right() + 16.0;
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, rect.top() + 8.0), vec2(160.0, 14.0)),
        7,
        pal.input,
    );
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, rect.top() + 30.0), vec2(110.0, 12.0)),
        6,
        pal.input,
    );
}

fn invalid_card(ui: &mut egui::Ui, pal: &Palette, sent_by_me: bool) {
    let header = if sent_by_me {
        "You sent an invite, but…"
    } else {
        "You received an invite, but…"
    };
    ui.label(pal.section_label(header));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(CARD_ICON, CARD_ICON), Sense::hover());
        ui.painter().rect_filled(rect, 16, pal.input);
        cross_icon(ui.painter(), rect.center(), 9.0, pal.danger);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Invalid Invite")
                    .size(16.0)
                    .bold()
                    .color(pal.danger),
            );
            ui.label(RichText::new("Try sending a new invite!").color(pal.muted));
        });
    });
}

fn ready_card(ui: &mut egui::Ui, inv: &Invite, cx: &CardCtx) -> Option<InviteAction> {
    let pal = cx.pal;
    let mut action = None;
    let header = match (inv.kind, cx.sent_by_me) {
        (InviteKind::Guild, false) => "You've been invited to join a server",
        (InviteKind::Guild, true) => "You sent an invite to join a server",
        (InviteKind::GroupDm, false) => "You've been invited to join a group DM",
        (InviteKind::GroupDm, true) => "You sent an invite to join a group DM",
        (InviteKind::Friend, _) => "Friend invite",
    };
    ui.label(pal.section_label(header));
    ui.add_space(8.0);

    let member = inv.target_id().is_some_and(|id| (cx.is_member)(id))
        || *cx.cache.join_state(&inv.code) == JoinState::Joined;
    let join = cx.cache.join_state(&inv.code);
    let show_button = cx.button && inv.kind != InviteKind::Friend && (member || cx.can_join);
    let button_w = if show_button { 96.0 } else { 0.0 };

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(CARD_ICON, CARD_ICON), Sense::hover());
        server_icon(ui, rect, inv.icon_url.as_deref(), &inv.name, pal);
        ui.add_space(10.0);
        let text_w = (ui.available_width() - button_w - 8.0).max(60.0);
        ui.allocate_ui(vec2(text_w, CARD_ICON), |ui| {
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if inv.verified() {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        check_badge(ui.painter(), r, pal);
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(&inv.name).size(16.0).bold().color(pal.strong),
                        )
                        .truncate(),
                    );
                });
                counts_row(ui, inv, pal);
            });
        });
        if show_button {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if member {
                    let b = egui::Button::new(RichText::new("Joined").color(Color32::WHITE).bold())
                        .fill(pal.online.gamma_multiply(0.6))
                        .min_size(vec2(80.0, 36.0));
                    if ui.add(b).on_hover_text("Go to it").clicked()
                        && let Some(id) = inv.target_id()
                    {
                        action = Some(InviteAction::Open(id.to_string()));
                    }
                } else {
                    let joining = *join == JoinState::Joining;
                    // After a captcha the user joins from the official app.
                    let blocked = joining || *join == JoinState::Captcha;
                    let label = if joining { "Joining…" } else { "Join" };
                    let b = egui::Button::new(RichText::new(label).color(Color32::WHITE).bold())
                        .fill(pal.online)
                        .min_size(vec2(80.0, 36.0));
                    if ui.add_enabled(!blocked, b).clicked() {
                        action = Some(InviteAction::Join(inv.code.clone()));
                    }
                }
            });
        }
    });

    match join {
        JoinState::Captcha => {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Discord wants a captcha — join from the official app/browser.")
                    .color(pal.idle),
            );
            if ui
                .add(egui::Button::new("Open invite in browser").fill(pal.input))
                .clicked()
            {
                action = Some(InviteAction::OpenLink(invite_url(&inv.code)));
            }
        }
        JoinState::Error(e) => {
            ui.add_space(8.0);
            ui.label(RichText::new(e).color(pal.danger));
        }
        _ => {}
    }
    if inv.kind == InviteKind::Friend {
        ui.add_space(6.0);
        ui.label(
            RichText::new("Accept friend invites from the official app.")
                .size(13.0)
                .color(pal.muted),
        );
    }
    action
}

fn counts_row(ui: &mut egui::Ui, inv: &Invite, pal: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let Some(online) = inv.online {
            dot(ui, pal.online);
            ui.label(
                RichText::new(format!("{} Online", group_digits(online)))
                    .size(14.0)
                    .color(pal.muted),
            );
            ui.add_space(8.0);
        }
        if let Some(members) = inv.members {
            dot(ui, pal.offline);
            let noun = if members == 1 { "Member" } else { "Members" };
            ui.label(
                RichText::new(format!("{} {noun}", group_digits(members)))
                    .size(14.0)
                    .color(pal.muted),
            );
        }
        if inv.online.is_none()
            && inv.members.is_none()
            && let Some(by) = &inv.inviter
        {
            ui.label(
                RichText::new(format!("from {by}"))
                    .size(14.0)
                    .color(pal.muted),
            );
        }
    });
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}

/// 12345 → "12,345".
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Up to three initials ("RustCord Test Server" → "RTS").
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(3)
        .collect()
}

/// A rounded server icon, or initials on a plain tile.
pub fn server_icon(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    url: Option<&str>,
    name: &str,
    pal: &Palette,
) {
    let radius = (rect.width() / 3.0).round() as u8;
    ui.painter().rect_filled(rect, radius, pal.rail_icon);
    match url {
        Some(url) => {
            egui::Image::from_uri(url)
                .corner_radius(radius)
                .show_loading_spinner(false)
                .paint_at(ui, rect);
        }
        None => {
            let ini = initials(name);
            let size = if ini.chars().count() > 2 { 14.0 } else { 16.0 };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                ini,
                theme::bold_font(size),
                pal.text,
            );
        }
    }
}

fn cross_icon(p: &egui::Painter, c: egui::Pos2, r: f32, color: Color32) {
    let s = Stroke::new(2.5, color);
    p.line_segment([c + vec2(-r, -r), c + vec2(r, r)], s);
    p.line_segment([c + vec2(-r, r), c + vec2(r, -r)], s);
}

/// Small green seal with a check mark (verified / partnered servers).
fn check_badge(p: &egui::Painter, r: egui::Rect, pal: &Palette) {
    let c = r.center();
    p.circle_filled(c, 7.0, pal.online);
    let s = Stroke::new(1.8, Color32::WHITE);
    p.line_segment([c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.5)], s);
    p.line_segment([c + vec2(-1.0, 2.5), c + vec2(3.5, -2.5)], s);
}

/// A "+" drawn with two strokes.
fn plus_icon(p: &egui::Painter, c: egui::Pos2, r: f32, color: Color32) {
    let s = Stroke::new(2.0, color);
    p.line_segment([c + vec2(-r, 0.0), c + vec2(r, 0.0)], s);
    p.line_segment([c + vec2(0.0, -r), c + vec2(0.0, r)], s);
}

// ---------------------------------------------------------------------------
// Server rail "+" button
// ---------------------------------------------------------------------------

/// The "Add a Server" button at the bottom of the server rail: a green "+"
/// that turns into a green tile on hover, like Discord. `wide`: the rail
/// shows names (`RAIL_NAMES_WIDTH`), so draw the label too.
pub fn rail_add_button(ui: &mut egui::Ui, pal: &Palette, wide: bool) -> egui::Response {
    let width = ui.available_width();
    let (row, resp) = ui.allocate_exact_size(vec2(width, RAIL_SIZE), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let t = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("shape"), hovered, 0.12);
    let radius = egui::lerp(RAIL_SIZE / 2.0..=16.0, t);
    let x = if wide {
        row.left() + 12.0
    } else {
        row.center().x - RAIL_SIZE / 2.0
    };
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), vec2(RAIL_SIZE, RAIL_SIZE));
    let p = ui.painter();
    p.rect_filled(
        rect,
        radius,
        if hovered { pal.online } else { pal.rail_icon },
    );
    plus_icon(
        p,
        rect.center(),
        8.0,
        if hovered { Color32::WHITE } else { pal.online },
    );
    if wide {
        p.text(
            egui::pos2(rect.right() + 12.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            "Add a Server",
            egui::FontId::proportional(15.0),
            if hovered { pal.unread } else { pal.muted },
        );
        resp
    } else {
        resp.on_hover_text("Add a Server")
    }
}

// ---------------------------------------------------------------------------
// "Add a Server" dialog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Choose,
    Join,
}

/// What the dialog asks for. `Fetch` / `Join` / `Open` / `OpenLink` work
/// like [`InviteAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddServerAction {
    Invite(InviteAction),
    Close,
}

/// "Add a Server" → "Join a Server": paste a link or code, preview, join.
#[derive(Debug, Clone)]
pub struct AddServerDialog {
    page: Page,
    input: String,
    /// Code being previewed.
    code: Option<String>,
    error: Option<String>,
}

impl Default for AddServerDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl AddServerDialog {
    /// Starts on the "Add a Server" page (from the rail's "+").
    pub fn new() -> Self {
        Self {
            page: Page::Choose,
            input: String::new(),
            code: None,
            error: None,
        }
    }

    /// Straight to "Join a Server", optionally prefilled.
    pub fn join(prefill: &str) -> Self {
        let mut d = Self::new();
        d.page = Page::Join;
        d.input = prefill.to_string();
        d.code = parse_invite_input(prefill);
        d
    }

    /// The code being previewed.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// Draw the modal. `cx.button` is ignored (the dialog has its own).
    pub fn show(&mut self, ctx: &egui::Context, cx: &CardCtx) -> Option<AddServerAction> {
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_add_server")).show(ctx, |ui| {
            ui.set_width(440.0);
            match self.page {
                Page::Choose => self.choose_page(ui, cx.pal, &mut action),
                Page::Join => self.join_page(ui, cx, &mut action),
            }
        });
        if action.is_none() && modal.should_close() {
            action = Some(AddServerAction::Close);
        }
        action
    }

    fn choose_page(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        action: &mut Option<AddServerAction>,
    ) {
        ui.vertical_centered(|ui| {
            ui.heading("Add a Server");
            ui.label(
                RichText::new(
                    "Your server is where you and your friends hang out. \
                     Join one with an invite.",
                )
                .color(pal.muted),
            );
        });
        ui.add_space(12.0);
        if choice_row(ui, pal, "Join a Server", "Have an invite already?", true).clicked() {
            self.page = Page::Join;
        }
        ui.add_space(6.0);
        choice_row(
            ui,
            pal,
            "Create My Own",
            "Not available in RustCord yet",
            false,
        );
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                *action = Some(AddServerAction::Close);
            }
        });
    }

    fn join_page(&mut self, ui: &mut egui::Ui, cx: &CardCtx, action: &mut Option<AddServerAction>) {
        let pal = cx.pal;
        ui.vertical_centered(|ui| {
            ui.heading("Join a Server");
            ui.label(
                RichText::new("Enter an invite below to join an existing server").color(pal.muted),
            );
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(pal.section_label("Invite link"));
            ui.label(RichText::new("*").color(pal.danger));
        });
        let before = self.input.len();
        let edit = ui.add(
            egui::TextEdit::singleline(&mut self.input)
                .hint_text("https://discord.gg/hTKzmak")
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 8.0)),
        );
        let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if edit.changed() {
            self.error = None;
            let parsed = parse_invite_input(&self.input);
            // A pasted link previews at once; typing waits for Enter /
            // Preview (no request per keystroke).
            let pasted = self.input.len() >= before + 4 && self.input.contains('/');
            if parsed != self.code {
                self.code = if pasted { parsed } else { None };
            }
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(pal.danger).size(13.0));
        }

        ui.add_space(10.0);
        let code = self.code.clone();
        match &code {
            Some(code) => {
                let card = CardCtx {
                    pal,
                    cache: cx.cache,
                    is_member: cx.is_member,
                    can_join: cx.can_join,
                    sent_by_me: false,
                    button: false,
                };
                if let Some(a) = invite_card(ui, code, &card) {
                    *action = Some(AddServerAction::Invite(a));
                }
            }
            None => {
                ui.label(pal.section_label("Invites should look like"));
                for ex in [
                    "hTKzmak",
                    "https://discord.gg/hTKzmak",
                    "https://discord.gg/cool-people",
                ] {
                    ui.label(RichText::new(ex).size(14.0).color(pal.text));
                }
            }
        }
        if !cx.can_join {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Bot accounts can't join servers through invites.")
                    .size(13.0)
                    .color(pal.idle),
            );
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                self.page = Page::Choose;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, enabled, todo) = self.primary(cx);
                let button = pal.primary_button(label).min_size(vec2(120.0, 34.0));
                let clicked = ui.add_enabled(enabled, button).clicked();
                if clicked || (entered && enabled) {
                    match todo {
                        Primary::Preview => match parse_invite_input(&self.input) {
                            Some(code) => {
                                self.code = Some(code.clone());
                                if cx.cache.state(&code).is_none() {
                                    *action =
                                        Some(AddServerAction::Invite(InviteAction::Fetch(code)));
                                }
                            }
                            None => {
                                self.error =
                                    Some("Please enter a valid invite link or invite code.".into());
                            }
                        },
                        Primary::Join(code) => {
                            *action = Some(AddServerAction::Invite(InviteAction::Join(code)));
                        }
                        Primary::Open(id) => {
                            *action = Some(AddServerAction::Invite(InviteAction::Open(id)));
                        }
                        Primary::Retry(code) => {
                            *action = Some(AddServerAction::Invite(InviteAction::Retry(code)));
                        }
                        Primary::None => {}
                    }
                }
            });
        });
    }

    /// The bottom-right button: Preview → Join Server → Go to Server.
    fn primary(&self, cx: &CardCtx) -> (&'static str, bool, Primary) {
        let typed = parse_invite_input(&self.input);
        let Some(code) = self.code.as_ref().filter(|c| typed.as_ref() == Some(*c)) else {
            return ("Preview", !self.input.trim().is_empty(), Primary::Preview);
        };
        match cx.cache.state(code) {
            None | Some(InviteState::Loading) => ("Loading…", false, Primary::None),
            Some(InviteState::Invalid) => ("Join Server", false, Primary::None),
            Some(InviteState::Failed(_)) => ("Try Again", true, Primary::Retry(code.clone())),
            Some(InviteState::Ready(inv)) => {
                let member = inv.target_id().is_some_and(|id| (cx.is_member)(id))
                    || *cx.cache.join_state(code) == JoinState::Joined;
                if member {
                    match inv.target_id() {
                        Some(id) => ("Go to Server", true, Primary::Open(id.to_string())),
                        None => ("Joined", false, Primary::None),
                    }
                } else if inv.kind == InviteKind::Friend || !cx.can_join {
                    ("Join Server", false, Primary::None)
                } else {
                    match cx.cache.join_state(code) {
                        JoinState::Joining => ("Joining…", false, Primary::None),
                        JoinState::Captcha => ("Join Server", false, Primary::None),
                        _ => ("Join Server", true, Primary::Join(code.clone())),
                    }
                }
            }
        }
    }
}

enum Primary {
    Preview,
    Join(String),
    Open(String),
    Retry(String),
    None,
}

/// A full-width option row in "Add a Server".
fn choice_row(
    ui: &mut egui::Ui,
    pal: &Palette,
    title: &str,
    sub: &str,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), sense);
    let hovered = enabled && resp.hovered();
    let p = ui.painter();
    p.rect_filled(rect, 8, if hovered { pal.hover } else { pal.input });
    let text_col = if enabled { pal.strong } else { pal.muted };
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        vec2(32.0, 32.0),
    );
    p.circle_filled(
        icon.center(),
        16.0,
        if enabled { pal.online } else { pal.selected },
    );
    plus_icon(p, icon.center(), 7.0, Color32::WHITE);
    p.text(
        egui::pos2(icon.right() + 12.0, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        title,
        theme::bold_font(15.0),
        text_col,
    );
    p.text(
        egui::pos2(icon.right() + 12.0, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        sub,
        egui::FontId::proportional(13.0),
        pal.muted,
    );
    if enabled {
        // Chevron ">".
        let c = egui::pos2(rect.right() - 20.0, rect.center().y);
        let s = Stroke::new(2.0, pal.muted);
        p.line_segment([c + vec2(-3.0, -6.0), c + vec2(3.0, 0.0)], s);
        p.line_segment([c + vec2(3.0, 0.0), c + vec2(-3.0, 6.0)], s);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_all_forms() {
        let text = "join https://discord.gg/abc123 or discord.com/invite/Cool-People \
                    and http://www.discordapp.com/invite/XyZ?event=1 \
                    plus https://canary.discord.com/invite/canaryCode/";
        assert_eq!(extract_invite_codes(text), ["abc123", "Cool-People", "XyZ"]);
        // Capped at three; the fourth would be canaryCode.
        assert_eq!(scan(text).len(), 4);
        assert_eq!(scan(text)[3], "canaryCode");
    }

    #[test]
    fn extraction_dedups_and_case() {
        assert_eq!(
            extract_invite_codes("HTTPS://DISCORD.GG/abc x discord.gg/abc discord.gg/ABC"),
            ["abc", "ABC"]
        );
        assert_eq!(
            extract_invite_codes("<https://discord.gg/wrapped>"),
            ["wrapped"]
        );
        assert_eq!(extract_invite_codes("(see discord.gg/paren)."), ["paren"]);
        assert_eq!(
            extract_invite_codes("discord.gg/invite/viaInvite"),
            ["viaInvite"]
        );
    }

    #[test]
    fn extraction_ignores_lookalikes() {
        for text in [
            "notdiscord.gg/abc",
            "evil-discord.gg/abc",
            "https://example.com/discord.gg/abc",
            "user@discord.gg/abc",
            "ftp://discord.gg/abc",
            "discord.gg/",
            "discord.gg/a",
            "discord.gg/-abc",
            "discord.com/channels/1/2",
            "discord.gg",
        ] {
            assert!(extract_invite_codes(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn extraction_skips_code() {
        assert!(extract_invite_codes("`discord.gg/inline`").is_empty());
        assert!(extract_invite_codes("```\ndiscord.gg/block\n```").is_empty());
        assert_eq!(
            extract_invite_codes("`x` discord.gg/real ```discord.gg/nope```"),
            ["real"]
        );
        // Unclosed backtick is plain text.
        assert_eq!(extract_invite_codes("` discord.gg/open"), ["open"]);
        // Non-ASCII around the link doesn't break offsets.
        assert_eq!(extract_invite_codes("ünï discord.gg/ok ✓"), ["ok"]);
    }

    #[test]
    fn parses_dialog_input() {
        assert_eq!(parse_invite_input(" hTKzmak ").unwrap(), "hTKzmak");
        assert_eq!(parse_invite_input("cool-people").unwrap(), "cool-people");
        assert_eq!(
            parse_invite_input("https://discord.gg/hTKzmak").unwrap(),
            "hTKzmak"
        );
        assert_eq!(
            parse_invite_input("<discord.com/invite/abc>").unwrap(),
            "abc"
        );
        assert_eq!(parse_invite_input("discord.gg/abc").unwrap(), "abc");
        for junk in [
            "",
            "a",
            "hello world",
            "https://example.com/x",
            "ab_c",
            "-ab",
        ] {
            assert!(parse_invite_input(junk).is_none(), "{junk}");
        }
    }

    fn guild_invite() -> Value {
        json!({
            "type": 0,
            "code": "abc",
            "expires_at": null,
            "guild": {
                "id": "42", "name": "Pegasi", "icon": "hash", "description": "Mousepads",
                "features": ["VERIFIED", "GUILD_ONBOARDING"]
            },
            "channel": { "id": "7", "name": "welcome", "type": 0 },
            "inviter": { "id": "1", "username": "leo", "global_name": "Leo" },
            "approximate_member_count": 12345,
            "approximate_presence_count": 678
        })
    }

    #[test]
    fn parses_guild_invite() {
        let inv = Invite::from_json(&guild_invite()).unwrap();
        assert_eq!(inv.kind, InviteKind::Guild);
        assert_eq!(inv.name, "Pegasi");
        assert_eq!(inv.guild_id.as_deref(), Some("42"));
        assert_eq!(inv.target_id(), Some("42"));
        assert_eq!(
            inv.icon_url.as_deref(),
            Some("https://cdn.discordapp.com/icons/42/hash.png?size=64")
        );
        assert_eq!(inv.members, Some(12345));
        assert_eq!(inv.online, Some(678));
        assert_eq!(inv.inviter.as_deref(), Some("Leo"));
        assert!(inv.verified());
        assert_eq!(inv.expires_at, None);
    }

    #[test]
    fn parses_group_dm_and_friend() {
        let gdm = Invite::from_json(&json!({
            "type": 1, "code": "g",
            "channel": { "id": "9", "type": 3, "name": null,
                         "recipients": [{ "username": "a" }, { "username": "b" }] }
        }))
        .unwrap();
        assert_eq!(gdm.kind, InviteKind::GroupDm);
        assert_eq!(gdm.name, "a, b");
        assert_eq!(gdm.target_id(), Some("9"));
        assert_eq!(gdm.members, Some(2));

        let friend = Invite::from_json(&json!({
            "type": 2, "code": "f", "inviter": { "id": "5", "username": "sam" }
        }))
        .unwrap();
        assert_eq!(friend.kind, InviteKind::Friend);
        assert_eq!(friend.name, "sam");
        assert_eq!(friend.target_id(), None);
        assert!(Invite::from_json(&json!({ "guild": {} })).is_none());
    }

    #[test]
    fn classifies_fetch() {
        assert!(matches!(
            classify_fetch(200, &guild_invite()),
            InviteFetch::Ok(_)
        ));
        assert_eq!(
            classify_fetch(404, &json!({ "message": "Unknown Invite", "code": 10006 })),
            InviteFetch::Invalid
        );
        assert_eq!(classify_fetch(404, &Value::Null), InviteFetch::Invalid);
        assert_eq!(
            classify_fetch(500, &json!({ "message": "oops" })),
            InviteFetch::Failed("oops".into())
        );
        assert_eq!(
            classify_fetch(502, &Value::Null),
            InviteFetch::Failed("HTTP 502".into())
        );
    }

    #[test]
    fn classifies_join() {
        let captcha = json!({
            "captcha_key": ["captcha-required"],
            "captcha_sitekey": "x",
            "captcha_service": "hcaptcha"
        });
        assert_eq!(classify_join(400, &captcha), JoinOutcome::Captcha);
        assert_eq!(
            classify_join(404, &json!({ "code": 10006 })),
            JoinOutcome::Invalid
        );
        assert_eq!(
            classify_join(403, &json!({ "code": 40007 })),
            JoinOutcome::Banned
        );
        assert_eq!(
            classify_join(400, &json!({ "code": 30001 })),
            JoinOutcome::ServerLimit
        );
        assert_eq!(
            classify_join(429, &json!({ "retry_after": 4.2 })),
            JoinOutcome::Failed("You're joining too fast. Try again in 5 s.".into())
        );
        assert_eq!(
            classify_join(403, &json!({ "message": "Missing Access", "code": 50001 })),
            JoinOutcome::Failed("Missing Access".into())
        );

        let mut ok = guild_invite();
        ok["new_member"] = json!(false);
        ok["show_verification_form"] = json!(true);
        match classify_join(200, &ok) {
            JoinOutcome::Joined {
                guild_id,
                channel_id,
                new_member,
                show_verification_form,
                features,
            } => {
                assert_eq!(guild_id.as_deref(), Some("42"));
                assert_eq!(channel_id.as_deref(), Some("7"));
                assert!(!new_member);
                assert!(show_verification_form);
                assert!(features.iter().any(|f| f == "GUILD_ONBOARDING"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify_join(200, &json!({})),
            JoinOutcome::Joined {
                new_member: true,
                ..
            }
        ));
    }

    #[test]
    fn cache_flow() {
        let mut c = InviteCache::new(2);
        assert!(c.begin_fetch("a"));
        assert!(!c.begin_fetch("a"), "already loading");
        assert!(!c.begin_join("a"), "not loaded yet");
        c.set_fetched("a", classify_fetch(200, &guild_invite()));
        assert!(c.invite("a").is_some());
        assert!(c.begin_join("a"));
        assert!(!c.begin_join("a"), "already joining");
        c.set_join_result("a", &JoinOutcome::Captcha);
        assert_eq!(*c.join_state("a"), JoinState::Captcha);
        assert!(!c.begin_join("a"), "captcha: no retry from here");

        assert!(c.begin_fetch("b"));
        c.set_fetched("b", InviteFetch::Failed("x".into()));
        assert!(c.begin_fetch("b"), "failed previews can be retried");
        assert_eq!(c.state("b"), Some(&InviteState::Loading));

        // Cap 2: "a" was touched before "b", so "a" goes.
        assert!(c.begin_fetch("c"));
        assert_eq!(c.len(), 2);
        assert!(c.state("a").is_none());
        assert_eq!(*c.join_state("a"), JoinState::Idle);

        c.set_fetched("c", classify_fetch(200, &guild_invite()));
        assert!(c.begin_join("c"));
        c.set_join_result("c", &JoinOutcome::Banned);
        assert!(matches!(c.join_state("c"), JoinState::Error(_)));
        assert!(c.begin_join("c"), "errors can be retried");
        c.set_join_result("c", &JoinOutcome::Invalid);
        assert_eq!(c.state("c"), Some(&InviteState::Invalid));
    }

    #[test]
    fn digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(initials("RustCord Test Server Four"), "RTS");
    }
}
