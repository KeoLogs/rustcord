//! Settings → Layout: presets, the current layout's preview, and sharing a
//! layout as text. Everything works on the column model (`layout_v3`); the
//! page only returns a [`SettingsAction`] and the app applies it.

use eframe::egui::{self, RichText};

use crate::config::Config;
use crate::icons;
use crate::layout_presets;
use crate::layout_v3::{self, LayoutV3};
use crate::settings::SettingsAction;
use crate::theme::Palette;

/// The Layout page's own state (kept while Settings is open).
#[derive(Default)]
pub struct LayoutEditor {
    /// Import box: pasted text, and its checked layout or error.
    import_text: String,
    import: Option<Result<LayoutV3, String>>,
}

fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

impl LayoutEditor {
    /// Draw the page for the live `layout`.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        layout: &LayoutV3,
        config: &Config,
        pal: &Palette,
    ) -> Option<SettingsAction> {
        let mut action = None;
        hint(
            ui,
            pal,
            "RustCord's window is a row of columns: servers, channels, chat and \
             members. Only the chat stretches; the others keep their width and fold \
             into drawers when the window gets narrow.",
        );
        ui.add_space(10.0);
        if ui
            .button(icons::label(icons::RESET, "Reset to default"))
            .clicked()
        {
            action = Some(SettingsAction::ResetLayout);
        }
        if config.modules.layout_presets {
            ui.add_space(16.0);
            ui.label(pal.section_label("Presets"));
            ui.add_space(6.0);
            if let Some(p) = layout_presets::preset_picker(ui, pal, layout.detect()) {
                action = Some(SettingsAction::ApplyPreset(p));
            }
            ui.add_space(16.0);
            if let Some(a) = self.share(ui, layout, pal) {
                action = Some(a);
            }
        }
        action
    }

    /// Copy the layout as text, or paste one to import it (with a preview).
    fn share(
        &mut self,
        ui: &mut egui::Ui,
        _layout: &LayoutV3,
        pal: &Palette,
    ) -> Option<SettingsAction> {
        let mut action = None;
        ui.label(pal.section_label("Share a layout"));
        ui.add_space(6.0);
        if ui
            .button(icons::label(icons::COPY, "Copy layout"))
            .on_hover_text("Copy the current layout as text to share it")
            .clicked()
        {
            action = Some(SettingsAction::CopyLayout);
        }
        ui.add_space(6.0);
        let edit = ui.add(
            egui::TextEdit::multiline(&mut self.import_text)
                .hint_text("Paste a layout here to import it")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if edit.changed() {
            self.import = (!self.import_text.trim().is_empty())
                .then(|| layout_v3::import(&self.import_text).map_err(|e| format!("{e:#}")));
        }
        match &self.import {
            Some(Ok(l)) => {
                ui.add_space(6.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(240.0, 150.0), egui::Sense::hover());
                layout_presets::paint_preview(ui.painter(), pal, l, rect);
                ui.add_space(6.0);
                if ui
                    .add(pal.primary_button(&icons::label(icons::CHECK, "Use this layout")))
                    .clicked()
                {
                    action = Some(SettingsAction::ImportLayout(l.clone()));
                    self.import_text.clear();
                    self.import = None;
                }
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(pal.danger));
            }
            None => {}
        }
        action
    }
}
