//! Composer autocomplete (Discord Messenger: `windows/AutoComplete.cpp`):
//! `@` → people (and roles, @everyone / @here in servers), `#` → channels,
//! `:` + 2 characters → emoji (Unicode via the `emojis` crate, plus the
//! server's custom emoji). ↑↓ move, Tab / Enter insert, Esc dismisses.
//!
//! Pure logic here (finding the word being typed, matching, inserting);
//! `app.rs` gathers the candidates and draws the popup.

use eframe::egui::{self, Color32, RichText, vec2};

use crate::theme::Palette;
use crate::types::GuildEmoji;

/// Most suggestions shown.
pub const MAX_ITEMS: usize = 8;
/// Characters needed after `:` before emoji are suggested.
const EMOJI_MIN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    User,
    Channel,
    Emoji,
}

/// The word being completed: `draft[start..end]` (byte offsets), where
/// `start` is the trigger character and `end` the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub trigger: Trigger,
    pub start: usize,
    pub end: usize,
    /// What's typed after the trigger.
    pub text: String,
}

/// Find the word under the cursor (`cursor` in characters, as egui reports
/// it) if it starts with a trigger.
pub fn find_query(draft: &str, cursor: usize) -> Option<Query> {
    let end = draft
        .char_indices()
        .nth(cursor)
        .map_or(draft.len(), |(i, _)| i);
    let before = &draft[..end];
    // The word starts after the last whitespace.
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let mut chars = word.chars();
    let trigger = match chars.next()? {
        '@' => Trigger::User,
        '#' => Trigger::Channel,
        ':' => Trigger::Emoji,
        _ => return None,
    };
    let text: String = chars.collect();
    if trigger == Trigger::Emoji && (text.chars().count() < EMOJI_MIN || text.contains(':')) {
        return None;
    }
    // Already a finished mention like "<@123>" isn't a query.
    if text.contains(['<', '>']) {
        return None;
    }
    Some(Query {
        trigger,
        start,
        end,
        text,
    })
}

/// One suggestion row.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub label: String,
    /// Greyed text on the right (username, category, `:shortcode:`).
    pub detail: String,
    /// Text that replaces the query, e.g. `<@123>`, `<#456>`, `😀`.
    pub insert: String,
    /// Shown left of the label: a Unicode emoji or nothing.
    pub glyph: String,
    /// Custom emoji image URL.
    pub image: Option<String>,
    pub color: Option<Color32>,
}

/// A person, channel or role that could be suggested.
pub struct Candidate {
    pub name: String,
    pub detail: String,
    pub insert: String,
    pub color: Option<Color32>,
}

/// Match candidates by name: prefix matches first, then "contains".
pub fn rank_named(query: &str, candidates: Vec<Candidate>) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    let mut scored: Vec<(u8, Candidate)> = candidates
        .into_iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            let detail = c.detail.to_lowercase();
            let rank = if name.starts_with(&q) {
                0
            } else if detail.starts_with(&q) {
                1
            } else if name.contains(&q) || detail.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, c))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.name.len().cmp(&b.1.name.len()))
    });
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, c)| seen.insert(c.insert.clone()))
        .take(MAX_ITEMS)
        .map(|(_, c)| Suggestion {
            label: c.name,
            detail: c.detail,
            insert: c.insert,
            glyph: String::new(),
            image: None,
            color: c.color,
        })
        .collect()
}

/// Emoji whose shortcode matches: server emoji first, then Unicode.
pub fn emoji(query: &str, guild_emojis: &[GuildEmoji]) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    let mut out: Vec<(u8, Suggestion)> = Vec::new();
    for e in guild_emojis {
        let name = e.name.to_lowercase();
        let rank = if name.starts_with(&q) {
            0
        } else if name.contains(&q) {
            2
        } else {
            continue;
        };
        let ext = if e.animated { "gif" } else { "png" };
        out.push((
            rank,
            Suggestion {
                label: format!(":{}:", e.name),
                detail: "server".into(),
                insert: e.markup(),
                glyph: String::new(),
                image: Some(format!(
                    "https://cdn.discordapp.com/emojis/{}.{ext}?size=32",
                    e.id
                )),
                color: None,
            },
        ));
    }
    for e in emojis::iter() {
        let Some(code) = e
            .shortcodes()
            .find(|c| c.starts_with(&q))
            .map(|c| (0, c))
            .or_else(|| e.shortcodes().find(|c| c.contains(&q)).map(|c| (2, c)))
        else {
            continue;
        };
        out.push((
            code.0 + 1,
            Suggestion {
                label: format!(":{}:", code.1),
                detail: String::new(),
                insert: e.as_str().to_string(),
                glyph: e.as_str().to_string(),
                image: None,
                color: None,
            },
        ));
        if out.len() > 200 {
            break;
        }
    }
    // Stable: keeps the crate's (popularity-ish) order within a rank.
    out.sort_by_key(|(r, s)| (*r, s.label.len()));
    out.into_iter().take(MAX_ITEMS).map(|(_, s)| s).collect()
}

/// Replace the query with `insert` plus a space. Returns the new cursor
/// position in characters.
pub fn apply(draft: &mut String, q: &Query, insert: &str) -> usize {
    let mut text = insert.to_string();
    if !draft[q.end..].starts_with(' ') {
        text.push(' ');
    }
    draft.replace_range(q.start..q.end, &text);
    draft[..q.start + text.len()].chars().count()
        + usize::from(draft[q.start + text.len()..].starts_with(' ') && !text.ends_with(' '))
}

/// Open suggestions for the message box.
#[derive(Default)]
pub struct Autocomplete {
    pub query: Option<Query>,
    pub items: Vec<Suggestion>,
    pub selected: usize,
    /// Esc pressed for this query (start offset): hide until it changes.
    pub dismissed: Option<usize>,
}

impl Autocomplete {
    pub fn active(&self) -> bool {
        self.query.is_some() && !self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.query = None;
        self.items.clear();
        self.selected = 0;
    }
}

/// Draw the list above `anchor` (the message box). Returns a clicked row.
pub fn popup(
    ctx: &egui::Context,
    anchor: egui::Rect,
    ac: &Autocomplete,
    pal: &Palette,
) -> Option<usize> {
    let title = match ac.query.as_ref()?.trigger {
        Trigger::User => "MEMBERS",
        Trigger::Channel => "TEXT CHANNELS",
        Trigger::Emoji => "EMOJI MATCHING",
    };
    let mut clicked = None;
    // Hangs above the message box, growing upwards.
    egui::Area::new(egui::Id::unique("rc_autocomplete"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(egui::pos2(anchor.left(), anchor.top() - 6.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(pal.sidebar)
                .stroke(egui::Stroke::new(1.0, pal.divider))
                .corner_radius(8)
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.set_width(anchor.width() - 12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        let q = &ac
                            .query
                            .as_ref()
                            .map(|q| q.text.clone())
                            .unwrap_or_default();
                        ui.label(pal.section_label(title));
                        if !q.is_empty() {
                            ui.label(RichText::new(q).size(11.5).color(pal.text));
                        }
                    });
                    ui.add_space(2.0);
                    for (i, s) in ac.items.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 32.0),
                            egui::Sense::click(),
                        );
                        if i == ac.selected || resp.hovered() {
                            ui.painter().rect_filled(rect, 4, pal.selected);
                        }
                        let y = rect.center().y;
                        let mut x = rect.left() + 10.0;
                        if let Some(url) = &s.image {
                            let r = egui::Rect::from_center_size(
                                egui::pos2(x + 10.0, y),
                                vec2(20.0, 20.0),
                            );
                            egui::Image::from_uri(url)
                                .show_loading_spinner(false)
                                .paint_at(ui, r);
                            x += 28.0;
                        } else if !s.glyph.is_empty() {
                            ui.painter().text(
                                egui::pos2(x + 10.0, y),
                                egui::Align2::CENTER_CENTER,
                                &s.glyph,
                                egui::FontId::proportional(18.0),
                                pal.text,
                            );
                            x += 28.0;
                        }
                        ui.painter().text(
                            egui::pos2(x, y),
                            egui::Align2::LEFT_CENTER,
                            &s.label,
                            egui::FontId::proportional(15.0),
                            s.color.unwrap_or(pal.strong),
                        );
                        if !s.detail.is_empty() {
                            ui.painter().text(
                                egui::pos2(rect.right() - 10.0, y),
                                egui::Align2::RIGHT_CENTER,
                                &s.detail,
                                egui::FontId::proportional(13.0),
                                pal.muted,
                            );
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                });
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_queries() {
        let q = find_query("hi @sa", 6).unwrap();
        assert_eq!(
            (q.trigger, q.start, q.end, q.text.as_str()),
            (Trigger::User, 3, 6, "sa")
        );
        assert_eq!(find_query("hi @", 4).unwrap().text, "");
        assert_eq!(find_query("#gen", 4).unwrap().trigger, Trigger::Channel);
        // Emoji need two characters, and a finished :code: isn't a query.
        assert!(find_query(":s", 2).is_none());
        assert_eq!(find_query("so :smi", 7).unwrap().trigger, Trigger::Emoji);
        assert!(find_query(":smile:", 7).is_none());
        // Cursor in the middle: only the part before it counts.
        assert_eq!(find_query("@sam hello", 3).unwrap().text, "sa");
        // Not a trigger word.
        assert!(find_query("email@x", 7).is_none());
        assert!(find_query("hello", 5).is_none());
        // Non-ASCII before the cursor (char vs byte offsets).
        let q = find_query("përsh @ä", 8).unwrap();
        assert_eq!(q.text, "ä");
    }

    #[test]
    fn inserts() {
        let mut d = "hi @sa".to_string();
        let q = find_query(&d, 6).unwrap();
        let cur = apply(&mut d, &q, "<@123>");
        assert_eq!(d, "hi <@123> ");
        assert_eq!(cur, 10);
        // Text after the cursor stays; no double space.
        let mut d = "@sa hello".to_string();
        let q = find_query(&d, 3).unwrap();
        let cur = apply(&mut d, &q, "<@1>");
        assert_eq!(d, "<@1> hello");
        assert_eq!(cur, 5);
    }

    #[test]
    fn ranks_names() {
        let c = |n: &str, d: &str| Candidate {
            name: n.into(),
            detail: d.into(),
            insert: format!("<@{n}>"),
            color: None,
        };
        let r = rank_named(
            "sa",
            vec![c("Lisa", "lisa"), c("Sam", "sam"), c("Bob", "sanity")],
        );
        let names: Vec<_> = r.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(names, ["Sam", "Bob", "Lisa"]);
    }

    #[test]
    fn emoji_matches() {
        let guild = vec![GuildEmoji {
            id: "1".into(),
            name: "pepega".into(),
            animated: false,
        }];
        let r = emoji("pep", &guild);
        assert_eq!(r[0].insert, "<:pepega:1>");
        let r = emoji("thumbsup", &[]);
        assert!(r.iter().any(|s| s.insert == "👍"));
    }
}
