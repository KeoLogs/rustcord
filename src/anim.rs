//! Animated GIFs.
//!
//! egui animates an image when its URI ends in `.gif` (egui_extras decodes
//! every frame and uploads each one as its own texture, `<uri>#<frame>`).
//! Discord's URLs carry a query (`…/x.gif?size=64`, `…?ex=…&width=…`), so
//! egui sees no `.gif` and draws the first frame only. [`animated_uri`]
//! adds a harmless `rc_anim=.gif` query parameter so egui animates it.
//!
//! - GIFs in messages and embeds autoplay (`app::attachment_ui`,
//!   `embed::media_image`).
//! - Avatars and custom emoji stay still (their plain URL) until hovered
//!   ([`on_hover`]).
//! - Only GIFs on screen keep their decoded frames: every animated URI is
//!   [`mark`]ed when drawn, and [`sweep`] (end of every pass) frees the
//!   ones not drawn for [`KEEP`] with `ctx.forget_image` (bytes, decoded
//!   frames and every frame texture).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

/// Query parameter that makes egui treat a URL as a GIF.
const MARKER: &str = "rc_anim=.gif";

/// How long a GIF that left the screen keeps its frames (so scrolling it
/// back in at once doesn't download and decode it again).
pub const KEEP: Duration = Duration::from_millis(750);

/// Animated URIs drawn recently, with when they were last drawn. Global
/// (not per `egui::Context` viewport) so a GIF shown in two account
/// windows isn't freed by one of them.
static SEEN: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// Does the URL's path end in `.gif` (query and fragment ignored)?
pub fn is_gif_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.len() > 4 && path[path.len() - 4..].eq_ignore_ascii_case(".gif")
}

/// The URI egui animates for a GIF URL, or `None` if it isn't a GIF.
pub fn animated_uri(url: &str) -> Option<String> {
    if !is_gif_url(url) || url.contains('#') {
        return None;
    }
    if url.ends_with(MARKER) || !url.contains('?') {
        // `.gif` is already the extension egui looks at.
        return Some(url.to_string());
    }
    Some(format!("{url}&{MARKER}"))
}

/// The plain URL again (for opening / copying an animated URI).
#[cfg(test)]
pub fn plain_url(uri: &str) -> &str {
    uri.strip_suffix(MARKER)
        .map(|u| u.strip_suffix('&').unwrap_or(u))
        .unwrap_or(uri)
}

/// Remember that `uri` (an [`animated_uri`]) was drawn this pass.
pub fn mark(uri: &str) {
    if let Ok(mut seen) = SEEN.lock() {
        match seen.get_mut(uri) {
            Some(t) => *t = Instant::now(),
            None => {
                seen.insert(uri.to_string(), Instant::now());
            }
        }
    }
}

/// An animated GIF to draw now: its [`animated_uri`] (marked as on screen),
/// or `url` unchanged if it isn't a GIF.
pub fn autoplay(url: &str) -> Cow<'_, str> {
    match animated_uri(url) {
        Some(uri) => {
            mark(&uri);
            Cow::Owned(uri)
        }
        None => Cow::Borrowed(url),
    }
}

/// Avatars and emoji: still (the plain `url`) unless the pointer is over
/// `rect`, then animated.
pub fn on_hover<'a>(ui: &egui::Ui, rect: egui::Rect, url: &'a str) -> Cow<'a, str> {
    if is_gif_url(url) && ui.rect_contains_pointer(rect) {
        autoplay(url)
    } else {
        Cow::Borrowed(url)
    }
}

/// A GIF in a message or embed at its exact `size`: autoplays while any
/// of it is on screen; off screen it only takes its space (nothing loaded,
/// so the sweep frees it). With a `still` (embed previews), that shows
/// while the GIF loads or if it can't load. `None` if `url` isn't a GIF: draw it as usual.
pub fn gif_image(
    ui: &mut egui::Ui,
    url: &str,
    still: Option<&str>,
    size: egui::Vec2,
    radius: f32,
    sense: egui::Sense,
) -> Option<egui::Response> {
    let uri = animated_uri(url)?;
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        mark(&uri);
        // The still shows while the GIF loads, and if it fails.
        let ready = matches!(
            egui::Image::from_uri(uri.as_str()).load_for_size(ui.ctx(), size),
            Ok(egui::load::TexturePoll::Ready { .. })
        );
        let shown = match still {
            Some(still) if !ready => still.to_string(),
            _ => uri,
        };
        egui::Image::from_uri(shown)
            .corner_radius(radius)
            .paint_at(ui, rect);
    }
    Some(resp)
}

/// URIs not marked since `now - KEEP`, removed from `seen`.
fn expired(seen: &mut HashMap<String, Instant>, now: Instant) -> Vec<String> {
    let mut out = Vec::new();
    seen.retain(|uri, t| {
        let keep = now.saturating_duration_since(*t) < KEEP;
        if !keep {
            out.push(uri.clone());
        }
        keep
    });
    out
}

/// Call once at the end of every pass: frees the GIFs that left the screen.
pub fn sweep(ctx: &egui::Context) {
    let (gone, any_left) = match SEEN.lock() {
        Ok(mut seen) => {
            let gone = expired(&mut seen, Instant::now());
            (gone, !seen.is_empty())
        }
        Err(_) => return,
    };
    for uri in gone {
        forget(ctx, &uri);
    }
    if any_left {
        // Come back to free them even if nothing else repaints.
        ctx.request_repaint_after(KEEP);
    }
}

/// Free an animated URI: the bytes, the decoded frames and every frame's
/// texture (egui keys those `<uri>#<n>`, which `forget_image(uri)` misses).
pub fn forget(ctx: &egui::Context, uri: &str) {
    // The frame count egui_extras stored when it decoded the GIF. Read it
    // first: no `ctx` calls inside `data` closures.
    let id = egui::Id::unique(uri);
    let durations: Option<egui::FrameDurations> = ctx.data(|d| d.get_temp(id));
    let frames = durations.map_or(1, |d| d.all().len().max(1));
    for i in 0..frames {
        ctx.forget_image(&format!("{uri}#{i}"));
    }
    ctx.forget_image(uri);
    ctx.data_mut(|d| d.remove::<egui::FrameDurations>(id));
    if let Ok(mut seen) = SEEN.lock() {
        seen.remove(uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_urls() {
        assert!(is_gif_url(
            "https://cdn.discordapp.com/avatars/1/a_x.gif?size=64"
        ));
        assert!(is_gif_url("https://media.tenor.com/abcAAAAC/cat.GIF"));
        assert!(!is_gif_url(
            "https://cdn.discordapp.com/avatars/1/x.png?size=64"
        ));
        assert!(!is_gif_url("https://example.com/gif"));
        assert!(!is_gif_url("https://example.com/x.png?name=a.gif"));
    }

    #[test]
    fn animated_uris() {
        // No query: already animated by egui.
        let u = "https://media.tenor.com/abcAAAAC/cat.gif";
        assert_eq!(animated_uri(u).as_deref(), Some(u));
        // A query hides the extension from egui: add the marker.
        let a = "https://cdn.discordapp.com/emojis/1.gif?size=48";
        let anim = animated_uri(a).unwrap();
        assert_eq!(anim, format!("{a}&rc_anim=.gif"));
        assert!(
            std::path::Path::new(&anim)
                .extension()
                .is_some_and(|e| e == "gif")
        );
        assert_eq!(plain_url(&anim), a);
        // Idempotent.
        assert_eq!(animated_uri(&anim).as_deref(), Some(anim.as_str()));
        assert_eq!(animated_uri("https://x.com/a.png"), None);
        // `#` would clash with egui's frame suffix.
        assert_eq!(animated_uri("https://x.com/a.gif#x"), None);
    }

    #[test]
    fn only_recent_ones_stay() {
        let mut seen = HashMap::new();
        let now = Instant::now();
        seen.insert("on".to_string(), now);
        seen.insert("off".to_string(), now - KEEP * 2);
        assert_eq!(expired(&mut seen, now), vec!["off".to_string()]);
        assert!(seen.contains_key("on") && !seen.contains_key("off"));
    }
}
