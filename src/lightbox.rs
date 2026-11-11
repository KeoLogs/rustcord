//! Image viewer (the "Image viewer" module): Discord's lightbox. Clicking an
//! image / GIF attachment or an embed picture opens it over the app in a
//! dimmed overlay, fitted to the window.
//!
//! - Wheel (or Ctrl+wheel / pinch) zooms around the pointer, drag pans,
//!   double-click switches between fit and 100 %.
//! - Esc, the X or a click on the dimmed background closes it.
//! - ←/→ (and the side buttons) step through the message's media, "n / m".
//! - Open original (through `State::open_link`), Copy image and Save (bytes
//!   fetched by the backend), Copy link.
//!
//! Memory: exactly one image is decoded, a copy from Discord's media proxy
//! sized to the window × DPI. The full-size file is only loaded when the
//! user zooms past the proxy copy (Settings → Media can turn that off). Both
//! are `forget_image`d when the viewer closes or moves on, frames of animated
//! images included. With the module off nothing here is created.
//!
//! The pure parts (fitting, zoom about a point, pan limits, the sizes to
//! request, URLs) are at the top and unit tested.

use std::collections::HashSet;

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Vec2, pos2, vec2};

use crate::icons;
use crate::theme::Palette;
use crate::widgets;

/// Space kept free around the fitted image for the bars (points).
const MARGIN_X: f32 = 72.0;
const MARGIN_Y: f32 = 64.0;
/// Most zoom: 8 image pixels per screen pixel.
const MAX_ZOOM: f32 = 8.0;
/// Load the full file once the shown size is this much past the proxy copy.
const FULL_SLACK: f32 = 1.1;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// One picture the viewer can show.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaItem {
    /// Identifies the picture within its message (what a click reports).
    pub key: String,
    /// The original link: Open original and Copy link.
    pub original: String,
    /// What Save and Copy image download (attachments: the CDN file;
    /// embeds: Discord's proxy copy, never the third-party host).
    pub download: String,
    /// Proxy URL without size parameters; the viewer adds its own size.
    pub base: String,
    pub filename: String,
    /// Original pixel size, when Discord sent it.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The URI the chat already shows for this picture. The viewer draws it
    /// as a placeholder while loading and never forgets it.
    pub thumb_uri: String,
    /// A GIF: loaded so that egui animates it (see [`gif_uri`]).
    pub animated: bool,
}

impl MediaItem {
    /// URI of the full-size file the viewer loads.
    pub fn full_uri(&self) -> String {
        self.load_uri(self.base.clone())
    }

    /// URI of the proxy copy sized `w` x `h`.
    pub fn sized_uri(&self, w: u32, h: u32) -> String {
        self.load_uri(sized_url(&self.base, w, h))
    }

    fn load_uri(&self, url: String) -> String {
        if self.animated { gif_uri(url) } else { url }
    }

    /// Pixel size as a vector, if known.
    pub fn size(&self) -> Option<Vec2> {
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(vec2(w as f32, h as f32)),
            _ => None,
        }
    }
}

/// Discord's media proxies (media.discordapp.net, images-ext-N) resize on
/// request.
pub fn is_resizing_proxy(url: &str) -> bool {
    crate::links::host(url).is_some_and(|h| h.ends_with(".discordapp.net"))
}

/// `base` with a requested size, when the host can resize. Other hosts get
/// the base URL (there is nothing smaller to ask for).
pub fn sized_url(base: &str, w: u32, h: u32) -> String {
    if !is_resizing_proxy(base) {
        return base.to_string();
    }
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}width={w}&height={h}")
}

/// egui only animates URIs whose path ends in `.gif`; Discord's links
/// carry a query (`?ex=…&width=…`). A trailing dummy parameter ending in
/// `.gif` makes the URI count as a GIF; the proxy ignores it.
pub fn gif_uri(url: String) -> String {
    if egui::load::has_extension(&url, "gif") {
        return url;
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}rc=.gif")
}

/// Is this URL (path, without the query) a GIF?
pub fn is_gif_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".gif")
}

/// Pixel size to request from the proxy: the image fitted into `avail_px`
/// (window × DPI), never larger than the original.
pub fn request_size(image: Vec2, avail_px: Vec2) -> (u32, u32) {
    let s = (avail_px.x / image.x).min(avail_px.y / image.y).min(1.0);
    let w = (image.x * s).round().max(1.0);
    let h = (image.y * s).round().max(1.0);
    (w as u32, h as u32)
}

/// Points per image pixel that fit `image` (pixels) into `view` (points),
/// never larger than 100 % (one image pixel per screen pixel).
pub fn fit_scale(image: Vec2, view: Vec2, ppp: f32) -> f32 {
    (view.x / image.x)
        .min(view.y / image.y)
        .min(1.0 / ppp)
        .max(1e-4)
}

/// Zoom limits: from the fit (or 100 %, if smaller) to `MAX_ZOOM`.
pub fn zoom_limits(fit: f32, ppp: f32) -> (f32, f32) {
    let min = fit.min(1.0 / ppp);
    (min, (MAX_ZOOM / ppp).max(min))
}

/// New pan offset after zooming from `old` to `new` scale about `anchor`
/// (given relative to the view centre), so the point under it stays put.
pub fn zoom_about(offset: Vec2, anchor: Vec2, old: f32, new: f32) -> Vec2 {
    (offset - anchor) * (new / old) + anchor
}

/// Keep the image on screen: an axis that fits is centred, a larger one
/// can pan only until its edge meets the view's edge.
pub fn clamp_offset(offset: Vec2, shown: Vec2, view: Vec2) -> Vec2 {
    let axis = |o: f32, s: f32, v: f32| {
        if s <= v {
            0.0
        } else {
            let m = (s - v) / 2.0;
            o.clamp(-m, m)
        }
    };
    vec2(
        axis(offset.x, shown.x, view.x),
        axis(offset.y, shown.y, view.y),
    )
}

/// Next index when stepping by `delta` (wraps around, like Discord).
pub fn step_index(index: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (index as i64 + delta as i64).rem_euclid(len as i64) as usize
}

/// Should the full-size file be loaded? Only past the proxy copy's pixels,
/// and only if the proxy copy is smaller than the original.
pub fn needs_full(shown_px_w: f32, requested_w: u32, original_w: Option<u32>) -> bool {
    let Some(orig) = original_w else {
        return false;
    };
    requested_w < orig && shown_px_w > requested_w as f32 * FULL_SLACK
}

/// File name for Save, from a URL's last path segment.
pub fn filename_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    if name.is_empty() || !name.contains('.') {
        "image.png".into()
    } else {
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

/// Viewer options (Settings → Media).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The plain wheel zooms (otherwise only Ctrl+wheel / pinch).
    pub wheel_zoom: bool,
    /// Load the full-size file when zooming past the proxy copy.
    pub load_original: bool,
    /// A click on the dimmed background closes the viewer.
    pub click_outside_closes: bool,
}

/// What the user picked in the viewer; the app runs it.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerAction {
    Close,
    /// Through `State::open_link` (the link warning rules apply).
    OpenOriginal(String),
    /// Backend downloads and decodes it, the UI copies the pixels.
    CopyImage(String),
    /// Backend asks where and downloads it: (url, file name).
    Save(String, String),
    CopyLink(String),
}

/// The open viewer.
pub struct Viewer {
    items: Vec<MediaItem>,
    index: usize,
    /// Points per image pixel; `None` = fitted to the window.
    scale: Option<f32>,
    /// Image centre relative to the view centre (points).
    offset: Vec2,
    /// The sized proxy copy for the current item: (uri, requested width).
    proxy: Option<(String, u32)>,
    /// Zoomed past the proxy copy: the full file is wanted.
    want_full: bool,
    /// Every URI this viewer painted (frame URIs of GIFs included), so they
    /// can all be forgotten.
    painted: HashSet<String>,
}

impl Viewer {
    /// Open on `items[index]`. `items` must not be empty.
    pub fn new(items: Vec<MediaItem>, index: usize) -> Self {
        let index = index.min(items.len().saturating_sub(1));
        Self {
            items,
            index,
            scale: None,
            offset: Vec2::ZERO,
            proxy: None,
            want_full: false,
            painted: HashSet::new(),
        }
    }

    fn item(&self) -> &MediaItem {
        &self.items[self.index]
    }

    /// Free everything this viewer loaded (never the chat's own copy).
    pub fn free(&mut self, ctx: &egui::Context) {
        let keep = &self.items[self.index].thumb_uri;
        let mut uris: HashSet<String> = std::mem::take(&mut self.painted);
        if let Some((u, _)) = self.proxy.take() {
            uris.insert(u);
        }
        uris.insert(self.items[self.index].full_uri());
        for uri in uris {
            // Frame URIs ("uri#3") of the chat's copy belong to the chat.
            let base = uri.split('#').next().unwrap_or(&uri);
            if base == keep {
                continue;
            }
            ctx.forget_image(&uri);
        }
        self.want_full = false;
    }

    /// Show another item: free this one first.
    pub fn step(&mut self, ctx: &egui::Context, delta: i32) {
        if self.items.len() < 2 {
            return;
        }
        self.free(ctx);
        self.index = step_index(self.index, self.items.len(), delta);
        self.scale = None;
        self.offset = Vec2::ZERO;
    }

    /// Esc closes, ←/→ step. Call before the rest of the UI draws so the
    /// message box never sees these keys.
    pub fn keys(&mut self, ctx: &egui::Context) -> Option<ViewerAction> {
        use egui::{Key, Modifiers};
        let (esc, left, right) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
            )
        });
        // Typing elsewhere must not happen behind the viewer.
        ctx.memory_mut(|m| m.stop_text_input());
        if esc {
            return Some(ViewerAction::Close);
        }
        if left {
            self.step(ctx, -1);
        } else if right {
            self.step(ctx, 1);
        }
        None
    }

    /// Draw the overlay. Returns what the user picked.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pal: &Palette,
        opts: Options,
        toast: Option<&str>,
    ) -> Option<ViewerAction> {
        let screen = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let mut action = None;
        let mut step = 0;

        egui::Area::new(egui::Id::unique("rc_lightbox"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // The dimmed background takes every click and drag, so
                // nothing under the viewer reacts.
                let bg = ui.allocate_rect(screen, egui::Sense::click_and_drag());
                ui.painter()
                    .rect_filled(screen, 0, Color32::from_black_alpha(215));
                let view = screen.shrink2(vec2(MARGIN_X, MARGIN_Y));
                let item = self.item().clone();

                // Proxy copy sized to the window × DPI, chosen once per item.
                if self.proxy.is_none() {
                    let (w, h) = match item.size() {
                        Some(px) => request_size(px, view.size() * ppp),
                        None => (0, 0),
                    };
                    let uri = if w == 0 {
                        item.full_uri()
                    } else {
                        item.sized_uri(w, h)
                    };
                    self.proxy = Some((uri, w));
                }
                let (proxy_uri, req_w) = self.proxy.clone().unwrap_or_default();

                // Which copy to paint: the full file once it's loaded, else
                // the proxy copy, else the chat's thumbnail with a spinner.
                let full = item.full_uri();
                let full_uri = (self.want_full && proxy_uri != full).then_some(full);
                let loaded = |uri: &str| {
                    matches!(
                        egui::Image::from_uri(uri.to_string()).load_for_size(ctx, view.size()),
                        Ok(egui::load::TexturePoll::Ready { .. })
                    )
                };
                let proxy_ready = loaded(&proxy_uri);
                let full_ready = full_uri.as_deref().is_some_and(loaded);
                if full_ready && proxy_uri != item.thumb_uri {
                    // Only one decoded copy: drop the proxy one.
                    self.forget_uri(ctx, &proxy_uri, &item.thumb_uri);
                }
                let (paint_uri, spinner) = match (&full_uri, full_ready, proxy_ready) {
                    (Some(f), true, _) => (f.clone(), false),
                    (_, _, true) => (proxy_uri.clone(), false),
                    _ => (item.thumb_uri.clone(), true),
                };
                let tex_size = egui::Image::from_uri(paint_uri.clone())
                    .load_for_size(ctx, view.size())
                    .ok()
                    .and_then(|t| t.size());
                let natural = item.size().or(tex_size);

                // Zoom and pan.
                let mut img_rect = None;
                if let Some(px) = natural {
                    let fit = fit_scale(px, view.size(), ppp);
                    let (min, max) = zoom_limits(fit, ppp);
                    let mut scale = self.scale.unwrap_or(fit).clamp(min, max);
                    let pointer = ui.input(|i| i.pointer.hover_pos());
                    let over = pointer.is_some_and(|p| screen.contains(p));
                    let (wheel, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                    let mut factor = pinch;
                    if opts.wheel_zoom && wheel != 0.0 {
                        factor *= (wheel * 0.0025).exp();
                    }
                    if bg.double_clicked() {
                        // Fit <-> 100 %, keeping the clicked point under the pointer.
                        if self.scale.is_some() {
                            self.scale = None;
                            self.offset = Vec2::ZERO;
                        } else {
                            let target = 1.0 / ppp;
                            let anchor = pointer.map_or(Vec2::ZERO, |p| p - view.center());
                            self.offset = zoom_about(self.offset, anchor, scale, target);
                            self.scale = Some(target);
                        }
                        scale = self.scale.unwrap_or(fit);
                    } else if over && (factor - 1.0).abs() > 1e-4 {
                        let new = (scale * factor).clamp(min, max);
                        let anchor = pointer.map_or(Vec2::ZERO, |p| p - view.center());
                        self.offset = zoom_about(self.offset, anchor, scale, new);
                        scale = new;
                        self.scale = if (new - fit).abs() < 1e-4 {
                            None
                        } else {
                            Some(new)
                        };
                    }
                    if bg.dragged() {
                        self.offset += bg.drag_delta();
                    }
                    let shown = px * scale;
                    // Pan limits use the whole screen, so a zoomed image can
                    // reach under the bars.
                    self.offset = clamp_offset(self.offset, shown, screen.size());
                    if self.scale.is_none() {
                        self.offset = Vec2::ZERO;
                    }
                    let rect = Rect::from_center_size(view.center() + self.offset, shown);
                    // Past the proxy copy's pixels: ask for the full file.
                    if opts.load_original
                        && !self.want_full
                        && needs_full(shown.x * ppp, req_w, item.width)
                    {
                        self.want_full = true;
                        ctx.request_repaint();
                    }
                    let img = egui::Image::from_uri(paint_uri.clone())
                        .show_loading_spinner(false)
                        .corner_radius(crate::theme::corner(2));
                    let src = img.source(ctx).uri().map(str::to_string);
                    img.paint_at(ui, rect);
                    if let Some(s) = src {
                        if !self.painted.contains(&s) {
                            self.painted.insert(s);
                        }
                    }
                    if spinner {
                        egui::Spinner::new()
                            .size(32.0)
                            .color(pal.strong)
                            .paint_at(ui, Rect::from_center_size(rect.center(), vec2(32.0, 32.0)));
                    }
                    img_rect = Some(rect);
                    bg.clone().on_hover_cursor(if self.scale.is_some() {
                        egui::CursorIcon::Grab
                    } else {
                        egui::CursorIcon::Default
                    });
                } else {
                    egui::Spinner::new()
                        .size(32.0)
                        .color(pal.strong)
                        .paint_at(ui, Rect::from_center_size(view.center(), vec2(32.0, 32.0)));
                }

                // A click on the background (not the picture) closes.
                if opts.click_outside_closes && bg.clicked() {
                    let on_image = bg
                        .interact_pointer_pos()
                        .zip(img_rect)
                        .is_some_and(|(p, r)| r.contains(p));
                    if !on_image {
                        action = Some(ViewerAction::Close);
                    }
                }

                // ---- Bars (drawn after the background, so they get the clicks).
                let top = Rect::from_min_size(screen.min, vec2(screen.width(), MARGIN_Y));
                let buttons_w = 5.0 * 36.0 + 18.0;
                let info_w = (screen.width() - buttons_w - 40.0).max(0.0);
                if info_w > 60.0 {
                    let info = Rect::from_min_size(
                        top.min + vec2(16.0, 12.0),
                        vec2(info_w, top.height() - 24.0),
                    );
                    info_block(ui, info, &item, self.index, self.items.len(), pal);
                }
                let bar = Rect::from_min_size(
                    pos2(top.right() - 16.0 - buttons_w, top.top() + 12.0),
                    vec2(buttons_w, 40.0),
                );
                if let Some(a) = tool_bar(ui, bar, &item, pal) {
                    action = Some(a);
                }

                // Side arrows (more than one item).
                if self.items.len() > 1 {
                    let mid = screen.center().y;
                    let l =
                        Rect::from_center_size(pos2(screen.left() + 36.0, mid), vec2(40.0, 40.0));
                    let r =
                        Rect::from_center_size(pos2(screen.right() - 36.0, mid), vec2(40.0, 40.0));
                    if side_button(ui, l, icons::CARET_LEFT, "Previous (Left arrow)", pal) {
                        step = -1;
                    }
                    if side_button(ui, r, icons::CARET_RIGHT, "Next (Right arrow)", pal) {
                        step = 1;
                    }
                }

                // Zoom level, bottom centre.
                if let Some(px) = natural {
                    let fit = fit_scale(px, view.size(), ppp);
                    let scale = self.scale.unwrap_or(fit);
                    let pct = (scale * ppp * 100.0).round();
                    let text = if self.scale.is_none() {
                        format!("Fit · {pct}%")
                    } else {
                        format!("{pct}%")
                    };
                    let c = pos2(screen.center().x, screen.bottom() - MARGIN_Y / 2.0);
                    pill(ui, c, &text, pal);
                }
                if let Some(t) = toast {
                    let c = pos2(screen.center().x, screen.bottom() - MARGIN_Y / 2.0 - 36.0);
                    pill(ui, c, t, pal);
                }
            });
        if step != 0 {
            self.step(ctx, step);
        }
        action
    }

    fn forget_uri(&mut self, ctx: &egui::Context, uri: &str, keep: &str) {
        if uri == keep {
            return;
        }
        let frames: Vec<String> = self
            .painted
            .iter()
            .filter(|p| p.split('#').next() == Some(uri))
            .cloned()
            .collect();
        for f in frames {
            self.painted.remove(&f);
            ctx.forget_image(&f);
        }
        ctx.forget_image(uri);
        // The proxy copy is gone for good while this item stays open.
        if let Some((p, _)) = &self.proxy
            && p == uri
        {
            self.proxy = Some((self.items[self.index].full_uri(), u32::MAX));
        }
    }
}

/// File name, "n / m" and the pixel size, top left.
fn info_block(
    ui: &mut egui::Ui,
    rect: Rect,
    item: &MediaItem,
    index: usize,
    len: usize,
    pal: &Palette,
) {
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(
                egui::Label::new(
                    RichText::new(&item.filename)
                        .font(crate::theme::bold_font(15.0))
                        .color(Color32::WHITE),
                )
                .truncate()
                .selectable(false),
            );
            let mut sub = Vec::new();
            if len > 1 {
                sub.push(format!("{} / {len}", index + 1));
            }
            if let (Some(w), Some(h)) = (item.width, item.height) {
                sub.push(format!("{w} × {h}"));
            }
            if !sub.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(sub.join("  ·  ")).size(12.0).color(pal.muted))
                        .truncate()
                        .selectable(false),
                );
            }
        },
    );
}

/// Open original, Copy image, Save, Copy link, X — one block, top right.
fn tool_bar(
    ui: &mut egui::Ui,
    rect: Rect,
    item: &MediaItem,
    pal: &Palette,
) -> Option<ViewerAction> {
    ui.painter().rect(
        rect,
        crate::theme::corner(4),
        pal.menu_bg(),
        egui::Stroke::new(1.0, pal.divider.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let mut out = None;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(4.0, 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let b = |ui: &mut egui::Ui, icon: &str, tip: &str| {
                widgets::icon_button(ui, icon, tip, false, true, pal).clicked()
            };
            if b(ui, icons::ARROW_SQUARE_OUT, "Open original") {
                out = Some(ViewerAction::OpenOriginal(item.original.clone()));
            }
            if b(ui, icons::COPY, "Copy image") {
                out = Some(ViewerAction::CopyImage(item.download.clone()));
            }
            if b(ui, icons::DOWNLOAD, "Save") {
                out = Some(ViewerAction::Save(
                    item.download.clone(),
                    item.filename.clone(),
                ));
            }
            if b(ui, icons::LINK, "Copy link") {
                out = Some(ViewerAction::CopyLink(item.original.clone()));
            }
            let (sep, _) = ui.allocate_exact_size(vec2(1.0, 24.0), egui::Sense::hover());
            ui.painter().rect_filled(sep, 0, pal.divider);
            if b(ui, icons::X, "Close (Esc)") {
                out = Some(ViewerAction::Close);
            }
        },
    );
    out
}

/// A square ‹ / › button at the side.
fn side_button(ui: &mut egui::Ui, rect: Rect, icon: &str, tip: &str, pal: &Palette) -> bool {
    ui.painter().rect(
        rect,
        crate::theme::corner(4),
        pal.menu_bg().gamma_multiply(0.92),
        egui::Stroke::new(1.0, pal.divider.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let mut clicked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(4.0)), |ui| {
        clicked = widgets::icon_button(ui, icon, tip, false, true, pal).clicked();
    });
    clicked
}

/// A small text block centred at `c` (zoom level, "Copied").
fn pill(ui: &egui::Ui, c: Pos2, text: &str, pal: &Palette) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(13.0),
        pal.strong,
    );
    let r = Rect::from_center_size(c, galley.size() + vec2(20.0, 10.0));
    ui.painter().rect(
        r,
        crate::theme::corner(4),
        pal.menu_bg(),
        egui::Stroke::new(1.0, pal.divider.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(r.center() - galley.size() / 2.0, galley, pal.strong);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_size_fits_window_and_never_upscales() {
        // 4000 x 3000 into a 1920 x 1080 px area.
        assert_eq!(
            request_size(vec2(4000.0, 3000.0), vec2(1920.0, 1080.0)),
            (1440, 1080)
        );
        // A small image keeps its size.
        assert_eq!(
            request_size(vec2(300.0, 200.0), vec2(1920.0, 1080.0)),
            (300, 200)
        );
    }

    #[test]
    fn sized_url_only_for_discord_proxies() {
        let p = "https://media.discordapp.net/attachments/1/2/cat.png";
        assert_eq!(sized_url(p, 10, 20), format!("{p}?width=10&height=20"));
        let q = "https://media.discordapp.net/attachments/1/2/cat.png?ex=1";
        assert_eq!(sized_url(q, 10, 20), format!("{q}&width=10&height=20"));
        let other = "https://example.org/cat.png";
        assert_eq!(sized_url(other, 10, 20), other);
        assert!(!is_resizing_proxy(
            "https://discordapp.net.evil.example/x.png"
        ));
    }

    #[test]
    fn fit_never_passes_one_to_one() {
        // Big image: fitted to the view.
        let s = fit_scale(vec2(2000.0, 1000.0), vec2(1000.0, 1000.0), 1.0);
        assert!((s - 0.5).abs() < 1e-6);
        // Small image: 100 % (1 image px per screen px; 2 px per point).
        let s = fit_scale(vec2(100.0, 100.0), vec2(1000.0, 1000.0), 2.0);
        assert!((s - 0.5).abs() < 1e-6);
        let (min, max) = zoom_limits(0.25, 1.0);
        assert_eq!((min, max), (0.25, MAX_ZOOM));
    }

    #[test]
    fn zoom_keeps_point_under_pointer() {
        // Image point under the pointer: (anchor - offset) / scale.
        let (offset, anchor) = (vec2(10.0, -5.0), vec2(100.0, 50.0));
        let before = (anchor - offset) / 0.5;
        let o2 = zoom_about(offset, anchor, 0.5, 2.0);
        let after = (anchor - o2) / 2.0;
        assert!((before - after).length() < 1e-3);
    }

    #[test]
    fn pan_is_limited() {
        let view = vec2(800.0, 600.0);
        // Fits: centred.
        assert_eq!(
            clamp_offset(vec2(50.0, 50.0), vec2(400.0, 300.0), view),
            Vec2::ZERO
        );
        // Wider than the view: up to the edge.
        assert_eq!(
            clamp_offset(vec2(500.0, 0.0), vec2(1200.0, 300.0), view),
            vec2(200.0, 0.0)
        );
    }

    #[test]
    fn stepping_wraps() {
        assert_eq!(step_index(0, 3, -1), 2);
        assert_eq!(step_index(2, 3, 1), 0);
        assert_eq!(step_index(1, 3, 1), 2);
        assert_eq!(step_index(0, 0, 1), 0);
    }

    #[test]
    fn full_file_only_past_the_proxy_copy() {
        assert!(!needs_full(1000.0, 1440, Some(4000)));
        assert!(needs_full(2000.0, 1440, Some(4000)));
        // The proxy copy already is the original.
        assert!(!needs_full(2000.0, 300, Some(300)));
        assert!(!needs_full(2000.0, 300, None));
    }

    #[test]
    fn gifs_get_a_gif_uri() {
        assert_eq!(gif_uri("https://x.org/a.gif".into()), "https://x.org/a.gif");
        let u = gif_uri("https://media.discordapp.net/a.gif?ex=1&width=2".into());
        assert_eq!(u, "https://media.discordapp.net/a.gif?ex=1&width=2&rc=.gif");
        assert!(egui::load::has_extension(&u, "gif"));
        assert!(is_gif_url("https://cdn.discordapp.com/a/B.GIF?ex=1"));
        assert!(!is_gif_url("https://cdn.discordapp.com/a/b.png?x=.gif"));
    }

    #[test]
    fn save_names() {
        assert_eq!(filename_of("https://x.org/a/cat.gif?size=4"), "cat.gif");
        assert_eq!(filename_of("https://x.org/a/"), "image.png");
        assert_eq!(filename_of("https://x.org/view/12345"), "image.png");
    }
}
