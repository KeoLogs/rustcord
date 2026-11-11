//! Full-window settings page, laid out like Discord's: a list of sections on
//! the left, the selected section on the right, ✕ / Esc to close. Edits
//! are kept in a draft until Save Changes (see `SettingsPage`).

use eframe::egui::{self, RichText, vec2};

use crate::config::{ClientIdentity, Config, MODULES, ModuleCategory, ModuleInfo, Modules};
use crate::icons;
use crate::settings_proto::{ExplicitFilter, UserSettings};
use crate::theme::{Bold, Palette};
use crate::types::User;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Account,
    Appearance,
    Chat,
    Notifications,
    Privacy,
    Modules,
    Layout,
    Themes,
    Performance,
    About,
}

type NavItem = (Section, &'static str, &'static str);

const SECTIONS: &[(&str, &[NavItem])] = &[
    (
        "USER SETTINGS",
        &[(Section::Account, "My Account", icons::USER_CIRCLE)],
    ),
    (
        "APP SETTINGS",
        &[
            (Section::Appearance, "Appearance", icons::PALETTE),
            (Section::Chat, "Chat", icons::CHAT_CIRCLE),
            (Section::Notifications, "Notifications", icons::BELL),
            (Section::Privacy, "Privacy & Safety", icons::SHIELD),
        ],
    ),
    (
        "RUSTCORD",
        &[
            (Section::Modules, "Modules", icons::PUZZLE),
            (Section::Layout, "Layout", icons::LAYOUT),
            (Section::Themes, "Themes", icons::PALETTE),
            (Section::Performance, "Performance", icons::GAUGE),
            (Section::About, "About", icons::INFO),
        ],
    ),
];

pub enum SettingsAction {
    Close,
    EditLayout,
    ResetLayout,
    /// Copy the current layout to the clipboard as text.
    CopyLayout,
    /// Use a pasted layout (already checked by `layout_presets::import`).
    ImportLayout(Box<egui_tiles::Tree<crate::layout::Pane>>),
    /// Settings → Themes → Import: open the file / folder picker (backend).
    PickTheme {
        folder: bool,
    },
    LogOut,
    /// "Save Changes" (or Ctrl+S): the new config, plus synced settings to
    /// PATCH (user accounts). Nothing is applied or sent before this.
    Save {
        config: Box<Config>,
        sync: Vec<SyncChange>,
    },
}

/// A synced (settings-proto) setting the user changed.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncChange {
    ExplicitFilter(ExplicitFilter),
    AllowGuildDms(bool),
    Compact(bool),
}

/// Synced settings edited but not saved yet (None = unchanged).
#[derive(Default, Clone, PartialEq)]
struct SyncedDraft {
    explicit_filter: Option<ExplicitFilter>,
    allow_guild_dms: Option<bool>,
}

impl SyncedDraft {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The settings page. Edits go into `draft` (a copy of the config) and are
/// only applied when the user presses Save Changes, like Discord: a bar
/// "Careful — you have unsaved changes!" floats at the bottom meanwhile.
pub struct SettingsPage {
    pub section: Section,
    /// Edited copy of the config (theme previews live from it; zoom and
    /// everything else apply on Save).
    pub draft: Config,
    /// The config the draft started from.
    base: Config,
    synced: SyncedDraft,
    /// When the unsaved-changes bar started shaking (tried to leave).
    shake_at: Option<f64>,
    /// Settings → Layout → Import: pasted text, its checked tree or error.
    import_text: String,
    import: Option<Result<egui_tiles::Tree<crate::layout::Pane>, String>>,
    /// The themes folder was re-read since this page opened.
    themes_read: bool,
}

impl SettingsPage {
    pub fn new(config: &Config) -> Self {
        Self {
            section: Section::default(),
            draft: config.clone(),
            base: config.clone(),
            synced: SyncedDraft::default(),
            shake_at: None,
            import_text: String::new(),
            import: None,
            themes_read: false,
        }
    }

    /// Unsaved changes?
    pub fn dirty(&self) -> bool {
        self.draft != self.base || !self.synced.is_empty()
    }

    /// Throw the edits away.
    pub fn reset(&mut self) {
        self.draft = self.base.clone();
        self.synced = SyncedDraft::default();
    }

    /// Build the Save action (and treat the draft as saved).
    fn save(&mut self, synced: Option<&UserSettings>) -> SettingsAction {
        let mut sync = Vec::new();
        if synced.is_some() {
            if let Some(f) = self.synced.explicit_filter {
                sync.push(SyncChange::ExplicitFilter(f));
            }
            if let Some(on) = self.synced.allow_guild_dms {
                sync.push(SyncChange::AllowGuildDms(on));
            }
            if self.draft.compact_messages != self.base.compact_messages {
                sync.push(SyncChange::Compact(self.draft.compact_messages));
            }
        }
        self.base = self.draft.clone();
        self.synced = SyncedDraft::default();
        self.shake_at = None;
        SettingsAction::Save {
            config: Box::new(self.draft.clone()),
            sync,
        }
    }

    /// Trying to leave with unsaved changes: shake the bar instead.
    fn block_leave(&mut self, ui: &egui::Ui) -> bool {
        if self.dirty() {
            self.shake_at = Some(ui.input(|i| i.time));
            true
        } else {
            false
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &Config,
        me: Option<&User>,
        synced: Option<&UserSettings>,
        pal: &Palette,
        themes: &mut crate::themes::ThemeStore,
    ) -> Option<SettingsAction> {
        // The real config changed underneath (e.g. synced compact mode from
        // another device): follow it unless the user is editing.
        if *config != self.base {
            if self.draft == self.base {
                self.draft = config.clone();
            }
            self.base = config.clone();
        }
        let mut action = None;
        let save_key = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        if ui.input_mut(|i| i.consume_shortcut(&save_key)) && self.dirty() {
            action = Some(self.save(synced));
        }
        // Raw events: a focused widget (e.g. the slider just dragged) may
        // have consumed Escape to drop its focus.
        let esc = ui.input(|i| {
            i.raw.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            })
        });
        if esc && !self.block_leave(ui) {
            action = Some(SettingsAction::Close);
        }

        egui::Panel::left("settings_nav")
            .exact_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 12,
                        top: 60,
                        bottom: 16,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (header, items) in SECTIONS {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(pal.section_label(header));
                        });
                        ui.add_space(2.0);
                        for (section, label, icon) in *items {
                            let selected = self.section == *section;
                            if nav_item(ui, icon, label, selected, pal.text, pal).clicked() {
                                self.section = *section;
                            }
                        }
                        ui.add_space(8.0);
                        let (r, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 1.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().hline(
                            r.shrink2(vec2(10.0, 0.0)).x_range(),
                            r.center().y,
                            egui::Stroke::new(1.0, pal.divider),
                        );
                        ui.add_space(8.0);
                    }
                    if nav_item(ui, icons::SIGN_OUT, "Log Out", false, pal.danger, pal).clicked() {
                        action = Some(SettingsAction::LogOut);
                    }
                });
            });

        let central = egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.chat)
                    .inner_margin(egui::Margin {
                        left: 40,
                        right: 40,
                        top: 60,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let close = close_button(ui, pal);
                        if close.clicked() && !self.block_leave(ui) {
                            action = Some(SettingsAction::Close);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                            ui.heading(section_title(self.section));
                        });
                    });
                });
                ui.add_space(20.0);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        // At most 680 px, narrower with the window.
                        ui.set_max_width(ui.available_width().min(680.0));
                        ui.spacing_mut().item_spacing.y = 6.0;
                        if let Some(a) = self.section_ui(ui, me, synced, pal, themes) {
                            action = Some(a);
                        }
                        // Room so the unsaved-changes bar never covers the end.
                        ui.add_space(96.0);
                    });
            });

        if matches!(action, Some(SettingsAction::EditLayout)) && self.block_leave(ui) {
            action = None;
        }
        if let Some(a) = self.unsaved_bar(ui, central.response.rect, synced, pal) {
            action = Some(a);
        }
        action
    }

    /// Discord's floating "Careful — you have unsaved changes!" bar with
    /// Reset and Save Changes. Slides up when there are changes; shakes and
    /// turns red when the user tries to leave without saving.
    fn unsaved_bar(
        &mut self,
        ui: &egui::Ui,
        area: egui::Rect,
        synced: Option<&UserSettings>,
        pal: &Palette,
    ) -> Option<SettingsAction> {
        let ctx = ui.ctx().clone();
        let dirty = self.dirty();
        let t = ctx.animate_bool_with_time(egui::Id::unique("rc_unsaved_bar"), dirty, 0.15);
        if t <= 0.0 {
            self.shake_at = None;
            return None;
        }
        let now = ui.input(|i| i.time);
        let (dx, alarm) = match self.shake_at {
            Some(at) if now - at < 0.6 => {
                ctx.request_repaint();
                let k = (now - at) as f32;
                ((k * 45.0).sin() * 10.0 * (1.0 - k / 0.6), true)
            }
            Some(at) if now - at < 2.5 => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                (0.0, true)
            }
            _ => (0.0, false),
        };
        let w = (area.width() - 40.0).clamp(280.0, 700.0);
        let h = 52.0;
        let y = area.bottom() - (h + 20.0) * t;
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.left() + 40.0 + dx, y),
            vec2(w.min(area.width() - 60.0), h),
        );
        let mut action = None;
        egui::Area::new(egui::Id::unique("rc_unsaved_bar_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .show(&ctx, |ui| {
                let fill = if alarm { pal.danger } else { pal.menu_bg() };
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(if pal.dark { 120 } else { 50 }),
                    })
                    .show(ui, |ui| {
                        ui.set_width(rect.width() - 32.0);
                        ui.horizontal_centered(|ui| {
                            let text_col = if alarm {
                                egui::Color32::WHITE
                            } else {
                                pal.strong
                            };
                            ui.add(
                                egui::Label::new(
                                    RichText::new("Careful — you have unsaved changes!")
                                        .size(15.0)
                                        .color(text_col),
                                )
                                .truncate()
                                .selectable(false),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let save = ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Save Changes")
                                                    .color(egui::Color32::WHITE)
                                                    .bold(),
                                            )
                                            .fill(pal.positive())
                                            .corner_radius(4)
                                            .min_size(vec2(112.0, 32.0)),
                                        )
                                        .on_hover_text("Ctrl+S");
                                    if save.clicked() {
                                        action = Some(self.save(synced));
                                    }
                                    let reset = ui.add(
                                        egui::Label::new(
                                            RichText::new("Reset").size(14.0).color(text_col),
                                        )
                                        .sense(egui::Sense::click())
                                        .selectable(false),
                                    );
                                    let reset =
                                        reset.on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if reset.hovered() {
                                        let r = reset.rect;
                                        ui.painter().hline(
                                            r.x_range(),
                                            r.bottom(),
                                            egui::Stroke::new(1.0, text_col),
                                        );
                                    }
                                    if reset.clicked() {
                                        self.reset();
                                    }
                                },
                            );
                        });
                    });
            });
        action
    }

    fn section_ui(
        &mut self,
        ui: &mut egui::Ui,
        me: Option<&User>,
        synced: Option<&UserSettings>,
        pal: &Palette,
        themes: &mut crate::themes::ThemeStore,
    ) -> Option<SettingsAction> {
        let mut action = None;
        let config = &mut self.draft;
        match self.section {
            Section::Account => account(ui, me, pal),
            Section::Appearance => {
                label(ui, pal, "THEME");
                ui.horizontal_wrapped(|ui| {
                    let name = match &config.custom_theme {
                        Some(id) if config.modules.custom_themes => themes
                            .get(id)
                            .map(|t| t.meta.name.clone())
                            .unwrap_or_else(|| id.clone()),
                        _ => config.builtin_theme.name().to_owned(),
                    };
                    let is_default = (config.custom_theme.is_none()
                        || !config.modules.custom_themes)
                        && config.builtin_theme == Default::default();
                    ui.label(RichText::new(name).bold().color(pal.strong));
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(
                            !is_default,
                            egui::Button::new(icons::label(icons::RESET, "Reset to default")),
                        )
                        .on_hover_text("Back to RustCord Dark")
                        .clicked()
                    {
                        config.custom_theme = None;
                        config.builtin_theme = Default::default();
                    }
                });
                hint(
                    ui,
                    pal,
                    "Pick a built-in theme or import others in Settings → Themes.",
                );
                ui.add_space(12.0);
                label(ui, pal, "UI SCALE");
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 420.0;
                    ui.add(
                        egui::Slider::new(&mut config.zoom, 0.75..=1.75)
                            .step_by(0.05)
                            .show_value(false)
                            .trailing_fill(true)
                            .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{:.0}%", config.zoom * 100.0))
                            .bold()
                            .color(pal.strong),
                    );
                });
                hint(
                    ui,
                    pal,
                    "The new size is applied when you press Save Changes (Ctrl+S).",
                );
                ui.add_space(12.0);
                label(ui, pal, "MESSAGE DISPLAY");
                ui.radio_value(
                    &mut config.compact_messages,
                    false,
                    "Cozy: modern, beautiful, and easy on your eyes.",
                );
                ui.radio_value(
                    &mut config.compact_messages,
                    true,
                    "Compact: fit more messages on screen at one time.",
                );
                if synced.is_some() {
                    hint(ui, pal, "Synced with your Discord account.");
                }
                ui.add_space(12.0);
                label(ui, pal, "MEMBER LIST");
                switch_row(
                    ui,
                    &mut config.compact_member_list,
                    "Compact member list",
                    "Smaller rows without the custom status / activity line.",
                    pal,
                );
                switch_row(
                    ui,
                    &mut config.bot_member_intents,
                    "Bots: load the full member list",
                    "Bot accounts only. Asks Discord for the Server Members and Presence \
                     intents, which you must first switch on for your bot in the Developer \
                     Portal (Bot → Privileged Gateway Intents). Takes effect next time you \
                     log in. Without it, bots show who's recently active.",
                    pal,
                );
                ui.add_space(12.0);
                label(ui, pal, "PROFILES");
                switch_row(
                    ui,
                    &mut config.profile_themes,
                    "Show profile themes",
                    "Profile cards use people's own profile colours (Nitro profile \
                     themes) and show their avatar decorations. Off: every card uses \
                     your theme's colours.",
                    pal,
                );
                if crate::window_frame::SUPPORTED {
                    ui.add_space(12.0);
                    label(ui, pal, "WINDOW");
                    switch_row(
                        ui,
                        &mut config.system_title_bar,
                        "Use system title bar",
                        "Show your system's title bar instead of RustCord's own minimise, \
                         maximise and close buttons in the top-right panel. Applied when \
                         you save.",
                        pal,
                    );
                }
            }
            Section::Chat => chat(ui, config, pal),
            Section::Notifications => notifications(ui, config, pal),
            Section::Privacy => {
                privacy_synced(ui, synced, &mut self.synced, pal);
                ui.add_space(12.0);
                label(ui, pal, "SHOW AS");
                hint(
                    ui,
                    pal,
                    "Which app Discord thinks you're using (the icon friends see next to \
                     your status). User accounts only. Saving a change reconnects with a fresh \
                     login so Discord sees it; if the official app or a browser tab is also \
                     open, Discord may keep showing that one's icon.",
                );
                for id in ClientIdentity::ALL {
                    ui.radio_value(&mut config.client_identity, id, id.label());
                }
                ui.label(
                    RichText::new(
                        "Desktop/Mobile make RustCord identify as Discord's official app. \
                         This only changes the icon friends see, but it misrepresents the \
                         client to Discord and may slightly increase the chance of your \
                         account being flagged. Default: Web.",
                    )
                    .small()
                    .color(pal.muted),
                );
                ui.add_space(12.0);
                label(ui, pal, "THIS PC");
                switch_row(
                    ui,
                    &mut config.remember_login,
                    "Remember me on this PC",
                    "Keeps your logins in the system's credential store (Windows \
                     Credential Manager, macOS Keychain or the Secret Service on \
                     Linux), never in RustCord's settings file. Turning this off \
                     removes every saved login from this PC.",
                    pal,
                );
            }
            Section::Modules => {
                hint(
                    ui,
                    pal,
                    "Optional parts of RustCord. A module that's off doesn't run at all, \
                     which saves memory and CPU.",
                );
                ui.add_space(10.0);
                modules_grid(ui, &mut config.modules, pal);
            }
            Section::Layout => {
                hint(
                    ui,
                    pal,
                    "Rearrange RustCord's panels. In edit mode, hover a panel to highlight \
                     it, then hold and drag it to a new spot: drop on the edge of another \
                     panel to dock next to it, or in the middle to stack them as tabs. \
                     Drag the gaps between panels to resize.",
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let edit = ui.add_enabled(
                        config.modules.layout_editor,
                        pal.primary_button(&icons::label(icons::MOVE, "Edit layout")),
                    );
                    if edit.clicked() {
                        action = Some(SettingsAction::EditLayout);
                    }
                    if ui
                        .button(icons::label(icons::RESET, "Reset to default"))
                        .clicked()
                    {
                        action = Some(SettingsAction::ResetLayout);
                    }
                });
                if !config.modules.layout_editor {
                    hint(
                        ui,
                        pal,
                        "Turn on the Layout editor module to edit the layout.",
                    );
                }
                if config.modules.layout_sharing {
                    ui.add_space(16.0);
                    ui.label(pal.section_label("Share a layout"));
                    ui.add_space(6.0);
                    if ui
                        .button(icons::label(icons::COPY, "Copy layout"))
                        .on_hover_text("Copy the current layout as text to share it")
                        .clicked()
                    {
                        action = Some(SettingsAction::CopyLayout);
                    }
                    ui.add_space(6.0);
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut self.import_text)
                            .hint_text("Paste a layout here to import it")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    if edit.changed() {
                        self.import = (!self.import_text.trim().is_empty()).then(|| {
                            crate::layout_presets::import(&self.import_text)
                                .map_err(|e| format!("{e:#}"))
                        });
                    }
                    match &self.import {
                        Some(Ok(tree)) => {
                            ui.add_space(6.0);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(240.0, 150.0),
                                egui::Sense::hover(),
                            );
                            crate::layout_presets::paint_preview(ui.painter(), pal, tree, rect);
                            ui.add_space(6.0);
                            if ui
                                .add(
                                    pal.primary_button(&icons::label(
                                        icons::CHECK,
                                        "Use this layout",
                                    )),
                                )
                                .clicked()
                            {
                                action = Some(SettingsAction::ImportLayout(Box::new(tree.clone())));
                                self.import_text.clear();
                                self.import = None;
                            }
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(e).color(pal.danger));
                        }
                        None => {}
                    }
                }
            }
            Section::Themes => {
                use crate::themes_page::{ThemesAction, ThemesPage};
                // Like the other settings, a theme applies to the draft
                // (live preview) and is kept with Save Changes.
                themes.sync_with(config);
                if !std::mem::replace(&mut self.themes_read, true) {
                    themes.refresh();
                }
                match ThemesPage::show(ui, themes, pal) {
                    Some(ThemesAction::ResetDefault) => {
                        config.custom_theme = None;
                        config.builtin_theme = Default::default();
                    }
                    Some(ThemesAction::ApplyBuiltin(t)) => {
                        config.custom_theme = None;
                        config.builtin_theme = t;
                    }
                    Some(ThemesAction::Apply(id)) => config.custom_theme = Some(id),
                    Some(ThemesAction::Delete(id)) => {
                        if themes.delete(&id).is_ok() && config.custom_theme.as_ref() == Some(&id) {
                            config.custom_theme = None;
                        }
                    }
                    Some(ThemesAction::Import { folder }) => {
                        action = Some(SettingsAction::PickTheme { folder });
                    }
                    Some(ThemesAction::OpenFolder) => {
                        if let Some(dir) = themes.dir() {
                            let _ = crate::themes::open_folder(dir);
                        }
                    }
                    None => {}
                }
            }
            Section::Performance => {
                if let Some(usage) = memory_stats::memory_stats() {
                    ui.label(
                        RichText::new(format!(
                            "RustCord is using {:.0} MB of RAM",
                            usage.physical_mem as f64 / 1_048_576.0
                        ))
                        .bold(),
                    );
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs(1));
                }
                ui.add_space(10.0);
                label(ui, pal, "MESSAGES KEPT PER CHANNEL");
                ui.add(egui::Slider::new(
                    &mut config.max_messages_per_channel,
                    50..=1000,
                ));
                hint(
                    ui,
                    pal,
                    "Older messages are dropped from memory (they're still on Discord).",
                );
                ui.add_space(8.0);
                label(ui, pal, "CHANNELS KEPT IN MEMORY");
                ui.add(egui::Slider::new(&mut config.cached_channels, 1..=30));
                hint(
                    ui,
                    pal,
                    "When you've visited more channels than this, the oldest one's messages \
                     and images are freed. Lower = less RAM, more reloading.",
                );
                ui.add_space(8.0);
                hint(
                    ui,
                    pal,
                    "Tip: turning off the Images module (Modules page) saves the most memory.",
                );
            }
            Section::About => {
                ui.label(
                    RichText::new(format!("RustCord {}", env!("CARGO_PKG_VERSION")))
                        .bold()
                        .size(16.0)
                        .color(pal.strong),
                );
                ui.label("A lightweight, customisable Discord client written in Rust.");
                ui.hyperlink_to(
                    "GitHub: KeoLogs/rustcord",
                    "https://github.com/KeoLogs/rustcord",
                );
                ui.add_space(8.0);
                hint(
                    ui,
                    pal,
                    "Licensed under GPL-3.0. Feature ideas from Discord Messenger by \
                     iProgramInCpp (MIT). Emoji graphics by Twemoji (CC-BY 4.0). Fonts: \
                     Cascadia Mono by Microsoft (SIL Open Font License 1.1), Noto Sans, Noto Sans Symbols / Symbols 2 / Math / Mono by the Noto \
                     Project Authors (SIL Open Font License 1.1). Icons: Phosphor Icons \
                     (MIT).",
                );
                hint(
                    ui,
                    pal,
                    "Not affiliated with Discord. Third-party clients are against Discord's \
                     Terms of Service.",
                );
            }
        }
        action
    }
}

/// Settings → Chat (Discord Messenger's DisableFormatting, ShowEmbedContent,
/// ShowEmbedImages, Use12HourTime, ReplyMentionDefault, ShowBlockedMessages).
fn chat(ui: &mut egui::Ui, config: &mut Config, pal: &Palette) {
    label(ui, pal, "MESSAGES");
    switch_row(
        ui,
        &mut config.disable_formatting,
        "Disable formatting",
        "Show messages as typed: no bold, italics, quotes or code blocks. Mentions, \
         emoji and links still work.",
        pal,
    );
    switch_row(
        ui,
        &mut config.use_12_hour_time,
        "Use 12-hour time",
        "3:05 PM instead of 15:05.",
        pal,
    );
    switch_row(
        ui,
        &mut config.show_blocked_messages,
        "Show blocked messages",
        "Show messages from people you've blocked instead of \"1 blocked message — Show\".",
        pal,
    );
    ui.add_space(12.0);
    label(ui, pal, "EMBEDS AND LINK PREVIEWS");
    switch_row(
        ui,
        &mut config.modules.embeds,
        "Show embeds",
        "Cards for links and bot embeds under messages (same as the Embeds module).",
        pal,
    );
    ui.add_enabled_ui(config.modules.embeds, |ui| {
        switch_row(
            ui,
            &mut config.show_embed_images,
            "Show images in embeds",
            "Thumbnails and pictures in link previews. Off saves memory.",
            pal,
        );
    });
    ui.add_space(12.0);
    label(ui, pal, "REPLIES");
    switch_row(
        ui,
        &mut config.mention_on_reply,
        "Mention author when replying",
        "New replies ping the person you reply to. You can still switch it per reply \
         with the @ ON / OFF button above the message box.",
        pal,
    );
    ui.add_space(12.0);
    label(ui, pal, "LINKS");
    switch_row(
        ui,
        &mut config.warn_before_links,
        "Warn before opening links",
        "Asks before opening links from messages in your browser. Discord's own \
         domains always open straight away.",
        pal,
    );
    if !config.trusted_domains.is_empty() {
        ui.label(RichText::new("Trusted domains").small().color(pal.muted));
        let mut remove = None;
        for (i, d) in config.trusted_domains.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(d).color(pal.text));
                if ui
                    .small_button(icons::X)
                    .on_hover_text("Stop trusting")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            config.trusted_domains.remove(i);
        }
    }
}

/// Privacy & Safety settings synced with Discord (user accounts only).
/// Changes stay in `draft` until Save.
fn privacy_synced(
    ui: &mut egui::Ui,
    synced: Option<&UserSettings>,
    draft: &mut SyncedDraft,
    pal: &Palette,
) {
    let Some(s) = synced else {
        label(ui, pal, "SAFE DIRECT MESSAGING");
        hint(
            ui,
            pal,
            "These settings are stored on your Discord account. They show up here once \
             RustCord has loaded them (user accounts only).",
        );
        return;
    };
    label(ui, pal, "SAFE DIRECT MESSAGING");
    hint(
        ui,
        pal,
        "Automatically scan and delete direct messages you receive that contain \
         explicit media content.",
    );
    let current = draft.explicit_filter.or(s.explicit_content_filter);
    for (f, text) in [
        (
            ExplicitFilter::Everyone,
            "Keep me safe — scan direct messages from everyone",
        ),
        (
            ExplicitFilter::NonFriends,
            "My friends are nice — scan messages from everyone unless they're a friend",
        ),
        (
            ExplicitFilter::Off,
            "Do not scan — direct messages will not be scanned",
        ),
    ] {
        if ui.radio(current == Some(f), text).clicked() {
            draft.explicit_filter = (s.explicit_content_filter != Some(f)).then_some(f);
        }
    }
    ui.add_space(12.0);
    label(ui, pal, "SERVER PRIVACY DEFAULTS");
    let mut allow = draft
        .allow_guild_dms
        .unwrap_or(s.allow_dms_from_guild_members);
    switch_row(
        ui,
        &mut allow,
        "Allow direct messages from server members",
        "Applies to servers you join from now on. Saved to your account when you \
         press Save Changes.",
        pal,
    );
    draft.allow_guild_dms = (allow != s.allow_dms_from_guild_members).then_some(allow);
}

fn section_title(s: Section) -> &'static str {
    SECTIONS
        .iter()
        .flat_map(|(_, items)| items.iter())
        .find(|(sec, _, _)| *sec == s)
        .map(|(_, l, _)| *l)
        .unwrap_or("Settings")
}

fn account(ui: &mut egui::Ui, me: Option<&User>, pal: &Palette) {
    let Some(me) = me else {
        ui.label("Not connected yet.");
        return;
    };
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::from_uri(&me.avatar_url)
                        .fit_to_exact_size(vec2(72.0, 72.0))
                        .corner_radius(crate::theme::avatar_shape().radius(72.0)),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(&me.name).size(20.0).bold().color(pal.strong));
                    if !me.username.is_empty() {
                        ui.label(RichText::new(format!("@{}", me.username)).color(pal.muted));
                    }
                    ui.label(
                        RichText::new(if me.bot {
                            "Bot account"
                        } else {
                            "User account"
                        })
                        .small()
                        .color(pal.muted),
                    );
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(pal.section_label("User ID"));
                ui.label(&me.id);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(me.id.clone());
                }
            });
        });
}

fn notifications(ui: &mut egui::Ui, config: &mut Config, pal: &Palette) {
    let row = |ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str| {
        egui::Frame::new()
            .fill(pal.sidebar)
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        // Leave room for the switch on the right.
                        ui.set_max_width(ui.available_width() - 60.0);
                        ui.label(RichText::new(title).bold().color(pal.strong));
                        ui.label(RichText::new(desc).small().color(pal.muted));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        toggle(ui, on, pal)
                    });
                });
            });
        ui.add_space(6.0);
    };
    // Works like the rail's badges, with or without the Notifications module.
    let badge_row = |ui: &mut egui::Ui, on: &mut bool| {
        row(
            ui,
            on,
            "Show unread badge on taskbar icon",
            "A red count of your mentions and unread DMs over RustCord's taskbar icon              (9+ past nine). Muted servers and channels don't count. Windows only for now.",
        )
    };
    if !config.modules.notifications {
        hint(
            ui,
            pal,
            "The Notifications module is off, so RustCord shows no desktop notifications              and keeps no mentions inbox. Unread markers and mention badges still work.",
        );
        ui.add_space(8.0);
        if ui
            .add(pal.primary_button("Turn on Notifications module"))
            .clicked()
        {
            config.modules.notifications = true;
        }
        ui.add_space(12.0);
        badge_row(ui, &mut config.taskbar_badge);
        return;
    }
    row(
        ui,
        &mut config.desktop_notifications,
        "Enable desktop notifications",
        "Pop up a notification for mentions and DMs (and, in servers set to          \"All messages\", every message) when the window isn't focused or the          channel isn't open. Muted servers and channels never notify.",
    );
    row(
        ui,
        &mut config.flash_taskbar,
        "Flash taskbar",
        "Flash RustCord's taskbar / dock icon when a notification arrives and the          window isn't focused.",
    );
    badge_row(ui, &mut config.taskbar_badge);
    ui.add_space(6.0);
    hint(
        ui,
        pal,
        "Notification levels and mutes come from your Discord account (set them in          the official client for now). Bot accounts only notify on mentions and DMs.          Recent mentions are listed under the bell in the chat header.",
    );
}

fn label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.label(pal.section_label(text));
}

/// Discord's settings row: title and switch on one line, a muted
/// description under it, then a divider.
fn switch_row(ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str, pal: &Palette) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(15.0).color(pal.strong));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            toggle(ui, on, pal)
        });
    });
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width() - 60.0);
        hint(ui, pal, desc);
    });
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        r.x_range(),
        r.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
    ui.add_space(10.0);
}

/// A settings sidebar entry: hover / selected backgrounds like Discord's.
fn nav_item(
    ui: &mut egui::Ui,
    icon: &str,
    text: &str,
    selected: bool,
    color: egui::Color32,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let col = if selected || resp.hovered() {
        if color == pal.text { pal.strong } else { color }
    } else if color == pal.text {
        pal.muted
    } else {
        color
    };
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        icons::font(18.0),
        col,
    );
    ui.painter().text(
        rect.left_center() + vec2(36.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(15.0),
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Discord's round ✕ with "ESC" under it.
fn close_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 54.0), egui::Sense::click());
    let c = egui::pos2(rect.center().x, rect.top() + 18.0);
    let col = if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    ui.painter()
        .circle_stroke(c, 17.0, egui::Stroke::new(2.0, col));
    ui.painter().text(
        c,
        egui::Align2::CENTER_CENTER,
        icons::X,
        icons::font(18.0),
        col,
    );
    ui.painter().text(
        egui::pos2(c.x, rect.bottom() - 6.0),
        egui::Align2::CENTER_CENTER,
        "ESC",
        crate::theme::bold_font(11.5),
        pal.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close (Esc)")
}

fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

/// iOS/Discord-style on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off_col = egui::Color32::from_gray(if pal.dark { 0x50 } else { 0xb0 });
    let bg = lerp_color(off_col, pal.online, t);
    let bg = if resp.hovered() {
        // A touch brighter on hover, darker while pressed.
        crate::widgets::pressed_fill(&resp, lerp_color(bg, pal.strong, 0.12))
    } else {
        bg
    };
    ui.painter().rect_filled(rect, 11, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, egui::Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Modules page: a compact grid of check boxes grouped by category
// ---------------------------------------------------------------------------

/// Narrowest module column; the grid has 3 columns on a normal window,
/// 2 or 1 when narrow.
const MODULE_COL_MIN: f32 = 190.0;
const MODULE_COL_GAP: f32 = 24.0;
const MODULE_ROW_H: f32 = 28.0;

fn modules_grid(ui: &mut egui::Ui, modules: &mut Modules, pal: &Palette) {
    let width = ui.available_width();
    let cols = (((width + MODULE_COL_GAP) / (MODULE_COL_MIN + MODULE_COL_GAP)).floor() as usize)
        .clamp(1, 3);
    let col_w = (width - MODULE_COL_GAP * (cols - 1) as f32) / cols as f32;
    let groups: Vec<(ModuleCategory, Vec<&ModuleInfo>)> = ModuleCategory::ALL
        .iter()
        .map(|&c| (c, MODULES.iter().filter(|m| m.category == c).collect()))
        .filter(|(_, items): &(_, Vec<_>)| !items.is_empty())
        .collect();
    // A header weighs about two rows (label + gap).
    let sizes: Vec<usize> = groups.iter().map(|(_, items)| items.len() + 2).collect();
    let columns = split_columns(&sizes, cols);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = MODULE_COL_GAP;
        for col in &columns {
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, &g) in col.iter().enumerate() {
                    let (cat, items) = &groups[g];
                    if i > 0 {
                        ui.add_space(14.0);
                    }
                    ui.label(pal.section_label(cat.label()));
                    ui.add_space(4.0);
                    for m in items {
                        module_row(ui, m, (m.get)(modules), pal);
                    }
                }
            });
        }
    });
}

/// Split groups of `sizes` (in order) into at most `cols` columns so the
/// tallest column is as short as possible. Returns group indices per column.
fn split_columns(sizes: &[usize], cols: usize) -> Vec<Vec<usize>> {
    fn best(sizes: &[usize], start: usize, cols: usize) -> (usize, Vec<usize>) {
        // (tallest column, where each column after the first starts)
        let rest = sizes.len() - start;
        if cols <= 1 || rest <= 1 {
            return (sizes[start..].iter().sum(), Vec::new());
        }
        let mut out = (usize::MAX, Vec::new());
        let mut first = 0;
        for end in start + 1..sizes.len() {
            first += sizes[end - 1];
            let (tail, mut cuts) = best(sizes, end, cols - 1);
            let tallest = first.max(tail);
            if tallest < out.0 {
                cuts.insert(0, end);
                out = (tallest, cuts);
            }
        }
        out
    }
    if sizes.is_empty() {
        return Vec::new();
    }
    let (_, cuts) = best(sizes, 0, cols.clamp(1, sizes.len()));
    let mut bounds = vec![0];
    bounds.extend(cuts);
    bounds.push(sizes.len());
    bounds.windows(2).map(|w| (w[0]..w[1]).collect()).collect()
}

/// One module: its name on the left, a small square check box on the
/// right (accent fill with a tick = on). The whole row toggles it; the
/// description shows on hover.
fn module_row(ui: &mut egui::Ui, m: &ModuleInfo, on: &mut bool, pal: &Palette) {
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width(), MODULE_ROW_H),
        egui::Sense::click(),
    );
    if resp.clicked() {
        *on = !*on;
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *on, m.name));
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let painter = ui.painter();
        if hovered {
            painter.rect_filled(rect, 4, pal.hover);
        }
        let box_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            vec2(16.0, 16.0),
        );
        let text_w = box_rect.left() - rect.left() - 8.0 - 8.0;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = if hovered { pal.strong } else { pal.text };
        let mut job = egui::text::LayoutJob::simple_singleline(m.name.to_string(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w.max(10.0));
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        if *on {
            painter.rect_filled(box_rect, 3, pal.accent);
            let c = box_rect.center();
            painter.add(egui::Shape::line(
                vec![
                    c + vec2(-4.0, 0.2),
                    c + vec2(-1.2, 3.0),
                    c + vec2(4.2, -2.8),
                ],
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            ));
        } else {
            painter.rect_stroke(
                box_rect,
                3,
                egui::Stroke::new(1.5, if hovered { pal.text } else { pal.muted }),
                egui::StrokeKind::Inside,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(m.description);
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::split_columns;

    #[test]
    fn module_columns_balance() {
        // Messaging, Social, Servers, Appearance (rows + header).
        let sizes = [8, 5, 4, 8];
        assert_eq!(split_columns(&sizes, 3), vec![vec![0], vec![1, 2], vec![3]]);
        assert_eq!(split_columns(&sizes, 2), vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(split_columns(&sizes, 1), vec![vec![0, 1, 2, 3]]);
        // More columns than groups: one group each.
        assert_eq!(split_columns(&[3, 3], 3), vec![vec![0], vec![1]]);
        assert!(split_columns(&[], 3).is_empty());
    }
}
