//! Ctrl+K quick switcher (Discord Messenger: `windows/QuickSwitcher.cpp`):
//! fuzzy search over channels, DMs and servers, recently visited first.
//! Arrow keys move, Enter goes, Esc closes.

use eframe::egui::{self, RichText, vec2};

use crate::theme::{Bold, Palette};

/// Most results shown.
const MAX_RESULTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Channel,
    Dm,
    Server,
}

/// Something the switcher can jump to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// "general", "Sam", "RustCord Test Server".
    pub name: String,
    /// Shown on the right: the server name for channels.
    pub detail: String,
    pub guild_id: String,
    /// None for servers.
    pub channel_id: Option<String>,
    /// Position in the recently-visited list (0 = most recent).
    pub recent: Option<usize>,
}

/// Fuzzy match: every query character must appear in order. Higher is
/// better; consecutive runs, word starts and a prefix match score extra.
pub fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score = 0;
    let mut prev_match: Option<usize> = None;
    for (ti, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if prev_match == Some(ti.wrapping_sub(1)) {
                score += 5;
            }
            if ti == 0 || matches!(t[ti - 1], ' ' | '-' | '_' | '.') {
                score += 8;
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    if t.starts_with(&q) {
        score += 20;
    }
    // Shorter names win ties.
    Some(score * 4 - t.len() as i32)
}

/// Best matches for `query`. With an empty query: recently visited first,
/// then everything else in order.
pub fn rank(entries: &[Entry], query: &str) -> Vec<Entry> {
    let query = query.trim();
    // "#gen" / "@sam" / "*server" narrow the kind, like Discord.
    let (kind, query) = match query.chars().next() {
        Some('#') => (Some(Kind::Channel), &query[1..]),
        Some('@') => (Some(Kind::Dm), &query[1..]),
        Some('*') => (Some(Kind::Server), &query[1..]),
        _ => (None, query),
    };
    let mut scored: Vec<(i32, &Entry)> = entries
        .iter()
        .filter(|e| kind.is_none_or(|k| k == e.kind))
        .filter_map(|e| {
            let s = score(query, &e.name)?;
            // Recent places float up.
            let bonus = e.recent.map_or(0, |r| 60 - (r as i32 * 2).min(58));
            Some((s + bonus, e))
        })
        .collect();
    if query.is_empty() {
        scored.retain(|(_, e)| e.recent.is_some() || kind.is_some());
    }
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, e)| e.clone())
        .collect()
}

/// The switcher's state while open.
#[derive(Default)]
pub struct QuickSwitcher {
    pub query: String,
    selected: usize,
    opened: bool,
}

pub enum Action {
    Close,
    Go(Entry),
}

impl QuickSwitcher {
    /// Draw the modal. `results` should be `rank(entries, &self.query)`.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        results: &[Entry],
        pal: &Palette,
    ) -> Option<Action> {
        let mut action = None;
        // Keys first, so the text box doesn't eat them.
        let (up, down, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        if !results.is_empty() {
            if down {
                self.selected = (self.selected + 1) % results.len();
            }
            if up {
                self.selected = (self.selected + results.len() - 1) % results.len();
            }
        }
        self.selected = self.selected.min(results.len().saturating_sub(1));
        if enter && let Some(e) = results.get(self.selected) {
            action = Some(Action::Go(e.clone()));
        }

        let modal = egui::Modal::new(egui::Id::unique("rc_quick_switcher")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(
                RichText::new("Search for servers, channels or DMs")
                    .bold()
                    .size(15.0)
                    .color(pal.strong),
            );
            ui.add_space(6.0);
            let before = self.query.clone();
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Where would you like to go?")
                    .font(egui::FontId::proportional(17.0))
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(10, 8)),
            );
            if !self.opened {
                r.request_focus();
                self.opened = true;
            }
            if self.query != before {
                self.selected = 0;
            }
            ui.add_space(6.0);
            if results.is_empty() {
                ui.label(RichText::new("No results").color(pal.muted));
            } else if self.query.trim().is_empty() {
                ui.label(pal.section_label("PREVIOUS CHANNELS"));
            }
            for (i, e) in results.iter().enumerate() {
                if result_row(ui, e, i == self.selected, pal).clicked() {
                    action = Some(Action::Go(e.clone()));
                }
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Tip: start with # for channels, @ for DMs, * for servers · ↑↓ Enter",
                )
                .size(11.0)
                .color(pal.muted),
            );
        });
        if action.is_none() && modal.should_close() {
            action = Some(Action::Close);
        }
        action
    }
}

fn result_row(ui: &mut egui::Ui, e: &Entry, selected: bool, pal: &Palette) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.selected);
    }
    let icon = match e.kind {
        Kind::Channel => "#",
        Kind::Dm => "@",
        Kind::Server => "🏠",
    };
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(16.0),
        pal.muted,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, y),
        egui::Align2::LEFT_CENTER,
        &e.name,
        crate::theme::bold_font(15.0),
        if selected { pal.strong } else { pal.text },
    );
    if !e.detail.is_empty() {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, y),
            egui::Align2::RIGHT_CENTER,
            e.detail.to_uppercase(),
            crate::theme::bold_font(11.5),
            pal.muted,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, name: &str, recent: Option<usize>) -> Entry {
        Entry {
            kind,
            name: name.into(),
            detail: String::new(),
            guild_id: "g".into(),
            channel_id: Some(name.into()),
            recent,
        }
    }

    #[test]
    fn fuzzy_scores() {
        assert!(score("gen", "general").is_some());
        assert!(score("gnrl", "general").is_some());
        assert!(score("xyz", "general").is_none());
        assert!(score("gen", "general") > score("gen", "big-energy"));
        // Word starts count.
        assert!(score("ts", "test-server") > score("ts", "pests"));
    }

    #[test]
    fn ranking() {
        let entries = vec![
            entry(Kind::Channel, "general", None),
            entry(Kind::Channel, "memes", Some(0)),
            entry(Kind::Dm, "Sam", None),
            entry(Kind::Server, "Gaming Hangout", Some(3)),
        ];
        // Empty query: only recent places, most recent first.
        let r = rank(&entries, "");
        assert_eq!(
            r.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["memes", "Gaming Hangout"]
        );
        // Prefix filters.
        let r = rank(&entries, "@");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "Sam");
        let r = rank(&entries, "#g");
        assert_eq!(r[0].name, "general");
        // Plain query across kinds.
        let r = rank(&entries, "ga");
        assert_eq!(r[0].name, "Gaming Hangout");
    }
}
