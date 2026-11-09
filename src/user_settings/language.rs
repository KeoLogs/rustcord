//! Settings → Language: placeholder until RustCord has translations.
//! The choice is stored in `AppOptions::language`.

use eframe::egui::{self, RichText};

use super::options::AppOptions;
use super::widgets;
use super::{Action, Ctx};

/// (tag, name) of the languages RustCord can show. English only for now.
pub const LANGUAGES: &[(&str, &str)] = &[("en-US", "English, US")];

/// The language to use: the stored tag if supported, else English.
pub fn effective(tag: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(t, _)| t.eq_ignore_ascii_case(tag))
        .map_or(LANGUAGES[0].0, |(t, _)| t)
}

#[derive(Default)]
pub struct LanguagePage;

impl LanguagePage {
    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx, opts: &mut AppOptions) -> Option<Action> {
        let pal = c.pal;
        widgets::label(ui, pal, "SELECT A LANGUAGE");
        let current = effective(&opts.language);
        for (tag, name) in LANGUAGES {
            if ui.radio(current == *tag, *name).clicked() {
                opts.language = tag.to_string();
            }
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "RustCord is only available in English for now. Translations are planned; \
                 message content, server names and the like always show as written.",
            )
            .size(13.0)
            .color(pal.muted),
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_english() {
        assert_eq!(effective("en-us"), "en-US");
        assert_eq!(effective("fr"), "en-US");
        assert_eq!(effective(""), "en-US");
    }
}
