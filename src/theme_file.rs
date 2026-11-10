//! The `.rctheme` file format: a small TOML file that recolours and restyles
//! RustCord.
//!
//! ```toml
//! format = 1                      # required; this build reads format 1
//!
//! [meta]
//! name = "My theme"               # required
//! author = "me"
//! version = "1.0"
//! description = "One line about it"
//! base = "light"                  # "dark" (default) or "light"
//!
//! [colors]                        # any `Palette` field, #RRGGBB or #RRGGBBAA;
//! accent = "#2f7fd8"              # missing ones come from the base theme
//! link = "#0066cc"                # extra: hyperlink colour
//!
//! [style]
//! corner_radius = { widgets = 6, windows = 12, menus = 8 }
//! window_shadow = { offset = [0, 6], blur = 24, spread = 0, color = "#00000040" }
//! popup_shadow = { offset = [0, 4], blur = 16, color = "#00000030" }
//! panel_shadow = { offset = [0, 2], blur = 10, color = "#00000030" }  # inset panes
//! font_scale = 1.0                # 0.8 – 1.25
//! glass_opacity = 0.6             # fill opacity of `glass` panes
//! widget_border = "#8faacc"       # outline buttons / inputs / checkboxes
//! backdrop = { top = "#..", mid = "#..", bottom = "#.." }  # behind glass / inset panes
//! accent_gradient = { top = "#..", bottom = "#.." }        # primary buttons
//! corner_scale = 0.25             # 0 – 2, multiplies every corner radius
//! font_family = "mono"            # "mono" (Cascadia Mono, default) or "sans" (Noto Sans)
//!
//! [style.frame]                   # bordered panes (the default look)
//! enabled = true                  # false = flat panes, no borders or gaps
//! border = "#3f4147"              # default: [colors] divider
//! label = "#949ba4"               # notch label colour; default: [colors] muted
//! labels = true                   # pane names cut into the top border
//! gap = 8                         # 0 – 24 px between panes
//!
//! [style.panes.messages]          # servers, profile, channels, messages, composer, members
//! gradient = { top = "#..", bottom = "#.." }   # or: fill = "#.."
//! glass = true                    # translucent fill + light top highlight
//! corner_radius = 10
//! inset = 4                       # gap around the pane (shows the backdrop)
//!
//! [images]
//! chat_background = "bubbles.png" # relative to the theme folder, png/jpg, <= 4 MB
//! opacity = 0.3
//! tint = "#ffffff"
//! fit = "cover"                   # or "tile"
//! ```
//!
//! Parsing never panics: every problem is an error message that names the key
//! (and the line, for TOML mistakes).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use eframe::egui::{Color32, Shadow};
use serde::Deserialize;

use crate::layout::Pane;
use crate::theme::{Palette, ThemeChoice, UiFont};

/// Theme format this build reads.
pub const FORMAT: i64 = 1;
/// File name inside a theme folder.
pub const THEME_FILE: &str = "theme.rctheme";
/// Largest theme file accepted.
pub const MAX_THEME_FILE: u64 = 64 * 1024;
/// Largest background image accepted.
pub const MAX_IMAGE_FILE: u64 = 4 * 1024 * 1024;

/// Pane names used in `[style.panes.<name>]`, in `ThemeStyle::panes` order.
pub const PANE_KEYS: [&str; 6] = [
    "servers", "profile", "channels", "messages", "composer", "members",
];

/// Colour names accepted in `[colors]` (every `Palette` colour + `link`).
pub const COLOR_KEYS: [&str; 21] = [
    "servers",
    "sidebar",
    "chat",
    "input",
    "hover",
    "selected",
    "text",
    "muted",
    "accent",
    "online",
    "idle",
    "dnd",
    "offline",
    "danger",
    "mention",
    "unread",
    "strong",
    "msg_hover",
    "divider",
    "rail_icon",
    "link",
];

/// Index of `pane` in `ThemeStyle::panes` / `PANE_KEYS`.
pub fn pane_index(pane: Pane) -> Option<usize> {
    // An if-chain (not a match) so new `Pane` variants don't break this.
    let order = [
        Pane::Servers,
        Pane::Profile,
        Pane::Channels,
        Pane::Messages,
        Pane::Composer,
        Pane::Members,
    ];
    order.iter().position(|p| *p == pane)
}

/// A parsed, validated theme.
#[derive(Clone, Debug)]
pub struct ThemeDef {
    pub meta: ThemeMeta,
    pub palette: Palette,
    pub style: ThemeStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeMeta {
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub base: ThemeChoice,
}

/// A vertical gradient (optional middle stop at 50%).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    pub top: Color32,
    pub mid: Option<Color32>,
    pub bottom: Color32,
}

impl Gradient {
    pub fn flat(c: Color32) -> Self {
        Self {
            top: c,
            mid: None,
            bottom: c,
        }
    }

    /// Colour at `t` (0 = top, 1 = bottom).
    pub fn at(&self, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        match self.mid {
            None => lerp(self.top, self.bottom, t),
            Some(m) if t < 0.5 => lerp(self.top, m, t * 2.0),
            Some(m) => lerp(m, self.bottom, (t - 0.5) * 2.0),
        }
    }
}

/// Lerp in unmultiplied sRGB (no dark fringes between translucent stops).
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let a = a.to_srgba_unmultiplied();
    let b = b.to_srgba_unmultiplied();
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(0), m(1), m(2), m(3))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneStyle {
    /// Background gradient; `None` = the palette's pane colour.
    pub gradient: Option<Gradient>,
    pub glass: bool,
    pub corner_radius: u8,
    pub inset: u8,
}

impl PaneStyle {
    /// True when the pane needs more than a flat `Frame` fill.
    pub fn is_custom(&self) -> bool {
        self.gradient.is_some() || self.glass || self.corner_radius > 0 || self.inset > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImageFit {
    #[default]
    Cover,
    Tile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageSpec {
    /// Relative to the theme folder; validated (no `..`, not absolute).
    pub path: PathBuf,
    pub opacity: f32,
    pub tint: Color32,
    pub fit: ImageFit,
}

/// Everything a theme can change beyond the colours.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeStyle {
    pub widget_radius: u8,
    pub window_radius: u8,
    pub menu_radius: u8,
    /// `None` = what `Palette::apply` sets.
    pub window_shadow: Option<Shadow>,
    pub popup_shadow: Option<Shadow>,
    /// Shadow under panes with an `inset`.
    pub panel_shadow: Option<Shadow>,
    pub font_scale: f32,
    pub glass_opacity: f32,
    pub backdrop: Option<Gradient>,
    pub accent_gradient: Option<Gradient>,
    pub link: Option<Color32>,
    /// Outline for interactive widgets (none in the Discord look).
    pub widget_border: Option<Color32>,
    pub panes: [PaneStyle; 6],
    pub image: Option<ImageSpec>,
    /// Multiplies every corner radius (widgets, windows, menus, panes).
    pub corner_scale: f32,
    /// Bordered panes with notched labels.
    pub frame: PaneFrame,
    /// The UI font.
    pub font: UiFont,
}

/// The blocky look: each pane is a box with a 1 px border, its name cut
/// into the top border, and small even gaps between the boxes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneFrame {
    pub enabled: bool,
    /// Border colour; `None` = the palette's `divider`.
    pub border: Option<Color32>,
    /// Label colour; `None` = the palette's `muted`.
    pub label: Option<Color32>,
    /// Show the pane names in the top border.
    pub labels: bool,
    /// Space between two panes (half of it around each pane).
    pub gap: u8,
}

impl Default for PaneFrame {
    fn default() -> Self {
        Self {
            enabled: true,
            border: None,
            label: None,
            labels: true,
            gap: 8,
        }
    }
}

impl Default for ThemeStyle {
    /// The built-in look: blocky bordered panes (matches `Palette::apply`
    /// plus the frame and corner scale).
    fn default() -> Self {
        Self {
            widget_radius: 4,
            window_radius: 8,
            menu_radius: 6,
            window_shadow: None,
            popup_shadow: None,
            panel_shadow: None,
            font_scale: 1.0,
            glass_opacity: 0.6,
            backdrop: None,
            accent_gradient: None,
            link: None,
            widget_border: None,
            panes: [PaneStyle::default(); 6],
            image: None,
            corner_scale: 0.25,
            frame: PaneFrame::default(),
            font: UiFont::Mono,
        }
    }
}

impl ThemeStyle {
    pub fn pane(&self, pane: Pane) -> PaneStyle {
        pane_index(pane).map(|i| self.panes[i]).unwrap_or_default()
    }

    /// `r` times the theme's corner scale.
    pub fn radius(&self, r: u8) -> u8 {
        (f32::from(r) * self.corner_scale).round().clamp(0.0, 255.0) as u8
    }
}

// ---- Raw TOML shape ---------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    #[allow(dead_code)] // checked separately (before the strict parse)
    format: i64,
    meta: RawMeta,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    style: RawStyle,
    #[serde(default)]
    images: RawImages,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    base: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawStyle {
    corner_radius: Option<RawRadius>,
    window_shadow: Option<RawShadow>,
    popup_shadow: Option<RawShadow>,
    panel_shadow: Option<RawShadow>,
    font_scale: Option<f32>,
    glass_opacity: Option<f32>,
    widget_border: Option<String>,
    backdrop: Option<RawGradient>,
    accent_gradient: Option<RawGradient>,
    corner_scale: Option<f32>,
    font_family: Option<String>,
    frame: Option<RawFrame>,
    #[serde(default)]
    panes: BTreeMap<String, RawPane>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrame {
    enabled: Option<bool>,
    border: Option<String>,
    label: Option<String>,
    labels: Option<bool>,
    gap: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRadius {
    widgets: Option<u8>,
    windows: Option<u8>,
    menus: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShadow {
    offset: Option<[i8; 2]>,
    blur: Option<u8>,
    spread: Option<u8>,
    color: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGradient {
    top: String,
    mid: Option<String>,
    bottom: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPane {
    gradient: Option<RawGradient>,
    fill: Option<String>,
    glass: Option<bool>,
    corner_radius: Option<u8>,
    inset: Option<u8>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawImages {
    chat_background: Option<String>,
    opacity: Option<f32>,
    tint: Option<String>,
    fit: Option<String>,
}

// ---- Parsing ----------------------------------------------------------------

/// `#RRGGBB` or `#RRGGBBAA` (alpha unmultiplied).
pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let a = if h.len() == 8 { byte(6)? } else { 255 };
    Some(Color32::from_rgba_unmultiplied(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        a,
    ))
}

fn color(key: &str, s: &str) -> Result<Color32> {
    parse_hex(s).ok_or_else(|| anyhow!("{key}: \"{s}\" is not a colour (use #RRGGBB or #RRGGBBAA)"))
}

fn gradient(key: &str, g: &RawGradient) -> Result<Gradient> {
    Ok(Gradient {
        top: color(&format!("{key}.top"), &g.top)?,
        mid: g
            .mid
            .as_deref()
            .map(|m| color(&format!("{key}.mid"), m))
            .transpose()?,
        bottom: color(&format!("{key}.bottom"), &g.bottom)?,
    })
}

fn shadow(key: &str, s: &RawShadow, dark: bool) -> Result<Shadow> {
    let blur = s.blur.unwrap_or(16);
    let spread = s.spread.unwrap_or(0);
    if blur > 64 {
        bail!("{key}.blur: {blur} is too large (max 64)");
    }
    if spread > 32 {
        bail!("{key}.spread: {spread} is too large (max 32)");
    }
    let offset = s.offset.unwrap_or([0, 4]);
    if offset.iter().any(|o| o.unsigned_abs() > 32) {
        bail!("{key}.offset: values must be between -32 and 32");
    }
    let color = match &s.color {
        Some(c) => color(&format!("{key}.color"), c)?,
        None => Color32::from_black_alpha(if dark { 90 } else { 40 }),
    };
    Ok(Shadow {
        offset,
        blur,
        spread,
        color,
    })
}

fn max(key: &str, v: u8, max: u8) -> Result<u8> {
    if v > max {
        bail!("{key}: {v} is too large (max {max})");
    }
    Ok(v)
}

fn range(key: &str, v: f32, lo: f32, hi: f32) -> Result<f32> {
    if !v.is_finite() || v < lo || v > hi {
        bail!("{key}: {v} must be between {lo} and {hi}");
    }
    Ok(v)
}

/// A path inside the theme folder: relative, no `..`, png/jpg.
pub fn safe_asset_path(s: &str) -> Result<PathBuf> {
    let bad = |why: &str| anyhow!("[images] chat_background: \"{s}\" {why}");
    if s.is_empty() || s.len() > 200 {
        return Err(bad("must be a short relative path"));
    }
    if s.contains('\\') || s.contains(':') || s.contains('\0') {
        return Err(bad("must use / and no drive letters"));
    }
    let p = Path::new(s);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(bad(
            "must stay inside the theme folder (no '..' or absolute paths)",
        ));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        return Err(bad("must be a .png or .jpg file"));
    }
    Ok(p.to_path_buf())
}

/// The `format = N` line, read loosely so newer themes get a clear message.
fn check_format(text: &str) -> Result<()> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow!("not valid TOML: {e}"))?;
    match table.get("format") {
        None => bail!("missing `format = {FORMAT}` at the top of the file"),
        Some(toml::Value::Integer(n)) if *n == FORMAT => Ok(()),
        Some(toml::Value::Integer(n)) if *n > FORMAT => bail!(
            "this theme uses format {n}, made for a newer RustCord (this version reads format {FORMAT})"
        ),
        Some(v) => bail!("format: {v:?} is not supported (expected {FORMAT})"),
    }
}

/// Parse and validate a theme file's text.
pub fn parse_theme(text: &str) -> Result<ThemeDef> {
    if text.len() as u64 > MAX_THEME_FILE {
        bail!("theme file is too big (max {} KB)", MAX_THEME_FILE / 1024);
    }
    check_format(text)?;
    let raw: RawTheme = toml::from_str(text).map_err(|e| anyhow!("{e}"))?;

    // [meta]
    let name = raw.meta.name.trim().to_string();
    if name.is_empty() {
        bail!("[meta] name: must not be empty");
    }
    if name.chars().count() > 64 {
        bail!("[meta] name: too long (max 64 characters)");
    }
    let short = |key: &str, v: &str, n: usize| -> Result<String> {
        let v = v.trim();
        if v.chars().count() > n {
            bail!("[meta] {key}: too long (max {n} characters)");
        }
        Ok(v.to_string())
    };
    let base = match raw.meta.base.as_deref().map(str::trim) {
        None | Some("dark") => ThemeChoice::DiscordDark,
        Some("light") => ThemeChoice::DiscordLight,
        Some(b) => bail!("[meta] base: \"{b}\" must be \"dark\" or \"light\""),
    };
    let meta = ThemeMeta {
        name,
        author: short("author", &raw.meta.author, 64)?,
        version: short("version", &raw.meta.version, 32)?,
        description: short("description", &raw.meta.description, 300)?,
        base,
    };

    // [colors]
    let mut palette = base.palette();
    let mut style = ThemeStyle::default();
    for (key, value) in &raw.colors {
        let c = color(&format!("[colors] {key}"), value)?;
        let slot = match key.as_str() {
            "servers" => &mut palette.servers,
            "sidebar" => &mut palette.sidebar,
            "chat" => &mut palette.chat,
            "input" => &mut palette.input,
            "hover" => &mut palette.hover,
            "selected" => &mut palette.selected,
            "text" => &mut palette.text,
            "muted" => &mut palette.muted,
            "accent" => &mut palette.accent,
            "online" => &mut palette.online,
            "idle" => &mut palette.idle,
            "dnd" => &mut palette.dnd,
            "offline" => &mut palette.offline,
            "danger" => &mut palette.danger,
            "mention" => &mut palette.mention,
            "unread" => &mut palette.unread,
            "strong" => &mut palette.strong,
            "msg_hover" => &mut palette.msg_hover,
            "divider" => &mut palette.divider,
            "rail_icon" => &mut palette.rail_icon,
            "link" => {
                style.link = Some(c);
                continue;
            }
            _ => bail!(
                "[colors] {key}: unknown colour (known: {})",
                COLOR_KEYS.join(", ")
            ),
        };
        *slot = c;
    }

    // [style]
    let s = &raw.style;
    let dark = palette.dark;
    if let Some(r) = &s.corner_radius {
        if let Some(v) = r.widgets {
            style.widget_radius = max("[style] corner_radius.widgets", v, 16)?;
        }
        if let Some(v) = r.windows {
            style.window_radius = max("[style] corner_radius.windows", v, 24)?;
        }
        if let Some(v) = r.menus {
            style.menu_radius = max("[style] corner_radius.menus", v, 16)?;
        }
    }
    style.window_shadow = s
        .window_shadow
        .as_ref()
        .map(|v| shadow("[style] window_shadow", v, dark))
        .transpose()?;
    style.popup_shadow = s
        .popup_shadow
        .as_ref()
        .map(|v| shadow("[style] popup_shadow", v, dark))
        .transpose()?;
    style.panel_shadow = s
        .panel_shadow
        .as_ref()
        .map(|v| shadow("[style] panel_shadow", v, dark))
        .transpose()?;
    if let Some(v) = s.font_scale {
        style.font_scale = range("[style] font_scale", v, 0.8, 1.25)?;
    }
    if let Some(v) = s.glass_opacity {
        style.glass_opacity = range("[style] glass_opacity", v, 0.2, 1.0)?;
    }
    style.widget_border = s
        .widget_border
        .as_deref()
        .map(|c| color("[style] widget_border", c))
        .transpose()?;
    style.backdrop = s
        .backdrop
        .as_ref()
        .map(|g| gradient("[style] backdrop", g))
        .transpose()?;
    style.accent_gradient = s
        .accent_gradient
        .as_ref()
        .map(|g| gradient("[style] accent_gradient", g))
        .transpose()?;
    if let Some(v) = s.corner_scale {
        style.corner_scale = range("[style] corner_scale", v, 0.0, 2.0)?;
    }
    style.font = match s.font_family.as_deref().map(str::trim) {
        None | Some("mono") => UiFont::Mono,
        Some("sans") => UiFont::Sans,
        Some(f) => bail!("[style] font_family: \"{f}\" must be \"mono\" or \"sans\""),
    };
    if let Some(f) = &s.frame {
        let d = PaneFrame::default();
        style.frame = PaneFrame {
            enabled: f.enabled.unwrap_or(d.enabled),
            border: f
                .border
                .as_deref()
                .map(|c| color("[style.frame] border", c))
                .transpose()?,
            label: f
                .label
                .as_deref()
                .map(|c| color("[style.frame] label", c))
                .transpose()?,
            labels: f.labels.unwrap_or(d.labels),
            gap: max("[style.frame] gap", f.gap.unwrap_or(d.gap), 24)?,
        };
    }
    for (key, p) in &s.panes {
        let Some(i) = PANE_KEYS.iter().position(|k| k == key) else {
            bail!(
                "[style.panes.{key}]: unknown pane (known: {})",
                PANE_KEYS.join(", ")
            );
        };
        let k = format!("[style.panes.{key}]");
        let gradient = match (&p.gradient, &p.fill) {
            (Some(_), Some(_)) => bail!("{k}: use either gradient or fill, not both"),
            (Some(g), None) => Some(gradient(&format!("{k} gradient"), g)?),
            (None, Some(f)) => Some(Gradient::flat(color(&format!("{k} fill"), f)?)),
            (None, None) => None,
        };
        style.panes[i] = PaneStyle {
            gradient,
            glass: p.glass.unwrap_or(false),
            corner_radius: max(
                &format!("{k} corner_radius"),
                p.corner_radius.unwrap_or(0),
                24,
            )?,
            inset: max(&format!("{k} inset"), p.inset.unwrap_or(0), 12)?,
        };
    }

    // [images]
    let im = &raw.images;
    if let Some(path) = &im.chat_background {
        style.image = Some(ImageSpec {
            path: safe_asset_path(path)?,
            opacity: range("[images] opacity", im.opacity.unwrap_or(0.25), 0.0, 1.0)?,
            tint: match &im.tint {
                Some(t) => color("[images] tint", t)?,
                None => Color32::WHITE,
            },
            fit: match im.fit.as_deref() {
                None | Some("cover") => ImageFit::Cover,
                Some("tile") => ImageFit::Tile,
                Some(f) => bail!("[images] fit: \"{f}\" must be \"cover\" or \"tile\""),
            },
        });
    } else if im.opacity.is_some() || im.tint.is_some() || im.fit.is_some() {
        bail!("[images]: opacity/tint/fit need a chat_background");
    }

    Ok(ThemeDef {
        meta,
        palette,
        style,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = "format = 1\n[meta]\nname = \"T\"\n";

    #[test]
    fn hex() {
        assert_eq!(parse_hex("#ff0080"), Some(Color32::from_rgb(255, 0, 128)));
        assert_eq!(parse_hex(" #FF0080 "), Some(Color32::from_rgb(255, 0, 128)));
        assert_eq!(
            parse_hex("#ffffff80"),
            Some(Color32::from_rgba_unmultiplied(255, 255, 255, 128))
        );
        for bad in [
            "ff0080",
            "#ff008",
            "#ff00800",
            "#gg0080",
            "#",
            "",
            "#ff0080ff00",
            "#ÿÿÿ",
        ] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn minimal_theme_falls_back_to_base() {
        let t = parse_theme(MIN).unwrap();
        assert_eq!(t.meta.name, "T");
        assert_eq!(t.meta.base, ThemeChoice::DiscordDark);
        assert_eq!(
            t.palette.chat,
            ThemeChoice::DiscordDark.palette().chat,
            "missing colours come from the base"
        );
        assert_eq!(t.style, ThemeStyle::default());

        let light = parse_theme(&format!(
            "{MIN}base = \"light\"\n[colors]\naccent = \"#112233\"\n"
        ))
        .unwrap();
        assert!(!light.palette.dark);
        assert_eq!(light.palette.accent, Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(light.palette.chat, ThemeChoice::DiscordLight.palette().chat);
    }

    #[test]
    fn full_style() {
        let t = parse_theme(&format!(
            r##"{MIN}
[colors]
link = "#0000ff"
[style]
corner_radius = {{ widgets = 6, windows = 12, menus = 8 }}
window_shadow = {{ offset = [0, 6], blur = 24, color = "#00000040" }}
font_scale = 0.9
widget_border = "#8faacc"
backdrop = {{ top = "#ffffff", mid = "#88ccff", bottom = "#0088ff" }}
accent_gradient = {{ top = "#66ff66", bottom = "#008800" }}
[style.panes.messages]
gradient = {{ top = "#ffffff", bottom = "#ddeeff" }}
glass = true
corner_radius = 10
inset = 4
[style.panes.servers]
fill = "#123456"
[images]
chat_background = "img/bubbles.png"
opacity = 0.3
fit = "tile"
"##
        ))
        .unwrap();
        let s = &t.style;
        assert_eq!(
            (s.widget_radius, s.window_radius, s.menu_radius),
            (6, 12, 8)
        );
        assert_eq!(s.window_shadow.unwrap().blur, 24);
        assert_eq!(s.font_scale, 0.9);
        assert_eq!(s.widget_border, Some(Color32::from_rgb(0x8f, 0xaa, 0xcc)));
        assert_eq!(s.link, Some(Color32::from_rgb(0, 0, 255)));
        let m = s.pane(Pane::Messages);
        assert!(m.glass && m.corner_radius == 10 && m.inset == 4);
        assert_eq!(
            s.pane(Pane::Servers).gradient,
            Some(Gradient::flat(Color32::from_rgb(0x12, 0x34, 0x56)))
        );
        assert!(!s.pane(Pane::Members).is_custom());
        let img = s.image.as_ref().unwrap();
        assert_eq!(img.path, PathBuf::from("img/bubbles.png"));
        assert_eq!(img.fit, ImageFit::Tile);
        let b = s.backdrop.unwrap();
        assert_eq!(b.at(0.5), Color32::from_rgb(0x88, 0xcc, 0xff));
        assert_eq!(b.at(0.0), Color32::WHITE);
        assert_eq!(b.at(1.0), Color32::from_rgb(0, 0x88, 0xff));
    }

    fn err(extra: &str) -> String {
        parse_theme(&format!("{MIN}{extra}"))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn errors_are_clear() {
        assert!(
            parse_theme("[meta]\nname = \"x\"")
                .unwrap_err()
                .to_string()
                .contains("missing `format = 1`")
        );
        assert!(
            parse_theme("format = 2\n[meta]\nname = \"x\"")
                .unwrap_err()
                .to_string()
                .contains("newer RustCord")
        );
        assert!(
            parse_theme("format = \"1\"")
                .unwrap_err()
                .to_string()
                .contains("not supported")
        );
        assert!(
            parse_theme("format = 1\n[meta\n")
                .unwrap_err()
                .to_string()
                .contains("TOML")
        );
        assert!(parse_theme("format = 1\n").is_err(), "meta is required");
        assert!(err("[colors]\naccent = \"blue\"").contains("[colors] accent"));
        assert!(err("[colors]\nacent = \"#000000\"").contains("unknown colour"));
        assert!(err("[style]\nfont_scale = 3.0").contains("font_scale"));
        assert!(err("[style]\nfont_scale = nan").contains("font_scale"));
        assert!(err("[style]\ncorner_radius = { widgets = 99 }").contains("max 16"));
        assert!(err("[style]\ncorner_radius = { widgets = 999 }").contains("u8"));
        assert!(err("[style]\nconer_radius = 3").contains("coner_radius"));
        assert!(err("[style.panes.chat]\nglass = true").contains("unknown pane"));
        assert!(
            err("[style.panes.messages]\nfill = \"#000000\"\ngradient = { top = \"#000000\", bottom = \"#000000\" }")
                .contains("either")
        );
        assert!(err("[images]\nchat_background = \"../x.png\"").contains("inside"));
        assert!(err("[images]\nchat_background = \"/etc/x.png\"").contains("inside"));
        assert!(err("[images]\nchat_background = \"C:\\\\x.png\"").contains("drive"));
        assert!(err("[images]\nchat_background = \"x.gif\"").contains(".png"));
        assert!(err("[images]\nopacity = 0.5").contains("need a chat_background"));
        assert!(
            parse_theme("format = 1\n[meta]\nname = \"  \"")
                .unwrap_err()
                .to_string()
                .contains("name")
        );
        assert!(
            parse_theme("format = 1\n[meta]\nname = \"x\"\nbase = \"blue\"")
                .unwrap_err()
                .to_string()
                .contains("base")
        );
        let big = format!("{MIN}# {}\n", "x".repeat(70_000));
        assert!(
            parse_theme(&big)
                .unwrap_err()
                .to_string()
                .contains("too big")
        );
    }

    #[test]
    fn example_themes_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/themes");
        for name in [
            "discord-classic",
            "discord-light",
            "2000s-messenger",
            "frosted-glass",
            "frutiger-aero",
        ] {
            let dir = root.join(name);
            let text = std::fs::read_to_string(dir.join(THEME_FILE)).unwrap();
            let t = parse_theme(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!t.meta.name.is_empty());
            if let Some(img) = &t.style.image {
                let p = dir.join(&img.path);
                let len = std::fs::metadata(&p).unwrap().len();
                assert!(len <= 300 * 1024, "{name}: image is {len} bytes");
            }
        }
    }

    #[test]
    fn frame_tokens() {
        let d = ThemeStyle::default();
        assert!(
            d.frame.enabled && d.frame.labels,
            "the default look is framed"
        );
        assert!(d.corner_scale < 0.5, "and almost square");
        let t = parse_theme(&format!(
            r##"{MIN}
[style]
corner_scale = 0
[style.frame]
border = "#102030"
label = "#405060"
labels = false
gap = 12
"##
        ))
        .unwrap();
        let f = t.style.frame;
        assert!(f.enabled && !f.labels);
        assert_eq!(f.border, Some(Color32::from_rgb(0x10, 0x20, 0x30)));
        assert_eq!(f.label, Some(Color32::from_rgb(0x40, 0x50, 0x60)));
        assert_eq!(f.gap, 12);
        assert_eq!(t.style.radius(8), 0);
        let off = parse_theme(&format!("{MIN}[style]\nframe = {{ enabled = false }}\n")).unwrap();
        assert!(!off.style.frame.enabled);
        assert_eq!(off.style.frame.gap, PaneFrame::default().gap);
        assert!(err("[style.frame]\ngap = 40").contains("max 24"));
        assert!(err("[style.frame]\nborder = \"red\"").contains("[style.frame] border"));
        assert!(err("[style.frame]\nlable = true").contains("lable"));
        assert!(err("[style]\ncorner_scale = 3.0").contains("corner_scale"));
    }

    #[test]
    fn font_family_token() {
        assert_eq!(
            ThemeStyle::default().font,
            UiFont::Mono,
            "monospace by default"
        );
        let sans = parse_theme(&format!("{MIN}[style]\nfont_family = \"sans\"\n")).unwrap();
        assert_eq!(sans.style.font, UiFont::Sans);
        let mono = parse_theme(&format!("{MIN}[style]\nfont_family = \"mono\"\n")).unwrap();
        assert_eq!(mono.style.font, UiFont::Mono);
        assert!(err("[style]\nfont_family = \"serif\"").contains("font_family"));
    }

    #[test]
    fn discord_classic_example_is_the_old_built_in() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/themes/discord-classic")
            .join(THEME_FILE);
        let t = parse_theme(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(t.meta.base, ThemeChoice::DiscordDark);
        assert_eq!(
            format!("{:?}", t.palette),
            format!("{:?}", ThemeChoice::DiscordDark.palette())
        );
        let classic = ThemeStyle {
            corner_scale: 1.0,
            font: UiFont::Sans,
            frame: PaneFrame {
                enabled: false,
                ..PaneFrame::default()
            },
            ..ThemeStyle::default()
        };
        assert_eq!(t.style, classic);
    }

    #[test]
    fn discord_light_example_is_the_old_built_in() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/themes/discord-light")
            .join(THEME_FILE);
        let t = parse_theme(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(t.meta.base, ThemeChoice::DiscordLight);
        assert_eq!(
            format!("{:?}", t.palette),
            format!("{:?}", ThemeChoice::DiscordLight.palette())
        );
    }
}
