//! Settings → Themes: a grid of installed theme cards (mini preview, name,
//! author, Active badge, Apply / Delete), "Import theme…", "Open themes
//! folder" and "Reset to default". RustCord has one built-in look (Discord
//! Dark); every other theme, Discord Light included, is imported from a
//! `.rctheme` file (examples in `examples/themes/`).
//!
//! Pure UI: it returns a `ThemesAction`; the app applies themes, deletes
//! (after the confirmation shown here) and asks the backend for the file
//! picker.

use crate::icons;
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, vec2};

use crate::theme::{Bold, Palette};
use crate::theme_file::ThemeDef;
use crate::themes::{Notice, ThemeStore, paint_preview};

/// What the user asked for on the Themes page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemesAction {
    /// Back to the default look (clear `Config::custom_theme`).
    ResetDefault,
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
                "Themes change RustCord's colours and look. The default look is Discord Dark. \
                 Import a .rctheme file, a theme folder or a .zip; Discord Light and other \
                 example themes are in the examples/themes folder of RustCord's source.",
            )
            .color(pal.muted),
        );
        ui.add_space(8.0);

        ui.horizontal_wrapped(|ui| {
            if store.enabled() {
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
                if store.dir().is_some()
                    && ui
                        .button(icons::label(icons::FOLDER, "Open themes folder"))
                        .clicked()
                {
                    action = Some(ThemesAction::OpenFolder);
                }
                if ui
                    .button(icons::RESET)
                    .on_hover_text("Reload the themes folder")
                    .clicked()
                {
                    store.refresh();
                }
            }
            // A custom theme is only ever applied while the module is on,
            // so with it off the default look is already active.
            if ui
                .add_enabled(
                    store.active_custom.is_some(),
                    egui::Button::new(icons::label(icons::RESET, "Reset to default")),
                )
                .on_hover_text("Back to the default look (Discord Dark)")
                .on_disabled_hover_text("The default look is already applied")
                .clicked()
            {
                action = Some(ThemesAction::ResetDefault);
            }
        });
        if !store.enabled() {
            ui.label(
                RichText::new(
                    "Custom themes are off (Settings → Modules). Only the default look is used.",
                )
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

        // Rows of cards (frames can't wrap themselves in egui).
        let per_row = cards_per_row(ui);
        ui.spacing_mut().item_spacing.y = 12.0;

        // ---- Installed
        if store.enabled() {
            ui.label(pal.section_label(&format!("Installed — {}", store.entries.len())));
            ui.add_space(4.0);
            if store.entries.is_empty() {
                ui.label(
                    RichText::new("No themes yet. Import one to see it here.").color(pal.muted),
                );
            }
            for row in store.entries.chunks(per_row) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                    for e in row {
                        let active = store.active_custom.as_deref() == Some(e.id.as_str());
                        let theme = e
                            .theme
                            .as_ref()
                            .map_err(|err| (e.id.as_str(), err.as_str()));
                        match card(ui, pal, theme, active, true) {
                            Some(CardAction::Apply) => {
                                action = Some(ThemesAction::Apply(e.id.clone()))
                            }
                            Some(CardAction::Delete) => {
                                ui.data_mut(|d| d.insert_temp(confirm_id, e.id.clone()));
                            }
                            None => {}
                        }
                    }
                });
            }
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
/// How many theme cards fit side by side (card + margins + gap).
fn cards_per_row(ui: &egui::Ui) -> usize {
    let step = CARD_W + 16.0 + 2.0 + 12.0;
    (((ui.available_width() + 12.0) / step).floor() as usize).max(1)
}

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
                            icons::WARNING,
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
                                ui.label(
                                    RichText::new(icons::label(icons::CHECK, "Active"))
                                        .color(pal.accent)
                                        .bold(),
                                );
                            });
                    } else if theme.is_ok() && ui.add(pal.primary_button("Apply")).clicked() {
                        out = Some(CardAction::Apply);
                    }
                    if deletable {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(RichText::new(icons::TRASH).color(pal.danger))
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
