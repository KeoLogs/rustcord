//! Discord-style colours. Each panel gets its own background like Discord
//! (darkest server list, slightly lighter channel list, lightest chat).

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

use crate::layout::Pane;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    #[default]
    DiscordDark,
    DiscordLight,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 2] = [ThemeChoice::DiscordDark, ThemeChoice::DiscordLight];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::DiscordDark => "Dark",
            ThemeChoice::DiscordLight => "Light",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            ThemeChoice::DiscordDark => Palette {
                servers: hex(0x1e1f22),
                sidebar: hex(0x2b2d31),
                chat: hex(0x313338),
                input: hex(0x383a40),
                hover: hex(0x35373c),
                selected: hex(0x404249),
                text: hex(0xdbdee1),
                muted: hex(0x949ba4),
                accent: hex(0x5865f2),
                online: hex(0x23a55a),
                danger: hex(0xda373c),
                mention: hex(0xf23f43),
                unread: hex(0xf2f3f5),
                dark: true,
            },
            ThemeChoice::DiscordLight => Palette {
                servers: hex(0xe3e5e8),
                sidebar: hex(0xf2f3f5),
                chat: hex(0xffffff),
                input: hex(0xebedef),
                hover: hex(0xe0e1e5),
                selected: hex(0xd4d7dc),
                text: hex(0x313338),
                muted: hex(0x5c5e66),
                accent: hex(0x5865f2),
                online: hex(0x23a55a),
                danger: hex(0xda373c),
                mention: hex(0xf23f43),
                unread: hex(0x060607),
                dark: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub servers: Color32,
    pub sidebar: Color32,
    pub chat: Color32,
    pub input: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub online: Color32,
    pub danger: Color32,
    /// Red mention-count badge.
    pub mention: Color32,
    /// Unread channel names and the unread pill (white in dark theme).
    pub unread: Color32,
    pub dark: bool,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Palette {
    /// Background for each panel.
    pub fn pane_fill(&self, pane: Pane) -> Color32 {
        match pane {
            Pane::Servers => self.servers,
            Pane::Channels | Pane::Members => self.sidebar,
            Pane::Profile => self.servers,
            Pane::Messages | Pane::Composer => self.chat,
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.panel_fill = self.chat;
        v.window_fill = self.sidebar;
        v.extreme_bg_color = self.input; // text edit background
        v.faint_bg_color = self.hover;
        v.override_text_color = Some(self.text);
        v.hyperlink_color = hex(0x00a8fc);
        v.selection.bg_fill = self.accent;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);

        let r = CornerRadius::same(4);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
            w.bg_stroke = Stroke::NONE;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        v.widgets.inactive.bg_fill = self.input;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.muted);
        v.widgets.hovered.weak_bg_fill = self.hover;
        v.widgets.hovered.bg_fill = self.hover;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.active.weak_bg_fill = self.selected;
        v.widgets.active.bg_fill = self.selected;
        v.widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.open.weak_bg_fill = self.selected;

        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
            s.spacing.button_padding = egui::vec2(8.0, 4.0);
        });
    }

    /// Muted channels / servers: greyed out.
    pub fn muted_text(&self) -> Color32 {
        self.muted.gamma_multiply(0.55)
    }

    /// A filled "primary" button in Discord blurple.
    pub fn primary_button(&self, text: &str) -> egui::Button<'static> {
        egui::Button::new(egui::RichText::new(text.to_string()).color(Color32::WHITE))
            .fill(self.accent)
            .corner_radius(4)
    }
}
