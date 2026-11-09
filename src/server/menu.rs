//! The server dropdown under the server name in the channel list header
//! (Discord's "server menu"). Items are shown by permission; the menu only
//! reports what was picked.
//!
//! Use it from the header: make the header rect clickable, then
//! `egui::Popup::menu(&resp).show(|ui| server_menu(ui, &ctx))`, and draw
//! [`header_chevron`] at the right edge of the header.

use eframe::egui::{self, Rect, vec2};

use super::permissions as p;
use super::widgets::{self, Icon};
use crate::theme::Palette;
use crate::types::Guild;

/// What the menu needs to know.
pub struct ServerMenuCtx<'a> {
    pub guild: &'a Guild,
    pub my_id: &'a str,
    /// The channel being viewed (Invite People uses its permissions).
    pub selected_channel: Option<&'a str>,
    /// Current "Hide Muted Channels" state for this server.
    pub hide_muted: bool,
    pub pal: &'a Palette,
}

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMenuAction {
    InvitePeople,
    ServerSettings,
    CreateChannel,
    CreateCategory,
    NotificationSettings,
    PrivacySettings,
    EditServerProfile,
    /// New value for "Hide Muted Channels".
    SetHideMutedChannels(bool),
    /// Leave (the caller shows its confirm dialog first).
    LeaveServer,
}

/// Which permission-gated items to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MenuItems {
    pub invite: bool,
    pub server_settings: bool,
    pub create_channel: bool,
    pub create_category: bool,
    /// Owners can't leave (they must transfer or delete the server).
    pub leave: bool,
}

/// Decide the items from my permissions.
pub fn menu_items(guild: &Guild, my_id: &str, selected_channel: Option<&str>) -> MenuItems {
    let gp = p::guild_permissions(guild, my_id);
    let can_invite_here = selected_channel
        .map(|c| p::channel_permissions_in(guild, c, my_id))
        .is_some_and(|cp| p::has(cp, p::CREATE_INSTANT_INVITE | p::VIEW_CHANNEL));
    let invite = can_invite_here || !super::invites::invitable_channels(guild, my_id).is_empty();
    let manage_channels = p::has(gp, p::MANAGE_CHANNELS);
    MenuItems {
        invite,
        server_settings: p::has_any(gp, p::SERVER_SETTINGS_ANY),
        create_channel: manage_channels,
        create_category: manage_channels,
        leave: guild.owner_id != my_id,
    }
}

/// Draw the menu contents. Call inside a popup; it closes the popup when
/// something is picked.
pub fn server_menu(ui: &mut egui::Ui, ctx: &ServerMenuCtx) -> Option<ServerMenuAction> {
    let pal = ctx.pal;
    let items = menu_items(ctx.guild, ctx.my_id, ctx.selected_channel);
    ui.set_width(220.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    let mut out = None;
    let mut entry = |ui: &mut egui::Ui,
                     label: &str,
                     icon: Icon,
                     color: egui::Color32,
                     action: ServerMenuAction| {
        if menu_row(ui, label, Some(icon), color, None, pal).clicked() {
            out = Some(action);
        }
    };
    use ServerMenuAction as A;
    if items.invite {
        entry(
            ui,
            "Invite People",
            Icon::PersonPlus,
            pal.accent,
            A::InvitePeople,
        );
    }
    if items.server_settings {
        entry(
            ui,
            "Server Settings",
            Icon::Gear,
            pal.text,
            A::ServerSettings,
        );
    }
    if items.create_channel {
        entry(
            ui,
            "Create Channel",
            Icon::PlusCircle,
            pal.text,
            A::CreateChannel,
        );
    }
    if items.create_category {
        entry(
            ui,
            "Create Category",
            Icon::FolderPlus,
            pal.text,
            A::CreateCategory,
        );
    }
    if items.invite || items.server_settings || items.create_channel {
        sep(ui, pal);
    }
    entry(
        ui,
        "Notification Settings",
        Icon::Bell,
        pal.text,
        A::NotificationSettings,
    );
    entry(
        ui,
        "Privacy Settings",
        Icon::Shield,
        pal.text,
        A::PrivacySettings,
    );
    sep(ui, pal);
    entry(
        ui,
        "Edit Server Profile",
        Icon::Pencil,
        pal.text,
        A::EditServerProfile,
    );
    if menu_row(
        ui,
        "Hide Muted Channels",
        None,
        pal.text,
        Some(ctx.hide_muted),
        pal,
    )
    .clicked()
    {
        out = Some(A::SetHideMutedChannels(!ctx.hide_muted));
    }
    if items.leave {
        sep(ui, pal);
        if menu_row(ui, "Leave Server", Some(Icon::Leave), pal.danger, None, pal).clicked() {
            out = Some(A::LeaveServer);
        }
    }
    if out.is_some() {
        ui.close();
    }
    out
}

fn sep(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        r.shrink2(vec2(6.0, 0.0)).x_range(),
        r.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
    ui.add_space(2.0);
}

/// One menu row: label left, icon or checkbox right. Hover fills with the
/// item's colour like Discord (accent / danger) and turns text white.
fn menu_row(
    ui: &mut egui::Ui,
    label: &str,
    icon: Option<Icon>,
    color: egui::Color32,
    check: Option<bool>,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::click());
    let hovered = resp.hovered();
    let special = color != pal.text;
    if hovered {
        ui.painter()
            .rect_filled(rect, 4, if special { color } else { pal.accent });
    }
    let fg = if hovered { egui::Color32::WHITE } else { color };
    ui.painter().text(
        rect.left_center() + vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        fg,
    );
    let r = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(16.0, 16.0));
    if let Some(on) = check {
        let stroke = egui::Stroke::new(1.5, fg);
        if on {
            ui.painter().rect_filled(
                r,
                3,
                if hovered {
                    egui::Color32::WHITE
                } else {
                    pal.accent
                },
            );
            widgets::paint_icon(
                ui.painter(),
                r.shrink(2.0),
                Icon::Check,
                if hovered {
                    pal.accent
                } else {
                    egui::Color32::WHITE
                },
            );
        } else {
            ui.painter()
                .rect_stroke(r, 3, stroke, egui::StrokeKind::Inside);
        }
    } else if let Some(icon) = icon {
        widgets::paint_icon(ui.painter(), r, icon, fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The chevron (closed) / cross (open) at the right of the server header.
pub fn header_chevron(painter: &egui::Painter, header: Rect, open: bool, pal: &Palette) {
    let r = Rect::from_center_size(header.right_center() - vec2(18.0, 0.0), vec2(14.0, 14.0));
    widgets::paint_icon(
        painter,
        r,
        if open { Icon::Close } else { Icon::ChevronDown },
        pal.strong,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guild(everyone: u64) -> Guild {
        let mut g = Guild::from_json(&json!({
            "id": "1", "owner_id": "owner",
            "roles": [
                { "id": "1", "name": "@everyone", "permissions": everyone.to_string() },
                { "id": "mod", "name": "Mod", "permissions": (p::MANAGE_CHANNELS | p::KICK_MEMBERS).to_string() }
            ],
            "channels": [{ "id": "10", "type": 0, "name": "general" }]
        }));
        g.my_roles = Vec::new();
        g
    }

    #[test]
    fn items_by_permission() {
        let g = guild(p::VIEW_CHANNEL | p::SEND_MESSAGES);
        let m = menu_items(&g, "u", Some("10"));
        assert_eq!(
            m,
            MenuItems {
                invite: false,
                server_settings: false,
                create_channel: false,
                create_category: false,
                leave: true
            }
        );
        let g = guild(p::VIEW_CHANNEL | p::CREATE_INSTANT_INVITE);
        assert!(menu_items(&g, "u", None).invite);

        let mut g = guild(p::VIEW_CHANNEL);
        g.my_roles = vec!["mod".into()];
        let m = menu_items(&g, "u", Some("10"));
        assert!(m.server_settings && m.create_channel && m.create_category && !m.invite);

        let m = menu_items(&g, "owner", None);
        assert!(m.invite && m.server_settings && m.create_channel && !m.leave);
    }
}
