//! Settings → Presets: the user's own presets as cards (name, layout
//! thumbnail, what Apply would change, Apply / Delete), "Save current as
//! preset", Import / Export (right-click Export for the copy options) and a
//! paste box for preset or theme code. Files and formats: `presets.rs`.
//!
//! The page saves, deletes and copies itself (small local files, the
//! clipboard). Applying, the file pickers and installing a pasted theme go
//! back to the settings page as a [`PresetsAction`].

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, vec2};
use egui_tiles::Tree;

use crate::config::Modules;
use crate::icons;
use crate::layout::Pane;
use crate::presets::{Notice, Pasted, Preset, PresetStore, PresetTheme, parse_code};
use crate::theme::{Bold, Palette};
use crate::widgets::{self, Tone};

/// What the page needs from the settings page.
#[derive(Debug)]
pub enum PresetsAction {
    /// Apply a preset to the draft (theme, modules; layout on Save).
    Apply(Box<Preset>),
    /// Open the file picker (backend), then `PresetStore::import`.
    PickFile,
    /// Save `text` to a file the user picks (backend).
    ExportFile { file_name: String, text: String },
    /// Install pasted theme code as a theme.
    SaveTheme(String),
}

/// What the user has now (the draft config and the live layout), for the
/// cards' "what changes" line.
pub struct Current<'a> {
    pub modules: &'a Modules,
    /// Applied theme id (`None` = the default look).
    pub theme: Option<&'a str>,
    pub tree: &'a Tree<Pane>,
    /// Is a theme with this id installed?
    pub installed: &'a dyn Fn(&str) -> bool,
    /// The current setup as a preset with this name (reads the theme file).
    pub capture: &'a dyn Fn(&str) -> Preset,
}

const CARD_W: f32 = 212.0;
const PREVIEW_H: f32 = 112.0;

/// UI state kept while Settings is open.
#[derive(Default)]
pub struct PresetsPage {
    /// Name for "Save current as preset".
    name: String,
    /// The paste box and what it holds.
    paste: String,
    pasted: Option<Result<Pasted, String>>,
}

impl PresetsPage {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        store: &mut PresetStore,
        now: &Current<'_>,
    ) -> Option<PresetsAction> {
        store.open();
        let mut action = None;
        let confirm_id = egui::Id::unique("presets_confirm_delete");

        ui.label(
            RichText::new(
                "A preset is your layout, theme and modules, saved together. Save your setup, \
                 share it as a .rcpreset file or code, and apply presets others share. Presets \
                 never hold your account, servers or channels.",
            )
            .color(pal.muted),
        );
        ui.add_space(8.0);

        // ---- Save current / Import / Export
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("Preset name")
                    .char_limit(crate::presets::MAX_NAME)
                    .desired_width(180.0),
            );
            if ui
                .add(pal.primary_button(&icons::label(icons::PLUS, "Save current as preset")))
                .on_hover_text("Saves the layout, theme and modules you have now")
                .clicked()
            {
                let p = (now.capture)(&self.name);
                if store.save(&p).is_ok() {
                    self.name.clear();
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(icons::label(icons::DOWNLOAD, "Import"))
                .on_hover_text("Import a .rcpreset file")
                .clicked()
            {
                action = Some(PresetsAction::PickFile);
            }
            let export = ui
                .button(icons::label(icons::UPLOAD, "Export"))
                .on_hover_text(
                    "Save your current setup as a .rcpreset file. Right-click to copy it as code.",
                );
            if export.clicked() {
                let p = (now.capture)(export_name(&self.name));
                action = Some(export_file(&p));
            }
            egui::Popup::context_menu(&export)
                .frame(widgets::menu_frame(pal))
                .show(|ui| {
                    if widgets::menu_item(
                        ui,
                        Some(icons::COPY),
                        "Copy preset code",
                        Tone::Normal,
                        pal,
                    )
                    .clicked()
                    {
                        let p = (now.capture)(export_name(&self.name));
                        ui.ctx().copy_text(p.code());
                        store.notice = Some(Notice::Ok("Preset code copied".into()));
                    }
                    if widgets::menu_item(
                        ui,
                        Some(icons::FILE_TEXT),
                        "Copy as JSON",
                        Tone::Normal,
                        pal,
                    )
                    .clicked()
                    {
                        let p = (now.capture)(export_name(&self.name));
                        ui.ctx().copy_text(p.export());
                        store.notice = Some(Notice::Ok("Preset JSON copied".into()));
                    }
                    let theme = (now.capture)("").theme.and_then(|t| t.file);
                    let tone = if theme.is_some() {
                        Tone::Normal
                    } else {
                        Tone::Disabled
                    };
                    let item =
                        widgets::menu_item(ui, Some(icons::PALETTE), "Copy theme code", tone, pal);
                    let item = if theme.is_none() {
                        item.on_hover_text("The default look, or a theme with an image")
                    } else {
                        item
                    };
                    if item.clicked()
                        && let Some(t) = theme
                    {
                        ui.ctx().copy_text(crate::presets::theme_code(&t));
                        store.notice = Some(Notice::Ok("Theme code copied".into()));
                    }
                });
            if ui
                .button(icons::RESET)
                .on_hover_text("Reload the presets folder")
                .clicked()
            {
                store.refresh();
            }
        });
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

        // ---- Cards
        ui.label(pal.section_label(&format!("Your presets — {}", store.entries.len())));
        ui.add_space(4.0);
        if store.entries.is_empty() {
            ui.label(
                RichText::new("No presets yet. Save your current setup or import one.")
                    .color(pal.muted),
            );
        }
        let per_row = cards_per_row(ui);
        ui.spacing_mut().item_spacing.y = 12.0;
        let mut copied = None;
        for row in store.entries.chunks(per_row) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                for e in row {
                    let p = e
                        .preset
                        .as_ref()
                        .map_err(|err| (e.id.as_str(), err.as_str()));
                    match card(ui, pal, p, now) {
                        Some(CardAction::Apply) => {
                            if let Ok(p) = &e.preset {
                                action = Some(PresetsAction::Apply(Box::new(p.clone())));
                            }
                        }
                        Some(CardAction::Delete) => {
                            ui.data_mut(|d| d.insert_temp(confirm_id, e.id.clone()));
                        }
                        Some(CardAction::Export) => {
                            if let Ok(p) = &e.preset {
                                action = Some(export_file(p));
                            }
                        }
                        Some(CardAction::CopyCode) => {
                            if let Ok(p) = &e.preset {
                                ui.ctx().copy_text(p.code());
                                copied = Some(Notice::Ok("Preset code copied".into()));
                            }
                        }
                        None => {}
                    }
                }
            });
        }
        if let Some(n) = copied {
            store.notice = Some(n);
        }

        // ---- Delete confirmation
        let pending: Option<String> = ui.data(|d| d.get_temp(confirm_id));
        if let Some(id) = pending {
            let name = store
                .get(&id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.clone());
            let mut close = false;
            let modal = egui::Modal::new(confirm_id.with("modal")).show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Delete preset").bold().size(20.0));
                ui.add_space(6.0);
                ui.label(format!(
                    "Delete \"{name}\"? Its file is removed from the presets folder."
                ));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let del =
                        egui::Button::new(RichText::new("Delete").color(Color32::WHITE).bold())
                            .fill(pal.danger);
                    if ui.add(del).clicked() {
                        let _ = store.delete(&id);
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

        // ---- Paste box
        ui.add_space(16.0);
        ui.label(pal.section_label("Paste preset or theme code"));
        ui.add_space(6.0);
        let edit = ui.add(
            egui::TextEdit::multiline(&mut self.paste)
                .hint_text("Paste rcpreset1:… / rctheme1:… code, preset JSON or a .rctheme file")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if edit.changed() {
            self.pasted = (!self.paste.trim().is_empty())
                .then(|| parse_code(&self.paste).map_err(|e| format!("{e:#}")));
        }
        let mut clear = false;
        match &self.pasted {
            Some(Ok(pasted)) => {
                let (what, has_theme) = match pasted {
                    Pasted::Preset(p) => (
                        format!("Preset \"{}\"", p.name),
                        p.theme.as_ref().is_some_and(|t| t.file.is_some()),
                    ),
                    Pasted::Theme { name, .. } => (format!("Theme \"{name}\""), true),
                };
                ui.label(RichText::new(icons::label(icons::CHECK, &what)).color(pal.online));
                ui.horizontal(|ui| {
                    if ui.add(pal.primary_button("Save as preset")).clicked() {
                        let p = match pasted {
                            Pasted::Preset(p) => (**p).clone(),
                            // A theme: your current layout and modules with it.
                            Pasted::Theme { text, name } => {
                                let mut p = (now.capture)(name);
                                p.theme = Some(PresetTheme {
                                    id: crate::themes::slug(name),
                                    name: name.clone(),
                                    file: Some(text.clone()),
                                });
                                p
                            }
                        };
                        clear = store.save(&p).is_ok();
                    }
                    let theme = ui
                        .add_enabled(has_theme, egui::Button::new("Save as theme"))
                        .on_disabled_hover_text(
                            "This preset has no theme file in it (it uses the default look, \
                             or a theme with a background image).",
                        );
                    if theme.clicked() {
                        let text = match pasted {
                            Pasted::Preset(p) => p.theme.as_ref().and_then(|t| t.file.clone()),
                            Pasted::Theme { text, .. } => Some(text.clone()),
                        };
                        if let Some(text) = text {
                            action = Some(PresetsAction::SaveTheme(text));
                            clear = true;
                        }
                    }
                });
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(pal.danger));
            }
            None => {}
        }
        if clear {
            self.paste.clear();
            self.pasted = None;
        }
        action
    }
}

fn export_name(name: &str) -> &str {
    if name.trim().is_empty() {
        "My setup"
    } else {
        name
    }
}

fn export_file(p: &Preset) -> PresetsAction {
    PresetsAction::ExportFile {
        file_name: format!(
            "{}.{}",
            crate::themes::slug(&p.name),
            crate::presets::EXTENSION
        ),
        text: p.export(),
    }
}

enum CardAction {
    Apply,
    Delete,
    Export,
    CopyCode,
}

fn cards_per_row(ui: &egui::Ui) -> usize {
    let step = CARD_W + 16.0 + 2.0 + 12.0;
    (((ui.available_width() + 12.0) / step).floor() as usize).max(1)
}

/// The "what Apply changes" lines of a card.
fn changes(p: &Preset, now: &Current<'_>) -> (String, String, Vec<&'static str>, bool) {
    let (theme, missing) = match &p.theme {
        None => ("Default look".to_string(), false),
        Some(t) => {
            let missing = t.file.is_none() && !(now.installed)(&t.id);
            (t.name.clone(), missing)
        }
    };
    let same_theme = match (&p.theme, now.theme) {
        (None, None) => true,
        (Some(t), Some(id)) => t.id == id,
        _ => false,
    };
    let theme_line = if same_theme {
        format!("Theme: {theme} (same)")
    } else if missing {
        format!("Theme: {theme} (not installed)")
    } else {
        format!("Theme: {theme}")
    };
    let layout_line = if p.tree == *now.tree {
        "Layout: same as now".to_string()
    } else {
        "Layout: changes".to_string()
    };
    (
        theme_line,
        layout_line,
        p.module_changes(now.modules),
        missing,
    )
}

fn card(
    ui: &mut egui::Ui,
    pal: &Palette,
    preset: Result<&Preset, (&str, &str)>,
    now: &Current<'_>,
) -> Option<CardAction> {
    let mut out = None;
    let mut menu_on: Vec<(egui::Response, bool)> = Vec::new();
    egui::Frame::new()
        .fill(pal.sidebar)
        .stroke(Stroke::new(1.0, pal.divider))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(CARD_W);
            ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
            ui.vertical(|ui| {
                let (rect, thumb) =
                    ui.allocate_exact_size(vec2(CARD_W, PREVIEW_H), egui::Sense::click());
                if preset.is_ok() {
                    // Right-click the thumbnail for Export / Copy.
                    menu_on.push((thumb, true));
                }
                match preset {
                    Ok(p) => {
                        // The layout in the preset's own theme colours when
                        // it carries the theme, else in the current ones.
                        let tp = p.palette();
                        crate::layout_presets::paint_preview(
                            ui.painter(),
                            tp.as_ref().unwrap_or(pal),
                            &p.tree,
                            rect,
                        );
                    }
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
                match preset {
                    Ok(p) => {
                        ui.add(
                            egui::Label::new(RichText::new(&p.name).bold().color(pal.strong))
                                .truncate(),
                        )
                        .on_hover_text(&p.description);
                        let (theme, layout, modules, missing) = changes(p, now);
                        let theme_col = if missing { pal.danger } else { pal.text };
                        ui.add(
                            egui::Label::new(RichText::new(theme).small().color(theme_col))
                                .truncate(),
                        )
                        .on_hover_text(if missing {
                            "This theme isn't installed and isn't in the preset; Apply uses the default look."
                        } else {
                            ""
                        });
                        ui.add(
                            egui::Label::new(RichText::new(layout).small().color(pal.text))
                                .truncate(),
                        );
                        let m = match modules.len() {
                            0 => "Modules: none change".to_string(),
                            1 => "1 module changes".to_string(),
                            n => format!("{n} modules change"),
                        };
                        let r = ui.add(
                            egui::Label::new(RichText::new(m).small().color(pal.text)).truncate(),
                        );
                        if !modules.is_empty() {
                            r.on_hover_text(modules_hover(p, &modules));
                        }
                    }
                    Err((id, err)) => {
                        ui.label(RichText::new(id).bold().color(pal.strong));
                        ui.add(
                            egui::Label::new(RichText::new(err).small().color(pal.danger))
                                .truncate(),
                        )
                        .on_hover_text(err);
                        ui.label(RichText::new(" ").small());
                        ui.label(RichText::new(" ").small());
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if preset.is_ok()
                        && ui
                            .add(pal.primary_button("Apply"))
                            .on_hover_text(
                                "Theme previews now; layout and modules change when you press \
                                 Save Changes",
                            )
                            .clicked()
                    {
                        out = Some(CardAction::Apply);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(RichText::new(icons::TRASH).color(pal.danger))
                            .on_hover_text("Delete preset")
                            .clicked()
                        {
                            out = Some(CardAction::Delete);
                        }
                        if preset.is_ok() {
                            let more = ui.button(icons::DOTS).on_hover_text("Export or copy");
                            menu_on.push((more, false));
                        }
                    });
                });
            });
        });
    for (resp, right_click) in menu_on {
        let popup = if right_click {
            egui::Popup::context_menu(&resp)
        } else {
            egui::Popup::menu(&resp)
        };
        popup.frame(widgets::menu_frame(pal)).show(|ui| {
            if widgets::menu_item(
                ui,
                Some(icons::UPLOAD),
                "Export to file…",
                Tone::Normal,
                pal,
            )
            .clicked()
            {
                out = Some(CardAction::Export);
            }
            if widgets::menu_item(ui, Some(icons::COPY), "Copy preset code", Tone::Normal, pal)
                .clicked()
            {
                out = Some(CardAction::CopyCode);
            }
        });
    }
    out
}

/// "Turns on: …" / "Turns off: …" for the modules tooltip.
fn modules_hover(p: &Preset, names: &[&'static str]) -> String {
    let mut on = Vec::new();
    let mut off = Vec::new();
    for m in crate::config::MODULES {
        if names.contains(&m.name) {
            let mut mods = p.modules.clone();
            if *(m.get)(&mut mods) {
                on.push(m.name);
            } else {
                off.push(m.name);
            }
        }
    }
    let mut s = String::new();
    if !on.is_empty() {
        s.push_str(&format!("Turns on: {}", on.join(", ")));
    }
    if !off.is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(&format!("Turns off: {}", off.join(", ")));
    }
    s
}
