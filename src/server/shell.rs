//! The full-window settings layout shared by Server Settings and Channel
//! Settings: a sidebar of grouped entries on the left, the page on the
//! right with a title and the round close button, and the floating
//! unsaved-changes bar. Like Discord, leaving a page with unsaved changes
//! is refused and the bar flashes red instead.

use eframe::egui::{self, RichText, vec2};

use super::widgets::{self, BarAction};
use crate::theme::Palette;

/// One sidebar entry.
pub struct NavItem<K> {
    pub key: K,
    pub label: String,
    /// Drawn in the danger colour ("Delete Channel").
    pub danger: bool,
}

/// A sidebar group with its small uppercase header (may be empty).
pub struct NavGroup<K> {
    pub header: String,
    pub items: Vec<NavItem<K>>,
}

impl<K> NavGroup<K> {
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, key: K, label: impl Into<String>) -> Self {
        self.items.push(NavItem {
            key,
            label: label.into(),
            danger: false,
        });
        self
    }

    pub fn danger(mut self, key: K, label: impl Into<String>) -> Self {
        self.items.push(NavItem {
            key,
            label: label.into(),
            danger: true,
        });
        self
    }
}

/// What happened in the shell this frame.
pub struct ShellOutput<K> {
    /// A sidebar entry was clicked (and leaving was allowed).
    pub nav: Option<K>,
    /// ✕ / Esc (and leaving was allowed).
    pub close: bool,
    pub bar: Option<BarAction>,
}

/// Flash state for the unsaved bar (time until which it stays red).
#[derive(Debug, Clone, Copy, Default)]
pub struct Flash(pub f64);

impl Flash {
    pub fn trigger(&mut self, now: f64) {
        self.0 = now + 0.9;
    }
}

/// Draw the shell. `dirty` blocks navigation and closing; `body` draws the
/// selected page inside a scroll area.
#[allow(clippy::too_many_arguments)]
pub fn show<K: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    pal: &Palette,
    groups: &[NavGroup<K>],
    selected: K,
    title: &str,
    dirty: bool,
    saving: bool,
    flash: &mut Flash,
    body: impl FnOnce(&mut egui::Ui),
) -> ShellOutput<K> {
    let mut nav = None;
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let now = ui.input(|i| i.time);

    egui::Panel::left(egui::Id::unique((id, "nav")))
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
            egui::ScrollArea::vertical()
                .id_salt((id, "nav_scroll"))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (gi, g) in groups.iter().enumerate() {
                        if gi > 0 {
                            widgets::divider(ui, pal);
                        }
                        if !g.header.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                ui.add(egui::Label::new(pal.section_label(&g.header)).truncate());
                            });
                            ui.add_space(2.0);
                        }
                        for item in &g.items {
                            let color = if item.danger { pal.danger } else { pal.text };
                            if widgets::nav_item(ui, &item.label, item.key == selected, color, pal)
                                .clicked()
                            {
                                nav = Some(item.key);
                            }
                        }
                    }
                });
        });

    let mut bar = None;
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
            let area = ui.max_rect().expand2(vec2(40.0, 20.0));
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::close_button(ui, pal).clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        ui.label(RichText::new(title).heading().color(pal.strong));
                    });
                });
            });
            ui.add_space(16.0);
            egui::ScrollArea::vertical()
                .id_salt((id, "page_scroll"))
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.set_max_width(740.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    body(ui);
                    // Room so the floating bar never covers the last row.
                    ui.add_space(if dirty { 90.0 } else { 20.0 });
                });
            if dirty {
                bar = widgets::unsaved_bar(ui, area, flash.0 > now, saving, pal);
                if flash.0 > now {
                    ui.ctx().request_repaint();
                }
            }
        });

    if dirty && (close || nav.is_some_and(|k| k != selected)) {
        flash.trigger(now);
        close = false;
        nav = None;
    }
    ShellOutput { nav, close, bar }
}
