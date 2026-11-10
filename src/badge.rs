//! Unread badges: the numbers on the server rail / DM icon and the taskbar
//! badge, from one place so they always agree.
//!
//! Also draws the taskbar badge picture (a red circle with a white count)
//! into RGBA in code, so no artwork ships with the app. Pure and tested;
//! the platform part lives in `taskbar.rs`.

use crate::notify::Mutes;
use crate::read_state::ReadState;
use crate::types::Guild;

/// Unread / mention / muted state of a server icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Badge {
    pub unread: bool,
    pub mentions: u32,
    pub muted: bool,
}

/// The badge of one server (or the DM pseudo-server). `now` is an ISO-8601
/// UTC time (`notify::now_iso`) for timed mutes.
pub fn guild_badge(g: &Guild, reads: &ReadState, mutes: &Mutes, now: &str) -> Badge {
    if g.is_dms() {
        // DMs: only a mention count (every unmuted DM message counts).
        let mentions = g
            .channels
            .iter()
            .filter(|c| !mutes.muted(&g.id, &c.id, c.parent_id.as_deref(), now))
            .map(|c| reads.mentions(&c.id))
            .sum();
        return Badge {
            unread: false,
            mentions,
            muted: false,
        };
    }
    if mutes.guild_muted(&g.id, now) {
        return Badge {
            unread: false,
            mentions: 0,
            muted: true,
        };
    }
    let mut badge = Badge::default();
    for c in g.channels.iter().filter(|c| c.is_text()) {
        if mutes.channel_muted(&g.id, &c.id, c.parent_id.as_deref(), now) {
            continue;
        }
        badge.unread |= reads.is_unread(&c.id);
        badge.mentions += reads.mentions(&c.id);
    }
    badge
}

/// The taskbar count: mentions in every server plus unread DMs (a DM
/// message counts as a mention), muted servers and channels left out.
/// Exactly the sum of the rail's red badges.
pub fn total_count(
    dms: &Guild,
    guilds: &[Guild],
    reads: &ReadState,
    mutes: &Mutes,
    now: &str,
) -> u32 {
    std::iter::once(dms)
        .chain(guilds)
        .map(|g| guild_badge(g, reads, mutes, now).mentions)
        .fold(0u32, u32::saturating_add)
}

/// What the taskbar badge shows: `1..=9`, or [`NINE_PLUS`] for "9+".
/// `None` = no badge.
pub fn label(count: u32) -> Option<u8> {
    match count {
        0 => None,
        1..=9 => Some(count as u8),
        _ => Some(NINE_PLUS),
    }
}

/// The "9+" label.
pub const NINE_PLUS: u8 = 10;

/// Text of a label ("1".."9", "9+").
pub fn label_text(label: u8) -> String {
    if label >= NINE_PLUS {
        "9+".into()
    } else {
        label.to_string()
    }
}

// ---------------------------------------------------------------------------
// Badge picture
// ---------------------------------------------------------------------------

/// 5x7 glyphs, one row per byte (bit 4 = left column).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        _ => [0; 7],
    }
}

/// Glyph cell `(x, y)` of `rows` is ink (out of range = no ink).
fn ink(rows: &[u8; 7], x: f32, y: f32) -> bool {
    if !(0.0..5.0).contains(&x) || !(0.0..7.0).contains(&y) {
        return false;
    }
    rows[y as usize] & (0x10 >> x as usize) != 0
}

/// Supersamples per pixel side.
const SS: u32 = 4;
/// Strokes are thickened by this much (in glyph units) on each side so the
/// count stays readable at 16 px.
const BOLD: f32 = 0.18;

/// Draw the badge for `label` as `size` x `size` RGBA (straight alpha, rows
/// top to bottom): a `fill` circle with the count in white, anti-aliased.
pub fn render(label: u8, size: u32, fill: [u8; 3]) -> Vec<u8> {
    let text = label_text(label);
    let glyphs: Vec<[u8; 7]> = text.chars().map(glyph).collect();
    let n = glyphs.len() as f32;
    let s = size as f32;
    // Glyph unit in pixels: 5 wide + 1 gap between glyphs, 7 tall.
    let text_w = n * 5.0 + (n - 1.0);
    let unit = if glyphs.len() == 1 {
        0.62 * s / 7.0
    } else {
        (0.82 * s / text_w).min(0.5 * s / 7.0)
    };
    let left = (s - text_w * unit) / 2.0;
    let top = (s - 7.0 * unit) / 2.0;
    let r = s / 2.0;
    let is_ink = |px: f32, py: f32| {
        let gx = (px - left) / unit;
        let gy = (py - top) / unit;
        if gy < -BOLD || gy >= 7.0 + BOLD {
            return false;
        }
        // Thickened strokes can reach just past a glyph's own cell.
        [gx - BOLD, gx + BOLD]
            .iter()
            .map(|v| (v / 6.0).floor())
            .filter(|j| *j >= 0.0 && *j < n)
            .any(|j| is_glyph_ink(&glyphs, j, gx, gy))
    };
    let mut out = vec![0u8; (size * size * 4) as usize];
    let step = 1.0 / SS as f32;
    for y in 0..size {
        for x in 0..size {
            let (mut inside, mut white) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) * step;
                    let py = y as f32 + (sy as f32 + 0.5) * step;
                    let (dx, dy) = (px - r, py - r);
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    inside += 1;
                    if is_ink(px, py) {
                        white += 1;
                    }
                }
            }
            if inside == 0 {
                continue;
            }
            let t = white as f32 / inside as f32;
            let mix = |c: u8| (c as f32 + (255.0 - c as f32) * t).round() as u8;
            let i = ((y * size + x) * 4) as usize;
            out[i] = mix(fill[0]);
            out[i + 1] = mix(fill[1]);
            out[i + 2] = mix(fill[2]);
            out[i + 3] = (inside * 255 / (SS * SS)) as u8;
        }
    }
    out
}

/// Ink of glyph `i` (as f32 index) at glyph-space `(gx, gy)`, strokes
/// thickened by [`BOLD`].
fn is_glyph_ink(glyphs: &[[u8; 7]], i: f32, gx: f32, gy: f32) -> bool {
    let rows = &glyphs[i as usize];
    let lx = gx - i.floor() * 6.0;
    [(-BOLD, -BOLD), (BOLD, -BOLD), (-BOLD, BOLD), (BOLD, BOLD)]
        .iter()
        .any(|(ox, oy)| ink(rows, lx + ox, gy + oy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::GuildSettings;
    use crate::types::{Channel, DM_GUILD_ID};
    use serde_json::json;

    fn guild(id: &str, channels: &[(&str, Option<&str>)]) -> Guild {
        let chans: Vec<_> = channels
            .iter()
            .map(|(cid, parent)| json!({ "id": cid, "name": cid, "type": 0, "parent_id": parent }))
            .collect();
        Guild::from_json(&json!({ "id": id, "name": id, "channels": chans }))
    }

    fn dms(ids: &[&str]) -> Guild {
        Guild::dms(
            ids.iter()
                .map(|id| Channel::from_json(&json!({ "id": id, "type": 1 })))
                .collect(),
        )
    }

    const NOW: &str = "2026-11-10T12:00:00";

    /// Two servers (one muted, one with a muted category) and two DMs (one
    /// muted), all with mentions.
    fn world() -> (Guild, Vec<Guild>, ReadState, Mutes) {
        let d = dms(&["dm1", "dm2"]);
        let a = guild(
            "a",
            &[
                ("a1", None),
                ("a2", Some("cat")),
                ("a3", None),
                ("cat", None),
            ],
        );
        let b = guild("b", &[("b1", None)]);
        let mut reads = ReadState::default();
        for (ch, n) in [("dm1", 2), ("dm2", 5), ("a1", 3), ("a2", 4), ("b1", 7)] {
            for i in 0..n {
                reads.on_message(ch, 1000 + i, true, false, false);
            }
        }
        // An unread channel without mentions only lights the pill.
        reads.track("a3", 1);
        reads.set_last("a3", 5);
        let mut mutes = Mutes::default();
        mutes.replace_all(vec![
            GuildSettings::from_json(&json!({ "guild_id": "b", "muted": true })),
            GuildSettings::from_json(&json!({
                "guild_id": "a",
                "channel_overrides": [{ "channel_id": "cat", "muted": true }]
            })),
            GuildSettings::from_json(&json!({
                "guild_id": null,
                "channel_overrides": [{ "channel_id": "dm2", "muted": true }]
            })),
        ]);
        (d, vec![a, b], reads, mutes)
    }

    #[test]
    fn rail_badges() {
        let (d, g, reads, mutes) = world();
        assert_eq!(d.id, DM_GUILD_ID);
        assert_eq!(guild_badge(&d, &reads, &mutes, NOW).mentions, 2);
        let a = guild_badge(&g[0], &reads, &mutes, NOW);
        // a2 sits in the muted category.
        assert_eq!(a.mentions, 3);
        assert!(a.unread && !a.muted);
        let b = guild_badge(&g[1], &reads, &mutes, NOW);
        assert_eq!(
            b,
            Badge {
                unread: false,
                mentions: 0,
                muted: true
            }
        );
    }

    #[test]
    fn taskbar_count_is_the_rail_sum() {
        let (d, g, reads, mutes) = world();
        let rail: u32 = std::iter::once(&d)
            .chain(&g)
            .map(|g| guild_badge(g, &reads, &mutes, NOW).mentions)
            .sum();
        assert_eq!(total_count(&d, &g, &reads, &mutes, NOW), rail);
        assert_eq!(rail, 5);
        // Nothing muted: everything counts.
        let none = Mutes::default();
        assert_eq!(total_count(&d, &g, &reads, &none, NOW), 2 + 5 + 3 + 4 + 7);
        // Nothing at all.
        assert_eq!(
            total_count(&dms(&[]), &[], &ReadState::default(), &none, NOW),
            0
        );
    }

    #[test]
    fn reading_clears_the_count() {
        let (d, g, mut reads, mutes) = world();
        reads.mark_read("dm1");
        reads.mark_read("a1");
        assert_eq!(total_count(&d, &g, &reads, &mutes, NOW), 0);
    }

    #[test]
    fn labels() {
        assert_eq!(label(0), None);
        assert_eq!(label(1), Some(1));
        assert_eq!(label(9), Some(9));
        assert_eq!(label(10), Some(NINE_PLUS));
        assert_eq!(label(u32::MAX), Some(NINE_PLUS));
        assert_eq!(label_text(3), "3");
        assert_eq!(label_text(NINE_PLUS), "9+");
    }

    #[test]
    fn picture() {
        let red = [0xda, 0x37, 0x3c];
        for size in [16, 20, 24, 32] {
            for l in 1..=NINE_PLUS {
                let px = render(l, size, red);
                assert_eq!(px.len(), (size * size * 4) as usize);
                let at = |x: u32, y: u32| {
                    let i = ((y * size + x) * 4) as usize;
                    [px[i], px[i + 1], px[i + 2], px[i + 3]]
                };
                // Corners are outside the circle.
                assert_eq!(at(0, 0)[3], 0);
                assert_eq!(at(size - 1, size - 1)[3], 0);
                // Some solid red and some white text.
                assert!(px.chunks(4).any(|p| p == [red[0], red[1], red[2], 255]));
                assert!(
                    px.chunks(4).any(|p| p == [255, 255, 255, 255]),
                    "{l} @ {size}"
                );
            }
        }
        // Different counts, different pictures.
        assert_ne!(render(1, 16, red), render(7, 16, red));
        assert_ne!(render(9, 16, red), render(NINE_PLUS, 16, red));
    }
}
