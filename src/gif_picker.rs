//! GIF picker (GIF picker module): the GIF button in the message box opens
//! a popup with a search box, trending categories and a grid of GIFs, like
//! Discord's.
//!
//! Data comes from Discord's own GIF endpoints (user accounts), called by
//! the backend (`ToBackend::LoadGifs`):
//!
//! - `GET /gifs/trending?provider=tenor&locale=en-US&media_format=gif` →
//!   `{ "categories": [{ "name", "src" }], "gifs": [{…}] }`
//! - `GET /gifs/search?q=…&provider=tenor&locale=en-US&media_format=gif` →
//!   `[{ "url", "src", "gif_src", "preview", "width", "height" }]`
//!
//! These endpoints are undocumented, so parsing is lenient (both an array and
//! an object with `gifs` / `results` are accepted; missing fields are
//! skipped). `src` with `media_format=gif` is a .gif; `preview` is a still
//! image. To keep memory low the grid shows still previews and only the
//! hovered GIF is animated; every texture is freed when the popup closes
//! (`GifPicker::free`). Picking a GIF sends its link like Discord does
//! (an explicit click by the user).
//!
//! Favourites: the star on each GIF adds it to (or removes it from)
//! `Config::favourite_gifs`, shown in the Favourites tab. They are only
//! saved locally, and only when the star is clicked (no settings sync).

use std::collections::HashSet;

use eframe::egui::{self, Color32, RichText, vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::icons;
use crate::theme::{Bold, Palette};

/// One GIF (also what a favourite saves).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gif {
    /// Link that goes into the message (Tenor page; Discord embeds it).
    pub url: String,
    /// Animated GIF to show on hover.
    pub gif: String,
    /// Still preview for the grid (falls back to `gif`).
    pub preview: String,
    pub width: f32,
    pub height: f32,
}

/// A trending search term with a picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    pub src: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GifResults {
    pub gifs: Vec<Gif>,
    pub categories: Vec<Category>,
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

fn parse_gif(v: &Value) -> Option<Gif> {
    let gif = str_field(v, &["gif_src", "src"])?.to_string();
    let url = str_field(v, &["url"]).unwrap_or(&gif).to_string();
    // A still preview, unless it's an mp4 (egui can't show video).
    let preview = str_field(v, &["preview"])
        .filter(|p| !p.ends_with(".mp4"))
        .map(str::to_string)
        .unwrap_or_else(|| gif.clone());
    let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some(Gif {
        url,
        gif,
        preview,
        width: num("width"),
        height: num("height"),
    })
}

/// Parse a trending / search response (see the module docs for shapes).
pub fn parse(v: &Value) -> GifResults {
    let list = v
        .as_array()
        .or_else(|| v.get("gifs").and_then(Value::as_array))
        .or_else(|| v.get("results").and_then(Value::as_array));
    let gifs = list
        .map(|a| a.iter().filter_map(parse_gif).collect())
        .unwrap_or_default();
    let categories = v
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(Category {
                        name: str_field(c, &["name"])?.to_string(),
                        src: str_field(c, &["src"]).unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    GifResults { gifs, categories }
}

/// What the user did in the picker.
#[derive(Debug, Clone, PartialEq)]
pub enum GifAction {
    /// Load trending (None) or search.
    Load(Option<String>),
    /// Send this GIF link.
    Pick(String),
    /// Star clicked: add to / remove from Favourites.
    ToggleFavourite(Gif),
    Close,
}

/// The picker's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    /// Trending / search results.
    #[default]
    Gifs,
    Favourites,
}

/// Star / unstar `gif` (matched by its link); new favourites go first.
pub fn toggle_favourite(list: &mut Vec<Gif>, gif: &Gif) {
    if let Some(i) = list.iter().position(|g| g.url == gif.url) {
        list.remove(i);
    } else {
        list.insert(0, gif.clone());
    }
}

/// Popup state. Dropped (after `free`) when closed.
#[derive(Default)]
pub struct GifPicker {
    pub query: String,
    /// What the shown results are for (None = trending).
    pub shown: Option<Option<String>>,
    pub results: GifResults,
    pub loading: bool,
    pub error: Option<String>,
    pub tab: Tab,
    /// Image URIs drawn so far, freed on close.
    uris: HashSet<String>,
    /// Query typed but not sent yet (sent after a short pause).
    typed_at: Option<f64>,
}

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 440.0;

impl GifPicker {
    /// Results arrived.
    pub fn set_results(&mut self, query: Option<String>, results: GifResults) {
        self.shown = Some(query);
        self.results = results;
        self.loading = false;
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.loading = false;
        self.error = Some(error);
    }

    /// Free every GIF / preview texture this picker loaded.
    pub fn free(&self, ctx: &egui::Context) {
        for uri in &self.uris {
            ctx.forget_image(uri);
        }
    }

    /// Draw the popup above `anchor` (the GIF button).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        anchor: egui::Rect,
        images: bool,
        favourites: Option<&[Gif]>,
        pal: &Palette,
    ) -> Option<GifAction> {
        let mut action = None;
        // Favourite GIFs module off (`None`): only the GIFs tab.
        if favourites.is_none() {
            self.tab = Tab::Gifs;
        }
        if self.tab == Tab::Gifs && self.shown.is_none() && !self.loading {
            self.loading = true;
            action = Some(GifAction::Load(None));
        }
        let pos = egui::pos2(anchor.right() - WIDTH, anchor.top() - HEIGHT - 12.0);
        let area = egui::Area::new(egui::Id::unique("rc_gif_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                crate::widgets::menu_frame(pal).show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.set_height(HEIGHT);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("GIFs").bold().size(15.0).color(pal.strong));
                        ui.label(RichText::new("via Tenor").size(12.0).color(pal.muted));
                    });
                    ui.add_space(4.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text(format!("{}  Search Tenor", icons::MAGNIFYING_GLASS))
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                    if !edit.has_focus() && self.query.is_empty() && self.shown.is_none() {
                        edit.request_focus();
                    }
                    let now = ui.input(|i| i.time);
                    if edit.changed() {
                        self.typed_at = Some(now);
                        // Searching shows results, not favourites.
                        self.tab = Tab::Gifs;
                    }
                    // Search after a short pause (or Enter), not every key.
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if let Some(at) = self.typed_at {
                        if enter || now - at > 0.45 {
                            self.typed_at = None;
                            let q = self.query.trim().to_string();
                            let want = (!q.is_empty()).then_some(q);
                            if self.shown.as_ref() != Some(&want) {
                                self.loading = true;
                                action = Some(GifAction::Load(want));
                            }
                        } else {
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(100));
                        }
                    }
                    ui.add_space(6.0);
                    if favourites.is_some() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (tab, icon, label) in [
                                (Tab::Gifs, icons::GIF, "GIFs"),
                                (Tab::Favourites, icons::STAR, "Favourites"),
                            ] {
                                if tab_button(ui, icon, label, self.tab == tab, pal).clicked() {
                                    self.tab = tab;
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                    let a = match (self.tab, favourites) {
                        (Tab::Gifs, _) | (Tab::Favourites, None) => {
                            self.grid(ui, images, favourites, pal)
                        }
                        (Tab::Favourites, Some([])) => {
                            empty_favourites(ui, pal);
                            None
                        }
                        (Tab::Favourites, Some(favourites)) => {
                            egui::ScrollArea::vertical()
                                .auto_shrink(false)
                                .show(ui, |ui| {
                                    gif_grid(
                                        ui,
                                        favourites,
                                        Some(favourites),
                                        &mut self.uris,
                                        images,
                                        pal,
                                    )
                                })
                                .inner
                        }
                    };
                    if a.is_some() {
                        action = a;
                    }
                });
            });
        // Click outside or Esc closes.
        let outside = ctx.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p))
        });
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(GifAction::Close);
        }
        action
    }

    fn grid(
        &mut self,
        ui: &mut egui::Ui,
        images: bool,
        favourites: Option<&[Gif]>,
        pal: &Palette,
    ) -> Option<GifAction> {
        let mut action = None;
        if let Some(e) = &self.error {
            ui.label(RichText::new(format!("Couldn't load GIFs: {e}")).color(pal.danger));
            if ui.button("Retry").clicked() {
                self.loading = true;
                let q = self.query.trim().to_string();
                action = Some(GifAction::Load((!q.is_empty()).then_some(q)));
            }
            return action;
        }
        if self.loading && self.results.gifs.is_empty() {
            ui.centered_and_justified(|ui| ui.spinner());
            return None;
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                // Trending categories as chips (click = search).
                let trending = matches!(self.shown, Some(None));
                if trending && !self.results.categories.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for c in self.results.categories.iter().take(16) {
                            let chip = ui.add(
                                egui::Button::new(RichText::new(&c.name).size(13.0))
                                    .fill(pal.input)
                                    .corner_radius(12),
                            );
                            if chip.clicked() {
                                self.query = c.name.clone();
                                self.loading = true;
                                action = Some(GifAction::Load(Some(c.name.clone())));
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                if self.results.gifs.is_empty() {
                    ui.label(RichText::new("No GIFs found").color(pal.muted));
                    return;
                }
                if let Some(a) = gif_grid(
                    ui,
                    &self.results.gifs,
                    favourites,
                    &mut self.uris,
                    images,
                    pal,
                ) {
                    action = Some(a);
                }
            });
        action
    }
}

/// A tab in the picker header (icon + label, filled when selected).
fn tab_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    selected: bool,
    pal: &Palette,
) -> egui::Response {
    let col = if selected { pal.strong } else { pal.muted };
    let button = egui::Button::new(
        RichText::new(format!("{icon}  {label}"))
            .size(13.0)
            .color(col),
    )
    .corner_radius(4)
    .min_size(vec2(0.0, 26.0));
    let button = if selected {
        button.fill(pal.selected)
    } else {
        button.frame(false)
    };
    ui.add(button)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Favourites tab with nothing starred yet.
fn empty_favourites(ui: &mut egui::Ui, pal: &Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.25);
        ui.label(RichText::new(icons::STAR).size(40.0).color(pal.muted));
        ui.add_space(6.0);
        ui.label(
            RichText::new("No favourite GIFs yet")
                .bold()
                .size(15.0)
                .color(pal.strong),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new("Click the star on any GIF to keep it here.")
                .size(13.0)
                .color(pal.muted),
        );
    });
}

/// Two columns of GIFs (each keeps its aspect ratio), with a star in the
/// corner of each: shown on hover, always shown (filled) for favourites.
/// `favourites` is `None` when the Favourite GIFs module is off (no star).
fn gif_grid(
    ui: &mut egui::Ui,
    gifs: &[Gif],
    favourites: Option<&[Gif]>,
    uris: &mut HashSet<String>,
    images: bool,
    pal: &Palette,
) -> Option<GifAction> {
    let mut action = None;
    let col_w = ((ui.available_width() - 8.0) / 2.0).max(40.0);
    let mut cols = [Vec::new(), Vec::new()];
    let mut heights = [0.0f32, 0.0];
    for g in gifs {
        let h = if g.width > 0.0 && g.height > 0.0 {
            (col_w * g.height / g.width).clamp(60.0, 260.0)
        } else {
            col_w * 0.75
        };
        let c = if heights[0] <= heights[1] { 0 } else { 1 };
        heights[c] += h + 8.0;
        cols[c].push((g, h));
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for col in &cols {
            ui.vertical(|ui| {
                ui.set_width(col_w);
                for (g, h) in col {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(col_w, *h), egui::Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    ui.painter().rect_filled(rect, 6, pal.input);
                    // The star sits on top, so ask the pointer, not `resp`.
                    let hovered = ui.rect_contains_pointer(rect);
                    if images {
                        let uri = if hovered { &g.gif } else { &g.preview };
                        uris.insert(uri.clone());
                        egui::Image::from_uri(uri.as_str())
                            .corner_radius(6)
                            .show_loading_spinner(false)
                            .paint_at(ui, rect);
                    } else {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            icons::GIF,
                            icons::font(28.0),
                            pal.muted,
                        );
                    }
                    if hovered {
                        ui.painter().rect_stroke(
                            rect,
                            6,
                            egui::Stroke::new(2.0, pal.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let resp = resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Click to send");
                    if resp.clicked() {
                        action = Some(GifAction::Pick(g.url.clone()));
                    }
                    if let Some(favourites) = favourites {
                        let fav = favourites.iter().any(|f| f.url == g.url);
                        if (hovered || fav) && star(ui, rect, &g.url, fav, pal) {
                            action = Some(GifAction::ToggleFavourite((*g).clone()));
                        }
                    }
                }
            });
        }
    });
    action
}

/// The favourite star in a GIF's top-right corner. Returns clicked.
fn star(ui: &mut egui::Ui, tile: egui::Rect, url: &str, fav: bool, pal: &Palette) -> bool {
    let size = 26.0;
    let rect = egui::Rect::from_min_size(
        egui::pos2(tile.right() - size - 6.0, tile.top() + 6.0),
        vec2(size, size),
    );
    let resp = ui.interact(
        rect,
        ui.scope_id().with(("rc_gif_star", url)),
        egui::Sense::click(),
    );
    // A dark disc so the star reads on any GIF (it sits on the picture,
    // not on a theme surface).
    let alpha = if resp.hovered() { 200 } else { 150 };
    ui.painter()
        .circle_filled(rect.center(), size / 2.0, Color32::from_black_alpha(alpha));
    let (font, col) = if fav {
        (icons::fill_font(16.0), pal.idle)
    } else if resp.hovered() {
        (icons::font(16.0), pal.idle)
    } else {
        (icons::font(16.0), Color32::WHITE)
    };
    icons::paint(ui, rect, icons::STAR, font, col);
    let tip = if fav {
        "Remove from Favourites"
    } else {
        "Add to Favourites"
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
        .clicked()
}

/// Percent-encode a search query for a URL.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fake results for demo mode (links don't load offline).
pub fn demo_results(query: Option<&str>) -> GifResults {
    let names = [
        "happy",
        "cat",
        "dance",
        "thumbs up",
        "wow",
        "facepalm",
        "party",
        "sad",
    ];
    let gifs = (0..10)
        .map(|i| {
            let tag = query.unwrap_or(names[i % names.len()]).replace(' ', "-");
            Gif {
                url: format!("https://tenor.com/view/{tag}-gif-{}", 1000 + i),
                gif: format!("https://media.tenor.com/demo{i}/{tag}.gif"),
                preview: format!("https://media.tenor.com/demo{i}/{tag}.png"),
                width: 220.0,
                height: [160.0, 220.0, 124.0, 180.0][i % 4],
            }
        })
        .collect();
    let categories = if query.is_none() {
        names
            .iter()
            .map(|n| Category {
                name: n.to_string(),
                src: String::new(),
            })
            .collect()
    } else {
        Vec::new()
    };
    GifResults { gifs, categories }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_search_array() {
        let v = json!([
            { "url": "https://tenor.com/view/a", "src": "https://media.tenor.com/a.gif",
              "preview": "https://media.tenor.com/a.png", "width": 200, "height": 100 },
            { "url": "https://tenor.com/view/b", "gif_src": "https://media.tenor.com/b.gif",
              "src": "https://media.tenor.com/b.mp4", "preview": "https://media.tenor.com/b.mp4" },
            { "url": "no media" },
        ]);
        let r = parse(&v);
        assert_eq!(r.gifs.len(), 2);
        assert_eq!(r.gifs[0].width, 200.0);
        assert_eq!(r.gifs[0].preview, "https://media.tenor.com/a.png");
        // gif_src wins over an mp4 src; mp4 previews fall back to the gif.
        assert_eq!(r.gifs[1].gif, "https://media.tenor.com/b.gif");
        assert_eq!(r.gifs[1].preview, "https://media.tenor.com/b.gif");
    }

    #[test]
    fn parse_trending_object() {
        let v = json!({
            "categories": [{ "name": "happy", "src": "https://x/happy.mp4" }, { "src": "x" }],
            "gifs": [{ "url": "u", "src": "https://media.tenor.com/c.gif" }],
        });
        let r = parse(&v);
        assert_eq!(r.categories.len(), 1);
        assert_eq!(r.categories[0].name, "happy");
        assert_eq!(r.gifs.len(), 1);
        assert_eq!(r.gifs[0].url, "u");
        assert_eq!(parse(&json!({ "nothing": 1 })), GifResults::default());
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(url_encode("thumbs up"), "thumbs%20up");
        assert_eq!(url_encode("a&b=ç"), "a%26b%3D%C3%A7");
    }

    #[test]
    fn toggles_favourites() {
        let gifs = demo_results(None).gifs;
        let mut favs = Vec::new();
        toggle_favourite(&mut favs, &gifs[0]);
        toggle_favourite(&mut favs, &gifs[1]);
        // Newest first.
        assert_eq!(favs, vec![gifs[1].clone(), gifs[0].clone()]);
        toggle_favourite(&mut favs, &gifs[1]);
        assert_eq!(favs, vec![gifs[0].clone()]);
    }

    #[test]
    fn partial_favourites_load() {
        // A favourite saved without some fields still loads.
        let g: Gif = serde_json::from_str(r#"{"url":"u","gif":"g"}"#).unwrap();
        assert_eq!(g.url, "u");
        assert_eq!(g.preview, "");
    }

    #[test]
    fn demo_has_results() {
        assert!(!demo_results(None).categories.is_empty());
        assert_eq!(demo_results(Some("cat")).gifs.len(), 10);
    }
}
