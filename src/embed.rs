//! Message embeds: link previews (YouTube, articles, images, GIFs) and the
//! rich embeds bots send.
//!
//! Parsing follows Discord Messenger (`core/models/Message.cpp`,
//! `RichEmbed::Load`) and is lenient like `types.rs`: a missing or odd
//! field becomes an empty value instead of an error.
//!
//! Drawing ([`render_embed`]) never opens anything: clicked links come back
//! in [`EmbedOutput`] so the caller can run them through the link warning
//! (`links::check`). Images always load through Discord's media proxy
//! (`proxy_url`), resized to the size they're shown at, so third-party
//! sites don't see the user's IP and big images don't fill RAM.

use std::ops::Range;

use eframe::egui::{self, Color32, CornerRadius, RichText, vec2};
use serde_json::Value;

use crate::theme::Palette;

/// Widest an embed card gets (Discord: 516px + bar).
pub const MAX_WIDTH: f32 = 520.0;
/// Large image inside a card, or a bare image / GIF embed.
pub const IMAGE_MAX: (f32, f32) = (400.0, 350.0);
/// Video preview (YouTube etc.).
pub const VIDEO_MAX: (f32, f32) = (400.0, 225.0);
/// Small thumbnail at the top right of a card.
pub const THUMB_MAX: (f32, f32) = (80.0, 80.0);
const AUTHOR_ICON: f32 = 24.0;
const FOOTER_ICON: f32 = 20.0;
/// Discord allows 25 fields; ignore anything past that.
const MAX_FIELDS: usize = 25;
/// Discord allows 10 embeds per message.
const MAX_EMBEDS: usize = 10;
const BAR_WIDTH: f32 = 4.0;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EmbedKind {
    #[default]
    Rich,
    Image,
    Video,
    /// An animated GIF shown as a muted looping video (Tenor, Giphy).
    Gifv,
    Article,
    Link,
    /// Anything else (poll_result, auto_moderation_message, …).
    Other(String),
}

impl EmbedKind {
    fn parse(s: &str) -> Self {
        match s {
            "" | "rich" => Self::Rich,
            "image" => Self::Image,
            "video" => Self::Video,
            "gifv" => Self::Gifv,
            "article" => Self::Article,
            "link" => Self::Link,
            other => Self::Other(other.to_string()),
        }
    }
}

/// An image, thumbnail or video.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedMedia {
    /// Original URL (what a click opens).
    pub url: String,
    /// Discord's media proxy copy (what we load). May be empty.
    pub proxy_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedAuthor {
    pub name: String,
    pub url: Option<String>,
    /// Proxied icon URL if Discord gave one, else the original.
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedFooter {
    pub text: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbedProvider {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embed {
    pub kind: EmbedKind,
    /// Left bar colour, 0xRRGGBB. None = Discord's default grey.
    pub color: Option<u32>,
    pub author: Option<EmbedAuthor>,
    pub title: String,
    pub url: Option<String>,
    pub description: String,
    pub fields: Vec<EmbedField>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub footer: Option<EmbedFooter>,
    /// ISO-8601 timestamp shown in the footer.
    pub timestamp: Option<String>,
    pub provider: Option<EmbedProvider>,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A non-empty string field.
fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn dim(v: &Value, key: &str) -> Option<u32> {
    let n = v.get(key)?;
    let n = n.as_u64().or_else(|| n.as_f64().map(|f| f as u64))?;
    (n > 0).then(|| n.min(u32::MAX as u64) as u32)
}

/// Embed colour: normally an integer, but accept "#rrggbb" too.
fn color_of(v: &Value) -> Option<u32> {
    let c = v.get("color")?;
    let c = c.as_u64().or_else(|| {
        c.as_str().and_then(|s| {
            let s = s.trim();
            match s.strip_prefix('#') {
                Some(hex) => u32::from_str_radix(hex, 16).ok().map(u64::from),
                None => s.parse().ok(),
            }
        })
    })?;
    (c <= 0xFF_FFFF).then_some(c as u32)
}

impl EmbedMedia {
    fn from_json(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let m = Self {
            url: str_of(v, "url"),
            proxy_url: str_of(v, "proxy_url"),
            width: dim(v, "width"),
            height: dim(v, "height"),
        };
        (!m.url.is_empty() || !m.proxy_url.is_empty()).then_some(m)
    }

    /// The URL to load: the proxy copy if there is one.
    pub fn source(&self) -> &str {
        if self.proxy_url.is_empty() {
            &self.url
        } else {
            &self.proxy_url
        }
    }

    /// The URL a click opens.
    pub fn link(&self) -> &str {
        if self.url.is_empty() {
            &self.proxy_url
        } else {
            &self.url
        }
    }

    /// Size it's shown at (keeping aspect ratio, never upscaled).
    pub fn display_size(&self, max_w: f32, max_h: f32) -> Option<(f32, f32)> {
        let (w, h) = (self.width? as f32, self.height? as f32);
        let scale = (max_w / w).min(max_h / h).min(1.0);
        Some(((w * scale).max(1.0), (h * scale).max(1.0)))
    }

    /// URL to load for a box of `max_w` x `max_h`. Discord's proxies resize
    /// on request, so ask for the shown size rather than the full image.
    pub fn sized_url(&self, max_w: f32, max_h: f32) -> String {
        let base = self.source();
        let Some((w, h)) = self.display_size(max_w, max_h) else {
            return base.to_string();
        };
        let shrunk = self.width.is_some_and(|ow| (w.round() as u32) < ow);
        if !shrunk || !is_discord_proxy(base) {
            return base.to_string();
        }
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}width={}&height={}", w.round(), h.round())
    }
}

/// media.discordapp.net / images-ext-N.discordapp.net accept width/height.
fn is_discord_proxy(url: &str) -> bool {
    crate::links::host(url).is_some_and(|h| h.ends_with(".discordapp.net"))
}

impl Embed {
    pub fn from_json(v: &Value) -> Self {
        let author = v
            .get("author")
            .filter(|a| a.is_object())
            .map(|a| EmbedAuthor {
                name: str_of(a, "name"),
                url: opt_str(a, "url"),
                icon_url: opt_str(a, "proxy_icon_url").or_else(|| opt_str(a, "icon_url")),
            })
            .filter(|a| !a.name.is_empty());
        let footer = v
            .get("footer")
            .filter(|f| f.is_object())
            .map(|f| EmbedFooter {
                text: str_of(f, "text"),
                icon_url: opt_str(f, "proxy_icon_url").or_else(|| opt_str(f, "icon_url")),
            })
            .filter(|f| !f.text.is_empty());
        let provider = v
            .get("provider")
            .filter(|p| p.is_object())
            .map(|p| EmbedProvider {
                name: str_of(p, "name"),
                url: opt_str(p, "url"),
            })
            .filter(|p| !p.name.is_empty());
        let fields = v
            .get("fields")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|f| f.is_object())
                    .map(|f| EmbedField {
                        name: str_of(f, "name"),
                        value: str_of(f, "value"),
                        inline: f.get("inline").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .filter(|f| !f.name.is_empty() || !f.value.is_empty())
                    .take(MAX_FIELDS)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kind: EmbedKind::parse(v.get("type").and_then(Value::as_str).unwrap_or("")),
            color: color_of(v),
            author,
            title: str_of(v, "title"),
            url: opt_str(v, "url"),
            description: str_of(v, "description"),
            fields,
            image: v.get("image").and_then(EmbedMedia::from_json),
            thumbnail: v.get("thumbnail").and_then(EmbedMedia::from_json),
            video: v.get("video").and_then(EmbedMedia::from_json),
            footer,
            timestamp: opt_str(v, "timestamp"),
            provider,
        }
    }

    /// Nothing worth drawing.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.description.is_empty()
            && self.fields.is_empty()
            && self.author.is_none()
            && self.footer.is_none()
            && self.provider.is_none()
            && self.image.is_none()
            && self.thumbnail.is_none()
            && self.video.is_none()
    }

    /// Image / GIF link previews are drawn as the bare picture, no card.
    pub fn is_bare_media(&self) -> bool {
        matches!(self.kind, EmbedKind::Image | EmbedKind::Gifv) && self.main_media().is_some()
    }

    /// The picture for a bare image/GIF embed or a video preview.
    fn main_media(&self) -> Option<&EmbedMedia> {
        self.thumbnail.as_ref().or(self.image.as_ref())
    }

    /// Does this card show its thumbnail as a large preview (videos)?
    fn big_thumbnail(&self) -> bool {
        self.kind == EmbedKind::Video && self.image.is_none()
    }

    /// Where a click on the picture goes.
    fn media_link(&self, media: &EmbedMedia) -> String {
        match self.kind {
            EmbedKind::Video | EmbedKind::Gifv => self
                .url
                .clone()
                .or_else(|| self.video.as_ref().map(|v| v.link().to_string()))
                .unwrap_or_else(|| media.link().to_string()),
            _ => media.link().to_string(),
        }
    }

    /// Every image URI [`render_embed`] may load for this embed, so the
    /// caller can `ctx.forget_image` them when the channel is evicted.
    pub fn image_uris(&self) -> Vec<String> {
        let mut uris = Vec::new();
        if let Some(icon) = self.author.as_ref().and_then(|a| a.icon_url.clone()) {
            uris.push(icon);
        }
        if let Some(icon) = self.footer.as_ref().and_then(|f| f.icon_url.clone()) {
            uris.push(icon);
        }
        if self.is_bare_media() {
            if let Some(m) = self.main_media() {
                uris.push(m.sized_url(IMAGE_MAX.0, IMAGE_MAX.1));
            }
            return uris;
        }
        if let Some(t) = &self.thumbnail {
            uris.push(if self.big_thumbnail() {
                t.sized_url(VIDEO_MAX.0, VIDEO_MAX.1)
            } else {
                t.sized_url(THUMB_MAX.0, THUMB_MAX.1)
            });
        }
        if let Some(i) = &self.image {
            uris.push(i.sized_url(IMAGE_MAX.0, IMAGE_MAX.1));
        }
        uris
    }

    /// One-line summary for previews (pins, replies, notifications).
    pub fn summary(&self) -> String {
        [
            &self.title,
            &self.description,
            self.author.as_ref().map_or(&String::new(), |a| &a.name),
        ]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .map(|s| s.replace('\n', " "))
        .unwrap_or_else(|| match self.kind {
            EmbedKind::Gifv => "GIF".into(),
            EmbedKind::Image => "Image".into(),
            EmbedKind::Video => "Video".into(),
            _ => "Embed".into(),
        })
    }
}

/// A message's embeds (empty if it has none). Use for MESSAGE_CREATE and
/// history.
pub fn parse_embeds(message: &Value) -> Vec<Embed> {
    embeds_patch(message).unwrap_or_default()
}

/// For MESSAGE_UPDATE: `None` if the payload has no `embeds` key (leave the
/// old ones), otherwise the new list. Discord sends link previews this way,
/// a moment after the message itself.
pub fn embeds_patch(message: &Value) -> Option<Vec<Embed>> {
    let arr = message.get("embeds")?.as_array()?;
    Some(
        arr.iter()
            .filter(|e| e.is_object())
            .map(Embed::from_json)
            .filter(|e| !e.is_empty())
            .take(MAX_EMBEDS)
            .collect(),
    )
}

/// Group fields into rows: consecutive inline fields share a row (up to
/// `per_row`), others get a row each.
pub fn field_rows(fields: &[EmbedField], per_row: usize) -> Vec<Range<usize>> {
    let per_row = per_row.max(1);
    let mut rows = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let mut end = i + 1;
        if fields[i].inline {
            while end < fields.len() && fields[end].inline && end - i < per_row {
                end += 1;
            }
        }
        rows.push(i..end);
        i = end;
    }
    rows
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draws markdown text and returns a clicked link, if any.
pub type TextFn<'a> = &'a dyn Fn(&mut egui::Ui, &str) -> Option<String>;

/// Settings for [`render_embed`].
pub struct EmbedCtx<'a> {
    pub pal: &'a Palette,
    /// "Show embeds" (Discord Messenger: ShowEmbedContent). Off = draw nothing.
    pub show_content: bool,
    /// "Show images in embeds" (ShowEmbedImages).
    pub show_images: bool,
    /// The Images module (`Config::modules.images`). Off = load no images.
    pub images_enabled: bool,
    /// Widest the card may be (clamped to [`MAX_WIDTH`]).
    pub max_width: f32,
    /// Markdown renderer for descriptions and field values. `None` = plain
    /// text.
    pub text: Option<TextFn<'a>>,
}

impl<'a> EmbedCtx<'a> {
    pub fn new(pal: &'a Palette) -> Self {
        Self {
            pal,
            show_content: true,
            show_images: true,
            images_enabled: true,
            max_width: MAX_WIDTH,
            text: None,
        }
    }

    fn images(&self) -> bool {
        self.show_images && self.images_enabled
    }
}

/// What happened while drawing an embed.
#[derive(Default)]
pub struct EmbedOutput {
    /// A link the user clicked. Run it through `links::check`; never open
    /// it directly.
    pub clicked_link: Option<String>,
    /// The whole embed (for hover / context menus). None if nothing drawn.
    pub response: Option<egui::Response>,
}

/// Draw one embed. See [`EmbedCtx`] for the switches.
pub fn render_embed(ui: &mut egui::Ui, embed: &Embed, ctx: &EmbedCtx) -> EmbedOutput {
    let mut out = EmbedOutput::default();
    if !ctx.show_content || embed.is_empty() {
        return out;
    }
    let max_width = ctx.max_width.min(MAX_WIDTH).min(ui.available_width());

    if embed.is_bare_media()
        && let Some(media) = embed.main_media()
    {
        let resp = if ctx.images() {
            let gif = embed.kind == EmbedKind::Gifv;
            // Same fit as attachments: min(400, column - 16), 350 high.
            let fit = crate::widgets::media_width(max_width, IMAGE_MAX.0);
            media_image(ui, embed, media, IMAGE_MAX, fit, gif, &mut out)
        } else {
            let link = embed.media_link(media);
            link_text(ui, RichText::new(&link), Some(&link), &mut out)
        };
        out.response = Some(resp);
        return out;
    }

    let pal = ctx.pal;
    let bar = embed.color.map(crate::members::rgb).unwrap_or(pal.servers);
    let frame = egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(4)
        .inner_margin(egui::Margin {
            left: (BAR_WIDTH + 12.0) as i8,
            right: 12,
            top: 8,
            bottom: 10,
        })
        .show(ui, |ui| {
            let inner = (max_width - BAR_WIDTH - 24.0).max(60.0);
            ui.set_max_width(inner);
            card_contents(ui, embed, ctx, inner, &mut out);
        });
    let rect = frame.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, vec2(BAR_WIDTH, rect.height())),
        CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        bar,
    );
    out.response = Some(frame.response);
    out
}

fn card_contents(
    ui: &mut egui::Ui,
    embed: &Embed,
    ctx: &EmbedCtx,
    inner: f32,
    out: &mut EmbedOutput,
) {
    let small_thumb = ctx.images()
        && !embed.big_thumbnail()
        && embed.thumbnail.is_some()
        && inner > THUMB_MAX.0 * 2.5;
    let text_w = if small_thumb {
        inner - THUMB_MAX.0 - 16.0
    } else {
        inner
    };

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(text_w);
            ui.spacing_mut().item_spacing.y = 4.0;
            text_parts(ui, embed, ctx, small_thumb, out);
        });
        if small_thumb && let Some(t) = &embed.thumbnail {
            ui.add_space(8.0);
            media_image(ui, embed, t, THUMB_MAX, THUMB_MAX.0, false, out);
        }
    });

    if ctx.images() {
        let big = if embed.big_thumbnail() {
            embed.thumbnail.as_ref().map(|t| (t, VIDEO_MAX))
        } else {
            embed.image.as_ref().map(|i| (i, IMAGE_MAX))
        };
        if let Some((media, max)) = big {
            ui.add_space(6.0);
            let resp = media_image(ui, embed, media, max, inner, false, out);
            if embed.kind == EmbedKind::Video {
                play_badge(ui, resp.rect);
            }
        }
    }

    footer(ui, embed, ctx);
}

fn text_parts(
    ui: &mut egui::Ui,
    embed: &Embed,
    ctx: &EmbedCtx,
    small_thumb: bool,
    out: &mut EmbedOutput,
) {
    let pal = ctx.pal;
    if let Some(p) = &embed.provider {
        link_text(
            ui,
            RichText::new(&p.name).small().color(pal.muted),
            p.url.as_deref(),
            out,
        );
    }
    if let Some(a) = &embed.author {
        ui.horizontal(|ui| {
            if ctx.images()
                && let Some(icon) = &a.icon_url
            {
                ui.add(
                    egui::Image::from_uri(icon)
                        .fit_to_exact_size(vec2(AUTHOR_ICON, AUTHOR_ICON))
                        .corner_radius(AUTHOR_ICON / 2.0),
                );
            }
            link_text(
                ui,
                RichText::new(&a.name).small().strong().color(pal.text),
                a.url.as_deref(),
                out,
            );
        });
    }
    if !embed.title.is_empty() {
        let mut title = RichText::new(&embed.title).strong();
        title = if embed.url.is_some() {
            title.color(ui.visuals().hyperlink_color)
        } else {
            title.color(pal.text)
        };
        link_text(ui, title, embed.url.as_deref(), out);
    }
    if !embed.description.is_empty() {
        body_text(ui, &embed.description, ctx, out);
    }
    if !embed.fields.is_empty() {
        ui.add_space(2.0);
        let per_row = if small_thumb { 2 } else { 3 };
        for row in field_rows(&embed.fields, per_row) {
            let fields = &embed.fields[row];
            if fields.len() == 1 {
                field(ui, &fields[0], ctx, out);
            } else {
                ui.columns(fields.len(), |cols| {
                    for (col, f) in cols.iter_mut().zip(fields) {
                        field(col, f, ctx, out);
                    }
                });
            }
        }
    }
}

fn field(ui: &mut egui::Ui, f: &EmbedField, ctx: &EmbedCtx, out: &mut EmbedOutput) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        if !f.name.is_empty() {
            ui.add(
                egui::Label::new(RichText::new(&f.name).small().strong().color(ctx.pal.text))
                    .wrap(),
            );
        }
        if !f.value.is_empty() {
            body_text(ui, &f.value, ctx, out);
        }
    });
}

fn body_text(ui: &mut egui::Ui, text: &str, ctx: &EmbedCtx, out: &mut EmbedOutput) {
    match ctx.text {
        Some(render) => {
            if let Some(link) = render(ui, text) {
                out.clicked_link = Some(link);
            }
        }
        None => {
            ui.add(egui::Label::new(RichText::new(text).color(ctx.pal.text)).wrap());
        }
    }
}

fn footer(ui: &mut egui::Ui, embed: &Embed, ctx: &EmbedCtx) {
    let time = embed.timestamp.as_deref().map(crate::timefmt::full_iso);
    let text = match (&embed.footer, time) {
        (Some(f), Some(t)) => format!("{} • {t}", f.text),
        (Some(f), None) => f.text.clone(),
        (None, Some(t)) => t,
        (None, None) => return,
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ctx.images()
            && let Some(icon) = embed.footer.as_ref().and_then(|f| f.icon_url.as_ref())
        {
            ui.add(
                egui::Image::from_uri(icon)
                    .fit_to_exact_size(vec2(FOOTER_ICON, FOOTER_ICON))
                    .corner_radius(FOOTER_ICON / 2.0),
            );
        }
        ui.add(egui::Label::new(RichText::new(text).small().color(ctx.pal.muted)).wrap());
    });
}

/// A label that, with a URL, is clickable and reports the click.
fn link_text(
    ui: &mut egui::Ui,
    text: RichText,
    url: Option<&str>,
    out: &mut EmbedOutput,
) -> egui::Response {
    match url {
        Some(url) => {
            let resp = ui
                .add(egui::Label::new(text).wrap().sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(url);
            if resp.clicked() {
                out.clicked_link = Some(url.to_string());
            }
            resp
        }
        None => ui.add(egui::Label::new(text).wrap()),
    }
}

/// A clickable embed picture, fitted into `max` and the available width.
fn media_image(
    ui: &mut egui::Ui,
    embed: &Embed,
    media: &EmbedMedia,
    max: (f32, f32),
    avail_w: f32,
    gif_badge: bool,
    out: &mut EmbedOutput,
) -> egui::Response {
    // The URL depends only on `max`, so `Embed::image_uris` can list it.
    let url = media.sized_url(max.0, max.1);
    let fit_w = max.0.min(avail_w).min(ui.available_width()).max(16.0);
    let mut image = egui::Image::from_uri(url)
        .corner_radius(4)
        .sense(egui::Sense::click());
    image = match media.display_size(fit_w, max.1) {
        Some((w, h)) => image.fit_to_exact_size(vec2(w, h)),
        None => image.max_size(vec2(fit_w, max.1)),
    };
    let link = embed.media_link(media);
    let resp = ui
        .add(image)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(&link);
    if gif_badge {
        let r = egui::Rect::from_min_size(resp.rect.left_top() + vec2(6.0, 6.0), vec2(30.0, 18.0));
        ui.painter()
            .rect_filled(r, 3, Color32::from_black_alpha(160));
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            "GIF",
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    if resp.clicked() {
        out.clicked_link = Some(link);
    }
    resp
}

/// The round ▶ over video previews.
fn play_badge(ui: &egui::Ui, rect: egui::Rect) {
    let c = rect.center();
    let p = ui.painter();
    p.circle_filled(c, 22.0, Color32::from_black_alpha(170));
    let tri = vec![
        c + vec2(-7.0, -10.0),
        c + vec2(-7.0, 10.0),
        c + vec2(11.0, 0.0),
    ];
    p.add(egui::Shape::convex_polygon(
        tri,
        Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn youtube() -> Value {
        json!({
            "type": "video",
            "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "title": "Rick Astley - Never Gonna Give You Up (Official Music Video)",
            "description": "The official video for “Never Gonna Give You Up” by Rick Astley.",
            "color": 16711680,
            "author": {
                "name": "Rick Astley",
                "url": "https://www.youtube.com/channel/UCuAXFkgsw1L7xaCfnd5JJOw"
            },
            "provider": { "name": "YouTube", "url": "https://www.youtube.com" },
            "thumbnail": {
                "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg",
                "proxy_url": "https://images-ext-1.discordapp.net/external/AbC/https/i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg",
                "width": 1280,
                "height": 720,
                "placeholder": "3PcNDwJ4h3iHeHiAeIiHd4h4dwAAAAAA",
                "placeholder_version": 1,
                "flags": 0
            },
            "video": {
                "url": "https://www.youtube.com/embed/dQw4w9WgXcQ",
                "width": 1280,
                "height": 720
            },
            "content_scan_version": 2
        })
    }

    fn rich_bot() -> Value {
        json!({
            "type": "rich",
            "title": "Server status",
            "url": "https://status.example.org",
            "description": "All systems **operational**.\nLast restart 3 days ago.",
            "color": 5793266,
            "timestamp": "2026-11-09T14:05:33.123000+00:00",
            "author": {
                "name": "KeoCraft Bot",
                "icon_url": "https://cdn.discordapp.com/avatars/1/abc.png",
                "proxy_icon_url": "https://images-ext-1.discordapp.net/external/X/https/cdn.discordapp.com/avatars/1/abc.png"
            },
            "thumbnail": {
                "url": "https://example.org/logo.png",
                "proxy_url": "https://images-ext-2.discordapp.net/external/Y/https/example.org/logo.png",
                "width": 256,
                "height": 256
            },
            "fields": [
                { "name": "Players", "value": "12/40", "inline": true },
                { "name": "TPS", "value": "19.8", "inline": true },
                { "name": "Version", "value": "1.21.3", "inline": true },
                { "name": "MOTD", "value": "Welcome back!", "inline": false },
                { "name": "Uptime", "value": "3d 4h", "inline": true },
                { "value": "no name", "inline": true },
                "junk",
                { "name": "", "value": "" }
            ],
            "footer": {
                "text": "KeoCraft",
                "icon_url": "https://example.org/f.png",
                "proxy_icon_url": "https://images-ext-1.discordapp.net/external/Z/https/example.org/f.png"
            },
            "content_scan_version": 2
        })
    }

    fn image() -> Value {
        json!({
            "type": "image",
            "url": "https://i.imgur.com/abcd123.png",
            "thumbnail": {
                "url": "https://i.imgur.com/abcd123.png",
                "proxy_url": "https://images-ext-2.discordapp.net/external/Q/https/i.imgur.com/abcd123.png",
                "width": 1600,
                "height": 900
            },
            "content_scan_version": 2
        })
    }

    fn gifv() -> Value {
        json!({
            "type": "gifv",
            "url": "https://tenor.com/view/cat-typing-gif-12345",
            "provider": { "name": "Tenor", "url": "https://tenor.co" },
            "thumbnail": {
                "url": "https://media.tenor.com/abc/AAAAe/cat-typing.png",
                "proxy_url": "https://images-ext-1.discordapp.net/external/T/https/media.tenor.com/abc/AAAAe/cat-typing.png",
                "width": 498,
                "height": 373
            },
            "video": {
                "url": "https://media.tenor.com/abc/AAAPo/cat-typing.mp4",
                "proxy_url": "https://images-ext-1.discordapp.net/external/V/https/media.tenor.com/abc/AAAPo/cat-typing.mp4",
                "width": 498,
                "height": 373,
                "placeholder": "x",
                "placeholder_version": 1
            },
            "content_scan_version": 2
        })
    }

    #[test]
    fn youtube_video() {
        let e = Embed::from_json(&youtube());
        assert_eq!(e.kind, EmbedKind::Video);
        assert_eq!(e.color, Some(0xFF0000));
        assert_eq!(e.provider.as_ref().unwrap().name, "YouTube");
        assert_eq!(e.author.as_ref().unwrap().name, "Rick Astley");
        assert!(e.author.as_ref().unwrap().icon_url.is_none());
        assert!(e.title.starts_with("Rick Astley"));
        let t = e.thumbnail.as_ref().unwrap();
        assert_eq!((t.width, t.height), (Some(1280), Some(720)));
        assert!(
            t.source()
                .starts_with("https://images-ext-1.discordapp.net/")
        );
        let v = e.video.as_ref().unwrap();
        assert_eq!(v.link(), "https://www.youtube.com/embed/dQw4w9WgXcQ");
        assert!(!e.is_bare_media());
        assert!(e.big_thumbnail());
        // Clicking the preview opens the watch page, not the iframe URL.
        assert_eq!(
            e.media_link(t),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        // Loaded resized through the proxy.
        let uris = e.image_uris();
        assert_eq!(uris.len(), 1);
        assert!(uris[0].ends_with("maxresdefault.jpg?width=400&height=225"));
    }

    #[test]
    fn rich_bot_embed() {
        let e = Embed::from_json(&rich_bot());
        assert_eq!(e.kind, EmbedKind::Rich);
        assert_eq!(e.color, Some(0x5865F2));
        assert_eq!(e.url.as_deref(), Some("https://status.example.org"));
        let a = e.author.as_ref().unwrap();
        // Proxied icon preferred.
        assert!(a.icon_url.as_deref().unwrap().contains("images-ext-1"));
        let f = e.footer.as_ref().unwrap();
        assert_eq!(f.text, "KeoCraft");
        assert!(f.icon_url.as_deref().unwrap().contains("images-ext-1"));
        // Junk and empty fields dropped, nameless value kept.
        assert_eq!(e.fields.len(), 6);
        assert_eq!(e.fields[5].name, "");
        assert_eq!(e.fields[5].value, "no name");
        assert!(e.fields[0].inline && !e.fields[3].inline);
        assert_ne!(
            crate::timefmt::parse_iso(e.timestamp.as_deref().unwrap()),
            0
        );
        assert_eq!(field_rows(&e.fields, 3), vec![0..3, 3..4, 4..6]);
        assert_eq!(field_rows(&e.fields, 2), vec![0..2, 2..3, 3..4, 4..6]);
        // Small 256px thumbnail shown at 80px.
        let uris = e.image_uris();
        assert!(
            uris.iter()
                .any(|u| u.ends_with("logo.png?width=80&height=80"))
        );
        assert_eq!(uris.len(), 3);
        assert_eq!(e.summary(), "Server status");
    }

    #[test]
    fn image_embed() {
        let e = Embed::from_json(&image());
        assert_eq!(e.kind, EmbedKind::Image);
        assert!(e.is_bare_media());
        let m = e.main_media().unwrap();
        assert_eq!(m.display_size(400.0, 300.0), Some((400.0, 225.0)));
        assert_eq!(e.media_link(m), "https://i.imgur.com/abcd123.png");
        assert_eq!(
            e.image_uris(),
            vec![
                "https://images-ext-2.discordapp.net/external/Q/https/i.imgur.com/abcd123.png\
                 ?width=400&height=225"
                    .to_string()
            ]
        );
        assert_eq!(e.summary(), "Image");
    }

    #[test]
    fn gifv_embed() {
        let e = Embed::from_json(&gifv());
        assert_eq!(e.kind, EmbedKind::Gifv);
        assert!(e.is_bare_media());
        let m = e.main_media().unwrap();
        // Static preview loaded; click goes to the Tenor page.
        assert!(m.source().ends_with("cat-typing.png"));
        assert_eq!(
            e.media_link(m),
            "https://tenor.com/view/cat-typing-gif-12345"
        );
        assert_eq!(e.video.as_ref().unwrap().width, Some(498));
        // 498x373 shrinks to fit 400x300.
        assert_eq!(
            e.image_uris(),
            vec![format!("{}?width=400&height=300", m.proxy_url)]
        );
        // Already small enough: URL unchanged.
        assert_eq!(m.sized_url(600.0, 600.0), m.proxy_url);
        assert_eq!(e.summary(), "GIF");
    }

    #[test]
    fn message_level_parsing() {
        let msg = json!({
            "id": "1",
            "content": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "embeds": [youtube(), {}, "junk", null, image(), { "type": "rich" }]
        });
        let embeds = parse_embeds(&msg);
        assert_eq!(embeds.len(), 2);
        assert_eq!(embeds[1].kind, EmbedKind::Image);

        assert!(parse_embeds(&json!({ "id": "1" })).is_empty());
        assert!(parse_embeds(&json!({ "embeds": "nope" })).is_empty());
        assert_eq!(embeds_patch(&json!({ "id": "1" })), None);
        assert_eq!(embeds_patch(&json!({ "embeds": [] })), Some(vec![]));

        let many = json!({ "embeds": vec![rich_bot(); 15] });
        assert_eq!(parse_embeds(&many).len(), MAX_EMBEDS);
    }

    #[test]
    fn lenient_values() {
        let e = Embed::from_json(&json!({
            "type": "auto_moderation_message",
            "title": 5,
            "description": "flagged",
            "color": "#00ff00",
            "author": "junk",
            "footer": { "icon_url": "https://x.org/a.png" },
            "image": { "url": "https://x.org/i.png", "width": -3, "height": 1.5e3 },
            "thumbnail": { "url": "" },
            "provider": { "name": "" },
            "url": "   "
        }));
        assert_eq!(e.kind, EmbedKind::Other("auto_moderation_message".into()));
        assert_eq!(e.title, "");
        assert_eq!(e.color, Some(0x00FF00));
        assert!(e.author.is_none() && e.footer.is_none() && e.provider.is_none());
        assert!(e.thumbnail.is_none());
        assert!(e.url.is_none());
        let i = e.image.unwrap();
        assert_eq!((i.width, i.height), (None, Some(1500)));
        // No size: loaded as-is.
        assert_eq!(i.sized_url(400.0, 300.0), "https://x.org/i.png");

        assert_eq!(color_of(&json!({ "color": 0x1000000 })), None);
        assert_eq!(color_of(&json!({ "color": "123" })), Some(123));
        assert_eq!(color_of(&json!({ "color": null })), None);
    }

    #[test]
    fn non_proxy_images_not_rewritten() {
        let m = EmbedMedia {
            url: "https://example.org/big.png".into(),
            proxy_url: String::new(),
            width: Some(4000),
            height: Some(3000),
        };
        assert_eq!(m.sized_url(400.0, 300.0), "https://example.org/big.png");
        let p = EmbedMedia {
            proxy_url: "https://media.discordapp.net/attachments/1/2/a.png?ex=1".into(),
            ..m
        };
        assert_eq!(
            p.sized_url(400.0, 300.0),
            "https://media.discordapp.net/attachments/1/2/a.png?ex=1&width=400&height=300"
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(crate::timefmt::full_iso("garbage"), "garbage");
        assert!(!crate::timefmt::full_iso("2024-03-01T12:34:56Z").is_empty());
    }

    #[test]
    fn field_rows_edge_cases() {
        let f = |inline| EmbedField {
            name: "n".into(),
            value: "v".into(),
            inline,
        };
        assert!(field_rows(&[], 3).is_empty());
        assert_eq!(field_rows(&vec![f(true); 7], 3), vec![0..3, 3..6, 6..7]);
        assert_eq!(field_rows(&[f(false), f(false)], 3), vec![0..1, 1..2]);
        assert_eq!(field_rows(&[f(true), f(true)], 0), vec![0..1, 1..2]);
    }
}
