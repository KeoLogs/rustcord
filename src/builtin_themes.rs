//! The four built-in themes: RustCord Dark (the default), RustCord Light,
//! Discord Dark and Discord Light. They are the only theme presets in the
//! binary, can't be deleted, and work with the Custom themes module off.
//! Every other theme is an imported `.rctheme` file (`themes.rs`).
//!
//! Each one is `.rctheme` text parsed with the same parser as imported
//! files, so a built-in can't drift from what a file can express. The two
//! Discord ones match `examples/themes/discord-classic` and
//! `examples/themes/discord-light` (checked by a test).
//!
//! `Config::builtin_theme` picks one; it is used whenever no imported theme
//! is applied (`Config::custom_theme` is `None` or the module is off).

use serde::{Deserialize, Serialize};

use crate::theme_file::{ThemeDef, parse_theme};

/// A built-in theme. Saved in `Config::builtin_theme`; older configs
/// without the key load as `RustcordDark`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinTheme {
    /// Blocky bordered panes with notch labels, Cascadia Mono, square
    /// shapes, Discord Dark colours. "Reset to default" comes back here.
    #[default]
    RustcordDark,
    /// The same blocky look in light colours.
    RustcordLight,
    /// The classic flat Discord look: Noto Sans, round shapes.
    DiscordDark,
    /// The classic flat Discord look in light colours.
    DiscordLight,
}

/// Every built-in theme, in the order the Themes page shows them.
pub const ALL: [BuiltinTheme; 4] = [
    BuiltinTheme::RustcordDark,
    BuiltinTheme::RustcordLight,
    BuiltinTheme::DiscordDark,
    BuiltinTheme::DiscordLight,
];

const RUSTCORD_DARK: &str = r##"format = 1

[meta]
name = "RustCord Dark"
author = "RustCord"
version = "1.0"
description = "The default look: bordered panes with labels, monospace, square corners."
base = "dark"

[style]
shape = "square"
avatar_shape = "square"
"##;

const RUSTCORD_LIGHT: &str = r##"format = 1

[meta]
name = "RustCord Light"
author = "RustCord"
version = "1.0"
description = "The bordered RustCord look in light colours."
base = "light"

[style]
shape = "square"
avatar_shape = "square"

[style.frame]
# Light panes need a darker line than `divider` to read as boxes.
border = "#c4c9cf"
label = "#5c5e66"
"##;

const DISCORD_DARK: &str = r##"format = 1

[meta]
name = "Discord Dark"
author = "RustCord"
version = "1.0"
description = "The classic flat Discord look: no borders, round avatars, Noto Sans."
base = "dark"

[style]
corner_scale = 1.0
font_family = "sans"
shape = "round"
avatar_shape = "round"

[style.frame]
enabled = false
"##;

const DISCORD_LIGHT: &str = r##"format = 1

[meta]
name = "Discord Light"
author = "RustCord"
version = "1.0"
description = "The classic flat Discord look in light colours."
base = "light"

[style]
corner_scale = 1.0
font_family = "sans"
shape = "round"
avatar_shape = "round"

[style.frame]
enabled = false
"##;

impl BuiltinTheme {
    /// The theme's `.rctheme` text.
    fn source(self) -> &'static str {
        match self {
            BuiltinTheme::RustcordDark => RUSTCORD_DARK,
            BuiltinTheme::RustcordLight => RUSTCORD_LIGHT,
            BuiltinTheme::DiscordDark => DISCORD_DARK,
            BuiltinTheme::DiscordLight => DISCORD_LIGHT,
        }
    }

    /// The parsed theme (a few hundred bytes of TOML; parsed when a theme
    /// is applied or the Themes page draws its cards).
    pub fn def(self) -> ThemeDef {
        // The sources are constants covered by tests; a parse error is a
        // bug in this file, so fall back to the plain default look.
        parse_theme(self.source()).unwrap_or_else(|_| ThemeDef {
            meta: crate::theme_file::ThemeMeta {
                name: self.name().to_string(),
                author: "RustCord".into(),
                version: String::new(),
                description: String::new(),
                base: crate::theme::ThemeChoice::DiscordDark,
            },
            palette: crate::theme::ThemeChoice::DiscordDark.palette(),
            style: crate::theme_file::ThemeStyle::default(),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BuiltinTheme::RustcordDark => "RustCord Dark",
            BuiltinTheme::RustcordLight => "RustCord Light",
            BuiltinTheme::DiscordDark => "Discord Dark",
            BuiltinTheme::DiscordLight => "Discord Light",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Shape, ThemeChoice, UiFont};
    use crate::theme_file::{THEME_FILE, ThemeStyle};
    use std::path::Path;

    #[test]
    fn exactly_four_and_they_parse() {
        assert_eq!(ALL.len(), 4);
        for t in ALL {
            let def = parse_theme(t.source()).unwrap_or_else(|e| panic!("{t:?}: {e}"));
            assert_eq!(def.meta.name, t.name());
            assert!(!def.meta.description.is_empty());
        }
        assert_eq!(BuiltinTheme::default(), BuiltinTheme::RustcordDark);
        // Distinct names.
        let mut names: Vec<_> = ALL.iter().map(|t| t.name()).collect();
        names.dedup();
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn rustcord_dark_is_the_default_look() {
        let d = BuiltinTheme::RustcordDark.def();
        assert_eq!(d.style, ThemeStyle::default());
        assert_eq!(
            format!("{:?}", d.palette),
            format!("{:?}", ThemeChoice::DiscordDark.palette())
        );
    }

    #[test]
    fn rustcord_themes_are_blocky_and_square() {
        for t in [BuiltinTheme::RustcordDark, BuiltinTheme::RustcordLight] {
            let s = t.def().style;
            assert!(s.frame.enabled && s.frame.labels, "{t:?}");
            assert_eq!(s.font, UiFont::Mono);
            assert!(s.corner_scale < 0.5);
            assert_eq!((s.shape, s.avatar_shape), (Shape::Square, Shape::Square));
        }
        assert!(!BuiltinTheme::RustcordLight.def().palette.dark);
        assert!(BuiltinTheme::RustcordDark.def().palette.dark);
    }

    #[test]
    fn discord_themes_are_flat_and_round() {
        for t in [BuiltinTheme::DiscordDark, BuiltinTheme::DiscordLight] {
            let s = t.def().style;
            assert!(!s.frame.enabled, "{t:?}");
            assert_eq!(s.font, UiFont::Sans);
            assert_eq!((s.shape, s.avatar_shape), (Shape::Round, Shape::Round));
        }
        assert!(!BuiltinTheme::DiscordLight.def().palette.dark);
    }

    /// The Discord built-ins look exactly like the example files.
    #[test]
    fn discord_themes_match_the_examples() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/themes");
        for (t, dir) in [
            (BuiltinTheme::DiscordDark, "discord-classic"),
            (BuiltinTheme::DiscordLight, "discord-light"),
        ] {
            let text = std::fs::read_to_string(root.join(dir).join(THEME_FILE)).unwrap();
            let file = parse_theme(&text).unwrap();
            let built = t.def();
            assert_eq!(built.style, file.style, "{dir}");
            assert_eq!(
                format!("{:?}", built.palette),
                format!("{:?}", file.palette),
                "{dir}"
            );
        }
    }
}
