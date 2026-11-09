//! Channel Settings (also used for categories): Overview, Permissions,
//! Invites and Delete, in the same full-window layout as Server Settings.
//!
//! Overview starts from the cached `Channel` and is replaced by the full
//! object from `GET channels/{id}` (nsfw, slowmode, bitrate, user limit are
//! only there). Saving sends a `PATCH channels/{id}` with the changed
//! fields; permission edits are saved as PUT / DELETE overwrite calls.

use eframe::egui::{self, RichText};

use super::channels::{
    self, BITRATE_MAX, BITRATE_MIN, ChannelDraft, ChannelKind, SLOWMODE_OPTIONS,
};
use super::invites::InviteList;
use super::overwrites::OverwriteEditor;
use super::permissions as p;
use super::rest::ServerRequest;
use super::shell::{self, Flash, NavGroup};
use super::widgets::{self, BarAction, Confirm};
use super::{AdminCtx, UiEvent};
use crate::perms::Overwrite;
use crate::types::{CHANNEL_CATEGORY, Channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSection {
    Overview,
    Permissions,
    Invites,
    Delete,
}

pub struct ChannelSettings {
    pub guild_id: String,
    pub channel_id: String,
    pub section: ChannelSection,
    original: ChannelDraft,
    draft: ChannelDraft,
    /// Full channel object arrived.
    loaded: bool,
    overwrites: OverwriteEditor,
    pub(super) invites: InviteList,
    saving: bool,
    error: Option<String>,
    confirm_delete: bool,
    flash: Flash,
}

impl ChannelSettings {
    /// State plus the load request.
    pub fn new(guild_id: &str, channel: &Channel) -> (Self, ServerRequest) {
        let d = ChannelDraft::from_channel(channel);
        let s = Self {
            guild_id: guild_id.to_string(),
            channel_id: channel.id.clone(),
            section: ChannelSection::Overview,
            original: d.clone(),
            draft: d,
            loaded: false,
            overwrites: OverwriteEditor::new(guild_id, channel.kind, &channel.overwrites),
            invites: InviteList::default(),
            saving: false,
            error: None,
            confirm_delete: false,
            flash: Flash::default(),
        };
        (
            s,
            ServerRequest::LoadChannel {
                channel_id: channel.id.clone(),
            },
        )
    }

    pub fn on_channel(&mut self, data: &serde_json::Value) {
        let fresh = ChannelDraft::from_json(data);
        if self.saving || !self.draft.is_dirty(&self.original) {
            self.draft = fresh.clone();
        }
        self.original = fresh;
        self.loaded = true;
        self.saving = false;
        self.error = None;
        let ow = Overwrite::parse_list(&data["permission_overwrites"]);
        if !self.overwrites.is_dirty() {
            self.overwrites.reset_to(&ow);
        }
    }

    pub fn on_overwrites_saved(&mut self, ow: &[Overwrite]) {
        self.overwrites.reset_to(ow);
        self.saving = false;
        self.error = None;
    }

    /// Keep the editor in step with gateway updates while nothing is edited.
    pub fn sync_from(&mut self, channel: &Channel) {
        if !self.saving
            && !self.overwrites.is_dirty()
            && self.overwrites.original != channel.overwrites
        {
            self.overwrites.reset_to(&channel.overwrites);
        }
    }

    pub fn on_failed(&mut self, error: String) {
        self.saving = false;
        self.error = Some(error);
    }

    fn dirty(&self) -> bool {
        match self.section {
            ChannelSection::Overview => self.draft.is_dirty(&self.original),
            ChannelSection::Permissions => self.overwrites.is_dirty(),
            _ => false,
        }
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui, actx: &AdminCtx, out: &mut Vec<UiEvent>) {
        let pal = actx.pal;
        let guild = actx.guild;
        let Some(channel) = guild.channel(&self.channel_id) else {
            out.push(UiEvent::CloseChannelSettings);
            return;
        };
        self.sync_from(channel);
        let is_cat = channel.kind == CHANNEL_CATEGORY;
        let kind = ChannelKind::from_type(channel.kind);
        let my = p::channel_permissions_in(guild, &channel.id, actx.my_id);
        let can_manage = p::has(my, p::MANAGE_CHANNELS);
        let can_perms = p::has(my, p::MANAGE_ROLES);

        let header = if is_cat {
            channel.name.to_uppercase()
        } else {
            let prefix = if kind.slug_names() { "#" } else { "" };
            format!("{prefix}{}  {} channel", channel.name, kind.label())
        };
        let mut group = NavGroup::new(header).item(ChannelSection::Overview, "Overview");
        if can_perms {
            group = group.item(ChannelSection::Permissions, "Permissions");
        }
        if !is_cat && can_manage {
            group = group.item(ChannelSection::Invites, "Invites");
        }
        let mut groups = vec![group];
        if can_manage {
            groups.push(NavGroup::new("").danger(
                ChannelSection::Delete,
                if is_cat {
                    "Delete Category"
                } else {
                    "Delete Channel"
                },
            ));
        }
        let dirty = self.dirty();
        let section = self.section;
        let title = match section {
            ChannelSection::Overview => "Overview",
            ChannelSection::Permissions => "Channel Permissions",
            ChannelSection::Invites => "Invites",
            ChannelSection::Delete => "Overview",
        };
        let mut flash = self.flash;
        let mut events = Vec::new();
        let res = shell::show(
            ui,
            "channel_settings",
            pal,
            &groups,
            section,
            title,
            dirty,
            self.saving,
            &mut flash,
            |ui| {
                widgets::error_line(ui, pal, &self.error);
                match section {
                    ChannelSection::Overview | ChannelSection::Delete => {
                        self.overview_ui(ui, actx, can_manage)
                    }
                    ChannelSection::Permissions => {
                        widgets::hint(
                            ui,
                            pal,
                            "Use permissions to customise who can do what in this channel. Neutral (/) keeps what their roles give them.",
                        );
                        ui.add_space(8.0);
                        self.overwrites.ui(ui, guild, actx.members, pal);
                    }
                    ChannelSection::Invites => {
                        ui.horizontal(|ui| {
                            if ui.add(pal.primary_button("Create invite")).clicked() {
                                events.push(UiEvent::OpenInvite(Some(self.channel_id.clone())));
                            }
                        });
                        ui.add_space(8.0);
                        if let Some(r) = self.invites.ui(ui, false, pal) {
                            events.push(UiEvent::Request(r));
                        }
                    }
                }
            },
        );
        self.flash = flash;
        out.extend(events);

        if let Some(bar) = res.bar {
            match (section, bar) {
                (ChannelSection::Permissions, BarAction::Reset) => self.overwrites.reset(),
                (ChannelSection::Permissions, BarAction::Save) => {
                    self.saving = true;
                    self.error = None;
                    out.push(UiEvent::Request(ServerRequest::SaveOverwrites {
                        channel_id: self.channel_id.clone(),
                        changes: self.overwrites.changes(),
                        result: self.overwrites.result(),
                    }));
                }
                (_, BarAction::Reset) => {
                    self.draft = self.original.clone();
                    self.error = None;
                }
                (_, BarAction::Save) => match self.draft.validate() {
                    Ok(()) => {
                        self.saving = true;
                        self.error = None;
                        out.push(UiEvent::Request(ServerRequest::PatchChannel {
                            channel_id: self.channel_id.clone(),
                            body: self.draft.patch_body(&self.original),
                        }));
                    }
                    Err(e) => self.error = Some(e),
                },
            }
        }
        if let Some(s) = res.nav {
            if s == ChannelSection::Delete {
                self.confirm_delete = true;
            } else {
                if s == ChannelSection::Invites && s != self.section {
                    out.push(UiEvent::Request(ServerRequest::LoadChannelInvites {
                        channel_id: self.channel_id.clone(),
                    }));
                }
                self.section = s;
            }
        }
        if res.close {
            out.push(UiEvent::CloseChannelSettings);
        }
        if self.confirm_delete {
            let c = if is_cat {
                Confirm::new(
                    "Delete Category",
                    format!(
                        "Are you sure you want to delete {}? The channels in it will stay, without a category. This cannot be undone.",
                        channel.name
                    ),
                    "Delete Category",
                )
            } else {
                Confirm::new(
                    "Delete Channel",
                    format!(
                        "Are you sure you want to delete #{}? This cannot be undone.",
                        channel.name
                    ),
                    "Delete Channel",
                )
            };
            match c.show(ui.ctx(), pal) {
                Some(true) => {
                    self.confirm_delete = false;
                    out.push(UiEvent::Request(ServerRequest::DeleteChannel {
                        channel_id: self.channel_id.clone(),
                    }));
                }
                Some(false) => self.confirm_delete = false,
                None => {}
            }
        }
    }

    fn overview_ui(&mut self, ui: &mut egui::Ui, actx: &AdminCtx, can_manage: bool) {
        let pal = actx.pal;
        let d = &mut self.draft;
        let kind = d.kind;
        if !self.loaded {
            ui.horizontal(|ui| {
                ui.spinner();
                widgets::hint(ui, pal, "Loading channel details…");
            });
        }
        ui.add_enabled_ui(can_manage, |ui| {
            widgets::field_label(ui, pal, if kind == ChannelKind::Category { "Category name" } else { "Channel name" });
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY).char_limit(100));
            if kind.slug_names() {
                let n = channels::normalize_name(kind, &d.name);
                if !n.is_empty() && n != d.name {
                    widgets::hint(ui, pal, &format!("Will be saved as #{n}"));
                }
            }
            if kind.has_topic() {
                widgets::field_label(ui, pal, if kind == ChannelKind::Forum { "Post guidelines" } else { "Channel topic" });
                ui.add(
                    egui::TextEdit::multiline(&mut d.topic)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .char_limit(kind.topic_limit())
                        .hint_text("Let everyone know how to use this channel!"),
                );
                widgets::hint(ui, pal, &format!("{} / {}", d.topic.chars().count(), kind.topic_limit()));
            }
            if kind.has_slowmode() {
                widgets::divider(ui, pal);
                widgets::field_label(ui, pal, "Slowmode");
                let mut idx = SLOWMODE_OPTIONS
                    .iter()
                    .position(|s| *s >= d.slowmode)
                    .unwrap_or(SLOWMODE_OPTIONS.len() - 1);
                let label = channels::slowmode_label(SLOWMODE_OPTIONS[idx]);
                let resp = ui.add(
                    egui::Slider::new(&mut idx, 0..=SLOWMODE_OPTIONS.len() - 1)
                        .show_value(false)
                        .text(label),
                );
                if resp.changed() {
                    d.slowmode = SLOWMODE_OPTIONS[idx];
                }
                widgets::hint(
                    ui,
                    pal,
                    "Members will be restricted to sending one message per this interval, unless they have Manage Channel or Manage Messages.",
                );
            }
            if kind.has_nsfw() {
                widgets::divider(ui, pal);
                widgets::toggle_row(
                    ui,
                    "Age-Restricted Channel",
                    "Users will need to confirm they are of legal age to view the content in this channel.",
                    &mut d.nsfw,
                    pal,
                );
            }
            if kind.has_voice_settings() {
                widgets::divider(ui, pal);
                widgets::field_label(ui, pal, "Bitrate");
                let mut kbps = d.bitrate / 1000;
                let max = (BITRATE_MAX / 1000).max(kbps);
                if ui
                    .add(egui::Slider::new(&mut kbps, BITRATE_MIN / 1000..=max).suffix("kbps"))
                    .changed()
                {
                    d.bitrate = kbps * 1000;
                }
                widgets::hint(ui, pal, "Higher bitrate sounds better but uses more bandwidth.");
                widgets::field_label(ui, pal, "User limit");
                let max_users = if kind == ChannelKind::Stage { 10_000 } else { 99 };
                ui.add(
                    egui::Slider::new(&mut d.user_limit, 0..=max_users)
                        .custom_formatter(|v, _| if v == 0.0 { "No limit".into() } else { format!("{v:.0} users") }),
                );
            }
            if kind != ChannelKind::Category {
                widgets::divider(ui, pal);
                widgets::field_label(ui, pal, "Category");
                let cats: Vec<(Option<String>, String)> = std::iter::once((None, "No category".to_string()))
                    .chain(
                        actx.guild
                            .channels
                            .iter()
                            .filter(|c| c.kind == CHANNEL_CATEGORY)
                            .map(|c| (Some(c.id.clone()), c.name.clone())),
                    )
                    .collect();
                widgets::combo(ui, "channel_parent", &mut d.parent_id, &cats, 260.0);
            }
        });
        if !can_manage {
            ui.add_space(8.0);
            ui.label(RichText::new("You need Manage Channels to edit this.").color(pal.muted));
        }
    }
}
