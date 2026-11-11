//! Server settings → Channels: create, edit, delete and drag-reorder
//! channels and categories (topic, slowmode, NSFW, bitrate, user limit)
//! and the permission overwrites for each role and member.
//!
//! Editing a channel needs Manage Channels in that channel; its
//! overwrites need Manage Roles there, and only permissions I have in the
//! channel can be changed (as Discord enforces).

use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};

use super::{Confirm, Cx, Kind, Method, Out, Request, b, n, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

pub const TEXT: u64 = 0;
pub const VOICE: u64 = 2;
pub const CATEGORY: u64 = 4;
pub const NEWS: u64 = 5;
pub const STAGE: u64 = 13;
pub const FORUM: u64 = 15;

fn is_voice(kind: u64) -> bool {
    matches!(kind, VOICE | STAGE)
}

pub fn kind_icon(kind: u64) -> &'static str {
    match kind {
        VOICE => icons::SPEAKER,
        CATEGORY => icons::FOLDER,
        NEWS => icons::MEGAPHONE,
        STAGE => icons::BROADCAST,
        FORUM => icons::CHATS,
        _ => icons::HASH,
    }
}

pub const SLOWMODE: &[(u64, &str)] = &[
    (0, "Off"),
    (5, "5 seconds"),
    (10, "10 seconds"),
    (15, "15 seconds"),
    (30, "30 seconds"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
    (7200, "2 hours"),
    (21_600, "6 hours"),
];

/// Highest voice bitrate (kbps) for a boost tier.
pub fn max_bitrate(premium_tier: u8) -> u64 {
    match premium_tier {
        0 => 96,
        1 => 128,
        2 => 256,
        _ => 384,
    }
}

#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub name: String,
    pub kind: u64,
    pub topic: String,
    pub slowmode: u64,
    pub nsfw: bool,
    /// kbps.
    pub bitrate: u64,
    pub user_limit: u64,
    pub parent: String,
}

impl Draft {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: s(v, "name").to_string(),
            kind: n(v, "type"),
            topic: s(v, "topic").to_string(),
            slowmode: n(v, "rate_limit_per_user"),
            nsfw: b(v, "nsfw"),
            bitrate: (n(v, "bitrate") / 1000).max(8),
            user_limit: n(v, "user_limit"),
            parent: s(v, "parent_id").to_string(),
        }
    }

    /// The name as Discord stores it: text channels are lower-case with
    /// dashes for spaces.
    pub fn clean_name(&self) -> String {
        let name = self.name.trim();
        if is_voice(self.kind) || self.kind == CATEGORY {
            name.to_string()
        } else {
            name.to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-")
        }
    }

    pub fn valid(&self) -> bool {
        (1..=100).contains(&self.clean_name().chars().count())
    }

    /// Fields that differ from `base` (all that apply, for a new channel).
    pub fn patch(&self, base: Option<&Draft>) -> Map<String, Value> {
        let mut m = Map::new();
        let new = base.is_none();
        let name = self.clean_name();
        if base.is_none_or(|b| b.name != name) {
            m.insert("name".into(), json!(name));
        }
        if new {
            m.insert("type".into(), json!(self.kind));
        }
        if self.kind != CATEGORY && base.is_none_or(|b| b.parent != self.parent) {
            m.insert("parent_id".into(), super::id_or_null(&self.parent));
        }
        if self.kind == CATEGORY {
            return m;
        }
        if base.is_none_or(|b| b.nsfw != self.nsfw) && (!new || self.nsfw) {
            m.insert("nsfw".into(), json!(self.nsfw));
        }
        if is_voice(self.kind) {
            if base.is_none_or(|b| b.bitrate != self.bitrate) {
                m.insert("bitrate".into(), json!(self.bitrate * 1000));
            }
            if base.is_none_or(|b| b.user_limit != self.user_limit) && (!new || self.user_limit > 0)
            {
                m.insert("user_limit".into(), json!(self.user_limit));
            }
        } else {
            let topic = self.topic.trim();
            if base.is_none_or(|b| b.topic.trim() != topic) && (!new || !topic.is_empty()) {
                m.insert("topic".into(), json!(topic));
            }
            if base.is_none_or(|b| b.slowmode != self.slowmode) && (!new || self.slowmode > 0) {
                m.insert("rate_limit_per_user".into(), json!(self.slowmode));
            }
        }
        m
    }
}

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadChannels,
        Method::Get,
        format!("/guilds/{guild_id}/channels"),
    )
}

pub fn create(guild_id: &str, draft: &Draft) -> Option<Request> {
    draft.valid().then(|| {
        Request::new(
            guild_id,
            Kind::CreateChannel,
            Method::Post,
            format!("/guilds/{guild_id}/channels"),
        )
        .body(Value::Object(draft.patch(None)))
    })
}

pub fn save(guild_id: &str, id: &str, base: &Draft, draft: &Draft) -> Option<Request> {
    let patch = draft.patch(Some(base));
    if patch.is_empty() || !draft.valid() {
        return None;
    }
    Some(
        Request::new(
            guild_id,
            Kind::SaveChannel,
            Method::Patch,
            format!("/channels/{id}"),
        )
        .key(id)
        .body(Value::Object(patch)),
    )
}

pub fn delete(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteChannel,
        Method::Delete,
        format!("/channels/{id}"),
    )
    .key(id)
}

/// Set an overwrite. `target`: 0 role, 1 member. Reply key: `channel:id`.
pub fn save_overwrite(
    guild_id: &str,
    channel_id: &str,
    target_id: &str,
    target: u8,
    allow: u64,
    deny: u64,
) -> Request {
    Request::new(
        guild_id,
        Kind::SaveOverwrite,
        Method::Put,
        format!("/channels/{channel_id}/permissions/{target_id}"),
    )
    .key(&format!("{channel_id}:{target_id}"))
    .body(json!({ "type": target, "allow": allow.to_string(), "deny": deny.to_string() }))
}

pub fn delete_overwrite(guild_id: &str, channel_id: &str, target_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteOverwrite,
        Method::Delete,
        format!("/channels/{channel_id}/permissions/{target_id}"),
    )
    .key(&format!("{channel_id}:{target_id}"))
}

/// One line of the channel list as shown (and dragged).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub category: bool,
}

/// Channels in display order: channels without a category, then each
/// category followed by its channels (text before voice, by position).
pub fn rows(channels: &[Value]) -> Vec<Row> {
    let pos = |c: &Value| c["position"].as_i64().unwrap_or(0);
    let mut out = Vec::new();
    let children = |parent: Option<&str>, out: &mut Vec<Row>| {
        let mut list: Vec<&Value> = channels
            .iter()
            .filter(|c| n(c, "type") != CATEGORY)
            .filter(|c| match parent {
                Some(p) => s(c, "parent_id") == p,
                None => s(c, "parent_id").is_empty(),
            })
            .collect();
        list.sort_by_key(|c| (is_voice(n(c, "type")), pos(c)));
        out.extend(list.iter().map(|c| Row {
            id: s(c, "id").to_string(),
            parent: parent.map(str::to_string),
            category: false,
        }));
    };
    children(None, &mut out);
    let mut cats: Vec<&Value> = channels
        .iter()
        .filter(|c| n(c, "type") == CATEGORY)
        .collect();
    cats.sort_by_key(|c| pos(c));
    for c in cats {
        out.push(Row {
            id: s(c, "id").to_string(),
            parent: None,
            category: true,
        });
        children(Some(s(c, "id")), &mut out);
    }
    out
}

/// Drop row `from` before what is at `to`. A category moves with its
/// channels and only lands between categories; a channel takes the
/// category it lands under.
pub fn drop_row(rows: &[Row], from: usize, to: usize) -> Vec<Row> {
    let Some(moved) = rows.get(from) else {
        return rows.to_vec();
    };
    let mut out = rows.to_vec();
    if moved.category {
        let len = 1 + rows[from + 1..]
            .iter()
            .take_while(|r| r.parent.as_deref() == Some(moved.id.as_str()))
            .count();
        // Land on a category boundary (or the end).
        let mut to = to.min(rows.len());
        while to < rows.len() && !rows[to].category {
            to += 1;
        }
        if to > from && to < from + len {
            return out;
        }
        let block: Vec<Row> = out.drain(from..from + len).collect();
        let at = if to > from { to - len } else { to };
        // Never above the channels that have no category.
        let first_cat = out.iter().position(|r| r.category).unwrap_or(out.len());
        let at = at.max(first_cat).min(out.len());
        out.splice(at..at, block);
    } else {
        let mut row = out.remove(from);
        let at = (if to > from { to - 1 } else { to }).min(out.len());
        row.parent = out[..at].iter().rev().find_map(|r| {
            if r.category {
                Some(r.id.clone())
            } else {
                r.parent.clone()
            }
        });
        out.insert(at, row);
    }
    out
}

/// The bulk position update for a reordered list: only channels whose
/// position or category changed. `None` when nothing did.
pub fn reorder(guild_id: &str, channels: &[Value], order: &[Row]) -> Option<Request> {
    let mut moved = Vec::new();
    let mut cat_pos = 0;
    let mut child_pos: std::collections::HashMap<Option<&str>, i64> = Default::default();
    for row in order {
        let Some(c) = channels.iter().find(|c| s(c, "id") == row.id) else {
            continue;
        };
        let was = c["position"].as_i64().unwrap_or(0);
        if row.category {
            if was != cat_pos {
                moved.push(json!({ "id": row.id, "position": cat_pos }));
            }
            cat_pos += 1;
            continue;
        }
        let slot = child_pos.entry(row.parent.as_deref()).or_insert(0);
        let pos = *slot;
        *slot += 1;
        let was_parent = s(c, "parent_id");
        let parent = row.parent.as_deref().unwrap_or("");
        if was_parent != parent {
            moved.push(json!({ "id": row.id, "position": pos,
                               "parent_id": super::id_or_null(parent) }));
        } else if was != pos {
            moved.push(json!({ "id": row.id, "position": pos }));
        }
    }
    if moved.is_empty() {
        return None;
    }
    Some(
        Request::new(
            guild_id,
            Kind::MoveChannels,
            Method::Patch,
            format!("/guilds/{guild_id}/channels"),
        )
        .body(Value::Array(moved)),
    )
}

/// Apply a saved bulk position update to the loaded channels.
fn apply_moves(channels: &mut [Value], moves: &Value) {
    for m in moves.as_array().into_iter().flatten() {
        if let Some(c) = channels.iter_mut().find(|c| s(c, "id") == s(m, "id")) {
            super::merge(c, m);
        }
    }
}

/// Set or remove one overwrite in a channel object.
fn apply_overwrite(channel: &mut Value, target_id: &str, new: Option<&Value>) {
    let Some(o) = channel.as_object_mut() else {
        return;
    };
    let list = o
        .entry("permission_overwrites")
        .or_insert_with(|| json!([]));
    let Some(list) = list.as_array_mut() else {
        return;
    };
    list.retain(|x| s(x, "id") != target_id);
    if let Some(new) = new {
        let mut item = new.clone();
        if let Some(io) = item.as_object_mut() {
            io.insert("id".into(), json!(target_id));
        }
        list.push(item);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Permissions,
}

/// The overwrite being edited.
struct OwEdit {
    id: String,
    /// 0 role, 1 member.
    target: u8,
    name: String,
    allow: u64,
    deny: u64,
    base: (u64, u64),
    exists: bool,
}

struct Edit {
    id: Option<String>,
    base: Draft,
    draft: Draft,
    page: Page,
    ow: Option<OwEdit>,
    add_role: String,
}

#[derive(Default)]
pub struct State {
    channels: Option<Vec<Value>>,
    order: Option<Vec<Row>>,
    edit: Option<Edit>,
    /// Bodies of writes whose reply is empty (204), applied on success.
    sent_moves: Option<Value>,
    sent_overwrite: Option<(String, Value)>,
    query: String,
    found: Option<Vec<Value>>,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadChannels => {
                self.channels = Some(super::list(v));
                self.order = None;
            }
            Kind::CreateChannel | Kind::SaveChannel => {
                if let Some(list) = &mut self.channels {
                    super::upsert(list, v);
                }
                self.edit = None;
            }
            Kind::DeleteChannel => {
                if let Some(list) = &mut self.channels {
                    super::remove(list, key);
                    for c in list.iter_mut().filter(|c| s(c, "parent_id") == key) {
                        c["parent_id"] = Value::Null;
                    }
                }
                self.edit = None;
            }
            Kind::MoveChannels => {
                if let (Some(list), Some(moves)) = (&mut self.channels, self.sent_moves.take()) {
                    apply_moves(list, &moves);
                }
                self.order = None;
            }
            Kind::SaveOverwrite | Kind::DeleteOverwrite => {
                let Some((cid, oid)) = key.split_once(':') else {
                    return;
                };
                let sent = self
                    .sent_overwrite
                    .take()
                    .filter(|(k, _)| k == key && kind == Kind::SaveOverwrite)
                    .map(|(_, body)| body);
                if let Some(c) = self
                    .channels
                    .iter_mut()
                    .flatten()
                    .find(|c| s(c, "id") == cid)
                {
                    apply_overwrite(c, oid, sent.as_ref());
                }
                if let Some(ow) = self.edit.as_mut().and_then(|e| e.ow.as_mut()) {
                    if kind == Kind::SaveOverwrite {
                        ow.base = (ow.allow, ow.deny);
                        ow.exists = true;
                    } else if let Some(e) = &mut self.edit {
                        e.ow = None;
                    }
                }
            }
            Kind::ChannelMemberSearch => self.found = Some(super::list(v)),
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        if self.channels.is_none() {
            aui::loading(ui, cx.pal);
            return;
        }
        if self.edit.is_some() {
            self.edit_ui(ui, cx, out);
        } else {
            self.list_ui(ui, cx, out);
        }
    }

    /// Demo screenshots: #general's permission overwrites.
    pub fn demo_open(&mut self) {
        let Some(c) = self
            .channels
            .iter()
            .flatten()
            .find(|c| s(c, "name") == "general")
            .cloned()
        else {
            return;
        };
        self.start_edit(Some(&c), 0);
        if let Some(e) = &mut self.edit {
            e.page = Page::Permissions;
            e.ow = Some(OwEdit {
                id: "501".into(),
                target: 0,
                name: "Moderators".into(),
                allow: perms::MANAGE_MESSAGES,
                deny: 0,
                base: (perms::MANAGE_MESSAGES, 0),
                exists: true,
            });
        }
    }

    fn start_edit(&mut self, c: Option<&Value>, kind: u64) {
        let draft = match c {
            Some(c) => Draft::from_json(c),
            None => Draft {
                kind,
                bitrate: 64,
                ..Default::default()
            },
        };
        self.edit = Some(Edit {
            id: c.map(|c| s(c, "id").to_string()),
            base: draft.clone(),
            draft,
            page: Page::Overview,
            ow: None,
            add_role: String::new(),
        });
        self.found = None;
        self.query.clear();
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(channels) = &self.channels else {
            return;
        };
        let can_create = cx.can(perms::MANAGE_CHANNELS);
        let mut create_kind = None;
        if can_create {
            ui.horizontal(|ui| {
                if aui::primary(ui, pal, "Create Channel", true).clicked() {
                    create_kind = Some(TEXT);
                }
                if aui::plain(ui, "Create Category").clicked() {
                    create_kind = Some(CATEGORY);
                }
            });
            aui::hint(
                ui,
                pal,
                "Drag a channel or category by its grip to reorder, then save the order.",
            );
            ui.add_space(6.0);
        }
        let order = self.order.clone().unwrap_or_else(|| rows(channels));
        let mut reorder = aui::Reorder::new("admin_channels_order");
        let mut open = None;
        for (i, row) in order.iter().enumerate() {
            let Some(c) = channels.iter().find(|c| s(c, "id") == row.id) else {
                continue;
            };
            let kind = n(c, "type");
            let editable = cx.channel_perms(&row.id) & perms::MANAGE_CHANNELS != 0;
            let (resp, clicked) = aui::row(ui, pal, false, |ui| {
                if row.parent.is_some() {
                    ui.add_space(18.0);
                }
                reorder.handle(ui, i, editable && can_create, pal);
                ui.label(
                    RichText::new(kind_icon(kind))
                        .font(icons::font(16.0))
                        .color(pal.muted),
                );
                if row.category {
                    ui.label(pal.section_label(s(c, "name")));
                } else {
                    ui.label(RichText::new(s(c, "name")).color(pal.strong));
                }
                if b(c, "nsfw") {
                    ui.label(RichText::new("age-restricted").size(12.0).color(pal.muted));
                }
                if n(c, "rate_limit_per_user") > 0 {
                    ui.label(
                        RichText::new(icons::TIMER)
                            .font(icons::font(14.0))
                            .color(pal.muted),
                    )
                    .on_hover_text("Slowmode is on");
                }
                aui::right(ui, |ui| {
                    if editable {
                        crate::widgets::icon_button(
                            ui,
                            icons::PENCIL,
                            "Edit channel",
                            false,
                            true,
                            pal,
                        )
                        .clicked()
                    } else {
                        ui.label(
                            RichText::new(icons::LOCK)
                                .font(icons::font(16.0))
                                .color(pal.muted),
                        )
                        .on_hover_text("You can't manage this channel");
                        false
                    }
                })
            });
            reorder.row(resp.rect);
            if clicked {
                open = Some(c.clone());
            }
        }
        if order.is_empty() {
            aui::empty(ui, pal, icons::HASH, "This server has no channels.");
        }
        if let Some((from, to)) = reorder.finish(ui, pal) {
            let next = drop_row(&order, from, to);
            if next != rows(channels) {
                self.order = Some(next);
            } else {
                self.order = None;
            }
        }
        if let Some(next) = &self.order {
            aui::divider(ui, pal);
            let mut reset = false;
            ui.horizontal(|ui| {
                if aui::save(ui, pal, "Save Order", true).clicked() {
                    match reorder_request(&cx.guild.id, channels, next) {
                        Some(req) => {
                            self.sent_moves = req.body.clone();
                            out.send.push(req);
                        }
                        None => reset = true,
                    }
                }
                if aui::plain(ui, "Reset").clicked() {
                    reset = true;
                }
                ui.label(RichText::new("The new order isn't saved yet.").color(pal.muted));
            });
            if reset {
                self.order = None;
            }
        }
        if let Some(c) = open {
            self.start_edit(Some(&c), 0);
        } else if let Some(kind) = create_kind {
            self.start_edit(None, kind);
        }
    }

    fn edit_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(channels) = &self.channels else {
            return;
        };
        let Some(e) = &mut self.edit else { return };
        let mut back = false;
        ui.horizontal(|ui| {
            if aui::icon_text(ui, icons::CARET_LEFT, "Channels").clicked() {
                back = true;
            }
            ui.label(
                RichText::new(kind_icon(e.draft.kind))
                    .font(icons::font(18.0))
                    .color(pal.muted),
            );
            let title = match (&e.id, e.draft.kind) {
                (None, CATEGORY) => "New category".to_string(),
                (None, _) => "New channel".to_string(),
                _ => e.base.name.clone(),
            };
            ui.label(RichText::new(title).size(16.0).color(pal.strong).bold());
        });
        let chan_perms = e.id.as_deref().map_or(cx.perms, |id| cx.channel_perms(id));
        let can_overwrite = cx.owner || chan_perms & perms::MANAGE_ROLES != 0;
        if e.id.is_some() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(e.page == Page::Overview, "Overview")
                    .clicked()
                {
                    e.page = Page::Overview;
                }
                if can_overwrite
                    && ui
                        .selectable_label(e.page == Page::Permissions, "Permissions")
                        .clicked()
                {
                    e.page = Page::Permissions;
                }
            });
        }
        aui::divider(ui, pal);

        if e.page == Page::Permissions && can_overwrite {
            let id = e.id.clone().unwrap_or_default();
            let current = channels.iter().find(|c| s(c, "id") == id);
            overwrites_ui(
                ui,
                cx,
                &id,
                current,
                chan_perms,
                e,
                &mut self.query,
                &self.found,
                &mut self.sent_overwrite,
                out,
            );
        } else {
            overview_ui(ui, cx, channels, e);
            aui::divider(ui, pal);
            let dirty = e.draft != e.base || e.id.is_none();
            ui.horizontal(|ui| {
                let label = match (&e.id, e.draft.kind) {
                    (Some(_), _) => "Save Changes",
                    (None, CATEGORY) => "Create Category",
                    (None, _) => "Create Channel",
                };
                if aui::save(ui, pal, label, dirty && e.draft.valid()).clicked() {
                    let req = match &e.id {
                        Some(id) => save(&cx.guild.id, id, &e.base, &e.draft),
                        None => create(&cx.guild.id, &e.draft),
                    };
                    out.send.extend(req);
                }
                if e.id.is_some() && e.draft != e.base && aui::plain(ui, "Reset").clicked() {
                    e.draft = e.base.clone();
                }
                if let Some(id) = &e.id {
                    aui::right(ui, |ui| {
                        let what = if e.base.kind == CATEGORY {
                            "Delete Category"
                        } else {
                            "Delete Channel"
                        };
                        if aui::danger(ui, pal, what, true).clicked() {
                            let body = if e.base.kind == CATEGORY {
                                format!(
                                    "Delete the category \"{}\"? Its channels are kept, without a category. This can't be undone.",
                                    e.base.name
                                )
                            } else {
                                format!(
                                    "Delete \"{}\"? All of its messages are lost. This can't be undone.",
                                    e.base.name
                                )
                            };
                            out.confirm = Some(
                                Confirm::new(what, &body, what, delete(&cx.guild.id, id))
                                    .typed(&e.base.name)
                                    .reason(),
                            );
                        }
                    });
                }
            });
        }
        if back {
            self.edit = None;
        }
    }
}

fn reorder_request(guild_id: &str, channels: &[Value], next: &[Row]) -> Option<Request> {
    reorder(guild_id, channels, next)
}

fn overview_ui(ui: &mut egui::Ui, cx: &Cx, channels: &[Value], e: &mut Edit) {
    let pal = cx.pal;
    let new = e.id.is_none();
    let d = &mut e.draft;
    if new && d.kind != CATEGORY {
        aui::label(ui, pal, "Channel type");
        let mut kinds = vec![(TEXT, "Text".to_string()), (VOICE, "Voice".to_string())];
        if cx.guild.features.iter().any(|f| f == "COMMUNITY") {
            kinds.push((NEWS, "Announcement".to_string()));
            kinds.push((FORUM, "Forum".to_string()));
            kinds.push((STAGE, "Stage".to_string()));
        }
        aui::select(ui, pal, "ch_kind", &mut d.kind, &kinds);
    }
    aui::label(
        ui,
        pal,
        if d.kind == CATEGORY {
            "Category name"
        } else {
            "Channel name"
        },
    );
    aui::text(ui, &mut d.name, "new-channel", 100);
    if !is_voice(d.kind) && d.kind != CATEGORY && d.clean_name() != d.name {
        aui::hint(ui, pal, &format!("Will be saved as #{}", d.clean_name()));
    }
    if d.kind == CATEGORY {
        return;
    }

    aui::label(ui, pal, "Category");
    let mut cats = vec![(String::new(), "No category".to_string())];
    cats.extend(
        channels
            .iter()
            .filter(|c| n(c, "type") == CATEGORY)
            .map(|c| (s(c, "id").to_string(), s(c, "name").to_string())),
    );
    aui::select(ui, pal, "ch_parent", &mut d.parent, &cats);

    if is_voice(d.kind) {
        aui::label(ui, pal, "Bitrate");
        let max = max_bitrate(cx.guild.premium_tier);
        d.bitrate = d.bitrate.clamp(8, max);
        ui.add(egui::Slider::new(&mut d.bitrate, 8..=max).suffix(" kbps"));
        aui::hint(
            ui,
            pal,
            "Higher sounds better but needs a better connection. Above 64 kbps may hurt people on slow connections.",
        );
        aui::label(ui, pal, "User limit");
        ui.add(
            egui::Slider::new(&mut d.user_limit, 0..=99).custom_formatter(|v, _| {
                if v == 0.0 {
                    "No limit".to_string()
                } else {
                    format!("{v:.0} users")
                }
            }),
        );
    } else {
        aui::label(ui, pal, "Channel topic");
        aui::text_area(
            ui,
            &mut d.topic,
            "Let everyone know how to use this channel",
            1024,
            3,
        );
        aui::label(ui, pal, "Slowmode");
        aui::select(ui, pal, "ch_slow", &mut d.slowmode, &aui::opts(SLOWMODE));
        aui::hint(
            ui,
            pal,
            "Members can send one message per interval, unless they have Manage Channel or Manage Messages.",
        );
    }
    aui::divider(ui, pal);
    crate::settings::switch_row(
        ui,
        &mut d.nsfw,
        "Age-restricted channel",
        "Members must confirm they are of legal age to view the content here.",
        pal,
    );
}

/// The overwrites page: pick a role or member, then allow / inherit / deny
/// each permission. Nothing is sent until "Save Permissions".
#[allow(clippy::too_many_arguments)]
fn overwrites_ui(
    ui: &mut egui::Ui,
    cx: &Cx,
    channel_id: &str,
    channel: Option<&Value>,
    chan_perms: u64,
    e: &mut Edit,
    query: &mut String,
    found: &Option<Vec<Value>>,
    sent: &mut Option<(String, Value)>,
    out: &mut Out,
) {
    let pal = cx.pal;
    let existing: Vec<&Value> = channel
        .and_then(|c| c["permission_overwrites"].as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let role_name = |id: &str| {
        cx.guild
            .roles
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
    };
    let target_of = |o: &Value| -> u8 {
        match &o["type"] {
            Value::Number(x) if x.as_u64() == Some(1) => 1,
            Value::String(x) if x == "member" => 1,
            _ => 0,
        }
    };
    let name_of = |o: &Value| -> String {
        let id = s(o, "id");
        if target_of(o) == 0 {
            role_name(id).unwrap_or_else(|| format!("Role {id}"))
        } else {
            found
                .iter()
                .flatten()
                .find(|m| m.pointer("/user/id").and_then(Value::as_str) == Some(id))
                .map(super::roles::member_name)
                .unwrap_or_else(|| format!("Member {id}"))
        }
    };
    let may_edit_role = |id: &str| {
        id == cx.guild.id
            || cx
                .guild
                .roles
                .iter()
                .find(|r| r.id == id)
                .is_none_or(|r| perms::outranks(cx.owner, cx.top, r.position))
    };

    aui::hint(
        ui,
        pal,
        "Overwrites change what a role or member can do in this channel only. A member's own overwrite wins over role overwrites.",
    );
    aui::label(ui, pal, "Roles and members");
    let mut pick: Option<OwEdit> = None;
    ui.horizontal_wrapped(|ui| {
        for o in &existing {
            let id = s(o, "id");
            let selected = e.ow.as_ref().is_some_and(|w| w.id == id);
            let icon = if target_of(o) == 0 {
                icons::ID
            } else {
                icons::USER
            };
            if ui
                .selectable_label(selected, format!("{icon} {}", name_of(o)))
                .clicked()
            {
                let allow = perms::parse_bits(&o["allow"]);
                let deny = perms::parse_bits(&o["deny"]);
                pick = Some(OwEdit {
                    id: id.to_string(),
                    target: target_of(o),
                    name: name_of(o),
                    allow,
                    deny,
                    base: (allow, deny),
                    exists: true,
                });
            }
        }
        if existing.is_empty() {
            ui.label(RichText::new("No overwrites yet.").color(pal.muted));
        }
    });

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let roles: Vec<(String, String)> = std::iter::once((String::new(), "Add a role…".into()))
            .chain(
                cx.guild
                    .roles
                    .iter()
                    .filter(|r| !existing.iter().any(|o| s(o, "id") == r.id))
                    .filter(|r| may_edit_role(&r.id))
                    .map(|r| (r.id.clone(), r.name.clone())),
            )
            .collect();
        if aui::select(ui, pal, "ow_add_role", &mut e.add_role, &roles) && !e.add_role.is_empty() {
            pick = Some(OwEdit {
                id: e.add_role.clone(),
                target: 0,
                name: role_name(&e.add_role).unwrap_or_default(),
                allow: 0,
                deny: 0,
                base: (0, 0),
                exists: false,
            });
            e.add_role.clear();
        }
    });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Add a member: name or nickname")
                .desired_width(ui.available_width().min(280.0)),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if aui::plain(ui, "Search").clicked() || enter {
            out.send.extend(super::roles::member_search(
                &cx.guild.id,
                Kind::ChannelMemberSearch,
                query,
            ));
        }
    });
    if let Some(found) = found {
        ui.horizontal_wrapped(|ui| {
            for m in found.iter().take(12) {
                let uid = m.pointer("/user/id").and_then(Value::as_str).unwrap_or("");
                if existing.iter().any(|o| s(o, "id") == uid) {
                    continue;
                }
                let name = super::roles::member_name(m);
                if ui
                    .button(format!("{} {name}", icons::USER_PLUS))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    pick = Some(OwEdit {
                        id: uid.to_string(),
                        target: 1,
                        name,
                        allow: 0,
                        deny: 0,
                        base: (0, 0),
                        exists: false,
                    });
                }
            }
            if found.is_empty() {
                ui.label(RichText::new("No members found.").color(pal.muted));
            }
        });
    }
    if let Some(p) = pick {
        e.ow = Some(p);
    }

    let Some(ow) = &mut e.ow else { return };
    aui::divider(ui, pal);
    let editable = ow.target == 1 || may_edit_role(&ow.id);
    aui::heading(ui, pal, &format!("Permissions for {}", ow.name));
    if !editable {
        aui::hint(
            ui,
            pal,
            "This role is not below your highest role, so you can't change its overwrites.",
        );
    }
    let voice = is_voice(e.base.kind);
    let mut group = "";
    for p in perms::PERMISSIONS.iter().filter(|p| p.channel) {
        // Voice permissions only matter in voice channels (and categories).
        if (p.group == "Voice" || p.group == "Stage") && !voice && e.base.kind != CATEGORY {
            continue;
        }
        if p.group != group {
            group = p.group;
            aui::label(ui, pal, &format!("{group} permissions"));
            ui.add_space(4.0);
        }
        let mine = cx.owner || chan_perms & p.bit != 0;
        let mut allow = ow.allow & p.bit != 0;
        let mut deny = ow.deny & p.bit != 0;
        ui.add_enabled_ui(mine && editable, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.name).size(15.0).color(pal.strong));
                aui::right(ui, |ui| {
                    // Right-to-left layout: draw the cells in a left-to-right group.
                    ui.allocate_ui_with_layout(
                        egui::vec2(82.0, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if aui::tri_state(ui, pal, &mut allow, &mut deny) {
                                ow.allow = (ow.allow & !p.bit) | if allow { p.bit } else { 0 };
                                ow.deny = (ow.deny & !p.bit) | if deny { p.bit } else { 0 };
                            }
                        },
                    );
                });
            });
        })
        .response
        .on_disabled_hover_text("You don't have this permission here, so you can't change it.");
        aui::hint(ui, pal, p.desc);
        ui.add_space(6.0);
    }
    aui::divider(ui, pal);
    let dirty = (ow.allow, ow.deny) != ow.base || !ow.exists;
    ui.horizontal(|ui| {
        if aui::save(ui, pal, "Save Permissions", dirty && editable).clicked() {
            let req = save_overwrite(
                &cx.guild.id,
                channel_id,
                &ow.id,
                ow.target,
                ow.allow,
                ow.deny,
            );
            *sent = Some((req.key.clone(), req.body.clone().unwrap_or(Value::Null)));
            out.send.push(req);
        }
        if (ow.allow, ow.deny) != ow.base && aui::plain(ui, "Reset").clicked() {
            (ow.allow, ow.deny) = ow.base;
        }
        if ow.exists && editable {
            aui::right(ui, |ui| {
                if aui::danger(ui, pal, "Remove Overwrite", true).clicked() {
                    out.confirm = Some(Confirm::new(
                        "Remove overwrite",
                        &format!(
                            "Remove the channel permissions set for {}? They go back to their normal permissions here.",
                            ow.name
                        ),
                        "Remove",
                        delete_overwrite(&cx.guild.id, channel_id, &ow.id),
                    ));
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chans() -> Vec<Value> {
        vec![
            json!({ "id": "w", "name": "welcome", "type": 0, "position": 0 }),
            json!({ "id": "A", "name": "Text", "type": 4, "position": 0 }),
            json!({ "id": "a1", "name": "general", "type": 0, "position": 0, "parent_id": "A" }),
            json!({ "id": "a2", "name": "memes", "type": 0, "position": 1, "parent_id": "A" }),
            json!({ "id": "B", "name": "Voice", "type": 4, "position": 1 }),
            json!({ "id": "b1", "name": "Lounge", "type": 2, "position": 0, "parent_id": "B" }),
        ]
    }

    fn ids(rows: &[Row]) -> String {
        rows.iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn rows_are_in_display_order() {
        let mut c = chans();
        c.reverse();
        assert_eq!(ids(&rows(&c)), "w A a1 a2 B b1");
    }

    #[test]
    fn dropping_a_channel_takes_the_category_above() {
        let r = rows(&chans());
        // memes → under Voice, after Lounge.
        let next = drop_row(&r, 3, 6);
        assert_eq!(ids(&next), "w A a1 B b1 a2");
        assert_eq!(next[5].parent.as_deref(), Some("B"));
        // general → the very top: no category.
        let next = drop_row(&r, 2, 0);
        assert_eq!(ids(&next), "a1 w A a2 B b1");
        assert_eq!(next[0].parent, None);
        // welcome → right under category A's header.
        let next = drop_row(&r, 0, 2);
        assert_eq!(ids(&next), "A w a1 a2 B b1");
        assert_eq!(next[1].parent.as_deref(), Some("A"));
    }

    #[test]
    fn a_category_moves_with_its_channels() {
        let r = rows(&chans());
        let next = drop_row(&r, 4, 1);
        assert_eq!(ids(&next), "w B b1 A a1 a2");
        // Dropped in the middle of another category: lands after it.
        let next = drop_row(&r, 1, 5);
        assert_eq!(ids(&next), "w B b1 A a1 a2");
        // Never above the channels without a category.
        let next = drop_row(&r, 4, 0);
        assert_eq!(ids(&next), "w B b1 A a1 a2");
        // Onto itself: unchanged.
        assert_eq!(drop_row(&r, 1, 2), r);
    }

    #[test]
    fn reorder_sends_only_what_moved() {
        let c = chans();
        let r = rows(&c);
        assert!(reorder("1", &c, &r).is_none());
        let next = drop_row(&r, 3, 6);
        let req = reorder("1", &c, &next).unwrap();
        assert_eq!(req.method, Method::Patch);
        assert_eq!(req.path, "/guilds/1/channels");
        assert_eq!(
            req.body,
            Some(json!([{ "id": "a2", "position": 1, "parent_id": "B" }]))
        );
        let next = drop_row(&r, 4, 1);
        assert_eq!(
            reorder("1", &c, &next).unwrap().body,
            Some(json!([{ "id": "B", "position": 0 }, { "id": "A", "position": 1 }]))
        );
    }

    #[test]
    fn drafts_send_only_fields_for_their_kind() {
        let text = Draft {
            name: "New Channel".into(),
            kind: TEXT,
            topic: "hi".into(),
            slowmode: 10,
            bitrate: 64,
            ..Default::default()
        };
        assert_eq!(
            create("1", &text).unwrap().body,
            Some(json!({ "name": "new-channel", "type": 0, "parent_id": null,
                         "topic": "hi", "rate_limit_per_user": 10 }))
        );
        let voice = Draft {
            name: "Lounge".into(),
            kind: VOICE,
            bitrate: 96,
            user_limit: 5,
            topic: "ignored".into(),
            parent: "B".into(),
            ..Default::default()
        };
        assert_eq!(
            create("1", &voice).unwrap().body,
            Some(json!({ "name": "Lounge", "type": 2, "parent_id": "B",
                         "bitrate": 96000, "user_limit": 5 }))
        );
        let cat = Draft {
            name: "Stuff".into(),
            kind: CATEGORY,
            ..Default::default()
        };
        assert_eq!(
            create("1", &cat).unwrap().body,
            Some(json!({ "name": "Stuff", "type": 4 }))
        );
    }

    #[test]
    fn save_patches_changes_only() {
        let base = Draft::from_json(&json!({
            "name": "general", "type": 0, "topic": "t", "rate_limit_per_user": 0,
            "nsfw": false, "parent_id": "A",
        }));
        assert!(save("1", "a1", &base, &base.clone()).is_none());
        let mut d = base.clone();
        d.nsfw = true;
        d.slowmode = 30;
        let r = save("1", "a1", &base, &d).unwrap();
        assert_eq!(r.path, "/channels/a1");
        assert_eq!(
            r.body,
            Some(json!({ "nsfw": true, "rate_limit_per_user": 30 }))
        );
        d.name = "  ".into();
        assert!(save("1", "a1", &base, &d).is_none());
    }

    #[test]
    fn overwrite_requests_and_local_apply() {
        let r = save_overwrite(
            "1",
            "a1",
            "role",
            0,
            perms::VIEW_CHANNEL,
            perms::MANAGE_ROLES,
        );
        assert_eq!(r.method, Method::Put);
        assert_eq!(r.path, "/channels/a1/permissions/role");
        assert_eq!(
            r.body,
            Some(json!({ "type": 0, "allow": "1024", "deny": "268435456" }))
        );
        let mut st = State {
            channels: Some(chans()),
            sent_overwrite: Some((r.key.clone(), r.body.clone().unwrap())),
            ..Default::default()
        };
        st.on_response(Kind::SaveOverwrite, &r.key, &Value::Null);
        let c = &st.channels.as_ref().unwrap()[2];
        assert_eq!(c["permission_overwrites"][0]["id"], "role");
        assert_eq!(c["permission_overwrites"][0]["allow"], "1024");
        let d = delete_overwrite("1", "a1", "role");
        assert_eq!(d.method, Method::Delete);
        st.on_response(Kind::DeleteOverwrite, &d.key, &Value::Null);
        let c = &st.channels.as_ref().unwrap()[2];
        assert_eq!(c["permission_overwrites"], json!([]));
    }

    #[test]
    fn deleting_a_category_frees_its_channels() {
        let mut st = State {
            channels: Some(chans()),
            ..Default::default()
        };
        st.on_response(Kind::DeleteChannel, "A", &Value::Null);
        let list = st.channels.unwrap();
        assert!(list.iter().all(|c| s(c, "id") != "A"));
        assert!(list.iter().all(|c| s(c, "parent_id") != "A"));
    }

    #[test]
    fn bitrate_cap_follows_boosts() {
        assert_eq!(max_bitrate(0), 96);
        assert_eq!(max_bitrate(3), 384);
    }
}
