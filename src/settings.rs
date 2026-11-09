//! Full-window settings page, laid out like Discord's: a list of sections on
//! the left, the selected section on the right, ✕ / Esc to close.

use eframe::egui::{self, RichText, vec2};

use crate::config::{Config, MODULES};
use crate::theme::{Bold, Palette, ThemeChoice};
use crate::types::User;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Account,
    Appearance,
    Notifications,
    Privacy,
    Modules,
    Layout,
    Performance,
    About,
}

const SECTIONS: &[(&str, &[(Section, &str)])] = &[
    ("USER SETTINGS", &[(Section::Account, "My Account")]),
    (
        "APP SETTINGS",
        &[
            (Section::Appearance, "Appearance"),
            (Section::Notifications, "Notifications"),
            (Section::Privacy, "Privacy & Login"),
        ],
    ),
    (
        "RUSTCORD",
        &[
            (Section::Modules, "Modules"),
            (Section::Layout, "Layout"),
            (Section::Performance, "Performance"),
            (Section::About, "About"),
        ],
    ),
];

pub enum SettingsAction {
    Close,
    EditLayout,
    ResetLayout,
    LogOut,
}

#[derive(Default)]
pub struct SettingsPage {
    pub section: Section,
}

impl SettingsPage {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        me: Option<&User>,
        pal: &Palette,
    ) -> Option<SettingsAction> {
        let mut action = None;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(SettingsAction::Close);
        }

        egui::Panel::left("settings_nav")
            .exact_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 12,
                        top: 60,
                        bottom: 16,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (header, items) in SECTIONS {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(pal.section_label(header));
                        });
                        ui.add_space(2.0);
                        for (section, label) in *items {
                            let selected = self.section == *section;
                            if nav_item(ui, label, selected, pal.text, pal).clicked() {
                                self.section = *section;
                            }
                        }
                        ui.add_space(8.0);
                        let (r, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 1.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().hline(
                            r.shrink2(vec2(10.0, 0.0)).x_range(),
                            r.center().y,
                            egui::Stroke::new(1.0, pal.divider),
                        );
                        ui.add_space(8.0);
                    }
                    if nav_item(ui, "Log Out", false, pal.danger, pal).clicked() {
                        action = Some(SettingsAction::LogOut);
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.chat)
                    .inner_margin(egui::Margin {
                        left: 40,
                        right: 40,
                        top: 60,
                        bottom: 20,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let close = close_button(ui, pal);
                        if close.clicked() {
                            action = Some(SettingsAction::Close);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                            ui.heading(section_title(self.section));
                        });
                    });
                });
                ui.add_space(20.0);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.set_max_width(680.0);
                        ui.spacing_mut().item_spacing.y = 6.0;
                        if let Some(a) = self.section_ui(ui, config, me, pal) {
                            action = Some(a);
                        }
                    });
            });

        action
    }

    fn section_ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        me: Option<&User>,
        pal: &Palette,
    ) -> Option<SettingsAction> {
        let mut action = None;
        match self.section {
            Section::Account => account(ui, me, pal),
            Section::Appearance => {
                label(ui, pal, "THEME");
                ui.horizontal(|ui| {
                    for t in ThemeChoice::ALL {
                        ui.selectable_value(&mut config.theme, t, t.label());
                    }
                });
                ui.add_space(12.0);
                label(ui, pal, "UI SCALE");
                ui.add(
                    egui::Slider::new(&mut config.zoom, 0.75..=1.75)
                        .step_by(0.05)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.add_space(12.0);
                label(ui, pal, "MEMBER LIST");
                switch_row(
                    ui,
                    &mut config.compact_member_list,
                    "Compact member list",
                    "Smaller rows without the custom status / activity line.",
                    pal,
                );
                switch_row(
                    ui,
                    &mut config.bot_member_intents,
                    "Bots: load the full member list",
                    "Bot accounts only. Asks Discord for the Server Members and Presence \
                     intents, which you must first switch on for your bot in the Developer \
                     Portal (Bot → Privileged Gateway Intents). Takes effect next time you \
                     log in. Without it, bots show who's recently active.",
                    pal,
                );
            }
            Section::Notifications => notifications(ui, config, pal),
            Section::Privacy => {
                switch_row(
                    ui,
                    &mut config.remember_login,
                    "Remember me on this PC",
                    "Keeps your token in RustCord's settings file so you stay logged in. \
                     Turn off to log in each time.",
                    pal,
                );
                if !config.remember_login {
                    config.saved_token = None;
                }
                switch_row(
                    ui,
                    &mut config.warn_before_links,
                    "Warn before opening links",
                    "Asks before opening links from messages in your browser.",
                    pal,
                );
            }
            Section::Modules => {
                hint(
                    ui,
                    pal,
                    "Optional parts of RustCord. A module that's off doesn't run at all, \
                     which saves memory and CPU.",
                );
                ui.add_space(8.0);
                for m in MODULES {
                    egui::Frame::new()
                        .fill(pal.sidebar)
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    // Leave room for the switch on the right.
                                    ui.set_max_width(ui.available_width() - 60.0);
                                    ui.label(RichText::new(m.name).bold().color(pal.strong));
                                    ui.label(RichText::new(m.description).small().color(pal.muted));
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| toggle(ui, (m.get)(&mut config.modules), pal),
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
            }
            Section::Layout => {
                hint(
                    ui,
                    pal,
                    "Rearrange RustCord's panels. In edit mode, hover a panel to highlight \
                     it, then hold and drag it to a new spot: drop on the edge of another \
                     panel to dock next to it, or in the middle to stack them as tabs. \
                     Drag the gaps between panels to resize.",
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let edit = ui.add_enabled(
                        config.modules.layout_editor,
                        pal.primary_button("✏ Edit layout"),
                    );
                    if edit.clicked() {
                        action = Some(SettingsAction::EditLayout);
                    }
                    if ui.button("↺ Reset to default").clicked() {
                        action = Some(SettingsAction::ResetLayout);
                    }
                });
                if !config.modules.layout_editor {
                    hint(
                        ui,
                        pal,
                        "Turn on the Layout editor module to edit the layout.",
                    );
                }
            }
            Section::Performance => {
                if let Some(usage) = memory_stats::memory_stats() {
                    ui.label(
                        RichText::new(format!(
                            "RustCord is using {:.0} MB of RAM",
                            usage.physical_mem as f64 / 1_048_576.0
                        ))
                        .bold(),
                    );
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
                }
                ui.add_space(10.0);
                label(ui, pal, "MESSAGES KEPT PER CHANNEL");
                ui.add(egui::Slider::new(
                    &mut config.max_messages_per_channel,
                    50..=1000,
                ));
                hint(
                    ui,
                    pal,
                    "Older messages are dropped from memory (they're still on Discord).",
                );
                ui.add_space(8.0);
                label(ui, pal, "CHANNELS KEPT IN MEMORY");
                ui.add(egui::Slider::new(&mut config.cached_channels, 1..=30));
                hint(
                    ui,
                    pal,
                    "When you've visited more channels than this, the oldest one's messages \
                     and images are freed. Lower = less RAM, more reloading.",
                );
                ui.add_space(8.0);
                hint(
                    ui,
                    pal,
                    "Tip: turning off the Images module (Modules page) saves the most memory.",
                );
            }
            Section::About => {
                ui.label(
                    RichText::new(format!("RustCord {}", env!("CARGO_PKG_VERSION")))
                        .bold()
                        .size(16.0)
                        .color(pal.strong),
                );
                ui.label("A lightweight, customisable Discord client written in Rust.");
                ui.hyperlink_to(
                    "GitHub: KeoLogs/rustcord",
                    "https://github.com/KeoLogs/rustcord",
                );
                ui.add_space(8.0);
                hint(
                    ui,
                    pal,
                    "Licensed under GPL-3.0. Feature ideas from Discord Messenger by \
                     iProgramInCpp (MIT). Emoji graphics by Twemoji (CC-BY 4.0). Font: \
                     Noto Sans by the Noto Project Authors (SIL Open Font License 1.1).",
                );
                hint(
                    ui,
                    pal,
                    "Not affiliated with Discord. Third-party clients are against Discord's \
                     Terms of Service.",
                );
            }
        }
        action
    }
}

fn section_title(s: Section) -> &'static str {
    SECTIONS
        .iter()
        .flat_map(|(_, items)| items.iter())
        .find(|(sec, _)| *sec == s)
        .map(|(_, l)| *l)
        .unwrap_or("Settings")
}

fn account(ui: &mut egui::Ui, me: Option<&User>, pal: &Palette) {
    let Some(me) = me else {
        ui.label("Not connected yet.");
        return;
    };
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::from_uri(&me.avatar_url)
                        .fit_to_exact_size(vec2(72.0, 72.0))
                        .corner_radius(36),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(&me.name).size(20.0).bold().color(pal.strong));
                    if !me.username.is_empty() {
                        ui.label(RichText::new(format!("@{}", me.username)).color(pal.muted));
                    }
                    ui.label(
                        RichText::new(if me.bot {
                            "Bot account"
                        } else {
                            "User account"
                        })
                        .small()
                        .color(pal.muted),
                    );
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(pal.section_label("User ID"));
                ui.label(&me.id);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(me.id.clone());
                }
            });
        });
}

fn notifications(ui: &mut egui::Ui, config: &mut Config, pal: &Palette) {
    if !config.modules.notifications {
        hint(
            ui,
            pal,
            "The Notifications module is off, so RustCord shows no desktop notifications \
             and keeps no mentions inbox. Unread markers and mention badges still work.",
        );
        ui.add_space(8.0);
        if ui
            .add(pal.primary_button("Turn on Notifications module"))
            .clicked()
        {
            config.modules.notifications = true;
        }
        return;
    }
    let row = |ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str| {
        egui::Frame::new()
            .fill(pal.sidebar)
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        // Leave room for the switch on the right.
                        ui.set_max_width(ui.available_width() - 60.0);
                        ui.label(RichText::new(title).bold().color(pal.strong));
                        ui.label(RichText::new(desc).small().color(pal.muted));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        toggle(ui, on, pal)
                    });
                });
            });
        ui.add_space(6.0);
    };
    row(
        ui,
        &mut config.desktop_notifications,
        "Enable desktop notifications",
        "Pop up a notification for mentions and DMs (and, in servers set to \
         \"All messages\", every message) when the window isn't focused or the \
         channel isn't open. Muted servers and channels never notify.",
    );
    row(
        ui,
        &mut config.flash_taskbar,
        "Flash taskbar",
        "Flash RustCord's taskbar / dock icon when a notification arrives and the \
         window isn't focused.",
    );
    ui.add_space(6.0);
    hint(
        ui,
        pal,
        "Notification levels and mutes come from your Discord account (set them in \
         the official client for now). Bot accounts only notify on mentions and DMs. \
         Recent mentions are listed under 🔔 in the chat header.",
    );
}

fn label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.label(pal.section_label(text));
}

/// Discord's settings row: title and switch on one line, a muted
/// description under it, then a divider.
fn switch_row(ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str, pal: &Palette) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(15.0).color(pal.strong));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            toggle(ui, on, pal)
        });
    });
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width() - 60.0);
        hint(ui, pal, desc);
    });
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        r.x_range(),
        r.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
    ui.add_space(10.0);
}

/// A settings sidebar entry: hover / selected backgrounds like Discord's.
fn nav_item(
    ui: &mut egui::Ui,
    text: &str,
    selected: bool,
    color: egui::Color32,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let col = if selected || resp.hovered() {
        if color == pal.text { pal.strong } else { color }
    } else if color == pal.text {
        pal.muted
    } else {
        color
    };
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(15.0),
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Discord's round ✕ with "ESC" under it.
fn close_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 54.0), egui::Sense::click());
    let c = egui::pos2(rect.center().x, rect.top() + 18.0);
    let col = if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    ui.painter()
        .circle_stroke(c, 17.0, egui::Stroke::new(2.0, col));
    ui.painter().text(
        c,
        egui::Align2::CENTER_CENTER,
        "✕",
        egui::FontId::proportional(16.0),
        col,
    );
    ui.painter().text(
        egui::pos2(c.x, rect.bottom() - 6.0),
        egui::Align2::CENTER_CENTER,
        "ESC",
        crate::theme::bold_font(11.5),
        pal.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close (Esc)")
}

fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

/// iOS/Discord-style on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off_col = egui::Color32::from_gray(if pal.dark { 0x50 } else { 0xb0 });
    let bg = lerp_color(off_col, pal.online, t);
    ui.painter().rect_filled(rect, 11, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, egui::Color32::WHITE);
    resp
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
