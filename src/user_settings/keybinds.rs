//! Settings → Keybinds: RustCord's keyboard shortcuts.
//!
//! A static table ([`KEYBINDS`]) the integrator extends as features land
//! (flip `works` to true, or add rows with [`KeybindsPage::extra`]). Editing
//! shortcuts isn't supported yet. On macOS "Ctrl" means Cmd for shortcuts
//! that use egui's `Modifiers::COMMAND` (shown as "Cmd" there).

use eframe::egui::{self, RichText};

use super::widgets;
use super::{Action, Ctx};

#[derive(Debug, Clone, PartialEq)]
pub struct Keybind {
    pub group: &'static str,
    pub action: &'static str,
    /// Key chord as separate caps, e.g. `["Ctrl", "K"]`. Alternatives are
    /// separate rows.
    pub keys: &'static [&'static str],
    /// Implemented in RustCord today (false = planned, shown greyed).
    pub works: bool,
}

const fn kb(
    group: &'static str,
    action: &'static str,
    keys: &'static [&'static str],
    works: bool,
) -> Keybind {
    Keybind {
        group,
        action,
        keys,
        works,
    }
}

/// Shortcuts, grouped. Discord's defaults where RustCord has the feature.
pub const KEYBINDS: &[Keybind] = &[
    kb("Navigation", "Quick switcher", &["Ctrl", "K"], true),
    kb("Navigation", "Previous / next channel", &["Alt", "Up / Down"], false),
    kb("Navigation", "Previous / next unread channel", &["Alt", "Shift", "Up / Down"], false),
    kb("Navigation", "Previous / next server", &["Ctrl", "Alt", "Up / Down"], false),
    kb("Navigation", "Toggle pins", &["Ctrl", "P"], false),
    kb("Navigation", "Toggle mentions inbox", &["Ctrl", "I"], false),
    kb("Navigation", "Toggle member list", &["Ctrl", "U"], false),
    kb("Navigation", "Search", &["Ctrl", "F"], false),
    kb("Navigation", "Close popup / dialog / settings", &["Esc"], true),
    kb("Messages", "Send message", &["Enter"], true),
    kb("Messages", "New line", &["Shift", "Enter"], false),
    kb("Messages", "Accept autocomplete", &["Tab"], true),
    kb("Messages", "Move in autocomplete", &["Up / Down"], true),
    kb("Messages", "Cancel edit / reply", &["Esc"], true),
    kb("Messages", "Edit last message", &["Up"], false),
    kb("Messages", "Mark channel read", &["Esc"], false),
    kb("Messages", "Mark server read", &["Shift", "Esc"], false),
    kb("Messages", "Upload a file", &["Ctrl", "Shift", "U"], false),
    kb("Messages", "Emoji picker", &["Ctrl", "E"], false),
    kb("Profile", "Save note", &["Ctrl", "Enter"], true),
    kb("Voice", "Toggle mute", &["Ctrl", "Shift", "M"], false),
    kb("Voice", "Toggle deafen", &["Ctrl", "Shift", "D"], false),
    kb("Window", "Keyboard shortcuts", &["Ctrl", "/"], false),
];

/// Groups in first-seen order.
pub fn groups(list: &[Keybind]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for k in list {
        if !out.contains(&k.group) {
            out.push(k.group);
        }
    }
    out
}

/// Key label for this OS ("Ctrl" → "Cmd" on macOS).
pub fn key_label(key: &str) -> &str {
    if cfg!(target_os = "macos") && key == "Ctrl" {
        "Cmd"
    } else {
        key
    }
}

#[derive(Default)]
pub struct KeybindsPage {
    /// Extra rows from the integrator / other modules.
    pub extra: Vec<Keybind>,
    /// Hide planned (not yet working) shortcuts.
    pub only_working: bool,
}

impl KeybindsPage {
    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        widgets::hint(
            ui,
            pal,
            "RustCord's keyboard shortcuts. Greyed-out ones are planned and don't work yet. \
             Changing shortcuts isn't supported yet.",
        );
        ui.checkbox(&mut self.only_working, "Only show shortcuts that work now");
        ui.add_space(8.0);
        let all: Vec<Keybind> = KEYBINDS.iter().cloned().chain(self.extra.iter().cloned()).collect();
        for group in groups(&all) {
            widgets::label(ui, pal, group);
            for k in all
                .iter()
                .filter(|k| k.group == group && (k.works || !self.only_working))
            {
                ui.horizontal(|ui| {
                    ui.set_min_height(28.0);
                    let col = if k.works { pal.text } else { pal.muted };
                    ui.label(RichText::new(k.action).size(14.0).color(col));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !k.works {
                            ui.label(RichText::new("planned").size(11.5).color(pal.muted));
                        }
                        for (i, key) in k.keys.iter().enumerate().rev() {
                            widgets::key_cap(ui, pal, key_label(key));
                            if i > 0 {
                                ui.label(RichText::new("+").color(pal.muted));
                            }
                        }
                    });
                });
            }
            ui.add_space(6.0);
            widgets::divider(ui, pal);
            ui.add_space(6.0);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sane() {
        assert!(KEYBINDS.iter().all(|k| !k.keys.is_empty() && !k.action.is_empty()));
        assert!(KEYBINDS.iter().any(|k| k.works && k.keys == ["Ctrl", "K"]));
        let g = groups(KEYBINDS);
        assert_eq!(g[0], "Navigation");
        assert_eq!(g.len(), 5);
        // No duplicate action within a group.
        for (i, a) in KEYBINDS.iter().enumerate() {
            for b in &KEYBINDS[i + 1..] {
                assert!(!(a.group == b.group && a.action == b.action), "{}", a.action);
            }
        }
    }
}
