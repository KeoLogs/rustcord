//! Voice UI widgets, data in / actions out (no networking):
//!
//! - [`voice_channel_row`]: a voice channel in the channel list (speaker
//!   icon, name, member count when collapsed), clickable to join.
//! - [`channel_members`]: the people under a voice channel (avatar with a
//!   green ring while speaking, name, LIVE / camera / muted / deafened).
//! - [`connected_bar`]: Discord's "Voice Connected" box above the profile
//!   bar (status, channel / server, E2EE lock, disconnect).
//! - [`self_controls`]: mute and deafen toggles for the profile bar.
//!
//! The caller turns [`VoiceAction`]s into `ToBackend` requests. Joining is
//! only ever the result of the user's click.

use eframe::egui::{self, Color32, CursorIcon, Rect, Sense, pos2, vec2};

use super::icons;
use super::state::{Call, ConnStatus, Quality, VoiceState};
use crate::theme::{self, Palette};

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceAction {
    /// Clicked a voice channel.
    Join {
        guild_id: Option<String>,
        channel_id: String,
    },
    Disconnect,
    ToggleMute,
    ToggleDeafen,
    /// Clicked someone in a voice channel: open their profile popout.
    OpenProfile {
        user_id: String,
    },
    /// Clicked the status line: show connection details (ping, mode,
    /// E2EE privacy code) — a later stage.
    ShowConnectionInfo,
}

/// How to show one person (the caller resolves nicknames from its member
/// cache and builds the avatar URL).
#[derive(Debug, Clone, Default)]
pub struct MemberDisplay {
    pub name: String,
    pub avatar_url: String,
    /// Role colour for the name, if any.
    pub color: Option<Color32>,
}

/// What [`channel_members`] needs from the app.
pub struct ListCtx<'a> {
    pub pal: &'a Palette,
    /// The Images module: when off, avatars are letter discs (no download).
    pub images: bool,
    pub lookup: &'a dyn Fn(&VoiceState) -> MemberDisplay,
    pub is_speaking: &'a dyn Fn(&str) -> bool,
}

/// One voice channel in the channel list. `count` is shown at the right
/// when > 0 and the member list under it isn't shown (e.g. collapsed
/// category). `connected`: we're in this channel (name in strong colour).
pub fn voice_channel_row(
    ui: &mut egui::Ui,
    name: &str,
    count: usize,
    connected: bool,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let hovered = resp.hovered();
    if connected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if hovered {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let color = if connected {
        pal.strong
    } else if hovered {
        pal.text
    } else {
        pal.muted
    };
    let icon = Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(18.0, 18.0));
    icons::speaker(ui.painter(), icon, color);
    let x = rect.left() + 36.0;
    let right = if count > 0 { 30.0 } else { 8.0 };
    let galley = truncate(
        ui,
        name,
        egui::FontId::proportional(15.0),
        rect.right() - right - x,
    );
    ui.painter().galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if count > 0 {
        ui.painter().text(
            pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(12.0),
            pal.muted,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if connected {
            "You're in this channel"
        } else {
            "Join voice"
        })
}

/// The people in a voice channel, indented under it.
pub fn channel_members(
    ui: &mut egui::Ui,
    members: &[&VoiceState],
    ctx: &ListCtx,
) -> Option<VoiceAction> {
    let mut action = None;
    let pal = ctx.pal;
    for m in members {
        let shown = (ctx.lookup)(m);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let hovered = resp.hovered();
        if hovered {
            ui.painter().rect_filled(rect, 4, pal.hover);
        }
        let speaking = (ctx.is_speaking)(&m.user_id);
        let av =
            Rect::from_center_size(pos2(rect.left() + 48.0, rect.center().y), vec2(24.0, 24.0));
        paint_avatar(
            ui,
            av,
            &m.user_id,
            &shown.name,
            &shown.avatar_url,
            ctx.images,
            pal,
        );
        if speaking {
            icons::speaking_ring(ui.painter(), av, pal.online);
        }

        // Status icons from the right edge inwards.
        let mut right = rect.right() - 6.0;
        let icon_size = 16.0;
        let bg = if hovered { pal.hover } else { pal.sidebar };
        let mut slot = |ui: &egui::Ui, draw: &dyn Fn(&egui::Painter, Rect)| {
            let r = Rect::from_center_size(
                pos2(right - icon_size / 2.0, rect.center().y),
                vec2(icon_size, icon_size),
            );
            draw(ui.painter(), r);
            right -= icon_size + 4.0;
        };
        let off = if m.is_server_muted() {
            pal.danger
        } else {
            pal.muted
        };
        if m.is_deafened() {
            slot(ui, &|p, r| {
                icons::headphones(p, r, off);
                icons::slash(p, r, off, bg);
            });
        } else if m.is_muted() {
            slot(ui, &|p, r| {
                icons::microphone(p, r, off);
                icons::slash(p, r, off, bg);
            });
        }
        if m.self_video {
            slot(ui, &|p, r| icons::camera(p, r, pal.muted));
        }
        if m.self_stream {
            let w = 34.0;
            let r = Rect::from_center_size(pos2(right - w / 2.0, rect.center().y), vec2(w, 16.0));
            icons::pill(ui.painter(), r, pal.danger, "LIVE", Color32::WHITE);
            right -= w + 4.0;
        }

        let x = av.right() + 8.0;
        let name_color = if speaking || hovered {
            pal.strong
        } else {
            shown.color.unwrap_or(pal.muted)
        };
        let font = if speaking {
            theme::bold_font(15.0)
        } else {
            egui::FontId::proportional(15.0)
        };
        let galley = truncate(ui, &shown.name, font, (right - x).max(10.0));
        ui.painter().galley(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            name_color,
        );
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            action = Some(VoiceAction::OpenProfile {
                user_id: m.user_id.clone(),
            });
        }
    }
    action
}

/// The "Voice Connected" box. Draw it right above the profile bar (same
/// background); it's 52 px tall plus a 1 px divider on top.
pub fn connected_bar(ui: &mut egui::Ui, call: &Call, pal: &Palette) -> Option<VoiceAction> {
    let mut action = None;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 53.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, pal.divider),
    );
    let body = rect.shrink2(vec2(8.0, 6.0));

    // Disconnect button on the right.
    let btn = Rect::from_center_size(pos2(body.right() - 16.0, body.center().y), vec2(32.0, 32.0));
    let bresp = ui
        .interact(btn, ui.scope_id().with("voice_disconnect"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Disconnect");
    if bresp.hovered() {
        icons::button_bg(ui.painter(), btn, pal.hover);
    }
    icons::hangup(
        ui.painter(),
        btn.shrink(7.0),
        if bresp.hovered() {
            pal.danger
        } else {
            pal.text
        },
    );
    if bresp.clicked() {
        action = Some(VoiceAction::Disconnect);
    }

    // Status line: bars + "Voice Connected" (green / yellow / red).
    let status_color = match (&call.status, call.quality()) {
        (ConnStatus::Connected { .. }, Some(Quality::Okay)) => pal.idle,
        (ConnStatus::Connected { .. }, Some(Quality::Bad)) => pal.dnd,
        (ConnStatus::Connected { .. }, _) => pal.online,
        (ConnStatus::Failed(_), _) => pal.danger,
        _ => pal.idle,
    };
    let line1_y = body.top() + 10.0;
    let bars = Rect::from_center_size(pos2(body.left() + 9.0, line1_y), vec2(16.0, 16.0));
    icons::signal_bars(ui.painter(), bars, status_color);
    let text_x = bars.right() + 6.0;
    let max_w = btn.left() - 8.0 - text_x;
    let status = truncate(
        ui,
        &call.status_label(),
        theme::bold_font(14.5),
        max_w - 18.0,
    );
    let status_w = status.size().x;
    let status_rect =
        Rect::from_min_size(pos2(text_x, line1_y - status.size().y / 2.0), status.size());
    ui.painter().galley(status_rect.min, status, status_color);
    if call.e2ee {
        let l = Rect::from_center_size(pos2(text_x + status_w + 10.0, line1_y), vec2(13.0, 13.0));
        icons::lock(ui.painter(), l, pal.online);
        ui.interact(l, ui.scope_id().with("voice_e2ee"), Sense::hover())
            .on_hover_text("End-to-end encrypted");
    }
    let sresp = ui
        .interact(
            status_rect.expand(2.0),
            ui.scope_id().with("voice_status"),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(match call.status {
            ConnStatus::Connected { ping_ms: Some(p) } => format!("Ping: {p} ms"),
            _ => "Connection info".to_string(),
        });
    if sresp.clicked() {
        action = Some(VoiceAction::ShowConnectionInfo);
    }

    // Line 2: "General / My Server".
    let loc = truncate(
        ui,
        &call.location_label(),
        egui::FontId::proportional(12.0),
        btn.left() - 8.0 - body.left(),
    );
    ui.painter()
        .galley(pos2(body.left(), body.top() + 22.0), loc, pal.muted);
    action
}

/// Mute and deafen toggles (32 px icon buttons), for the profile bar.
/// When on, the icon gets a red slash like Discord.
pub fn self_controls(
    ui: &mut egui::Ui,
    self_mute: bool,
    self_deaf: bool,
    pal: &Palette,
) -> Option<VoiceAction> {
    let mut action = None;
    let mute_tip = if self_mute { "Unmute" } else { "Mute" };
    if toggle_icon(ui, self_mute, mute_tip, pal, icons::microphone).clicked() {
        action = Some(VoiceAction::ToggleMute);
    }
    let deaf_tip = if self_deaf { "Undeafen" } else { "Deafen" };
    if toggle_icon(ui, self_deaf, deaf_tip, pal, icons::headphones).clicked() {
        action = Some(VoiceAction::ToggleDeafen);
    }
    action
}

fn toggle_icon(
    ui: &mut egui::Ui,
    on: bool,
    tip: &str,
    pal: &Palette,
    draw: fn(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let bg = if resp.hovered() {
        icons::button_bg(ui.painter(), rect, pal.hover);
        pal.hover
    } else {
        pal.servers
    };
    let icon = rect.shrink(7.0);
    let color = if on {
        pal.danger
    } else if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    draw(ui.painter(), icon, color);
    if on {
        icons::slash(ui.painter(), icon, pal.danger, bg);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip)
}

fn truncate(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::simple_singleline(text.to_string(), font, Color32::PLACEHOLDER);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(1.0));
    ui.painter().layout_job(job)
}

/// Round avatar, or a coloured letter disc with the Images module off.
/// Same look as `app::paint_avatar`; kept local so this module doesn't
/// depend on app.rs (the integrator may swap it for the shared helper).
fn paint_avatar(
    ui: &egui::Ui,
    rect: Rect,
    user_id: &str,
    name: &str,
    url: &str,
    images: bool,
    pal: &Palette,
) {
    let size = rect.width();
    if images && !url.is_empty() {
        ui.painter()
            .circle_filled(rect.center(), size / 2.0, pal.hover);
        egui::Image::from_uri(url)
            .corner_radius(size / 2.0)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
    } else {
        let hue = user_id
            .bytes()
            .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let col = egui::ecolor::Hsva::new((hue % 360) as f32 / 360.0, 0.45, 0.6, 1.0);
        ui.painter().circle_filled(rect.center(), size / 2.0, col);
        let letter = name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            theme::bold_font(11.5),
            Color32::WHITE,
        );
    }
}
