//! Monochrome line icons from the bundled Phosphor icon font
//! (`assets/fonts/Phosphor.ttf`, MIT). They're ordinary text glyphs in the
//! Private Use Area, so they take any colour (use `Palette` colours, never
//! hard-coded ones) and can be mixed into labels: `format!("{} Reply",
//! icons::REPLY)`.
//!
//! `theme::install_fonts` adds the regular weight as a fallback of the
//! proportional and bold families, and the filled weight as the `icons-fill`
//! family (`fill_font`). Codepoints come from Phosphor 2.x; any other
//! Phosphor icon can be added here by its codepoint.

// A small palette of icons for every screen; not all are used yet.
#![allow(dead_code)]

use eframe::egui::{self, FontFamily, FontId};

/// Name of the filled-icon font family.
pub const FILL: &str = "icons-fill";

/// A filled Phosphor icon font of `size` points (Discord's solid header icons).
pub fn fill_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FILL.into()))
}

/// A regular (line) Phosphor icon font of `size` points.
pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

// Chat header / toolbar
pub const BELL: &str = "\u{E0CE}";
pub const BELL_SLASH: &str = "\u{E0D4}";
pub const PUSH_PIN: &str = "\u{E3E2}";
pub const PUSH_PIN_SLASH: &str = "\u{E3E4}";
pub const USERS: &str = "\u{E4D6}";
pub const USER: &str = "\u{E4C2}";
pub const USER_CIRCLE: &str = "\u{E4C4}";
pub const GEAR: &str = "\u{E270}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const TRAY: &str = "\u{E4AA}";
pub const SIDEBAR: &str = "\u{EC24}";
pub const CARET_DOUBLE_LEFT: &str = "\u{E128}";
pub const CARET_DOUBLE_RIGHT: &str = "\u{E12A}";

// Channels
pub const HASH: &str = "\u{E2A2}";
pub const SPEAKER: &str = "\u{E44A}";
pub const MEGAPHONE: &str = "\u{E324}";
pub const BROADCAST: &str = "\u{E0F2}";
pub const AT: &str = "\u{E0AC}";
pub const CHATS: &str = "\u{E17C}";
pub const CHAT_CIRCLE: &str = "\u{E168}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CARET_RIGHT: &str = "\u{E13A}";
pub const CARET_LEFT: &str = "\u{E138}";
pub const FOLDER: &str = "\u{E24A}";
pub const DISCORD_LOGO: &str = "\u{E61A}";

// Messages / composer
pub const REPLY: &str = "\u{E024}";
pub const PENCIL: &str = "\u{E3B4}";
pub const DOTS: &str = "\u{E1FE}";
pub const TRASH: &str = "\u{E4A6}";
pub const COPY: &str = "\u{E1CA}";
pub const LINK: &str = "\u{E2E2}";
pub const ID: &str = "\u{E6F6}";
pub const EYE: &str = "\u{E220}";
pub const EYE_SLASH: &str = "\u{E224}";
pub const DOWNLOAD: &str = "\u{E20C}";
pub const PLUS: &str = "\u{E3D4}";
pub const PLUS_CIRCLE: &str = "\u{E3D6}";
pub const SMILEY: &str = "\u{E436}";
pub const GIF: &str = "\u{E274}";
pub const STICKER: &str = "\u{E5AC}";
pub const SEND: &str = "\u{E396}";
pub const FILE: &str = "\u{E230}";
pub const FILE_TEXT: &str = "\u{E23A}";
pub const PAPERCLIP: &str = "\u{E39A}";
pub const IMAGE: &str = "\u{E2CA}";
pub const UPLOAD: &str = "\u{E4C0}";
pub const X: &str = "\u{E4F6}";
// Window controls (`window_frame`)
pub const MINUS: &str = "\u{E32A}";
pub const SQUARE: &str = "\u{E45E}";
pub const CHECK_SQUARE: &str = "\u{E186}";
pub const QR_CODE: &str = "\u{E3E6}";
pub const ARROW_CLOCKWISE: &str = "\u{E036}";
pub const DEVICE_MOBILE: &str = "\u{E1E0}";
pub const CHECK: &str = "\u{E182}";
pub const CHECKS: &str = "\u{E53A}";
pub const PROHIBIT: &str = "\u{E3DE}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
pub const ARROW_UP: &str = "\u{E08E}";
pub const ARROW_DOWN: &str = "\u{E03E}";
pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";

// System messages
pub const USER_PLUS: &str = "\u{E4D0}";
pub const USER_MINUS: &str = "\u{E4CE}";
pub const PHONE: &str = "\u{E3B8}";
pub const PHONE_X: &str = "\u{E3C4}";
pub const ROCKET: &str = "\u{E3FE}";
pub const STAR: &str = "\u{E46A}";
pub const SPARKLE: &str = "\u{E6A2}";
pub const HAND_WAVING: &str = "\u{E580}";
pub const WARNING: &str = "\u{E4E0}";
pub const INFO: &str = "\u{E2CE}";

// Settings / misc
pub const SIGN_OUT: &str = "\u{E42A}";
pub const RESET: &str = "\u{E038}";
pub const PLAY: &str = "\u{E3D0}";
pub const CODE: &str = "\u{E1BC}";
pub const BUG: &str = "\u{E5F4}";
pub const MOVE: &str = "\u{E0A4}";
pub const LAYOUT: &str = "\u{E6D6}";
pub const PUZZLE: &str = "\u{E596}";
pub const PALETTE: &str = "\u{E6C8}";
pub const KEYBOARD: &str = "\u{E2D8}";
pub const SHIELD: &str = "\u{E40A}";
pub const GAUGE: &str = "\u{E628}";
pub const MEMORY: &str = "\u{E9C4}";
pub const WIFI: &str = "\u{E4EA}";
pub const WIFI_SLASH: &str = "\u{E4F2}";
pub const LOCK: &str = "\u{E308}";
pub const ENVELOPE: &str = "\u{E218}";
pub const SQUARES_FOUR: &str = "\u{E464}";
/// Hamburger: opens the left drawer on narrow windows.
pub const LIST: &str = "\u{E2F0}";

/// Paint `icon` centred in `rect`.
pub fn paint(ui: &egui::Ui, rect: egui::Rect, icon: &str, font: FontId, color: egui::Color32) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        font,
        color,
    );
}

/// A menu row label: icon, two spaces, text.
pub fn label(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

// Emoji picker categories and the skin tone button
pub const CLOCK: &str = "\u{E19A}";
pub const PAW_PRINT: &str = "\u{E648}";
pub const HAMBURGER: &str = "\u{E790}";
pub const AIRPLANE: &str = "\u{E002}";
pub const SOCCER_BALL: &str = "\u{E716}";
pub const LIGHTBULB: &str = "\u{E2DC}";
pub const HEART: &str = "\u{E2A8}";
pub const FLAG: &str = "\u{E244}";
pub const HAND: &str = "\u{E298}";
pub const CARET_UP_DOWN: &str = "\u{E140}";
pub const SORT_ASCENDING: &str = "\u{E444}";
pub const TIMER: &str = "\u{E492}";
