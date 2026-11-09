//! Status picker (Online / Idle / Do Not Disturb / Invisible) and the
//! "Set custom status" dialog, opened from the avatar in the profile bar.
//!
//! Changes are explicit user actions: the app saves them to Discord's synced
//! settings (`PATCH users/@me/settings-proto/1`, see `settings_proto.rs`) and
//! sends a gateway op 3 presence update ([`presence_op`]) so the new status
//! shows straight away, like Discord's client does.

use chrono::{Local, TimeZone};
use eframe::egui::{self, Color32, RichText};
use serde_json::{Value, json};

use crate::icons;
use crate::settings_proto::{CustomStatus, Status};
use crate::theme::{Bold, Palette};
use crate::widgets::{self, Tone};

/// Gateway op 3. `custom` becomes a type 4 "Custom Status" activity.
pub fn presence_op(status: &str, custom: Option<&CustomStatus>) -> Value {
    let activities: Vec<Value> = custom
        .filter(|c| !c.text.is_empty() || c.emoji_name.is_some())
        .map(|c| {
            let mut a = json!({ "type": 4, "name": "Custom Status", "state": c.text });
            if let Some(name) = &c.emoji_name {
                a["emoji"] = match c.emoji_id {
                    Some(id) => json!({ "name": name, "id": id.to_string(), "animated": false }),
                    None => json!({ "name": name }),
                };
            }
            a
        })
        .into_iter()
        .collect();
    json!({ "op": 3, "d": {
        "status": status,
        "since": 0,
        "activities": activities,
        "afk": false,
    }})
}

pub const ALL: [Status; 4] = [Status::Online, Status::Idle, Status::Dnd, Status::Invisible];

pub fn label(s: Status) -> &'static str {
    match s {
        Status::Online => "Online",
        Status::Idle => "Idle",
        Status::Dnd => "Do Not Disturb",
        Status::Invisible => "Invisible",
    }
}

pub fn color(s: Status, pal: &Palette) -> Color32 {
    match s {
        Status::Online => pal.online,
        Status::Idle => pal.idle,
        Status::Dnd => pal.dnd,
        Status::Invisible => pal.offline,
    }
}

/// What the user picked in the status menu.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Set(Status),
    EditCustom,
    ClearCustom,
}

/// The menu under the profile bar avatar: Discord's minimal picker, one
/// full-width row per status (dot + label), then the custom status rows.
pub fn menu(
    ui: &mut egui::Ui,
    current: Status,
    custom: Option<&CustomStatus>,
    pal: &Palette,
) -> Option<MenuAction> {
    let mut action = None;
    ui.set_width(220.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    for s in ALL {
        if widgets::dot_item(ui, color(s, pal), label(s), s == current, pal).clicked() {
            action = Some(MenuAction::Set(s));
        }
    }
    widgets::menu_separator(ui, pal);
    let text = match custom {
        Some(c) => {
            let t = if c.text.is_empty() {
                "Edit custom status"
            } else {
                &c.text
            };
            match c.emoji_name.as_deref() {
                Some(e) => format!("{e} {t}"),
                None => t.to_string(),
            }
        }
        None => "Set custom status".into(),
    };
    if widgets::menu_item(ui, Some(icons::SMILEY), &text, Tone::Normal, pal).clicked() {
        action = Some(MenuAction::EditCustom);
    }
    if custom.is_some()
        && widgets::menu_item(ui, Some(icons::X), "Clear custom status", Tone::Normal, pal)
            .clicked()
    {
        action = Some(MenuAction::ClearCustom);
    }
    action
}

/// "Clear after" choices of the custom status dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearAfter {
    Minutes30,
    Hour1,
    Hours4,
    Today,
    Never,
}

impl ClearAfter {
    pub const ALL: [ClearAfter; 5] = [
        ClearAfter::Minutes30,
        ClearAfter::Hour1,
        ClearAfter::Hours4,
        ClearAfter::Today,
        ClearAfter::Never,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ClearAfter::Minutes30 => "30 minutes",
            ClearAfter::Hour1 => "1 hour",
            ClearAfter::Hours4 => "4 hours",
            ClearAfter::Today => "Today",
            ClearAfter::Never => "Don't clear",
        }
    }

    /// When the status expires (unix ms), from `now_ms`. "Today" = local
    /// midnight.
    pub fn expires_at(self, now_ms: u64) -> Option<u64> {
        let mins = |m: u64| Some(now_ms + m * 60_000);
        match self {
            ClearAfter::Minutes30 => mins(30),
            ClearAfter::Hour1 => mins(60),
            ClearAfter::Hours4 => mins(240),
            ClearAfter::Never => None,
            ClearAfter::Today => {
                let now = Local.timestamp_millis_opt(now_ms as i64).single()?;
                let midnight = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
                let t = Local.from_local_datetime(&midnight).earliest()?;
                Some(t.timestamp_millis() as u64)
            }
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What the dialog decided.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogResult {
    Save(CustomStatus),
    Clear,
    Cancel,
}

/// "Set a custom status" modal.
pub struct CustomStatusDialog {
    text: String,
    emoji: String,
    clear_after: ClearAfter,
}

impl CustomStatusDialog {
    pub fn new(existing: Option<&CustomStatus>) -> Self {
        Self {
            text: existing.map(|c| c.text.clone()).unwrap_or_default(),
            emoji: existing
                .and_then(|c| c.emoji_name.clone())
                .unwrap_or_default(),
            clear_after: if existing.is_some_and(|c| c.expires_at_ms.is_none()) {
                ClearAfter::Never
            } else {
                ClearAfter::Today
            },
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> Option<DialogResult> {
        let mut result = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_custom_status")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new("Set a custom status")
                    .bold()
                    .size(18.0)
                    .color(pal.strong),
            );
            ui.add_space(10.0);
            ui.label(pal.section_label("WHAT'S COOKING?"));
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.emoji)
                        .hint_text("😀")
                        .desired_width(36.0)
                        .char_limit(16),
                )
                .on_hover_text("Emoji (paste or type one)");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.text)
                        .hint_text("Support has arrived!")
                        .desired_width(f32::INFINITY)
                        .char_limit(128),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    result = Some(self.save());
                }
            });
            ui.add_space(8.0);
            ui.label(pal.section_label("CLEAR AFTER"));
            egui::ComboBox::from_id_salt("rc_clear_after")
                .selected_text(self.clear_after.label())
                .width(200.0)
                .show_ui(ui, |ui| {
                    for c in ClearAfter::ALL {
                        ui.selectable_value(&mut self.clear_after, c, c.label());
                    }
                });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    result = Some(DialogResult::Cancel);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(pal.primary_button("Save")).clicked() {
                        result = Some(self.save());
                    }
                    if ui.button("Clear status").clicked() {
                        result = Some(DialogResult::Clear);
                    }
                });
            });
        });
        if result.is_none() && modal.should_close() {
            result = Some(DialogResult::Cancel);
        }
        result
    }

    fn save(&self) -> DialogResult {
        let text = self.text.trim().to_string();
        let emoji = self.emoji.trim().to_string();
        if text.is_empty() && emoji.is_empty() {
            return DialogResult::Clear;
        }
        let now = now_ms();
        DialogResult::Save(CustomStatus {
            text,
            emoji_name: (!emoji.is_empty()).then_some(emoji),
            emoji_id: None,
            expires_at_ms: self.clear_after.expires_at(now),
            created_at_ms: Some(now),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op3_with_custom_status() {
        let c = CustomStatus {
            text: "coding".into(),
            emoji_name: Some("🦀".into()),
            ..Default::default()
        };
        let op = presence_op("dnd", Some(&c));
        assert_eq!(op["op"], 3);
        assert_eq!(op["d"]["status"], "dnd");
        assert_eq!(op["d"]["afk"], false);
        let a = &op["d"]["activities"][0];
        assert_eq!(a["type"], 4);
        assert_eq!(a["state"], "coding");
        assert_eq!(a["emoji"]["name"], "🦀");
        // No custom status: no activities.
        let op = presence_op("online", None);
        assert_eq!(op["d"]["activities"], json!([]));
    }

    /// Op 3 only carries status + activities: it must never touch the
    /// IDENTIFY properties that decide "Show as" (desktop / mobile / web).
    #[test]
    fn op3_keeps_show_as() {
        let op = presence_op("idle", None);
        let mut keys: Vec<&str> = op["d"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["activities", "afk", "since", "status"]);
        let text = op.to_string();
        for k in ["properties", "browser", "client_status", "os"] {
            assert!(!text.contains(&format!("\"{k}\"")), "{k} in {text}");
        }
    }

    #[test]
    fn clear_after() {
        let now = 1_000_000_000_000;
        assert_eq!(ClearAfter::Minutes30.expires_at(now), Some(now + 1_800_000));
        assert_eq!(
            ClearAfter::Hours4.expires_at(now),
            Some(now + 4 * 3_600_000)
        );
        assert_eq!(ClearAfter::Never.expires_at(now), None);
        let today = ClearAfter::Today.expires_at(now).unwrap();
        assert!(today > now && today <= now + 25 * 3_600_000);
    }
}
