//! "New Group DM": tick up to 9 friends, then press Create.
//!
//! Part of the Friends module (the dialog only exists while it is open).
//! Nothing is sent while friends are being ticked: the one request
//! (`FriendsAction::CreateGroup`) is made by [`GroupDmDialog::create`],
//! which the UI calls only from the Create button's click.

use eframe::egui::{self, RichText, vec2};

use crate::friends::{AvatarFn, FriendsAction, RelKind, RelationshipStore};
use crate::icons;
use crate::theme::{Bold as _, Palette};

/// A group DM holds 10 people: you and up to 9 friends.
pub const MAX_FRIENDS: usize = 9;

/// The open dialog.
#[derive(Default, Debug)]
pub struct GroupDmDialog {
    /// Ticked friends (user ids), in the order they were ticked.
    selected: Vec<String>,
    /// Filter text.
    query: String,
}

/// What the dialog wants done after a frame.
#[derive(Debug, PartialEq, Eq)]
pub enum GroupDmEvent {
    /// Cancel / Escape / clicked outside.
    Close,
    /// The Create button was clicked.
    Create(FriendsAction),
}

impl GroupDmDialog {
    pub fn is_selected(&self, user_id: &str) -> bool {
        self.selected.iter().any(|s| s == user_id)
    }

    /// Tick or untick a friend. Returns false if the group is already full.
    pub fn toggle(&mut self, user_id: &str) -> bool {
        if let Some(i) = self.selected.iter().position(|s| s == user_id) {
            self.selected.remove(i);
            true
        } else if self.selected.len() < MAX_FRIENDS && !user_id.is_empty() {
            self.selected.push(user_id.to_owned());
            true
        } else {
            false
        }
    }

    pub fn can_create(&self) -> bool {
        !self.selected.is_empty() && self.selected.len() <= MAX_FRIENDS
    }

    /// How many more friends fit.
    pub fn remaining(&self) -> usize {
        MAX_FRIENDS.saturating_sub(self.selected.len())
    }

    /// The request for the Create click: `None` when nothing is ticked.
    /// One friend is an ordinary DM; more make a group. People who stopped
    /// being friends while the dialog was open are left out.
    pub fn create(&self, friends: &RelationshipStore) -> Option<FriendsAction> {
        let ids: Vec<String> = self
            .selected
            .iter()
            .filter(|id| is_friend(friends, id))
            .take(MAX_FRIENDS)
            .cloned()
            .collect();
        match ids.len() {
            0 => None,
            1 => ids.into_iter().next().map(FriendsAction::Message),
            _ => Some(FriendsAction::CreateGroup(ids)),
        }
    }

    /// Draw the dialog. The returned `Create` only ever comes from the
    /// Create button being clicked this frame.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pal: &Palette,
        friends: &RelationshipStore,
        avatar: AvatarFn<'_>,
    ) -> Option<GroupDmEvent> {
        let mut event = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_new_group_dm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new("Select Friends")
                    .bold()
                    .size(16.0)
                    .color(pal.strong),
            );
            ui.label(
                RichText::new(match self.remaining() {
                    0 => "This group is full.".to_owned(),
                    1 => "You can add 1 more friend.".to_owned(),
                    n => format!("You can add {n} more friends."),
                })
                .small()
                .color(pal.muted),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Type the username of a friend")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            let rows = matching(friends, &self.query);
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        ui.label(RichText::new("No friends found.").color(pal.muted));
                    }
                    for r in rows {
                        let id = &r.user.id;
                        let ticked = self.is_selected(id);
                        let (rect, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 40.0),
                            egui::Sense::click(),
                        );
                        if resp.hovered() || ticked {
                            ui.painter()
                                .rect_filled(rect, crate::theme::corner(4), pal.hover);
                        }
                        let av = egui::Rect::from_center_size(
                            rect.left_center() + vec2(24.0, 0.0),
                            vec2(32.0, 32.0),
                        );
                        avatar(ui, av, &r.user);
                        ui.painter().text(
                            rect.left_center() + vec2(50.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            r.display_name(),
                            egui::FontId::proportional(14.0),
                            pal.strong,
                        );
                        ui.painter().text(
                            rect.right_center() - vec2(16.0, 0.0),
                            egui::Align2::CENTER_CENTER,
                            if ticked {
                                icons::CHECK_SQUARE
                            } else {
                                icons::SQUARE
                            },
                            egui::FontId::proportional(18.0),
                            if ticked { pal.accent } else { pal.muted },
                        );
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.toggle(id);
                        }
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let label = if self.selected.len() > 1 {
                    "Create Group DM"
                } else {
                    "Create DM"
                };
                if ui
                    .add_enabled(self.can_create(), pal.primary_button(label))
                    .clicked()
                    && let Some(action) = self.create(friends)
                {
                    event = Some(GroupDmEvent::Create(action));
                }
                if ui.button("Cancel").clicked() {
                    event = Some(GroupDmEvent::Close);
                }
            });
        });
        if event.is_none() && modal.should_close() {
            event = Some(GroupDmEvent::Close);
        }
        event
    }
}

fn is_friend(friends: &RelationshipStore, user_id: &str) -> bool {
    friends
        .iter()
        .any(|r| r.kind == RelKind::Friend && r.user.id == user_id)
}

/// Friends whose name contains the filter text, by name.
fn matching<'a>(
    friends: &'a RelationshipStore,
    query: &str,
) -> Vec<&'a crate::friends::Relationship> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<_> = friends
        .iter()
        .filter(|r| r.kind == RelKind::Friend)
        .filter(|r| {
            q.is_empty()
                || r.display_name().to_lowercase().contains(&q)
                || r.user.name.to_lowercase().contains(&q)
        })
        .collect();
    rows.sort_by_key(|r| r.display_name().to_lowercase());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::friends::Relationship;
    use serde_json::json;

    fn store(friends: usize) -> RelationshipStore {
        let mut list: Vec<Relationship> = (1..=friends)
            .filter_map(|i| {
                Relationship::from_json(
                    &json!({
                        "id": i.to_string(), "type": 1,
                        "user": { "id": i.to_string(), "username": format!("friend{i}") },
                    }),
                    &Default::default(),
                )
            })
            .collect();
        // A blocked user and an incoming request: never pickable.
        for (id, kind, name) in [("900", 2, "blocked"), ("901", 3, "pending")] {
            list.extend(Relationship::from_json(
                &json!({ "id": id, "type": kind, "user": { "id": id, "username": name } }),
                &Default::default(),
            ));
        }
        let mut s = RelationshipStore::default();
        s.set_all(list);
        s
    }

    #[test]
    fn ticking_friends_never_creates_anything() {
        let friends = store(4);
        let mut d = GroupDmDialog::default();
        // Nothing ticked: nothing to create, and the button is disabled.
        assert!(!d.can_create());
        assert_eq!(d.create(&friends), None);
        // Ticking is local state only: `toggle` returns a bool, not a
        // request, and the dialog holds no sender.
        assert!(d.toggle("1"));
        assert!(d.toggle("2"));
        assert!(d.toggle("3"));
        assert!(d.toggle("2"));
        assert!(d.is_selected("1") && !d.is_selected("2") && d.is_selected("3"));
        // The request exists only when `create` (the click) asks for it.
        assert_eq!(
            d.create(&friends),
            Some(FriendsAction::CreateGroup(vec!["1".into(), "3".into()]))
        );
    }

    #[test]
    fn one_friend_is_a_plain_dm() {
        let friends = store(3);
        let mut d = GroupDmDialog::default();
        d.toggle("2");
        assert!(d.can_create());
        assert_eq!(d.create(&friends), Some(FriendsAction::Message("2".into())));
    }

    #[test]
    fn at_most_nine_friends() {
        let friends = store(12);
        let mut d = GroupDmDialog::default();
        for i in 1..=9 {
            assert!(d.toggle(&i.to_string()), "{i}");
        }
        assert_eq!(d.remaining(), 0);
        assert!(!d.toggle("10"));
        assert!(!d.is_selected("10"));
        // Unticking makes room again.
        assert!(d.toggle("4"));
        assert!(d.toggle("10"));
        match d.create(&friends) {
            Some(FriendsAction::CreateGroup(ids)) => assert_eq!(ids.len(), 9),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_current_friends_are_sent() {
        let friends = store(2);
        let mut d = GroupDmDialog::default();
        for id in ["1", "2", "900", "901", "777", ""] {
            d.toggle(id);
        }
        assert_eq!(
            d.create(&friends),
            Some(FriendsAction::CreateGroup(vec!["1".into(), "2".into()]))
        );
    }

    #[test]
    fn the_filter_lists_friends_only() {
        let friends = store(3);
        assert_eq!(matching(&friends, "").len(), 3);
        assert_eq!(matching(&friends, "FRIEND2").len(), 1);
        assert!(matching(&friends, "blocked").is_empty());
        assert!(matching(&friends, "pending").is_empty());
    }
}
