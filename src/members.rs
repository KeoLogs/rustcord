//! Server roles, member info and the member list.
//!
//! Protocol notes (from Discord Messenger, `DiscordInstance.cpp`):
//! - Roles come with each guild (READY `guilds[].roles` for user accounts,
//!   GUILD_CREATE for bots): `{ id, name, color, position, hoist }`. A
//!   member's name colour is the colour of their highest-positioned role
//!   that has one (colour 0 = "no colour").
//! - **User accounts** get the sidebar list by "subscribing" to a channel
//!   with op 14 `{ guild_id, typing, activities, threads, channels: { id:
//!   [[0, 99], …] } }` (DI:1596). Discord answers with
//!   `GUILD_MEMBER_LIST_UPDATE { guild_id, id, member_count, online_count,
//!   groups: [{ id, count }], ops }` (DI:2886). Each op works on row
//!   indexes of the whole list (group headers count as rows):
//!   `SYNC { range, items }`, `INSERT { index, item }`, `UPDATE { index,
//!   item }`, `DELETE { index }`, `INVALIDATE { range }` (DI:3137-3212).
//!   An item is `{ group: { id, count } }` (id = role id, "online" or
//!   "offline") or `{ member: { user, roles, nick, presence, joined_at } }`.
//!   `id` names the list (it depends on who can see the channel).
//! - **Bots** can't use op 14. Op 8 REQUEST_GUILD_MEMBERS with `user_ids`
//!   works for anyone (DI:199) and is used here to learn message authors'
//!   roles; with `query: "", limit` it lists the whole server but needs the
//!   privileged GUILD_MEMBERS intent. Answers arrive as GUILD_MEMBERS_CHUNK
//!   `{ guild_id, members, presences?, not_found?, nonce }` (DI:3062).

use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::{Value, json};

use crate::config::Config;
use crate::presence::{Presence, Presences, Status};
use crate::theme::Palette;
use crate::types::User;

/// Rows per op 14 range (Discord's client uses 100 too).
pub const RANGE_SIZE: u32 = 100;
/// Most members remembered across all servers (nick + roles only).
const MAX_CACHED_MEMBERS: usize = 4000;
/// Members asked for when a bot loads a server's list (needs the intent).
pub const BOT_LIST_LIMIT: u32 = 1000;
/// `nonce` of op 8 requests, so the chunk can be told apart.
pub const NONCE_LIST: &str = "rc-list";
pub const NONCE_IDS: &str = "rc-ids";

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
    pub id: String,
    pub name: String,
    /// 0xRRGGBB; 0 means no colour.
    pub color: u32,
    pub position: i64,
    /// Shown as its own group in the member list.
    pub hoist: bool,
    /// Permission bits (see `perms.rs`).
    pub permissions: u64,
}

impl Role {
    pub fn from_json(v: &Value) -> Self {
        // Newer payloads also have `colors.primary_color` (gradient roles);
        // `color` is still sent and matches the primary colour.
        let color = v
            .get("color")
            .and_then(Value::as_u64)
            .filter(|c| *c != 0)
            .or_else(|| v.pointer("/colors/primary_color").and_then(Value::as_u64))
            .unwrap_or(0) as u32;
        Self {
            id: str_of(v, "id"),
            name: str_of(v, "name"),
            color,
            position: v.get("position").and_then(Value::as_i64).unwrap_or(0),
            hoist: v.get("hoist").and_then(Value::as_bool).unwrap_or(false),
            permissions: crate::perms::parse_bits(&v["permissions"]),
        }
    }

    /// A guild's `roles` array, highest first (like Discord's role list).
    pub fn parse_list(v: &Value) -> Vec<Role> {
        let mut roles: Vec<Role> = v
            .as_array()
            .map(|a| a.iter().map(Role::from_json).collect())
            .unwrap_or_default();
        sort_roles(&mut roles);
        roles
    }

    pub fn color32(&self) -> Option<Color32> {
        (self.color != 0).then(|| rgb(self.color))
    }
}

/// Highest position first; ties broken by id like Discord.
pub fn sort_roles(roles: &mut [Role]) {
    roles.sort_by(|a, b| {
        b.position.cmp(&a.position).then_with(|| {
            let id = |r: &Role| r.id.parse::<u64>().unwrap_or(0);
            id(a).cmp(&id(b))
        })
    });
}

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A member's roles, highest first. `guild_roles` must be sorted (see
/// `Role::parse_list`). Unknown role ids are skipped.
pub fn member_roles<'a>(guild_roles: &'a [Role], member: &[String]) -> Vec<&'a Role> {
    guild_roles
        .iter()
        .filter(|r| member.contains(&r.id))
        .collect()
}

/// Name colour: the highest role that has a colour.
pub fn role_color(guild_roles: &[Role], member: &[String]) -> Option<Color32> {
    guild_roles
        .iter()
        .filter(|r| r.color != 0 && member.contains(&r.id))
        .map(|r| rgb(r.color))
        .next()
}

/// The group a member is listed under: their highest hoisted role.
pub fn hoisted_role<'a>(guild_roles: &'a [Role], member: &[String]) -> Option<&'a Role> {
    guild_roles
        .iter()
        .find(|r| r.hoist && member.contains(&r.id))
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Member {
    pub user: User,
    /// Server nickname.
    pub nick: Option<String>,
    pub roles: Vec<String>,
    /// ISO-8601, when they joined the server ("" if unknown).
    pub joined_at: String,
    pub presence: Option<Presence>,
}

impl Member {
    /// A guild member object (`user` may be missing when `user_id` is given).
    pub fn from_json(v: &Value) -> Self {
        let mut user = User::from_json(&v["user"]);
        if user.id.is_empty() {
            user.id = str_of(v, "user_id");
        }
        Self {
            user,
            nick: v
                .get("nick")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_string),
            roles: string_list(&v["roles"]),
            joined_at: str_of(v, "joined_at"),
            presence: v
                .get("presence")
                .filter(|p| p.is_object())
                .map(Presence::from_json),
        }
    }

    pub fn display_name(&self) -> &str {
        self.nick.as_deref().unwrap_or(&self.user.name)
    }
}

/// What we remember about someone in a server (for message name colours
/// and the profile popout).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CachedMember {
    pub nick: Option<String>,
    pub roles: Vec<String>,
    pub joined_at: String,
}

/// guild id -> user id -> member info. Capped; when full, servers other than
/// the one being viewed are forgotten first.
#[derive(Default)]
pub struct MemberCache {
    guilds: HashMap<String, HashMap<String, CachedMember>>,
    len: usize,
}

impl MemberCache {
    pub fn get(&self, guild_id: &str, user_id: &str) -> Option<&CachedMember> {
        self.guilds.get(guild_id)?.get(user_id)
    }

    pub fn insert(&mut self, guild_id: &str, m: &Member) {
        let entry = CachedMember {
            nick: m.nick.clone(),
            roles: m.roles.clone(),
            joined_at: m.joined_at.clone(),
        };
        self.insert_cached(guild_id, &m.user.id, entry);
    }

    pub fn insert_cached(&mut self, guild_id: &str, user_id: &str, mut entry: CachedMember) {
        if user_id.is_empty() || guild_id.is_empty() {
            return;
        }
        let existing = self.guilds.get(guild_id).and_then(|g| g.get(user_id));
        // Some payloads (MESSAGE_CREATE) leave out joined_at; keep ours.
        if entry.joined_at.is_empty()
            && let Some(old) = existing
        {
            entry.joined_at = old.joined_at.clone();
        }
        if self.len >= MAX_CACHED_MEMBERS && existing.is_none() {
            self.trim(guild_id);
        }
        let g = self.guilds.entry(guild_id.to_string()).or_default();
        if g.insert(user_id.to_string(), entry).is_none() {
            self.len += 1;
        }
    }

    /// Free other servers; if that's not enough, start this one over.
    fn trim(&mut self, keep: &str) {
        self.guilds.retain(|id, _| id == keep);
        self.len = self.guilds.values().map(HashMap::len).sum();
        if self.len >= MAX_CACHED_MEMBERS {
            self.guilds.clear();
            self.len = 0;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Everyone cached for a server (autocomplete).
    pub fn guild(&self, guild_id: &str) -> impl Iterator<Item = (&String, &CachedMember)> {
        self.guilds.get(guild_id).into_iter().flatten()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }
}

// ---------------------------------------------------------------------------
// Member list (GUILD_MEMBER_LIST_UPDATE)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Row {
    /// Group header: role id, "online" or "offline", and how many are in it.
    Group {
        id: String,
        count: u32,
    },
    Member(Box<Member>),
}

impl Row {
    fn from_json(v: &Value) -> Option<Self> {
        if let Some(g) = v.get("group") {
            return Some(Row::Group {
                id: str_of(g, "id"),
                count: g.get("count").and_then(Value::as_u64).unwrap_or(0) as u32,
            });
        }
        v.get("member")
            .filter(|m| m.is_object())
            .map(|m| Row::Member(Box::new(Member::from_json(m))))
    }
}

#[derive(Clone, Debug)]
pub enum ListOp {
    Sync {
        start: usize,
        end: usize,
        items: Vec<Row>,
    },
    Insert {
        index: usize,
        item: Row,
    },
    Update {
        index: usize,
        item: Row,
    },
    Delete {
        index: usize,
    },
    Invalidate {
        start: usize,
        end: usize,
    },
}

impl ListOp {
    fn from_json(v: &Value) -> Option<Self> {
        let index = || v.get("index").and_then(Value::as_u64).map(|i| i as usize);
        let range = || {
            let r = v.get("range")?.as_array()?;
            let a = r.first()?.as_u64()? as usize;
            let b = r.get(1)?.as_u64()? as usize;
            Some((a, b.max(a)))
        };
        let item = || v.get("item").and_then(Row::from_json);
        Some(match v.get("op")?.as_str()? {
            "SYNC" => {
                let (start, end) = range()?;
                let items = v
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Row::from_json).collect())
                    .unwrap_or_default();
                ListOp::Sync { start, end, items }
            }
            "INSERT" => ListOp::Insert {
                index: index()?,
                item: item()?,
            },
            "UPDATE" => ListOp::Update {
                index: index()?,
                item: item()?,
            },
            "DELETE" => ListOp::Delete { index: index()? },
            "INVALIDATE" => {
                let (start, end) = range()?;
                ListOp::Invalidate { start, end }
            }
            _ => return None,
        })
    }
}

/// One GUILD_MEMBER_LIST_UPDATE.
#[derive(Clone, Debug)]
pub struct ListUpdate {
    pub guild_id: String,
    pub list_id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub groups: Vec<(String, u32)>,
    pub ops: Vec<ListOp>,
}

impl ListUpdate {
    pub fn from_json(d: &Value) -> Option<Self> {
        let count = |k: &str| d.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        Some(Self {
            guild_id: d.get("guild_id")?.as_str()?.to_string(),
            list_id: str_of(d, "id"),
            member_count: count("member_count"),
            online_count: count("online_count"),
            groups: d
                .get("groups")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|g| {
                            let n = g.get("count").and_then(Value::as_u64).unwrap_or(0);
                            (str_of(g, "id"), n as u32)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ops: d
                .get("ops")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(ListOp::from_json).collect())
                .unwrap_or_default(),
        })
    }

    /// Every member in the update, for the member/presence caches.
    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.ops.iter().flat_map(|op| {
            let items: &[Row] = match op {
                ListOp::Sync { items, .. } => items,
                ListOp::Insert { item, .. } | ListOp::Update { item, .. } => {
                    std::slice::from_ref(item)
                }
                _ => &[],
            };
            items.iter().filter_map(|r| match r {
                Row::Member(m) => Some(m.as_ref()),
                Row::Group { .. } => None,
            })
        })
    }
}

/// The member list of the server being viewed. Only the subscribed ranges
/// are filled in; other rows are `None`.
#[derive(Clone, Debug, Default)]
pub struct MemberList {
    pub guild_id: String,
    pub list_id: String,
    pub rows: Vec<Option<Row>>,
    pub groups: Vec<(String, u32)>,
    pub member_count: u32,
    pub online_count: u32,
}

impl MemberList {
    pub fn new(guild_id: &str, list_id: &str) -> Self {
        Self {
            guild_id: guild_id.to_string(),
            list_id: list_id.to_string(),
            ..Self::default()
        }
    }

    pub fn apply(&mut self, upd: ListUpdate) {
        self.member_count = upd.member_count;
        self.online_count = upd.online_count;
        if !upd.groups.is_empty() {
            self.groups = upd.groups;
        }
        for op in upd.ops {
            match op {
                ListOp::Sync { start, end, items } => {
                    let n = items.len();
                    if self.rows.len() < start + n {
                        self.rows.resize(start + n, None);
                    }
                    for (i, item) in items.into_iter().enumerate() {
                        self.rows[start + i] = Some(item);
                    }
                    // Fewer items than asked for: that's the end of the list.
                    if n < end - start + 1 {
                        self.rows.truncate(start + n);
                    }
                }
                ListOp::Insert { index, item } => {
                    if index <= self.rows.len() {
                        self.rows.insert(index, Some(item));
                    } else {
                        self.rows.resize(index, None);
                        self.rows.push(Some(item));
                    }
                }
                ListOp::Update { index, item } => {
                    if let Some(row) = self.rows.get_mut(index) {
                        *row = Some(item);
                    }
                }
                ListOp::Delete { index } => {
                    if index < self.rows.len() {
                        self.rows.remove(index);
                    }
                }
                ListOp::Invalidate { start, end } => {
                    let end = end.min(self.rows.len().saturating_sub(1));
                    for row in self.rows.iter_mut().take(end + 1).skip(start) {
                        *row = None;
                    }
                }
            }
        }
        // Nothing useful at the end.
        while matches!(self.rows.last(), Some(None)) {
            self.rows.pop();
        }
        self.fix_group_counts();
    }

    /// Group headers in the ops don't always carry a count (Discord sends
    /// `{ group: { id } }`), which showed "ONLINE — 0" above online people.
    /// Use the update's `groups` counts, else count the loaded rows.
    fn fix_group_counts(&mut self) {
        let mut i = 0;
        while i < self.rows.len() {
            let Some(Row::Group { id, count }) = &self.rows[i] else {
                i += 1;
                continue;
            };
            let from_groups = self
                .groups
                .iter()
                .find(|(g, _)| g == id)
                .map(|(_, n)| *n)
                .filter(|n| *n > 0);
            let below = self.rows[i + 1..]
                .iter()
                .take_while(|r| !matches!(r, Some(Row::Group { .. })))
                .count() as u32;
            let n = from_groups.unwrap_or(if *count > 0 { *count } else { below });
            if let Some(Row::Group { count, .. }) = &mut self.rows[i] {
                *count = n;
            }
            i += 1;
        }
    }

    /// Online and offline totals for the header pills. Uses Discord's
    /// counts when present, else counts the (bot) list's rows.
    pub fn online_offline(&self, presences: &Presences) -> (u32, u32) {
        if self.member_count > 0 {
            let online = self.online_count.min(self.member_count);
            return (online, self.member_count - online);
        }
        count_online(self.rows.iter().flatten(), presences)
    }

    /// Rows in the whole list going by the group counts (headers included).
    pub fn total_rows(&self) -> usize {
        let from_groups: usize = self
            .groups
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(_, n)| *n as usize + 1)
            .sum();
        from_groups.max(self.rows.len())
    }

    pub fn row(&self, i: usize) -> Option<&Row> {
        self.rows.get(i).and_then(Option::as_ref)
    }
}

/// (online, offline) among the member rows.
pub fn count_online<'a>(rows: impl Iterator<Item = &'a Row>, presences: &Presences) -> (u32, u32) {
    let (mut on, mut off) = (0, 0);
    for r in rows {
        if let Row::Member(m) = r {
            let st = presences
                .status(&m.user.id)
                .or(m.presence.as_ref().map(|p| p.status))
                .unwrap_or(Status::Offline);
            if st == Status::Offline {
                off += 1;
            } else {
                on += 1;
            }
        }
    }
    (on, off)
}

/// The member pane header: a green pill with the online count and a grey
/// one with the offline count, from `left` along `center_y`.
pub fn count_pills(
    ui: &egui::Ui,
    left: f32,
    center_y: f32,
    online: u32,
    offline: u32,
    pal: &Palette,
) {
    let mut x = left;
    for (n, fill, tip, key) in [
        (online, pal.online, format!("{online} online"), "pill_on"),
        (
            offline,
            pal.offline,
            format!("{offline} offline"),
            "pill_off",
        ),
    ] {
        let galley = ui.painter().layout_no_wrap(
            n.to_string(),
            crate::theme::bold_font(12.0),
            Color32::WHITE,
        );
        let h = 20.0;
        let w = (galley.size().x + 16.0).max(h + 8.0);
        let rect = egui::Rect::from_min_size(egui::pos2(x, center_y - h / 2.0), vec2(w, h));
        ui.painter().rect_filled(rect, h / 2.0, fill);
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
        ui.interact(rect, ui.scope_id().with(key), egui::Sense::hover())
            .on_hover_text(tip);
        x = rect.right() + 6.0;
    }
}

/// Op 14 ranges covering the visible rows: always [0, 99], plus the
/// chunks the user has scrolled to (at most 3 ranges, like Discord).
pub fn ranges_for(first_visible: usize, last_visible: usize) -> Vec<(u32, u32)> {
    let size = RANGE_SIZE as usize;
    let mut out = vec![(0, RANGE_SIZE - 1)];
    let (a, b) = (first_visible / size, last_visible / size);
    for chunk in a.max(1)..=b.max(a.max(1)) {
        if chunk == 0 || out.len() >= 3 {
            continue;
        }
        if chunk * size <= last_visible {
            let start = (chunk * size) as u32;
            out.push((start, start + RANGE_SIZE - 1));
        }
    }
    out
}

/// Header text: "ONLINE — 12", "MODERATORS — 3".
pub fn group_label(id: &str, count: u32, roles: &[Role]) -> String {
    let name = match id {
        "online" => "Online".to_string(),
        "offline" => "Offline".to_string(),
        "dm" => "Members".to_string(),
        "recent" => "Recently Active".to_string(),
        id => roles
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "Members".into()),
    };
    format!("{} — {count}", name.to_uppercase())
}

/// Build a Discord-style grouped list ourselves (bots get a flat member
/// list from GUILD_MEMBERS_CHUNK): online members under their highest
/// hoisted role, other online members under "Online", then "Offline".
/// Sorted by name inside each group.
pub fn build_rows(members: &[Member], roles: &[Role], presences: &Presences) -> Vec<Option<Row>> {
    let status = |m: &Member| {
        presences
            .status(&m.user.id)
            .or(m.presence.as_ref().map(|p| p.status))
            .unwrap_or(Status::Offline)
    };
    // Group key: (order, id). Hoisted roles first in role order.
    let mut groups: Vec<(usize, String, Vec<&Member>)> = Vec::new();
    for m in members {
        let (order, id) = if status(m) == Status::Offline {
            (usize::MAX, "offline".to_string())
        } else if let Some(r) = hoisted_role(roles, &m.roles) {
            let order = roles.iter().position(|x| x.id == r.id).unwrap_or(0);
            (order, r.id.clone())
        } else {
            (usize::MAX - 1, "online".to_string())
        };
        match groups.iter_mut().find(|g| g.1 == id) {
            Some(g) => g.2.push(m),
            None => groups.push((order, id, vec![m])),
        }
    }
    groups.sort_by_key(|g| g.0);
    let mut rows = Vec::with_capacity(members.len() + groups.len());
    for (_, id, mut list) in groups {
        list.sort_by_cached_key(|m| m.display_name().to_lowercase());
        rows.push(Some(Row::Group {
            id,
            count: list.len() as u32,
        }));
        rows.extend(
            list.into_iter()
                .map(|m| Some(Row::Member(Box::new(m.clone())))),
        );
    }
    rows
}

// ---------------------------------------------------------------------------
// Gateway ops
// ---------------------------------------------------------------------------

/// Op 14: subscribe to a channel's member list (user accounts only).
pub fn subscribe_op(guild_id: &str, channel_id: &str, ranges: &[(u32, u32)]) -> Value {
    let ranges: Vec<[u32; 2]> = ranges.iter().map(|&(a, b)| [a, b]).collect();
    json!({ "op": 14, "d": {
        "guild_id": guild_id,
        "typing": true,
        "activities": true,
        "threads": true,
        "channels": { channel_id: ranges },
    }})
}

/// Op 8 for specific people (anyone may do this; up to 100 ids).
pub fn request_members_op(guild_id: &str, user_ids: &[String], is_bot: bool) -> Value {
    let ids: Vec<&String> = user_ids.iter().take(100).collect();
    // Discord Messenger sends guild_id as an array (user accounts); the bot
    // docs want a single snowflake.
    let guild = if is_bot {
        json!(guild_id)
    } else {
        json!([guild_id])
    };
    json!({ "op": 8, "d": {
        "guild_id": guild,
        "user_ids": ids,
        "presences": !is_bot,
        "nonce": NONCE_IDS,
    }})
}

/// Op 8 for a whole server (bots with the GUILD_MEMBERS intent).
pub fn request_member_list_op(guild_id: &str) -> Value {
    json!({ "op": 8, "d": {
        "guild_id": guild_id,
        "query": "",
        "limit": BOT_LIST_LIMIT,
        "presences": true,
        "nonce": NONCE_LIST,
    }})
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Row height in the member list (uniform so only visible rows are drawn).
pub fn row_height(compact: bool) -> f32 {
    if compact { 30.0 } else { 44.0 }
}

/// Coloured status dot with a ring in the background colour, at the
/// bottom-right of an avatar.
pub fn status_dot(ui: &egui::Ui, avatar: egui::Rect, status: Status, ring: Color32, pal: &Palette) {
    let r = (avatar.width() * 0.16).clamp(3.5, 8.0);
    let c = avatar.right_bottom() - vec2(r * 0.8, r * 0.8);
    ui.painter().circle_filled(c, r + 2.0, ring);
    ui.painter().circle_filled(c, r, status.color(pal));
    // Discord's shapes: offline is a ring, idle a crescent, DND a dash.
    match status {
        Status::Offline => {
            ui.painter().circle_filled(c, r * 0.45, ring);
        }
        Status::Idle => {
            ui.painter()
                .circle_filled(c - vec2(r * 0.5, r * 0.5), r * 0.62, ring);
        }
        Status::Dnd => {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(c, vec2(r * 1.25, (r * 0.42).max(1.5))),
                r * 0.2,
                ring,
            );
        }
        Status::Online => {}
    }
}

/// What was clicked in the list: the member and where their row is.
pub struct Clicked {
    pub member: Member,
    pub rect: egui::Rect,
}

pub struct ListView<'a> {
    pub rows: &'a dyn Fn(usize) -> Option<&'a Row>,
    pub total: usize,
    pub roles: &'a [Role],
    pub presences: &'a Presences,
}

/// Draw a grouped member list. Returns the clicked member (if any) and the
/// range of rows that were visible, for requesting more of the list.
pub fn list_ui(
    ui: &mut egui::Ui,
    view: ListView<'_>,
    config: &Config,
    pal: &Palette,
) -> (Option<Clicked>, std::ops::Range<usize>) {
    let compact = config.compact_member_list;
    let h = row_height(compact);
    let mut clicked = None;
    let mut visible = 0..0;
    // show_rows spaces rows by row height + item_spacing.y; we draw them
    // back to back, so without this the list ended in a blank strip.
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("member_list")
        .auto_shrink(false)
        .show_rows(ui, h, view.total, |ui, range| {
            visible = range.clone();
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::click());
                match (view.rows)(i) {
                    Some(Row::Group { id, count }) => {
                        // "ADMIN — 1" with a faint line after it (Discord
                        // Messenger's role headers).
                        let text = group_label(id, *count, view.roles);
                        let galley = ui.painter().layout_no_wrap(
                            text,
                            crate::theme::bold_font(11.5),
                            pal.muted,
                        );
                        let pos =
                            egui::pos2(rect.left() + 8.0, rect.bottom() - 6.0 - galley.size().y);
                        let line_y = pos.y + galley.size().y / 2.0;
                        let x0 = pos.x + galley.size().x + 8.0;
                        ui.painter().galley(pos, galley, pal.muted);
                        if x0 < rect.right() - 12.0 {
                            ui.painter().hline(
                                x0..=rect.right() - 8.0,
                                line_y,
                                egui::Stroke::new(1.0, pal.divider),
                            );
                        }
                    }
                    Some(Row::Member(m)) => {
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if member_row(ui, rect, &resp, m, &view, compact, config, pal) {
                            clicked = Some(Clicked {
                                member: (**m).clone(),
                                rect,
                            });
                        }
                    }
                    None => {
                        // Not loaded yet: a grey placeholder like Discord.
                        let a = rect.left_center() + vec2(4.0 + h * 0.35, 0.0);
                        ui.painter().circle_filled(a, h * 0.32, pal.hover);
                        let bar = egui::Rect::from_min_size(
                            egui::pos2(a.x + h * 0.5, a.y - 5.0),
                            vec2(rect.width() * 0.45, 10.0),
                        );
                        ui.painter().rect_filled(bar, 5, pal.hover);
                    }
                }
            }
        });
    ui.spacing_mut().item_spacing.y = spacing;
    (clicked, visible)
}

#[allow(clippy::too_many_arguments)]
fn member_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    resp: &egui::Response,
    m: &Member,
    view: &ListView<'_>,
    compact: bool,
    config: &Config,
    pal: &Palette,
) -> bool {
    let presence = view
        .presences
        .get(&m.user.id)
        .or(m.presence.as_ref())
        .cloned()
        .unwrap_or_default();
    let offline = presence.status == Status::Offline;
    let ring = if resp.hovered() {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(0.0, 1.0)), 4, pal.hover);
        pal.hover
    } else {
        pal.sidebar
    };
    let size = if compact { 24.0 } else { 32.0 };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if offline && !resp.hovered() {
        child.set_opacity(0.5);
    }
    let av = crate::app::avatar(
        &mut child,
        &m.user.id,
        m.display_name(),
        &m.user.avatar_url,
        size,
        config,
        pal,
    );
    if !offline {
        status_dot(&child, av, presence.status, ring, pal);
    }
    child.add_space(6.0);
    let name_color = role_color(view.roles, &m.roles).unwrap_or(pal.muted);
    let name_color = if resp.hovered() && name_color == pal.muted {
        pal.text
    } else {
        name_color
    };
    let activity = presence.activity.as_deref().filter(|_| !compact);
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // Centre the name when there's no second line.
        let top = if activity.is_some() {
            3.0
        } else {
            (ui.available_height() - 18.0) / 2.0
        };
        ui.add_space(top.max(0.0));
        ui.horizontal(|ui| {
            let name = RichText::new(m.display_name()).size(14.5).color(name_color);
            // The BOT tag keeps its room; the name is cut first (full
            // name on hover).
            let reserve = if m.user.bot { 36.0 } else { 0.0 };
            crate::widgets::truncated_label(ui, egui::Label::new(name).selectable(false), reserve);
            if m.user.bot {
                bot_tag(ui, pal);
            }
        });
        if let Some(a) = activity {
            ui.add(
                egui::Label::new(RichText::new(a).small().color(pal.muted))
                    .truncate()
                    .selectable(false),
            );
        }
    });
    if let Some(a) = presence.activity.as_deref().filter(|_| compact) {
        resp.clone().on_hover_text(a);
    }
    resp.clicked()
}

/// Discord's blurple "BOT" tag.
pub fn bot_tag(ui: &mut egui::Ui, pal: &Palette) {
    egui::Frame::new()
        .fill(pal.accent)
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(RichText::new("BOT").small().strong().color(Color32::WHITE));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles() -> Vec<Role> {
        Role::parse_list(&json!([
            { "id": "1", "name": "@everyone", "color": 0, "position": 0 },
            { "id": "10", "name": "Admin", "color": 0xe91e63, "position": 3, "hoist": true },
            { "id": "11", "name": "Helper", "color": 0, "position": 2, "hoist": true },
            { "id": "12", "name": "Green", "color": 0x2ecc71, "position": 1 },
        ]))
    }

    #[test]
    fn roles_sorted_highest_first() {
        let r = roles();
        let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Admin", "Helper", "Green", "@everyone"]);
    }

    #[test]
    fn colour_is_highest_coloured_role() {
        let r = roles();
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Helper is higher but has no colour, so Green wins.
        assert_eq!(role_color(&r, &ids(&["12", "11"])), Some(rgb(0x2ecc71)));
        assert_eq!(role_color(&r, &ids(&["12", "10"])), Some(rgb(0xe91e63)));
        assert_eq!(role_color(&r, &ids(&["11"])), None);
        assert_eq!(role_color(&r, &[]), None);
        assert_eq!(
            hoisted_role(&r, &ids(&["12", "11"])).unwrap().name,
            "Helper"
        );
        let chips: Vec<&str> = member_roles(&r, &ids(&["12", "10", "999"]))
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(chips, ["Admin", "Green"]);
    }

    #[test]
    fn gradient_role_colour_fallback() {
        let r =
            Role::from_json(&json!({ "id": "1", "color": 0, "colors": { "primary_color": 255 } }));
        assert_eq!(r.color, 255);
    }

    fn member(id: &str) -> Value {
        json!({ "member": { "user": { "id": id, "username": format!("u{id}") }, "roles": [] } })
    }

    fn ids(list: &MemberList) -> Vec<String> {
        list.rows
            .iter()
            .map(|r| match r {
                Some(Row::Group { id, .. }) => format!("g:{id}"),
                Some(Row::Member(m)) => m.user.id.clone(),
                None => "-".into(),
            })
            .collect()
    }

    fn update(ops: Value) -> ListUpdate {
        ListUpdate::from_json(&json!({
            "guild_id": "g", "id": "everyone", "member_count": 5, "online_count": 3,
            "groups": [{ "id": "online", "count": 2 }, { "id": "offline", "count": 1 }],
            "ops": ops,
        }))
        .unwrap()
    }

    #[test]
    fn list_ops() {
        let mut list = MemberList::new("g", "everyone");
        list.apply(update(json!([{ "op": "SYNC", "range": [0, 99], "items": [
            { "group": { "id": "online", "count": 2 } }, member("a"), member("b"),
            { "group": { "id": "offline", "count": 1 } }, member("c"),
        ]}])));
        assert_eq!(ids(&list), ["g:online", "a", "b", "g:offline", "c"]);
        assert_eq!(list.total_rows(), 5);
        assert_eq!(list.online_count, 3);

        list.apply(update(json!([
            { "op": "DELETE", "index": 4 },
            { "op": "INSERT", "index": 3, "item": member("c") },
            { "op": "UPDATE", "index": 1, "item": member("z") },
        ])));
        assert_eq!(ids(&list), ["g:online", "z", "b", "c", "g:offline"]);

        // Out of range ops are ignored rather than panicking.
        list.apply(update(json!([
            { "op": "DELETE", "index": 50 },
            { "op": "UPDATE", "index": 50, "item": member("q") },
            { "op": "BOGUS" },
        ])));
        assert_eq!(list.rows.len(), 5);

        list.apply(update(json!([{ "op": "INVALIDATE", "range": [0, 1] }])));
        assert_eq!(ids(&list), ["-", "-", "b", "c", "g:offline"]);
        list.apply(update(json!([{ "op": "INVALIDATE", "range": [2, 99] }])));
        assert!(list.rows.is_empty());
    }

    #[test]
    fn group_counts_without_count_field() {
        let count = |list: &MemberList, i: usize| match list.row(i) {
            Some(Row::Group { count, .. }) => *count,
            _ => panic!("not a group"),
        };
        // No counts on the header items: the `groups` counts are used.
        let mut list = MemberList::new("g", "everyone");
        list.apply(update(json!([{ "op": "SYNC", "range": [0, 99], "items": [
            { "group": { "id": "online" } }, member("a"), member("b"),
            { "group": { "id": "offline" } }, member("c"),
        ]}])));
        assert_eq!((count(&list, 0), count(&list, 3)), (2, 1));
        // No `groups` either: count the rows under each header.
        let mut upd = update(json!([{ "op": "SYNC", "range": [0, 99], "items": [
            { "group": { "id": "online" } }, member("a"), member("b"), member("d"),
        ]}]));
        upd.groups.clear();
        let mut list = MemberList::new("g", "everyone");
        list.apply(upd);
        assert_eq!(count(&list, 0), 3);
        assert_eq!(list.online_offline(&Presences::default()), (3, 2));
    }

    #[test]
    fn sync_second_range_and_shrink() {
        let mut list = MemberList::new("g", "x");
        let items: Vec<Value> = (0..3).map(|i| member(&format!("m{i}"))).collect();
        list.apply(update(
            json!([{ "op": "SYNC", "range": [100, 199], "items": items }]),
        ));
        assert_eq!(list.rows.len(), 103);
        assert!(list.row(0).is_none());
        assert!(list.row(101).is_some());
        // A later SYNC of the first range with fewer items ends the list.
        list.apply(update(
            json!([{ "op": "SYNC", "range": [0, 99], "items": [member("a")] }]),
        ));
        assert_eq!(ids(&list), ["a"]);
    }

    #[test]
    fn insert_past_end_pads() {
        let mut list = MemberList::new("g", "x");
        list.apply(update(
            json!([{ "op": "INSERT", "index": 2, "item": member("a") }]),
        ));
        assert_eq!(ids(&list), ["-", "-", "a"]);
    }

    #[test]
    fn update_members_iter() {
        let u = update(json!([
            { "op": "SYNC", "range": [0, 99], "items": [{ "group": { "id": "online", "count": 1 } }, member("a")] },
            { "op": "INSERT", "index": 2, "item": member("b") },
        ]));
        let ids: Vec<&str> = u.members().map(|m| m.user.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn ranges() {
        assert_eq!(ranges_for(0, 30), vec![(0, 99)]);
        assert_eq!(ranges_for(80, 120), vec![(0, 99), (100, 199)]);
        assert_eq!(ranges_for(250, 290), vec![(0, 99), (200, 299)]);
        assert_eq!(ranges_for(280, 320), vec![(0, 99), (200, 299), (300, 399)]);
    }

    #[test]
    fn labels() {
        let r = roles();
        assert_eq!(group_label("online", 12, &r), "ONLINE — 12");
        assert_eq!(group_label("10", 2, &r), "ADMIN — 2");
        assert_eq!(group_label("404", 1, &r), "MEMBERS — 1");
    }

    #[test]
    fn local_grouping_for_bots() {
        let r = roles();
        let m = |id: &str, name: &str, roles: &[&str], status: &str| {
            Member::from_json(&json!({
                "user": { "id": id, "username": name },
                "roles": roles,
                "presence": { "status": status },
            }))
        };
        let members = vec![
            m("1", "zed", &[], "online"),
            m("2", "amy", &["10"], "online"),
            m("3", "bob", &["10"], "offline"),
            m("4", "cat", &["11", "12"], "dnd"),
            m("5", "abe", &[], "idle"),
        ];
        let rows = build_rows(&members, &r, &Presences::default());
        let list = MemberList {
            rows,
            ..MemberList::default()
        };
        assert_eq!(
            ids(&list),
            [
                "g:10",
                "2",
                "g:11",
                "4",
                "g:online",
                "5",
                "1",
                "g:offline",
                "3"
            ]
        );
    }

    #[test]
    fn member_cache_cap() {
        let mut c = MemberCache::default();
        let m = |id: usize| Member {
            user: User {
                id: id.to_string(),
                ..User::default()
            },
            roles: vec!["r".into()],
            ..Member::default()
        };
        for i in 0..MAX_CACHED_MEMBERS {
            c.insert("other", &m(i));
        }
        c.insert("here", &m(1));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get("here", "1").unwrap().roles, ["r"]);
        assert!(c.get("other", "1").is_none());
    }

    #[test]
    fn ops_json() {
        let op = subscribe_op("g", "c", &[(0, 99)]);
        assert_eq!(op["op"], 14);
        assert_eq!(op["d"]["channels"]["c"], json!([[0, 99]]));
        let op = request_members_op("g", &["1".into()], false);
        assert_eq!(op["d"]["guild_id"], json!(["g"]));
        let op = request_members_op("g", &["1".into()], true);
        assert_eq!(op["d"]["guild_id"], json!("g"));
        assert_eq!(op["d"]["presences"], json!(false));
    }
}
