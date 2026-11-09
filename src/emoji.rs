//! Splitting message text into plain text and emoji.
//!
//! egui can't draw colour emoji from fonts, so — like Discord itself — we
//! show emoji as images: Twemoji for standard Unicode emoji, and Discord's CDN
//! for custom server emoji written as `<:name:id>` / `<a:name:id>`.

use unicode_segmentation::UnicodeSegmentation;

const TWEMOJI_BASE: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@latest/assets/72x72";

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    Emoji { url: String, alt: String },
}

#[cfg(test)]
/// Split `text` into text and emoji segments, in order.
pub fn segments(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut rest = text;
    // Custom emoji first: <:name:id> or <a:name:id>
    while let Some(start) = rest.find('<') {
        if let Some((len, emoji)) = parse_custom(&rest[start..]) {
            push_unicode(&mut out, &rest[..start]);
            out.push(emoji);
            rest = &rest[start + len..];
        } else {
            push_unicode(&mut out, &rest[..start + 1]);
            rest = &rest[start + 1..];
        }
    }
    push_unicode(&mut out, rest);
    out
}

/// Split `text` into text and Unicode emoji only. Custom emoji syntax
/// (`<:name:id>`) stays as text; the markdown parser handles that itself.
pub fn unicode_segments(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    push_unicode(&mut out, text);
    out
}

#[cfg(test)]
/// True if the message is only emoji (and whitespace) — Discord shows those big.
pub fn is_jumbo(segments: &[Segment]) -> bool {
    let mut count = 0;
    for s in segments {
        match s {
            Segment::Emoji { .. } => count += 1,
            Segment::Text(t) if t.trim().is_empty() => {}
            Segment::Text(_) => return false,
        }
    }
    (1..=27).contains(&count)
}

#[cfg(test)]
/// Parse `<:name:id>` / `<a:name:id>` at the start of `s`.
/// Returns the length consumed and the emoji segment.
fn parse_custom(s: &str) -> Option<(usize, Segment)> {
    let end = s.find('>')?;
    let inner = &s[1..end];
    let (animated, inner) = match inner.strip_prefix("a:") {
        Some(rest) => (true, rest),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, id) = inner.split_once(':')?;
    if name.is_empty() || id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let ext = if animated { "gif" } else { "png" };
    Some((
        end + 1,
        Segment::Emoji {
            url: format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48"),
            alt: format!(":{name}:"),
        },
    ))
}

/// Split plain text into text runs and Unicode emoji.
fn push_unicode(out: &mut Vec<Segment>, text: &str) {
    let mut buf = String::new();
    for g in text.graphemes(true) {
        if is_emoji(g) {
            if !buf.is_empty() {
                push_text(out, std::mem::take(&mut buf));
            }
            out.push(Segment::Emoji {
                url: format!("{TWEMOJI_BASE}/{}.png", twemoji_code(g)),
                alt: g.to_string(),
            });
        } else {
            buf.push_str(g);
        }
    }
    if !buf.is_empty() {
        push_text(out, buf);
    }
}

fn push_text(out: &mut Vec<Segment>, text: String) {
    if let Some(Segment::Text(prev)) = out.last_mut() {
        prev.push_str(&text);
    } else {
        out.push(Segment::Text(text));
    }
}

fn is_emoji(g: &str) -> bool {
    // Plain digits, '#' and '*' are in the emoji list (as keycap bases) but
    // should stay text unless they're an actual keycap sequence.
    if g.chars().count() == 1 && g.is_ascii() {
        return false;
    }
    emojis::get(g).is_some()
}

/// Twemoji file name: code points in hex joined with '-'. The FE0F variation
/// selector is dropped unless the sequence contains a ZWJ (U+200D).
fn twemoji_code(g: &str) -> String {
    let has_zwj = g.contains('\u{200D}');
    g.chars()
        .filter(|&c| has_zwj || c != '\u{FE0F}')
        .map(|c| format!("{:x}", c as u32))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emoji_alts(s: &str) -> Vec<String> {
        segments(s)
            .into_iter()
            .filter_map(|seg| match seg {
                Segment::Emoji { alt, .. } => Some(alt),
                Segment::Text(_) => None,
            })
            .collect()
    }

    #[test]
    fn plain_text_stays_text() {
        assert_eq!(
            segments("hello 123 #general"),
            vec![Segment::Text("hello 123 #general".into())]
        );
    }

    #[test]
    fn finds_unicode_emoji() {
        assert_eq!(emoji_alts("hi 😀 there 👍🏽!"), vec!["😀", "👍🏽"]);
    }

    #[test]
    fn twemoji_codes() {
        assert_eq!(twemoji_code("😀"), "1f600");
        assert_eq!(twemoji_code("❤️"), "2764"); // FE0F dropped
        assert_eq!(twemoji_code("👍🏽"), "1f44d-1f3fd");
        // ZWJ sequence keeps FE0F
        assert_eq!(twemoji_code("🏳️‍🌈"), "1f3f3-fe0f-200d-1f308");
    }

    #[test]
    fn custom_emoji() {
        let segs = segments("gg <:pepe:123456> <a:dance:789>");
        assert!(segs.contains(&Segment::Emoji {
            url: "https://cdn.discordapp.com/emojis/123456.png?size=48".into(),
            alt: ":pepe:".into()
        }));
        assert!(segs.contains(&Segment::Emoji {
            url: "https://cdn.discordapp.com/emojis/789.gif?size=48".into(),
            alt: ":dance:".into()
        }));
    }

    #[test]
    fn not_custom_emoji() {
        assert_eq!(
            segments("a < b and <@123> mention"),
            vec![Segment::Text("a < b and <@123> mention".into())]
        );
    }

    #[test]
    fn jumbo() {
        assert!(is_jumbo(&segments("😀 🎉")));
        assert!(!is_jumbo(&segments("😀 hi")));
        assert!(!is_jumbo(&segments("hi")));
    }
}
