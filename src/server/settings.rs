//! Server Settings: the full-window page with Discord's sidebar. Built-in
//! pages are Overview, Channels and Invites; other pages (Roles, Members,
//! Bans, Emoji, Audit Log, Onboarding, …) plug in through
//! [`ExternalPages`], implemented by whoever owns them.
//!
//! Overview edits a [`GuildDraft`] loaded with `GET guilds/{id}` and saves
//! only the changed fields with `PATCH guilds/{id}` when the user presses
//! "Save Changes".

use eframe::egui::{self, RichText, vec2};
use serde_json::{Map, Value, json};

use super::channels::{self, ChannelKind};
use super::invites::InviteList;
use super::permissions::{self as p};
use super::rest::{self, ServerRequest};
use super::shell::{self, Flash, NavGroup};
use super::widgets::{self, BarAction, Confirm, Icon};
use super::{AdminCtx, UiEvent};
use crate::theme::{Bold, Palette};
use crate::types::{CHANNEL_CATEGORY, CHANNEL_VOICE, Channel, Guild};

/// Pages of Server Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerSettingsSection {
    Overview,
    Roles,
    Emoji,
    Channels,
    AuditLog,
    Onboarding,
    Members,
    Invites,
    Bans,
    /// Anything else a plugin adds (the string is its sidebar label).
    Other(&'static str),
}

impl ServerSettingsSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Roles => "Roles",
            Self::Emoji => "Emoji",
            Self::Channels => "Channels",
            Self::AuditLog => "Audit Log",
            Self::Onboarding => "Onboarding",
            Self::Members => "Members",
            Self::Invites => "Invites",
            Self::Bans => "Bans",
            Self::Other(s) => s,
        }
    }

    /// Drawn by this module (the rest come from [`ExternalPages`]).
    pub fn is_builtin(self) -> bool {
        matches!(self, Self::Overview | Self::Channels | Self::Invites)
    }

    /// Sidebar group: 0 = server, 1 = community, 2 = user management.
    fn group(self) -> usize {
        match self {
            Self::Onboarding => 1,
            Self::Members | Self::Invites | Self::Bans => 2,
            _ => 0,
        }
    }

    /// Order inside the sidebar.
    fn rank(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Roles => 1,
            Self::Emoji => 2,
            Self::Channels => 3,
            Self::Other(_) => 4,
            Self::AuditLog => 5,
            Self::Onboarding => 6,
            Self::Members => 7,
            Self::Invites => 8,
            Self::Bans => 9,
        }
    }
}

/// Whether Discord would let me open a page, from my server permissions
/// (`permissions::guild_permissions`). External pages may use this too.
pub fn section_allowed(section: ServerSettingsSection, perms: u64) -> bool {
    use ServerSettingsSection as S;
    match section {
        S::Overview | S::Invites => p::has(perms, p::MANAGE_GUILD),
        S::Roles => p::has(perms, p::MANAGE_ROLES),
        S::Emoji => p::has_any(
            perms,
            p::MANAGE_GUILD_EXPRESSIONS | p::CREATE_GUILD_EXPRESSIONS,
        ),
        S::Channels => p::has(perms, p::MANAGE_CHANNELS),
        S::AuditLog => p::has(perms, p::VIEW_AUDIT_LOG),
        S::Onboarding => p::has(perms, p::MANAGE_GUILD | p::MANAGE_ROLES),
        S::Members => p::has_any(
            perms,
            p::KICK_MEMBERS
                | p::BAN_MEMBERS
                | p::MODERATE_MEMBERS
                | p::MANAGE_ROLES
                | p::MANAGE_NICKNAMES,
        ),
        S::Bans => p::has(perms, p::BAN_MEMBERS),
        S::Other(_) => true,
    }
}

/// Pages drawn by other modules. All methods have no-op defaults; the
/// implementor keeps its own state and sends its own backend requests.
pub trait ExternalPages {
    /// Which external pages exist for this server (check permissions with
    /// [`section_allowed`]).
    fn sections(&self, _guild: &Guild, _my_perms: u64) -> Vec<ServerSettingsSection> {
        Vec::new()
    }
    /// The page was opened (load its data here).
    fn opened(&mut self, _section: ServerSettingsSection, _guild: &Guild) {}
    /// Draw the page body (title and close button are drawn already).
    fn ui(
        &mut self,
        _section: ServerSettingsSection,
        _ui: &mut egui::Ui,
        _guild: &Guild,
        _pal: &Palette,
    ) {
    }
    /// Unsaved edits: shows the bar and blocks leaving the page.
    fn is_dirty(&self, _section: ServerSettingsSection) -> bool {
        false
    }
    fn is_saving(&self, _section: ServerSettingsSection) -> bool {
        false
    }
    /// "Reset" on the unsaved bar.
    fn reset(&mut self, _section: ServerSettingsSection) {}
    /// "Save Changes" on the unsaved bar.
    fn save(&mut self, _section: ServerSettingsSection) {}
}

/// No external pages.
pub struct NoExternalPages;
impl ExternalPages for NoExternalPages {}

// ---------------------------------------------------------------------------
// Overview data
// ---------------------------------------------------------------------------

/// A new icon, or removing it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IconChange {
    #[default]
    Keep,
    /// `data:image/...;base64,...`
    Set(String),
    Remove,
}

pub const AFK_TIMEOUTS: &[(u32, &str)] = &[
    (60, "1 minute"),
    (300, "5 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

pub const VERIFICATION_LEVELS: &[(u8, &str, &str)] = &[
    (0, "None", "Unrestricted"),
    (
        1,
        "Low",
        "Must have a verified email on their Discord account.",
    ),
    (
        2,
        "Medium",
        "Must also be registered on Discord for longer than 5 minutes.",
    ),
    (
        3,
        "High",
        "Must also be a member of this server for longer than 10 minutes.",
    ),
    (
        4,
        "Highest",
        "Must have a verified phone on their Discord account.",
    ),
];

pub const CONTENT_FILTERS: &[(u8, &str, &str)] = &[
    (
        0,
        "Don't scan any media content",
        "Not recommended for public servers.",
    ),
    (
        1,
        "Scan media content from members without a role",
        "Recommended for servers that use roles for trusted members.",
    ),
    (
        2,
        "Scan media content from all members",
        "Recommended for public servers.",
    ),
];

/// Editable server settings (Overview page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildDraft {
    pub name: String,
    pub description: String,
    /// Current icon hash (for the preview), unchanged by edits.
    pub icon_hash: Option<String>,
    pub icon: IconChange,
    pub afk_channel_id: Option<String>,
    pub afk_timeout: u32,
    pub system_channel_id: Option<String>,
    /// 0 = all messages, 1 = only @mentions.
    pub default_notifications: u8,
    pub verification_level: u8,
    pub explicit_content_filter: u8,
}

impl GuildDraft {
    pub fn from_json(v: &Value) -> Self {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Self {
            name: s("name").unwrap_or_default(),
            description: s("description").unwrap_or_default(),
            icon_hash: s("icon"),
            icon: IconChange::Keep,
            afk_channel_id: s("afk_channel_id"),
            afk_timeout: v.get("afk_timeout").and_then(Value::as_u64).unwrap_or(300) as u32,
            system_channel_id: s("system_channel_id"),
            default_notifications: n("default_message_notifications").min(1) as u8,
            verification_level: n("verification_level").min(4) as u8,
            explicit_content_filter: n("explicit_content_filter").min(2) as u8,
        }
    }

    /// `PATCH guilds/{id}` body with only the changed fields.
    pub fn patch_body(&self, original: &GuildDraft) -> Value {
        let mut m = Map::new();
        let name = self.name.trim();
        if name != original.name {
            m.insert("name".into(), json!(name));
        }
        if self.description.trim() != original.description.trim() {
            let d = self.description.trim();
            m.insert(
                "description".into(),
                if d.is_empty() { Value::Null } else { json!(d) },
            );
        }
        match &self.icon {
            IconChange::Keep => {}
            IconChange::Set(uri) => {
                m.insert("icon".into(), json!(uri));
            }
            IconChange::Remove if original.icon_hash.is_some() => {
                m.insert("icon".into(), Value::Null);
            }
            IconChange::Remove => {}
        }
        if self.afk_channel_id != original.afk_channel_id {
            m.insert("afk_channel_id".into(), json!(self.afk_channel_id));
        }
        if self.afk_timeout != original.afk_timeout {
            m.insert("afk_timeout".into(), json!(self.afk_timeout));
        }
        if self.system_channel_id != original.system_channel_id {
            m.insert("system_channel_id".into(), json!(self.system_channel_id));
        }
        if self.default_notifications != original.default_notifications {
            m.insert(
                "default_message_notifications".into(),
                json!(self.default_notifications),
            );
        }
        if self.verification_level != original.verification_level {
            m.insert("verification_level".into(), json!(self.verification_level));
        }
        if self.explicit_content_filter != original.explicit_content_filter {
            m.insert(
                "explicit_content_filter".into(),
                json!(self.explicit_content_filter),
            );
        }
        Value::Object(m)
    }

    pub fn is_dirty(&self, original: &GuildDraft) -> bool {
        self.patch_body(original)
            .as_object()
            .is_some_and(|m| !m.is_empty())
    }

    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.trim().chars().count();
        if !(2..=100).contains(&n) {
            return Err("The server name must be between 2 and 100 characters.".into());
        }
        if self.description.chars().count() > 120 {
            return Err("The description can be at most 120 characters.".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// Server Settings state.
pub struct ServerSettings {
    pub guild_id: String,
    pub section: ServerSettingsSection,
    /// (original, draft) once `GET guilds/{id}` answered.
    overview: Option<(GuildDraft, GuildDraft)>,
    overview_error: Option<String>,
    saving: bool,
    /// Image URI of the picked icon's preview (freed on close).
    pub(super) preview_uri: Option<String>,
    pub(super) invites: InviteList,
    channels_error: Option<String>,
    confirm_delete: Option<(String, String, bool)>,
    flash: Flash,
}

impl ServerSettings {
    /// Returns the state and the first load request for `section`.
    pub fn new(guild_id: &str, section: ServerSettingsSection) -> Self {
        Self {
            guild_id: guild_id.to_string(),
            section,
            overview: None,
            overview_error: None,
            saving: false,
            preview_uri: None,
            invites: InviteList::default(),
            channels_error: None,
            confirm_delete: None,
            flash: Flash::default(),
        }
    }

    /// Request(s) a page needs when it opens.
    pub(super) fn load_for(&self, section: ServerSettingsSection) -> Option<ServerRequest> {
        match section {
            ServerSettingsSection::Overview if self.overview.is_none() => {
                Some(ServerRequest::LoadGuild {
                    guild_id: self.guild_id.clone(),
                })
            }
            ServerSettingsSection::Invites => Some(ServerRequest::LoadGuildInvites {
                guild_id: self.guild_id.clone(),
            }),
            _ => None,
        }
    }

    pub fn on_guild(&mut self, data: &Value) {
        let fresh = GuildDraft::from_json(data);
        match &mut self.overview {
            Some((orig, draft)) if !self.saving && draft.is_dirty(orig) => *orig = fresh,
            _ => self.overview = Some((fresh.clone(), fresh)),
        }
        self.saving = false;
        self.overview_error = None;
    }

    pub fn on_failed(&mut self, req: &ServerRequest, error: String) {
        match req {
            ServerRequest::LoadGuild { .. } | ServerRequest::PatchGuild { .. } => {
                self.saving = false;
                self.overview_error = Some(error);
            }
            ServerRequest::LoadGuildInvites { .. } | ServerRequest::RevokeInvite { .. } => {
                if self.invites.invites.is_none() {
                    self.invites.invites = Some(Vec::new());
                }
                self.invites.error = Some(error);
            }
            ServerRequest::ReorderChannels { .. } | ServerRequest::DeleteChannel { .. } => {
                self.channels_error = Some(error)
            }
            _ => {}
        }
    }

    /// The picked icon (from `rest::pick_icon_data_uri`).
    pub fn set_icon(&mut self, ctx: &egui::Context, result: Result<String, String>) {
        match result {
            Ok(uri) => {
                self.forget_preview(ctx);
                if let Some(bytes) = rest::decode_data_uri(&uri) {
                    let key = format!("bytes://rustcord-server-icon-{}", bytes.len());
                    ctx.include_bytes(key.clone(), bytes);
                    self.preview_uri = Some(key);
                }
                if let Some((_, d)) = &mut self.overview {
                    d.icon = IconChange::Set(uri);
                }
                self.overview_error = None;
            }
            Err(e) => self.overview_error = Some(e),
        }
    }

    pub fn forget_preview(&mut self, ctx: &egui::Context) {
        if let Some(uri) = self.preview_uri.take() {
            ctx.forget_image(&uri);
        }
    }

    fn sections(&self, actx: &AdminCtx, pages: &dyn ExternalPages) -> Vec<ServerSettingsSection> {
        let perms = p::guild_permissions(actx.guild, actx.my_id);
        let mut list: Vec<ServerSettingsSection> = [
            ServerSettingsSection::Overview,
            ServerSettingsSection::Channels,
            ServerSettingsSection::Invites,
        ]
        .into_iter()
        .filter(|s| section_allowed(*s, perms))
        .collect();
        for s in pages.sections(actx.guild, perms) {
            if !s.is_builtin() && !list.contains(&s) {
                list.push(s);
            }
        }
        list.sort_by_key(|s| (s.group(), s.rank()));
        list
    }

    fn dirty(&self, pages: &dyn ExternalPages) -> bool {
        match self.section {
            ServerSettingsSection::Overview => {
                self.overview.as_ref().is_some_and(|(o, d)| d.is_dirty(o))
            }
            s if !s.is_builtin() => pages.is_dirty(s),
            _ => false,
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        actx: &AdminCtx,
        pages: &mut dyn ExternalPages,
        out: &mut Vec<UiEvent>,
    ) {
        let pal = actx.pal;
        let sections = self.sections(actx, pages);
        if !sections.contains(&self.section)
            && let Some(first) = sections.first()
        {
            self.section = *first;
            if let Some(r) = self.load_for(*first) {
                out.push(UiEvent::Request(r));
            }
            if !first.is_builtin() {
                pages.opened(*first, actx.guild);
            }
        }
        let mut groups = vec![
            NavGroup::new(actx.guild.name.clone()),
            NavGroup::new("Community"),
            NavGroup::new("User Management"),
        ];
        for s in &sections {
            let g = std::mem::replace(&mut groups[s.group()], NavGroup::new(""));
            groups[s.group()] = g.item(*s, s.label());
        }
        groups.retain(|g| !g.items.is_empty());

        let dirty = self.dirty(pages);
        let saving = match self.section {
            ServerSettingsSection::Overview => self.saving,
            s if !s.is_builtin() => pages.is_saving(s),
            _ => false,
        };
        let section = self.section;
        let mut flash = self.flash;
        let mut body_events = Vec::new();
        let res = shell::show(
            ui,
            "server_settings",
            pal,
            &groups,
            section,
            section.label(),
            dirty,
            saving,
            &mut flash,
            |ui| match section {
                ServerSettingsSection::Overview => self.overview_ui(ui, actx, &mut body_events),
                ServerSettingsSection::Channels => self.channels_ui(ui, actx, &mut body_events),
                ServerSettingsSection::Invites => {
                    if let Some(r) = self.invites.ui(ui, true, pal) {
                        body_events.push(UiEvent::Request(r));
                    }
                }
                s => pages.ui(s, ui, actx.guild, pal),
            },
        );
        self.flash = flash;
        out.extend(body_events);

        if let Some(bar) = res.bar {
            match (section, bar) {
                (ServerSettingsSection::Overview, BarAction::Reset) => {
                    if let Some((o, d)) = &mut self.overview {
                        *d = o.clone();
                    }
                    self.forget_preview(ui.ctx());
                    self.overview_error = None;
                }
                (ServerSettingsSection::Overview, BarAction::Save) => {
                    if let Some((o, d)) = &self.overview {
                        match d.validate() {
                            Ok(()) => {
                                self.saving = true;
                                self.overview_error = None;
                                out.push(UiEvent::Request(ServerRequest::PatchGuild {
                                    guild_id: self.guild_id.clone(),
                                    body: d.patch_body(o),
                                }));
                            }
                            Err(e) => self.overview_error = Some(e),
                        }
                    }
                }
                (s, BarAction::Reset) => pages.reset(s),
                (s, BarAction::Save) => pages.save(s),
            }
        }
        if let Some(s) = res.nav
            && s != self.section
        {
            self.section = s;
            if let Some(r) = self.load_for(s) {
                out.push(UiEvent::Request(r));
            }
            if !s.is_builtin() {
                pages.opened(s, actx.guild);
            }
        }
        if res.close {
            out.push(UiEvent::CloseServerSettings);
        }
        self.confirm_ui(ui.ctx(), pal, out);
    }

    fn confirm_ui(&mut self, ctx: &egui::Context, pal: &Palette, out: &mut Vec<UiEvent>) {
        let Some((id, name, is_cat)) = self.confirm_delete.clone() else {
            return;
        };
        let c = if is_cat {
            Confirm::new(
                "Delete Category",
                format!(
                    "Are you sure you want to delete {name}? The channels in it will stay, without a category. This cannot be undone."
                ),
                "Delete Category",
            )
        } else {
            Confirm::new(
                "Delete Channel",
                format!("Are you sure you want to delete #{name}? This cannot be undone."),
                "Delete Channel",
            )
        };
        match c.show(ctx, pal) {
            Some(true) => {
                self.confirm_delete = None;
                out.push(UiEvent::Request(ServerRequest::DeleteChannel {
                    channel_id: id,
                }));
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    fn overview_ui(&mut self, ui: &mut egui::Ui, actx: &AdminCtx, out: &mut Vec<UiEvent>) {
        let pal = actx.pal;
        widgets::error_line(ui, pal, &self.overview_error);
        let Some((_, d)) = &mut self.overview else {
            if self.overview_error.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading server settings…").color(pal.muted));
                });
            } else if ui.button("Try again").clicked() {
                self.overview_error = None;
                out.push(UiEvent::Request(ServerRequest::LoadGuild {
                    guild_id: self.guild_id.clone(),
                }));
            }
            return;
        };
        let guild = actx.guild;

        // Icon + name.
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(100.0, 100.0), egui::Sense::hover());
            let c = rect.center();
            ui.painter().circle_filled(c, 50.0, pal.rail_icon);
            let img_uri = match &d.icon {
                IconChange::Set(_) => self.preview_uri.clone(),
                IconChange::Remove => None,
                IconChange::Keep => d.icon_hash.as_ref().filter(|_| actx.show_images).map(|h| {
                    format!(
                        "https://cdn.discordapp.com/icons/{}/{h}.png?size=128",
                        guild.id
                    )
                }),
            };
            match img_uri {
                Some(uri) => {
                    egui::Image::new(uri)
                        .corner_radius(50)
                        .fit_to_exact_size(vec2(100.0, 100.0))
                        .paint_at(ui, rect);
                }
                None => {
                    let initials: String = d
                        .name
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .take(3)
                        .collect();
                    ui.painter().text(
                        c,
                        egui::Align2::CENTER_CENTER,
                        initials,
                        crate::theme::bold_font(20.0),
                        pal.strong,
                    );
                }
            }
            ui.add_space(16.0);
            ui.vertical(|ui| {
                widgets::hint(
                    ui,
                    pal,
                    "We recommend an image of at least 512x512 for the server.",
                );
                ui.horizontal(|ui| {
                    if ui.add(pal.primary_button("Upload Image")).clicked() {
                        out.push(UiEvent::PickIcon);
                    }
                    let has_icon = matches!(d.icon, IconChange::Set(_))
                        || (d.icon == IconChange::Keep && d.icon_hash.is_some());
                    if has_icon && ui.add(egui::Button::new("Remove").frame(false)).clicked() {
                        d.icon = IconChange::Remove;
                    }
                });
                widgets::field_label(ui, pal, "Server name");
                ui.add(
                    egui::TextEdit::singleline(&mut d.name)
                        .desired_width(320.0)
                        .char_limit(100),
                );
            });
        });

        widgets::field_label(ui, pal, "Description");
        ui.add(
            egui::TextEdit::multiline(&mut d.description)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .char_limit(120)
                .hint_text("Shown in invites and Server Discovery (Community servers)."),
        );

        widgets::divider(ui, pal);
        let voice: Vec<(Option<String>, String)> =
            std::iter::once((None, "No Inactive Channel".to_string()))
                .chain(
                    guild
                        .channels
                        .iter()
                        .filter(|c| c.kind == CHANNEL_VOICE)
                        .map(|c| (Some(c.id.clone()), c.name.clone())),
                )
                .collect();
        let timeouts: Vec<(u32, String)> = AFK_TIMEOUTS
            .iter()
            .map(|(v, l)| (*v, l.to_string()))
            .collect();
        ui.columns(2, |cols| {
            widgets::field_label(&mut cols[0], pal, "Inactive channel");
            widgets::combo(
                &mut cols[0],
                "afk_channel",
                &mut d.afk_channel_id,
                &voice,
                260.0,
            );
            widgets::field_label(&mut cols[1], pal, "Inactive timeout");
            widgets::combo(
                &mut cols[1],
                "afk_timeout",
                &mut d.afk_timeout,
                &timeouts,
                200.0,
            );
        });
        widgets::hint(
            ui,
            pal,
            "Members who have been idle longer than the timeout are moved to this channel and muted.",
        );

        widgets::divider(ui, pal);
        let text: Vec<(Option<String>, String)> =
            std::iter::once((None, "No System Messages".to_string()))
                .chain(
                    guild
                        .channels
                        .iter()
                        .filter(|c| c.kind == 0)
                        .map(|c| (Some(c.id.clone()), format!("#{}", c.name))),
                )
                .collect();
        widgets::field_label(ui, pal, "System messages channel");
        widgets::combo(ui, "system_channel", &mut d.system_channel_id, &text, 260.0);
        widgets::hint(
            ui,
            pal,
            "The channel where Discord posts welcome and boost messages.",
        );

        widgets::divider(ui, pal);
        widgets::field_label(ui, pal, "Default notification settings");
        widgets::hint(
            ui,
            pal,
            "Members get this unless they set their own. Only @mentions is recommended for public servers.",
        );
        ui.radio_value(&mut d.default_notifications, 0, "All Messages");
        ui.radio_value(&mut d.default_notifications, 1, "Only @mentions");

        widgets::divider(ui, pal);
        ui.label(
            RichText::new("Safety Setup")
                .bold()
                .size(15.0)
                .color(pal.strong),
        );
        widgets::field_label(ui, pal, "Verification level");
        for (v, name, desc) in VERIFICATION_LEVELS {
            ui.radio_value(&mut d.verification_level, *v, *name);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                widgets::hint(ui, pal, desc);
            });
        }
        widgets::field_label(ui, pal, "Explicit image filter");
        for (v, name, desc) in CONTENT_FILTERS {
            ui.radio_value(&mut d.explicit_content_filter, *v, *name);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                widgets::hint(ui, pal, desc);
            });
        }
    }

    fn channels_ui(&mut self, ui: &mut egui::Ui, actx: &AdminCtx, out: &mut Vec<UiEvent>) {
        let pal = actx.pal;
        let guild = actx.guild;
        ui.horizontal(|ui| {
            if ui.add(pal.primary_button("Create Channel")).clicked() {
                out.push(UiEvent::OpenCreate {
                    parent: None,
                    category: false,
                });
            }
            if ui.button("Create Category").clicked() {
                out.push(UiEvent::OpenCreate {
                    parent: None,
                    category: true,
                });
            }
        });
        widgets::hint(
            ui,
            pal,
            "Use the arrows to reorder. Changes to the order are saved right away.",
        );
        widgets::error_line(ui, pal, &self.channels_error);
        ui.add_space(8.0);

        let mut cats: Vec<&Channel> = guild
            .channels
            .iter()
            .filter(|c| c.kind == CHANNEL_CATEGORY)
            .collect();
        cats.sort_by_key(|c| (c.position, c.id.parse::<u64>().unwrap_or(0)));
        let children = |parent: Option<&str>| -> Vec<&Channel> {
            let mut v: Vec<&Channel> = guild
                .channels
                .iter()
                .filter(|c| c.kind != CHANNEL_CATEGORY && c.parent_id.as_deref() == parent)
                .collect();
            v.sort_by_key(|c| (c.is_voice(), c.position, c.id.parse::<u64>().unwrap_or(0)));
            v
        };
        let mut moves = Vec::new();
        for c in children(None) {
            self.channel_row(ui, guild, c, false, pal, out, &mut moves);
        }
        for cat in cats {
            ui.add_space(6.0);
            self.channel_row(ui, guild, cat, false, pal, out, &mut moves);
            for c in children(Some(&cat.id)) {
                self.channel_row(ui, guild, c, true, pal, out, &mut moves);
            }
        }
        if !moves.is_empty() {
            self.channels_error = None;
            out.push(UiEvent::Request(ServerRequest::ReorderChannels {
                guild_id: guild.id.clone(),
                body: channels::reorder_body(&moves),
            }));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &mut self,
        ui: &mut egui::Ui,
        guild: &Guild,
        c: &Channel,
        indent: bool,
        pal: &Palette,
        out: &mut Vec<UiEvent>,
        moves: &mut Vec<channels::PositionUpdate>,
    ) {
        let kind = ChannelKind::from_type(c.kind);
        let is_cat = kind == ChannelKind::Category;
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::hover());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4, pal.hover);
        }
        let x0 = rect.left() + if indent { 24.0 } else { 6.0 };
        let icon_rect =
            egui::Rect::from_center_size(egui::pos2(x0 + 9.0, rect.center().y), vec2(14.0, 14.0));
        widgets::paint_icon(ui.painter(), icon_rect, kind.icon(), pal.muted);
        let (text, font, col) = if is_cat {
            (
                c.name.to_uppercase(),
                crate::theme::bold_font(13.0),
                pal.muted,
            )
        } else {
            (c.name.clone(), egui::FontId::proportional(14.5), pal.text)
        };
        ui.painter().text(
            egui::pos2(x0 + 24.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            col,
        );
        // Buttons on the right.
        let mut b = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(6.0, 5.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if widgets::icon_button(
            &mut b,
            Icon::Trash,
            24.0,
            if is_cat {
                "Delete Category"
            } else {
                "Delete Channel"
            },
            pal,
        )
        .clicked()
        {
            self.confirm_delete = Some((c.id.clone(), c.name.clone(), is_cat));
        }
        if widgets::icon_button(&mut b, Icon::Gear, 24.0, "Edit", pal).clicked() {
            out.push(UiEvent::OpenChannelSettings(c.id.clone()));
        }
        if is_cat
            && widgets::icon_button(
                &mut b,
                Icon::Plus,
                24.0,
                "Create Channel in this category",
                pal,
            )
            .clicked()
        {
            out.push(UiEvent::OpenCreate {
                parent: Some(c.id.clone()),
                category: false,
            });
        }
        if widgets::icon_button(&mut b, Icon::ChevronDown, 24.0, "Move down", pal).clicked() {
            *moves = channels::move_step(guild, &c.id, 1);
        }
        if widgets::icon_button(&mut b, Icon::ChevronUp, 24.0, "Move up", pal).clicked() {
            *moves = channels::move_step(guild, &c.id, -1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_patch_only_changes() {
        let v = json!({
            "id": "1", "name": "Pegasi", "icon": "abc", "description": null,
            "afk_channel_id": null, "afk_timeout": 300, "system_channel_id": "10",
            "default_message_notifications": 0, "verification_level": 1,
            "explicit_content_filter": 2
        });
        let orig = GuildDraft::from_json(&v);
        assert_eq!(orig.icon_hash.as_deref(), Some("abc"));
        assert_eq!(orig.system_channel_id.as_deref(), Some("10"));
        let mut d = orig.clone();
        assert!(!d.is_dirty(&orig));
        d.name = " Pegasi Peripherals ".into();
        d.system_channel_id = None;
        d.afk_channel_id = Some("20".into());
        d.afk_timeout = 900;
        d.verification_level = 3;
        d.icon = IconChange::Remove;
        assert_eq!(
            d.patch_body(&orig),
            json!({
                "name": "Pegasi Peripherals", "icon": null, "afk_channel_id": "20",
                "afk_timeout": 900, "system_channel_id": null, "verification_level": 3
            })
        );
        d = orig.clone();
        d.icon = IconChange::Set("data:image/png;base64,AA==".into());
        d.description = "hi".into();
        assert_eq!(
            d.patch_body(&orig),
            json!({ "icon": "data:image/png;base64,AA==", "description": "hi" })
        );
        // Removing a non-existent icon is not a change.
        let mut no_icon = orig.clone();
        no_icon.icon_hash = None;
        let mut d = no_icon.clone();
        d.icon = IconChange::Remove;
        assert!(!d.is_dirty(&no_icon));
        d.name = "x".into();
        assert!(d.validate().is_err());
    }

    #[test]
    fn sections_by_permission() {
        use ServerSettingsSection as S;
        assert!(section_allowed(S::Overview, p::MANAGE_GUILD));
        assert!(!section_allowed(S::Overview, p::MANAGE_CHANNELS));
        assert!(section_allowed(S::Channels, p::MANAGE_CHANNELS));
        assert!(section_allowed(S::Bans, u64::MAX));
        assert!(!section_allowed(S::Onboarding, p::MANAGE_GUILD));
        assert!(section_allowed(S::Members, p::KICK_MEMBERS));
        assert!(S::Invites.is_builtin() && !S::Roles.is_builtin());
        let mut all = vec![
            S::Bans,
            S::Invites,
            S::Roles,
            S::Overview,
            S::Onboarding,
            S::Other("Plugins"),
        ];
        all.sort_by_key(|s| (s.group(), s.rank()));
        assert_eq!(
            all,
            vec![
                S::Overview,
                S::Roles,
                S::Other("Plugins"),
                S::Onboarding,
                S::Invites,
                S::Bans
            ]
        );
    }
}
