//! The RustCord UI: login screen, main layout, settings page.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, vec2};
use tokio::sync::mpsc::UnboundedSender;

use crate::autocomplete::{self, Autocomplete, Candidate, Trigger};
use crate::config::{CONFIG_KEY, Config};
use crate::embed;
use crate::emoji_picker::{self, CustomEmojiGroup, EmojiRef, Picked, PickerCtx, RecentEmoji};
use crate::forum::ForumView;
use crate::friends::{
    FriendsAction, FriendsCtx, FriendsPage, MessageRequestsPage, Outcome, RelationshipStore,
    RequestAction, RequestStore, RequestsCtx,
};
use crate::gif_picker::{GifAction, GifPicker};
use crate::home::{self, HomeAction, HomeCtx, HomeSelected, HomeState};
use crate::invites::{self, AddServerAction, AddServerDialog, CardCtx, InviteAction, InviteCache};
use crate::layout::{self, LAYOUT_KEY, LayoutBehavior, Pane};
use crate::links::{self, LinkCheck, LinkDecision, LinkWarning};
use crate::login::{LoginAction, LoginForm};
use crate::markdown::{self, Block, MarkdownAction, ParseCache};
use crate::members::{self, Member, MemberCache, MemberList, Row};
use crate::notify::{self, Inbox, InboxItem, Mutes, NotifyCheck};
use crate::onboarding::{ChannelsRoles, OnboardingAction, OnboardingFlow};
use crate::perms;
use crate::pins::{PinnedMessage, PinsAction, PinsPopup, PinsView};
use crate::presence::{Presences, Status};
use crate::profile::{self, Popout, PopoutAction, ProfileCache};
use crate::quick_switcher::{self, QuickSwitcher};
use crate::reactions::{self, PendingReactions, ReactionAction, ReactionCtx};
use crate::read_state::ReadState;
use crate::screening::{ScreeningAction, ScreeningDialog};
use crate::search::{SearchAction, SearchCtx, SearchPanel, SearchScope, Suggestion};
use crate::settings::{SettingsAction, SettingsPage, SyncChange};
use crate::settings_proto::{self as sp, RailItem, UserSettings};
use crate::status::{self, CustomStatusDialog, DialogResult};
use crate::theme::{self, Bold, Palette, ThemeChoice};
use crate::threads::{self, DialogOutcome, Thread, ThreadAction, ThreadDialog, ThreadStore};
use crate::timefmt;
use crate::types::{Attachment, CHANNEL_DM, DM_GUILD_ID, Guild, Message, ToBackend, ToUi, User};
use crate::widgets::{self, Tone, icon_button};
use crate::{icons, sysmsg};

const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
const TYPING_RESEND: Duration = Duration::from_secs(8);
const IMAGE_MAX: (f32, f32) = (400.0, 300.0);
/// Discord's default upload limit for accounts without Nitro.
const UPLOAD_LIMIT: u64 = 10 * 1024 * 1024;
/// Acks for messages arriving in the open channel are sent at most this often.
const ACK_INTERVAL: Duration = Duration::from_secs(3);
/// Shown when a form is submitted while a rate-limit wait is running.
const SLOW_DOWN_RETRY: &str = "Slow down: try again in a few seconds.";

// ===========================================================================
// App shell
// ===========================================================================

enum Screen {
    Login(LoginForm),
    Main(Box<State>),
}

pub struct RustCordApp {
    config: Config,
    screen: Screen,
    tree: egui_tiles::Tree<Pane>,
    editing_layout: bool,
    settings: Option<SettingsPage>,
    /// Zoom currently applied (with the active theme's visuals), to
    /// re-apply only when it or the theme changes.
    applied: Option<f32>,
    /// Installed custom themes (nothing read while the module is off).
    theme_store: crate::themes::ThemeStore,
    /// The theme the UI draws with.
    active_theme: crate::themes::ActiveTheme,
    /// (built-in, custom id, module on) the active theme was built for.
    theme_key: Option<(ThemeChoice, Option<String>, bool)>,
    /// Status bar module's RAM reading.
    ram: RamSampler,
    /// The running session's token and demo flag (in memory only), to
    /// reconnect with a fresh IDENTIFY when "Show as" changes.
    session: Option<(String, bool)>,
    /// Demo mode only (`RUSTCORD_DEMO_OPEN=view`): a view to open once the
    /// demo data is in, for screenshots.
    demo_open: Option<String>,
    /// A `RUSTCORD_DEMO_OPEN` run: nothing is saved (the view may change
    /// the config, e.g. a theme, only for the screenshot).
    demo_shot: bool,
    /// Demo mode only (`RUSTCORD_DEMO_SHOT=file.png`): save a screenshot of
    /// the window after a few seconds, then quit. Works while the desktop
    /// is locked, unlike capturing the window from outside.
    demo_shot_path: Option<String>,
    demo_shot_asked: bool,
}

impl RustCordApp {
    pub fn new(cc: &eframe::CreationContext<'_>, env_token: Option<String>, demo: bool) -> Self {
        let config: Config = cc
            .storage
            .and_then(|s| eframe::get_value(s, CONFIG_KEY))
            .unwrap_or_default();
        let mut tree = cc
            .storage
            .and_then(|s| eframe::get_value::<egui_tiles::Tree<Pane>>(s, LAYOUT_KEY))
            .filter(layout::is_complete)
            // A saved layout must be a well-formed tree with every panel once.
            .and_then(|t| crate::layout_presets::sanitize(&t).ok())
            .unwrap_or_else(layout::default_tree);
        if demo && std::env::var("RUSTCORD_DEMO_LAYOUT").as_deref() == Ok("alt") {
            tree = layout::alt_tree();
        }

        // Demo screenshots (`RUSTCORD_DEMO_OPEN` / `_SHOT`) use a scratch
        // themes folder so they never touch the user's installed themes.
        let demo_shot = demo
            && (std::env::var("RUSTCORD_DEMO_OPEN").is_ok()
                || std::env::var("RUSTCORD_DEMO_SHOT").is_ok());
        let themes_dir = if demo_shot {
            Some(std::env::temp_dir().join("rustcord-demo-themes"))
        } else {
            crate::themes::default_dir(demo)
        };
        let theme_store = crate::themes::ThemeStore::new(themes_dir, config.modules.custom_themes);
        let active_theme = crate::themes::ActiveTheme::builtin(config.theme);
        let mut app = Self {
            screen: Screen::Login(LoginForm::new(config.remember_login, None)),
            theme_store,
            active_theme,
            theme_key: None,
            config,
            tree,
            editing_layout: false,
            settings: None,
            applied: None,
            ram: RamSampler::default(),
            session: None,
            demo_open: demo
                .then(|| std::env::var("RUSTCORD_DEMO_OPEN").ok())
                .flatten(),
            demo_shot,
            demo_shot_path: demo
                .then(|| std::env::var("RUSTCORD_DEMO_SHOT").ok())
                .flatten(),
            demo_shot_asked: false,
        };

        // Log in straight away if we can: demo, DISCORD_TOKEN, or a saved token.
        let saved = app
            .config
            .remember_login
            .then(|| app.config.saved_token.clone())
            .flatten();
        if demo {
            app.start_session(&cc.egui_ctx, String::new(), true);
        } else if let Some(token) = env_token.or(saved) {
            app.start_session(&cc.egui_ctx, token, false);
        }
        app
    }

    fn start_session(&mut self, ctx: &egui::Context, token: String, demo: bool) {
        crate::config::live::sync(&self.config.modules);
        let (ui_tx, ui_rx) = mpsc::channel();
        let (be_tx, be_rx) = tokio::sync::mpsc::unbounded_channel();
        let options = crate::backend::Options {
            bot_member_intents: self.config.bot_member_intents,
            identity: self.config.client_identity,
        };
        self.session = Some((token.clone(), demo));
        crate::backend::spawn(token, demo, options, ui_tx, be_rx, ctx.clone());
        self.screen = Screen::Main(Box::new(State::new(ui_rx, be_tx)));
    }

    /// Demo mode only: open the `RUSTCORD_DEMO_OPEN` view (`view` or
    /// `view:arg`) once the demo data is in, for screenshots.
    fn run_demo_open(&mut self, ctx: &egui::Context) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if self.demo_open.is_none() || state.guilds.is_empty() || ctx.input(|i| i.time) < 1.0 {
            return;
        }
        let what = self.demo_open.take().unwrap_or_default();
        let (view, arg) = what.split_once(':').unwrap_or((what.as_str(), ""));
        let section = match view {
            "modules" => Some(crate::settings::Section::Modules),
            "themes" => Some(crate::settings::Section::Themes),
            "layout" => Some(crate::settings::Section::Layout),
            _ => None,
        };
        let theme = (arg == "frosted")
            .then(|| {
                if self.theme_store.get("frosted-glass").is_some() {
                    return Some("frosted-glass".to_string());
                }
                self.theme_store
                    .import(std::path::Path::new("examples/themes/frosted-glass"))
                    .ok()
            })
            .flatten();
        if let Some(section) = section {
            let mut page = SettingsPage::new(&self.config);
            page.section = section;
            if theme.is_some() {
                page.draft.custom_theme = theme;
            }
            self.settings = Some(page);
        } else {
            if theme.is_some() {
                // Not saved: `demo_shot` turns saving off.
                self.config.custom_theme = theme;
            }
            if !state.demo_open(view, ctx, &self.config) {
                // Not ready yet (e.g. history still loading): next frame.
                self.demo_open = Some(what);
            }
        }
    }

    /// Demo mode only: `RUSTCORD_DEMO_SHOT` screenshot (and
    /// `RUSTCORD_DEMO_SIZE=WxH` window size), then close the window.
    fn run_demo_shot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.demo_shot_path.clone() else {
            return;
        };
        let t = ctx.input(|i| i.time);
        ctx.request_repaint();
        if !self.demo_shot_asked && t < 0.5 {
            if let Some((w, h)) = std::env::var("RUSTCORD_DEMO_SIZE").ok().and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
            }) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
            }
            return;
        }
        if !self.demo_shot_asked && t > 4.5 {
            self.demo_shot_asked = true;
            let c = ctx.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    let _ = buf.save(&path);
                }
                c.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }

    /// Back to the login screen. Dropping the State drops the backend's
    /// channel, which shuts the backend thread down.
    fn log_out(&mut self, error: Option<String>, forget_token: bool) {
        if forget_token {
            self.config.saved_token = None;
        }
        self.settings = None;
        self.editing_layout = false;
        self.session = None;
        self.screen = Screen::Login(LoginForm::new(self.config.remember_login, error));
    }

    /// Apply theme + zoom when they change. While Settings is open the
    /// theme previews from the draft; zoom only changes on Save (applying it
    /// live made the slider jump under the cursor).
    fn apply_theme(&mut self, ctx: &egui::Context) {
        // Settings previews the draft's theme live.
        let cfg = self.settings.as_ref().map_or(&self.config, |p| &p.draft);
        let custom = cfg
            .custom_theme
            .clone()
            .filter(|_| cfg.modules.custom_themes);
        let key = (cfg.theme, custom, cfg.modules.custom_themes);
        if self.theme_key.as_ref() != Some(&key) {
            self.theme_store.set_enabled(cfg.modules.custom_themes);
            let (active, error) =
                crate::themes::ActiveTheme::from_config(ctx, cfg, &self.theme_store);
            if let (Some(e), Screen::Main(state)) = (error, &mut self.screen) {
                state.toast = Some((format!("Theme not loaded: {e}"), Instant::now()));
            }
            // Dropping the old theme frees its background texture.
            self.active_theme = active;
            self.theme_key = Some(key);
            self.applied = None;
        }
        let want = self.config.zoom;
        if self.applied != Some(want) {
            self.active_theme.apply(ctx);
            ctx.set_zoom_factor(want);
            self.applied = Some(want);
        }
    }

    /// Thin bar across the top, only while editing the layout or while not
    /// connected (like Discord's "Connecting…" notice; the Status bar module
    /// shows the connection state instead).
    fn top_bar(&mut self, ui: &mut egui::Ui, pal: &Palette, status: &str, connected: bool) {
        let notice = !connected && !self.config.modules.status_bar;
        if !self.editing_layout && !notice {
            return;
        }
        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::new()
                    .fill(if self.editing_layout {
                        pal.servers
                    } else {
                        pal.accent
                    })
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.editing_layout {
                        ui.label(
                            RichText::new(icons::label(icons::MOVE, "Editing layout"))
                                .bold()
                                .color(pal.accent),
                        );
                        ui.label(
                            RichText::new(
                                "hover a panel, hold and drag to move · drag the gaps to resize",
                            )
                            .small()
                            .color(pal.muted),
                        );
                        if ui.button(icons::label(icons::RESET, "Reset")).clicked() {
                            self.tree = layout::default_tree();
                        }
                        if ui
                            .add(pal.primary_button(&icons::label(icons::CHECK, "Done")))
                            .clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.editing_layout = false;
                        }
                    }
                    if notice {
                        ui.with_layout(
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                ui.label(
                                    RichText::new(status)
                                        .size(13.0)
                                        .bold()
                                        .color(Color32::WHITE),
                                );
                            },
                        );
                    }
                });
            });
    }
}

/// Apply config changes the chat UI asked for (trusted link domains, the
/// synced compact setting).
fn apply_config_updates(config: &mut Config, state: &mut State) {
    for u in state.config_updates.drain(..) {
        match u {
            ConfigUpdate::TrustDomain(d) => {
                if !config.trusted_domains.contains(&d) {
                    config.trusted_domains.push(d);
                }
            }
            ConfigUpdate::Compact(c) => config.compact_messages = c,
            ConfigUpdate::RecentEmoji(e) => config.recent_emoji.record(&e),
            ConfigUpdate::SkinTone(t) => config.skin_tone = t.min(5),
        }
    }
}

/// RAM use for the status bar, read at most every 2 seconds.
#[derive(Default)]
struct RamSampler {
    last: Option<(Instant, u64)>,
}

impl RamSampler {
    fn sample(&mut self) -> Option<u64> {
        let stale = self
            .last
            .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(2));
        if stale {
            self.last =
                memory_stats::memory_stats().map(|m| (Instant::now(), m.physical_mem as u64));
        }
        self.last.map(|(_, b)| b)
    }
}

impl eframe::App for RustCordApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        // Module flags for the gateway task, then module state on / off.
        crate::config::live::sync(&self.config.modules);
        state.set_reactions_module(self.config.modules.reactions);
        state.set_friends_module(self.config.modules.friends);
        state.set_threads_module(self.config.modules.threads);
        state.set_invites_module(self.config.modules.invites);
        if !self.config.modules.search {
            state.search = None;
        }
        if let Some(rs) = &mut state.reactions {
            rs.pending
                .prune(ctx.input(|i| i.time), reactions::PENDING_MAX_AGE);
        }
        state.drain_events(ctx, &self.config);
        apply_config_updates(&mut self.config, state);
        if let Some(path) = state.picked_theme.take() {
            // Small file copies; the picker itself ran in the backend.
            let _ = self.theme_store.import(&path);
        }
        state.ack_open_channel(ctx, self.settings.is_none());
        if !self.config.modules.notifications && !state.inbox.items.is_empty() {
            // Module switched off: free the inbox.
            state.inbox = Inbox::default();
        }
        if !self.config.modules.member_list {
            // Module switched off: free the member list and presences.
            state.free_member_list(ctx);
        }

        // Files dropped onto the window go into the upload list.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            state.add_files(dropped);
        }

        if self.config.modules.typing_indicators && state.expire_typing() {
            // Keep ticking so "is typing…" disappears on time.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        timefmt::set_12_hour(self.config.use_12_hour_time);
        let pal = self.active_theme.palette;

        // ---- Login screen
        if let Screen::Login(form) = &mut self.screen {
            match form.ui(ui, &pal) {
                Some(LoginAction::Login { token, remember }) => {
                    self.config.remember_login = remember;
                    self.config.saved_token = remember.then(|| token.clone());
                    self.start_session(&ctx, token, false);
                }
                Some(LoginAction::Demo) => self.start_session(&ctx, String::new(), true),
                None => {}
            }
            return;
        }

        // ---- Main screen
        let (status, connected, fatal, wants_settings) = match &mut self.screen {
            Screen::Main(state) => (
                state.status.clone(),
                state.connected,
                state.fatal.take(),
                std::mem::take(&mut state.open_settings),
            ),
            Screen::Login(_) => unreachable!(),
        };
        if let Some(reason) = fatal {
            // Bad token etc.: back to login and forget the saved token.
            self.log_out(Some(reason), true);
            return;
        }
        self.run_demo_open(&ctx);
        self.run_demo_shot(&ctx);
        if wants_settings {
            self.settings = Some(SettingsPage::new(&self.config));
            self.editing_layout = false;
        }

        // Settings page replaces the main UI while open.
        if let Some(page) = &mut self.settings {
            let Screen::Main(state) = &mut self.screen else {
                return;
            };
            page.layout_preset = crate::layout_presets::detect(&self.tree);
            let action = page.ui(
                ui,
                &self.config,
                state.me.as_ref(),
                state.user_settings(),
                &pal,
                &mut self.theme_store,
            );
            match action {
                Some(SettingsAction::Save { config, sync }) => {
                    let reconnect = config.client_identity != self.config.client_identity;
                    self.config = *config;
                    for change in sync {
                        state.apply_sync_change(change);
                    }
                    // "Show as" only changes with a fresh IDENTIFY: start a
                    // new session (a new backend never RESUMEs the old one).
                    if reconnect
                        && let Some((token, demo)) = self.session.clone()
                        && !demo
                        && !state.is_bot_session()
                    {
                        self.start_session(&ctx, token, false);
                        if let Screen::Main(state) = &mut self.screen {
                            state.status = "Reconnecting to apply \"Show as\"…".into();
                        }
                    }
                }
                Some(SettingsAction::Close) => self.settings = None,
                Some(SettingsAction::EditLayout) => {
                    self.settings = None;
                    self.editing_layout = true;
                }
                Some(SettingsAction::ResetLayout) => self.tree = layout::default_tree(),
                Some(SettingsAction::ApplyPreset(p)) => {
                    self.tree = crate::layout_presets::build(p);
                    state.toast = Some((format!("Layout: {}", p.name()), Instant::now()));
                }
                Some(SettingsAction::CopyLayout) => {
                    ctx.copy_text(crate::layout_presets::export(&self.tree));
                }
                Some(SettingsAction::ImportLayout(tree)) => self.tree = *tree,
                Some(SettingsAction::PickTheme { folder }) => {
                    let _ = state.tx.send(ToBackend::PickTheme { folder });
                }
                Some(SettingsAction::LogOut) => self.log_out(None, true),
                None => {}
            }
            return;
        }

        if !self.config.modules.layout_editor {
            self.editing_layout = false;
        }
        self.top_bar(ui, &pal, &status, connected);

        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if self.config.modules.status_bar {
            let ram = self.ram.sample();
            state.status_bar(ui, &self.config, &pal, ram);
        }
        // Ctrl+K (Cmd+K on macOS): quick switcher.
        let quick = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K);
        if ctx.input_mut(|i| i.consume_shortcut(&quick)) {
            state.toggle_switcher();
        }
        // Ctrl+F: search this channel; Ctrl+Shift+F: the whole server / DM.
        if self.config.modules.search {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            let (whole, here) = ctx.input_mut(|i| {
                (
                    i.consume_shortcut(&K::new(M::COMMAND | M::SHIFT, Key::F)),
                    i.consume_shortcut(&K::new(M::COMMAND, Key::F)),
                )
            });
            if whole || here {
                state.open_search(whole);
            }
        }
        state.shortcuts(&ctx, &self.config);
        state.members_visible = layout::pane_visible(&self.tree, Pane::Members);
        state.channels_visible = layout::pane_visible(&self.tree, Pane::Channels);
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut behavior = LayoutBehavior {
                state,
                config: &self.config,
                pal,
                theme: &self.active_theme,
                editing: self.editing_layout,
            };
            // Theme backdrop behind inset / glass panes and the gaps.
            self.active_theme
                .paint_backdrop(ui.painter(), ui.max_rect());
            self.tree.ui(&mut behavior, ui);
        });
        layout::end_frame(&self.tree, &ctx);
        if std::mem::take(&mut state.toggle_members) {
            layout::toggle_pane(&mut self.tree, Pane::Members);
        }
        if std::mem::take(&mut state.toggle_channels) {
            layout::toggle_pane(&mut self.tree, Pane::Channels);
        }

        state.overlays(ui, &self.config, &pal);
        apply_config_updates(&mut self.config, state);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.demo_shot {
            return;
        }
        if !self.config.remember_login {
            self.config.saved_token = None;
        }
        eframe::set_value(storage, CONFIG_KEY, &self.config);
        eframe::set_value(storage, LAYOUT_KEY, &self.tree);
    }
}

// ===========================================================================
// Chat state
// ===========================================================================

/// Buttons in the message box that open pickers (the emoji picker is a
/// popup on its button, see `composer_pane`).
pub enum ComposerAction {
    ToggleGifPicker(egui::Rect),
}

/// What the message box is currently doing.
enum Compose {
    New,
    Reply {
        message_id: String,
        author: String,
        /// Ping the author (allowed_mentions `replied_user`).
        ping: bool,
    },
    Edit {
        message_id: String,
    },
}

/// Something the user clicked on a message; applied after drawing.
enum MessageAction {
    Reply(String, String),
    Edit(String, String),
    Delete(String),
    Copy(String),
    MarkUnread(String),
    /// Clicked an avatar or name: open their profile next to this rect.
    Profile(User, egui::Rect),
    /// A link in a message / embed / attachment (goes through the warning).
    OpenLink(String),
    /// Clicked a `<@user>` mention.
    Mention(String, egui::Rect),
    /// Clicked a `<#channel>` mention.
    GoToChannel(String),
    /// Pin (`true`) / unpin: asks for confirmation first.
    Pin(String, bool),
    /// Save an image attachment: (url, file name).
    SaveImage(String, String),
    /// Reveal a run of blocked messages (id of the first one).
    ShowBlocked(String),
    /// Clicked a reply preview: scroll to the original message.
    JumpTo(String),
    /// "Create Thread" from a message: (message id, content).
    CreateThread(String, String),
    /// An invite card under a message.
    Invite(InviteAction),
    /// Reaction bar / add-reaction picker on message (id).
    React(String, ReactionAction),
}

/// Changes to the saved config that come from the chat UI; the app applies
/// them (State only gets `&Config`).
pub enum ConfigUpdate {
    TrustDomain(String),
    /// Synced compact/cozy setting arrived from Discord.
    Compact(bool),
    /// Picked an emoji: move it to the front of the recent list.
    RecentEmoji(EmojiRef),
    /// Changed the emoji picker's skin tone.
    SkinTone(u8),
}

// --- reactions ---
/// Reactions module state (`None` in `State` while the module is off).
#[derive(Default)]
struct ReactionState {
    pending: PendingReactions,
    /// Who reacted, per (message, emoji, burst), for pill tooltips.
    reactors: HashMap<(String, EmojiRef, bool), Vec<String>>,
    /// Reactor lists already requested (each is asked for once).
    asked: HashSet<(String, EmojiRef, bool)>,
}

// --- onboarding ---
/// Invites / screening / onboarding module state (`None` while off).
#[derive(Default)]
struct InvitesState {
    cache: InviteCache,
    add_server: Option<AddServerDialog>,
    screening: Option<ScreeningDialog>,
    onboarding: Option<OnboardingFlow>,
    channels_roles: Option<ChannelsRoles>,
    /// Just joined this server and it has rules screening: open it once
    /// the onboarding questions are done (onboarding -> screening).
    screening_next: Option<String>,
}

/// What a message row needs to draw invite cards.
struct InviteParts<'a> {
    cache: &'a InviteCache,
    is_member: &'a dyn Fn(&str) -> bool,
    can_join: bool,
    my_id: &'a str,
}

// --- threads ---
/// Threads module state (`None` in `State` while the module is off).
#[derive(Default)]
struct ThreadsState {
    store: ThreadStore,
    forum_view: Option<ForumView>,
    dialog: Option<ThreadDialog>,
    /// Thread browser popout: (channel id, header button rect).
    browser: Option<(String, egui::Rect)>,
    /// Servers whose active-thread list answered 403/404: READY /
    /// GUILD_CREATE data only, "Not available here", never asked again.
    unavailable: HashSet<String>,
    /// Servers / channels already asked (once per session; no polling).
    asked_active: HashSet<String>,
    asked_archived: HashSet<String>,
    /// More archived threads to load, per channel.
    archived_more: HashMap<String, bool>,
}

// --- friends ---
/// Friends module state (`None` in `State` while the module is off).
#[derive(Default)]
struct FriendsState {
    rels: RelationshipStore,
    requests: RequestStore,
    /// Home header search text.
    home: HomeState,
    /// Friends / Message Requests page shown instead of the chat.
    page: HomeSelected,
    friends_page: FriendsPage,
    requests_page: MessageRequestsPage,
}

// --- search ---
/// The Ctrl+F search popup (`None` in `State` while closed or the module
/// is off).
struct SearchState {
    panel: SearchPanel,
    scope: SearchScope,
    /// Header search button (the popup hangs below it).
    anchor: egui::Rect,
}

/// What a message row needs to draw reactions.
struct ReactParts<'a> {
    images: bool,
    /// Add Reactions permission here and not rate limited.
    can_react: bool,
    custom: &'a [CustomEmojiGroup],
    recent: &'a RecentEmoji,
    skin_tone: &'a Cell<u8>,
    state: &'a ReactionState,
}

/// Context for a message's menus.
struct MenuCtx {
    mine: bool,
    /// Server id for "Copy message link" (None in DMs).
    guild_id: Option<String>,
    /// May pin / unpin here (DM, or Manage Messages).
    can_pin: bool,
    /// Threads module on and Create Public Threads here.
    can_thread: bool,
}

/// An op 14 subscription: (guild, channel, ranges).
type Subscription = (String, String, Vec<(u32, u32)>);

/// A bot's whole-server member list (op 8 with the Server Members intent),
/// grouped locally since bots can't use op 14.
struct BotList {
    guild_id: String,
    members: Vec<Member>,
    list: MemberList,
    dirty: bool,
}

struct Typer {
    user_id: String,
    name: String,
    since: Instant,
}

/// Why a channel's messages aren't shown (drawn as the chat column's empty
/// state instead of a spinner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// No VIEW_CHANNEL (from roles + overwrites), or Discord said 403.
    NoAccess,
    /// Loading failed; user-facing text, never a raw HTTP line.
    Failed(String),
}

pub struct State {
    rx: Receiver<ToUi>,
    tx: UnboundedSender<ToBackend>,
    /// Channels whose messages couldn't be loaded (or that I can't view).
    channel_errors: HashMap<String, ChannelError>,

    status: String,
    connected: bool,
    /// Set when the session can't continue (bad token); the app logs out.
    fatal: Option<String>,
    /// Set by the ⚙ button; the app opens settings.
    open_settings: bool,

    me: Option<User>,
    guilds: Vec<Guild>,
    dms: Guild,
    selected_guild: Option<String>,
    selected_channel: Option<String>,

    /// channel_id -> messages (oldest first)
    messages: HashMap<String, Vec<Message>>,
    /// Most recently viewed channels first; used to free old channels.
    recent_channels: VecDeque<String>,
    /// channel_id -> people typing there
    typing: HashMap<String, Vec<Typer>>,

    draft: String,
    compose: Compose,
    pending_files: Vec<PathBuf>,
    /// Pending files to upload as spoilers (`SPOILER_` prefix).
    pending_spoilers: HashSet<PathBuf>,
    /// Ctrl+/ keyboard shortcuts dialog open.
    show_shortcuts: bool,
    /// GIF picker popup (GIF picker module) and its button's rect.
    gif_picker: Option<(GifPicker, egui::Rect)>,
    /// Emoji button's rect while the emoji picker is open (placeholder until
    /// the real picker lands).
    /// The composer's emoji picker popup was open last frame.
    emoji_picker_open: bool,
    /// Demo mode only: open the composer's emoji picker once.
    demo_picker: bool,
    /// Custom emoji offered by the pickers: (server id, groups), rebuilt
    /// when the server changes or its emoji do.
    emoji_groups: Option<(String, Vec<CustomEmojiGroup>)>,
    /// Reactions module state; `None` while the module is off.
    reactions: Option<ReactionState>,
    /// Search popup; `None` while closed or the module is off.
    search: Option<SearchState>,
    /// Friends module state; `None` while the module is off.
    friends: Option<FriendsState>,
    /// Threads module state; `None` while the module is off.
    threads: Option<ThreadsState>,
    /// Invites / onboarding module state; `None` while the module is off.
    invites: Option<InvitesState>,
    /// A theme file / folder picked in Settings → Themes, for the app to
    /// import.
    picked_theme: Option<PathBuf>,
    last_typing_sent: Option<(String, Instant)>,
    confirm_delete: Option<(String, String)>,

    /// Unread / mention counts per channel.
    reads: ReadState,
    /// Mute and notification settings.
    mutes: Mutes,
    /// Recent mentions (Notifications module).
    inbox: Inbox,
    /// Channel the user marked unread: not auto-acked until opened again.
    held_unread: Option<String>,
    /// Ack for the open channel waiting to be sent (rate limited).
    ack_pending: Option<(String, u64)>,
    last_ack_at: Option<Instant>,
    /// (channel, last read message when opened): where the "NEW" line goes.
    new_marker: Option<(String, u64)>,

    /// Online status and activities (Member list module).
    presences: Presences,
    /// Nicknames and roles of people seen in servers (name colours).
    member_cache: MemberCache,
    /// "guild:user" pairs already asked for with op 8.
    asked_members: HashSet<String>,
    /// Member list of the server being viewed (user accounts, op 14).
    member_list: Option<MemberList>,
    /// What we last subscribed to with op 14: (guild, channel, ranges).
    subscribed: Option<Subscription>,
    /// Bots with the Server Members intent: the server's members.
    bot_list: Option<BotList>,
    /// Server whose whole member list a bot asked for.
    bot_list_requested: Option<String>,
    /// Loaded profiles and notes.
    profiles: ProfileCache,
    /// Open profile card.
    popout: Option<Popout>,

    /// Collapsed channel categories (this session only).
    collapsed: HashSet<String>,
    /// Is the Members panel shown (set by the app from the layout each frame).
    members_visible: bool,
    /// The members header button was clicked; the app toggles the panel.
    toggle_members: bool,
    /// Is the Channels panel shown (« in its header hides it)?
    channels_visible: bool,
    /// Header button asked to show / hide the Channels panel.
    toggle_channels: bool,
    /// Reply / Edit was picked: focus the message box next frame.
    focus_composer: bool,

    /// Parsed markdown per message (Batch C).
    md_cache: ParseCache,
    /// Display names of people seen in messages (for `<@id>` mentions and
    /// autocomplete). Capped.
    user_names: HashMap<String, String>,
    /// "Leaving RustCord" dialog for a clicked link.
    link_warning: Option<LinkWarning>,
    /// Pinned messages popup, and the pin button's rect it hangs from.
    pins: Option<(PinsPopup, egui::Rect)>,
    /// Scroll to (and flash) this message once it's on screen.
    scroll_to: Option<(String, String)>,
    highlight: Option<(String, Instant)>,
    /// Confirm pin / unpin: (channel, message, pin?).
    confirm_pin: Option<(String, String, bool)>,
    /// Confirm leaving a server: (id, name).
    confirm_leave: Option<(String, String)>,

    /// Synced settings (user accounts): status, folders, privacy, compact.
    user_settings: Option<UserSettings>,
    status_dialog: Option<CustomStatusDialog>,
    /// Open folders in the server rail.
    open_folders: HashSet<i64>,
    /// Blocked users (READY relationships type 2).
    blocked: HashSet<String>,
    /// Runs of blocked messages the user chose to show (first message id).
    revealed_blocked: HashSet<String>,

    /// Ctrl+K switcher, while open.
    switcher: Option<QuickSwitcher>,
    /// Recently opened channels (most recent first) for the switcher.
    visited: VecDeque<String>,
    /// Composer suggestions.
    ac: Autocomplete,
    /// Short notice at the bottom of the chat ("Saved …").
    toast: Option<(String, Instant)>,
    /// A write action was rate limited: user write controls stay disabled
    /// and the toast counts down until then (never auto-retried).
    slow_down_until: Option<Instant>,
    /// Config changes for the app to apply.
    pub config_updates: Vec<ConfigUpdate>,
}

impl State {
    fn new(rx: Receiver<ToUi>, tx: UnboundedSender<ToBackend>) -> Self {
        Self {
            rx,
            tx,
            status: "Connecting…".into(),
            connected: false,
            fatal: None,
            open_settings: false,
            me: None,
            guilds: Vec::new(),
            dms: Guild::dms(Vec::new()),
            selected_guild: None,
            selected_channel: None,
            messages: HashMap::new(),
            recent_channels: VecDeque::new(),
            typing: HashMap::new(),
            draft: String::new(),
            compose: Compose::New,
            pending_files: Vec::new(),
            pending_spoilers: HashSet::new(),
            show_shortcuts: false,
            gif_picker: None,
            emoji_picker_open: false,
            demo_picker: false,
            emoji_groups: None,
            reactions: None,
            search: None,
            friends: None,
            threads: None,
            invites: None,
            picked_theme: None,
            last_typing_sent: None,
            confirm_delete: None,
            reads: ReadState::default(),
            mutes: Mutes::default(),
            inbox: Inbox::default(),
            held_unread: None,
            ack_pending: None,
            last_ack_at: None,
            new_marker: None,
            presences: Presences::default(),
            member_cache: MemberCache::default(),
            asked_members: HashSet::new(),
            member_list: None,
            subscribed: None,
            bot_list: None,
            bot_list_requested: None,
            profiles: ProfileCache::default(),
            popout: None,
            collapsed: HashSet::new(),
            members_visible: true,
            toggle_members: false,
            channels_visible: true,
            toggle_channels: false,
            focus_composer: false,
            md_cache: ParseCache::default(),
            user_names: HashMap::new(),
            link_warning: None,
            pins: None,
            scroll_to: None,
            channel_errors: HashMap::new(),
            highlight: None,
            confirm_pin: None,
            confirm_leave: None,
            user_settings: None,
            status_dialog: None,
            open_folders: HashSet::new(),
            blocked: HashSet::new(),
            revealed_blocked: HashSet::new(),
            switcher: None,
            visited: VecDeque::new(),
            ac: Autocomplete::default(),
            toast: None,
            slow_down_until: None,
            config_updates: Vec::new(),
        }
    }

    // ----- events --------------------------------------------------------

    fn drain_events(&mut self, ctx: &egui::Context, config: &Config) {
        let focused = window_focused(ctx);
        while let Ok(ev) = self.rx.try_recv() {
            self.handle_event(ev, ctx, config, focused);
        }
    }

    fn handle_event(&mut self, ev: ToUi, ctx: &egui::Context, config: &Config, focused: bool) {
        match ev {
            ToUi::Status(s) => {
                if !s.starts_with("Error") {
                    self.connected = false;
                }
                self.status = s;
            }
            ToUi::Fatal(reason) => self.fatal = Some(reason),
            ToUi::Ready {
                me,
                guilds,
                dms,
                read_state,
                guild_settings,
                blocked,
                user_settings,
            } => {
                self.status = "Connected".into();
                self.connected = true;
                if let Some(ts) = &mut self.threads {
                    ts.store.set_me(&me.id);
                }
                self.blocked = blocked.into_iter().collect();
                match user_settings {
                    Some(s) => self.set_user_settings(*s),
                    // User accounts: load them over REST.
                    None if !me.bot => {
                        let _ = self.tx.send(ToBackend::LoadUserSettings);
                    }
                    None => {}
                }
                self.me = Some(me);
                self.guilds = guilds;
                self.dms = Guild::dms(dms);
                self.reads.clear();
                for c in self
                    .guilds
                    .iter()
                    .chain([&self.dms])
                    .flat_map(|g| &g.channels)
                {
                    self.reads.track(&c.id, c.last_message_id);
                }
                self.reads.load(&read_state);
                self.mutes.replace_all(guild_settings);
                // New session: Discord forgot our subscriptions; start over.
                self.free_member_list(ctx);
                self.member_cache.clear();
                self.asked_members.clear();
                if self.selected_guild.is_none() {
                    self.selected_guild = self.guilds.first().map(|g| g.id.clone());
                }
            }
            ToUi::Resumed => {
                self.status = "Connected".into();
                self.connected = true;
            }
            ToUi::MessageAck(entry) => {
                self.reads
                    .ack(&entry.channel_id, entry.last_acked, entry.mention_count);
                if self.reads.is_unread(&entry.channel_id) {
                    // Marked unread elsewhere: don't immediately re-ack it here.
                    if self.selected_channel.as_ref() == Some(&entry.channel_id) {
                        self.held_unread = Some(entry.channel_id);
                    }
                } else {
                    self.inbox.mark_channel_read(&entry.channel_id);
                }
            }
            ToUi::LastMessages(list) => {
                for (cid, last) in list {
                    self.reads.set_last(&cid, last);
                }
            }
            ToUi::GuildSettingsUpdated(settings) => self.mutes.update(settings),
            ToUi::GuildAdded(guild) => {
                for c in &guild.channels {
                    self.reads.track(&c.id, c.last_message_id);
                }
                match self.guilds.iter_mut().find(|g| g.id == guild.id) {
                    Some(existing) => *existing = guild,
                    None => self.guilds.push(guild),
                }
                if self.selected_guild.is_none() {
                    self.selected_guild = self.guilds.first().map(|g| g.id.clone());
                }
            }
            ToUi::ChannelUpserted { guild_id, channel } => {
                self.reads.set_last(&channel.id, channel.last_message_id);
                match guild_id {
                    Some(gid) => {
                        if let Some(g) = self.guilds.iter_mut().find(|g| g.id == gid) {
                            g.upsert_channel(channel);
                        }
                    }
                    None if channel.is_dm() => self.dms.upsert_channel(channel),
                    None => {}
                }
            }
            ToUi::History {
                channel_id,
                mut messages,
            } => {
                self.request_unknown_authors(&channel_id, &messages);
                self.remember_users(&mut messages, config);
                self.channel_errors.remove(&channel_id);
                self.messages.insert(channel_id, messages);
            }
            ToUi::HistoryFailed {
                channel_id,
                error,
                no_access,
            } => {
                // The status bar gets a short line at most; the chat column
                // shows the empty state.
                self.status = if no_access {
                    "No access".into()
                } else {
                    error.clone()
                };
                let e = if no_access {
                    ChannelError::NoAccess
                } else {
                    ChannelError::Failed(error)
                };
                self.channel_errors.insert(channel_id, e);
            }
            ToUi::HistoryAround {
                channel_id,
                message_id,
                mut messages,
            } => {
                self.request_unknown_authors(&channel_id, &messages);
                self.remember_users(&mut messages, config);
                if let Some(old) = self.messages.insert(channel_id.clone(), messages) {
                    forget_message_images(ctx, &old);
                }
                self.scroll_to = Some((channel_id, message_id));
            }
            ToUi::FilesPicked(paths) => self.add_files(paths),
            ToUi::Gifs { query, results } => {
                if let Some((p, _)) = &mut self.gif_picker {
                    p.set_results(query, results);
                }
            }
            ToUi::GifsFailed { error, .. } => {
                if let Some((p, _)) = &mut self.gif_picker {
                    p.set_error(error);
                }
            }
            ToUi::NewMessage(mut msg) => {
                if let Some(ts) = &mut self.threads {
                    ts.store.on_message(&msg.channel_id, &msg.id);
                }
                if let (Some(gid), Some(member)) = (&msg.guild_id, msg.member.take()) {
                    self.member_cache
                        .insert_cached(gid, &msg.author.id, *member);
                }
                if let Some(typers) = self.typing.get_mut(&msg.channel_id) {
                    typers.retain(|t| t.user_id != msg.author.id);
                }
                self.remember_users(std::slice::from_mut(&mut *msg), config);
                self.track_new_message(&msg, ctx, config, focused);
                if let Some(c) = self
                    .dms
                    .channels
                    .iter_mut()
                    .find(|c| c.id == msg.channel_id)
                {
                    c.last_message_id = msg.id.parse().unwrap_or(c.last_message_id);
                    self.dms.sort_channels();
                }
                // Only keep live messages for channels we have cached.
                if let Some(list) = self.messages.get_mut(&msg.channel_id)
                    && !list.iter().any(|m| m.id == msg.id)
                {
                    list.push(*msg);
                    let cap = config.max_messages_per_channel.max(10);
                    if list.len() > cap {
                        let removed: Vec<Message> = list.drain(..list.len() - cap).collect();
                        forget_message_images(ctx, &removed);
                    }
                }
            }
            ToUi::MessageUpdated(mut patch) => {
                if !config.modules.embeds {
                    patch.embeds = None;
                }
                if let Some(m) = self
                    .messages
                    .get_mut(&patch.channel_id)
                    .and_then(|list| list.iter_mut().find(|m| m.id == patch.id))
                {
                    m.apply_patch(&patch);
                }
            }
            ToUi::MessageDeleted {
                channel_id,
                message_id,
            } => {
                if let Some(list) = self.messages.get_mut(&channel_id) {
                    list.retain(|m| m.id != message_id);
                }
                self.md_cache.remove(&message_id);
                if let Some((p, _)) = &mut self.pins {
                    p.remove(&message_id);
                }
            }
            ToUi::Typing {
                channel_id,
                user_id,
                name,
            } => {
                if !config.modules.typing_indicators
                    || self.me.as_ref().is_some_and(|me| me.id == user_id)
                {
                    return;
                }
                let name = name.unwrap_or_else(|| self.name_for(&channel_id, &user_id));
                let typers = self.typing.entry(channel_id).or_default();
                typers.retain(|t| t.user_id != user_id);
                typers.push(Typer {
                    user_id,
                    name,
                    since: Instant::now(),
                });
            }
            ToUi::Presences(list) => {
                if !config.modules.member_list {
                    return;
                }
                for (id, p) in list {
                    self.presences.set(&id, p);
                }
                if let Some(b) = &mut self.bot_list {
                    b.dirty = true;
                }
            }
            ToUi::MemberList(upd) => {
                if !config.modules.member_list {
                    return;
                }
                for m in upd.members() {
                    self.member_cache.insert(&upd.guild_id, m);
                    if let Some(p) = &m.presence {
                        self.presences.set(&m.user.id, p.clone());
                    }
                }
                // Only the list of the server being viewed is kept.
                if self.selected_guild.as_ref() != Some(&upd.guild_id) {
                    return;
                }
                let same = self
                    .member_list
                    .as_ref()
                    .is_some_and(|l| l.guild_id == upd.guild_id && l.list_id == upd.list_id);
                let has_sync = upd
                    .ops
                    .iter()
                    .any(|op| matches!(op, members::ListOp::Sync { .. }));
                if !same {
                    if !has_sync {
                        return;
                    }
                    if let Some(old) = self.member_list.take() {
                        self.forget_list_avatars(ctx, &old);
                    }
                    self.member_list = Some(MemberList::new(&upd.guild_id, &upd.list_id));
                }
                if let Some(list) = &mut self.member_list {
                    list.apply(upd);
                }
            }
            ToUi::Members {
                guild_id,
                members,
                full_list,
            } => {
                for m in &members {
                    self.member_cache.insert(&guild_id, m);
                    if let (Some(p), true) = (&m.presence, config.modules.member_list) {
                        self.presences.set(&m.user.id, p.clone());
                    }
                }
                if full_list
                    && config.modules.member_list
                    && self.selected_guild.as_ref() == Some(&guild_id)
                {
                    let b = match &mut self.bot_list {
                        Some(b) if b.guild_id == guild_id => b,
                        _ => self.bot_list.insert(BotList {
                            guild_id,
                            members: Vec::new(),
                            list: MemberList::default(),
                            dirty: true,
                        }),
                    };
                    for m in members {
                        if b.members.len() < members::BOT_LIST_LIMIT as usize
                            && !b.members.iter().any(|o| o.user.id == m.user.id)
                        {
                            b.members.push(m);
                        }
                    }
                    b.dirty = true;
                }
            }
            ToUi::RoleUpserted { guild_id, role } => {
                if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
                    g.roles.retain(|r| r.id != role.id);
                    g.roles.push(role);
                    members::sort_roles(&mut g.roles);
                }
            }
            ToUi::RoleDeleted { guild_id, role_id } => {
                if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
                    g.roles.retain(|r| r.id != role_id);
                }
            }
            ToUi::Profile(p) => {
                if let (Some(gid), Some(m)) = (&p.guild_id, &p.member) {
                    self.member_cache.insert_cached(gid, &p.user_id, m.clone());
                }
                self.profiles.insert(*p);
            }
            ToUi::ProfileUnavailable(user_id) => self.profiles.mark_unavailable(&user_id),
            ToUi::Note { user_id, note } => {
                let old = self.profiles.note(&user_id).map(str::to_string);
                if let Some(pop) = self.popout.as_mut().filter(|p| p.user.id == user_id) {
                    // Don't overwrite what the user is typing.
                    let untouched = match (&pop.note_draft, &old) {
                        (None, _) => true,
                        (Some(d), Some(o)) => d == o,
                        (Some(d), None) => d.is_empty(),
                    };
                    if untouched {
                        pop.note_draft = Some(note.clone());
                    }
                }
                self.profiles.set_note(&user_id, note);
            }
            ToUi::GuildRemoved(id) => {
                if let Some(ts) = &mut self.threads {
                    ts.store.remove_guild(&id);
                }
                self.guilds.retain(|g| g.id != id);
                if self.selected_guild.as_ref() == Some(&id) {
                    self.selected_guild = Some(DM_GUILD_ID.into());
                    self.selected_channel = None;
                }
            }
            ToUi::Pins { channel_id, pins } => {
                let mut list = PinnedMessage::list_from_json(&pins);
                // Previews as plain text (mentions resolved, no markup).
                let gid = self
                    .find_channel(&channel_id)
                    .map(|(g, _)| g.id.clone())
                    .unwrap_or_default();
                for p in &mut list {
                    if !p.message.content.trim().is_empty() {
                        let text = self.plain_text(&p.message.content, &gid);
                        p.preview = notify::preview(&text, 300);
                    }
                }
                if let Some((p, _)) = &mut self.pins
                    && p.channel_id == channel_id
                {
                    p.set_pins(list);
                }
            }
            ToUi::PinsFailed { channel_id, error } => {
                if let Some((p, _)) = &mut self.pins
                    && p.channel_id == channel_id
                {
                    p.set_failed(error);
                }
            }
            ToUi::PinChanged {
                channel_id,
                message_id,
                pinned,
            } => {
                if let Some(m) = self
                    .messages
                    .get_mut(&channel_id)
                    .and_then(|l| l.iter_mut().find(|m| m.id == message_id))
                {
                    m.pinned = pinned;
                }
                if let Some((p, _)) = &mut self.pins
                    && p.channel_id == channel_id
                {
                    if pinned {
                        let _ = self.tx.send(ToBackend::LoadPins { channel_id });
                    } else {
                        p.remove(&message_id);
                    }
                }
            }
            ToUi::PinFailed {
                channel_id,
                message_id,
                error,
            } => match &mut self.pins {
                Some((p, _)) if p.channel_id == channel_id => p.unpin_failed(&message_id, error),
                _ => self.toast = Some((error, Instant::now())),
            },
            ToUi::PinsUpdated(channel_id) => {
                if let Some((p, _)) = &self.pins
                    && p.channel_id == channel_id
                {
                    let _ = self.tx.send(ToBackend::LoadPins { channel_id });
                }
            }
            ToUi::UserSettings(s) => self.set_user_settings(*s),
            ToUi::UserSettingsUpdate(d) => {
                if let Some(s) = &mut self.user_settings
                    && let Ok(true) = s.apply_update_event(&d)
                {
                    let compact = s.compact_mode;
                    if let Some(c) = compact {
                        self.config_updates.push(ConfigUpdate::Compact(c));
                    }
                }
            }
            ToUi::Blocked { user_id, blocked } => {
                if blocked {
                    self.blocked.insert(user_id);
                } else {
                    self.blocked.remove(&user_id);
                }
            }
            ToUi::Notice(text) => self.toast = Some((text, Instant::now())),
            ToUi::RateLimited(rl) => self.rate_limited(rl),
            ToUi::GuildEmojis { guild_id, emojis } => {
                if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
                    g.emojis = emojis;
                }
                self.emoji_groups = None;
            }
            // --- themes --- (the app imports it: it owns the ThemeStore)
            ToUi::ThemePicked(path) => self.picked_theme = path,
            // --- onboarding ---
            ToUi::InviteResolved { code, result } => {
                if let Some(s) = &mut self.invites {
                    s.cache.set_fetched(&code, result);
                }
            }
            ToUi::InviteJoined { code, outcome } => self.invite_joined(code, outcome, ctx),
            ToUi::ScreeningLoaded { guild_id, result } => {
                if let Some(d) = self
                    .invites
                    .as_mut()
                    .and_then(|s| s.screening.as_mut())
                    .filter(|d| d.guild_id == guild_id)
                {
                    match result {
                        Ok(form) => d.set_loaded(form),
                        Err(e) => d.set_failed(e),
                    }
                }
            }
            ToUi::ScreeningSubmitted { guild_id, result } => {
                if let Some(d) = self
                    .invites
                    .as_mut()
                    .and_then(|s| s.screening.as_mut())
                    .filter(|d| d.guild_id == guild_id)
                {
                    d.set_result(result);
                }
            }
            ToUi::OnboardingLoaded {
                guild_id,
                page,
                result,
            } => self.onboarding_loaded(guild_id, page, result),
            ToUi::OnboardingSaved { guild_id, result } => {
                if let Some(s) = &mut self.invites {
                    if let Some(f) = s.onboarding.as_mut().filter(|f| f.guild_id() == guild_id) {
                        f.set_saved(result);
                    } else if let Some(p) = s
                        .channels_roles
                        .as_mut()
                        .filter(|p| p.guild_id() == guild_id)
                    {
                        p.set_saved(result);
                    }
                }
            }
            ToUi::MyMemberUpdate {
                guild_id,
                pending,
                flags,
            } => {
                if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
                    g.my_pending = pending;
                    g.my_member_flags = flags;
                }
            }
            // --- threads ---
            ToUi::Thread(ev) => {
                if let Some(ts) = &mut self.threads {
                    ts.store.apply(ev);
                }
            }
            ToUi::ThreadPage { page } => {
                if let Some(ts) = &mut self.threads {
                    ts.store.apply_page(page);
                }
            }
            ToUi::ArchivedThreads { channel_id, page } => {
                if let Some(ts) = &mut self.threads {
                    ts.archived_more.insert(channel_id, page.has_more);
                    ts.store.apply_page(page);
                }
            }
            ToUi::ForumPage {
                forum_id,
                query,
                page,
            } => {
                if let Some(ts) = &mut self.threads {
                    ts.store.apply_forum_page(&forum_id, &query, page);
                }
            }
            ToUi::ForumPageFailed { forum_id, error } => {
                if let Some(ts) = &mut self.threads {
                    ts.store.fail_load(&forum_id, error);
                }
            }
            ToUi::ThreadsUnavailable { guild_id } => {
                if let Some(ts) = &mut self.threads {
                    ts.unavailable.insert(guild_id);
                }
            }
            ToUi::ThreadCreated(t) => {
                if self.threads.is_some() {
                    self.open_thread(t, ctx, config);
                }
            }
            ToUi::ThreadJoined { thread_id, joined } => {
                if let Some(ts) = &mut self.threads {
                    ts.store.set_joined(&thread_id, joined);
                }
            }
            ToUi::ThreadFailed { error } => {
                if self.slow_down_secs().is_none() {
                    self.toast = Some((error, Instant::now()));
                }
            }
            ToUi::ThreadParentDeleted(id) => {
                if let Some(ts) = &mut self.threads {
                    ts.store.remove_parent(&id);
                }
            }
            // --- friends ---
            ToUi::Relationships(list) => {
                if let Some(fs) = &mut self.friends {
                    fs.rels.set_all(list);
                }
            }
            ToUi::MessageRequests(list) => {
                if let Some(fs) = &mut self.friends {
                    fs.requests.set_all(list);
                }
            }
            ToUi::RelationshipEvent(ev) => {
                if let Some(fs) = &mut self.friends {
                    fs.rels.apply(ev);
                }
            }
            ToUi::MessageRequestUpdate { channel_id, info } => {
                if let Some(fs) = &mut self.friends {
                    fs.requests.apply(channel_id, info);
                }
            }
            ToUi::FriendResult { action, result } => {
                self.friend_result(action, result, ctx, config)
            }
            ToUi::RequestResult { action, result } => match result {
                Ok(()) => {
                    let id = match &action {
                        RequestAction::Accept(id) | RequestAction::Ignore(id) => id.clone(),
                        RequestAction::Open(_) => return,
                    };
                    if let Some(fs) = &mut self.friends {
                        fs.requests.remove(&id);
                    }
                    if matches!(action, RequestAction::Ignore(_)) {
                        self.dms.channels.retain(|c| c.id != id);
                        if self.selected_channel.as_deref() == Some(id.as_str()) {
                            self.selected_channel = None;
                        }
                    } else if let Some(c) = self.dms.channels.iter_mut().find(|c| c.id == id) {
                        c.is_request = false;
                    }
                }
                Err(e) => {
                    if self.slow_down_secs().is_none() {
                        self.toast = Some((e, Instant::now()));
                    }
                }
            },
            // --- search ---
            ToUi::SearchResults {
                seq,
                total,
                messages,
                partial,
            } => {
                if let Some(s) = &mut self.search {
                    s.panel.on_results(seq, total, messages, partial);
                }
            }
            ToUi::SearchIndexing {
                seq,
                retry_after,
                documents_indexed,
            } => {
                if let Some(s) = &mut self.search {
                    s.panel.on_indexing(seq, retry_after, documents_indexed);
                }
            }
            ToUi::SearchFailed { seq, error } => {
                if let Some(s) = &mut self.search {
                    s.panel.on_error(seq, error);
                }
            }
            // --- reactions ---
            ToUi::Reaction(ev) => {
                let me = self.me.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
                if let Some(rs) = &mut self.reactions
                    && let Some(m) = self
                        .messages
                        .get_mut(ev.channel_id())
                        .and_then(|l| l.iter_mut().find(|m| m.id == ev.message_id()))
                {
                    rs.pending.on_event(&mut m.reactions, &ev, me);
                    rs.reactors.retain(|(id, _, _), _| id != ev.message_id());
                    rs.asked.retain(|(id, _, _)| id != ev.message_id());
                }
            }
            ToUi::ReactionFailed {
                channel_id,
                message_id,
                emoji,
                add,
                burst,
                error,
            } => {
                if let Some(rs) = &mut self.reactions
                    && let Some(m) = self
                        .messages
                        .get_mut(&channel_id)
                        .and_then(|l| l.iter_mut().find(|m| m.id == message_id))
                {
                    rs.pending
                        .on_failure(&mut m.reactions, &message_id, &emoji, add, burst);
                }
                // A rate limit already shows the "Slow down" countdown.
                if self.slow_down_secs().is_none() {
                    self.toast = Some((error, Instant::now()));
                }
            }
            ToUi::Reactors {
                message_id,
                emoji,
                burst,
                names,
            } => {
                if let Some(rs) = &mut self.reactions {
                    if rs.reactors.len() > 256 {
                        rs.reactors.clear();
                        rs.asked.clear();
                    }
                    rs.reactors.insert((message_id, emoji, burst), names);
                }
            }
        }
    }

    // --- onboarding ---
    /// Turn the module's state on / off (off frees everything it held).
    fn set_invites_module(&mut self, on: bool) {
        match (on, self.invites.is_some()) {
            (true, false) => self.invites = Some(InvitesState::default()),
            (false, true) => self.invites = None,
            _ => {}
        }
    }

    /// An invite card / Join a Server action. Joining only from the user's
    /// click, one at a time, nothing while rate limited.
    fn invite_action(&mut self, a: InviteAction, ctx: &egui::Context, config: &Config) {
        let slow = self.slow_down_secs().is_some();
        let Some(s) = &mut self.invites else {
            return;
        };
        match a {
            InviteAction::Fetch(code) | InviteAction::Retry(code) => {
                if s.cache.begin_fetch(&code) {
                    let _ = self.tx.send(ToBackend::ResolveInvite { code });
                }
            }
            InviteAction::Join(code) => {
                if !slow && s.cache.begin_join(&code) {
                    let _ = self.tx.send(ToBackend::JoinInvite { code });
                }
            }
            InviteAction::Open(code) => {
                let target = s
                    .cache
                    .invite(&code)
                    .and_then(|i| i.target_id())
                    .map(str::to_string);
                if let Some(id) = target {
                    s.add_server = None;
                    if self.guilds.iter().any(|g| g.id == id) {
                        self.select_guild(id);
                    } else if self.dms.channel(&id).is_some() {
                        self.select_guild(DM_GUILD_ID.to_string());
                        self.select_channel(id, ctx, config);
                    }
                }
            }
            InviteAction::OpenLink(url) => self.open_link(&url, ctx, config),
        }
    }

    /// Joined: onboarding questions first, then rules screening (each
    /// only asks; answers are sent when the user presses the button).
    fn invite_joined(&mut self, code: String, outcome: invites::JoinOutcome, ctx: &egui::Context) {
        let _ = ctx;
        let Some(s) = &mut self.invites else {
            return;
        };
        s.cache.set_join_result(&code, &outcome);
        let invites::JoinOutcome::Joined {
            guild_id: Some(gid),
            show_verification_form,
            features,
            ..
        } = outcome
        else {
            return;
        };
        s.add_server = None;
        let screening = show_verification_form || crate::screening::guild_has_screening(&features);
        // Just joined: member flags are 0 (nothing completed yet).
        if crate::onboarding::needs_onboarding(&features, 0) {
            s.screening_next = screening.then(|| gid.clone());
            let _ = self.tx.send(ToBackend::LoadOnboarding {
                guild_id: gid,
                page: false,
            });
        } else if screening {
            self.open_screening(&gid);
        }
    }

    fn open_screening(&mut self, guild_id: &str) {
        let (name, icon) = self
            .guilds
            .iter()
            .find(|g| g.id == guild_id)
            .map(|g| (g.name.clone(), g.icon_url.clone()))
            .unwrap_or_default();
        if let Some(s) = &mut self.invites {
            s.screening = Some(ScreeningDialog::new(guild_id, &name, icon));
            let _ = self.tx.send(ToBackend::LoadScreening {
                guild_id: guild_id.to_string(),
            });
        }
    }

    fn onboarding_loaded(
        &mut self,
        guild_id: String,
        page: bool,
        result: Result<crate::onboarding::Onboarding, String>,
    ) {
        let (name, roles) = self
            .guilds
            .iter()
            .find(|g| g.id == guild_id)
            .map(|g| (g.name.clone(), g.my_roles.clone()))
            .unwrap_or_default();
        let Some(s) = &mut self.invites else {
            return;
        };
        match result {
            Ok(onb) => {
                let prior = crate::onboarding::guess_selected(&onb, &roles);
                if page {
                    s.channels_roles = Some(ChannelsRoles::new(onb, &name, prior));
                } else {
                    s.onboarding = Some(OnboardingFlow::new(onb, &name, prior));
                }
            }
            Err(e) => {
                if !page && let Some(gid) = s.screening_next.take() {
                    // No questions after all: straight to the rules.
                    self.open_screening(&gid);
                } else if self.slow_down_secs().is_none() {
                    self.toast = Some((e, Instant::now()));
                }
            }
        }
    }

    /// The join / screening / onboarding / Channels & Roles dialogs.
    fn invite_dialogs(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        let Some(mut s) = self.invites.take() else {
            return;
        };
        let slow = self.slow_down_secs().is_some();
        let mut invite_action = None;
        let mut screening_after = None;
        if let Some(d) = &mut s.add_server {
            let guilds = &self.guilds;
            let dms = &self.dms;
            let is_member =
                |id: &str| guilds.iter().any(|g| g.id == id) || dms.channel(id).is_some();
            let cx = CardCtx {
                pal,
                cache: &s.cache,
                is_member: &is_member,
                can_join: !self.me.as_ref().is_some_and(|u| u.bot),
                sent_by_me: false,
                button: true,
            };
            match d.show(ctx, &cx) {
                Some(AddServerAction::Invite(a)) => invite_action = Some(a),
                Some(AddServerAction::Close) => s.add_server = None,
                None => {}
            }
        }
        if let Some(f) = &mut s.onboarding {
            match f.show(ctx, pal) {
                Some(OnboardingAction::Submit { guild_id, body }) if !slow => {
                    let _ = self.tx.send(ToBackend::SubmitOnboarding { guild_id, body });
                }
                // Held by a slow-down: unlock the dialog instead of leaving it
                // stuck on "Submitting…".
                Some(OnboardingAction::Submit { .. }) => {
                    f.set_saved(Err(SLOW_DOWN_RETRY.into()));
                }
                Some(OnboardingAction::Close) => {
                    s.onboarding = None;
                    screening_after = s.screening_next.take();
                }
                _ => {}
            }
            if s.onboarding.as_ref().is_some_and(|f| f.is_done()) {
                s.onboarding = None;
                screening_after = s.screening_next.take();
            }
        }
        if let Some(d) = &mut s.screening {
            match d.show(ctx, pal) {
                Some(ScreeningAction::Submit { guild_id, body }) if !slow => {
                    let _ = self.tx.send(ToBackend::SubmitScreening { guild_id, body });
                }
                Some(ScreeningAction::Submit { .. }) => {
                    d.set_result(Err(SLOW_DOWN_RETRY.into()));
                }
                Some(ScreeningAction::Retry { guild_id }) => {
                    let _ = self.tx.send(ToBackend::LoadScreening { guild_id });
                }
                Some(ScreeningAction::Close) => s.screening = None,
                _ => {}
            }
        }
        if let Some(p) = &mut s.channels_roles {
            let mut open = true;
            let mut act = None;
            egui::Modal::new(egui::Id::unique("rc_channels_roles"))
                .frame(widgets::menu_frame(pal).inner_margin(16))
                .show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.set_max_height(ctx.content_rect().height() * 0.8);
                    act = p.show(ui, pal);
                });
            match act {
                Some(OnboardingAction::Submit { guild_id, body }) if !slow => {
                    let _ = self.tx.send(ToBackend::SubmitOnboarding { guild_id, body });
                }
                Some(OnboardingAction::Submit { .. }) => {
                    p.set_saved(Err(SLOW_DOWN_RETRY.into()));
                }
                Some(OnboardingAction::Close) => open = false,
                _ => {}
            }
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                s.channels_roles = None;
            }
        }
        self.invites = Some(s);
        if let Some(gid) = screening_after {
            self.open_screening(&gid);
        }
        if let Some(a) = invite_action {
            self.invite_action(a, ctx, config);
        }
    }

    // --- threads ---
    /// Turn the module's state on / off (off frees everything it held).
    fn set_threads_module(&mut self, on: bool) {
        match (on, self.threads.is_some()) {
            (true, false) => {
                let mut ts = ThreadsState::default();
                if let Some(me) = &self.me {
                    ts.store.set_me(&me.id);
                }
                self.threads = Some(ts);
            }
            (false, true) => self.threads = None,
            _ => {}
        }
    }

    /// Make an opened thread selectable like a channel: a (hidden) channel
    /// entry under its server, then select it.
    fn open_thread(&mut self, t: Thread, ctx: &egui::Context, config: &Config) {
        let channel = crate::types::Channel::from_json(&serde_json::json!({
            "id": t.id, "name": t.name, "type": t.kind, "parent_id": t.parent_id,
            "last_message_id": t.last_message_id.to_string(),
        }));
        let gid = t.guild_id.clone();
        let id = t.id.clone();
        if let Some(ts) = &mut self.threads {
            ts.store.apply(threads::ThreadEvent::Upsert(t));
        }
        let Some(g) = self.guilds.iter_mut().find(|g| g.id == gid) else {
            return;
        };
        if g.channel(&id).is_none() {
            g.upsert_channel(channel);
        }
        self.select_guild(gid);
        self.select_channel(id, ctx, config);
    }

    /// A click in a thread row, the thread browser, a forum or a dialog.
    /// Writes only from clicks, one at a time, nothing while rate limited.
    fn thread_action(&mut self, a: ThreadAction, ctx: &egui::Context, config: &Config) {
        let Some(ts) = &mut self.threads else {
            return;
        };
        match a {
            ThreadAction::OpenThread { thread_id, .. } => {
                if let Some(t) = ts.store.get(&thread_id).cloned() {
                    self.open_thread(t, ctx, config);
                }
            }
            ThreadAction::LoadMore { forum_id, query } => {
                ts.store.begin_load(&forum_id, &query);
                let _ = self.tx.send(ToBackend::Threads(ThreadAction::LoadMore {
                    forum_id,
                    query,
                }));
            }
            ThreadAction::LoadArchived { .. } => {
                let _ = self.tx.send(ToBackend::Threads(a));
            }
            write => {
                if self.slow_down_secs().is_none() {
                    let _ = self.tx.send(ToBackend::Threads(write));
                }
            }
        }
    }

    /// Thread browser opened for a channel: ask for active (once per
    /// server) and archived (once per channel) threads. No polling.
    fn open_thread_browser(&mut self, channel_id: String, anchor: egui::Rect) {
        let guild_id = self.selected_guild.clone().unwrap_or_default();
        let Some(ts) = &mut self.threads else {
            return;
        };
        if !ts.unavailable.contains(&guild_id) && ts.asked_active.insert(guild_id.clone()) {
            let _ = self.tx.send(ToBackend::LoadActiveThreads { guild_id });
        }
        if ts.asked_archived.insert(channel_id.clone()) {
            let _ = self.tx.send(ToBackend::Threads(ThreadAction::LoadArchived {
                channel_id: channel_id.clone(),
                before: None,
            }));
        }
        ts.browser = Some((channel_id, anchor));
    }

    /// Forum channel: the post list instead of messages.
    fn forum_page(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let Some(cid) = self.selected_channel.clone() else {
            return;
        };
        let Some(forum) = self
            .find_channel(&cid)
            .and_then(|(_, c)| c.forum.as_deref().cloned())
        else {
            ui.label(RichText::new("This forum isn't loaded yet.").color(pal.muted));
            return;
        };
        let Some(mut ts) = self.threads.take() else {
            return;
        };
        let reads = &self.reads;
        let names = &self.user_names;
        let is_unread = |id: &str| reads.is_unread(id);
        let user_name = |id: &str| names.get(id).cloned();
        let tctx = threads::Ctx {
            pal,
            selected: Some(&cid),
            is_unread: &is_unread,
            user_name: &user_name,
            load_images: config.modules.images,
            can_manage_threads: false,
        };
        let view = ts
            .forum_view
            .get_or_insert_with(|| ForumView::new(&forum.id));
        let action = egui::Frame::new()
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| view.show(ui, &forum, &ts.store, &tctx))
            .inner;
        self.threads = Some(ts);
        if let Some(a) = action {
            let ctx = ui.ctx().clone();
            self.thread_action(a, &ctx, config);
        }
    }

    /// The Create Thread dialog and the thread browser popout.
    fn thread_dialogs(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        let Some(mut ts) = self.threads.take() else {
            return;
        };
        let mut action = None;
        if let Some(d) = &mut ts.dialog {
            match d.show(ctx, pal) {
                DialogOutcome::Open => {}
                DialogOutcome::Cancelled => ts.dialog = None,
                DialogOutcome::Submit(a) => {
                    action = Some(a);
                    ts.dialog = None;
                }
            }
        }
        if let Some((cid, anchor)) = ts.browser.clone() {
            let reads = &self.reads;
            let names = &self.user_names;
            let is_unread = |id: &str| reads.is_unread(id);
            let user_name = |id: &str| names.get(id).cloned();
            let tctx = threads::Ctx {
                pal,
                selected: self.selected_channel.as_deref(),
                is_unread: &is_unread,
                user_name: &user_name,
                load_images: config.modules.images,
                can_manage_threads: false,
            };
            let gid = self.selected_guild.clone().unwrap_or_default();
            let width = 360.0;
            let area = egui::Area::new(egui::Id::unique("rc_thread_browser"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(
                    (anchor.right() - width).max(8.0),
                    anchor.bottom() + 6.0,
                ))
                .constrain(true)
                .show(ctx, |ui| {
                    widgets::menu_frame(pal)
                        .show(ui, |ui| {
                            ui.set_width(width);
                            ui.label(
                                RichText::new(icons::label(icons::CHATS, "Threads"))
                                    .bold()
                                    .color(pal.strong),
                            );
                            if ts.unavailable.contains(&gid) {
                                ui.label(
                                    RichText::new("Not available here (showing what Discord sent)")
                                        .small()
                                        .color(pal.muted),
                                );
                            }
                            if ui
                                .add(
                                    pal.primary_button(&icons::label(icons::PLUS, "Create Thread")),
                                )
                                .clicked()
                            {
                                ts.dialog = Some(ThreadDialog::standalone(&cid));
                            }
                            ui.add_space(4.0);
                            let more = ts.archived_more.get(&cid).copied().unwrap_or(false);
                            threads::thread_browser(ui, &cid, &ts.store, &tctx, more)
                        })
                        .inner
                });
            if let Some(a) = area.inner {
                action = Some(a);
            }
            let outside = ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p))
            });
            if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ts.browser = None;
            }
        }
        self.threads = Some(ts);
        if let Some(a) = action {
            if matches!(a, ThreadAction::OpenThread { .. })
                && let Some(ts) = &mut self.threads
            {
                ts.browser = None;
            }
            self.thread_action(a, ctx, config);
        }
    }

    // --- friends ---
    /// Turn the module's state on / off (off frees everything it held).
    fn set_friends_module(&mut self, on: bool) {
        match (on, self.friends.is_some()) {
            (true, false) => self.friends = Some(FriendsState::default()),
            (false, true) => self.friends = None,
            _ => {}
        }
    }

    /// The open channel is a forum (no messages of its own).
    fn selected_is_forum(&self) -> bool {
        self.selected_channel
            .as_deref()
            .and_then(|id| self.find_channel(id))
            .is_some_and(|(_, c)| c.is_forum())
    }

    /// The Friends or Message Requests page is showing instead of the chat.
    fn home_page_open(&self) -> bool {
        self.selected_guild.as_deref() == Some(DM_GUILD_ID)
            && self
                .friends
                .as_ref()
                .is_some_and(|f| f.page != HomeSelected::None)
    }

    /// A DM that is a message request: never acked automatically.
    fn is_request_channel(&self, channel_id: &str) -> bool {
        self.dms.channel(channel_id).is_some_and(|c| c.is_request)
            || self
                .friends
                .as_ref()
                .is_some_and(|f| f.requests.is_request(channel_id))
    }

    fn set_home_page(&mut self, page: HomeSelected) {
        if let Some(fs) = &mut self.friends {
            fs.page = page;
        }
    }

    fn home_action(&mut self, a: HomeAction, ctx: &egui::Context, config: &Config) {
        match a {
            HomeAction::Friends => self.set_home_page(HomeSelected::Friends),
            HomeAction::Requests => self.set_home_page(HomeSelected::Requests),
            HomeAction::Dm(user_id) => {
                self.friends_action(FriendsAction::Message(user_id), ctx, config)
            }
        }
    }

    /// Open an existing DM locally (no API call).
    fn open_dm_channel(&mut self, channel_id: String, ctx: &egui::Context, config: &Config) {
        self.set_home_page(HomeSelected::None);
        self.select_guild(DM_GUILD_ID.to_string());
        self.select_channel(channel_id, ctx, config);
    }

    /// A Friends page / Home click. Only ever from the user; nothing while
    /// rate limited; an existing DM opens without an API call.
    fn friends_action(&mut self, a: FriendsAction, ctx: &egui::Context, config: &Config) {
        if let FriendsAction::Message(uid) = &a
            && let Some(cid) = self
                .dms
                .channels
                .iter()
                .find(|c| {
                    c.kind == CHANNEL_DM && c.recipients.first().is_some_and(|u| &u.id == uid)
                })
                .map(|c| c.id.clone())
        {
            self.open_dm_channel(cid, ctx, config);
            return;
        }
        if let Some(secs) = self.slow_down_secs() {
            let msg = format!("Slow down, try again in {secs}s");
            if let (FriendsAction::SendRequest(_), Some(fs)) = (&a, &mut self.friends) {
                fs.friends_page.add_result(Err(msg));
            }
            return;
        }
        let _ = self.tx.send(ToBackend::Friends(a));
    }

    fn request_action(&mut self, a: RequestAction, ctx: &egui::Context, config: &Config) {
        match a {
            // Just view it: loads history, never acks (see ack_open_channel).
            RequestAction::Open(id) => self.open_dm_channel(id, ctx, config),
            _ if self.slow_down_secs().is_some() => {}
            a => {
                let _ = self.tx.send(ToBackend::MessageRequest(a));
            }
        }
    }

    fn friend_result(
        &mut self,
        action: FriendsAction,
        result: Result<Outcome, String>,
        ctx: &egui::Context,
        config: &Config,
    ) {
        match result {
            Ok(Outcome::DmOpened(v)) => {
                let channel = crate::types::Channel::from_json(&v);
                let id = channel.id.clone();
                if id.is_empty() {
                    return;
                }
                self.dms.upsert_channel(channel);
                self.open_dm_channel(id, ctx, config);
            }
            Ok(Outcome::RequestSent(name)) => {
                if let Some(fs) = &mut self.friends {
                    fs.friends_page.add_result(Ok(name));
                }
            }
            Ok(Outcome::Done) => {}
            Err(e) => match (&action, &mut self.friends) {
                (FriendsAction::SendRequest(_), Some(fs)) => fs.friends_page.add_result(Err(e)),
                _ => {
                    if self.slow_down_secs().is_none() {
                        self.toast = Some((e, Instant::now()));
                    }
                }
            },
        }
    }

    /// The Friends / Message Requests page in place of the chat.
    fn home_page(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let Some(mut fs) = self.friends.take() else {
            return;
        };
        let is_bot = self.me.as_ref().is_some_and(|u| u.bot);
        let avatar = |ui: &egui::Ui, r: egui::Rect, u: &User| {
            paint_avatar(ui, r, &u.id, &u.name, &u.avatar_url, config, pal);
        };
        let mut friends_action = None;
        let mut request_action = None;
        match fs.page {
            HomeSelected::Friends => {
                let presences = &self.presences;
                let presence = |id: &str| presences.get(id).cloned();
                let cx = FriendsCtx {
                    pal,
                    store: &fs.rels,
                    is_bot,
                    presence: &presence,
                    avatar: &avatar,
                };
                friends_action = fs.friends_page.show(ui, &cx);
            }
            HomeSelected::Requests => {
                let messages = &self.messages;
                let preview = |cid: &str| {
                    messages
                        .get(cid)
                        .and_then(|l| l.last())
                        .map(|m| m.content.clone())
                };
                let cx = RequestsCtx {
                    pal,
                    store: &fs.requests,
                    dms: &self.dms.channels,
                    is_bot,
                    preview: &preview,
                    avatar: &avatar,
                };
                request_action = fs.requests_page.show(ui, &cx);
            }
            HomeSelected::None => {}
        }
        self.friends = Some(fs);
        let ctx = ui.ctx().clone();
        if let Some(a) = friends_action {
            self.friends_action(a, &ctx, config);
        }
        if let Some(a) = request_action {
            self.request_action(a, &ctx, config);
        }
    }

    // --- search ---
    /// Ctrl+F (`whole: false`, this channel) / Ctrl+Shift+F or the header
    /// button (`whole: true`, the whole server or DM).
    fn open_search(&mut self, whole: bool) {
        let Some((g, c)) = self
            .selected_channel
            .as_deref()
            .and_then(|id| self.find_channel(id))
        else {
            return;
        };
        let scope = if g.is_dms() {
            SearchScope::Dm(c.id.clone())
        } else {
            SearchScope::Guild(g.id.clone())
        };
        let channel = (!whole && !g.is_dms()).then(|| (c.id.clone(), c.name.clone()));
        let s = self.search.get_or_insert_with(|| SearchState {
            panel: SearchPanel::new(),
            scope: scope.clone(),
            anchor: egui::Rect::NOTHING,
        });
        if s.scope != scope {
            s.panel.reset();
            s.scope = scope;
        }
        s.panel.open(
            channel
                .as_ref()
                .map(|(id, name)| (id.as_str(), name.as_str())),
        );
    }

    /// The search popup (under the header's search button).
    fn search_popup(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        let Some(mut s) = self.search.take() else {
            return;
        };
        let (gid, in_dm) = match &s.scope {
            SearchScope::Guild(g) => (g.clone(), false),
            SearchScope::Dm(c) => (c.clone(), true),
        };
        let guild = self.guilds.iter().find(|g| g.id == gid);
        let placeholder = match (&s.scope, guild) {
            (SearchScope::Guild(_), Some(g)) => format!("Search {}", g.name),
            (SearchScope::Dm(c), _) => match self.find_channel(c) {
                Some((_, ch)) if !ch.name.is_empty() => format!("Search {}", ch.name),
                Some((_, ch)) => format!(
                    "Search @{}",
                    ch.recipients.first().map_or("", |u| u.name.as_str())
                ),
                None => "Search".into(),
            },
            _ => "Search".into(),
        };
        let names = &self.user_names;
        let me = self.me.as_ref();
        let user = |name: &str| {
            let n = name.to_lowercase();
            me.filter(|u| u.name.to_lowercase() == n || u.username.to_lowercase() == n)
                .map(|u| u.id.clone())
                .or_else(|| {
                    names
                        .iter()
                        .find(|(_, v)| v.to_lowercase() == n)
                        .map(|(k, _)| k.clone())
                })
        };
        let text_channels = || {
            guild
                .into_iter()
                .flat_map(|g| g.channels.iter())
                .filter(|c| c.is_text())
        };
        let channel = |name: &str| {
            text_channels()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .map(|c| c.id.clone())
        };
        let suggest_users = |q: &str, n: usize| {
            let q = q.to_lowercase();
            let mut out: Vec<Suggestion> = names
                .values()
                .filter(|v| v.to_lowercase().contains(&q))
                .take(n)
                .map(|v| Suggestion {
                    value: v.clone(),
                    label: v.clone(),
                    detail: String::new(),
                })
                .collect();
            out.sort_by_key(|s| !s.value.to_lowercase().starts_with(&q));
            out
        };
        let suggest_channels = |q: &str, n: usize| {
            let q = q.to_lowercase();
            text_channels()
                .filter(|c| c.name.to_lowercase().contains(&q))
                .take(n)
                .map(|c| Suggestion {
                    value: c.name.clone(),
                    label: format!("#{}", c.name),
                    detail: String::new(),
                })
                .collect::<Vec<_>>()
        };
        let user_name = |id: &str| names.get(id).cloned();
        let channel_name = |id: &str| {
            std::iter::once(&self.dms)
                .chain(&self.guilds)
                .find_map(|g| g.channel(id))
                .map(|c| c.name.clone())
        };
        let cx = SearchCtx {
            pal,
            placeholder: &placeholder,
            in_dm,
            include_nsfw: false,
            show_images: config.modules.images,
            resolver: crate::search::Resolver {
                user: &user,
                channel: &channel,
            },
            suggest_users: &suggest_users,
            suggest_channels: &suggest_channels,
            user_name: &user_name,
            channel_name: &channel_name,
            // Room for the box, header and pager under the button.
            list_height: Some(
                (ctx.content_rect().bottom() - s.anchor.bottom() - 150.0).clamp(160.0, 420.0),
            ),
        };
        let width = 420.0;
        let pos = egui::pos2((s.anchor.right() - width).max(8.0), s.anchor.bottom() + 6.0);
        let mut action = None;
        let area = egui::Area::new(egui::Id::unique("rc_search_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                widgets::menu_frame(pal).show(ui, |ui| {
                    ui.set_width(width);
                    action = s.panel.show(ui, &cx);
                });
            });
        let outside = ctx.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !s.anchor.contains(p))
        });
        let mut keep = !outside;
        match action {
            Some(SearchAction::Search { seq, query, offset }) => {
                let _ = self.tx.send(ToBackend::Search {
                    seq,
                    scope: s.scope.clone(),
                    query,
                    offset,
                });
            }
            Some(SearchAction::Jump {
                channel_id,
                message_id,
            }) => {
                self.go_to_channel(&channel_id, ctx, config);
                self.jump_to(channel_id, message_id);
            }
            Some(SearchAction::Close) => keep = false,
            None => {}
        }
        if keep {
            self.search = Some(s);
        }
    }

    // --- reactions ---
    /// Turn the module's state on / off (off frees everything it held).
    fn set_reactions_module(&mut self, on: bool) {
        match (on, self.reactions.is_some()) {
            (true, false) => self.reactions = Some(ReactionState::default()),
            (false, true) => {
                self.reactions = None;
                for list in self.messages.values_mut() {
                    for m in list {
                        m.reactions = Vec::new();
                    }
                }
            }
            _ => {}
        }
    }

    /// A click in a reaction bar or the add-reaction picker. Reacting only
    /// ever happens here, from the user's click: one request at a time per
    /// message, nothing while rate limited, never retried.
    fn react(&mut self, channel_id: &str, message_id: &str, action: ReactionAction, now: f64) {
        let slow = self.slow_down_secs().is_some();
        let Some(rs) = &mut self.reactions else {
            return;
        };
        let (emoji, add, burst, from_picker) = match action {
            ReactionAction::WantReactors { emoji, burst } => {
                let key = (message_id.to_string(), emoji.clone(), burst);
                if rs.asked.insert(key) {
                    let _ = self.tx.send(ToBackend::LoadReactors {
                        channel_id: channel_id.to_string(),
                        message_id: message_id.to_string(),
                        emoji,
                        burst,
                    });
                }
                return;
            }
            ReactionAction::Add { emoji, from_picker } => (emoji, true, false, from_picker),
            ReactionAction::Remove { emoji, burst } => (emoji, false, burst, false),
        };
        if slow || rs.pending.has_message(message_id) {
            return;
        }
        if from_picker {
            self.config_updates
                .push(ConfigUpdate::RecentEmoji(emoji.clone()));
        }
        let Some(m) = self
            .messages
            .get_mut(channel_id)
            .and_then(|l| l.iter_mut().find(|m| m.id == message_id))
        else {
            return;
        };
        if rs.pending.begin(
            &mut m.reactions,
            channel_id,
            message_id,
            &emoji,
            add,
            burst,
            now,
        ) {
            let _ = self.tx.send(ToBackend::React {
                channel_id: channel_id.to_string(),
                message_id: message_id.to_string(),
                emoji,
                add,
                burst,
            });
        }
    }

    /// Custom emoji for the pickers in the current server (none in DMs:
    /// using other servers' emoji needs Nitro).
    fn refresh_emoji_groups(&mut self) {
        let guild = self
            .selected_channel
            .as_deref()
            .and_then(|c| self.find_channel(c))
            .map(|(g, _)| g)
            .filter(|g| !g.is_dms());
        let key = guild.map(|g| g.id.clone()).unwrap_or_default();
        if self.emoji_groups.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let groups = guild
            .map(|g| {
                vec![CustomEmojiGroup {
                    name: g.name.clone(),
                    emojis: g
                        .emojis
                        .iter()
                        .map(|e| EmojiRef::custom(&e.id, &e.name, e.animated))
                        .collect(),
                }]
            })
            .unwrap_or_default();
        self.emoji_groups = Some((key, groups));
    }

    fn emoji_groups(&self) -> &[CustomEmojiGroup] {
        self.emoji_groups
            .as_ref()
            .map_or(&[], |(_, g)| g.as_slice())
    }

    /// Show the quiet "Slow down" notice and hold write controls until the
    /// wait is over.
    fn rate_limited(&mut self, rl: crate::ratelimit::RateLimited) {
        let now = Instant::now();
        let until = now + rl.retry_after;
        self.slow_down_until = Some(self.slow_down_until.map_or(until, |u| u.max(until)));
        self.toast = Some((rl.to_string(), now));
    }

    /// Seconds left while user write actions are rate limited.
    fn slow_down_secs(&self) -> Option<u64> {
        let left = self
            .slow_down_until?
            .checked_duration_since(Instant::now())?;
        Some(left.as_secs_f64().ceil().max(1.0) as u64)
    }

    /// New synced settings (full message from READY / GET / PATCH).
    fn set_user_settings(&mut self, s: UserSettings) {
        if let Some(c) = s.compact_mode {
            self.config_updates.push(ConfigUpdate::Compact(c));
        }
        self.user_settings = Some(s);
    }

    /// Learn names from messages (for mentions and autocomplete) and drop
    /// embeds when the Embeds module is off.
    fn remember_users(&mut self, messages: &mut [Message], config: &Config) {
        if self.user_names.len() > 5000 {
            self.user_names.clear();
        }
        for m in messages {
            if !config.modules.embeds {
                m.embeds = Vec::new();
            }
            self.user_names
                .insert(m.author.id.clone(), m.author.name.clone());
            for (id, name) in &m.mention_names {
                self.user_names.insert(id.clone(), name.clone());
            }
        }
    }

    // ----- members, presence, profiles ------------------------------------

    /// Free the member list, bot list and presences (module off, new
    /// session).
    fn free_member_list(&mut self, ctx: &egui::Context) {
        if let Some(old) = self.member_list.take() {
            self.forget_list_avatars(ctx, &old);
        }
        if let Some(old) = self.bot_list.take() {
            self.forget_list_avatars(ctx, &old.list);
        }
        if self.presences.len() > 0 {
            self.presences.clear();
        }
        self.subscribed = None;
        self.bot_list_requested = None;
    }

    /// Free avatar images of a dropped member list, except ones still shown
    /// next to cached messages.
    fn forget_list_avatars(&self, ctx: &egui::Context, list: &MemberList) {
        let in_use: HashSet<&str> = self
            .messages
            .values()
            .flatten()
            .map(|m| m.author.avatar_url.as_str())
            .collect();
        for row in list.rows.iter().flatten() {
            if let Row::Member(m) = row
                && !in_use.contains(m.user.avatar_url.as_str())
            {
                ctx.forget_image(&m.user.avatar_url);
            }
        }
    }

    /// Ask (op 8) for the roles of message authors we know nothing about,
    /// so their names get role colours.
    fn request_unknown_authors(&mut self, channel_id: &str, messages: &[Message]) {
        let Some((g, _)) = self.find_channel(channel_id) else {
            return;
        };
        if g.is_dms() {
            return;
        }
        let gid = g.id.clone();
        if self.asked_members.len() > 5000 {
            self.asked_members.clear();
        }
        let mut ids = Vec::new();
        for m in messages {
            let key = format!("{gid}:{}", m.author.id);
            if self.member_cache.get(&gid, &m.author.id).is_none()
                && !ids.contains(&m.author.id)
                && self.asked_members.insert(key)
            {
                ids.push(m.author.id.clone());
            }
        }
        if !ids.is_empty() {
            let _ = self.tx.send(ToBackend::RequestMembers {
                guild_id: gid,
                user_ids: ids,
            });
        }
    }

    /// Server nickname and role colour for a message author.
    fn author_style<'a>(&'a self, guild: Option<&'a Guild>, u: &'a User) -> AuthorStyle<'a> {
        let cached = guild.and_then(|g| self.member_cache.get(&g.id, &u.id));
        AuthorStyle {
            name: cached.and_then(|c| c.nick.as_deref()).unwrap_or(&u.name),
            color: cached.and_then(|c| members::role_color(&guild?.roles, &c.roles)),
        }
    }

    fn is_bot_session(&self) -> bool {
        self.me.as_ref().is_some_and(|m| m.bot)
    }

    /// Open someone's profile card next to `anchor`, loading the profile
    /// and note if we don't have them (user accounts).
    fn open_profile(
        &mut self,
        user: User,
        guild_id: Option<String>,
        anchor: egui::Rect,
        ctx: &egui::Context,
    ) {
        if self.popout.as_ref().is_some_and(|p| p.user.id == user.id) {
            self.close_popout(ctx);
            return;
        }
        self.close_popout(ctx);
        if !self.is_bot_session() {
            if self.profiles.get(&user.id, guild_id.as_deref()).is_none()
                && !self.profiles.is_unavailable(&user.id)
            {
                let _ = self.tx.send(ToBackend::LoadProfile {
                    user_id: user.id.clone(),
                    guild_id: guild_id.clone(),
                });
            }
            if self.profiles.note(&user.id).is_none() {
                let _ = self.tx.send(ToBackend::LoadNote {
                    user_id: user.id.clone(),
                });
            }
        }
        self.popout = Some(Popout {
            note_draft: self.profiles.note(&user.id).map(str::to_string),
            user,
            guild_id,
            anchor,
            opened_frame: ctx.cumulative_frame_nr(),
        });
    }

    fn close_popout(&mut self, ctx: &egui::Context) {
        let Some(pop) = self.popout.take() else {
            return;
        };
        // The card's big avatar and banner aren't used anywhere else.
        ctx.forget_image(&pop.user.avatar_url.replace("size=64", "size=128"));
        if let Some(url) = self
            .profiles
            .get(&pop.user.id, pop.guild_id.as_deref())
            .and_then(|p| p.banner_url.clone())
        {
            ctx.forget_image(&url);
        }
    }

    fn name_for(&self, channel_id: &str, user_id: &str) -> String {
        if let Some(u) = self
            .dms
            .channel(channel_id)
            .and_then(|c| c.recipients.iter().find(|u| u.id == user_id))
        {
            return u.name.clone();
        }
        self.messages
            .get(channel_id)
            .and_then(|list| list.iter().rev().find(|m| m.author.id == user_id))
            .map(|m| m.author.name.clone())
            .unwrap_or_else(|| "Someone".into())
    }

    // ----- read state & notifications ------------------------------------

    /// The server (or DM pseudo-server) and the channel for a channel id.
    fn find_channel(&self, channel_id: &str) -> Option<(&Guild, &crate::types::Channel)> {
        std::iter::once(&self.dms)
            .chain(&self.guilds)
            .find_map(|g| g.channel(channel_id).map(|c| (g, c)))
    }

    fn is_muted(&self, guild_id: &str, c: &crate::types::Channel, now: &str) -> bool {
        self.mutes
            .muted(guild_id, &c.id, c.parent_id.as_deref(), now)
    }

    /// Update unread/mention counts for a new message, and notify if needed.
    fn track_new_message(
        &mut self,
        msg: &Message,
        ctx: &egui::Context,
        config: &Config,
        focused: bool,
    ) {
        let Some(me) = &self.me else { return };
        let now = notify::now_iso();
        let (guild_id, guild_name, channel_name, parent_id, is_dm, default_lvl, roles) =
            match self.find_channel(&msg.channel_id) {
                Some((g, c)) => (
                    g.id.clone(),
                    g.name.clone(),
                    c.name.clone(),
                    c.parent_id.clone(),
                    g.is_dms(),
                    g.default_notifications,
                    g.my_roles.clone(),
                ),
                // Unknown channel: a new DM if there's no guild id.
                None => (
                    msg.guild_id.clone().unwrap_or_else(|| DM_GUILD_ID.into()),
                    String::new(),
                    String::new(),
                    None,
                    msg.guild_id.is_none(),
                    0,
                    Vec::new(),
                ),
            };
        let mine = msg.author.id == me.id;
        let settings = self.mutes.get(&guild_id);
        let mentioned = notify::mentions_me(msg, &me.id, &roles, settings)
            || (is_dm && !mine && !self.mutes.muted(&guild_id, &msg.channel_id, None, &now));
        let viewing = focused
            && self.selected_channel.as_ref() == Some(&msg.channel_id)
            && self.held_unread.as_ref() != Some(&msg.channel_id);
        let id: u64 = msg.id.parse().unwrap_or(0);
        self.reads
            .on_message(&msg.channel_id, id, mentioned, mine, viewing);
        if viewing && !mine {
            self.ack_pending = Some((msg.channel_id.clone(), id));
        }

        if !config.modules.notifications || mine {
            return;
        }
        // Mentions resolved, markup and spoilers hidden.
        let plain = self.plain_text(&msg.content, &guild_id);
        let place = if is_dm {
            "Direct message".to_string()
        } else {
            format!("#{channel_name} · {guild_name}")
        };
        if mentioned {
            self.inbox.push(InboxItem {
                guild_id: guild_id.clone(),
                channel_id: msg.channel_id.clone(),
                message_id: msg.id.clone(),
                place: place.clone(),
                author: msg.author.name.clone(),
                preview: notify::preview(&plain, 140),
                time: msg.short_time().to_string(),
                read: viewing,
            });
        }
        let check = NotifyCheck {
            guild_id: &guild_id,
            channel_id: &msg.channel_id,
            parent_id: parent_id.as_deref(),
            is_dm,
            mine,
            mentioned,
            viewing,
            guild_default: default_lvl,
            is_bot: me.bot,
            now: &now,
        };
        if !notify::should_notify(&self.mutes, &check) {
            return;
        }
        if config.desktop_notifications {
            let title = if is_dm {
                msg.author.name.clone()
            } else {
                format!("{} ({place})", msg.author.name)
            };
            let _ = self.tx.send(ToBackend::Notify {
                title,
                body: notify::preview(&plain, 200),
            });
        }
        if config.flash_taskbar && !focused {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
    }

    /// Keep the open channel read while the window is focused (new messages,
    /// or coming back to the window), sending at most one ack per few seconds.
    fn ack_open_channel(&mut self, ctx: &egui::Context, chat_visible: bool) {
        if chat_visible
            && window_focused(ctx)
            && !self.home_page_open()
            && let Some(cid) = self.selected_channel.clone()
            && self.held_unread.as_ref() != Some(&cid)
            // Message requests are never acked automatically.
            && !self.is_request_channel(&cid)
            && let Some(mid) = self.reads.mark_read(&cid)
        {
            self.inbox.mark_channel_read(&cid);
            self.ack_pending = Some((cid, mid));
        }
        let Some((cid, mid)) = self.ack_pending.clone() else {
            return;
        };
        let wait = self
            .last_ack_at
            .map_or(Duration::ZERO, |t| ACK_INTERVAL.saturating_sub(t.elapsed()));
        if wait.is_zero() {
            self.send_ack(cid, mid);
            self.ack_pending = None;
        } else {
            ctx.request_repaint_after(wait);
        }
    }

    fn send_ack(&mut self, channel_id: String, message_id: u64) {
        self.last_ack_at = Some(Instant::now());
        let _ = self.tx.send(ToBackend::Ack {
            channel_id,
            message_id: message_id.to_string(),
        });
    }

    /// Channel context menu → Mark as read.
    fn mark_channel_read(&mut self, channel_id: &str) {
        self.inbox.mark_channel_read(channel_id);
        if let Some(mid) = self.reads.mark_read(channel_id) {
            self.send_ack(channel_id.to_string(), mid);
        }
    }

    /// Server context menu → Mark as read (one ack-bulk request).
    fn mark_guild_read(&mut self, guild_id: &str) {
        let guild = if guild_id == DM_GUILD_ID {
            Some(&self.dms)
        } else {
            self.guilds.iter().find(|g| g.id == guild_id)
        };
        let ids: Vec<String> = guild
            .map(|g| g.channels.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        let mut acks = Vec::new();
        for cid in ids {
            // Message requests are only marked read when opened on purpose.
            if self.is_request_channel(&cid) {
                continue;
            }
            self.inbox.mark_channel_read(&cid);
            if let Some(mid) = self.reads.mark_read(&cid) {
                acks.push((cid, mid.to_string()));
            }
        }
        if !acks.is_empty() {
            let _ = self.tx.send(ToBackend::AckBulk(acks));
        }
    }

    /// Message menu → Mark unread: everything from this message on is unread.
    fn mark_unread(&mut self, channel_id: &str, message_id: &str) {
        let Some(me) = self.me.clone() else { return };
        let Ok(id) = message_id.parse::<u64>() else {
            return;
        };
        let (guild_id, roles, is_dm) = match self.find_channel(channel_id) {
            Some((g, _)) => (g.id.clone(), g.my_roles.clone(), g.is_dms()),
            None => (DM_GUILD_ID.to_string(), Vec::new(), true),
        };
        let list = self
            .messages
            .get(channel_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        // Ack the message before it (or id - 1 if it's the oldest loaded).
        let before = list
            .iter()
            .filter_map(|m| m.id.parse::<u64>().ok())
            .filter(|&m| m < id)
            .max()
            .unwrap_or(id - 1);
        let settings = self.mutes.get(&guild_id);
        let mentions = list
            .iter()
            .filter(|m| m.id.parse::<u64>().is_ok_and(|m| m >= id))
            .filter(|m| {
                notify::mentions_me(m, &me.id, &roles, settings) || (is_dm && m.author.id != me.id)
            })
            .count() as u32;
        self.reads.ack(channel_id, before, mentions);
        self.held_unread = Some(channel_id.to_string());
        self.new_marker = Some((channel_id.to_string(), before));
        if self
            .ack_pending
            .as_ref()
            .is_some_and(|(c, _)| c == channel_id)
        {
            self.ack_pending = None;
        }
        let _ = self.tx.send(ToBackend::MarkUnread {
            channel_id: channel_id.to_string(),
            message_id: before.to_string(),
            mention_count: mentions,
        });
    }

    /// Unread pill / mention badge / muted look for a server icon.
    fn guild_badge(&self, g: &Guild, now: &str) -> Badge {
        if g.is_dms() {
            // DMs: only a mention count (every unmuted DM message counts).
            let mentions = g
                .channels
                .iter()
                .filter(|c| !self.is_muted(&g.id, c, now))
                .map(|c| self.reads.mentions(&c.id))
                .sum();
            return Badge {
                unread: false,
                mentions,
                muted: false,
            };
        }
        if self.mutes.guild_muted(&g.id, now) {
            return Badge {
                unread: false,
                mentions: 0,
                muted: true,
            };
        }
        let mut badge = Badge::default();
        for c in g.channels.iter().filter(|c| c.is_text()) {
            if self
                .mutes
                .channel_muted(&g.id, &c.id, c.parent_id.as_deref(), now)
            {
                continue;
            }
            badge.unread |= self.reads.is_unread(&c.id);
            badge.mentions += self.reads.mentions(&c.id);
        }
        badge
    }

    /// Drop typing entries older than 10s. Returns true if anyone is still typing.
    fn expire_typing(&mut self) -> bool {
        for typers in self.typing.values_mut() {
            typers.retain(|t| t.since.elapsed() < TYPING_TIMEOUT);
        }
        self.typing.retain(|_, t| !t.is_empty());
        !self.typing.is_empty()
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        if self.selected_channel.is_none() {
            self.status = "Open a channel before adding files".into();
            return;
        }
        for p in paths {
            if p.is_file() && !self.pending_files.contains(&p) {
                self.pending_files.push(p);
            } else if !p.exists() {
                // Demo mode "files" don't exist on disk; keep them anyway.
                if !self.pending_files.contains(&p) {
                    self.pending_files.push(p);
                }
            }
        }
    }

    // ----- selection -----------------------------------------------------

    fn current_guild(&self) -> Option<&Guild> {
        let id = self.selected_guild.as_deref()?;
        if id == DM_GUILD_ID {
            return Some(&self.dms);
        }
        self.guilds.iter().find(|g| g.id == id)
    }

    /// Demo mode only: open one view for a screenshot (see
    /// `RustCordApp::run_demo_open`).
    fn demo_open(&mut self, view: &str, ctx: &egui::Context, config: &Config) -> bool {
        let guild = self
            .guilds
            .first()
            .map(|g| g.id.clone())
            .unwrap_or_default();
        if view == "friends" || view == "requests" {
            self.select_guild(DM_GUILD_ID.into());
            self.set_home_page(if view == "friends" {
                HomeSelected::Friends
            } else {
                HomeSelected::Requests
            });
            return true;
        }
        self.select_guild(guild.clone());
        let channel = if view == "forum" { "205" } else { "201" };
        self.select_channel(channel.into(), ctx, config);
        let screen = ctx.content_rect();
        match view {
            "search" | "search-empty" | "search-indexing" | "search-error" => {
                self.open_search(true);
                if let Some(s) = &mut self.search {
                    s.panel.demo(view);
                }
            }
            "browser" => self.open_thread_browser(
                "201".into(),
                egui::Rect::from_min_size(
                    egui::pos2(screen.right() - 420.0, 12.0),
                    vec2(24.0, 24.0),
                ),
            ),
            "thread" => {
                if let Some(ts) = &mut self.threads {
                    ts.dialog = Some(ThreadDialog::from_message(
                        "201",
                        "1",
                        "has anyone tried the new layout presets? the compact one is great",
                    ));
                }
            }
            "addserver" => {
                if let Some(s) = &mut self.invites {
                    s.add_server = Some(AddServerDialog::new());
                }
            }
            "pending" => {
                if let Some(g) = self.guilds.first_mut() {
                    g.my_pending = true;
                }
            }
            "roles" | "onboarding" => {
                let _ = self.tx.send(ToBackend::LoadOnboarding {
                    guild_id: guild,
                    page: view == "roles",
                });
            }
            "screening" => self.open_screening(&guild),
            "slow" => self.rate_limited(crate::ratelimit::RateLimited {
                retry_after: Duration::from_secs(12),
                global: false,
            }),
            "picker" => self.demo_picker = true,
            "invite" => {
                let invite = self.messages.get("201").and_then(|ms| {
                    ms.iter()
                        .find(|m| m.content.contains("discord.gg"))
                        .map(|m| m.id.clone())
                });
                match invite {
                    Some(id) => self.jump_to("201".into(), id),
                    None => return false,
                }
            }
            _ => {}
        }
        true
    }

    fn select_guild(&mut self, id: String) {
        if self.selected_guild.as_ref() != Some(&id) {
            self.selected_guild = Some(id);
            self.selected_channel = None;
            self.compose = Compose::New;
        }
    }

    fn select_channel(&mut self, channel_id: String, ctx: &egui::Context, config: &Config) {
        let forum = self
            .find_channel(&channel_id)
            .is_some_and(|(_, c)| c.is_forum());
        if !forum && !self.messages.contains_key(&channel_id) {
            // Never fetch a channel I can't view (no VIEW_CHANNEL).
            if self.can_view_channel(&channel_id) {
                self.channel_errors.remove(&channel_id);
                let _ = self.tx.send(ToBackend::LoadHistory {
                    channel_id: channel_id.clone(),
                });
            } else {
                self.channel_errors
                    .insert(channel_id.clone(), ChannelError::NoAccess);
            }
        }
        if self.selected_channel.as_ref() != Some(&channel_id) {
            self.compose = Compose::New;
            self.pending_files.clear();
            // Red "NEW" line above the first unread message.
            self.new_marker = self
                .reads
                .is_unread(&channel_id)
                .then(|| (channel_id.clone(), self.reads.last_acked(&channel_id)));
        }
        // Opening a channel reads it (user action, so ack right away),
        // except message requests: viewing one never acks it.
        self.held_unread = None;
        if !self.is_request_channel(&channel_id) {
            self.mark_channel_read(&channel_id);
        }

        // Most-recent-first list; free the oldest channels past the limit.
        self.recent_channels.retain(|c| c != &channel_id);
        self.recent_channels.push_front(channel_id.clone());
        while self.recent_channels.len() > config.cached_channels.max(1) {
            if let Some(old) = self.recent_channels.pop_back()
                && let Some(msgs) = self.messages.remove(&old)
            {
                forget_message_images(ctx, &msgs);
                // Parsed markdown of the dropped channel goes too.
                self.md_cache.clear();
            }
        }
        // For the quick switcher's "recent" list.
        self.visited.retain(|c| c != &channel_id);
        self.visited.push_front(channel_id.clone());
        self.visited.truncate(30);
        if self
            .pins
            .as_ref()
            .is_some_and(|(p, _)| p.channel_id != channel_id)
        {
            self.pins = None;
        }
        self.ac.clear();
        self.selected_channel = Some(channel_id);
    }

    fn channel_title(&self) -> Option<String> {
        let cid = self.selected_channel.as_deref()?;
        self.current_guild()?.channel(cid).map(|c| c.label())
    }

    // ----- panes ---------------------------------------------------------

    pub fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane, config: &Config, pal: &Palette) {
        match pane {
            Pane::Servers => self.servers_pane(ui, config, pal),
            Pane::Profile => self.profile_pane(ui, config, pal),
            Pane::Channels => self.channels_pane(ui, config, pal),
            Pane::Messages => self.messages_pane(ui, config, pal),
            Pane::Composer => self.composer_pane(ui, config, pal),
            Pane::Members => self.members_pane(ui, config, pal),
        }
    }

    /// Discord's server rail: round icons, a white pill on the left for
    /// unread / hovered / selected, Home (DMs) at the top, then servers and
    /// folders in the user's synced order. Wider than `RAIL_NAMES_WIDTH`
    /// (the user dragged it), it shows names too.
    fn servers_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let mut ev = RailEvents::default();
        let now = notify::now_iso();
        // Folders and order from synced settings (user accounts).
        let items: Vec<RailItem> = match &self.user_settings {
            Some(us) => sp::ordered_guilds(us, self.guilds.iter().map(|g| g.id.as_str())),
            None => self
                .guilds
                .iter()
                .map(|g| RailItem::Guild(g.id.clone()))
                .collect(),
        };
        egui::ScrollArea::vertical()
            .id_salt("servers")
            .auto_shrink(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let dm_selected = self.selected_guild.as_deref() == Some(DM_GUILD_ID);
                let badge = self.guild_badge(&self.dms, &now);
                let resp = rail_item(
                    ui,
                    RailIcon::Home,
                    "Direct Messages",
                    dm_selected,
                    badge,
                    config,
                    pal,
                );
                if resp.clicked() {
                    ev.clicked = Some(DM_GUILD_ID.to_string());
                }
                egui::Popup::context_menu(&resp)
                    .frame(widgets::menu_frame(pal))
                    .show(|ui| {
                        if widgets::menu_item(
                            ui,
                            Some(icons::CHECKS),
                            "Mark as Read",
                            Tone::Normal,
                            pal,
                        )
                        .clicked()
                        {
                            ev.mark_read = Some(DM_GUILD_ID.to_string());
                        }
                    });
                // Separator under Home, centred under the icons.
                let (sep, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 2.0), egui::Sense::hover());
                let x = if sep.width() >= RAIL_NAMES_WIDTH {
                    sep.left() + 12.0 + 24.0
                } else {
                    sep.center().x
                };
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(egui::pos2(x, sep.center().y), vec2(32.0, 2.0)),
                    1,
                    pal.divider,
                );
                for item in &items {
                    match item {
                        RailItem::Guild(id) => {
                            if let Some(g) = self.guilds.iter().find(|g| &g.id == id) {
                                self.rail_guild(ui, g, &now, &mut ev, config, pal);
                            }
                        }
                        RailItem::Folder {
                            id,
                            name,
                            color,
                            guild_ids,
                        } => {
                            let guilds: Vec<&Guild> = guild_ids
                                .iter()
                                .filter_map(|gid| self.guilds.iter().find(|g| &g.id == gid))
                                .collect();
                            if guilds.is_empty() {
                                continue;
                            }
                            let open = self.open_folders.contains(id);
                            let mut badge = Badge::default();
                            for g in &guilds {
                                let b = self.guild_badge(g, &now);
                                badge.unread |= b.unread;
                                badge.mentions += b.mentions;
                            }
                            let label = name.clone().unwrap_or_else(|| {
                                guilds
                                    .iter()
                                    .map(|g| g.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            });
                            let folder = FolderIcon {
                                name: &label,
                                color: color.map(members::rgb).unwrap_or(pal.accent),
                                guilds: &guilds,
                                open,
                                selected: guilds
                                    .iter()
                                    .any(|g| self.selected_guild.as_ref() == Some(&g.id)),
                                badge: if open { Badge::default() } else { badge },
                            };
                            // Background strip behind an open folder.
                            let bg = ui.painter().add(egui::Shape::Noop);
                            let top = ui.cursor().top();
                            if folder_item(ui, &folder, config, pal).clicked() {
                                ev.toggle_folder = Some(*id);
                            }
                            if open {
                                for g in &guilds {
                                    self.rail_guild(ui, g, &now, &mut ev, config, pal);
                                }
                                let w = ui.available_width();
                                let x = if w >= RAIL_NAMES_WIDTH {
                                    ui.min_rect().left() + 12.0 + 24.0
                                } else {
                                    ui.min_rect().center().x
                                };
                                let rect = egui::Rect::from_x_y_ranges(
                                    x - 28.0..=x + 28.0,
                                    top - 4.0..=ui.cursor().top() - 4.0,
                                );
                                ui.painter().set(
                                    bg,
                                    egui::Shape::rect_filled(
                                        rect,
                                        16,
                                        folder.color.gamma_multiply(0.18),
                                    ),
                                );
                            }
                        }
                    }
                }
                // --- onboarding --- Add a Server.
                if self.invites.is_some() {
                    let wide = ui.available_width() >= RAIL_NAMES_WIDTH;
                    if invites::rail_add_button(ui, pal, wide).clicked() {
                        ev.add_server = true;
                    }
                }
            });
        if ev.add_server
            && let Some(s) = &mut self.invites
        {
            s.add_server = Some(AddServerDialog::new());
        }
        if let Some(gid) = ev.channels_roles {
            let _ = self.tx.send(ToBackend::LoadOnboarding {
                guild_id: gid,
                page: true,
            });
        }
        if let Some(id) = ev.toggle_folder
            && !self.open_folders.remove(&id)
        {
            self.open_folders.insert(id);
        }
        if let Some(id) = ev.mark_read {
            self.mark_guild_read(&id);
        }
        if let Some(leave) = ev.leave {
            self.confirm_leave = Some(leave);
        }
        if let Some(id) = ev.clicked {
            self.select_guild(id);
        }
    }

    /// One server in the rail, with its right-click menu.
    fn rail_guild(
        &self,
        ui: &mut egui::Ui,
        g: &Guild,
        now: &str,
        ev: &mut RailEvents,
        config: &Config,
        pal: &Palette,
    ) {
        let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
        let initials = initials(&g.name);
        let badge = self.guild_badge(g, now);
        let icon = match &g.icon_url {
            Some(url) if config.modules.images => RailIcon::Image(url),
            _ => RailIcon::Initials(&initials),
        };
        let resp = rail_item(ui, icon, &g.name, selected, badge, config, pal);
        if resp.clicked() {
            ev.clicked = Some(g.id.clone());
        }
        egui::Popup::context_menu(&resp)
            .frame(widgets::menu_frame(pal))
            .show(|ui| {
                let any_unread = badge.unread || badge.mentions > 0;
                let tone = if any_unread {
                    Tone::Normal
                } else {
                    Tone::Disabled
                };
                if widgets::menu_item(ui, Some(icons::CHECKS), "Mark as Read", tone, pal).clicked()
                {
                    ev.mark_read = Some(g.id.clone());
                }
                if badge.muted {
                    widgets::menu_note(ui, icons::BELL_SLASH, "Muted", pal);
                }
                widgets::menu_separator(ui, pal);
                if self.invites.is_some()
                    && g.features.iter().any(|f| f == crate::onboarding::FEATURE)
                    && widgets::menu_item(
                        ui,
                        Some(icons::SQUARES_FOUR),
                        "Channels & Roles",
                        Tone::Normal,
                        pal,
                    )
                    .clicked()
                {
                    ev.channels_roles = Some(g.id.clone());
                }
                if widgets::menu_item(ui, Some(icons::ID), "Copy Server ID", Tone::Normal, pal)
                    .clicked()
                {
                    ui.ctx().copy_text(g.id.clone());
                }
                widgets::menu_separator(ui, pal);
                if widgets::menu_item(ui, Some(icons::SIGN_OUT), "Leave Server", Tone::Danger, pal)
                    .clicked()
                {
                    ev.leave = Some((g.id.clone(), g.name.clone()));
                }
            });
    }

    /// Bottom-left user panel (Discord's): avatar 32 with a status ring,
    /// SemiBold name over the custom status / status (ellipsized), and the
    /// gear. The avatar + name area opens the status picker (user
    /// accounts). Narrow panels collapse to avatar + gear (side by side, or
    /// stacked in the icon-only server rail).
    fn profile_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let (name, uid, url) = match &self.me {
            Some(me) => (me.name.clone(), me.id.clone(), me.avatar_url.clone()),
            None => ("…".into(), String::new(), String::new()),
        };
        let my_status = self.my_status();
        let (status_text, dot) = match my_status {
            Some(st) => (status::label(st).to_string(), status::color(st, pal)),
            None => ("Offline".to_string(), pal.offline),
        };
        let custom = self.my_custom_status();
        let line2 = match &custom {
            Some(c) => format!("{} {}", c.emoji_name.as_deref().unwrap_or(""), c.text)
                .trim()
                .to_string(),
            None => status_text.clone(),
        };
        let can_pick = self.user_settings.is_some() && self.connected;
        let full = ui.max_rect();
        let w = full.width();
        let bg = pal.pane_fill(Pane::Profile);
        let mut open = false;
        let mut picked = None;

        // Where things go: (avatar+name hit area, gear button).
        let (hit, gear, show_text) = if w >= 150.0 {
            let gear = egui::Rect::from_center_size(
                egui::pos2(full.right() - 22.0, full.center().y),
                vec2(32.0, 32.0),
            );
            let hit = egui::Rect::from_min_max(
                egui::pos2(full.left(), full.center().y - 21.0),
                egui::pos2(gear.left() - 4.0, full.center().y + 21.0),
            );
            (hit, gear, true)
        } else if w >= 84.0 || full.height() < 84.0 {
            let hit = egui::Rect::from_center_size(
                egui::pos2(full.left() + 22.0, full.center().y),
                vec2(40.0, 40.0),
            );
            let gear = egui::Rect::from_center_size(
                egui::pos2((hit.right() + full.right()) / 2.0, full.center().y),
                vec2(28.0, 28.0),
            );
            (hit, gear, false)
        } else {
            let c = full.center();
            let hit = egui::Rect::from_center_size(c - vec2(0.0, 18.0), vec2(40.0, 40.0));
            let gear = egui::Rect::from_center_size(c + vec2(0.0, 20.0), vec2(28.0, 28.0));
            (hit, gear, false)
        };

        let resp = ui.interact(hit, ui.scope_id().with("me_panel"), egui::Sense::click());
        if resp.hovered() && can_pick {
            ui.painter().rect_filled(hit, 4, pal.hover);
        }
        let av = egui::Rect::from_center_size(
            egui::pos2(hit.left() + 4.0 + 16.0, hit.center().y),
            vec2(32.0, 32.0),
        );
        let av = if show_text {
            av
        } else {
            egui::Rect::from_center_size(hit.center(), vec2(32.0, 32.0))
        };
        paint_avatar(ui, av, &uid, &name, &url, config, pal);
        // Status dot with a ring in the panel colour (Discord's cut-out).
        let ring = if resp.hovered() && can_pick {
            pal.hover
        } else {
            bg
        };
        let c = av.right_bottom() - vec2(3.0, 3.0);
        ui.painter().circle_filled(c, 7.0, ring);
        ui.painter().circle_filled(c, 5.0, dot);
        if show_text {
            let x = av.right() + 8.0;
            let max_w = hit.right() - x - 4.0;
            let n = truncated(ui, &name, theme::bold_font(14.0), pal.strong, max_w);
            let l2 = truncated(
                ui,
                &line2,
                egui::FontId::proportional(12.0),
                pal.muted,
                max_w,
            );
            let total = n.size().y + l2.size().y - 2.0;
            let top = hit.center().y - total / 2.0;
            let nh = n.size().y;
            ui.painter().galley(egui::pos2(x, top), n, pal.strong);
            ui.painter()
                .galley(egui::pos2(x, top + nh - 2.0), l2, pal.muted);
        }
        let resp = if can_pick {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        };
        let resp = resp.on_hover_text(if can_pick {
            format!("{name} · {status_text} · click to set your status")
        } else {
            format!("{name} · {status_text}")
        });
        if can_pick {
            egui::Popup::menu(&resp)
                .align(egui::RectAlign::TOP_START)
                .frame(widgets::menu_frame(pal))
                .show(|ui| {
                    if let Some(a) = status::menu(
                        ui,
                        my_status.unwrap_or(sp::Status::Online),
                        custom.as_ref(),
                        pal,
                    ) {
                        picked = Some(a);
                        ui.close();
                    }
                });
        }
        let mut gear_ui = ui.new_child(egui::UiBuilder::new().max_rect(gear));
        let size = gear.width();
        if widgets::icon_button_sized(
            &mut gear_ui,
            icons::GEAR,
            "User Settings",
            false,
            true,
            size,
            20.0,
            pal,
        )
        .clicked()
        {
            open = true;
        }
        ui.allocate_rect(full, egui::Sense::hover());
        if open {
            self.open_settings = true;
        }
        match picked {
            Some(status::MenuAction::Set(st)) => self.set_status(st),
            Some(status::MenuAction::EditCustom) => {
                self.status_dialog = Some(CustomStatusDialog::new(custom.as_ref()));
            }
            Some(status::MenuAction::ClearCustom) => self.set_custom_status(None),
            None => {}
        }
    }

    /// My status: synced setting (user accounts), else online while connected.
    fn my_status(&self) -> Option<sp::Status> {
        if !self.connected {
            return None;
        }
        Some(
            self.user_settings
                .as_ref()
                .and_then(|s| s.status)
                .unwrap_or(sp::Status::Online),
        )
    }

    /// My custom status, unless it has expired.
    fn my_custom_status(&self) -> Option<sp::CustomStatus> {
        self.user_settings
            .as_ref()?
            .custom_status
            .clone()
            .filter(|c| !c.is_expired(status::now_ms()))
            .filter(|c| !c.text.is_empty() || c.emoji_name.is_some())
    }

    /// Status picked in the menu: save it (PATCH) and show it now (op 3).
    fn set_status(&mut self, st: sp::Status) {
        let custom = self.my_custom_status();
        let Some(us) = &mut self.user_settings else {
            return;
        };
        let b64 = us.patch_status(st);
        let _ = us.apply_patch(&b64);
        let _ = self
            .tx
            .send(ToBackend::PatchUserSettings { settings_b64: b64 });
        let _ = self.tx.send(ToBackend::SetPresence {
            status: st.as_str().into(),
            custom,
        });
    }

    /// Custom status dialog saved (or cleared).
    fn set_custom_status(&mut self, custom: Option<sp::CustomStatus>) {
        let st = self.my_status().unwrap_or(sp::Status::Online);
        let Some(us) = &mut self.user_settings else {
            return;
        };
        let b64 = us.patch_custom_status(custom.as_ref());
        let _ = us.apply_patch(&b64);
        let _ = self
            .tx
            .send(ToBackend::PatchUserSettings { settings_b64: b64 });
        let _ = self.tx.send(ToBackend::SetPresence {
            status: st.as_str().into(),
            custom,
        });
    }

    /// A synced setting changed in Settings: PATCH it (explicit user action).
    pub fn apply_sync_change(&mut self, change: SyncChange) {
        let Some(us) = &mut self.user_settings else {
            return;
        };
        let b64 = match change {
            SyncChange::ExplicitFilter(f) => us.patch_explicit_filter(f),
            SyncChange::AllowGuildDms(on) => us.patch_allow_dms_from_guild_members(on),
            SyncChange::Compact(on) => us.patch_compact(on),
        };
        let _ = us.apply_patch(&b64);
        let _ = self
            .tx
            .send(ToBackend::PatchUserSettings { settings_b64: b64 });
    }

    pub fn user_settings(&self) -> Option<&UserSettings> {
        self.user_settings.as_ref()
    }

    /// The pin header button: open (and load) or close the pins popup.
    fn toggle_pins(&mut self, anchor: egui::Rect) {
        if self.pins.take().is_some() {
            return;
        }
        let Some(cid) = self.selected_channel.clone() else {
            return;
        };
        let name = self
            .find_channel(&cid)
            .map(|(_, c)| {
                if c.is_dm() {
                    format!("@{}", c.name)
                } else {
                    format!("#{}", c.name)
                }
            })
            .unwrap_or_default();
        self.pins = Some((PinsPopup::new(cid.clone(), name), anchor));
        let _ = self.tx.send(ToBackend::LoadPins { channel_id: cid });
    }

    fn channels_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        // The Friends module state is lent out so the DM list can draw the
        // Home header while the channels are borrowed.
        let mut friends = self.friends.take();
        let home = self.channels_list(ui, config, pal, friends.as_mut());
        self.friends = friends;
        if let Some(a) = home {
            let ctx = ui.ctx().clone();
            self.home_action(a, &ctx, config);
        }
    }

    fn channels_list(
        &mut self,
        ui: &mut egui::Ui,
        config: &Config,
        pal: &Palette,
        mut friends: Option<&mut FriendsState>,
    ) -> Option<HomeAction> {
        let Some(guild) = self.current_guild() else {
            let rect = header_bar(ui, pal);
            ui.painter().text(
                rect.left_center() + vec2(16.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "No server selected",
                egui::FontId::proportional(14.5),
                pal.muted,
            );
            return None;
        };
        // Header: server name (DMs: "Direct Messages"), with a border + soft
        // shadow under it, and « to collapse the panel (Discord Messenger).
        let rect = header_bar(ui, pal);
        let collapse = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 24.0, rect.center().y),
            vec2(28.0, 28.0),
        );
        let title = if guild.is_dms() {
            "Direct Messages"
        } else {
            guild.name.as_str()
        };
        let galley = truncated(
            ui,
            title,
            theme::bold_font(15.0),
            pal.strong,
            collapse.left() - rect.left() - 20.0,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0),
            galley,
            pal.strong,
        );
        let mut collapse_ui = ui.new_child(egui::UiBuilder::new().max_rect(collapse));
        let hide = widgets::icon_button_sized(
            &mut collapse_ui,
            icons::CARET_DOUBLE_LEFT,
            "Hide channel list",
            false,
            true,
            28.0,
            16.0,
            pal,
        )
        .clicked();

        let mut clicked = None;
        let mut first = None;
        let mut mark_read = None;
        let mut toggle_cat = None;
        let mut close_dm = None;
        let mut home_action = None;
        let mut thread_action = None;
        let now = notify::now_iso();
        let guild_muted = self.mutes.guild_muted(&guild.id, &now);
        // (`self.friends` is lent out as `friends` here.)
        let home_page = guild.is_dms()
            && friends
                .as_ref()
                .is_some_and(|f| f.page != HomeSelected::None);
        egui::ScrollArea::vertical()
            .id_salt("channels")
            .auto_shrink(false)
            .show(ui, |ui| {
                let groups = guild.channel_groups();
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 8,
                        bottom: 8,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if groups.is_empty() {
                            ui.label(
                                RichText::new(if guild.is_dms() {
                                    "No DMs"
                                } else {
                                    "No text channels"
                                })
                                .color(pal.muted),
                            );
                        }
                        if guild.is_dms() {
                            match friends.as_mut() {
                                // Friends module: search + Friends + Message
                                // Requests above the DMs.
                                Some(fs) => {
                                    let avatar = |ui: &egui::Ui, r: egui::Rect, u: &User| {
                                        paint_avatar(
                                            ui,
                                            r,
                                            &u.id,
                                            &u.name,
                                            &u.avatar_url,
                                            config,
                                            pal,
                                        );
                                    };
                                    let has_dm = |uid: &str| {
                                        guild.channels.iter().any(|c| {
                                            c.kind == CHANNEL_DM
                                                && c.recipients.first().is_some_and(|u| u.id == uid)
                                        })
                                    };
                                    let cx = HomeCtx {
                                        pal,
                                        selected: fs.page,
                                        requests: fs.requests.count(),
                                        pending_friends: fs.rels.incoming_count(),
                                        friends: &fs.rels,
                                        has_dm: &has_dm,
                                        avatar: &avatar,
                                    };
                                    ui.add_space(2.0);
                                    if let Some(a) = home::home_header(ui, &mut fs.home, &cx) {
                                        home_action = Some(a);
                                    }
                                }
                                None => {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(pal.section_label("Direct Messages"));
                                    });
                                    ui.add_space(4.0);
                                }
                            }
                        }
                        for group in groups {
                            let mut collapsed = false;
                            if let Some(cat) = group.category {
                                collapsed = self.collapsed.contains(&cat.id);
                                ui.add_space(12.0);
                                if category_header(ui, &cat.name, collapsed, pal).clicked() {
                                    toggle_cat = Some(cat.id.clone());
                                }
                            }
                            for c in group.channels {
                                // Friends module: requests live on their own
                                // page; the Home search filters the DMs.
                                if guild.is_dms()
                                    && let Some(fs) = friends.as_ref()
                                    && (fs.requests.is_request(&c.id)
                                        || home::filter_dms(&fs.home.query, [c]).is_empty())
                                {
                                    continue;
                                }
                                // Friends / Message Requests page: no DM
                                // is highlighted (the page entry is).
                                let selected = !home_page
                                    && self.selected_channel.as_deref() == Some(c.id.as_str());
                                let muted = guild_muted || self.is_muted(&guild.id, c, &now);
                                let unread = !c.is_voice() && !muted && self.reads.is_unread(&c.id);
                                let mentions = if muted { 0 } else { self.reads.mentions(&c.id) };
                                // Collapsed categories still show the open
                                // channel and unread ones, like Discord.
                                if collapsed && !selected && !unread && mentions == 0 {
                                    continue;
                                }
                                // Never auto-open a message request.
                                if !c.is_voice() && !c.is_request {
                                    first.get_or_insert_with(|| c.id.clone());
                                }
                                let status = (c.kind == CHANNEL_DM && config.modules.member_list)
                                    .then(|| c.recipients.first())
                                    .flatten()
                                    .and_then(|u| self.presences.status(&u.id));
                                let activity = (c.kind == CHANNEL_DM && config.modules.member_list)
                                    .then(|| c.recipients.first())
                                    .flatten()
                                    .and_then(|u| self.presences.get(&u.id))
                                    .and_then(|p| p.activity.as_deref().map(str::to_string));
                                let row = ChannelRow {
                                    selected,
                                    muted,
                                    unread,
                                    mentions,
                                    status,
                                    activity,
                                };
                                let (resp, close) = channel_row(ui, c, &row, config, pal);
                                if close {
                                    close_dm = Some(c.id.clone());
                                } else if resp.clicked() && !c.is_voice() {
                                    clicked = Some(c.id.clone());
                                }
                                if c.is_voice() {
                                    continue;
                                }
                                egui::Popup::context_menu(&resp)
                                    .frame(widgets::menu_frame(pal))
                                    .show(|ui| {
                                        let tone = if unread || mentions > 0 {
                                            Tone::Normal
                                        } else {
                                            Tone::Disabled
                                        };
                                        if widgets::menu_item(
                                            ui,
                                            Some(icons::CHECKS),
                                            "Mark as Read",
                                            tone,
                                            pal,
                                        )
                                        .clicked()
                                        {
                                            mark_read = Some(c.id.clone());
                                        }
                                        if muted {
                                            widgets::menu_note(ui, icons::BELL_SLASH, "Muted", pal);
                                        }
                                        widgets::menu_separator(ui, pal);
                                        if c.is_dm()
                                            && widgets::menu_item(
                                                ui,
                                                Some(icons::X),
                                                "Close DM",
                                                Tone::Normal,
                                                pal,
                                            )
                                            .clicked()
                                        {
                                            close_dm = Some(c.id.clone());
                                        }
                                        if widgets::menu_item(
                                            ui,
                                            Some(icons::ID),
                                            "Copy Channel ID",
                                            Tone::Normal,
                                            pal,
                                        )
                                        .clicked()
                                        {
                                            ui.ctx().copy_text(c.id.clone());
                                        }
                                        if let Some(u) =
                                            c.recipients.first().filter(|_| c.kind == CHANNEL_DM)
                                            && widgets::menu_item(
                                                ui,
                                                Some(icons::USER),
                                                "Copy User ID",
                                                Tone::Normal,
                                                pal,
                                            )
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(u.id.clone());
                                        }
                                    });
                                // --- threads --- rows under their channel.
                                if !guild.is_dms()
                                    && let Some(ts) = self.threads.as_ref()
                                {
                                    let reads = &self.reads;
                                    let names = &self.user_names;
                                    let is_unread = |id: &str| reads.is_unread(id);
                                    let user_name = |id: &str| names.get(id).cloned();
                                    let tctx = threads::Ctx {
                                        pal,
                                        selected: self.selected_channel.as_deref(),
                                        is_unread: &is_unread,
                                        user_name: &user_name,
                                        load_images: config.modules.images,
                                        can_manage_threads: false,
                                    };
                                    if let Some(a) =
                                        threads::thread_rows(ui, &c.id, &ts.store, &tctx)
                                    {
                                        thread_action = Some(a);
                                    }
                                }
                            }
                        }
                    });
            });

        if let Some(id) = toggle_cat
            && !self.collapsed.remove(&id)
        {
            self.collapsed.insert(id);
        }
        if let Some(id) = mark_read {
            self.mark_channel_read(&id);
        }
        if hide {
            self.toggle_channels = true;
        }
        if let Some(id) = close_dm {
            // Explicit user action (× on the row / menu), like Discord's.
            let _ = self.tx.send(ToBackend::CloseDm {
                channel_id: id.clone(),
            });
            self.dms.channels.retain(|c| c.id != id);
            if self.selected_channel.as_deref() == Some(id.as_str()) {
                self.selected_channel = None;
            }
            return None;
        }
        let auto = if self.selected_channel.is_none() {
            first
        } else {
            None
        };
        if clicked.is_some()
            && let Some(fs) = friends
        {
            fs.page = HomeSelected::None;
        }
        if let Some(a) = thread_action {
            let ctx = ui.ctx().clone();
            self.thread_action(a, &ctx, config);
        }
        if let Some(id) = clicked.or(auto) {
            let ctx = ui.ctx().clone();
            self.select_channel(id, &ctx, config);
        }
        home_action
    }

    /// Chat header: channel icon + bold name, topic, and the inbox / pins / members
    /// buttons on the right.
    fn chat_header(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let rect = header_bar(ui, pal);
        let channel = self
            .selected_channel
            .as_deref()
            .and_then(|id| self.current_guild()?.channel(id))
            .map(|c| (c.icon(), c.name.clone(), c.topic.clone(), c.is_dm()));
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        let ui = &mut inner;
        ui.spacing_mut().item_spacing.x = 4.0;
        // Right-hand buttons (right to left).
        if config.modules.member_list
            && icon_button(
                ui,
                icons::USERS,
                if self.members_visible {
                    "Hide Member List"
                } else {
                    "Show Member List"
                },
                self.members_visible,
                true,
                pal,
            )
            .clicked()
        {
            self.toggle_members = true;
        }
        // --- threads --- browser for server text channels.
        let thread_parent = self
            .selected_channel
            .as_deref()
            .and_then(|id| self.find_channel(id))
            .filter(|(g, c)| !g.is_dms() && matches!(c.kind, 0 | 5))
            .map(|(_, c)| c.id.clone());
        if self.threads.is_some()
            && let Some(cid) = thread_parent
        {
            let open = self.threads.as_ref().is_some_and(|t| t.browser.is_some());
            let btn = icon_button(ui, icons::CHATS, "Threads", open, true, pal);
            if btn.clicked() {
                if open {
                    if let Some(ts) = &mut self.threads {
                        ts.browser = None;
                    }
                } else {
                    self.open_thread_browser(cid, btn.rect);
                }
            }
        }
        if config.modules.search {
            let btn = icon_button(
                ui,
                icons::MAGNIFYING_GLASS,
                "Search (Ctrl+F)",
                self.search.is_some(),
                channel.is_some(),
                pal,
            );
            if btn.clicked() && channel.is_some() {
                if self.search.is_some() {
                    self.search = None;
                } else {
                    self.open_search(true);
                }
            }
            if let Some(s) = &mut self.search {
                s.anchor = btn.rect;
            }
        }
        let pin_btn = icon_button(
            ui,
            icons::PUSH_PIN,
            "Pinned Messages",
            self.pins.is_some(),
            channel.is_some(),
            pal,
        );
        if pin_btn.clicked() && channel.is_some() {
            self.toggle_pins(pin_btn.rect);
        }
        if let Some((_, anchor)) = &mut self.pins {
            *anchor = pin_btn.rect;
        }
        if config.modules.notifications {
            self.inbox_button(ui, config, pal);
        }
        ui.add_space(8.0);
        let show_channels = !self.channels_visible;
        let Some((icon, name, topic, dm)) = channel else {
            if show_channels {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if icon_button(
                        ui,
                        icons::CARET_DOUBLE_RIGHT,
                        "Show channel list",
                        false,
                        true,
                        pal,
                    )
                    .clicked()
                    {
                        self.toggle_channels = true;
                    }
                });
            }
            return;
        };
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if show_channels {
                // » brings back the channel list hidden with «.
                if icon_button(
                    ui,
                    icons::CARET_DOUBLE_RIGHT,
                    "Show channel list",
                    false,
                    true,
                    pal,
                )
                .clicked()
                {
                    self.toggle_channels = true;
                }
            } else {
                ui.add_space(4.0);
            }
            ui.label(RichText::new(icon).size(22.0).color(pal.muted));
            ui.add(
                egui::Label::new(RichText::new(&name).bold().size(15.0).color(pal.strong))
                    .truncate()
                    .selectable(false),
            );
            if !topic.is_empty() && !dm && ui.available_width() > 60.0 {
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, 24.0), egui::Sense::hover());
                ui.painter().rect_filled(sep, 0, pal.divider);
                let first_line = topic.lines().next().unwrap_or_default();
                ui.add(
                    egui::Label::new(RichText::new(first_line).size(13.0).color(pal.muted))
                        .truncate()
                        .selectable(false),
                )
                .on_hover_text(&topic);
            }
        });
    }

    fn messages_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        if self.home_page_open() {
            self.home_page(ui, config, pal);
            return;
        }
        if self.threads.is_some() && self.selected_is_forum() {
            self.forum_page(ui, config, pal);
            return;
        }
        self.chat_header(ui, config, pal);
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Pick a channel").color(pal.muted))
            });
            return;
        };
        let Some(count) = self.messages.get(&channel_id).map(Vec::len) else {
            match self.channel_error(&channel_id) {
                // Minimal empty state; the design (doc §4.1) is the UI's.
                Some(err) => {
                    let (title, sub) = match err {
                        ChannelError::NoAccess => {
                            let name = self
                                .find_channel(&channel_id)
                                .map(|(_, c)| c.name.clone())
                                .unwrap_or_default();
                            (
                                format!("You don't have access to #{name}"),
                                "Ask a server admin for the role that can see it.".to_string(),
                            )
                        }
                        ChannelError::Failed(e) => ("Couldn't load messages".into(), e.clone()),
                    };
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() / 2.0 - 48.0).max(0.0));
                        ui.label(RichText::new(icons::LOCK).size(48.0).color(pal.muted));
                        ui.label(RichText::new(title).size(16.0).color(pal.strong));
                        ui.label(RichText::new(sub).color(pal.muted));
                    });
                }
                None => {
                    ui.centered_and_justified(|ui| ui.spinner());
                }
            }
            return;
        };
        if count == 0 {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No messages yet").color(pal.muted))
            });
            return;
        }

        let formatting = !config.disable_formatting;
        let my_id = self.me.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        self.refresh_emoji_groups();
        let skin_tone = Cell::new(config.skin_tone);
        // Server nicknames and role colours for author names (not DMs).
        let guild_idx = self
            .guilds
            .iter()
            .position(|g| g.channel(&channel_id).is_some());

        // Parse (cached) message bodies and reply snippets first: that needs
        // the cache mutably, drawing needs the rest of the state.
        type Parsed = (Arc<[Block]>, Option<String>, Option<Arc<[Block]>>);
        let parsed: Vec<Parsed> = {
            let lookup = ChatLookup {
                guild: guild_idx.map(|i| &self.guilds[i]),
                guilds: &self.guilds,
                dms: &self.dms,
                members: &self.member_cache,
                names: &self.user_names,
            };
            let cache = &mut self.md_cache;
            self.messages[&channel_id]
                .iter()
                .map(|m| {
                    let blocks = cache.get(&m.id, &m.content, formatting);
                    let reply = m.reply.as_ref().map(|r| {
                        if r.content.is_empty() {
                            "Click to see attachment".to_string()
                        } else {
                            let b = cache.get(&format!("reply:{}", m.id), &r.content, formatting);
                            markdown::to_plain_text(&b, &lookup).replace('\n', " ")
                        }
                    });
                    let fwd = m
                        .forwarded
                        .as_ref()
                        .filter(|f| !f.content.is_empty())
                        .map(|f| cache.get(&format!("fwd:{}", m.id), &f.content, formatting));
                    (blocks, reply, fwd)
                })
                .collect()
        };

        let guild = guild_idx.map(|i| &self.guilds[i]);
        let lookup = ChatLookup {
            guild,
            guilds: &self.guilds,
            dms: &self.dms,
            members: &self.member_cache,
            names: &self.user_names,
        };
        let menu = MenuCtx {
            mine: false,
            guild_id: guild.map(|g| g.id.clone()),
            can_pin: self.can_pin(&channel_id),
            can_thread: self.threads.is_some()
                && guild.is_some_and(|g| !g.is_dms())
                && self
                    .find_channel(&channel_id)
                    .is_some_and(|(_, c)| matches!(c.kind, 0 | 5))
                && self.has_perm(&channel_id, perms::CREATE_PUBLIC_THREADS),
        };
        let guilds = &self.guilds;
        let dms = &self.dms;
        let is_member = |id: &str| guilds.iter().any(|g| g.id == id) || dms.channel(id).is_some();
        let is_bot = self.me.as_ref().is_some_and(|u| u.bot);
        let invite_parts = self.invites.as_ref().map(|s| InviteParts {
            cache: &s.cache,
            is_member: &is_member,
            can_join: !is_bot,
            my_id: &my_id,
        });
        let react_parts = self.reactions.as_ref().map(|state| ReactParts {
            images: config.modules.emoji_images,
            can_react: self.has_perm(&channel_id, perms::ADD_REACTIONS)
                && self.slow_down_secs().is_none(),
            custom: self.emoji_groups(),
            recent: &config.recent_emoji,
            skin_tone: &skin_tone,
            state,
        });
        let messages = &self.messages[&channel_id];
        let mut action = None;
        let scroll_id = self
            .scroll_to
            .as_ref()
            .filter(|(c, _)| *c == channel_id)
            .map(|(_, m)| m.clone());
        let mut scrolled = false;
        let flash = self
            .highlight
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(2))
            .map(|(id, _)| id.clone());
        if flash.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }

        let mut new_after = self
            .new_marker
            .as_ref()
            .filter(|(c, _)| *c == channel_id)
            .map(|(_, acked)| *acked);
        let hide_blocked = !config.show_blocked_messages;

        egui::ScrollArea::vertical()
            .id_salt(("messages", &channel_id))
            .auto_shrink(false)
            .stick_to_bottom(scroll_id.is_none())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Message text at 15 px (Discord: 16 px at 100%).
                ui.style_mut()
                    .text_styles
                    .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
                ui.add_space(8.0);
                let mut prev: Option<&Message> = None;
                let mut revealed_run = false;
                let mut i = 0;
                while i < messages.len() {
                    let m = &messages[i];
                    let day = timefmt::local_day(m.time);
                    let new_day =
                        day.is_some() && prev.and_then(|p| timefmt::local_day(p.time)) != day;
                    // Discord groups messages by the same person sent within 7 minutes.
                    let mut grouped = m.reply.is_none()
                        && !new_day
                        && !config.compact_messages
                        && prev.is_some_and(|p| {
                            sysmsg::is_normal(p.kind)
                                && p.author.id == m.author.id
                                && (m.time - p.time).abs() < 7 * 60
                        });
                    if let (true, Some(day)) = (new_day, day) {
                        date_divider(ui, &timefmt::day_label(day), pal);
                    }
                    if let Some(acked) = new_after
                        && m.id.parse::<u64>().is_ok_and(|id| id > acked)
                        && m.author.id != my_id
                    {
                        new_divider(ui, pal);
                        new_after = None;
                        grouped = false;
                    }

                    // Blocked people: one "N blocked messages — Show" line
                    // per run, unless shown.
                    let blocked = self.blocked.contains(&m.author.id);
                    if !blocked {
                        revealed_run = false;
                    } else if hide_blocked && !revealed_run && scroll_id.as_ref() != Some(&m.id) {
                        if self.revealed_blocked.contains(&m.id) {
                            revealed_run = true;
                        } else {
                            let start = i;
                            while i < messages.len()
                                && self.blocked.contains(&messages[i].author.id)
                            {
                                i += 1;
                            }
                            if blocked_row(ui, i - start, pal) {
                                action = Some(MessageAction::ShowBlocked(m.id.clone()));
                            }
                            prev = None;
                            continue;
                        }
                    }

                    let author = self.author_style(guild, &m.author);
                    // Joins, pins, calls, boosts…: one muted line.
                    if !sysmsg::is_normal(m.kind) {
                        let target = m.mention_names.first().map(|(_, n)| n.as_str());
                        let server = guild.map_or("", |g| g.name.as_str());
                        if let Some(line) = sysmsg::system_line(
                            m.kind,
                            author.name,
                            target,
                            &m.content,
                            server,
                            &m.id,
                        ) && let Some(a) = system_row(ui, m, &line, pal)
                        {
                            action = Some(a);
                        }
                        prev = Some(m);
                        i += 1;
                        continue;
                    }
                    let mentions_me = !my_id.is_empty()
                        && m.author.id != my_id
                        && (m.mention_everyone || m.mentions.contains(&my_id));
                    let menu = MenuCtx {
                        mine: m.author.id == my_id,
                        guild_id: menu.guild_id.clone(),
                        can_pin: menu.can_pin,
                        can_thread: menu.can_thread,
                    };
                    let rc = RowCtx {
                        grouped,
                        mentions_me,
                        compact: config.compact_messages,
                        highlight: flash.as_ref() == Some(&m.id),
                        menu: &menu,
                        reactions: react_parts.as_ref(),
                        invites: invite_parts.as_ref(),
                    };
                    let body = Body {
                        blocks: &parsed[i].0,
                        reply: parsed[i].1.as_deref(),
                        forwarded: parsed[i].2.as_deref(),
                        lookup: &lookup,
                    };
                    let (a, rect) = message_row(ui, m, &body, &author, &rc, config, pal);
                    if a.is_some() {
                        action = a;
                    }
                    if scroll_id.as_ref() == Some(&m.id) {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        scrolled = true;
                    }
                    prev = Some(m);
                    i += 1;
                }
                ui.add_space(20.0);
            });
        if scrolled && let Some((_, id)) = self.scroll_to.take() {
            self.highlight = Some((id, Instant::now()));
        }

        // Toast ("Saved …") over the bottom of the chat.
        // While rate limited the toast counts down instead of fading.
        if let Some(secs) = self.slow_down_secs() {
            self.toast = Some((format!("Slow down, try again in {secs}s"), Instant::now()));
        } else if self.slow_down_until.take().is_some() {
            self.toast = None;
        }
        if let Some((text, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            } else {
                let rect = ui.clip_rect();
                let slow = self.slow_down_until.is_some();
                let galley = ui.painter().layout(
                    text.clone(),
                    egui::FontId::proportional(13.0),
                    pal.strong,
                    (rect.width() - 80.0).max(120.0),
                );
                // Icon, text, padding; the rate-limit countdown gets a timer.
                let icon = if slow { icons::TIMER } else { icons::INFO };
                let icon_color = if slow { pal.idle } else { pal.muted };
                let size = galley.size() + vec2(24.0 + 24.0, 16.0);
                let r = egui::Rect::from_center_size(
                    egui::pos2(rect.center().x, rect.bottom() - 16.0 - size.y / 2.0),
                    size,
                );
                let painter = ui.painter();
                painter.add(
                    egui::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: Color32::from_black_alpha(if pal.dark { 90 } else { 35 }),
                    }
                    .as_shape(r, 8),
                );
                painter.rect(
                    r,
                    8,
                    pal.menu_bg(),
                    egui::Stroke::new(1.0, pal.divider.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    egui::pos2(r.left() + 20.0, r.center().y),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    icons::font(16.0),
                    icon_color,
                );
                painter.galley(
                    egui::pos2(r.left() + 34.0, r.center().y - galley.size().y / 2.0),
                    galley,
                    pal.strong,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
        }

        let ctx = ui.ctx().clone();
        match action {
            Some(MessageAction::Reply(id, author)) => {
                self.compose = Compose::Reply {
                    message_id: id,
                    author,
                    ping: config.mention_on_reply,
                };
                self.focus_composer = true;
            }
            Some(MessageAction::Edit(id, content)) => {
                self.draft = content;
                self.compose = Compose::Edit { message_id: id };
                self.focus_composer = true;
            }
            Some(MessageAction::Delete(id)) => self.confirm_delete = Some((channel_id, id)),
            Some(MessageAction::Copy(text)) => ctx.copy_text(text),
            Some(MessageAction::MarkUnread(id)) => self.mark_unread(&channel_id, &id),
            Some(MessageAction::Profile(user, rect)) => {
                let guild = self
                    .find_channel(&channel_id)
                    .filter(|(g, _)| !g.is_dms())
                    .map(|(g, _)| g.id.clone());
                self.open_profile(user, guild, rect, &ctx);
            }
            Some(MessageAction::OpenLink(url)) => self.open_link(&url, &ctx, config),
            Some(MessageAction::Mention(user_id, rect)) => {
                let user = self.known_user(&channel_id, &user_id);
                let guild = guild_idx.map(|i| self.guilds[i].id.clone());
                self.open_profile(user, guild, rect, &ctx);
            }
            Some(MessageAction::GoToChannel(cid)) => self.go_to_channel(&cid, &ctx, config),
            Some(MessageAction::Pin(id, pin)) => self.confirm_pin = Some((channel_id, id, pin)),
            Some(MessageAction::SaveImage(url, filename)) => {
                let _ = self.tx.send(ToBackend::SaveFile { url, filename });
            }
            Some(MessageAction::ShowBlocked(id)) => {
                self.revealed_blocked.insert(id);
            }
            Some(MessageAction::JumpTo(id)) => self.jump_to(channel_id.clone(), id),
            Some(MessageAction::Invite(a)) => self.invite_action(a, &ctx, config),
            Some(MessageAction::CreateThread(id, content)) => {
                if let Some(ts) = &mut self.threads {
                    ts.dialog = Some(ThreadDialog::from_message(&channel_id, &id, &content));
                }
            }
            Some(MessageAction::React(id, a)) => {
                let now = ctx.input(|i| i.time);
                self.react(&channel_id, &id, a, now);
            }
            None => {}
        }
    }

    /// Open a link: straight away if trusted (or warnings are off),
    /// otherwise via the "Leaving RustCord" dialog.
    fn open_link(&mut self, url: &str, ctx: &egui::Context, config: &Config) {
        match links::check(url, config.warn_before_links, &config.trusted_domains) {
            LinkCheck::Open(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
            LinkCheck::Ask(w) => self.link_warning = Some(w),
        }
    }

    /// Scroll to a message, loading the messages around it if needed.
    fn jump_to(&mut self, channel_id: String, message_id: String) {
        let loaded = self
            .messages
            .get(&channel_id)
            .is_some_and(|l| l.iter().any(|m| m.id == message_id));
        if loaded {
            self.scroll_to = Some((channel_id, message_id));
        } else {
            let _ = self.tx.send(ToBackend::LoadAround {
                channel_id,
                message_id,
            });
        }
    }

    /// Switch to a channel by id (channel mentions, quick switcher, pins).
    fn go_to_channel(&mut self, channel_id: &str, ctx: &egui::Context, config: &Config) {
        let Some((g, c)) = self.find_channel(channel_id) else {
            self.toast = Some((
                "You don't have access to that channel".into(),
                Instant::now(),
            ));
            return;
        };
        if !c.is_text() {
            return;
        }
        let gid = g.id.clone();
        self.select_guild(gid);
        self.select_channel(channel_id.to_string(), ctx, config);
    }

    /// The best `User` we have for an id (mention clicks).
    fn known_user(&self, channel_id: &str, user_id: &str) -> User {
        if let Some(u) = self
            .messages
            .get(channel_id)
            .and_then(|l| l.iter().rev().find(|m| m.author.id == user_id))
        {
            return u.author.clone();
        }
        if let Some(u) = self
            .dms
            .channels
            .iter()
            .flat_map(|c| &c.recipients)
            .find(|u| u.id == user_id)
        {
            return u.clone();
        }
        let name = self
            .user_names
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| "unknown-user".into());
        User {
            id: user_id.to_string(),
            username: name.to_lowercase(),
            avatar_url: crate::types::avatar_url(user_id, None),
            name,
            bot: false,
        }
    }

    /// May I pin / unpin in this channel? DMs: yes. Servers: Manage
    /// Messages, from my roles and the channel's overwrites.
    fn can_pin(&self, channel_id: &str) -> bool {
        self.has_perm(channel_id, perms::MANAGE_MESSAGES)
    }

    /// May I see this channel (VIEW_CHANNEL from roles + overwrites)? DMs
    /// and channels whose server data isn't known yet count as visible.
    /// The channel list hides channels where this is false.
    pub fn can_view_channel(&self, channel_id: &str) -> bool {
        let Some((g, c)) = self.find_channel(channel_id) else {
            return true;
        };
        if g.is_dms() {
            return true;
        }
        let Some(me) = &self.me else { return true };
        perms::can_view_channel(
            &g.id,
            &g.owner_id,
            &g.roles,
            &c.overwrites,
            &perms::Me {
                user_id: &me.id,
                roles: &g.my_roles,
            },
        )
    }

    /// Why the channel's messages aren't shown, if they can't be.
    pub fn channel_error(&self, channel_id: &str) -> Option<&ChannelError> {
        self.channel_errors.get(channel_id)
    }

    /// My permission `bit` in a channel (always true in DMs).
    fn has_perm(&self, channel_id: &str, bit: u64) -> bool {
        let Some((g, c)) = self.find_channel(channel_id) else {
            return false;
        };
        if g.is_dms() {
            return true;
        }
        let Some(me) = &self.me else { return false };
        let p = perms::channel_permissions(
            &g.id,
            &g.owner_id,
            &g.roles,
            &c.overwrites,
            &perms::Me {
                user_id: &me.id,
                roles: &g.my_roles,
            },
        );
        p & bit != 0
    }

    /// Markdown → one line of plain text for notifications and the inbox.
    fn plain_text(&self, content: &str, guild_id: &str) -> String {
        let lookup = ChatLookup {
            guild: self.guilds.iter().find(|g| g.id == guild_id),
            guilds: &self.guilds,
            dms: &self.dms,
            members: &self.member_cache,
            names: &self.user_names,
        };
        markdown::to_plain_text(&markdown::parse(content), &lookup)
    }

    /// Inbox button with a red badge when there are unread mentions; opens the
    /// inbox popup. Clicking a mention jumps to its channel.
    fn inbox_button(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let unread = self.inbox.unread();
        let bell = icon_button(ui, icons::TRAY, "Inbox", false, true, pal);
        if unread > 0 {
            mention_badge(
                ui,
                bell.rect.right_bottom() - vec2(1.0, 4.0),
                egui::Align2::RIGHT_BOTTOM,
                unread as u32,
                pal.chat,
                pal,
            );
        }
        let mut jump = None;
        let mut clear = false;
        egui::Popup::menu(&bell)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .frame(widgets::menu_frame(pal))
            .show(|ui| {
                ui.set_width(340.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(icons::label(icons::TRAY, "Inbox"))
                            .bold()
                            .size(16.0)
                            .color(pal.strong),
                    );
                    ui.label(RichText::new("Mentions").size(13.0).color(pal.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.inbox.items.is_empty() && ui.small_button("Clear").clicked() {
                            clear = true;
                        }
                    });
                });
                ui.separator();
                if self.inbox.items.is_empty() {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No recent mentions").color(pal.muted));
                    });
                    ui.add_space(12.0);
                    return;
                }
                let mut rows = |ui: &mut egui::Ui| {
                    for item in &self.inbox.items {
                        if inbox_row(ui, item, pal).clicked() {
                            jump = Some((item.guild_id.clone(), item.channel_id.clone()));
                        }
                    }
                };
                if self.inbox.items.len() <= 4 {
                    rows(ui);
                } else {
                    // Inside a popup the available height starts tiny, so give
                    // the scroll area an explicit height.
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(380.0)
                        .max_height(380.0)
                        .show(ui, rows);
                }
            });
        if clear {
            self.inbox.items.clear();
        }
        if let Some((gid, cid)) = jump {
            egui::Popup::close_all(ui.ctx());
            self.select_guild(gid);
            let ctx = ui.ctx().clone();
            self.select_channel(cid, &ctx, config);
        }
    }

    fn composer_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        if self.home_page_open() || self.selected_is_forum() {
            return;
        }
        // --- onboarding --- rules screening not done: no composer, just the
        // "Complete these steps" banner (sending is disabled).
        if self.invites.is_some()
            && let Some(gid) = self
                .current_guild()
                .filter(|g| g.my_pending)
                .map(|g| g.id.clone())
        {
            ui.add_space(8.0);
            if crate::screening::pending_banner(ui, pal) {
                self.open_screening(&gid);
            }
            return;
        }
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.add_space(12.0);
            ui.label(RichText::new("Pick a channel to start chatting").color(pal.muted));
            return;
        };

        let mut cancel = false;
        let mut send = false;
        let mut changed = false;
        let mut remove = None;
        let mut toggle_ping = false;
        let mut box_rect = egui::Rect::NOTHING;
        let mut edit_id = None;
        let mut cursor = None;
        let mut has_focus = false;
        let mut composer_action = None;
        let mut emoji_btn: Option<egui::Response> = None;
        let mut toggle_spoiler = None;
        self.refresh_emoji_groups();

        // Autocomplete keys go first so the text box doesn't see them.
        let ac_on = config.modules.autocomplete;
        let mut accept = None;
        if ac_on && self.ac.active() {
            let n = self.ac.items.len();
            let (up, down, tab, enter, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if down {
                self.ac.selected = (self.ac.selected + 1) % n;
            }
            if up {
                self.ac.selected = (self.ac.selected + n - 1) % n;
            }
            if tab || enter {
                accept = Some(self.ac.selected);
            }
            if esc {
                self.ac.dismissed = self.ac.query.as_ref().map(|q| q.start);
                self.ac.clear();
            }
        }

        // Built bottom-up (like Discord) so the input row always stays visible
        // and extra rows (files, reply banner) grow upwards.
        let used = ui
            .with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Who's typing (under the box)
                let typers: Vec<&str> = config
                    .modules
                    .typing_indicators
                    .then(|| self.typing.get(&channel_id))
                    .flatten()
                    .map(|t| t.iter().map(|t| t.name.as_str()).collect())
                    .unwrap_or_default();
                typing_row(ui, &typers, pal);

                // (+) [ message …               ] ➤
                let reply_banner = !matches!(self.compose, Compose::New);
                let mut box_frame = egui::Frame::new()
                    .fill(pal.input)
                    .corner_radius(8)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 10,
                        bottom: 10,
                    });
                if reply_banner {
                    box_frame = box_frame.corner_radius(egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 8,
                        se: 8,
                    });
                }
                box_rect = box_frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if plus_button(ui, pal).clicked() {
                                let _ = self.tx.send(ToBackend::PickFiles);
                            }
                            ui.add_space(8.0);
                            let can_send =
                                !self.draft.trim().is_empty() || !self.pending_files.is_empty();
                            let hint = match self
                                .find_channel(&channel_id)
                                .map(|(_, c)| (c.is_dm(), c.name.clone()))
                            {
                                Some((true, name)) => format!("Message @{name}"),
                                Some((false, name)) => format!("Message #{name}"),
                                None => "Message".into(),
                            };
                            // Right-hand buttons: [GIF] [emoji] [send].
                            let n_buttons =
                                1 + usize::from(config.modules.gif_picker) + usize::from(can_send);
                            let send_w = n_buttons as f32 * (28.0 + ui.spacing().item_spacing.x);
                            let out = egui::TextEdit::singleline(&mut self.draft)
                                .hint_text(RichText::new(hint).color(pal.muted))
                                .font(egui::FontId::proportional(15.0))
                                .frame(egui::Frame::NONE)
                                .margin(egui::Margin::ZERO)
                                .desired_width(ui.available_width() - send_w)
                                .show(ui);
                            let resp = out.response;
                            edit_id = Some(resp.id);
                            cursor = out.cursor_range.map(|r| r.primary.index.0);
                            has_focus = resp.has_focus();
                            if std::mem::take(&mut self.focus_composer) {
                                resp.request_focus();
                            }
                            changed = resp.changed();
                            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                cancel = true;
                            }
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if config.modules.gif_picker {
                                let open = self.gif_picker.is_some();
                                let gif = widgets::icon_button_sized(
                                    ui,
                                    icons::GIF,
                                    "GIFs",
                                    open,
                                    true,
                                    28.0,
                                    24.0,
                                    pal,
                                );
                                if gif.clicked() {
                                    composer_action =
                                        Some(ComposerAction::ToggleGifPicker(gif.rect));
                                }
                            }
                            let emoji = widgets::icon_button_sized(
                                ui,
                                icons::SMILEY,
                                "Emoji",
                                self.emoji_picker_open,
                                true,
                                28.0,
                                22.0,
                                pal,
                            );
                            emoji_btn = Some(emoji);
                            // Discord has no send button; a subtle one shows only
                            // when there's something to send.
                            let send_btn = can_send && send_button(ui, pal).clicked();
                            if enter || send_btn {
                                send = true;
                                resp.request_focus();
                            }
                        });
                    })
                    .response
                    .rect;

                // Reply / edit banner, attached to the top of the box.
                let (banner, ping) = match &self.compose {
                    Compose::New => (None, None),
                    Compose::Reply { author, ping, .. } => {
                        (Some(("Replying to ", author.clone())), Some(*ping))
                    }
                    Compose::Edit { .. } => (Some(("Editing message", String::new())), None),
                };
                if let Some((what, who)) = banner {
                    egui::Frame::new()
                        .fill(pal.sidebar)
                        .corner_radius(egui::CornerRadius {
                            nw: 8,
                            ne: 8,
                            sw: 0,
                            se: 0,
                        })
                        .inner_margin(egui::Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label(RichText::new(what).size(13.0).color(pal.muted));
                                if !who.is_empty() {
                                    ui.label(RichText::new(who).size(13.0).bold().color(pal.text));
                                } else {
                                    ui.label(
                                        RichText::new(" · Esc to cancel")
                                            .size(13.0)
                                            .color(pal.muted),
                                    );
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let x = ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(icons::X)
                                                        .size(16.0)
                                                        .color(pal.muted),
                                                )
                                                .frame(false),
                                            )
                                            .on_hover_text("Cancel (Esc)");
                                        cancel |= x.clicked();
                                        if let Some(on) = ping {
                                            ui.add_space(10.0);
                                            // Discord's "@ ON" switch: ping the author?
                                            let (text, col) = if on {
                                                ("@ ON", pal.accent)
                                            } else {
                                                ("@ OFF", pal.muted)
                                            };
                                            let chip = ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(text)
                                                            .size(12.0)
                                                            .bold()
                                                            .color(col),
                                                    )
                                                    .frame(false),
                                                )
                                                .on_hover_text(if on {
                                                    "Mentioning the author (click to turn off)"
                                                } else {
                                                    "Not mentioning the author (click to turn on)"
                                                });
                                            toggle_ping = chip.clicked();
                                            ui.add_space(6.0);
                                        }
                                    },
                                );
                            });
                        });
                }
            })
            .response
            .rect;

        // Files waiting to upload: Discord's attachment cards above the box
        // (floating over the chat, so the message box keeps its height),
        // with spoiler (eye) and remove (trash) buttons. The message text is
        // the comment.
        if !self.pending_files.is_empty() {
            let w = box_rect.width();
            egui::Area::new(egui::Id::unique("rc_upload_cards"))
                .order(egui::Order::Middle)
                .pivot(egui::Align2::LEFT_BOTTOM)
                .fixed_pos(egui::pos2(box_rect.left(), used.top() - 6.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(pal.input)
                        .corner_radius(8)
                        .inner_margin(10)
                        .shadow(egui::Shadow {
                            offset: [0, -2],
                            blur: 8,
                            spread: 0,
                            color: Color32::from_black_alpha(if pal.dark { 60 } else { 20 }),
                        })
                        .show(ui, |ui| {
                            ui.set_width(w - 20.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                                let mut total = 0;
                                for (i, p) in self.pending_files.iter().enumerate() {
                                    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                                    total += size;
                                    let spoiler = self.pending_spoilers.contains(p);
                                    let (t, r) = upload_card(ui, p, size, spoiler, pal);
                                    if t {
                                        toggle_spoiler = Some(p.clone());
                                    }
                                    if r {
                                        remove = Some(i);
                                    }
                                }
                                if total > UPLOAD_LIMIT {
                                    ui.label(
                                        RichText::new("Over 10 MB, Discord may reject this")
                                            .small()
                                            .color(pal.danger),
                                    );
                                }
                            });
                        });
                });
        }

        if let Some(i) = remove {
            let p = self.pending_files.remove(i);
            self.pending_spoilers.remove(&p);
        }
        if let Some(p) = toggle_spoiler
            && !self.pending_spoilers.remove(&p)
        {
            self.pending_spoilers.insert(p);
        }
        match composer_action {
            Some(ComposerAction::ToggleGifPicker(anchor)) => {
                if let Some((picker, _)) = self.gif_picker.take() {
                    picker.free(ui.ctx());
                } else {
                    self.gif_picker = Some((GifPicker::default(), anchor));
                }
            }
            None => {}
        }
        // Emoji picker (with the Symbols tab) on the composer's emoji button.
        if let Some(btn) = emoji_btn {
            if std::mem::take(&mut self.demo_picker) {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&btn));
            }
            let tone = Cell::new(config.skin_tone);
            let picked = emoji_picker::popup(
                &btn,
                &PickerCtx {
                    pal,
                    images: config.modules.emoji_images,
                    custom: self.emoji_groups(),
                    recent: &config.recent_emoji,
                    allow_text: true,
                    skin_tone: &tone,
                },
            );
            self.emoji_picker_open =
                egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&btn));
            if tone.get() != config.skin_tone {
                self.config_updates.push(ConfigUpdate::SkinTone(tone.get()));
            }
            if let Some(p) = picked {
                if let Picked::Emoji(e) = &p {
                    self.config_updates
                        .push(ConfigUpdate::RecentEmoji(e.clone()));
                }
                let text = p.insert_text();
                let at = cursor
                    .map(|c| {
                        self.draft
                            .char_indices()
                            .nth(c)
                            .map_or(self.draft.len(), |(i, _)| i)
                    })
                    .unwrap_or(self.draft.len());
                self.draft.insert_str(at, &text);
                self.focus_composer = true;
                changed = true;
            }
        }
        if toggle_ping && let Compose::Reply { ping, .. } = &mut self.compose {
            *ping = !*ping;
        }

        // Autocomplete: a clicked row, or Tab / Enter from above.
        if ac_on {
            if self.ac.active()
                && let Some(i) = autocomplete::popup(ui.ctx(), box_rect, &self.ac, pal)
            {
                accept = Some(i);
            }
            if let (Some(i), Some(q)) = (accept, self.ac.query.clone())
                && let Some(item) = self.ac.items.get(i)
            {
                let insert = item.insert.clone();
                let at = autocomplete::apply(&mut self.draft, &q, &insert);
                if let Some(id) = edit_id
                    && let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id)
                {
                    let c = egui::text::CCursor::new(at);
                    st.cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(c)));
                    st.store(ui.ctx(), id);
                    ui.ctx().memory_mut(|m| m.request_focus(id));
                }
                self.ac.clear();
            } else if has_focus {
                self.update_autocomplete(&channel_id, cursor);
            } else {
                self.ac.clear();
            }
        } else if self.ac.query.is_some() {
            self.ac = Autocomplete::default();
        }

        if cancel {
            if matches!(self.compose, Compose::Edit { .. }) {
                self.draft.clear();
            }
            self.compose = Compose::New;
        }
        if changed && !self.draft.is_empty() && config.modules.typing_indicators {
            self.maybe_send_typing(&channel_id);
        }
        if send {
            self.submit(channel_id);
        }
    }

    /// The member list: Discord's grouped list for servers (op 14 for user
    /// accounts, the full list for bots with the Server Members intent),
    /// people in the conversation for DMs, and "recently active" otherwise.
    fn members_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        // Same 48 px header strip as the chat, so the shadow line runs
        // across (Discord's header spans chat + members).
        let header = header_bar(ui, pal);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                right: 4,
                top: 8,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.members_inner(ui, header, config, pal);
            });
    }

    fn members_inner(
        &mut self,
        ui: &mut egui::Ui,
        header: egui::Rect,
        config: &Config,
        pal: &Palette,
    ) {
        if !config.modules.member_list {
            ui.label(
                RichText::new("Member list is off (Settings → Modules)")
                    .small()
                    .color(pal.muted),
            );
            return;
        }
        let Some(channel_id) = self.selected_channel.clone() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let guild = self
            .find_channel(&channel_id)
            .filter(|(g, _)| !g.is_dms())
            .map(|(g, _)| g.id.clone());
        let is_bot = self.is_bot_session();

        // Drop a list that belongs to another server: only the visible one is kept.
        if self
            .member_list
            .as_ref()
            .is_some_and(|l| Some(&l.guild_id) != guild.as_ref())
            && let Some(old) = self.member_list.take()
        {
            self.forget_list_avatars(&ctx, &old);
        }
        if self
            .bot_list
            .as_ref()
            .is_some_and(|b| Some(&b.guild_id) != guild.as_ref())
            && let Some(old) = self.bot_list.take()
        {
            self.forget_list_avatars(&ctx, &old.list);
        }
        if let Some(gid) = &guild
            && is_bot
            && config.bot_member_intents
            && self.bot_list_requested.as_ref() != Some(gid)
        {
            self.bot_list_requested = Some(gid.clone());
            let _ = self.tx.send(ToBackend::RequestMemberList {
                guild_id: gid.clone(),
            });
        }
        if let Some(b) = self.bot_list.as_mut().filter(|b| b.dirty) {
            let roles = self
                .guilds
                .iter()
                .find(|g| g.id == b.guild_id)
                .map(|g| g.roles.as_slice())
                .unwrap_or(&[]);
            b.list.rows = members::build_rows(&b.members, roles, &self.presences);
            b.dirty = false;
        }

        let roles: &[members::Role] = guild
            .as_ref()
            .and_then(|gid| self.guilds.iter().find(|g| &g.id == gid))
            .map(|g| g.roles.as_slice())
            .unwrap_or(&[]);
        let list = self
            .member_list
            .as_ref()
            .or(self.bot_list.as_ref().map(|b| &b.list))
            .filter(|l| !l.rows.is_empty());

        // Fallback rows: DM recipients, or people who spoke recently.
        let fallback: Vec<Option<Row>> = if list.is_some() {
            Vec::new()
        } else {
            let (group, people): (&str, Vec<User>) = match self.dms.channel(&channel_id) {
                Some(dm) => {
                    let mut people = dm.recipients.clone();
                    people.extend(self.me.clone());
                    ("dm", people)
                }
                None => {
                    let mut seen: Vec<User> = Vec::new();
                    for m in self.messages.get(&channel_id).into_iter().flatten().rev() {
                        if !seen.iter().any(|u| u.id == m.author.id) {
                            seen.push(m.author.clone());
                        }
                    }
                    ("recent", seen)
                }
            };
            let mut rows = vec![Some(Row::Group {
                id: group.into(),
                count: people.len() as u32,
            })];
            rows.extend(people.into_iter().map(|user| {
                let cached = guild
                    .as_deref()
                    .and_then(|g| self.member_cache.get(g, &user.id));
                Some(Row::Member(Box::new(Member {
                    nick: cached.and_then(|c| c.nick.clone()),
                    roles: cached.map(|c| c.roles.clone()).unwrap_or_default(),
                    user,
                    ..Member::default()
                })))
            }));
            rows
        };

        let row_fn = |i: usize| match list {
            Some(l) => l.row(i),
            None => fallback.get(i).and_then(Option::as_ref),
        };
        let total = list.map_or(fallback.len(), |l| l.total_rows().min(100_000));
        let title = match list.filter(|l| l.member_count > 0) {
            Some(l) => format!("Members — {} · {} online", l.member_count, l.online_count),
            None => "Members".to_string(),
        };
        let galley = truncated(
            ui,
            &title,
            theme::bold_font(14.0),
            pal.muted,
            header.width() - 32.0,
        );
        ui.painter().galley(
            egui::pos2(
                header.left() + 16.0,
                header.center().y - galley.size().y / 2.0,
            ),
            galley,
            pal.muted,
        );
        let (clicked, visible) = members::list_ui(
            ui,
            members::ListView {
                rows: &row_fn,
                total,
                roles,
                presences: &self.presences,
            },
            config,
            pal,
        );

        // Op 14 (user accounts): subscribe to this channel, with ranges for
        // what's on screen so scrolling loads more of the list.
        if let Some(gid) = &guild
            && !is_bot
            && self.connected
        {
            let ranges = members::ranges_for(visible.start, visible.end.saturating_sub(1));
            let want = (gid.clone(), channel_id.clone(), ranges);
            if self.subscribed.as_ref() != Some(&want) {
                let _ = self.tx.send(ToBackend::SubscribeMembers {
                    guild_id: want.0.clone(),
                    channel_id: want.1.clone(),
                    ranges: want.2.clone(),
                });
                self.subscribed = Some(want);
            }
        }
        if let Some(c) = clicked {
            self.open_profile(c.member.user, guild, c.rect, &ctx);
        }
    }

    /// Recompute composer suggestions for the word at the cursor.
    fn update_autocomplete(&mut self, channel_id: &str, cursor: Option<usize>) {
        let q = cursor.and_then(|c| autocomplete::find_query(&self.draft, c));
        let Some(q) = q else {
            self.ac.clear();
            self.ac.dismissed = None;
            return;
        };
        if self.ac.dismissed == Some(q.start) {
            return;
        }
        if self.ac.query.as_ref() == Some(&q) {
            return; // Nothing changed.
        }
        let guild = self.guilds.iter().find(|g| g.channel(channel_id).is_some());
        let items = match q.trigger {
            Trigger::User => {
                autocomplete::rank_named(&q.text, self.mention_candidates(channel_id, guild))
            }
            Trigger::Channel => {
                let candidates = guild
                    .map(|g| {
                        g.channels
                            .iter()
                            .filter(|c| c.is_text())
                            .map(|c| Candidate {
                                name: c.name.clone(),
                                detail: c
                                    .parent_id
                                    .as_deref()
                                    .and_then(|p| g.channel(p))
                                    .map(|p| p.name.to_uppercase())
                                    .unwrap_or_default(),
                                insert: format!("<#{}>", c.id),
                                color: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut items = autocomplete::rank_named(&q.text, candidates);
                for i in &mut items {
                    i.glyph = icons::HASH.into();
                }
                items
            }
            Trigger::Emoji => autocomplete::emoji(&q.text, guild.map_or(&[], |g| &g.emojis)),
        };
        let same_start = self.ac.query.as_ref().is_some_and(|o| o.start == q.start);
        if !same_start {
            self.ac.selected = 0;
        }
        self.ac.selected = self.ac.selected.min(items.len().saturating_sub(1));
        self.ac.items = items;
        self.ac.query = Some(q);
    }

    /// People (and roles, @everyone / @here) for `@` suggestions: the member
    /// list, cached members, message authors and DM recipients.
    fn mention_candidates(&self, channel_id: &str, guild: Option<&Guild>) -> Vec<Candidate> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut person = |out: &mut Vec<Candidate>, id: &str, name: &str, username: &str| {
            if id.is_empty() || !seen.insert(id.to_string()) {
                return;
            }
            let roles = guild
                .and_then(|g| self.member_cache.get(&g.id, id))
                .map(|m| m.roles.as_slice())
                .unwrap_or(&[]);
            out.push(Candidate {
                name: name.to_string(),
                detail: username.to_string(),
                insert: format!("<@{id}>"),
                color: guild.and_then(|g| members::role_color(&g.roles, roles)),
            });
        };
        let lists = self
            .member_list
            .iter()
            .chain(self.bot_list.as_ref().map(|b| &b.list));
        for list in lists {
            for row in list.rows.iter().flatten() {
                if let Row::Member(m) = row {
                    person(&mut out, &m.user.id, m.display_name(), &m.user.username);
                }
            }
        }
        for m in self.messages.get(channel_id).into_iter().flatten().rev() {
            let name = guild
                .and_then(|g| self.member_cache.get(&g.id, &m.author.id))
                .and_then(|c| c.nick.as_deref())
                .unwrap_or(&m.author.name);
            person(&mut out, &m.author.id, name, &m.author.username);
        }
        if let Some(dm) = self.dms.channel(channel_id) {
            for u in &dm.recipients {
                person(&mut out, &u.id, &u.name, &u.username);
            }
        }
        if let Some(g) = guild {
            for (id, m) in self.member_cache.guild(&g.id) {
                if let Some(name) = m
                    .nick
                    .as_deref()
                    .or(self.user_names.get(id).map(String::as_str))
                {
                    person(&mut out, id, name, "");
                }
            }
            for r in g.roles.iter().filter(|r| r.id != g.id) {
                out.push(Candidate {
                    name: r.name.clone(),
                    detail: "role".into(),
                    insert: format!("<@&{}>", r.id),
                    color: r.color32(),
                });
            }
            for e in ["everyone", "here"] {
                out.push(Candidate {
                    name: e.into(),
                    detail: if e == "here" {
                        "Notify everyone online".into()
                    } else {
                        "Notify everyone in this channel".into()
                    },
                    insert: format!("@{e}"),
                    color: None,
                });
            }
        }
        out
    }

    /// Tell Discord we're typing, at most every 8 seconds per channel.
    fn maybe_send_typing(&mut self, channel_id: &str) {
        let recently = self
            .last_typing_sent
            .as_ref()
            .is_some_and(|(cid, at)| cid == channel_id && at.elapsed() < TYPING_RESEND);
        if !recently {
            let _ = self.tx.send(ToBackend::Typing {
                channel_id: channel_id.to_string(),
            });
            self.last_typing_sent = Some((channel_id.to_string(), Instant::now()));
        }
    }

    fn submit(&mut self, channel_id: String) {
        let content = self.draft.trim().to_string();
        let files = std::mem::take(&mut self.pending_files);
        let spoiler_set = std::mem::take(&mut self.pending_spoilers);
        let spoilers: Vec<bool> = files.iter().map(|p| spoiler_set.contains(p)).collect();
        if content.is_empty() && files.is_empty() {
            return;
        }
        let req = match std::mem::replace(&mut self.compose, Compose::New) {
            Compose::Edit { message_id } => ToBackend::Edit {
                channel_id,
                message_id,
                content,
            },
            Compose::New => ToBackend::Send {
                channel_id,
                content,
                reply_to: None,
                reply_ping: false,
                files,
                spoilers,
            },
            Compose::Reply {
                message_id, ping, ..
            } => ToBackend::Send {
                channel_id,
                content,
                reply_to: Some(message_id),
                reply_ping: ping,
                files,
                spoilers,
            },
        };
        let _ = self.tx.send(req);
        self.draft.clear();
        self.last_typing_sent = None;
    }

    /// Status bar module: a thin bar at the bottom like Discord Messenger's
    /// (connection, where you are, who's typing, RAM).
    fn status_bar(&self, ui: &mut egui::Ui, config: &Config, pal: &Palette, ram: Option<u64>) {
        // Discord Messenger's segmented status bar: connection | where you
        // are | unread mentions … typing | RAM, each with a line icon.
        egui::Panel::bottom("status_bar")
            .exact_size(24.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.servers)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let small = |t: &str| RichText::new(t).size(12.0).color(pal.muted);
                    let seg = |ui: &mut egui::Ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(9.0, 14.0), egui::Sense::hover());
                        ui.painter().vline(
                            r.center().x,
                            r.y_range(),
                            egui::Stroke::new(1.0, pal.divider),
                        );
                    };
                    let (icon, col) = if self.connected {
                        (icons::WIFI, pal.online)
                    } else {
                        (icons::WIFI_SLASH, pal.idle)
                    };
                    ui.label(RichText::new(icon).size(14.0).color(col));
                    ui.label(small(&self.status));
                    if let Some(cid) = &self.selected_channel
                        && let Some((g, c)) = self.find_channel(cid)
                    {
                        seg(ui);
                        ui.label(RichText::new(c.icon()).size(14.0).color(pal.muted));
                        let place = if g.is_dms() {
                            c.name.clone()
                        } else {
                            format!("{} · {}", c.name, g.name)
                        };
                        ui.add(egui::Label::new(small(&place)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(bytes) = ram {
                            ui.label(small(&format!("{:.0} MB", bytes as f64 / 1_048_576.0)));
                            ui.label(RichText::new(icons::MEMORY).size(14.0).color(pal.muted))
                                .on_hover_text("RAM used by RustCord");
                            ui.ctx().request_repaint_after(Duration::from_secs(2));
                        }
                        let mentions = self.inbox.unread();
                        if mentions > 0 {
                            seg(ui);
                            ui.label(small(&format!(
                                "{mentions} mention{}",
                                if mentions == 1 { "" } else { "s" }
                            )));
                            ui.label(RichText::new(icons::AT).size(14.0).color(pal.mention));
                        }
                        let typing = config
                            .modules
                            .typing_indicators
                            .then_some(self.selected_channel.as_ref())
                            .flatten()
                            .and_then(|c| self.typing.get(c))
                            .filter(|t| !t.is_empty());
                        if let Some(t) = typing {
                            seg(ui);
                            let text = match t.as_slice() {
                                [one] => format!("{} is typing…", one.name),
                                _ => format!("{} people are typing…", t.len()),
                            };
                            ui.label(small(&text));
                            ui.label(RichText::new(icons::PENCIL).size(14.0).color(pal.muted));
                        }
                    });
                });
            });
    }

    /// Profile card, pins, dialogs and the "drop files here" overlay.
    fn overlays(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let ctx = ui.ctx().clone();
        self.picker_overlays(&ctx, config, pal);
        self.popout_overlay(&ctx, config, pal);
        self.pins_overlay(&ctx, config, pal);
        self.dialogs(&ctx, config, pal);

        // Drag-and-drop hint
        let hovering_files = ui.input(|i| !i.raw.hovered_files.is_empty());
        if hovering_files {
            let rect = ui.ctx().content_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop_overlay"),
            ));
            painter.rect_filled(rect, 0, Color32::from_black_alpha(170));
            let text = match self.channel_title() {
                Some(t) => format!("Drop to upload to {t}"),
                None => "Open a channel first".into(),
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(24.0),
                pal.text,
            );
        }

        let Some((channel_id, message_id)) = self.confirm_delete.clone() else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("confirm_delete")).show(ui.ctx(), |ui| {
            ui.heading("Delete Message");
            ui.label("Are you sure you want to delete this message?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("Delete").color(Color32::WHITE))
                            .fill(pal.danger),
                    )
                    .clicked()
                {
                    let _ = self.tx.send(ToBackend::Delete {
                        channel_id: channel_id.clone(),
                        message_id: message_id.clone(),
                    });
                    close = true;
                }
            });
        });
        if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_delete = None;
        }
    }
}

impl State {
    /// GIF picker and the emoji picker placeholder above the message box.
    fn picker_overlays(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        if (!config.modules.gif_picker || self.selected_channel.is_none())
            && let Some((p, _)) = self.gif_picker.take()
        {
            p.free(ctx);
        }
        if let Some((picker, anchor)) = &mut self.gif_picker {
            match picker.show(ctx, *anchor, config.modules.images, pal) {
                Some(GifAction::Load(query)) => {
                    let _ = self.tx.send(ToBackend::LoadGifs { query });
                }
                Some(GifAction::Pick(url)) => {
                    // The user clicked this GIF: send its link, like Discord.
                    if let Some(cid) = self.selected_channel.clone() {
                        let _ = self.tx.send(ToBackend::Send {
                            channel_id: cid,
                            content: url,
                            reply_to: None,
                            reply_ping: false,
                            files: Vec::new(),
                            spoilers: Vec::new(),
                        });
                    }
                    if let Some((p, _)) = self.gif_picker.take() {
                        p.free(ctx);
                    }
                }
                Some(GifAction::Close) => {
                    if let Some((p, _)) = self.gif_picker.take() {
                        p.free(ctx);
                    }
                }
                None => {}
            }
        }
    }

    /// Link warning, pin / leave confirmations, custom status, Ctrl+K.
    fn dialogs(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        self.shortcuts_dialog(ctx, pal);
        if config.modules.search {
            self.search_popup(ctx, config, pal);
        }
        self.thread_dialogs(ctx, config, pal);
        self.invite_dialogs(ctx, config, pal);
        if let Some(w) = &mut self.link_warning
            && let Some(decision) = w.show(ctx, pal)
        {
            if let LinkDecision::Visit { url, trust } = decision {
                if let Some(domain) = trust {
                    self.config_updates.push(ConfigUpdate::TrustDomain(domain));
                }
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            self.link_warning = None;
        }

        if let Some(dialog) = &mut self.status_dialog
            && let Some(result) = dialog.show(ctx, pal)
        {
            self.status_dialog = None;
            match result {
                DialogResult::Save(c) => self.set_custom_status(Some(c)),
                DialogResult::Clear => self.set_custom_status(None),
                DialogResult::Cancel => {}
            }
        }

        if let Some((cid, mid, pin)) = self.confirm_pin.clone() {
            let preview = self
                .messages
                .get(&cid)
                .and_then(|l| l.iter().find(|m| m.id == mid))
                .map(|m| {
                    let text = self.plain_text(&m.content, m.guild_id.as_deref().unwrap_or(""));
                    format!("{}: {}", m.author.name, notify::preview(&text, 120))
                })
                .unwrap_or_default();
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("rc_confirm_pin")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(if pin { "Pin It. Pin It Good." } else { "Unpin Message" });
                ui.label(if pin {
                    "Hey, just double checking that you want to pin this message to the channel for posterity and greatness?"
                } else {
                    "You sure you want to remove this pinned message?"
                });
                if !preview.is_empty() {
                    egui::Frame::new()
                        .fill(pal.input)
                        .corner_radius(4)
                        .inner_margin(8)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(&preview).color(pal.muted));
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let (label, fill) = if pin {
                        ("Pin It", pal.accent)
                    } else {
                        ("Remove it please!", pal.danger)
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(fill))
                        .clicked()
                    {
                        let _ = self.tx.send(ToBackend::SetPinned {
                            channel_id: cid.clone(),
                            message_id: mid.clone(),
                            pinned: pin,
                        });
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.confirm_pin = None;
            }
        }

        if let Some((gid, name)) = self.confirm_leave.clone() {
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("rc_confirm_leave")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(format!("Leave '{name}'"));
                ui.label(format!(
                    "Are you sure you want to leave {name}? You won't be able to rejoin this server unless you are re-invited."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Leave Server").color(Color32::WHITE))
                                .fill(pal.danger),
                        )
                        .clicked()
                    {
                        let _ = self.tx.send(ToBackend::LeaveGuild { guild_id: gid.clone() });
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.confirm_leave = None;
            }
        }

        if self.switcher.is_some() {
            let entries = self.switcher_entries();
            let Some(sw) = &mut self.switcher else { return };
            let results = quick_switcher::rank(&entries, &sw.query);
            match sw.show(ctx, &results, pal) {
                Some(quick_switcher::Action::Close) => self.switcher = None,
                Some(quick_switcher::Action::Go(e)) => {
                    self.switcher = None;
                    match e.channel_id {
                        Some(cid) => self.go_to_channel(&cid, ctx, config),
                        None => self.select_guild(e.guild_id),
                    }
                }
                None => {}
            }
        }
    }

    /// Discord's keyboard shortcuts (Ctrl+/ lists them):
    /// Alt+↑/↓ previous / next channel, Alt+Shift+↑/↓ previous / next
    /// unread channel, Ctrl+Alt+↑/↓ previous / next server, Ctrl+, settings.
    fn shortcuts(&mut self, ctx: &egui::Context, config: &Config) {
        use egui::{Key, KeyboardShortcut as K, Modifiers as M};
        let alt = M::ALT;
        let alt_shift = M::ALT | M::SHIFT;
        let ctrl_alt = M::COMMAND | M::ALT;
        let (prev, next, prev_unread, next_unread, prev_server, next_server, settings, help) = ctx
            .input_mut(|i| {
                (
                    i.consume_shortcut(&K::new(alt, Key::ArrowUp)),
                    i.consume_shortcut(&K::new(alt, Key::ArrowDown)),
                    i.consume_shortcut(&K::new(alt_shift, Key::ArrowUp)),
                    i.consume_shortcut(&K::new(alt_shift, Key::ArrowDown)),
                    i.consume_shortcut(&K::new(ctrl_alt, Key::ArrowUp)),
                    i.consume_shortcut(&K::new(ctrl_alt, Key::ArrowDown)),
                    i.consume_shortcut(&K::new(M::COMMAND, Key::Comma)),
                    i.consume_shortcut(&K::new(M::COMMAND, Key::Slash)),
                )
            });
        if settings {
            self.open_settings = true;
        }
        if help {
            self.show_shortcuts = !self.show_shortcuts;
        }
        let step = |back: bool| if back { -1 } else { 1 };
        if prev || next {
            self.step_channel(step(prev), false, ctx, config);
        }
        if prev_unread || next_unread {
            self.step_channel(step(prev_unread), true, ctx, config);
        }
        if prev_server || next_server {
            let mut ids = vec![DM_GUILD_ID.to_string()];
            ids.extend(self.guilds.iter().map(|g| g.id.clone()));
            let cur = self
                .selected_guild
                .as_ref()
                .and_then(|g| ids.iter().position(|x| x == g))
                .unwrap_or(0) as i64;
            let n = ids.len() as i64;
            let to = (cur + step(prev_server)).rem_euclid(n) as usize;
            self.select_guild(ids[to].clone());
        }
    }

    /// Move to the previous / next (unread) text channel of the open server.
    fn step_channel(&mut self, dir: i64, unread_only: bool, ctx: &egui::Context, config: &Config) {
        let Some(g) = self.current_guild() else {
            return;
        };
        let ids: Vec<String> = g
            .channel_groups()
            .iter()
            .flat_map(|grp| grp.channels.iter())
            .filter(|c| !c.is_voice())
            .map(|c| c.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len() as i64;
        let cur = self
            .selected_channel
            .as_ref()
            .and_then(|c| ids.iter().position(|x| x == c))
            .map_or(if dir > 0 { -1 } else { 0 }, |p| p as i64);
        for k in 1..=n {
            let id = &ids[(cur + dir * k).rem_euclid(n) as usize];
            if !unread_only || self.reads.is_unread(id) || self.reads.mentions(id) > 0 {
                let id = id.clone();
                self.select_channel(id, ctx, config);
                return;
            }
        }
    }

    /// Ctrl+/: the keyboard shortcuts list.
    fn shortcuts_dialog(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.show_shortcuts {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("rc_shortcuts")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new(icons::label(icons::KEYBOARD, "Keyboard Shortcuts"))
                    .bold()
                    .size(18.0)
                    .color(pal.strong),
            );
            ui.add_space(8.0);
            let rows = [
                ("Ctrl + K", "Quick switcher"),
                ("Ctrl + F", "Search this channel"),
                ("Ctrl + Shift + F", "Search the server / DM"),
                ("Alt + ↑ / ↓", "Previous / next channel"),
                ("Alt + Shift + ↑ / ↓", "Previous / next unread channel"),
                ("Ctrl + Alt + ↑ / ↓", "Previous / next server"),
                ("Ctrl + ,", "User settings"),
                ("Ctrl + S", "Save settings (in Settings)"),
                ("Enter / Esc", "Send / cancel reply or edit"),
                ("Tab / Enter", "Accept a suggestion (@, #, :)"),
                ("Ctrl + /", "Show this list"),
            ];
            egui::Grid::new("rc_shortcut_grid")
                .num_columns(2)
                .spacing(vec2(24.0, 8.0))
                .show(ui, |ui| {
                    for (keys, what) in rows {
                        egui::Frame::new()
                            .fill(pal.input)
                            .corner_radius(4)
                            .inner_margin(egui::Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.label(RichText::new(keys).bold().size(13.0).color(pal.strong));
                            });
                        ui.label(RichText::new(what).color(pal.text));
                        ui.end_row();
                    }
                });
        });
        if modal.should_close() {
            self.show_shortcuts = false;
        }
    }

    /// Ctrl+K.
    pub fn toggle_switcher(&mut self) {
        self.switcher = match self.switcher {
            Some(_) => None,
            None => Some(QuickSwitcher::default()),
        };
    }

    /// Everything the quick switcher can go to.
    fn switcher_entries(&self) -> Vec<quick_switcher::Entry> {
        use quick_switcher::{Entry, Kind};
        let recent = |cid: &str| self.visited.iter().position(|c| c == cid);
        let mut out = Vec::new();
        for g in &self.guilds {
            let mut guild_recent = None;
            for c in g.channels.iter().filter(|c| c.is_text()) {
                let r = recent(&c.id);
                guild_recent = match (guild_recent, r) {
                    (Some(a), Some(b)) => Some(std::cmp::min(a, b)),
                    (a, b) => a.or(b),
                };
                out.push(Entry {
                    kind: Kind::Channel,
                    name: c.name.clone(),
                    detail: g.name.clone(),
                    guild_id: g.id.clone(),
                    channel_id: Some(c.id.clone()),
                    recent: r,
                });
            }
            out.push(Entry {
                kind: Kind::Server,
                name: g.name.clone(),
                detail: String::new(),
                guild_id: g.id.clone(),
                channel_id: None,
                // Servers rank just below their most recent channel.
                recent: guild_recent.map(|r| r + 1),
            });
        }
        for c in &self.dms.channels {
            out.push(Entry {
                kind: Kind::Dm,
                name: c.name.clone(),
                detail: "Direct Message".into(),
                guild_id: DM_GUILD_ID.into(),
                channel_id: Some(c.id.clone()),
                recent: recent(&c.id),
            });
        }
        out
    }

    /// The pinned messages popup under the 📌 button.
    fn pins_overlay(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        let Some(channel_id) = self.pins.as_ref().map(|(p, _)| p.channel_id.clone()) else {
            return;
        };
        let can_unpin = self.can_pin(&channel_id);
        let guild = self
            .guilds
            .iter()
            .find(|g| g.channel(&channel_id).is_some());
        let members = &self.member_cache;
        let author = |u: &User| -> (String, Option<Color32>) {
            let cached = guild.and_then(|g| members.get(&g.id, &u.id));
            (
                cached
                    .and_then(|c| c.nick.clone())
                    .unwrap_or_else(|| u.name.clone()),
                cached.and_then(|c| members::role_color(&guild?.roles, &c.roles)),
            )
        };
        let view = PinsView {
            pal,
            config,
            can_unpin,
            author: Some(&author),
        };
        let Some((popup, anchor)) = &mut self.pins else {
            return;
        };
        let anchor = *anchor;
        let mut action = None;
        let area = egui::Area::new(egui::Id::unique("rc_pins_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(anchor.right() - 440.0, anchor.bottom() + 6.0))
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    action = popup.show_contents(ui, &view);
                });
            });
        // Click outside (but not on the 📌 button, which toggles) or Esc closes.
        let outside = ctx.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p))
        });
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if outside || esc {
            self.pins = None;
        }
        match action {
            Some(PinsAction::Jump {
                channel_id,
                message_id,
            }) => {
                self.pins = None;
                if self.selected_channel.as_ref() != Some(&channel_id) {
                    self.go_to_channel(&channel_id, ctx, config);
                }
                self.jump_to(channel_id, message_id);
            }
            // Confirmed in the popup.
            Some(PinsAction::Unpin {
                channel_id,
                message_id,
            }) => {
                let _ = self.tx.send(ToBackend::SetPinned {
                    channel_id,
                    message_id,
                    pinned: false,
                });
            }
            Some(PinsAction::Retry { channel_id }) => {
                if let Some((p, _)) = &mut self.pins {
                    p.set_loading();
                }
                let _ = self.tx.send(ToBackend::LoadPins { channel_id });
            }
            None => {}
        }
    }

    fn popout_overlay(&mut self, ctx: &egui::Context, config: &Config, pal: &Palette) {
        let is_bot_session = self.is_bot_session();
        let Some(pop) = &mut self.popout else {
            return;
        };
        let uid = pop.user.id.clone();
        let gid = pop.guild_id.clone();
        let roles = gid
            .as_ref()
            .and_then(|g| self.guilds.iter().find(|x| &x.id == g))
            .map_or(&[][..], |g| g.roles.as_slice());
        let profile = self.profiles.get(&uid, gid.as_deref());
        let data = profile::CardData {
            profile,
            loading: profile.is_none() && !is_bot_session && !self.profiles.is_unavailable(&uid),
            presence: config
                .modules
                .member_list
                .then(|| self.presences.get(&uid))
                .flatten(),
            member: gid.as_deref().and_then(|g| self.member_cache.get(g, &uid)),
            roles,
            note: self.profiles.note(&uid),
            is_bot_session,
        };
        match profile::popout_ui(ctx, pop, data, config, pal) {
            Some(PopoutAction::Close) => self.close_popout(ctx),
            Some(PopoutAction::SaveNote(note)) => {
                // Explicit user action (Save button / Ctrl+Enter).
                let _ = self.tx.send(ToBackend::SetNote {
                    user_id: uid.clone(),
                    note: note.clone(),
                });
                if let Some(p) = &mut self.popout {
                    p.note_draft = Some(note.clone());
                }
                self.profiles.set_note(&uid, note);
            }
            None => {}
        }
    }
}

/// Free the GPU/CPU copies of attachment and embed images for messages we
/// no longer keep.
fn forget_message_images(ctx: &egui::Context, messages: &[Message]) {
    let (w, h) = IMAGE_MAX;
    for m in messages {
        for a in m.attachments.iter().filter(|a| a.is_image()) {
            ctx.forget_image(&a.thumbnail_url(w as u32, h as u32));
        }
        for e in &m.embeds {
            for uri in e.image_uris() {
                ctx.forget_image(&uri);
            }
        }
        let fwd = m.forwarded.as_ref();
        for st in m
            .stickers
            .iter()
            .chain(fwd.into_iter().flat_map(|f| &f.stickers))
        {
            if let Some(url) = st.image_url(STICKER_SIZE as u32) {
                ctx.forget_image(&url);
            }
        }
        for a in fwd
            .into_iter()
            .flat_map(|f| &f.attachments)
            .filter(|a| a.is_image())
        {
            ctx.forget_image(&a.thumbnail_url(w as u32, h as u32));
        }
    }
}

// ===========================================================================
// Widgets
// ===========================================================================

/// Round avatar. With the Images module off, a coloured circle with the
/// first letter instead (no download, no texture).
pub(crate) fn avatar(
    ui: &mut egui::Ui,
    user_id: &str,
    name: &str,
    url: &str,
    size: f32,
    config: &Config,
    pal: &Palette,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint_avatar(ui, rect, user_id, name, url, config, pal);
    rect
}

/// `avatar`, painted into a given rect (no layout).
pub(crate) fn paint_avatar(
    ui: &egui::Ui,
    rect: egui::Rect,
    user_id: &str,
    name: &str,
    url: &str,
    config: &Config,
    pal: &Palette,
) {
    let size = rect.width();
    if config.modules.images && !url.is_empty() {
        // Placeholder disc while loading (or if the download fails).
        ui.painter()
            .circle_filled(rect.center(), size / 2.0, pal.hover);
        egui::Image::from_uri(url)
            .corner_radius(size / 2.0)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
    } else {
        let hue = user_id
            .bytes()
            .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let col = egui::ecolor::Hsva::new((hue % 360) as f32 / 360.0, 0.45, 0.6, 1.0);
        ui.painter().circle_filled(rect.center(), size / 2.0, col);
        let letter = name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            theme::bold_font(size * 0.42),
            Color32::WHITE,
        );
    }
}

/// A 48 px header strip across the top of a panel with a 1 px border and a
/// soft shadow under it (Discord's channel-list and chat headers). Returns
/// the header's rect.
fn header_bar(ui: &mut egui::Ui, pal: &Palette) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), egui::Sense::hover());
    let y = rect.bottom();
    let painter = ui.painter();
    painter.hline(rect.x_range(), y - 0.5, egui::Stroke::new(1.0, pal.servers));
    let shadow = if pal.dark { 40 } else { 14 };
    for i in 0..3 {
        let a = (shadow / (i + 1)) as u8;
        painter.hline(
            rect.x_range(),
            y + 0.5 + i as f32,
            egui::Stroke::new(1.0, Color32::from_black_alpha(a)),
        );
    }
    rect
}

/// "▾ TEXT CHANNELS" — click to collapse / expand.
fn category_header(
    ui: &mut egui::Ui,
    name: &str,
    collapsed: bool,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 22.0), egui::Sense::click());
    let col = if resp.hovered() { pal.text } else { pal.muted };
    let chevron = if collapsed {
        icons::CARET_RIGHT
    } else {
        icons::CARET_DOWN
    };
    ui.painter().text(
        rect.left_center() + vec2(1.0, 0.0),
        egui::Align2::LEFT_CENTER,
        chevron,
        icons::font(11.0),
        col,
    );
    let galley = truncated(
        ui,
        &name.to_uppercase(),
        theme::bold_font(11.5),
        col,
        rect.width() - 20.0,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
        galley,
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What a channel row needs to know besides the channel.
struct ChannelRow {
    selected: bool,
    muted: bool,
    unread: bool,
    mentions: u32,
    /// DM partner's status (shown on their avatar).
    status: Option<Status>,
    /// DM partner's custom status / activity (second line).
    activity: Option<String>,
}

/// One channel in the list: "# name" (32 px) or, for DMs, avatar + name
/// (42 px) with a × to close on hover; hover / selected backgrounds, white
/// unread text and pill, mention badge. Voice channels are listed greyed
/// out. Returns the row's response and whether × was clicked.
fn channel_row(
    ui: &mut egui::Ui,
    c: &crate::types::Channel,
    row: &ChannelRow,
    config: &Config,
    pal: &Palette,
) -> (egui::Response, bool) {
    let dm = c.is_dm();
    let h = if dm { 42.0 } else { 32.0 };
    let sense = if c.is_voice() {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), sense);
    let hovered = resp.hovered() && !c.is_voice();
    if row.selected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if hovered {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let color = if c.is_voice() {
        pal.muted_text()
    } else if row.selected {
        pal.strong
    } else if row.muted {
        pal.muted_text()
    } else if row.unread || row.mentions > 0 {
        pal.unread
    } else if hovered {
        pal.text
    } else {
        pal.muted
    };
    let bold = (row.unread || row.mentions > 0) && !row.muted;
    let font = if bold || row.selected {
        theme::bold_font(15.0)
    } else {
        egui::FontId::proportional(15.0)
    };

    let mut x = rect.left() + 8.0;
    if dm {
        let a =
            egui::Rect::from_center_size(egui::pos2(x + 16.0, rect.center().y), vec2(32.0, 32.0));
        match c.recipients.first().filter(|_| c.kind == CHANNEL_DM) {
            Some(u) => paint_avatar(ui, a, &u.id, &u.name, &u.avatar_url, config, pal),
            None => {
                ui.painter().circle_filled(a.center(), 16.0, pal.accent);
                ui.painter().text(
                    a.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::USERS,
                    icons::fill_font(17.0),
                    Color32::WHITE,
                );
            }
        }
        if let Some(st) = row.status {
            let ring = if row.selected {
                pal.selected
            } else if hovered {
                pal.hover
            } else {
                pal.sidebar
            };
            members::status_dot(ui, a, st, ring, pal);
        }
        x += 32.0 + 12.0;
    } else {
        ui.painter().text(
            egui::pos2(x + 10.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            c.icon(),
            icons::font(20.0),
            if row.selected || hovered || bold {
                color
            } else {
                pal.muted.gamma_multiply(0.85)
            },
        );
        x += 28.0;
    }
    // DMs: × on hover closes the conversation (like Discord).
    let show_close = dm && hovered && row.mentions == 0;
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        vec2(20.0, 20.0),
    );
    let mut close = false;
    if show_close {
        let over = ui.rect_contains_pointer(close_rect);
        icons::paint(
            ui,
            close_rect,
            icons::X,
            icons::font(16.0),
            if over { pal.strong } else { pal.muted },
        );
        if over && resp.clicked() {
            close = true;
        }
    }
    let right_pad = if row.mentions > 0 || show_close {
        34.0
    } else {
        8.0
    };
    let galley = truncated(ui, &c.name, font, color, rect.right() - right_pad - x);
    match row.activity.as_deref().filter(|_| dm) {
        // Name over the custom status / activity, like Discord's DM list.
        Some(act) => {
            let sub = truncated(
                ui,
                act,
                egui::FontId::proportional(12.0),
                pal.muted,
                rect.right() - right_pad - x,
            );
            let total = galley.size().y + sub.size().y - 3.0;
            let top = rect.center().y - total / 2.0;
            let gh = galley.size().y;
            ui.painter().galley(egui::pos2(x, top), galley, color);
            ui.painter()
                .galley(egui::pos2(x, top + gh - 3.0), sub, pal.muted);
        }
        None => {
            ui.painter().galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
    }
    if row.unread && !row.selected {
        unread_pill(ui, rect, 8.0, pal);
    }
    if row.mentions > 0 {
        mention_badge(
            ui,
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            row.mentions,
            if row.selected {
                pal.selected
            } else {
                pal.sidebar
            },
            pal,
        );
    }
    if c.is_voice() {
        return (resp.on_hover_text("Voice channels are coming later"), false);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = if show_close && ui.rect_contains_pointer(close_rect) {
        resp.on_hover_text("Close DM")
    } else {
        match row.status.filter(|_| dm) {
            Some(st) => resp.on_hover_text(st.label()),
            None => resp,
        }
    };
    (resp, close)
}

/// Discord's ⊕ upload button: a filled circle with a plus cut out.
fn plus_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
    let col = if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    // The filled Phosphor icon has the plus cut out, so it reads on any
    // theme (glass themes have a see-through input fill).
    icons::paint(ui, rect, icons::PLUS_CIRCLE, icons::fill_font(26.0), col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Upload files (or drag them onto the window)")
}

/// The subtle send arrow at the right of the message box (only when there's
/// something to send).
fn send_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::click());
    let col = if resp.hovered() {
        pal.accent
    } else {
        pal.muted
    };
    icons::paint(ui, rect, icons::SEND, icons::fill_font(20.0), col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Send (Enter)")
}

/// A file waiting to upload (Discord's attachment card): icon, name, size,
/// SPOILER tag, and spoiler / remove buttons. Returns (toggle spoiler,
/// remove).
fn upload_card(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    size: u64,
    spoiler: bool,
    pal: &Palette,
) -> (bool, bool) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut toggle = false;
    let mut remove = false;
    egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(196.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let icon = if spoiler {
                    icons::EYE_SLASH
                } else {
                    icons::FILE
                };
                ui.label(RichText::new(icon).size(26.0).color(pal.muted));
                ui.vertical(|ui| {
                    ui.set_width(104.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(
                        egui::Label::new(RichText::new(&name).size(13.0).color(pal.strong))
                            .truncate(),
                    );
                    let mut info = human_size(size);
                    if spoiler {
                        info.push_str(" · SPOILER");
                    }
                    ui.label(RichText::new(info).size(11.5).color(pal.muted));
                });
                let tip = if spoiler {
                    "Remove spoiler"
                } else {
                    "Mark as spoiler"
                };
                let eye = if spoiler {
                    icons::EYE
                } else {
                    icons::EYE_SLASH
                };
                toggle = widgets::icon_button_sized(ui, eye, tip, spoiler, true, 26.0, 16.0, pal)
                    .clicked();
                remove = widgets::icon_button_sized(
                    ui,
                    icons::TRASH,
                    "Remove",
                    false,
                    true,
                    26.0,
                    16.0,
                    pal,
                )
                .clicked();
            });
        });
    (toggle, remove)
}

/// "● ● ●  Sam is typing…" under the message box, with Discord's
/// bouncing dots. Always takes its height so the box doesn't jump.
fn typing_row(ui: &mut egui::Ui, names: &[&str], pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::hover());
    if names.is_empty() {
        return;
    }
    let t = ui.input(|i| i.time) as f32;
    let y = rect.center().y;
    for i in 0..3 {
        let phase = (t * 6.0 - i as f32 * 0.9).sin() * 0.5 + 0.5;
        let c = egui::pos2(rect.left() + 4.0 + i as f32 * 7.0, y - phase * 1.5);
        ui.painter()
            .circle_filled(c, 2.5, pal.text.gamma_multiply(0.35 + 0.65 * phase));
    }
    // Animate while visible (only when someone is typing).
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));

    let bold = |n: &str| (n.to_string(), true);
    let plain = |n: &str| (n.to_string(), false);
    let parts: Vec<(String, bool)> = match names {
        [one] => vec![bold(one), plain(" is typing…")],
        [a, b] => vec![bold(a), plain(" and "), bold(b), plain(" are typing…")],
        [a, b, c] => vec![
            bold(a),
            plain(", "),
            bold(b),
            plain(" and "),
            bold(c),
            plain(" are typing…"),
        ],
        _ => vec![plain("Several people are typing…")],
    };
    let mut job = egui::text::LayoutJob::default();
    for (text, is_bold) in parts {
        let font = if is_bold {
            theme::bold_font(12.0)
        } else {
            egui::FontId::proportional(12.0)
        };
        let col = if is_bold { pal.text } else { pal.muted };
        job.append(&text, 0.0, egui::TextFormat::simple(font, col));
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 30.0);
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(rect.left() + 26.0, y - galley.size().y / 2.0),
        galley,
        pal.muted,
    );
}

/// Unread / mention / muted state of a server icon.
#[derive(Clone, Copy, Default)]
struct Badge {
    unread: bool,
    mentions: u32,
    muted: bool,
}

/// Window has keyboard focus (unknown counts as focused).
fn window_focused(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().focused.unwrap_or(true))
}

/// Discord's little white pill on the left edge of an unread item. It sits
/// on the clip edge so only its right half shows, like Discord.
fn unread_pill(ui: &egui::Ui, item: egui::Rect, height: f32, pal: &Palette) {
    let x = ui.clip_rect().left();
    let rect = egui::Rect::from_center_size(egui::pos2(x, item.center().y), vec2(8.0, height));
    ui.painter().rect_filled(rect, 4, pal.unread);
}

/// Red rounded badge with a count ("99+" past 99).
fn mention_badge(
    ui: &egui::Ui,
    at: egui::Pos2,
    align: egui::Align2,
    count: u32,
    ring: Color32,
    pal: &Palette,
) {
    let text = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    let galley =
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(11.0), Color32::WHITE);
    let size = vec2((galley.size().x + 9.0).max(16.0), 16.0);
    let rect = align.align_size_within_rect(size, egui::Rect::from_min_size(at, egui::Vec2::ZERO));
    let rect = egui::Rect::from_min_size(rect.min, size);
    // Ring in the panel colour so it stands out over icons.
    ui.painter().rect_filled(rect.expand(2.0), 10, ring);
    ui.painter().rect_filled(rect, 8, pal.mention);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// Red "NEW" line above the first unread message.
fn new_divider(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), egui::Sense::hover());
    let rect = rect.shrink2(vec2(MSG_PAD, 0.0));
    let y = rect.center().y;
    ui.painter().hline(
        rect.left()..=rect.right() - 36.0,
        y,
        egui::Stroke::new(1.0, pal.mention),
    );
    let tag = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 34.0, rect.top()),
        rect.right_bottom(),
    );
    ui.painter().rect_filled(tag, 3, pal.mention);
    ui.painter().text(
        tag.center(),
        egui::Align2::CENTER_CENTER,
        "NEW",
        theme::bold_font(9.5),
        Color32::WHITE,
    );
}

/// One mention in the inbox popup.
fn inbox_row(ui: &mut egui::Ui, item: &InboxItem, pal: &Palette) -> egui::Response {
    let resp = egui::Frame::new()
        .fill(if item.read { pal.sidebar } else { pal.input })
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if !item.read {
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, pal.mention);
                }
                ui.label(RichText::new(&item.place).small().strong().color(pal.muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&item.time).small().color(pal.muted));
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&item.author).strong());
                let text = RichText::new(&item.preview);
                ui.label(if item.read {
                    text.color(pal.muted)
                } else {
                    text
                });
            });
        })
        .response;
    let resp = ui.interact(
        resp.rect,
        ui.scope_id().with(("inbox", &item.message_id)),
        egui::Sense::click(),
    );
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            6,
            egui::Stroke::new(1.0, pal.accent),
            egui::StrokeKind::Inside,
        );
    }
    resp.on_hover_text("Jump to message")
}

/// The rail shows server names when dragged at least this wide.
const RAIL_NAMES_WIDTH: f32 = 160.0;

/// Server icons are 48 px; a rail squeezed below Discord's 72 px (small
/// window, custom layout) shrinks them instead of clipping them.
fn rail_icon_size(width: f32) -> f32 {
    (width - 20.0).clamp(28.0, 48.0)
}

enum RailIcon<'a> {
    Home,
    Image(&'a str),
    Initials(&'a str),
}

/// One server in the rail: a 48 px icon that morphs from a circle into a
/// rounded square when hovered or selected, the white pill on the left
/// (small = unread, medium = hover, tall = selected), a red mention badge,
/// and greyed out when muted.
fn rail_item(
    ui: &mut egui::Ui,
    icon: RailIcon<'_>,
    name: &str,
    selected: bool,
    badge: Badge,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
    let width = ui.available_width();
    let wide = width >= RAIL_NAMES_WIDTH;
    let size = rail_icon_size(width);
    let (row, resp) = ui.allocate_exact_size(vec2(width, size), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let ctx = ui.ctx().clone();

    // Circle (radius 24) → rounded square (radius 16).
    let t = ctx.animate_bool_with_time(resp.id.with("shape"), selected || hovered, 0.12);
    let radius = egui::lerp(size / 2.0..=size / 3.0, t);
    let x = if wide {
        row.left() + 12.0
    } else {
        row.center().x - size / 2.0
    };
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), vec2(size, size));
    let active = selected || hovered;

    match icon {
        RailIcon::Image(url) => {
            let tint = if badge.muted && !active {
                Color32::from_gray(110)
            } else {
                Color32::WHITE
            };
            ui.painter().rect_filled(rect, radius, pal.rail_icon);
            egui::Image::from_uri(url)
                .corner_radius(radius)
                .tint(tint)
                .show_loading_spinner(false)
                .paint_at(ui, rect);
        }
        RailIcon::Home | RailIcon::Initials(_) => {
            let fill = if active { pal.accent } else { pal.rail_icon };
            ui.painter().rect_filled(rect, radius, fill);
            let (text, font) = match icon {
                RailIcon::Initials(i) => {
                    let size = if i.chars().count() > 2 { 14.0 } else { 16.0 };
                    (i, theme::bold_font(size))
                }
                _ => (icons::DISCORD_LOGO, icons::fill_font(26.0)),
            };
            let col = if active {
                Color32::WHITE
            } else if badge.muted {
                pal.muted_text()
            } else {
                pal.text
            };
            ui.painter()
                .text(rect.center(), egui::Align2::CENTER_CENTER, text, font, col);
        }
    }

    // The pill sits on the panel's left edge; only its right half shows.
    let target = if selected {
        40.0
    } else if hovered {
        20.0
    } else if badge.unread && !badge.muted {
        8.0
    } else {
        0.0
    };
    let h = ctx.animate_value_with_time(resp.id.with("pill"), target, 0.12);
    if h > 0.5 {
        let left = ui.clip_rect().left();
        let pill = egui::Rect::from_center_size(egui::pos2(left, rect.center().y), vec2(8.0, h));
        ui.painter().rect_filled(pill, 4, pal.unread);
    }

    if badge.mentions > 0 {
        mention_badge(
            ui,
            rect.right_bottom() + vec2(2.0, 2.0),
            egui::Align2::RIGHT_BOTTOM,
            badge.mentions,
            pal.servers,
            pal,
        );
    }

    if wide {
        let color = if selected {
            pal.strong
        } else if badge.muted {
            pal.muted_text()
        } else if badge.unread || hovered {
            pal.unread
        } else {
            pal.muted
        };
        let font = if badge.unread || selected {
            theme::bold_font(15.0)
        } else {
            egui::FontId::proportional(15.0)
        };
        let max_w = row.right() - rect.right() - 20.0;
        let galley = truncated(ui, name, font, color, max_w);
        ui.painter().galley(
            egui::pos2(rect.right() + 12.0, row.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    let _ = config;
    let hover = if badge.muted {
        format!("{name} (muted)")
    } else {
        name.to_string()
    };
    if wide {
        resp
    } else {
        resp.on_hover_text(hover)
    }
}

/// What the user did in the server rail this frame.
#[derive(Default)]
struct RailEvents {
    clicked: Option<String>,
    mark_read: Option<String>,
    toggle_folder: Option<i64>,
    /// (id, name) of a server to leave (after confirming).
    leave: Option<(String, String)>,
    /// The rail's "+" (Add a Server).
    add_server: bool,
    /// Guild context menu → Channels & Roles.
    channels_roles: Option<String>,
}

/// Up to three initials of a server name ("RustCord Test Server" → "RTS").
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(3)
        .collect()
}

/// A server folder in the rail.
struct FolderIcon<'a> {
    name: &'a str,
    color: Color32,
    guilds: &'a [&'a Guild],
    open: bool,
    /// One of its servers is selected.
    selected: bool,
    badge: Badge,
}

/// A folder: closed, a tinted rounded square with up to four tiny server
/// icons; open, a folder glyph in the folder's colour (its servers follow
/// below). Click toggles.
fn folder_item(
    ui: &mut egui::Ui,
    f: &FolderIcon,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
    let width = ui.available_width();
    let wide = width >= RAIL_NAMES_WIDTH;
    let size = rail_icon_size(width);
    let (row, resp) = ui.allocate_exact_size(vec2(width, size), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let x = if wide {
        row.left() + 12.0
    } else {
        row.center().x - size / 2.0
    };
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), vec2(size, size));
    let hovered = resp.hovered();
    let painter = ui.painter();
    if f.open {
        painter.rect_filled(rect, 16, if hovered { pal.hover } else { pal.rail_icon });
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icons::FOLDER,
            icons::fill_font(24.0),
            f.color,
        );
    } else {
        painter.rect_filled(
            rect,
            16,
            f.color.gamma_multiply(if hovered { 0.5 } else { 0.35 }),
        );
        // 2×2 grid of mini icons.
        let mini = 18.0 * size / 48.0;
        let gap = 4.0 * size / 48.0;
        let start = rect.center() - vec2(mini + gap / 2.0, mini + gap / 2.0);
        for (i, g) in f.guilds.iter().take(4).enumerate() {
            let (cx, cy) = ((i % 2) as f32, (i / 2) as f32);
            let r = egui::Rect::from_min_size(
                start + vec2(cx * (mini + gap), cy * (mini + gap)),
                vec2(mini, mini),
            );
            match &g.icon_url {
                Some(url) if config.modules.images => {
                    painter.circle_filled(r.center(), mini / 2.0, pal.rail_icon);
                    egui::Image::from_uri(url)
                        .corner_radius(mini / 2.0)
                        .show_loading_spinner(false)
                        .paint_at(ui, r);
                }
                _ => {
                    painter.circle_filled(r.center(), mini / 2.0, pal.rail_icon);
                    let i: String = initials(&g.name).chars().take(1).collect();
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        i,
                        theme::bold_font(9.5),
                        pal.text,
                    );
                }
            }
        }
    }
    // Pill: selected server inside a closed folder, hover, or unread.
    let target = if f.selected && !f.open {
        40.0
    } else if hovered {
        20.0
    } else if f.badge.unread {
        8.0
    } else {
        0.0
    };
    let h = ui
        .ctx()
        .animate_value_with_time(resp.id.with("pill"), target, 0.12);
    if h > 0.5 {
        let left = ui.clip_rect().left();
        let pill = egui::Rect::from_center_size(egui::pos2(left, rect.center().y), vec2(8.0, h));
        ui.painter().rect_filled(pill, 4, pal.unread);
    }
    if f.badge.mentions > 0 {
        mention_badge(
            ui,
            rect.right_bottom() + vec2(2.0, 2.0),
            egui::Align2::RIGHT_BOTTOM,
            f.badge.mentions,
            pal.servers,
            pal,
        );
    }
    if wide {
        let galley = truncated(
            ui,
            f.name,
            egui::FontId::proportional(15.0),
            pal.muted,
            row.right() - rect.right() - 20.0,
        );
        ui.painter().galley(
            egui::pos2(rect.right() + 12.0, row.center().y - galley.size().y / 2.0),
            galley,
            pal.muted,
        );
        resp
    } else {
        resp.on_hover_text(f.name)
    }
}

/// Lay out one line of text, cut off with "…" past `max_width`.
pub(crate) fn truncated(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(10.0));
    ui.painter().layout_job(job)
}

/// How to show a message author: server nickname, top role colour.
struct AuthorStyle<'a> {
    name: &'a str,
    color: Option<Color32>,
}

/// Names for markdown mentions: the open server's nicknames and roles,
/// people seen in messages, and every known channel.
struct ChatLookup<'a> {
    guild: Option<&'a Guild>,
    guilds: &'a [Guild],
    dms: &'a Guild,
    members: &'a MemberCache,
    names: &'a HashMap<String, String>,
}

impl markdown::Lookup for ChatLookup<'_> {
    fn user_name(&self, id: &str) -> Option<String> {
        self.guild
            .and_then(|g| self.members.get(&g.id, id))
            .and_then(|m| m.nick.clone())
            .or_else(|| self.names.get(id).cloned())
    }

    fn role(&self, id: &str) -> Option<(String, Option<Color32>)> {
        let r = self.guild?.roles.iter().find(|r| r.id == id)?;
        Some((r.name.clone(), r.color32()))
    }

    fn channel_name(&self, id: &str) -> Option<String> {
        std::iter::once(self.dms)
            .chain(self.guilds)
            .find_map(|g| g.channel(id))
            .map(|c| c.name.clone())
    }
}

/// Per-message flags for `message_row`.
struct RowCtx<'a> {
    /// Follow-up from the same author: no avatar / name line.
    grouped: bool,
    /// Mentions me (or @everyone): Discord's yellow highlight.
    mentions_me: bool,
    /// Compact display: time and name inline, no avatars.
    compact: bool,
    /// Just jumped here (pins): flash the background.
    highlight: bool,
    menu: &'a MenuCtx,
    /// Reactions module (None = off).
    reactions: Option<&'a ReactParts<'a>>,
    /// Invites module (None = off).
    invites: Option<&'a InviteParts<'a>>,
}

/// A message's parsed content.
struct Body<'a> {
    blocks: &'a [Block],
    /// Reply snippet as plain text.
    reply: Option<&'a str>,
    /// Text of a forwarded message.
    forwarded: Option<&'a [Block]>,
    lookup: &'a dyn markdown::Lookup,
}

/// Message layout, in px (Discord's): 16 px left padding, 40 px avatar,
/// content starts at 72 px; groups start 17 px apart.
const MSG_PAD: f32 = 16.0;
const MSG_AVATAR: f32 = 40.0;
const MSG_GUTTER: f32 = 72.0;
const GROUP_GAP: f32 = 17.0;
/// Compact mode: width of the time column.
const COMPACT_TIME: f32 = 64.0;

/// Draw one message. Returns an action if the user picked one (hover bar,
/// right-click menu, avatar / name click, links), and the row's rect.
fn message_row(
    ui: &mut egui::Ui,
    m: &Message,
    body: &Body,
    author: &AuthorStyle<'_>,
    rc: &RowCtx,
    config: &Config,
    pal: &Palette,
) -> (Option<MessageAction>, egui::Rect) {
    let mut action = None;
    if rc.compact {
        ui.add_space(if m.reply.is_some() { 6.0 } else { 2.0 });
    } else if !rc.grouped {
        ui.add_space(GROUP_GAP - 2.0);
    }
    let more_id = egui::Id::unique(("msg_more", &m.id));
    let react_id = egui::Id::unique(("msg_react", &m.id));
    let menu_open = egui::Popup::is_id_open(ui.ctx(), more_id.with("popup"))
        || egui::Popup::is_id_open(ui.ctx(), react_id.with("popup"));
    // Last frame's hover bar rect: keep the bar while the pointer is on it.
    let bar_id = egui::Id::unique("msg_hover_bar");
    let last_bar = ui
        .ctx()
        .memory(|mem| mem.data.get_temp::<(egui::Id, egui::Rect)>(bar_id));
    // (The bar is on its own layer, so check the raw pointer position.)
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let on_bar = last_bar.is_some_and(|(owner, r)| {
        owner == more_id && pointer.is_some_and(|p| r.expand(2.0).contains(p))
    });

    let row = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("msg", &m.id))
            .sense(egui::Sense::click()),
        |ui| {
            let bg = ui.painter().add(egui::Shape::Noop);
            let full = ui.available_rect_before_wrap();
            let mut gutter = egui::Rect::NOTHING;
            let inner = egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: MSG_PAD as i8,
                    right: 48,
                    top: if rc.compact { 1 } else { 2 },
                    bottom: if rc.compact { 1 } else { 2 },
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let (Some(reply), Some(text)) = (&m.reply, body.reply)
                        && reply_preview(ui, reply, text, rc.compact, config, pal).clicked()
                        && !reply.id.is_empty()
                    {
                        action = Some(MessageAction::JumpTo(reply.id.clone()));
                    }
                    if rc.compact {
                        compact_line(ui, m, body, author, rc, config, pal, &mut action);
                        return;
                    }
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if rc.grouped {
                            let (r, _) = ui.allocate_exact_size(
                                vec2(MSG_GUTTER - MSG_PAD, 20.0),
                                egui::Sense::hover(),
                            );
                            gutter = r;
                        } else {
                            let rect = avatar(
                                ui,
                                &m.author.id,
                                author.name,
                                &m.author.avatar_url,
                                MSG_AVATAR,
                                config,
                                pal,
                            );
                            let resp = ui
                                .interact(
                                    rect,
                                    ui.scope_id().with(("msg_avatar", &m.id)),
                                    egui::Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                action = Some(MessageAction::Profile(m.author.clone(), rect));
                            }
                            ui.add_space(MSG_GUTTER - MSG_PAD - MSG_AVATAR);
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
                            if !rc.grouped {
                                ui.horizontal(|ui| {
                                    if author_name(ui, m, author, 15.0, pal) {
                                        action = Some(MessageAction::Profile(
                                            m.author.clone(),
                                            ui.min_rect(),
                                        ));
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(timefmt::full(m.time))
                                                .size(12.0)
                                                .color(pal.muted),
                                        )
                                        .selectable(false),
                                    )
                                    .on_hover_text(timefmt::long(m.time));
                                });
                            }
                            message_content(ui, m, body, rc, config, pal, &mut action);
                            if let Some(ip) = rc.invites
                                && let Some(a) = invite_cards(ui, m, ip, pal)
                            {
                                action = Some(MessageAction::Invite(a));
                            }
                            if let Some(rp) = rc.reactions
                                && let Some(a) = reaction_row(ui, m, rp, pal)
                            {
                                action = Some(MessageAction::React(m.id.clone(), a));
                            }
                        });
                    });
                })
                .response
                .rect;
            (full.with_max_y(inner.bottom()), gutter, bg)
        },
    );
    let (rect, gutter, bg) = row.inner;
    let resp = row.response;
    let hovered = ui.rect_contains_pointer(rect) || on_bar || menu_open;

    // Backgrounds: mention highlight, jump flash, then hover.
    let mut shapes = Vec::new();
    if rc.mentions_me {
        shapes.push(egui::Shape::rect_filled(
            rect,
            0,
            pal.idle.gamma_multiply(0.10),
        ));
        shapes.push(egui::Shape::rect_filled(
            egui::Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
            0,
            pal.idle,
        ));
    }
    if rc.highlight {
        shapes.push(egui::Shape::rect_filled(
            rect,
            0,
            pal.accent.gamma_multiply(0.18),
        ));
    }
    if hovered {
        let c = if rc.mentions_me {
            pal.idle.gamma_multiply(0.06)
        } else {
            pal.msg_hover
        };
        shapes.push(egui::Shape::rect_filled(rect, 0, c));
    }
    ui.painter().set(bg, egui::Shape::Vec(shapes));

    // Grouped follow-ups: the time in the gutter on hover.
    if hovered && rc.grouped && gutter.is_positive() {
        ui.painter().text(
            egui::pos2(rect.left() + MSG_PAD + MSG_AVATAR / 2.0, gutter.center().y),
            egui::Align2::CENTER_CENTER,
            m.short_time(),
            egui::FontId::proportional(10.5),
            pal.muted,
        );
    }

    egui::Popup::context_menu(&resp)
        .frame(widgets::menu_frame(pal))
        .show(|ui| message_menu(ui, m, rc.menu, &mut action, pal));

    if hovered {
        let ids = BarIds {
            more: more_id,
            bar: bar_id,
            react: react_id,
        };
        hover_bar(ui, m, rc, rect, ids, &mut action, pal);
    }
    (action, rect)
}

/// Bold author name (role colour) and BOT tag. Returns true if clicked.
fn author_name(
    ui: &mut egui::Ui,
    m: &Message,
    author: &AuthorStyle<'_>,
    size: f32,
    pal: &Palette,
) -> bool {
    let name = RichText::new(author.name)
        .bold()
        .size(size)
        .color(author.color.unwrap_or(pal.strong));
    let resp = ui
        .add(
            egui::Label::new(name)
                .sense(egui::Sense::click())
                .selectable(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if m.author.bot {
        members::bot_tag(ui, pal);
    }
    resp.clicked()
}

/// Compact display: "15:05  Name  message…" on one line.
#[allow(clippy::too_many_arguments)]
fn compact_line(
    ui: &mut egui::Ui,
    m: &Message,
    body: &Body,
    author: &AuthorStyle<'_>,
    rc: &RowCtx,
    config: &Config,
    pal: &Palette,
    action: &mut Option<MessageAction>,
) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, _) = ui.allocate_exact_size(vec2(COMPACT_TIME - 6.0, 20.0), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(r.right(), r.center().y),
            egui::Align2::RIGHT_CENTER,
            m.short_time(),
            egui::FontId::proportional(11.0),
            pal.muted,
        );
        ui.interact(
            r,
            ui.scope_id().with(("ctime", &m.id)),
            egui::Sense::hover(),
        )
        .on_hover_text(timefmt::long(m.time));
        let name_rect = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if author_name(ui, m, author, 14.5, pal) {
                    *action = Some(MessageAction::Profile(m.author.clone(), ui.min_rect()));
                }
            })
            .response
            .rect;
        let _ = name_rect;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            message_content(ui, m, body, rc, config, pal, action);
        });
    });
}

/// Markdown body with "(edited)", then embeds and attachments.
fn message_content(
    ui: &mut egui::Ui,
    m: &Message,
    body: &Body,
    rc: &RowCtx,
    config: &Config,
    pal: &Palette,
    action: &mut Option<MessageAction>,
) {
    let mut rctx = markdown::RenderCtx::new(pal, body.lookup, &m.id);
    rctx.emoji_images = config.modules.emoji_images;
    rctx.bold_family = Some(theme::bold_family());
    let edited = |ui: &mut egui::Ui| {
        if m.edited {
            ui.add_space(4.0);
            let r = ui.add(
                egui::Label::new(RichText::new("(edited)").size(10.5).color(pal.muted))
                    .sense(egui::Sense::hover())
                    .selectable(false),
            );
            if m.edited_time > 0 {
                r.on_hover_text(format!("Edited {}", timefmt::long(m.edited_time)));
            }
        }
    };
    if !body.blocks.is_empty() {
        let out = markdown::render_with(ui, body.blocks, &rctx, edited);
        if let Some(r) = &out.response {
            egui::Popup::context_menu(r)
                .frame(widgets::menu_frame(pal))
                .show(|ui| message_menu(ui, m, rc.menu, action, pal));
        }
        match out.action {
            Some(MarkdownAction::Link(url)) => *action = Some(MessageAction::OpenLink(url)),
            Some(MarkdownAction::User { id, rect }) => {
                *action = Some(MessageAction::Mention(id, rect))
            }
            Some(MarkdownAction::Channel(id)) => *action = Some(MessageAction::GoToChannel(id)),
            Some(MarkdownAction::Role(_)) | None => {}
        }
    } else if m.edited {
        ui.horizontal(edited);
    }

    if config.modules.embeds {
        for (n, e) in m.embeds.iter().enumerate() {
            // Spoilers in embeds need their own ids.
            let key = format!("{}:embed{n}", m.id);
            let text = |ui: &mut egui::Ui, text: &str| -> Option<String> {
                let blocks = if config.disable_formatting {
                    markdown::plain(text)
                } else {
                    markdown::parse(text)
                };
                let mut c = markdown::RenderCtx::new(pal, body.lookup, &key);
                c.emoji_images = config.modules.emoji_images;
                c.bold_family = Some(theme::bold_family());
                match markdown::render(ui, &blocks, &c).action {
                    Some(MarkdownAction::Link(url)) => Some(url),
                    _ => None,
                }
            };
            let mut ectx = embed::EmbedCtx::new(pal);
            ectx.show_images = config.show_embed_images;
            ectx.images_enabled = config.modules.images;
            ectx.max_width = ui.available_width().min(embed::MAX_WIDTH);
            ectx.text = Some(&text);
            ui.add_space(2.0);
            let out = embed::render_embed(ui, e, &ectx);
            if let Some(url) = out.clicked_link {
                *action = Some(MessageAction::OpenLink(url));
            }
        }
    }
    for a in &m.attachments {
        if let Some(url) = attachment_ui(ui, a, config, pal) {
            *action = Some(MessageAction::OpenLink(url));
        }
    }
    for st in &m.stickers {
        sticker_ui(ui, st, config, pal);
    }
    if let Some(f) = &m.forwarded {
        forwarded_ui(ui, m, f, body, config, pal, action);
    }
    if let Some(q) = &m.poll {
        egui::Frame::new()
            .fill(pal.sidebar)
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(icons::label(icons::CHECKS, "Poll"))
                        .size(12.0)
                        .color(pal.muted),
                );
                ui.label(RichText::new(q).bold().size(15.0).color(pal.strong));
                ui.label(
                    RichText::new("Open Discord to see the answers and vote.")
                        .size(12.0)
                        .color(pal.muted),
                );
            });
    }
    // Never leave a message blank: say there's something we can't show.
    let drew_something = !body.blocks.is_empty()
        || (config.modules.embeds && !m.embeds.is_empty())
        || !m.attachments.is_empty()
        || !m.stickers.is_empty()
        || m.forwarded.is_some()
        || m.poll.is_some();
    if !drew_something {
        let what = if m.has_components {
            "Buttons or menus RustCord can't show yet"
        } else if !config.modules.embeds {
            "Embed (the Embeds module is off)"
        } else {
            "Content RustCord can't show yet"
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::INFO).size(14.0).color(pal.muted));
            ui.label(RichText::new(what).italics().size(14.0).color(pal.muted));
        });
    }
}

/// Discord's system message line (joins, pins, calls, boosts, threads):
/// a coloured icon in the avatar column, bold names, muted text, the time.
fn system_row(
    ui: &mut egui::Ui,
    m: &Message,
    line: &sysmsg::SystemLine,
    pal: &Palette,
) -> Option<MessageAction> {
    use sysmsg::{Part, SysKind};
    ui.add_space(GROUP_GAP - 7.0);
    let (icon, col) = match line.kind {
        SysKind::Join => (icons::ARROW_RIGHT, pal.online),
        SysKind::Leave => (icons::ARROW_RIGHT, pal.danger),
        SysKind::Pin => (icons::PUSH_PIN, pal.muted),
        SysKind::Call => (icons::PHONE, pal.online),
        SysKind::Boost => (icons::ROCKET, pal.accent),
        SysKind::Thread => (icons::CHATS, pal.muted),
        SysKind::Edit => (icons::PENCIL, pal.muted),
        SysKind::Info => (icons::INFO, pal.muted),
    };
    // One wrapped text job: bold names, muted words, then the time.
    let mut job = egui::text::LayoutJob::default();
    for p in &line.parts {
        let (text, font, color) = match p {
            Part::Name(n) => (n.as_str(), theme::bold_font(15.0), pal.strong),
            Part::Text(t) => (t.as_str(), egui::FontId::proportional(15.0), pal.muted),
        };
        if !text.is_empty() {
            job.append(text, 0.0, egui::TextFormat::simple(font, color));
        }
    }
    job.append(
        &timefmt::full(m.time),
        8.0,
        egui::TextFormat::simple(egui::FontId::proportional(12.0), pal.muted),
    );
    let width = ui.available_width() - MSG_GUTTER - 48.0;
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(60.0),
        ..Default::default()
    };
    let galley = ui.painter().layout_job(job);
    let h = galley.size().y.max(22.0);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), h + 4.0), egui::Sense::hover());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0, pal.msg_hover);
    }
    ui.painter().text(
        egui::pos2(
            rect.left() + MSG_PAD + MSG_AVATAR / 2.0,
            rect.top() + 2.0 + 11.0,
        ),
        egui::Align2::CENTER_CENTER,
        icon,
        icons::font(18.0),
        col,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + MSG_GUTTER, rect.top() + 2.0),
        galley,
        pal.muted,
    );
    resp.on_hover_text(timefmt::long(m.time));
    ui.add_space(2.0);
    None
}

/// A sticker: its picture (PNG/APNG/GIF, 160 px) or, for Lottie stickers
/// and with the Images module off, a card with its name.
fn sticker_ui(ui: &mut egui::Ui, st: &sysmsg::Sticker, config: &Config, pal: &Palette) {
    match st
        .image_url(STICKER_SIZE as u32)
        .filter(|_| config.modules.images)
    {
        Some(url) => {
            ui.add(
                egui::Image::from_uri(url)
                    .fit_to_exact_size(vec2(STICKER_SIZE, STICKER_SIZE))
                    .show_loading_spinner(true),
            )
            .on_hover_text(format!("Sticker: {}", st.name));
        }
        None => {
            egui::Frame::new()
                .fill(pal.sidebar)
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::STICKER).size(20.0).color(pal.muted));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new(&st.name).bold().color(pal.strong));
                            ui.label(
                                RichText::new(if st.format == 3 {
                                    "Animated sticker"
                                } else {
                                    "Sticker"
                                })
                                .size(12.0)
                                .color(pal.muted),
                            );
                        });
                    });
                });
        }
    }
}

/// Size stickers are shown at (Discord: 160).
const STICKER_SIZE: f32 = 160.0;

/// A forwarded message: "Forwarded" header and the original content behind
/// a quote bar.
fn forwarded_ui(
    ui: &mut egui::Ui,
    m: &Message,
    f: &sysmsg::Forwarded,
    body: &Body,
    config: &Config,
    pal: &Palette,
    action: &mut Option<MessageAction>,
) {
    let start = ui.cursor().min;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 0,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.label(
                RichText::new(icons::label(icons::ARROW_SQUARE_OUT, "Forwarded"))
                    .italics()
                    .size(13.0)
                    .color(pal.muted),
            );
            if let Some(blocks) = body.forwarded {
                let key = format!("{}:fwd", m.id);
                let mut rctx = markdown::RenderCtx::new(pal, body.lookup, &key);
                rctx.emoji_images = config.modules.emoji_images;
                rctx.bold_family = Some(theme::bold_family());
                if let Some(MarkdownAction::Link(url)) = markdown::render(ui, blocks, &rctx).action
                {
                    *action = Some(MessageAction::OpenLink(url));
                }
            }
            for a in &f.attachments {
                if let Some(url) = attachment_ui(ui, a, config, pal) {
                    *action = Some(MessageAction::OpenLink(url));
                }
            }
            for st in &f.stickers {
                sticker_ui(ui, st, config, pal);
            }
            if f.embeds > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} embed{} in the original message",
                        f.embeds,
                        if f.embeds == 1 { "" } else { "s" }
                    ))
                    .size(12.0)
                    .color(pal.muted),
                );
            }
        });
    let end = ui.min_rect().bottom();
    ui.painter().rect_filled(
        egui::Rect::from_min_max(start, egui::pos2(start.x + 4.0, end)),
        2,
        pal.divider,
    );
}

/// "N blocked messages — Show". Returns true if Show was clicked.
fn blocked_row(ui: &mut egui::Ui, n: usize, pal: &Palette) -> bool {
    ui.add_space(GROUP_GAP - 2.0);
    let mut show = false;
    ui.horizontal(|ui| {
        ui.add_space(MSG_GUTTER);
        ui.label(RichText::new(icons::PROHIBIT).size(15.0).color(pal.muted));
        let text = if n == 1 {
            "1 blocked message".to_string()
        } else {
            format!("{n} blocked messages")
        };
        ui.label(RichText::new(text).size(14.0).color(pal.muted));
        ui.label(RichText::new("—").color(pal.muted));
        show = ui
            .add(
                egui::Label::new(
                    RichText::new("Show")
                        .size(14.0)
                        .color(ui.visuals().hyperlink_color),
                )
                .sense(egui::Sense::click())
                .selectable(false),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked();
    });
    show
}

/// Discord's reply line above a message: a curved connector from the
/// avatar column, the replied-to author's small avatar and bold name, and
/// a muted one-line snippet.
fn reply_preview(
    ui: &mut egui::Ui,
    reply: &crate::types::ReplyPreview,
    text: &str,
    compact: bool,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 20.0), egui::Sense::click());
    let y = rect.center().y;
    let line = if pal.dark {
        Color32::from_gray(0x4e)
    } else {
        Color32::from_gray(0xc4)
    };
    let mut x = rect.left() + MSG_GUTTER - MSG_PAD;
    if compact {
        // Compact rows have no avatar column: a small ↪ before the name.
        x = rect.left() + COMPACT_TIME - 18.0;
        ui.painter().text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            icons::REPLY,
            icons::font(14.0),
            line,
        );
        x += 18.0;
    } else {
        // Connector: from above the avatar's centre, up and round to the right.
        let x0 = rect.left() + MSG_AVATAR / 2.0;
        let x1 = rect.left() + MSG_GUTTER - MSG_PAD - 6.0;
        let r = 6.0;
        let mut pts = vec![egui::pos2(x0, rect.bottom() + 4.0)];
        for i in 0..=6 {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / 6.0;
            pts.push(egui::pos2(x0 + r - r * a.cos(), y + r - r * a.sin()));
        }
        pts.push(egui::pos2(x1, y));
        ui.painter()
            .add(egui::Shape::line(pts, egui::Stroke::new(2.0, line)));
    }

    let av = egui::Rect::from_center_size(egui::pos2(x + 8.0, y), vec2(16.0, 16.0));
    paint_avatar(
        ui,
        av,
        &reply.author_id,
        &reply.author,
        &reply.avatar_url,
        config,
        pal,
    );
    x += 20.0;
    let name = truncated(ui, &reply.author, theme::bold_font(13.0), pal.text, 160.0);
    let w = name.size().x;
    ui.painter()
        .galley(egui::pos2(x, y - name.size().y / 2.0), name, pal.text);
    x += w + 6.0;
    let snippet = truncated(
        ui,
        text,
        egui::FontId::proportional(13.0),
        pal.muted,
        rect.right() - x,
    );
    ui.painter().galley(
        egui::pos2(x, y - snippet.size().y / 2.0),
        snippet,
        pal.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Jump to message")
}

/// Ids of the hover bar and its popups (stable per message, so an open
/// popup keeps the bar up).
#[derive(Clone, Copy)]
struct BarIds {
    more: egui::Id,
    bar: egui::Id,
    react: egui::Id,
}

// --- reactions ---
/// Reaction pills under a message (nothing without reactions).
fn reaction_row(
    ui: &mut egui::Ui,
    m: &Message,
    rp: &ReactParts,
    pal: &Palette,
) -> Option<ReactionAction> {
    if m.reactions.is_empty() {
        return None;
    }
    let tooltip = |e: &EmojiRef, burst: bool| {
        let names = rp.state.reactors.get(&(m.id.clone(), e.clone(), burst))?;
        let total = m
            .reactions
            .iter()
            .find(|r| r.emoji.same(e))
            .map_or(0, |r| if burst { r.burst } else { r.normal });
        Some(reactions::reactors_text(e, names, total))
    };
    let ctx = ReactionCtx {
        id: egui::Id::unique(("reactions", &m.id)),
        pal,
        images: rp.images,
        // Disabled while my request for this message is in flight.
        can_react: rp.can_react && !rp.state.pending.has_message(&m.id),
        show_add: false,
        custom: rp.custom,
        recent: rp.recent,
        skin_tone: rp.skin_tone,
        tooltip: &tooltip,
    };
    ui.add_space(2.0);
    reactions::reaction_bar(ui, &m.reactions, &ctx)
}

// --- onboarding ---
/// Invite cards for the invite links in a message (at most
/// `invites::MAX_CARDS_PER_MESSAGE`).
fn invite_cards(
    ui: &mut egui::Ui,
    m: &Message,
    ip: &InviteParts,
    pal: &Palette,
) -> Option<InviteAction> {
    // Cheap pre-check before the full scan.
    if !(m.content.contains("discord.gg") || m.content.contains("/invite/")) {
        return None;
    }
    let mut action = None;
    let cx = CardCtx {
        pal,
        cache: ip.cache,
        is_member: ip.is_member,
        can_join: ip.can_join,
        sent_by_me: m.author.id == ip.my_id,
        button: true,
    };
    for code in invites::extract_invite_codes(&m.content)
        .iter()
        .take(invites::MAX_CARDS_PER_MESSAGE)
    {
        ui.add_space(4.0);
        if let Some(a) = invites::invite_card(ui, code, &cx) {
            action = Some(a);
        }
    }
    action
}

/// The hover bar's Add Reaction button and its picker popup.
fn add_reaction_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    rp: &ReactParts,
    pal: &Palette,
) -> Option<ReactionAction> {
    let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
    let btn = ui.interact(r, id, egui::Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), id.with("popup"));
    let hot = btn.hovered() || open;
    if hot {
        ui.painter().rect_filled(r.shrink(2.0), 4, pal.hover);
    }
    icons::paint(
        ui,
        r,
        icons::SMILEY,
        icons::font(20.0),
        if hot { pal.strong } else { pal.muted },
    );
    let btn = btn.on_hover_text("Add Reaction");
    let no_tooltip = |_: &EmojiRef, _: bool| None;
    let ctx = ReactionCtx {
        id,
        pal,
        images: rp.images,
        can_react: true,
        show_add: false,
        custom: rp.custom,
        recent: rp.recent,
        skin_tone: rp.skin_tone,
        tooltip: &no_tooltip,
    };
    reactions::picker_popup(&btn, &ctx)
}

/// The floating Reply / Edit / More bar at the top-right of a hovered
/// message.
#[allow(clippy::too_many_arguments)]
fn hover_bar(
    ui: &mut egui::Ui,
    m: &Message,
    rc: &RowCtx,
    row: egui::Rect,
    ids: BarIds,
    action: &mut Option<MessageAction>,
    pal: &Palette,
) {
    let BarIds {
        more: more_id,
        bar: bar_id,
        react: react_id,
    } = ids;
    let menu = rc.menu;
    let mine = menu.mine;
    // Add Reaction: allowed here and no request of mine in flight.
    let react = rc
        .reactions
        .filter(|rp| rp.can_react && !rp.state.pending.has_message(&m.id));
    let buttons = if mine { 3.0 } else { 2.0 } + if react.is_some() { 1.0 } else { 0.0 };
    let size = vec2(buttons * 32.0 + 4.0, 36.0);
    let clip = ui.clip_rect();
    let y = (row.top() - 16.0).max(clip.top() + 2.0);
    let pos = egui::pos2(row.right() - 16.0 - size.x, y);
    if !clip.intersects(egui::Rect::from_min_size(pos, size)) {
        return;
    }
    let area = egui::Area::new(bar_id)
        .order(egui::Order::Middle)
        .fade_in(false)
        .fixed_pos(pos)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(pal.chat)
                .stroke(egui::Stroke::new(1.0, pal.divider))
                .corner_radius(6)
                .inner_margin(2)
                .shadow(egui::Shadow {
                    offset: [0, 2],
                    blur: 6,
                    spread: 0,
                    color: Color32::from_black_alpha(if pal.dark { 60 } else { 20 }),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if let Some(rp) = react
                            && let Some(a) = add_reaction_button(ui, react_id, rp, pal)
                        {
                            *action = Some(MessageAction::React(m.id.clone(), a));
                        }
                        if icon_button(ui, icons::REPLY, "Reply", false, true, pal).clicked() {
                            *action =
                                Some(MessageAction::Reply(m.id.clone(), m.author.name.clone()));
                        }
                        if mine
                            && icon_button(ui, icons::PENCIL, "Edit", false, true, pal).clicked()
                        {
                            *action = Some(MessageAction::Edit(m.id.clone(), m.content.clone()));
                        }
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
                        let more = ui.interact(r, more_id, egui::Sense::click());
                        let open = egui::Popup::is_id_open(ui.ctx(), more_id.with("popup"));
                        if more.hovered() || open {
                            ui.painter().rect_filled(r.shrink(2.0), 4, pal.hover);
                        }
                        icons::paint(
                            ui,
                            r,
                            icons::DOTS,
                            icons::font(20.0),
                            if more.hovered() || open {
                                pal.strong
                            } else {
                                pal.muted
                            },
                        );
                        let more = more.on_hover_text("More");
                        egui::Popup::menu(&more)
                            .align(egui::RectAlign::BOTTOM_END)
                            .frame(widgets::menu_frame(pal))
                            .show(|ui| message_menu(ui, m, menu, action, pal));
                    });
                });
        });
    let rect = area.response.rect;
    ui.ctx()
        .memory_mut(|mem| mem.data.insert_temp(bar_id, (more_id, rect)));
}

/// Line with the date in the middle, between messages of different days.
fn date_divider(ui: &mut egui::Ui, label: &str, pal: &Palette) {
    ui.add_space(GROUP_GAP + 4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), egui::Sense::hover());
    let line = rect.shrink2(vec2(MSG_PAD, 0.0));
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::bold_font(11.5), pal.muted);
    let text = egui::Rect::from_center_size(rect.center(), galley.size() + vec2(8.0, 0.0));
    // The line stops either side of the label (no filled box), so theme
    // gradients and background images show through.
    let stroke = egui::Stroke::new(1.0, pal.divider);
    ui.painter()
        .hline(line.left()..=text.left(), rect.center().y, stroke);
    ui.painter()
        .hline(text.right()..=line.right(), rect.center().y, stroke);
    ui.painter()
        .galley(text.min + vec2(4.0, 0.0), galley, pal.muted);
    ui.add_space(4.0);
}

fn message_menu(
    ui: &mut egui::Ui,
    m: &Message,
    menu: &MenuCtx,
    action: &mut Option<MessageAction>,
    pal: &Palette,
) {
    ui.set_min_width(200.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut pick = |ui: &mut egui::Ui, icon: &str, label: &str, tone: Tone, a: MessageAction| {
        if widgets::menu_item(ui, Some(icon), label, tone, pal).clicked() {
            *action = Some(a);
        }
    };
    let n = Tone::Normal;
    if menu.mine {
        pick(
            ui,
            icons::PENCIL,
            "Edit Message",
            n,
            MessageAction::Edit(m.id.clone(), m.content.clone()),
        );
    }
    pick(
        ui,
        icons::REPLY,
        "Reply",
        n,
        MessageAction::Reply(m.id.clone(), m.author.name.clone()),
    );
    if menu.can_thread {
        pick(
            ui,
            icons::CHATS,
            "Create Thread",
            n,
            MessageAction::CreateThread(m.id.clone(), m.content.clone()),
        );
    }
    if menu.can_pin {
        if m.pinned {
            pick(
                ui,
                icons::PUSH_PIN_SLASH,
                "Unpin Message",
                n,
                MessageAction::Pin(m.id.clone(), false),
            );
        } else {
            pick(
                ui,
                icons::PUSH_PIN,
                "Pin Message",
                n,
                MessageAction::Pin(m.id.clone(), true),
            );
        }
    }
    widgets::menu_separator(ui, pal);
    if !m.content.is_empty() {
        pick(
            ui,
            icons::COPY,
            "Copy Text",
            n,
            MessageAction::Copy(m.content.clone()),
        );
    }
    pick(
        ui,
        icons::EYE_SLASH,
        "Mark Unread",
        n,
        MessageAction::MarkUnread(m.id.clone()),
    );
    pick(
        ui,
        icons::LINK,
        "Copy Message Link",
        n,
        MessageAction::Copy(m.link(menu.guild_id.as_deref())),
    );
    for a in m.attachments.iter().filter(|a| a.is_image()) {
        let label = if m.attachments.len() == 1 {
            "Save Image".to_string()
        } else {
            format!("Save {}", a.filename)
        };
        pick(
            ui,
            icons::DOWNLOAD,
            &label,
            n,
            MessageAction::SaveImage(a.url.clone(), a.filename.clone()),
        );
    }
    if menu.mine {
        pick(
            ui,
            icons::TRASH,
            "Delete Message",
            Tone::Danger,
            MessageAction::Delete(m.id.clone()),
        );
    }
    widgets::menu_separator(ui, pal);
    pick(
        ui,
        icons::USER,
        "Copy User ID",
        n,
        MessageAction::Copy(m.author.id.clone()),
    );
    pick(
        ui,
        icons::ID,
        "Copy Message ID",
        n,
        MessageAction::Copy(m.id.clone()),
    );
}

/// An image (click: open full size) or a file card. Returns a link the user
/// clicked; the caller opens it through the link warning.
fn attachment_ui(
    ui: &mut egui::Ui,
    a: &Attachment,
    config: &Config,
    pal: &Palette,
) -> Option<String> {
    let mut clicked = None;
    if a.is_image() && config.modules.images {
        let (max_w, max_h) = IMAGE_MAX;
        let url = a.thumbnail_url(max_w as u32, max_h as u32);
        let mut image = egui::Image::from_uri(url)
            .corner_radius(6)
            .sense(egui::Sense::click());
        image = match a.display_size(max_w, max_h) {
            Some((w, h)) => image.fit_to_exact_size(vec2(w, h)),
            None => image.max_size(vec2(max_w, max_h)),
        };
        let resp = ui.add(image).on_hover_text(format!(
            "{} ({}) · click to open full size",
            a.filename,
            a.human_size()
        ));
        if resp.clicked() {
            clicked = Some(a.url.clone());
        }
    } else {
        egui::Frame::new()
            .fill(pal.sidebar)
            .corner_radius(6)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::FILE_TEXT).size(30.0).color(pal.muted));
                    ui.vertical(|ui| {
                        let link = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(&a.filename).color(ui.visuals().hyperlink_color),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(&a.url);
                        if link.clicked() {
                            clicked = Some(a.url.clone());
                        }
                        ui.label(RichText::new(a.human_size()).small().color(pal.muted));
                    });
                });
            });
    }
    clicked
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{b} B")
    }
}
