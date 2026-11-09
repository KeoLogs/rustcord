//! Settings → Themes: a grid of theme cards (mini preview, name, author,
//! Active badge, Apply / Delete), "Import theme…" and "Open themes folder".
//!
//! Pure UI: it returns a `ThemesAction`; the app applies themes, deletes
//! (after the confirmation shown here) and asks the backend for the file
//! picker.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, vec2};

use crate::theme::{Bold, Palette, ThemeChoice};
use crate::theme_file::{ThemeDef, ThemeStyle};
use crate::themes::{Notice, ThemeStore, paint_preview};

/// What the user asked for on the Themes page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemesAction {
    /// Use a built-in theme (set `Config::theme`, clear `custom_theme`).
    ApplyBuiltin(ThemeChoice),
    /// Use an installed theme (set `Config::custom_theme`).
    Apply(String),
    /// Delete an installed theme. Already confirmed by the user.
    Delete(String),
    /// Open the file picker (`themes::pick_import_source`) in the backend,
    /// then `ThemeStore::import` the result.
    Import { folder: bool },
    /// `themes::open_folder(store.dir())`.
    OpenFolder,
}

const CARD_W: f32 = 212.0;
const PREVIEW_H: f32 = 112.0;

pub struct ThemesPage;

impl ThemesPage {
    pub fn show(ui: &mut egui::Ui, store: &mut ThemeStore, pal: &Palette) -> Option<ThemesAction> {
        let mut action = None;
        let confirm_id = egui::Id::unique("themes_confirm_delete");

        ui.label(
            RichText::new(
                "Themes change RustCord's colours and look. Import a .rctheme file, a theme \
                 folder or a .zip — RustCord doesn't ship any.",
            )
            .color(pal.muted),
        );
        ui.add_space(8.0);

        if store.enabled() {
            ui.horizontal(|ui| {
                if ui
                    .add(pal.primary_button("Import theme…"))
                    .on_hover_text("A .rctheme file or a .zip")
                    .clicked()
                {
                    action = Some(ThemesAction::Import { folder: false });
                }
                if ui.button("Import folder…").clicked() {
                    action = Some(ThemesAction::Import { folder: true });
                }
                if store.dir().is_some() && ui.button("📂 Open themes folder").clicked() {
                    action = Some(ThemesAction::OpenFolder);
                }
                if ui
                    .button("⟳")
                    .on_hover_text("Reload the themes folder")
                    .clicked()
                {
                    store.refresh();
                }
            });
        } else {
            ui.label(
                RichText::new("Custom themes are off (Settings → Modules). Only the built-in themes are available.")
                    .color(pal.muted),
            );
        }
        match &store.notice {
            Some(Notice::Ok(m)) => {
                ui.label(RichText::new(m).color(pal.online));
            }
            Some(Notice::Err(m)) => {
                ui.label(RichText::new(m).color(pal.danger));
            }
            None => {}
        }
        ui.add_space(12.0);

        // ---- Built-in
        ui.label(pal.section_label("Built-in"));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
            for choice in ThemeChoice::ALL {
                let active = store.active_custom.is_none() && store.active_builtin == choice;
                let def = ThemeDef {
                    meta: crate::theme_file::ThemeMeta {
                        name: format!("Discord {}", choice.label()),
                        author: "RustCord".into(),
                        version: String::new(),
                        description: match choice {
                            ThemeChoice::DiscordDark => "Discord's dark look (default).".into(),
                            ThemeChoice::DiscordLight => "Discord's light look.".into(),
                        },
                        base: choice,
                    },
                    palette: choice.palette(),
                    style: ThemeStyle::default(),
                };
                if let Some(a) = card(ui, pal, Ok(&def), active, false) {
                    match a {
                        CardAction::Apply => action = Some(ThemesAction::ApplyBuiltin(choice)),
                        CardAction::Delete => {}
                    }
                }
            }
        });

        // ---- Installed
        if store.enabled() {
            ui.add_space(14.0);
            ui.label(pal.section_label(&format!("Installed — {}", store.entries.len())));
            ui.add_space(4.0);
            if store.entries.is_empty() {
                ui.label(
                    RichText::new("No themes yet. Import one to see it here.").color(pal.muted),
                );
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                for e in &store.entries {
                    let active = store.active_custom.as_deref() == Some(e.id.as_str());
                    let theme = e
                        .theme
                        .as_ref()
                        .map_err(|err| (e.id.as_str(), err.as_str()));
                    match card(ui, pal, theme, active, true) {
                        Some(CardAction::Apply) => action = Some(ThemesAction::Apply(e.id.clone())),
                        Some(CardAction::Delete) => {
                            ui.data_mut(|d| d.insert_temp(confirm_id, e.id.clone()));
                        }
                        None => {}
                    }
                }
            });
        }

        // ---- Delete confirmation
        let pending: Option<String> = ui.data(|d| d.get_temp(confirm_id));
        if let Some(id) = pending {
            let name = store
                .get(&id)
                .map(|t| t.meta.name.clone())
                .unwrap_or_else(|| id.clone());
            let mut close = false;
            let modal = egui::Modal::new(confirm_id.with("modal")).show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Delete theme").bold().size(20.0));
                ui.add_space(6.0);
                ui.label(format!(
                    "Delete \"{name}\"? Its files are removed from the themes folder."
                ));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let del =
                        egui::Button::new(RichText::new("Delete").color(Color32::WHITE).bold())
                            .fill(pal.danger);
                    if ui.add(del).clicked() {
                        action = Some(ThemesAction::Delete(id.clone()));
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                ui.data_mut(|d| d.remove::<String>(confirm_id));
            }
        }

        action
    }
}

enum CardAction {
    Apply,
    Delete,
}

/// One theme card. `theme` is `Err((id, error))` for a broken theme file.
fn card(
    ui: &mut egui::Ui,
    pal: &Palette,
    theme: Result<&ThemeDef, (&str, &str)>,
    active: bool,
    deletable: bool,
) -> Option<CardAction> {
    let mut out = None;
    let stroke = if active {
        Stroke::new(2.0, pal.accent)
    } else {
        Stroke::new(1.0, pal.divider)
    };
    egui::Frame::new()
        .fill(pal.sidebar)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(CARD_W);
            ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
            ui.vertical(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(CARD_W, PREVIEW_H), egui::Sense::hover());
                match theme {
                    Ok(t) => paint_preview(ui.painter(), rect, &t.palette, &t.style),
                    Err(_) => {
                        ui.painter().rect_filled(rect, 4, pal.servers);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "⚠",
                            egui::FontId::proportional(20.0),
                            pal.danger,
                        );
                    }
                }
                ui.add_space(4.0);
                match theme {
                    Ok(t) => {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&t.meta.name).bold().color(pal.strong),
                                )
                                .truncate(),
                            );
                        });
                        let mut by = if t.meta.author.is_empty() {
                            String::from("Unknown author")
                        } else {
                            format!("by {}", t.meta.author)
                        };
                        if !t.meta.version.is_empty() {
                            by.push_str(&format!(" · v{}", t.meta.version));
                        }
                        ui.add(
                            egui::Label::new(RichText::new(by).small().color(pal.muted)).truncate(),
                        );
                        let desc = if t.meta.description.is_empty() {
                            " "
                        } else {
                            &t.meta.description
                        };
                        ui.add(
                            egui::Label::new(RichText::new(desc).small().color(pal.text))
                                .truncate(),
                        )
                        .on_hover_text(&t.meta.description);
                    }
                    Err((id, err)) => {
                        ui.label(RichText::new(id).bold().color(pal.strong));
                        ui.add(
                            egui::Label::new(RichText::new(err).small().color(pal.danger))
                                .truncate(),
                        )
                        .on_hover_text(err);
                        ui.label(RichText::new(" ").small());
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if active {
                        egui::Frame::new()
                            .fill(pal.accent.gamma_multiply(0.18))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(RichText::new("✔ Active").color(pal.accent).bold());
                            });
                    } else if theme.is_ok() && ui.add(pal.primary_button("Apply")).clicked() {
                        out = Some(CardAction::Apply);
                    }
                    if deletable {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(RichText::new("🗑").color(pal.danger))
                                .on_hover_text("Delete theme")
                                .clicked()
                            {
                                out = Some(CardAction::Delete);
                            }
                        });
                    }
                });
            });
        });
    out
}
