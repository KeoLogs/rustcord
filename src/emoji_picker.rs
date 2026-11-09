//! A compact, reusable emoji picker (reactions now, the composer later).
//!
//! - [`EmojiRef`] is Discord's "partial emoji" (`{ id, name, animated }`):
//!   `id: None` for Unicode emoji (then `name` is the emoji itself), or a
//!   custom server emoji (`name` is its `:name:` without colons).
//! - [`RecentEmoji`] is a capped most-recently-used list. The caller owns it
//!   (and may save it, it's serde), and calls [`RecentEmoji::record`] after a
//!   pick.
//! - [`popup`] shows the picker as a popup under a button and returns a
//!   [`Picked`]. [`picker_ui`] is the bare picker for other containers.
//! - With [`PickerCtx::allow_text`] (the composer) there's also a Symbols
//!   tab: any Unicode character as text — symbol sets, kaomoji, a
//!   `U+XXXX` box and a fancy-text transformer (`unicode_symbols.rs`).
//!   Reactions leave it off: Discord only accepts emoji as reactions.
//!
//! RAM: only the cells that are on screen load an image, and when the popup
//! closes the images it loaded are forgotten again (except recent ones), so
//! scrolling through ~1,800 Twemoji doesn't leave them all in memory.

use std::cell::Cell;
use std::collections::HashSet;

use eframe::egui::{self, Color32, RichText, Sense, vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::emoji;
use crate::theme::Palette;
use crate::unicode_symbols::{self, FancyStyle};

/// Most entries kept in [`RecentEmoji`].
pub const RECENT_CAP: usize = 32;
/// Newest emoji version offered. Twemoji and Discord lag behind Unicode, and
/// an emoji Discord doesn't know fails with "Unknown Emoji".
const MAX_EMOJI_VERSION: (u32, u32) = (15, 1);
/// Picker geometry.
const CELL: f32 = 32.0;
const COLS: usize = 9;
const IMG: f32 = 24.0;
const GRID_HEIGHT: f32 = 300.0;
/// Search shows at most this many results.
const MAX_RESULTS: usize = 270;

// ---------------------------------------------------------------------------
// EmojiRef
// ---------------------------------------------------------------------------

/// A Unicode or custom emoji, as Discord's partial emoji object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmojiRef {
    /// Custom emoji id; `None` for Unicode emoji.
    pub id: Option<String>,
    /// The emoji itself (Unicode) or the custom emoji's name. Can be empty
    /// for a custom emoji that was deleted (Discord sends `name: null`).
    pub name: String,
    #[serde(default)]
    pub animated: bool,
}

impl EmojiRef {
    pub fn unicode(emoji: &str) -> Self {
        Self {
            id: None,
            name: emoji.to_string(),
            animated: false,
        }
    }

    pub fn custom(id: &str, name: &str, animated: bool) -> Self {
        Self {
            id: Some(id.to_string()),
            name: name.to_string(),
            animated,
        }
    }

    /// Parse `{ id, name, animated }` (reactions, gateway events, guild
    /// emoji lists). `None` if there's neither an id nor a name.
    pub fn from_json(v: &Value) -> Option<Self> {
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_none() && name.is_empty() {
            return None;
        }
        Some(Self {
            id,
            name,
            animated: v.get("animated").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn is_custom(&self) -> bool {
        self.id.is_some()
    }

    /// Same emoji? Custom emoji compare by id (names can change), Unicode
    /// emoji by value, ignoring the FE0F variation selector differences
    /// Discord and the emoji tables sometimes disagree on.
    pub fn same(&self, other: &EmojiRef) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => {
                self.name == other.name
                    || match (emojis::get(&self.name), emojis::get(&other.name)) {
                        (Some(a), Some(b)) => std::ptr::eq(a, b),
                        _ => {
                            let strip = |s: &str| s.replace('\u{FE0F}', "");
                            strip(&self.name) == strip(&other.name)
                        }
                    }
            }
            _ => false,
        }
    }

    /// Twemoji / Discord CDN image (the same URLs `emoji.rs` uses for
    /// message text, so textures are shared).
    pub fn image_url(&self) -> String {
        match &self.id {
            Some(id) => emoji::custom_emoji_url(id, self.animated),
            None => emoji::twemoji_url(&self.name),
        }
    }

    /// What to draw when images are off: the emoji itself, or `:name:`.
    pub fn text(&self) -> String {
        if self.is_custom() {
            format!(":{}:", self.display_name())
        } else {
            self.name.clone()
        }
    }

    /// `:shortcode:` for tooltips and the picker footer.
    pub fn shortcode(&self) -> String {
        if self.is_custom() {
            return format!(":{}:", self.display_name());
        }
        match emojis::get(&self.name) {
            // Prefer a word (":thumbsup:", like Discord) over ":+1:".
            Some(e) => match e
                .shortcodes()
                .find(|c| c.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .or_else(|| e.shortcode())
            {
                Some(code) => format!(":{code}:"),
                None => format!(":{}:", e.name().replace(' ', "_")),
            },
            None => self.name.clone(),
        }
    }

    fn display_name(&self) -> &str {
        if self.name.is_empty() {
            "deleted_emoji"
        } else {
            &self.name
        }
    }

    /// The emoji as message text: Unicode as-is, custom as `<:name:id>` /
    /// `<a:name:id>`.
    pub fn message_text(&self) -> String {
        match &self.id {
            Some(id) if self.animated => format!("<a:{}:{id}>", self.display_name()),
            Some(id) => format!("<:{}:{id}>", self.display_name()),
            None => self.name.clone(),
        }
    }

    /// The `{emoji}` path segment of the reaction endpoints: the
    /// percent-encoded Unicode emoji, or `name:id` for custom emoji. (Discord
    /// answers "Unknown Emoji" if Unicode isn't URL-encoded.)
    pub fn url_segment(&self) -> String {
        match &self.id {
            // Discord only looks at the id; any valid name works, so a
            // deleted emoji (no name) still gets a placeholder.
            Some(id) => {
                let name = if self.name.is_empty() {
                    "_"
                } else {
                    &self.name
                };
                format!("{}:{id}", percent_encode(name))
            }
            None => percent_encode(&self.name),
        }
    }
}

/// Percent-encode everything except RFC 3986 unreserved characters.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    Emoji(EmojiRef),
    /// Plain text from the Symbols tab (symbols, kaomoji, fancy text).
    Text(String),
}

impl Picked {
    /// Text to insert into a message.
    pub fn insert_text(&self) -> String {
        match self {
            Picked::Emoji(e) => e.message_text(),
            Picked::Text(t) => t.clone(),
        }
    }

    pub fn emoji(self) -> Option<EmojiRef> {
        match self {
            Picked::Emoji(e) => Some(e),
            Picked::Text(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Custom emoji and the recent list
// ---------------------------------------------------------------------------

/// One server's custom emoji, shown as its own picker section.
#[derive(Debug, Clone, Default)]
pub struct CustomEmojiGroup {
    /// Section title (server name).
    pub name: String,
    pub emojis: Vec<EmojiRef>,
}

/// Most recently used emoji, newest first, at most [`RECENT_CAP`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentEmoji {
    pub items: Vec<EmojiRef>,
}

impl RecentEmoji {
    /// Move `emoji` to the front (adding it if new) and drop the oldest.
    pub fn record(&mut self, emoji: &EmojiRef) {
        self.items.retain(|e| !e.same(emoji));
        self.items.insert(0, emoji.clone());
        self.items.truncate(RECENT_CAP);
    }
}

// ---------------------------------------------------------------------------
// Picker UI
// ---------------------------------------------------------------------------

/// What the picker needs from the caller.
pub struct PickerCtx<'a> {
    pub pal: &'a Palette,
    /// Emoji images module: false draws emoji as text and loads nothing.
    pub images: bool,
    /// Custom emoji the user may use here (e.g. the current server's).
    pub custom: &'a [CustomEmojiGroup],
    pub recent: &'a RecentEmoji,
    /// Offer the Symbols tab (plain text). False for reactions.
    pub allow_text: bool,
    /// Skin tone for emoji that support it (0 = default yellow, 1-5 light
    /// to dark). The picker's tone button changes it; the caller saves it.
    pub skin_tone: &'a Cell<u8>,
}

/// Skin tone names, index = tone (0-5).
const SKIN_TONES: [(&str, Color32); 6] = [
    ("Default", Color32::from_rgb(0xFF, 0xC9, 0x3A)),
    ("Light", Color32::from_rgb(0xF7, 0xDE, 0xCE)),
    ("Medium-Light", Color32::from_rgb(0xF3, 0xD2, 0xA2)),
    ("Medium", Color32::from_rgb(0xD5, 0xAB, 0x88)),
    ("Medium-Dark", Color32::from_rgb(0xAF, 0x7E, 0x57)),
    ("Dark", Color32::from_rgb(0x7C, 0x53, 0x3E)),
];

/// `e` in skin tone `tone` (0-5) if it has that variant, else `e`.
pub fn with_tone(e: &'static emojis::Emoji, tone: u8) -> &'static emojis::Emoji {
    use emojis::SkinTone::*;
    let t = match tone {
        1 => Light,
        2 => MediumLight,
        3 => Medium,
        4 => MediumDark,
        5 => Dark,
        _ => return e,
    };
    e.with_skin_tone(t).unwrap_or(e)
}

/// The tone button next to the search box: a coloured dot; click cycles.
fn tone_button(ui: &mut egui::Ui, tone: &Cell<u8>, pal: &Palette) {
    let t = usize::from(tone.get().min(5));
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    ui.painter()
        .circle_filled(rect.center(), 7.0, SKIN_TONES[t].1);
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Skin tone: {} (click to change)", SKIN_TONES[t].0));
    if resp.clicked() {
        tone.set(((t + 1) % SKIN_TONES.len()) as u8);
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    #[default]
    Emoji,
    Symbols,
}

/// Per-picker state, kept in egui temp memory while the picker is open.
#[derive(Clone, Default)]
struct PickerState {
    query: String,
    /// Section to scroll to (set by the category bar).
    jump: Option<usize>,
    /// Image URLs this picker loaded (forgotten again on close).
    loaded: HashSet<String>,
    focused: bool,
    tab: Tab,
}

/// Show the picker as a popup under `button` (toggled by clicking it).
/// Returns what was picked. The popup closes after a pick unless Shift is
/// held (like Discord), and frees its images when it closes.
pub fn popup(button: &egui::Response, ctx: &PickerCtx) -> Option<Picked> {
    let popup_id = egui::Popup::default_response_id(button);
    let state_id = popup_id.with("picker_state");
    let mut picked = None;
    egui::Popup::menu(button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[egui::RectAlign::TOP_START, egui::RectAlign::BOTTOM_END])
        .show(|ui| {
            picked = picker_ui(ui, state_id, ctx);
        });
    if picked.is_some() && !button.ctx.input(|i| i.modifiers.shift) {
        egui::Popup::close_id(&button.ctx, popup_id);
    }
    if !egui::Popup::is_id_open(&button.ctx, popup_id) {
        forget(&button.ctx, state_id, ctx.recent);
    }
    picked
}

/// Drop the picker's state and forget the images it loaded (keeping the
/// recent ones, which are likely to be shown again soon).
pub fn forget(egui_ctx: &egui::Context, state_id: egui::Id, recent: &RecentEmoji) {
    let state = egui_ctx.memory_mut(|m| m.data.remove_temp::<PickerState>(state_id));
    if let Some(state) = state {
        let keep: HashSet<String> = recent.items.iter().map(EmojiRef::image_url).collect();
        for url in state.loaded.difference(&keep) {
            egui_ctx.forget_image(url);
        }
    }
}

/// One picker section.
struct Section {
    title: String,
    /// Category bar icon.
    icon: EmojiRef,
    emojis: Vec<EmojiRef>,
}

/// The bare picker (tabs, search box, category bar, grid, footer).
/// `state_id` must be stable while it's shown; call [`forget`] with it when
/// done.
pub fn picker_ui(ui: &mut egui::Ui, state_id: egui::Id, ctx: &PickerCtx) -> Option<Picked> {
    let mut state: PickerState = ui
        .ctx()
        .memory_mut(|m| m.data.get_temp(state_id))
        .unwrap_or_default();
    let width = CELL * COLS as f32 + 14.0;
    ui.set_width(width);

    if ctx.allow_text {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::Emoji, "Emoji"), (Tab::Symbols, "Symbols")] {
                if ui.selectable_label(state.tab == tab, label).clicked() && state.tab != tab {
                    state.tab = tab;
                    state.query.clear();
                    state.focused = false;
                }
            }
        });
    } else {
        state.tab = Tab::Emoji;
    }
    let picked = match state.tab {
        Tab::Emoji => emoji_tab(ui, state_id, ctx, &mut state, width).map(Picked::Emoji),
        Tab::Symbols => symbols_tab(ui, state_id, ctx, &mut state, width).map(Picked::Text),
    };
    ui.ctx().memory_mut(|m| m.data.insert_temp(state_id, state));
    picked
}

fn emoji_tab(
    ui: &mut egui::Ui,
    state_id: egui::Id,
    ctx: &PickerCtx,
    state: &mut PickerState,
    width: f32,
) -> Option<EmojiRef> {
    let pal = ctx.pal;

    let search = ui
        .horizontal(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("Find the perfect emoji")
                    .desired_width(width - 32.0),
            );
            tone_button(ui, ctx.skin_tone, pal);
            search
        })
        .inner;
    if !state.focused {
        search.request_focus();
        state.focused = true;
    }
    let query = state.query.trim().to_lowercase();

    let sections = if query.is_empty() {
        sections(ctx)
    } else {
        vec![Section {
            title: "Search results".into(),
            icon: EmojiRef::unicode("🔍"),
            emojis: search_emoji(&query, ctx.custom, ctx.skin_tone.get()),
        }]
    };

    // Category bar.
    if query.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for (i, s) in sections.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 4, pal.hover);
                }
                draw_emoji(ui, &s.icon, rect.shrink(3.0), ctx.images, &mut state.loaded);
                if resp.on_hover_text(&s.title).clicked() {
                    state.jump = Some(i);
                }
            }
        });
    }
    ui.add(egui::Separator::default().spacing(4.0));

    let mut picked = None;
    let mut hovered: Option<EmojiRef> = None;
    let jump = state.jump.take();
    egui::ScrollArea::vertical()
        .id_salt(state_id.with("scroll"))
        .min_scrolled_height(GRID_HEIGHT)
        .max_height(GRID_HEIGHT)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            if sections.iter().all(|s| s.emojis.is_empty()) {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No emoji match that search").color(pal.muted));
                });
            }
            for (i, s) in sections.iter().enumerate() {
                if s.emojis.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                let header = ui.label(pal.section_label(&s.title));
                if jump == Some(i) {
                    header.scroll_to_me(Some(egui::Align::TOP));
                }
                ui.add_space(4.0);
                for (row_i, row) in s.emojis.chunks(COLS).enumerate() {
                    let (row_rect, _) =
                        ui.allocate_exact_size(vec2(CELL * COLS as f32, CELL), Sense::hover());
                    // Off-screen rows: no interaction, no image loading.
                    if !ui.is_rect_visible(row_rect) {
                        continue;
                    }
                    for (col, e) in row.iter().enumerate() {
                        let cell = egui::Rect::from_min_size(
                            row_rect.min + vec2(col as f32 * CELL, 0.0),
                            vec2(CELL, CELL),
                        );
                        let resp =
                            ui.interact(cell, state_id.with((i, row_i, col)), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(cell.shrink(1.0), 4, pal.hover);
                            hovered = Some(e.clone());
                        }
                        let img = egui::Rect::from_center_size(cell.center(), vec2(IMG, IMG));
                        draw_emoji(ui, e, img, ctx.images, &mut state.loaded);
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            picked = Some(e.clone());
                        }
                    }
                }
            }
        });

    // Footer: the hovered emoji and its shortcode, like Discord.
    ui.add(egui::Separator::default().spacing(4.0));
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        match &hovered {
            Some(e) => {
                let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                draw_emoji(ui, e, rect, ctx.images, &mut state.loaded);
                ui.label(RichText::new(e.shortcode()).color(pal.strong));
            }
            None => {
                ui.label(RichText::new("Shift-click to pick several").color(pal.muted));
            }
        }
    });
    picked
}

/// The Symbols tab: a box for text / `U+XXXX`, fancy-text versions of it,
/// kaomoji and the symbol sets. Everything is inserted as text.
fn symbols_tab(
    ui: &mut egui::Ui,
    state_id: egui::Id,
    ctx: &PickerCtx,
    state: &mut PickerState,
    width: f32,
) -> Option<String> {
    let pal = ctx.pal;
    let input = ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .hint_text("Text to style, or U+2603")
            .desired_width(width),
    );
    if !state.focused {
        input.request_focus();
        state.focused = true;
    }
    ui.add(egui::Separator::default().spacing(4.0));

    let mut picked = None;
    let mut hovered: Option<String> = None;
    let query = state.query.clone();
    egui::ScrollArea::vertical()
        .id_salt(state_id.with("sym_scroll"))
        .min_scrolled_height(GRID_HEIGHT)
        .max_height(GRID_HEIGHT)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            // Any code point.
            if let Some(c) = unicode_symbols::parse_codepoint(&query) {
                section_header(ui, pal, "Character");
                char_grid(
                    ui,
                    state_id.with("cp"),
                    &[c],
                    pal,
                    &mut picked,
                    &mut hovered,
                );
            }
            // Fancy text.
            section_header(ui, pal, "Fancy text");
            let sample = if query.trim().is_empty() {
                "Fancy text"
            } else {
                query.as_str()
            };
            ui.spacing_mut().item_spacing.y = 2.0;
            for style in FancyStyle::ALL {
                let out = unicode_symbols::fancy(sample, style);
                let resp = ui
                    .add(
                        egui::Button::new(RichText::new(&out).color(pal.text))
                            .frame(false)
                            .truncate(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() {
                    hovered = Some(format!("{} · click to insert", style.label()));
                }
                if resp.clicked() {
                    picked = Some(out);
                }
            }
            // Kaomoji.
            section_header(ui, pal, "Kaomoji");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for k in unicode_symbols::KAOMOJI {
                    let resp = ui
                        .add(egui::Button::new(RichText::new(*k).color(pal.text)))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        picked = Some(k.to_string());
                    }
                }
            });
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for (i, set) in unicode_symbols::SETS.iter().enumerate() {
                section_header(ui, pal, set.name);
                let chars: Vec<char> = unicode_symbols::chars(set).collect();
                char_grid(
                    ui,
                    state_id.with(("set", i)),
                    &chars,
                    pal,
                    &mut picked,
                    &mut hovered,
                );
            }
        });

    ui.add(egui::Separator::default().spacing(4.0));
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        let text = hovered.unwrap_or_else(|| "Inserted as text · Shift-click for several".into());
        ui.label(RichText::new(text).color(pal.muted));
    });
    picked
}

fn section_header(ui: &mut egui::Ui, pal: &Palette, title: &str) {
    ui.add_space(6.0);
    ui.label(pal.section_label(title));
    ui.add_space(4.0);
}

/// A grid of single characters drawn as text (only visible rows).
fn char_grid(
    ui: &mut egui::Ui,
    id: egui::Id,
    chars: &[char],
    pal: &Palette,
    picked: &mut Option<String>,
    hovered: &mut Option<String>,
) {
    for (row_i, row) in chars.chunks(COLS).enumerate() {
        let (row_rect, _) = ui.allocate_exact_size(vec2(CELL * COLS as f32, CELL), Sense::hover());
        if !ui.is_rect_visible(row_rect) {
            continue;
        }
        for (col, &c) in row.iter().enumerate() {
            let cell = egui::Rect::from_min_size(
                row_rect.min + vec2(col as f32 * CELL, 0.0),
                vec2(CELL, CELL),
            );
            let resp = ui.interact(cell, id.with((row_i, col)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(cell.shrink(1.0), 4, pal.hover);
                *hovered = Some(format!("{c}  U+{:04X}", c as u32));
            }
            ui.painter().text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                c,
                egui::FontId::proportional(18.0),
                pal.text,
            );
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                *picked = Some(c.to_string());
            }
        }
    }
}

/// Draw one emoji into `rect`: the image (only if visible), or text.
fn draw_emoji(
    ui: &egui::Ui,
    e: &EmojiRef,
    rect: egui::Rect,
    images: bool,
    loaded: &mut HashSet<String>,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    if images {
        let url = e.image_url();
        egui::Image::from_uri(url.clone())
            .show_loading_spinner(false)
            .paint_at(ui, rect);
        loaded.insert(url);
    } else {
        draw_emoji_text(ui, e, rect, ui.visuals().text_color());
    }
}

/// Text fallback for an emoji (images module off).
pub fn draw_emoji_text(ui: &egui::Ui, e: &EmojiRef, rect: egui::Rect, color: Color32) {
    let (text, size) = if e.is_custom() {
        // `:name:` won't fit a cell; show the first letters.
        (e.display_name().chars().take(3).collect::<String>(), 11.5)
    } else {
        (
            e.name.clone(),
            if rect.height() >= 20.0 { 18.0 } else { 15.0 },
        )
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(size),
        color,
    );
}

fn version_ok(e: &emojis::Emoji) -> bool {
    let v = e.emoji_version();
    (v.major(), v.minor()) <= MAX_EMOJI_VERSION
}

/// All sections when there's no search: recent, custom groups, then the
/// Unicode categories.
fn sections(ctx: &PickerCtx) -> Vec<Section> {
    let mut out = Vec::new();
    // Recent custom emoji only if they're usable here.
    let usable = |e: &EmojiRef| {
        !e.is_custom()
            || ctx
                .custom
                .iter()
                .any(|g| g.emojis.iter().any(|c| c.same(e)))
    };
    let recent: Vec<EmojiRef> = ctx
        .recent
        .items
        .iter()
        .filter(|e| usable(e))
        .cloned()
        .collect();
    if !recent.is_empty() {
        out.push(Section {
            title: "Recently used".into(),
            icon: EmojiRef::unicode("🕘"),
            emojis: recent,
        });
    }
    for g in ctx.custom {
        if let Some(first) = g.emojis.first() {
            out.push(Section {
                title: g.name.clone(),
                icon: first.clone(),
                emojis: g.emojis.clone(),
            });
        }
    }
    for group in emojis::Group::iter() {
        let (title, icon) = group_info(group);
        out.push(Section {
            title: title.into(),
            icon: EmojiRef::unicode(icon),
            emojis: group
                .emojis()
                .filter(|e| version_ok(e))
                .map(|e| EmojiRef::unicode(with_tone(e, ctx.skin_tone.get()).as_str()))
                .collect(),
        });
    }
    out
}

fn group_info(g: emojis::Group) -> (&'static str, &'static str) {
    use emojis::Group::*;
    match g {
        SmileysAndEmotion => ("Smileys & Emotion", "😀"),
        PeopleAndBody => ("People & Body", "👋"),
        AnimalsAndNature => ("Animals & Nature", "🐶"),
        FoodAndDrink => ("Food & Drink", "🍔"),
        TravelAndPlaces => ("Travel & Places", "✈️"),
        Activities => ("Activities", "⚽"),
        Objects => ("Objects", "💡"),
        Symbols => ("Symbols", "❤️"),
        Flags => ("Flags", "🏁"),
    }
}

/// Emoji whose name or shortcodes contain `query` (lowercase): custom
/// emoji first, then Unicode, capped at [`MAX_RESULTS`].
pub fn search_emoji(query: &str, custom: &[CustomEmojiGroup], tone: u8) -> Vec<EmojiRef> {
    let q = query
        .trim()
        .trim_matches(':')
        .to_lowercase()
        .replace('_', " ");
    if q.is_empty() {
        return Vec::new();
    }
    let norm = |s: &str| s.to_lowercase().replace('_', " ");
    let mut out: Vec<EmojiRef> = custom
        .iter()
        .flat_map(|g| g.emojis.iter())
        .filter(|e| norm(&e.name).contains(&q))
        .cloned()
        .collect();
    out.extend(
        emojis::iter()
            .filter(|e| version_ok(e))
            .filter(|e| norm(e.name()).contains(&q) || e.shortcodes().any(|s| norm(s).contains(&q)))
            .map(|e| EmojiRef::unicode(with_tone(e, tone).as_str())),
    );
    out.truncate(MAX_RESULTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_partial_emoji() {
        let u = EmojiRef::from_json(&json!({ "id": null, "name": "👍" })).unwrap();
        assert_eq!(u, EmojiRef::unicode("👍"));
        let c =
            EmojiRef::from_json(&json!({ "id": "123", "name": "pepe", "animated": true })).unwrap();
        assert_eq!(c, EmojiRef::custom("123", "pepe", true));
        // Deleted custom emoji: name null.
        let d = EmojiRef::from_json(&json!({ "id": "9", "name": null })).unwrap();
        assert_eq!(d.name, "");
        assert_eq!(d.text(), ":deleted_emoji:");
        assert!(EmojiRef::from_json(&json!({ "id": null, "name": null })).is_none());
        assert!(EmojiRef::from_json(&json!(null)).is_none());
    }

    #[test]
    fn url_segments() {
        assert_eq!(EmojiRef::unicode("👍").url_segment(), "%F0%9F%91%8D");
        // Skin tone + ZWJ sequences encode every byte.
        assert_eq!(
            EmojiRef::unicode("👍🏽").url_segment(),
            "%F0%9F%91%8D%F0%9F%8F%BD"
        );
        assert_eq!(EmojiRef::unicode("❤️").url_segment(), "%E2%9D%A4%EF%B8%8F");
        assert_eq!(
            EmojiRef::custom("123", "pepe_hi", false).url_segment(),
            "pepe_hi:123"
        );
        assert_eq!(EmojiRef::custom("123", "", true).url_segment(), "_:123");
        assert_eq!(percent_encode("a b/c:d"), "a%20b%2Fc%3Ad");
    }

    #[test]
    fn same_emoji() {
        assert!(EmojiRef::custom("1", "a", false).same(&EmojiRef::custom("1", "renamed", false)));
        assert!(!EmojiRef::custom("1", "a", false).same(&EmojiRef::custom("2", "a", false)));
        assert!(!EmojiRef::custom("1", "👍", false).same(&EmojiRef::unicode("👍")));
        // With and without the variation selector.
        assert!(EmojiRef::unicode("❤").same(&EmojiRef::unicode("❤️")));
        assert!(!EmojiRef::unicode("👍").same(&EmojiRef::unicode("👍🏽")));
    }

    #[test]
    fn text_forms() {
        assert_eq!(
            EmojiRef::custom("5", "pepe", false).message_text(),
            "<:pepe:5>"
        );
        assert_eq!(
            EmojiRef::custom("5", "dance", true).message_text(),
            "<a:dance:5>"
        );
        assert_eq!(EmojiRef::unicode("😀").message_text(), "😀");
        assert_eq!(EmojiRef::unicode("😀").shortcode(), ":grinning:");
        assert_eq!(EmojiRef::unicode("👍").shortcode(), ":thumbsup:");
        assert_eq!(
            EmojiRef::custom("5", "pepe", false).image_url(),
            "https://cdn.discordapp.com/emojis/5.png?size=48"
        );
        assert!(EmojiRef::unicode("😀").image_url().ends_with("/1f600.png"));
    }

    #[test]
    fn picked_insert_text() {
        let p = Picked::Emoji(EmojiRef::custom("5", "pepe", false));
        assert_eq!(p.insert_text(), "<:pepe:5>");
        assert_eq!(Picked::Text("★".into()).insert_text(), "★");
        assert!(Picked::Text("★".into()).emoji().is_none());
    }

    #[test]
    fn recent_is_mru_and_capped() {
        let mut r = RecentEmoji::default();
        r.record(&EmojiRef::unicode("😀"));
        r.record(&EmojiRef::unicode("👍"));
        r.record(&EmojiRef::unicode("😀"));
        assert_eq!(
            r.items,
            vec![EmojiRef::unicode("😀"), EmojiRef::unicode("👍")]
        );
        for i in 0..100 {
            r.record(&EmojiRef::custom(&i.to_string(), "x", false));
        }
        assert_eq!(r.items.len(), RECENT_CAP);
        assert_eq!(r.items[0].id.as_deref(), Some("99"));
        // Round-trips through serde (the caller saves it).
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<RecentEmoji>(&s).unwrap(), r);
    }

    #[test]
    fn search() {
        let custom = [CustomEmojiGroup {
            name: "S".into(),
            emojis: vec![EmojiRef::custom("1", "thumbs_dance", false)],
        }];
        let r = search_emoji("thumbs", &custom, 0);
        assert_eq!(r[0].id.as_deref(), Some("1"));
        assert!(r.contains(&EmojiRef::unicode("👍")));
        assert!(search_emoji(":", &custom, 0).is_empty());
        assert!(search_emoji("zzzznotanemoji", &custom, 0).is_empty());
        // Skin tones apply to emoji that have them, others stay as they are.
        let toned = search_emoji("thumbs up", &[], 3);
        assert!(toned.contains(&EmojiRef::unicode("👍🏽")));
        assert!(search_emoji("sunglasses", &[], 3).contains(&EmojiRef::unicode("😎")));
    }

    #[test]
    fn sections_have_all_groups() {
        let pal = crate::theme::ThemeChoice::DiscordDark.palette();
        let recent = RecentEmoji {
            items: vec![
                EmojiRef::unicode("😀"),
                // Custom emoji from a server we're not in: hidden.
                EmojiRef::custom("77", "elsewhere", false),
            ],
        };
        let tone = Cell::new(0);
        let ctx = PickerCtx {
            pal: &pal,
            images: false,
            custom: &[],
            recent: &recent,
            allow_text: false,
            skin_tone: &tone,
        };
        let s = sections(&ctx);
        assert_eq!(s.len(), 1 + 9);
        assert_eq!(s[0].emojis, vec![EmojiRef::unicode("😀")]);
        assert!(s[1].emojis.len() > 100);
    }
}
