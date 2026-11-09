//! Server administration: the server dropdown menu, Server Settings (with
//! pluggable pages), Channel Settings with the permission overwrite editor,
//! creating / editing / deleting / reordering channels and categories, and
//! invites.
//!
//! Data in, actions out. Nothing here does networking: windows emit
//! [`ServerRequest`]s, the backend runs them with [`rest::execute`] and the
//! answers come back through [`ServerAdmin::handle`]. Writes only ever
//! follow a user click (Save Changes, Create, …); deletes and revokes go
//! through a confirm dialog first.
//!
//! Wiring (for `app.rs` / `backend.rs`):
//! - Header: `menu::server_menu(ui, &ServerMenuCtx { .. })` inside a popup
//!   on the server-name header; pass the result to
//!   [`ServerAdmin::on_menu`]. It handles Invite People, Server Settings,
//!   Create Channel / Category and returns the other actions to the caller
//!   (notification / privacy / server profile / hide muted / leave).
//! - Each frame: if [`ServerAdmin::covers_window`], draw
//!   [`ServerAdmin::show`] with the root `ui` instead of the normal layout;
//!   otherwise still call it (it then only draws the modal dialogs).
//! - `AdminOutput::Request(r)` → `ToBackend::Server(r)` → backend runs
//!   `rest::execute(&api, r).await` → `ToUi::Server(resp)` →
//!   [`ServerAdmin::handle`].
//! - `AdminOutput::PickIcon` → backend runs `rest::pick_icon_data_uri()`
//!   → `ToUi::ServerIconPicked(Option<Result<String, String>>)` →
//!   [`ServerAdmin::set_icon`].

pub mod channel_settings;
pub mod channels;
pub mod invites;
pub mod menu;
pub mod overwrites;
pub mod permissions;
pub mod rest;
pub mod settings;
pub mod shell;
pub mod widgets;

use eframe::egui;

use crate::theme::Palette;
use crate::types::Guild;

#[allow(unused_imports)]
pub use menu::{ServerMenuAction, ServerMenuCtx, server_menu};
pub use rest::{ServerRequest, ServerResponse};
#[allow(unused_imports)]
pub use settings::{ExternalPages, NoExternalPages, ServerSettingsSection};

use channel_settings::ChannelSettings;
use channels::CreateChannelDialog;
use invites::InviteDialog;
use settings::ServerSettings;

/// What the windows need each frame.
pub struct AdminCtx<'a> {
    /// The server the windows are open for (see [`ServerAdmin::guild_id`]).
    pub guild: &'a Guild,
    pub my_id: &'a str,
    /// Known members `(user id, display name)` for member overwrites.
    pub members: &'a [(String, String)],
    /// The channel being viewed (default for Invite People).
    pub selected_channel: Option<&'a str>,
    /// Images module on (server icon preview from the CDN).
    pub show_images: bool,
    pub pal: &'a Palette,
}

/// Work for the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum AdminOutput {
    /// Send to the backend.
    Request(ServerRequest),
    /// Open the image picker for the server icon.
    PickIcon,
}

/// Internal events from the windows.
pub(crate) enum UiEvent {
    Request(ServerRequest),
    PickIcon,
    OpenChannelSettings(String),
    OpenCreate {
        parent: Option<String>,
        category: bool,
    },
    OpenInvite(Option<String>),
    CloseServerSettings,
    CloseChannelSettings,
}

/// All open server admin windows for one server.
#[derive(Default)]
pub struct ServerAdmin {
    guild_id: Option<String>,
    settings: Option<ServerSettings>,
    channel: Option<ChannelSettings>,
    invite: Option<InviteDialog>,
    create: Option<CreateChannelDialog>,
    /// Requests produced outside `show` (menu, open_*), handed out by the
    /// next `show`.
    pending: Vec<AdminOutput>,
}

impl ServerAdmin {
    /// The server the open windows belong to.
    pub fn guild_id(&self) -> Option<&str> {
        self.guild_id.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.settings.is_some()
            || self.channel.is_some()
            || self.invite.is_some()
            || self.create.is_some()
    }

    /// A full-window page (Server or Channel Settings) is open.
    pub fn covers_window(&self) -> bool {
        self.settings.is_some() || self.channel.is_some()
    }

    fn bind(&mut self, guild: &Guild) {
        if self.guild_id.as_deref() != Some(guild.id.as_str()) {
            self.settings = None;
            self.channel = None;
            self.invite = None;
            self.create = None;
            self.guild_id = Some(guild.id.clone());
        }
    }

    /// Close everything (e.g. the server was left / deleted). Frees the icon
    /// preview image.
    pub fn close_all(&mut self, ctx: &egui::Context) {
        if let Some(s) = &mut self.settings {
            s.forget_preview(ctx);
        }
        *self = Self::default();
    }

    /// Handle a server menu pick. Returns it back if it's not ours
    /// (notifications, privacy, server profile, hide muted, leave).
    pub fn on_menu(
        &mut self,
        action: ServerMenuAction,
        ctx: &AdminCtx,
    ) -> Option<ServerMenuAction> {
        match action {
            ServerMenuAction::InvitePeople => self.open_invite(ctx, ctx.selected_channel),
            ServerMenuAction::ServerSettings => {
                self.open_server_settings(ctx, ServerSettingsSection::Overview)
            }
            ServerMenuAction::CreateChannel => self.open_create(ctx, None, false),
            ServerMenuAction::CreateCategory => self.open_create(ctx, None, true),
            other => return Some(other),
        }
        None
    }

    /// Open Server Settings on `section` (falls back to the first page I
    /// may see).
    pub fn open_server_settings(&mut self, ctx: &AdminCtx, section: ServerSettingsSection) {
        self.bind(ctx.guild);
        let s = ServerSettings::new(&ctx.guild.id, section);
        if let Some(r) = s.load_for(section) {
            self.pending.push(AdminOutput::Request(r));
        }
        self.settings = Some(s);
    }

    /// Channel Settings for a channel or category ("Edit Channel").
    pub fn open_channel_settings(&mut self, ctx: &AdminCtx, channel_id: &str) {
        self.bind(ctx.guild);
        if let Some(ch) = ctx.guild.channel(channel_id) {
            let (s, r) = ChannelSettings::new(&ctx.guild.id, ch);
            self.channel = Some(s);
            self.pending.push(AdminOutput::Request(r));
        }
    }

    /// "Create Channel" (optionally in a category) / "Create Category".
    pub fn open_create(&mut self, ctx: &AdminCtx, parent: Option<String>, category: bool) {
        self.bind(ctx.guild);
        self.create = Some(CreateChannelDialog::new(&ctx.guild.id, category, parent));
    }

    /// "Invite People" (optionally for a given channel).
    pub fn open_invite(&mut self, ctx: &AdminCtx, channel: Option<&str>) {
        self.bind(ctx.guild);
        let (d, r) = InviteDialog::open(ctx.guild, ctx.my_id, channel);
        self.invite = Some(d);
        if let Some(r) = r {
            self.pending.push(AdminOutput::Request(r));
        }
    }

    /// Draw the open windows. With a full-window page open this fills `ui`;
    /// otherwise only modals are drawn. `pages` provides the external
    /// Server Settings pages.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &AdminCtx,
        pages: &mut dyn ExternalPages,
    ) -> Vec<AdminOutput> {
        if self.guild_id.as_deref() != Some(ctx.guild.id.as_str()) {
            if self.is_open() {
                // The server changed under us: close everything.
                self.close_all(ui.ctx());
            }
            return std::mem::take(&mut self.pending);
        }
        let mut events = Vec::new();
        if let Some(c) = &mut self.channel {
            c.ui(ui, ctx, &mut events);
        } else if let Some(s) = &mut self.settings {
            s.ui(ui, ctx, pages, &mut events);
        }
        let ectx = ui.ctx().clone();
        if let Some(d) = &mut self.create {
            let (close, req) = d.show(&ectx, ctx.guild, ctx.my_id, ctx.pal);
            if let Some(r) = req {
                events.push(UiEvent::Request(r));
            }
            if close {
                self.create = None;
            }
        }
        if let Some(d) = &mut self.invite {
            let (close, req) = d.show(&ectx, ctx.guild, ctx.my_id, ctx.pal);
            if let Some(r) = req {
                events.push(UiEvent::Request(r));
            }
            if close {
                self.invite = None;
            }
        }
        for e in events {
            match e {
                UiEvent::Request(r) => self.pending.push(AdminOutput::Request(r)),
                UiEvent::PickIcon => self.pending.push(AdminOutput::PickIcon),
                UiEvent::OpenChannelSettings(id) => self.open_channel_settings(ctx, &id),
                UiEvent::OpenCreate { parent, category } => self.open_create(ctx, parent, category),
                UiEvent::OpenInvite(ch) => self.open_invite(ctx, ch.as_deref()),
                UiEvent::CloseServerSettings => {
                    if let Some(s) = &mut self.settings {
                        s.forget_preview(&ectx);
                    }
                    self.settings = None;
                }
                UiEvent::CloseChannelSettings => self.channel = None,
            }
        }
        std::mem::take(&mut self.pending)
    }

    /// The icon picked after `AdminOutput::PickIcon` (`None` = cancelled).
    pub fn set_icon(&mut self, ctx: &egui::Context, picked: Option<Result<String, String>>) {
        if let (Some(s), Some(result)) = (&mut self.settings, picked) {
            s.set_icon(ctx, result);
        }
    }

    /// Feed a backend answer to the windows it belongs to.
    pub fn handle(&mut self, resp: ServerResponse) {
        match resp {
            ServerResponse::Guild { guild_id, data } => {
                if let Some(s) = self.settings.as_mut().filter(|s| s.guild_id == guild_id) {
                    s.on_guild(&data);
                }
            }
            ServerResponse::GuildInvites { guild_id, invites } => {
                if let Some(s) = self.settings.as_mut().filter(|s| s.guild_id == guild_id) {
                    s.invites.set(invites.clone());
                }
                if let Some(d) = self.invite.as_mut().filter(|d| d.guild_id == guild_id) {
                    d.on_invites(&invites);
                }
            }
            ServerResponse::ChannelInvites {
                channel_id,
                invites,
            } => {
                if let Some(c) = self.channel.as_mut().filter(|c| c.channel_id == channel_id) {
                    c.invites.set(invites.clone());
                }
                if let Some(d) = &mut self.invite {
                    d.on_invites(&invites);
                }
            }
            ServerResponse::InviteCreated { channel_id, invite } => {
                if let Some(c) = self.channel.as_mut().filter(|c| c.channel_id == channel_id)
                    && let Some(list) = &mut c.invites.invites
                {
                    list.insert(0, invite.clone());
                }
                if let Some(s) = &mut self.settings
                    && let Some(list) = &mut s.invites.invites
                {
                    list.retain(|i| i.code != invite.code);
                    list.insert(0, invite.clone());
                }
                if let Some(d) = &mut self.invite {
                    d.on_created(invite);
                }
            }
            ServerResponse::InviteRevoked { code } => {
                if let Some(s) = &mut self.settings {
                    s.invites.revoked(&code);
                }
                if let Some(c) = &mut self.channel {
                    c.invites.revoked(&code);
                }
                if let Some(d) = &mut self.invite {
                    d.on_revoked(&code);
                }
            }
            ServerResponse::Channel { channel_id, data } => {
                if let Some(c) = self.channel.as_mut().filter(|c| c.channel_id == channel_id) {
                    c.on_channel(&data);
                }
            }
            ServerResponse::ChannelCreated { guild_id, .. } => {
                if self.create.as_ref().is_some_and(|d| d.guild_id == guild_id) {
                    self.create = None;
                }
            }
            ServerResponse::ChannelDeleted { channel_id } => {
                if self
                    .channel
                    .as_ref()
                    .is_some_and(|c| c.channel_id == channel_id)
                {
                    self.channel = None;
                }
            }
            ServerResponse::Reordered { .. } => {}
            ServerResponse::OverwritesSaved {
                channel_id,
                overwrites,
            } => {
                if let Some(c) = self.channel.as_mut().filter(|c| c.channel_id == channel_id) {
                    c.on_overwrites_saved(&overwrites);
                }
            }
            ServerResponse::Failed { request, error } => self.failed(&request, error),
        }
    }

    fn failed(&mut self, req: &ServerRequest, error: String) {
        let msg = format!("{}: {error}", req.what());
        match req {
            ServerRequest::CreateChannel { .. } => {
                if let Some(d) = &mut self.create {
                    d.on_failed(msg);
                }
            }
            ServerRequest::CreateInvite { .. } => {
                if let Some(d) = &mut self.invite {
                    d.on_failed(msg);
                }
            }
            ServerRequest::LoadChannel { channel_id }
            | ServerRequest::PatchChannel { channel_id, .. }
            | ServerRequest::SaveOverwrites { channel_id, .. }
            | ServerRequest::LoadChannelInvites { channel_id }
                if self
                    .channel
                    .as_ref()
                    .is_some_and(|c| &c.channel_id == channel_id) =>
            {
                if let Some(c) = &mut self.channel {
                    if matches!(req, ServerRequest::LoadChannelInvites { .. }) {
                        c.invites.error = Some(msg);
                        if c.invites.invites.is_none() {
                            c.invites.invites = Some(Vec::new());
                        }
                    } else {
                        c.on_failed(msg);
                    }
                }
            }
            ServerRequest::DeleteChannel { channel_id }
                if self
                    .channel
                    .as_ref()
                    .is_some_and(|c| &c.channel_id == channel_id) =>
            {
                if let Some(c) = &mut self.channel {
                    c.on_failed(msg);
                }
            }
            ServerRequest::RevokeInvite { .. } => {
                if let Some(c) = &mut self.channel {
                    c.invites.error = Some(msg.clone());
                }
                if let Some(s) = &mut self.settings {
                    s.on_failed(req, msg);
                }
            }
            _ => {
                if let Some(s) = &mut self.settings {
                    s.on_failed(req, msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeChoice;
    use serde_json::json;

    #[test]
    fn menu_routing_and_responses() {
        let g = Guild::from_json(&json!({
            "id": "1", "owner_id": "me",
            "channels": [{ "id": "10", "type": 0, "name": "general" }]
        }));
        let pal = ThemeChoice::DiscordDark.palette();
        let ctx = AdminCtx {
            guild: &g,
            my_id: "me",
            members: &[],
            selected_channel: Some("10"),
            show_images: false,
            pal: &pal,
        };
        let mut a = ServerAdmin::default();
        assert_eq!(
            a.on_menu(ServerMenuAction::LeaveServer, &ctx),
            Some(ServerMenuAction::LeaveServer)
        );
        assert!(!a.is_open());
        assert_eq!(a.on_menu(ServerMenuAction::ServerSettings, &ctx), None);
        assert!(a.covers_window());
        assert_eq!(a.guild_id(), Some("1"));
        assert_eq!(
            a.pending,
            vec![AdminOutput::Request(ServerRequest::LoadGuild {
                guild_id: "1".into()
            })]
        );
        // Owner may list invites: opening the dialog loads them, creates nothing.
        a.on_menu(ServerMenuAction::InvitePeople, &ctx);
        assert!(matches!(
            a.pending.last(),
            Some(AdminOutput::Request(ServerRequest::LoadGuildInvites { .. }))
        ));
        assert!(
            !a.pending
                .iter()
                .any(|o| matches!(o, AdminOutput::Request(ServerRequest::CreateInvite { .. })))
        );

        a.on_menu(ServerMenuAction::CreateChannel, &ctx);
        assert!(a.create.is_some());
        a.handle(ServerResponse::ChannelCreated {
            guild_id: "1".into(),
            data: json!({}),
        });
        assert!(a.create.is_none());

        a.open_channel_settings(&ctx, "10");
        assert!(a.channel.is_some());
        a.handle(ServerResponse::ChannelDeleted {
            channel_id: "10".into(),
        });
        assert!(a.channel.is_none());
    }
}
