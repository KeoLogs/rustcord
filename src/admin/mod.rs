//! Server tools (the "Server admin" module): Discord's server management
//! screens. One file per area; this file holds what they share.
//!
//! Rules every area follows:
//! - A tool is only shown when my computed permissions allow it
//!   ([`Tab::available`], [`Cx::can`], the role hierarchy in `perms.rs`).
//! - Nothing is sent on its own. A [`Request`] is only ever built inside a
//!   click handler (Save, Create, Confirm, opening a tab, Search…) and
//!   handed to the app in [`Out::send`]. [`AdminPage::on_response`] returns
//!   nothing, so a reply can never trigger another request.
//! - Deleting a channel or role and banning go through [`Confirm`] with the
//!   target's name typed in.
//! - The UI does no I/O: requests go `ToBackend::Admin` → [`execute`] (in
//!   the backend) → `ToUi::Admin`; files are picked and read by the backend
//!   (`ToBackend::AdminPick` → `ToUi::AdminPicked`).
//! - The page only exists while it is open (`State::admin` is `None`
//!   otherwise), and every list is capped ([`LIST_CAP`]).

use std::sync::Arc;

use eframe::egui::{self, RichText, vec2};
use serde_json::Value;

use crate::icons;
use crate::perms;
use crate::theme::{Bold, Palette};
use crate::types::Guild;

pub mod channels;
pub mod demo;
pub mod members;
pub mod overview;
pub mod roles;
pub mod ui;

/// Most rows any admin list keeps (audit log pages, members found, bans…).
pub const LIST_CAP: usize = 500;

/// HTTP method of a [`Request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

/// What a request does; replies are routed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    LoadGuild,
    SaveGuild,
    SearchMembers,
    Kick,
    Ban,
    LoadBans,
    Unban,
    Timeout,
    SetNick,
    SetMemberRoles,
    LoadChannels,
    CreateChannel,
    SaveChannel,
    DeleteChannel,
    MoveChannels,
    SaveOverwrite,
    DeleteOverwrite,
    ChannelMemberSearch,
    LoadRoles,
    CreateRole,
    SaveRole,
    DeleteRole,
    MoveRoles,
    RoleMemberSearch,
    AddMemberRole,
    RemoveMemberRole,
}

impl Kind {
    /// User-facing name of the action, for error messages.
    pub fn what(self) -> &'static str {
        match self {
            Kind::LoadGuild => "Loading the server settings",
            Kind::SaveGuild => "Saving the server settings",
            Kind::SearchMembers => "Searching members",
            Kind::Kick => "Kicking the member",
            Kind::Ban => "Banning the member",
            Kind::LoadBans => "Loading bans",
            Kind::Unban => "Unbanning",
            Kind::Timeout => "Changing the timeout",
            Kind::SetNick => "Changing the nickname",
            Kind::SetMemberRoles => "Changing roles",
            Kind::LoadChannels => "Loading channels",
            Kind::CreateChannel => "Creating the channel",
            Kind::SaveChannel => "Saving the channel",
            Kind::DeleteChannel => "Deleting the channel",
            Kind::MoveChannels => "Reordering channels",
            Kind::SaveOverwrite => "Saving channel permissions",
            Kind::DeleteOverwrite => "Removing channel permissions",
            Kind::ChannelMemberSearch => "Searching members",
            Kind::LoadRoles => "Loading roles",
            Kind::CreateRole => "Creating the role",
            Kind::SaveRole => "Saving the role",
            Kind::DeleteRole => "Deleting the role",
            Kind::MoveRoles => "Reordering roles",
            Kind::RoleMemberSearch => "Searching members",
            Kind::AddMemberRole => "Adding the role",
            Kind::RemoveMemberRole => "Removing the role",
        }
    }

    fn tab(self) -> Tab {
        match self {
            Kind::LoadGuild | Kind::SaveGuild => Tab::Overview,
            Kind::LoadBans | Kind::Unban => Tab::Bans,
            Kind::SearchMembers
            | Kind::Kick
            | Kind::Ban
            | Kind::Timeout
            | Kind::SetNick
            | Kind::SetMemberRoles => Tab::Members,
            Kind::LoadChannels
            | Kind::CreateChannel
            | Kind::SaveChannel
            | Kind::DeleteChannel
            | Kind::MoveChannels
            | Kind::SaveOverwrite
            | Kind::DeleteOverwrite
            | Kind::ChannelMemberSearch => Tab::Channels,
            Kind::LoadRoles
            | Kind::CreateRole
            | Kind::SaveRole
            | Kind::DeleteRole
            | Kind::MoveRoles
            | Kind::RoleMemberSearch
            | Kind::AddMemberRole
            | Kind::RemoveMemberRole => Tab::Roles,
        }
    }
}

/// A file to upload as multipart form data (stickers).
#[derive(Clone)]
pub struct Upload {
    pub name: String,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
}

/// One REST call for the backend. Built only by the area modules' request
/// builders, from a click.
#[derive(Clone)]
pub struct Request {
    pub guild_id: String,
    pub kind: Kind,
    pub method: Method,
    /// Path under the API base, e.g. `/guilds/1/roles`.
    pub path: String,
    pub body: Option<Value>,
    /// Audit log reason (`X-Audit-Log-Reason`).
    pub reason: Option<String>,
    /// The thing acted on (role id, channel id…), echoed in the reply.
    pub key: String,
    pub file: Option<Upload>,
}

/// Bodies can hold images and paths can name webhooks: never printed.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "admin::Request({:?} {:?})", self.method, self.kind)
    }
}

impl Request {
    pub fn new(guild_id: &str, kind: Kind, method: Method, path: String) -> Self {
        Self {
            guild_id: guild_id.to_string(),
            kind,
            method,
            path,
            body: None,
            reason: None,
            key: String::new(),
            file: None,
        }
    }

    pub fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    pub fn key(mut self, key: &str) -> Self {
        self.key = key.to_string();
        self
    }

    /// Does it change anything on Discord?
    pub fn is_write(&self) -> bool {
        self.method != Method::Get
    }
}

/// The backend's reply to a [`Request`].
pub struct Response {
    pub guild_id: String,
    pub kind: Kind,
    pub key: String,
    pub was_write: bool,
    /// The JSON reply (`Null` for 204), or a user-facing error.
    pub result: Result<Value, String>,
}

/// Replies can hold webhook tokens: never printed.
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "admin::Response({:?} ok={})",
            self.kind,
            self.result.is_ok()
        )
    }
}

/// Which picture a file dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickSlot {
    Icon,
    Banner,
    RoleIcon,
}

/// A picked image, read by the backend.
pub struct Picked {
    pub slot: PickSlot,
    pub name: String,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "admin::Picked({:?}, {} bytes)",
            self.slot,
            self.bytes.len()
        )
    }
}

impl Picked {
    /// `data:image/png;base64,…` as Discord's image fields want it.
    pub fn data_uri(&self) -> String {
        use base64::Engine as _;
        format!(
            "data:{};base64,{}",
            self.mime,
            base64::engine::general_purpose::STANDARD.encode(self.bytes.as_slice())
        )
    }
}

/// Largest image the picker reads (Discord's own limit for icons/banners).
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Mime type by file extension, for the picker.
pub fn mime_for(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "apng" => "image/apng",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Backend side: run one request. All admin network I/O happens here.
pub async fn execute(api: &crate::api::Api, req: Request) -> Response {
    let result = api
        .admin(&req)
        .await
        .map_err(|e| crate::ratelimit::user_message(&e));
    Response {
        guild_id: req.guild_id,
        kind: req.kind,
        key: req.key,
        was_write: req.method != Method::Get,
        result,
    }
}

// ---------------------------------------------------------------------------
// JSON helpers (payloads are kept as `Value`, read leniently)
// ---------------------------------------------------------------------------

pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn n(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A JSON array as a capped list.
pub fn list(v: &Value) -> Vec<Value> {
    v.as_array()
        .map(|a| a.iter().take(LIST_CAP).cloned().collect())
        .unwrap_or_default()
}

/// Shallow-merge `src`'s fields into `dst` (PATCH replies, optimistic edits).
pub fn merge(dst: &mut Value, src: &Value) {
    if let (Some(d), Some(s)) = (dst.as_object_mut(), src.as_object()) {
        for (k, v) in s {
            d.insert(k.clone(), v.clone());
        }
    }
}

/// Replace the item with the same `id`, or add it (capped).
pub fn upsert(items: &mut Vec<Value>, item: &Value) {
    let id = s(item, "id");
    if id.is_empty() {
        return;
    }
    if let Some(old) = items.iter_mut().find(|i| s(i, "id") == id) {
        merge(old, item);
    } else if items.len() < LIST_CAP {
        items.push(item.clone());
    }
}

pub fn remove(items: &mut Vec<Value>, id: &str) {
    items.retain(|i| s(i, "id") != id);
}

/// `""` → JSON null, anything else → a string (optional snowflake fields).
pub fn id_or_null(id: &str) -> Value {
    if id.is_empty() {
        Value::Null
    } else {
        Value::String(id.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tabs and permissions
// ---------------------------------------------------------------------------

/// One screen of the admin page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Overview,
    Bans,
    Members,
    Channels,
    Roles,
}

/// Sidebar groups, in order.
const NAV: &[(&str, &[Tab])] = &[
    ("Server", &[Tab::Overview, Tab::Roles, Tab::Channels]),
    ("People", &[Tab::Members, Tab::Bans]),
];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Bans => "Bans",
            Tab::Members => "Members",
            Tab::Channels => "Channels",
            Tab::Roles => "Roles",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Overview => icons::GEAR,
            Tab::Bans => icons::PROHIBIT,
            Tab::Members => icons::USERS,
            Tab::Channels => icons::HASH,
            Tab::Roles => icons::ID,
        }
    }

    /// May someone with these server permissions (and server features) use
    /// this tab at all?
    pub fn available(self, perms: u64, features: &[String]) -> bool {
        let _ = features;
        let any = |bits: u64| perms & bits != 0;
        match self {
            Tab::Overview => any(perms::MANAGE_GUILD),
            Tab::Bans => any(perms::BAN_MEMBERS),
            Tab::Members => any(perms::KICK_MEMBERS
                | perms::BAN_MEMBERS
                | perms::MODERATE_MEMBERS
                | perms::MANAGE_NICKNAMES
                | perms::MANAGE_ROLES),
            Tab::Channels => any(perms::MANAGE_CHANNELS),
            Tab::Roles => any(perms::MANAGE_ROLES),
        }
    }

    /// Demo screenshots: `RUSTCORD_DEMO_OPEN=admin:<name>`.
    pub fn from_name(name: &str) -> Option<Tab> {
        NAV.iter()
            .flat_map(|(_, tabs)| tabs.iter())
            .copied()
            .find(|t| t.label().eq_ignore_ascii_case(&name.replace('-', " ")))
    }
}

/// The tabs someone may use, in sidebar order. Empty = no "Server
/// Settings" entry at all.
pub fn available_tabs(perms: u64, features: &[String]) -> Vec<Tab> {
    NAV.iter()
        .flat_map(|(_, tabs)| tabs.iter())
        .copied()
        .filter(|t| t.available(perms, features))
        .collect()
}

/// What the areas need to know about me and the server, per frame.
pub struct Cx<'a> {
    pub guild: &'a Guild,
    pub me_id: &'a str,
    /// My server-wide permissions.
    pub perms: u64,
    /// Position of my highest role.
    pub top: i64,
    pub owner: bool,
    pub pal: &'a Palette,
}

impl<'a> Cx<'a> {
    pub fn new(guild: &'a Guild, me_id: &'a str, pal: &'a Palette) -> Self {
        let me = perms::Me {
            user_id: me_id,
            roles: &guild.my_roles,
        };
        Self {
            guild,
            me_id,
            perms: perms::guild_permissions(&guild.id, &guild.owner_id, &guild.roles, &me),
            top: perms::top_role_position(&guild.roles, &guild.my_roles),
            owner: !guild.owner_id.is_empty() && guild.owner_id == me_id,
            pal,
        }
    }

    pub fn can(&self, bit: u64) -> bool {
        self.perms & bit != 0
    }

    /// My permissions in one channel, overwrites included.
    pub fn channel_perms(&self, channel_id: &str) -> u64 {
        let Some(c) = self.guild.channels.iter().find(|c| c.id == channel_id) else {
            return self.perms;
        };
        perms::channel_permissions(
            &self.guild.id,
            &self.guild.owner_id,
            &self.guild.roles,
            &c.overwrites,
            &perms::Me {
                user_id: self.me_id,
                roles: &self.guild.my_roles,
            },
        )
    }

    /// Channels of the given kinds as select options (id, "#name").
    pub fn channel_options(&self, kinds: &[u64], none: Option<&str>) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = none
            .map(|l| (String::new(), l.to_string()))
            .into_iter()
            .collect();
        out.extend(
            self.guild
                .channels
                .iter()
                .filter(|c| kinds.contains(&c.kind))
                .map(|c| (c.id.clone(), channel_label(c.kind, &c.name))),
        );
        out
    }
}

/// "#general" for text channels, the plain name for voice / categories.
pub fn channel_label(kind: u64, name: &str) -> String {
    match kind {
        2 | 4 | 13 => name.to_string(),
        _ => format!("#{name}"),
    }
}

/// What a frame of the page asks the app to do. Only filled from clicks.
#[derive(Default)]
pub struct Out {
    pub send: Vec<Request>,
    pub pick: Option<PickSlot>,
    pub confirm: Option<Confirm>,
    pub close: bool,
}

/// A confirm dialog in front of a destructive request.
pub struct Confirm {
    pub title: String,
    pub body: String,
    /// The name the user must type first (delete channel / role, ban).
    pub expect: Option<String>,
    pub typed: String,
    pub button: String,
    pub request: Request,
    /// Show an audit log "Reason" field.
    pub with_reason: bool,
    pub reason: String,
    /// Bans: delete this many seconds of the member's messages.
    pub ban_window: Option<u64>,
}

impl Confirm {
    pub fn new(title: &str, body: &str, button: &str, request: Request) -> Self {
        Self {
            title: title.to_string(),
            body: body.to_string(),
            expect: None,
            typed: String::new(),
            button: button.to_string(),
            request,
            with_reason: false,
            reason: String::new(),
            ban_window: None,
        }
    }

    /// Require the target's name to be typed.
    pub fn typed(mut self, name: &str) -> Self {
        self.expect = Some(name.to_string());
        self
    }

    pub fn reason(mut self) -> Self {
        self.with_reason = true;
        self
    }

    /// May the confirm button be pressed yet?
    pub fn ready(&self) -> bool {
        match &self.expect {
            Some(name) => !name.trim().is_empty() && self.typed.trim() == name.trim(),
            None => true,
        }
    }

    /// The request to send once confirmed (`None` until [`ready`](Self::ready)).
    pub fn into_request(self) -> Option<Request> {
        if !self.ready() {
            return None;
        }
        let mut req = self.request;
        let reason = self.reason.trim();
        if self.with_reason && !reason.is_empty() {
            req.reason = Some(reason.to_string());
        }
        if let Some(secs) = self.ban_window {
            req.body = Some(serde_json::json!({ "delete_message_seconds": secs }));
        }
        Some(req)
    }
}

/// Ban dialog: how much of the member's message history to delete.
pub const BAN_WINDOWS: &[(u64, &str)] = &[
    (0, "Don't delete any"),
    (3600, "Previous hour"),
    (21_600, "Previous 6 hours"),
    (43_200, "Previous 12 hours"),
    (86_400, "Previous 24 hours"),
    (259_200, "Previous 3 days"),
    (604_800, "Previous 7 days"),
];

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// The open admin page for one server.
pub struct AdminPage {
    pub guild_id: String,
    pub tab: Tab,
    /// Tabs whose data was already asked for (each loads once per opening;
    /// the Refresh button asks again).
    asked: Vec<Tab>,
    /// Requests sent and not answered yet.
    busy: u32,
    /// Last result line: text, is it an error.
    notice: Option<(String, bool)>,
    confirm: Option<Confirm>,
    /// Demo screenshots (`RUSTCORD_DEMO_OPEN=admin-roles-edit`): open the
    /// tab's first editor once its data is in.
    pub demo_detail: bool,
    overview: overview::State,
    members: members::State,
    channels: channels::State,
    roles: roles::State,
}

impl AdminPage {
    /// Open the page (the user clicked "Server Settings"). Returns the page
    /// and the first tab's load requests.
    pub fn open(guild_id: &str, tab: Tab) -> (Self, Vec<Request>) {
        let mut page = Self {
            guild_id: guild_id.to_string(),
            tab,
            asked: Vec::new(),
            busy: 0,
            notice: None,
            confirm: None,
            demo_detail: false,
            overview: Default::default(),
            members: Default::default(),
            channels: Default::default(),
            roles: Default::default(),
        };
        let reqs = page.select(tab);
        (page, reqs)
    }

    /// Switch tab (a click); the first visit loads its data.
    fn select(&mut self, tab: Tab) -> Vec<Request> {
        self.tab = tab;
        self.notice = None;
        if self.asked.contains(&tab) {
            return Vec::new();
        }
        self.asked.push(tab);
        let reqs = self.load_requests(tab);
        self.busy += reqs.len() as u32;
        reqs
    }

    /// The GETs a tab needs.
    fn load_requests(&self, tab: Tab) -> Vec<Request> {
        let g = &self.guild_id;
        match tab {
            Tab::Overview => vec![overview::load(g)],
            Tab::Bans => vec![members::load_bans(g)],
            Tab::Members => Vec::new(),
            Tab::Channels => vec![channels::load(g)],
            Tab::Roles => vec![roles::load(g)],
        }
    }

    /// A reply from the backend. Returns nothing on purpose: replies only
    /// update what is shown.
    pub fn on_response(&mut self, r: Response) {
        if r.guild_id != self.guild_id {
            return;
        }
        self.busy = self.busy.saturating_sub(1);
        let v = match r.result {
            Ok(v) => {
                if r.was_write {
                    self.notice = Some(("Saved.".to_string(), false));
                }
                v
            }
            Err(e) => {
                self.notice = Some((e, true));
                if r.was_write {
                    return;
                }
                // A failed load shows as empty, with the error on top.
                Value::Null
            }
        };
        match r.kind.tab() {
            Tab::Overview => self.overview.on_response(r.kind, &r.key, &v),
            Tab::Members | Tab::Bans => self.members.on_response(r.kind, &r.key, &v),
            Tab::Channels => self.channels.on_response(r.kind, &r.key, &v),
            Tab::Roles => self.roles.on_response(r.kind, &r.key, &v),
        }
        if std::mem::take(&mut self.demo_detail) {
            match self.tab {
                Tab::Roles => self.roles.demo_open(),
                Tab::Channels => self.channels.demo_open(),
                _ => {}
            }
        }
    }

    /// Demo screenshots: a tab's detail view that needs no reply first.
    pub fn demo_detail_now(&mut self) {
        if self.tab == Tab::Members {
            self.demo_detail = false;
            self.members.demo_open();
        }
    }

    /// A picked image arrived from the backend.
    pub fn on_picked(&mut self, p: Picked) {
        let p = Arc::new(p);
        match p.slot {
            PickSlot::Icon | PickSlot::Banner => self.overview.on_picked(p),
            PickSlot::RoleIcon => self.roles.on_picked(p),
        }
    }

    /// Draw the page (it replaces the main UI, like Settings).
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx) -> Out {
        let pal = cx.pal;
        let mut out = Out::default();
        let tabs = available_tabs(cx.perms, &cx.guild.features);
        if tabs.is_empty() {
            // Permissions were taken away while the page was open.
            out.close = true;
            return out;
        }
        if !tabs.contains(&self.tab) {
            self.tab = tabs[0];
        }
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if esc && self.confirm.is_none() {
            out.close = true;
        }

        let narrow = ui.available_width() < 760.0;
        let nav_w = if narrow { 168.0 } else { 232.0 };
        let mut clicked_tab = None;
        egui::Panel::left("admin_nav")
            .exact_size(nav_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .inner_margin(egui::Margin {
                        left: if narrow { 8 } else { 20 },
                        right: if narrow { 6 } else { 12 },
                        top: 60,
                        bottom: 16,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        crate::widgets::truncated_label(
                            ui,
                            egui::Label::new(
                                RichText::new(&cx.guild.name).color(pal.strong).bold(),
                            ),
                            0.0,
                        );
                    });
                    ui.add_space(8.0);
                    for (header, items) in NAV {
                        if !items.iter().any(|t| tabs.contains(t)) {
                            continue;
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(pal.section_label(header));
                        });
                        ui.add_space(2.0);
                        for tab in items.iter().filter(|t| tabs.contains(t)) {
                            let selected = self.tab == *tab;
                            if crate::settings::nav_item(
                                ui,
                                tab.icon(),
                                tab.label(),
                                selected,
                                pal.text,
                                pal,
                            )
                            .clicked()
                            {
                                clicked_tab = Some(*tab);
                            }
                        }
                        ui.add_space(8.0);
                    }
                });
            });
        if let Some(tab) = clicked_tab {
            out.send.extend(self.select(tab));
        }

        let side = if narrow { 16 } else { 40 };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.chat)
                    .inner_margin(egui::Margin {
                        left: side,
                        right: side,
                        top: 60,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if crate::settings::close_button(ui, pal).clicked() {
                            out.close = true;
                        }
                        if crate::widgets::icon_button(
                            ui,
                            icons::ARROW_CLOCKWISE,
                            "Refresh",
                            false,
                            self.busy == 0,
                            pal,
                        )
                        .clicked()
                        {
                            let reqs = self.load_requests(self.tab);
                            self.busy += reqs.len() as u32;
                            out.send.extend(reqs);
                        }
                        if self.busy > 0 {
                            ui.add(egui::Spinner::new().size(16.0).color(pal.muted));
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                            ui.heading(self.tab.label());
                        });
                    });
                });
                if let Some((text, error)) = &self.notice {
                    let (icon, col) = if *error {
                        (icons::WARNING, pal.danger)
                    } else {
                        (icons::CHECK, pal.positive())
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(icon).font(icons::font(16.0)).color(col));
                        ui.label(RichText::new(text).color(col));
                    });
                } else {
                    ui.add_space(4.0);
                }
                ui.add_space(12.0);
                egui::ScrollArea::vertical()
                    .id_salt(("admin_tab", self.tab))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(760.0));
                        ui.spacing_mut().item_spacing.y = 6.0;
                        let before = out.send.len();
                        match self.tab {
                            Tab::Overview => self.overview.ui(ui, cx, &mut out),
                            Tab::Bans => self.members.bans_ui(ui, cx, &mut out),
                            Tab::Members => self.members.ui(ui, cx, &mut out),
                            Tab::Channels => self.channels.ui(ui, cx, &mut out),
                            Tab::Roles => self.roles.ui(ui, cx, &mut out),
                        }
                        self.busy += (out.send.len() - before) as u32;
                        ui.add_space(48.0);
                    });
            });

        if let Some(c) = out.confirm.take() {
            self.confirm = Some(c);
        }
        if let Some(req) = self.confirm_ui(ui.ctx(), pal) {
            self.busy += 1;
            out.send.push(req);
        }
        out
    }

    /// The confirm dialog. Returns the request once the user confirms.
    fn confirm_ui(&mut self, ctx: &egui::Context, pal: &Palette) -> Option<Request> {
        let c = self.confirm.as_mut()?;
        let mut close = false;
        let mut go = false;
        let modal = egui::Modal::new(egui::Id::new("rc_admin_confirm"))
            .frame(crate::widgets::menu_frame(pal).inner_margin(egui::Margin::same(18)))
            .show(ctx, |ui| {
                ui.set_max_width(420.0_f32.min(ctx.content_rect().width() - 48.0));
                ui.label(RichText::new(&c.title).size(18.0).color(pal.strong).bold());
                ui.add_space(4.0);
                ui.label(RichText::new(&c.body).color(pal.text));
                if c.ban_window.is_some() {
                    ui::label(ui, pal, "Delete message history");
                    let mut w = c.ban_window.unwrap_or(0);
                    ui::select(ui, pal, "admin_ban_window", &mut w, &ui::opts(BAN_WINDOWS));
                    c.ban_window = Some(w);
                }
                if c.with_reason {
                    ui::label(ui, pal, "Reason (shown in the audit log)");
                    ui::text(ui, &mut c.reason, "Optional", 400);
                }
                if let Some(name) = &c.expect {
                    ui::label(ui, pal, &format!("Type \"{name}\" to confirm"));
                    ui::text(ui, &mut c.typed, name, 200);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui::plain(ui, "Cancel").clicked() {
                        close = true;
                    }
                    if ui::danger(ui, pal, &c.button, c.ready()).clicked() {
                        go = true;
                    }
                });
            });
        if go {
            return self.confirm.take().and_then(Confirm::into_request);
        }
        if close || modal.should_close() {
            self.confirm = None;
        }
        None
    }
}

/// Size of the icon / banner preview box.
pub(crate) const ICON_BOX: egui::Vec2 = vec2(72.0, 72.0);

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A server where "me" has role `mine` with `perms`.
    pub fn guild(perms: u64, features: &[&str]) -> Guild {
        let mut g = Guild::from_json(&serde_json::json!({
            "id": "1", "name": "Test", "owner_id": "owner",
            "features": features,
            "roles": [
                { "id": "1", "name": "@everyone", "position": 0, "permissions": "0" },
                { "id": "low", "name": "Low", "position": 1, "permissions": "0" },
                { "id": "mine", "name": "Mine", "position": 2, "permissions": perms.to_string() },
                { "id": "high", "name": "High", "position": 3, "permissions": "0" },
            ],
            "channels": [
                { "id": "10", "name": "general", "type": 0, "position": 0 },
                { "id": "11", "name": "Voice", "type": 2, "position": 1 },
            ],
        }));
        g.my_roles = vec!["mine".to_string()];
        g
    }

    /// Draw `page` for a few frames with no input at all and collect what
    /// it asked for.
    pub fn idle_frames(page: &mut AdminPage, g: &Guild) -> Vec<Request> {
        let pal = crate::theme::ThemeChoice::DiscordDark.palette();
        let ctx = egui::Context::default();
        let mut sent = Vec::new();
        for _ in 0..3 {
            let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
                let cx = Cx::new(g, "me", &pal);
                let out = page.ui(ui, &cx);
                sent.extend(out.send);
                assert!(out.pick.is_none());
            });
            full.textures_delta.clear();
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req() -> Request {
        Request::new("1", Kind::SaveGuild, Method::Patch, "/guilds/1".into())
    }

    #[test]
    fn tabs_follow_permissions() {
        assert!(available_tabs(0, &[]).is_empty());
        assert!(available_tabs(perms::MANAGE_MESSAGES, &[]).is_empty());
        assert!(available_tabs(perms::MANAGE_GUILD, &[]).contains(&Tab::Overview));
        // Owner / Administrator (all bits): every non-community tab.
        let all = available_tabs(u64::MAX, &[]);
        for (_, tabs) in NAV {
            for t in *tabs {
                assert_eq!(all.contains(t), t.available(u64::MAX, &[]));
            }
        }
    }

    #[test]
    fn typed_name_confirm_gates_the_request() {
        let mut c = Confirm::new("Delete", "Sure?", "Delete", req()).typed("general");
        assert!(!c.ready());
        c.typed = "genera".into();
        assert!(!c.ready());
        c.typed = "General".into();
        assert!(!c.ready(), "the name must match exactly");
        c.typed = " general ".into();
        assert!(c.ready());
        assert!(c.into_request().is_some());
        // Not ready: no request comes out, whatever the caller does.
        let c = Confirm::new("Delete", "Sure?", "Delete", req()).typed("general");
        assert!(c.into_request().is_none());
        // An empty expected name can never be confirmed.
        let c = Confirm::new("Delete", "Sure?", "Delete", req()).typed("");
        assert!(!c.ready());
    }

    #[test]
    fn confirm_adds_reason_and_ban_window() {
        let mut c = Confirm::new("Ban", "", "Ban", req()).reason();
        c.reason = "  spam  ".into();
        c.ban_window = Some(3600);
        let r = c.into_request().unwrap();
        assert_eq!(r.reason.as_deref(), Some("spam"));
        assert_eq!(r.body, Some(json!({ "delete_message_seconds": 3600 })));
    }

    #[test]
    fn debug_output_never_shows_bodies_or_paths() {
        let r = Request::new(
            "1",
            Kind::SaveGuild,
            Method::Patch,
            "/webhooks/9/SECRET".into(),
        )
        .body(json!({ "token": "SECRET" }));
        assert!(!format!("{r:?}").contains("SECRET"));
        let resp = Response {
            guild_id: "1".into(),
            kind: Kind::LoadGuild,
            key: String::new(),
            was_write: false,
            result: Ok(json!({ "token": "SECRET" })),
        };
        assert!(!format!("{resp:?}").contains("SECRET"));
    }

    #[test]
    fn json_list_helpers() {
        let mut items = vec![json!({ "id": "1", "name": "a", "x": 1 })];
        upsert(&mut items, &json!({ "id": "1", "name": "b" }));
        assert_eq!(items, vec![json!({ "id": "1", "name": "b", "x": 1 })]);
        upsert(&mut items, &json!({ "id": "2" }));
        upsert(&mut items, &json!({ "name": "no id" }));
        assert_eq!(items.len(), 2);
        remove(&mut items, "1");
        assert_eq!(items.len(), 1);
        assert_eq!(id_or_null(""), Value::Null);
        assert_eq!(mime_for("a.PNG"), Some("image/png"));
        assert_eq!(mime_for("a.exe"), None);
    }

    /// Opening the page loads the first tab once; after that an idle page
    /// (no clicks, replies arriving) never asks for anything.
    #[test]
    fn nothing_is_sent_without_a_click() {
        let g = testing::guild(u64::MAX & !perms::ADMINISTRATOR, &["COMMUNITY"]);
        for tab in available_tabs(u64::MAX, &g.features) {
            let (mut page, first) = AdminPage::open(&g.id, tab);
            assert!(first.iter().all(|r| !r.is_write()), "{tab:?} loads only");
            assert!(testing::idle_frames(&mut page, &g).is_empty(), "{tab:?}");
            // Replies (demo data) arrive: still nothing goes out.
            for r in first {
                let result = demo::respond(&r);
                page.on_response(Response {
                    guild_id: r.guild_id.clone(),
                    kind: r.kind,
                    key: r.key.clone(),
                    was_write: false,
                    result,
                });
            }
            assert!(testing::idle_frames(&mut page, &g).is_empty(), "{tab:?}");
        }
    }

    #[test]
    fn replies_for_another_server_are_ignored() {
        let (mut page, _) = AdminPage::open("1", Tab::Overview);
        page.on_response(Response {
            guild_id: "2".into(),
            kind: Kind::LoadGuild,
            key: String::new(),
            was_write: false,
            result: Err("nope".into()),
        });
        assert!(page.notice.is_none());
    }
}
