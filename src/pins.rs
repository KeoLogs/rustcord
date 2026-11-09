//! Pinned messages popup (Discord Messenger: `windows/PinList.cpp`).
//!
//! Data in, actions out: the popup never does networking. The caller sends
//! `GET channels/{id}/pins` through the backend (`Api::get_pins`), feeds the
//! answer in with [`PinsPopup::set_pins`] / [`PinsPopup::set_failed`], and
//! turns the returned [`PinsAction`]s into backend requests. Unpinning only
//! ever happens because the user pressed "Unpin" and confirmed it.

use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::Value;

use crate::config::Config;
use crate::embed;
use crate::theme::Palette;
use crate::types::{Message, User};

/// Longest preview shown per pin.
const PREVIEW_CHARS: usize = 300;
const POPUP_WIDTH: f32 = 420.0;
const LIST_HEIGHT: f32 = 420.0;
const AVATAR: f32 = 24.0;

/// A pinned message plus the bits the popup shows.
#[derive(Debug, Clone)]
pub struct PinnedMessage {
    pub message: Message,
    /// One-line text: content, else embed / attachment summary.
    pub preview: String,
    /// When it was pinned (newer pins endpoint only).
    pub pinned_at: Option<String>,
}

impl PinnedMessage {
    pub fn from_json(v: &Value) -> Self {
        let message = Message::from_json(v);
        let pinned_at = v
            .get("pinned_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        let preview = preview_for(v, &message);
        Self {
            message,
            preview,
            pinned_at,
        }
    }

    /// Parse a whole `Api::get_pins` answer, skipping entries without an id.
    pub fn list_from_json(values: &[Value]) -> Vec<Self> {
        values
            .iter()
            .map(Self::from_json)
            .filter(|p| !p.message.id.is_empty())
            .collect()
    }
}

fn preview_for(v: &Value, m: &Message) -> String {
    let text = m.content.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        return truncate(&text, PREVIEW_CHARS);
    }
    if let Some(e) = embed::parse_embeds(v).first() {
        return truncate(&format!("[{}]", e.summary()), PREVIEW_CHARS);
    }
    match m.attachments.len() {
        0 => "(no text)".into(),
        1 => format!("📎 {}", m.attachments[0].filename),
        n => format!("📎 {n} attachments"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

#[derive(Debug, Clone)]
pub enum PinsState {
    Loading,
    Loaded(Vec<PinnedMessage>),
    Failed(String),
}

/// What the user asked for. The caller does the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinsAction {
    /// Scroll to / load around this message (and close the popup).
    Jump {
        channel_id: String,
        message_id: String,
    },
    /// Confirmed "Unpin": send `DELETE channels/{c}/messages/pins/{m}`.
    Unpin {
        channel_id: String,
        message_id: String,
    },
    /// Load the list again after an error.
    Retry { channel_id: String },
}

/// Display name and optional colour for a message author.
pub type AuthorFn<'a> = &'a dyn Fn(&User) -> (String, Option<Color32>);

/// How to draw the list.
pub struct PinsView<'a> {
    pub pal: &'a Palette,
    pub config: &'a Config,
    /// Show "Unpin" (DMs, or Manage Messages in a server).
    pub can_unpin: bool,
    /// Display name and colour for an author (server nick / role colour).
    /// `None`: the user's global name in the normal text colour.
    pub author: Option<AuthorFn<'a>>,
}

/// The pinned messages popup for one channel.
#[derive(Debug, Clone)]
pub struct PinsPopup {
    pub channel_id: String,
    /// "#general" / "@Leo", shown in the header.
    pub channel_name: String,
    state: PinsState,
    /// Row asking "Unpin this message?".
    confirm_unpin: Option<String>,
    /// Unpin requests sent but not answered yet.
    unpinning: Vec<String>,
    /// Last unpin error, shown above the list.
    error: Option<String>,
}

impl PinsPopup {
    /// A popup in the loading state. Send `LoadPins` alongside it.
    pub fn new(channel_id: impl Into<String>, channel_name: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            channel_name: channel_name.into(),
            state: PinsState::Loading,
            confirm_unpin: None,
            unpinning: Vec::new(),
            error: None,
        }
    }

    #[cfg(test)]
    pub fn state(&self) -> &PinsState {
        &self.state
    }

    #[cfg(test)]
    pub fn pins(&self) -> &[PinnedMessage] {
        match &self.state {
            PinsState::Loaded(p) => p,
            _ => &[],
        }
    }

    pub fn set_loading(&mut self) {
        self.state = PinsState::Loading;
    }

    /// The list arrived (also used to refresh on CHANNEL_PINS_UPDATE).
    pub fn set_pins(&mut self, pins: Vec<PinnedMessage>) {
        self.unpinning
            .retain(|id| pins.iter().any(|p| &p.message.id == id));
        if let Some(id) = &self.confirm_unpin
            && !pins.iter().any(|p| &p.message.id == id)
        {
            self.confirm_unpin = None;
        }
        self.state = PinsState::Loaded(pins);
    }

    pub fn set_failed(&mut self, error: impl Into<String>) {
        self.state = PinsState::Failed(error.into());
    }

    /// Drop a message (unpinned, or deleted). Returns true if it was listed.
    pub fn remove(&mut self, message_id: &str) -> bool {
        self.unpinning.retain(|id| id != message_id);
        if self.confirm_unpin.as_deref() == Some(message_id) {
            self.confirm_unpin = None;
        }
        match &mut self.state {
            PinsState::Loaded(pins) => {
                let before = pins.len();
                pins.retain(|p| p.message.id != message_id);
                pins.len() != before
            }
            _ => false,
        }
    }

    /// The unpin request failed: re-enable the row and show why.
    pub fn unpin_failed(&mut self, message_id: &str, error: impl Into<String>) {
        self.unpinning.retain(|id| id != message_id);
        self.error = Some(error.into());
    }

    /// Draw the popup contents into `ui` (put it in an `egui::Popup::menu`
    /// or an `Area`).
    pub fn show_contents(&mut self, ui: &mut egui::Ui, view: &PinsView) -> Option<PinsAction> {
        let pal = view.pal;
        let mut action = None;
        ui.set_width(POPUP_WIDTH);

        ui.horizontal(|ui| {
            ui.label(RichText::new("📌 Pinned Messages").strong().size(15.0));
            ui.add(
                egui::Label::new(RichText::new(&self.channel_name).small().color(pal.muted))
                    .truncate(),
            );
        });
        ui.separator();

        if let Some(err) = &self.error {
            let mut dismiss = false;
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(err).small().color(pal.danger)).wrap());
                dismiss = ui.small_button("✕").clicked();
            });
            if dismiss {
                self.error = None;
            }
        }

        match &self.state {
            PinsState::Loading => {
                centred(ui, |ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading pinned messages…").color(pal.muted));
                });
            }
            PinsState::Failed(err) => {
                let mut retry = false;
                centred(ui, |ui| {
                    ui.label(RichText::new("Couldn't load pinned messages.").color(pal.text));
                    ui.add(egui::Label::new(RichText::new(err).small().color(pal.muted)).wrap());
                    retry = ui.button("Try again").clicked();
                });
                if retry {
                    action = Some(PinsAction::Retry {
                        channel_id: self.channel_id.clone(),
                    });
                }
            }
            PinsState::Loaded(pins) if pins.is_empty() => {
                centred(ui, |ui| {
                    ui.label(RichText::new("📌").size(28.0));
                    ui.label(
                        RichText::new("This channel doesn't have any pinned messages… yet.")
                            .color(pal.muted),
                    );
                });
            }
            PinsState::Loaded(pins) => {
                let mut row_action = None;
                let mut confirm = self.confirm_unpin.clone();
                let unpinning = &self.unpinning;
                let mut rows = |ui: &mut egui::Ui| {
                    for pin in pins {
                        let busy = unpinning.contains(&pin.message.id);
                        if let Some(a) = pin_row(ui, pin, view, busy, &mut confirm) {
                            row_action = Some(a);
                        }
                    }
                };
                if pins.len() <= 3 {
                    rows(ui);
                } else {
                    // Inside a popup the available height starts tiny, so
                    // give the scroll area an explicit height.
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(LIST_HEIGHT)
                        .max_height(LIST_HEIGHT)
                        .show(ui, rows);
                }
                self.confirm_unpin = confirm;
                if let Some(a) = row_action {
                    if let RowAction::Unpin(id) = &a {
                        self.unpinning.push(id.clone());
                        self.confirm_unpin = None;
                    }
                    action = Some(a.into_action(&self.channel_id));
                }
            }
        }
        action
    }
}

enum RowAction {
    Jump(String),
    Unpin(String),
}

impl RowAction {
    fn into_action(self, channel_id: &str) -> PinsAction {
        let channel_id = channel_id.to_string();
        match self {
            RowAction::Jump(message_id) => PinsAction::Jump {
                channel_id,
                message_id,
            },
            RowAction::Unpin(message_id) => PinsAction::Unpin {
                channel_id,
                message_id,
            },
        }
    }
}

fn centred(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(16.0);
    ui.vertical_centered(add);
    ui.add_space(16.0);
}

fn pin_row(
    ui: &mut egui::Ui,
    pin: &PinnedMessage,
    view: &PinsView,
    busy: bool,
    confirm: &mut Option<String>,
) -> Option<RowAction> {
    let pal = view.pal;
    let m = &pin.message;
    let mut action = None;
    let (name, colour) = match view.author {
        Some(f) => f(&m.author),
        None => (m.author.name.clone(), None),
    };
    egui::Frame::new()
        .fill(pal.chat)
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if busy {
                ui.disable();
            }
            ui.horizontal(|ui| {
                crate::app::avatar(
                    ui,
                    &m.author.id,
                    &m.author.name,
                    &m.author.avatar_url,
                    AVATAR,
                    view.config,
                    pal,
                );
                ui.label(
                    RichText::new(name)
                        .strong()
                        .color(colour.unwrap_or(pal.text)),
                );
                let time = ui.label(
                    RichText::new(crate::timefmt::full(m.time))
                        .small()
                        .color(pal.muted),
                );
                if let Some(at) = &pin.pinned_at {
                    time.on_hover_text(format!("Pinned {}", crate::timefmt::full_iso(at)));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if view.can_unpin
                        && confirm.as_deref() != Some(m.id.as_str())
                        && ui
                            .small_button("✕")
                            .on_hover_text("Unpin message")
                            .clicked()
                    {
                        *confirm = Some(m.id.clone());
                    }
                    if ui.small_button("Jump").clicked() {
                        action = Some(RowAction::Jump(m.id.clone()));
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(&pin.preview).color(pal.text)).wrap());

            if view.can_unpin && confirm.as_deref() == Some(m.id.as_str()) {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Unpin this message?")
                            .small()
                            .color(pal.muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let unpin = egui::Button::new(RichText::new("Unpin").color(Color32::WHITE))
                            .fill(pal.danger)
                            .min_size(vec2(0.0, 0.0));
                        if ui.add(unpin).clicked() {
                            action = Some(RowAction::Unpin(m.id.clone()));
                        }
                        if ui.small_button("Cancel").clicked() {
                            *confirm = None;
                        }
                    });
                });
            }
        });
    ui.add_space(4.0);
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pin(id: &str, content: &str) -> Value {
        json!({
            "id": id,
            "channel_id": "10",
            "content": content,
            "timestamp": "2026-11-09T14:05:33.123000+00:00",
            "author": { "id": "5", "username": "leo", "global_name": "Leo", "avatar": null },
            "attachments": [],
            "embeds": [],
            "pinned": true
        })
    }

    #[test]
    fn parse_pins() {
        let mut embed_only = pin("2", "");
        embed_only["embeds"] = json!([{ "type": "rich", "title": "Rules" }]);
        let mut files = pin("3", "  ");
        files["attachments"] = json!([
            { "id": "a", "filename": "a.png", "url": "u" },
            { "id": "b", "filename": "b.png", "url": "u" }
        ]);
        let mut one_file = pin("4", "");
        one_file["attachments"] = json!([{ "id": "a", "filename": "map.zip", "url": "u" }]);
        let mut paged = pin("5", "hello\n\n  world");
        paged["pinned_at"] = json!("2026-11-01T10:00:00+00:00");

        let list = PinnedMessage::list_from_json(&[
            pin("1", "Read the rules"),
            embed_only,
            files,
            one_file,
            paged,
            json!({ "content": "no id" }),
        ]);
        assert_eq!(list.len(), 5);
        assert_eq!(list[0].preview, "Read the rules");
        assert_eq!(list[0].message.author.name, "Leo");
        assert_eq!(list[1].preview, "[Rules]");
        assert_eq!(list[2].preview, "📎 2 attachments");
        assert_eq!(list[3].preview, "📎 map.zip");
        assert_eq!(list[4].preview, "hello world");
        assert_eq!(
            list[4].pinned_at.as_deref(),
            Some("2026-11-01T10:00:00+00:00")
        );

        let long = PinnedMessage::from_json(&pin("6", &"x".repeat(1000)));
        assert_eq!(long.preview.chars().count(), PREVIEW_CHARS + 1);
    }

    #[test]
    fn popup_state() {
        let mut p = PinsPopup::new("10", "#general");
        assert!(matches!(p.state(), PinsState::Loading));
        assert!(p.pins().is_empty());

        p.set_pins(PinnedMessage::list_from_json(&[
            pin("1", "a"),
            pin("2", "b"),
        ]));
        assert_eq!(p.pins().len(), 2);

        // Pretend the user confirmed unpinning #1.
        p.confirm_unpin = Some("1".into());
        p.unpinning.push("1".into());
        assert!(p.remove("1"));
        assert!(!p.remove("1"));
        assert!(p.unpinning.is_empty() && p.confirm_unpin.is_none());
        assert_eq!(p.pins().len(), 1);

        p.unpinning.push("2".into());
        p.unpin_failed("2", "Missing Permissions");
        assert!(p.unpinning.is_empty());
        assert_eq!(p.error.as_deref(), Some("Missing Permissions"));

        // A refresh drops stale pending state.
        p.unpinning.push("9".into());
        p.confirm_unpin = Some("9".into());
        p.set_pins(vec![]);
        assert!(p.unpinning.is_empty() && p.confirm_unpin.is_none());

        p.set_failed("HTTP 403");
        assert!(matches!(p.state(), PinsState::Failed(e) if e == "HTTP 403"));
        assert!(!p.remove("2"));
    }
}
