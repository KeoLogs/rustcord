//! Top of the Home (DMs) channel list, like Discord's: a "Find or start a
//! conversation" search field, the 👥 Friends and ✉ Message Requests entries,
//! then the "DIRECT MESSAGES" label. The integrator draws the (filtered) DM
//! rows below it with `filter_dms` + `RequestStore::visible_dms`.
//!
//! Typing filters DMs by name; friends without an open DM that match are
//! listed too ("start a conversation"). Ctrl+F is left alone: it belongs to
//! message search.

use eframe::egui::{self, RichText, vec2};

use crate::friends::{AvatarFn, RelKind, Relationship, RelationshipStore, count_badge};
use crate::theme::{self, Palette};
use crate::types::Channel;

/// Which Home page is open, for the selected highlight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HomeSelected {
    /// A DM (or nothing) is open.
    #[default]
    None,
    Friends,
    Requests,
}

pub struct HomeCtx<'a> {
    pub pal: &'a Palette,
    pub selected: HomeSelected,
    /// Message requests (not spam): badge on the entry.
    pub requests: usize,
    /// Incoming friend requests: badge on the Friends entry.
    pub pending_friends: usize,
    /// For "start a conversation" matches while searching.
    pub friends: &'a RelationshipStore,
    /// True if a DM with this user is already in the list.
    pub has_dm: &'a dyn Fn(&str) -> bool,
    pub avatar: AvatarFn<'a>,
}

/// UI state the app keeps for the header (the search text).
#[derive(Default)]
pub struct HomeState {
    pub query: String,
    /// Set to focus the search field next frame.
    pub focus: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeAction {
    Friends,
    Requests,
    /// A friend without an open DM was picked from the search results:
    /// open/create the DM (`FriendsAction::Message`).
    Dm(String),
}

/// Most friend matches shown under the search field.
const MAX_FRIEND_MATCHES: usize = 5;

/// Search field + Friends + Message Requests + "DIRECT MESSAGES" label.
pub fn home_header(
    ui: &mut egui::Ui,
    state: &mut HomeState,
    cx: &HomeCtx<'_>,
) -> Option<HomeAction> {
    let mut action = None;
    search_bar(ui, state, cx.pal);
    ui.add_space(8.0);
    if let Some(a) = home_entries(ui, cx) {
        action = Some(a);
    }
    let q = state.query.trim();
    if !q.is_empty() {
        let matches = friend_matches(q, cx.friends, cx.has_dm);
        if !matches.is_empty() {
            ui.add_space(12.0);
            ui.label(cx.pal.section_label("Friends"));
            ui.add_space(2.0);
            for r in matches {
                if friend_match_row(ui, r, cx) {
                    action = Some(HomeAction::Dm(r.user.id.clone()));
                }
            }
        }
    }
    ui.add_space(12.0);
    ui.label(cx.pal.section_label("Direct Messages"));
    ui.add_space(2.0);
    action
}

/// Discord's "Find or start a conversation" field (28 px, darkest fill).
/// Escape clears it.
pub fn search_bar(ui: &mut egui::Ui, state: &mut HomeState, pal: &Palette) -> egui::Response {
    let inner = egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 5))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .id_salt("home_search")
                        .hint_text(
                            RichText::new("Find or start a conversation")
                                .size(13.0)
                                .color(pal.muted),
                        )
                        .font(egui::FontId::proportional(13.0))
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .desired_width(ui.available_width() - 18.0),
                );
                if std::mem::take(&mut state.focus) {
                    resp.request_focus();
                }
                if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    state.query.clear();
                }
                if !state.query.is_empty()
                    && ui
                        .add(
                            egui::Label::new(RichText::new("✕").size(13.0).color(pal.muted))
                                .selectable(false)
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                {
                    state.query.clear();
                }
                resp
            })
            .inner
        });
    inner.inner
}

/// Just the Friends and Message Requests entries.
pub fn home_entries(ui: &mut egui::Ui, cx: &HomeCtx<'_>) -> Option<HomeAction> {
    let mut action = None;
    if entry(
        ui,
        cx.pal,
        "👥",
        "Friends",
        cx.selected == HomeSelected::Friends,
        cx.pending_friends,
    ) {
        action = Some(HomeAction::Friends);
    }
    if entry(
        ui,
        cx.pal,
        "✉",
        "Message Requests",
        cx.selected == HomeSelected::Requests,
        cx.requests,
    ) {
        action = Some(HomeAction::Requests);
    }
    action
}

/// One 42 px entry with the channel list's selected / hover look.
fn entry(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: &str,
    label: &str,
    selected: bool,
    badge: usize,
) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 42.0), egui::Sense::click());
    let hovered = resp.hovered();
    if selected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if hovered {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let color = if selected {
        pal.strong
    } else if hovered || badge > 0 {
        pal.text
    } else {
        pal.muted
    };
    let x = rect.left() + 8.0;
    ui.painter().text(
        egui::pos2(x + 16.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(20.0),
        color,
    );
    let font = if selected {
        theme::bold_font(15.0)
    } else {
        egui::FontId::proportional(15.0)
    };
    ui.painter().text(
        egui::pos2(x + 44.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    if badge > 0 {
        count_badge(
            ui,
            egui::pos2(rect.right() - 8.0, rect.center().y),
            badge,
            pal,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn friend_match_row(ui: &mut egui::Ui, r: &Relationship, cx: &HomeCtx<'_>) -> bool {
    let pal = cx.pal;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 42.0), egui::Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let a = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 24.0, rect.center().y),
        vec2(32.0, 32.0),
    );
    (cx.avatar)(ui, a, &r.user);
    let x = a.right() + 12.0;
    let color = if hovered { pal.text } else { pal.muted };
    let clip = egui::Rect::from_min_max(egui::pos2(x, rect.top()), rect.right_bottom());
    ui.painter().with_clip_rect(clip).text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        r.display_name(),
        egui::FontId::proportional(15.0),
        color,
    );
    resp.on_hover_text(if r.user.username.is_empty() {
        "Start a conversation".to_string()
    } else {
        format!("@{} · start a conversation", r.user.username)
    })
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .clicked()
}

fn contains(hay: &str, needle_lower: &str) -> bool {
    hay.to_lowercase().contains(needle_lower)
}

/// DMs whose name or any recipient's display name / username contains
/// `query` (case-insensitive). Empty query keeps everything, in order.
pub fn filter_dms<'a>(query: &str, dms: impl IntoIterator<Item = &'a Channel>) -> Vec<&'a Channel> {
    let q = query.trim().to_lowercase();
    dms.into_iter()
        .filter(|c| {
            q.is_empty()
                || contains(&c.name, &q)
                || c.recipients
                    .iter()
                    .any(|u| contains(&u.name, &q) || contains(&u.username, &q))
        })
        .collect()
}

/// Friends matching `query` that have no open DM yet, best matches first
/// (name starts with the query), at most `MAX_FRIEND_MATCHES`.
pub fn friend_matches<'a>(
    query: &str,
    store: &'a RelationshipStore,
    has_dm: &dyn Fn(&str) -> bool,
) -> Vec<&'a Relationship> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut list: Vec<&Relationship> = store
        .iter()
        .filter(|r| r.kind == RelKind::Friend && r.matches(&q) && !has_dm(&r.user.id))
        .collect();
    list.sort_by_cached_key(|r| {
        let name = r.display_name().to_lowercase();
        (!name.starts_with(&q), name, r.user.id.clone())
    });
    list.truncate(MAX_FRIEND_MATCHES);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::friends::parse_ready;
    use serde_json::json;

    fn dms() -> Vec<Channel> {
        [
            json!({ "id": "1", "type": 1, "recipients": [{ "id": "a", "username": "alice", "global_name": "Alice A" }] }),
            json!({ "id": "2", "type": 1, "recipients": [{ "id": "b", "username": "bob" }] }),
            json!({ "id": "3", "type": 3, "name": "Raid Squad", "recipients": [
                { "id": "c", "username": "carol" }, { "id": "a", "username": "alice" } ] }),
        ]
        .iter()
        .map(Channel::from_json)
        .collect()
    }

    fn ids(list: &[&Channel]) -> Vec<String> {
        list.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn filters_dms() {
        let d = dms();
        assert_eq!(ids(&filter_dms("", &d)), ["1", "2", "3"]);
        assert_eq!(ids(&filter_dms("  ", &d)), ["1", "2", "3"]);
        // Display name, username, group name, group member; any case.
        assert_eq!(ids(&filter_dms("ALICE", &d)), ["1", "3"]);
        assert_eq!(ids(&filter_dms("bo", &d)), ["2"]);
        assert_eq!(ids(&filter_dms("squad", &d)), ["3"]);
        assert_eq!(ids(&filter_dms("carol", &d)), ["3"]);
        assert!(filter_dms("zzz", &d).is_empty());
    }

    #[test]
    fn friend_search_skips_open_dms() {
        let mut store = RelationshipStore::default();
        store.set_all(parse_ready(&json!({ "relationships": [
            { "id": "a", "type": 1, "user": { "id": "a", "username": "alice" } },
            { "id": "m", "type": 1, "user": { "id": "m", "username": "malia" } },
            { "id": "l", "type": 1, "user": { "id": "l", "username": "alina" } },
            { "id": "x", "type": 3, "user": { "id": "x", "username": "alfred" } },
        ]})));
        let has_dm = |id: &str| id == "a";
        let names: Vec<&str> = friend_matches("ali", &store, &has_dm)
            .iter()
            .map(|r| r.user.username.as_str())
            .collect();
        // alice has a DM; alfred is only a request; "alina" starts with it.
        assert_eq!(names, ["alina", "malia"]);
        assert!(friend_matches("", &store, &has_dm).is_empty());
    }
}
