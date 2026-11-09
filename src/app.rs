//! The RustCord UI: login screen, main layout, settings page.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, vec2};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{CONFIG_KEY, Config};
use crate::emoji::{self, Segment};
use crate::layout::{self, LAYOUT_KEY, LayoutBehavior, Pane};
use crate::login::{LoginAction, LoginForm};
use crate::notify::{self, Inbox, InboxItem, Mutes, NotifyCheck};
use crate::read_state::ReadState;
use crate::settings::{SettingsAction, SettingsPage};
use crate::theme::{Palette, ThemeChoice};
use crate::types::{Attachment, DM_GUILD_ID, Guild, Message, ToBackend, ToUi, User};

const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
const TYPING_RESEND: Duration = Duration::from_secs(8);
const IMAGE_MAX: (f32, f32) = (400.0, 300.0);
/// Discord's default upload limit for accounts without Nitro.
const UPLOAD_LIMIT: u64 = 10 * 1024 * 1024;
/// Acks for messages arriving in the open channel are sent at most this often.
const ACK_INTERVAL: Duration = Duration::from_secs(3);

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
    /// Theme + zoom currently applied, to re-apply only when they change.
    applied: Option<(ThemeChoice, f32)>,
}

impl RustCordApp {
    pub fn new(cc: &eframe::CreationContext<'_>, env_token: Option<String>, demo: bool) -> Self {
        let config: Config = cc
            .storage
            .and_then(|s| eframe::get_value(s, CONFIG_KEY))
            .unwrap_or_default();
        let tree = cc
            .storage
            .and_then(|s| eframe::get_value::<egui_tiles::Tree<Pane>>(s, LAYOUT_KEY))
            .filter(layout::is_complete)
            .unwrap_or_else(layout::default_tree);

        let mut app = Self {
            screen: Screen::Login(LoginForm::new(config.remember_login, None)),
            config,
            tree,
            editing_layout: false,
            settings: None,
            applied: None,
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
        let (ui_tx, ui_rx) = mpsc::channel();
        let (be_tx, be_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::backend::spawn(token, demo, ui_tx, be_rx, ctx.clone());
        self.screen = Screen::Main(Box::new(State::new(ui_rx, be_tx)));
    }

    /// Back to the login screen. Dropping the State drops the backend's
    /// channel, which shuts the backend thread down.
    fn log_out(&mut self, error: Option<String>, forget_token: bool) {
        if forget_token {
            self.config.saved_token = None;
        }
        self.settings = None;
        self.editing_layout = false;
        self.screen = Screen::Login(LoginForm::new(self.config.remember_login, error));
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let want = (self.config.theme, self.config.zoom);
        if self.applied != Some(want) {
            self.config.theme.palette().apply(ctx);
            ctx.set_zoom_factor(self.config.zoom);
            self.applied = Some(want);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, pal: &Palette, status: &str) {
        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::new()
                    .fill(pal.servers)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("RustCord").strong());
                    if self.editing_layout {
                        ui.separator();
                        ui.label(RichText::new("✏ Editing layout").strong().color(pal.accent));
                        ui.label(
                            RichText::new(
                                "hover a panel, hold and drag to move · drag the gaps to resize",
                            )
                            .small()
                            .color(pal.muted),
                        );
                        if ui.button("↺ Reset").clicked() {
                            self.tree = layout::default_tree();
                        }
                        if ui.add(pal.primary_button("✔ Done")).clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            self.editing_layout = false;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(status).small().color(pal.muted));
                    });
                });
            });
    }
}

impl eframe::App for RustCordApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.drain_events(ctx, &self.config);
        state.ack_open_channel(ctx, self.settings.is_none());
        if !self.config.modules.notifications && !state.inbox.items.is_empty() {
            // Module switched off: free the inbox.
            state.inbox = Inbox::default();
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
        let pal = self.config.theme.palette();

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
        let (status, fatal, wants_settings) = match &mut self.screen {
            Screen::Main(state) => (
                state.status.clone(),
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
        if wants_settings {
            self.settings = Some(SettingsPage::default());
            self.editing_layout = false;
        }

        // Settings page replaces the main UI while open.
        if let Some(page) = &mut self.settings {
            let Screen::Main(state) = &mut self.screen else {
                return;
            };
            let action = page.ui(ui, &mut self.config, state.me.as_ref(), &pal);
            match action {
                Some(SettingsAction::Close) => self.settings = None,
                Some(SettingsAction::EditLayout) => {
                    self.settings = None;
                    self.editing_layout = true;
                }
                Some(SettingsAction::ResetLayout) => self.tree = layout::default_tree(),
                Some(SettingsAction::LogOut) => self.log_out(None, true),
                None => {}
            }
            return;
        }

        if !self.config.modules.layout_editor {
            self.editing_layout = false;
        }
        self.top_bar(ui, &pal, &status);

        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut behavior = LayoutBehavior {
                state,
                config: &self.config,
                pal,
                editing: self.editing_layout,
            };
            self.tree.ui(&mut behavior, ui);
        });
        layout::end_frame(&self.tree, &ctx);

        state.overlays(ui, &pal);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
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

/// What the message box is currently doing.
enum Compose {
    New,
    Reply { message_id: String, author: String },
    Edit { message_id: String },
}

/// Something the user clicked on a message; applied after drawing.
enum MessageAction {
    Reply(String, String),
    Edit(String, String),
    Delete(String),
    Copy(String),
    MarkUnread(String),
}

struct Typer {
    user_id: String,
    name: String,
    since: Instant,
}

pub struct State {
    rx: Receiver<ToUi>,
    tx: UnboundedSender<ToBackend>,

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
            last_typing_sent: None,
            confirm_delete: None,
            reads: ReadState::default(),
            mutes: Mutes::default(),
            inbox: Inbox::default(),
            held_unread: None,
            ack_pending: None,
            last_ack_at: None,
            new_marker: None,
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
            } => {
                self.status = "Connected".into();
                self.connected = true;
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
                messages,
            } => {
                self.messages.insert(channel_id, messages);
            }
            ToUi::FilesPicked(paths) => self.add_files(paths),
            ToUi::NewMessage(msg) => {
                if let Some(typers) = self.typing.get_mut(&msg.channel_id) {
                    typers.retain(|t| t.user_id != msg.author.id);
                }
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
                    list.push(msg);
                    let cap = config.max_messages_per_channel.max(10);
                    if list.len() > cap {
                        let removed: Vec<Message> = list.drain(..list.len() - cap).collect();
                        forget_message_images(ctx, &removed);
                    }
                }
            }
            ToUi::MessageUpdated(patch) => {
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
                preview: notify::preview(&msg.content, 140),
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
                body: notify::preview(&msg.content, 200),
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
            && let Some(cid) = self.selected_channel.clone()
            && self.held_unread.as_ref() != Some(&cid)
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

    fn select_guild(&mut self, id: String) {
        if self.selected_guild.as_ref() != Some(&id) {
            self.selected_guild = Some(id);
            self.selected_channel = None;
            self.compose = Compose::New;
        }
    }

    fn select_channel(&mut self, channel_id: String, ctx: &egui::Context, config: &Config) {
        if !self.messages.contains_key(&channel_id) {
            let _ = self.tx.send(ToBackend::LoadHistory {
                channel_id: channel_id.clone(),
            });
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
        // Opening a channel reads it (user action, so ack right away).
        self.held_unread = None;
        self.mark_channel_read(&channel_id);

        // Most-recent-first list; free the oldest channels past the limit.
        self.recent_channels.retain(|c| c != &channel_id);
        self.recent_channels.push_front(channel_id.clone());
        while self.recent_channels.len() > config.cached_channels.max(1) {
            if let Some(old) = self.recent_channels.pop_back()
                && let Some(msgs) = self.messages.remove(&old)
            {
                forget_message_images(ctx, &msgs);
            }
        }
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

    fn servers_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let mut clicked = None;
        let mut mark_read = None;
        let now = notify::now_iso();
        egui::ScrollArea::vertical()
            .id_salt("servers")
            .auto_shrink(false)
            .show(ui, |ui| {
                let dm_selected = self.selected_guild.as_deref() == Some(DM_GUILD_ID);
                let badge = self.guild_badge(&self.dms, &now);
                let resp = server_button(
                    ui,
                    dm_selected,
                    None,
                    "💬",
                    "Direct Messages",
                    badge,
                    config,
                    pal,
                );
                if resp.clicked() {
                    clicked = Some(DM_GUILD_ID.to_string());
                }
                egui::Popup::context_menu(&resp).show(|ui| {
                    if ui.button("✔ Mark as read").clicked() {
                        mark_read = Some(DM_GUILD_ID.to_string());
                    }
                });
                ui.separator();
                for g in &self.guilds {
                    let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
                    let initials: String = g
                        .name
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .take(2)
                        .collect();
                    let badge = self.guild_badge(g, &now);
                    let resp = server_button(
                        ui,
                        selected,
                        g.icon_url.as_deref(),
                        &initials,
                        &g.name,
                        badge,
                        config,
                        pal,
                    );
                    if resp.clicked() {
                        clicked = Some(g.id.clone());
                    }
                    egui::Popup::context_menu(&resp).show(|ui| {
                        let any_unread = badge.unread || badge.mentions > 0;
                        if ui
                            .add_enabled(any_unread, egui::Button::new("✔ Mark as read"))
                            .clicked()
                        {
                            mark_read = Some(g.id.clone());
                        }
                        if badge.muted {
                            ui.label(RichText::new("🔕 Muted").small().color(pal.muted));
                        }
                    });
                }
            });
        if let Some(id) = mark_read {
            self.mark_guild_read(&id);
        }
        if let Some(id) = clicked {
            self.select_guild(id);
        }
    }

    /// Bottom-left user panel: avatar with status dot, name, settings gear.
    fn profile_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let gear = ui
                    .add(egui::Button::new(RichText::new("⚙").size(18.0)).frame(false))
                    .on_hover_text("User Settings");
                if gear.clicked() {
                    self.open_settings = true;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (name, uid, url) = match &self.me {
                        Some(me) => (me.name.as_str(), me.id.as_str(), me.avatar_url.as_str()),
                        None => ("…", "", ""),
                    };
                    let rect = avatar(ui, uid, name, url, 32.0, config, pal);
                    // Status dot
                    let dot = rect.right_bottom() - vec2(4.0, 4.0);
                    let col = if self.connected {
                        pal.online
                    } else {
                        pal.muted
                    };
                    ui.painter().circle_filled(dot, 6.0, pal.servers);
                    ui.painter().circle_filled(dot, 4.0, col);

                    if ui.available_width() > 40.0 {
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                            let status = if self.connected { "Online" } else { "Offline" };
                            ui.label(RichText::new(status).small().color(pal.muted));
                        });
                    }
                });
            });
        });
    }

    fn channels_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let Some(guild) = self.current_guild() else {
            ui.label(RichText::new("No server selected").color(pal.muted));
            return;
        };
        ui.add(egui::Label::new(RichText::new(&guild.name).strong().size(15.0)).truncate());
        ui.separator();

        let mut clicked = None;
        let mut first = None;
        let mut mark_read = None;
        let now = notify::now_iso();
        let guild_muted = self.mutes.guild_muted(&guild.id, &now);
        egui::ScrollArea::vertical()
            .id_salt("channels")
            .auto_shrink(false)
            .show(ui, |ui| {
                let groups = guild.channel_groups();
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
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for group in groups {
                        if let Some(cat) = group.category {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(cat.name.to_uppercase())
                                    .small()
                                    .strong()
                                    .color(pal.muted),
                            );
                        }
                        for c in group.channels {
                            first.get_or_insert_with(|| c.id.clone());
                            let selected = self.selected_channel.as_deref() == Some(c.id.as_str());
                            let muted = guild_muted || self.is_muted(&guild.id, c, &now);
                            let unread = !muted && self.reads.is_unread(&c.id);
                            let mentions = if muted { 0 } else { self.reads.mentions(&c.id) };
                            let mut text = RichText::new(c.label());
                            text = if selected {
                                text.color(pal.text)
                            } else if muted {
                                text.color(pal.muted_text())
                            } else if unread || mentions > 0 {
                                text.color(pal.unread).strong()
                            } else {
                                text.color(pal.muted)
                            };
                            let resp = ui.add(egui::Button::selectable(selected, text).truncate());
                            if unread && !selected {
                                unread_pill(ui, resp.rect, 8.0, pal);
                            }
                            if mentions > 0 {
                                mention_badge(
                                    ui,
                                    egui::pos2(resp.rect.right() - 6.0, resp.rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    mentions,
                                    pal.sidebar,
                                    pal,
                                );
                            }
                            if resp.clicked() {
                                clicked = Some(c.id.clone());
                            }
                            egui::Popup::context_menu(&resp).show(|ui| {
                                if ui
                                    .add_enabled(
                                        unread || mentions > 0,
                                        egui::Button::new("✔ Mark as read"),
                                    )
                                    .clicked()
                                {
                                    mark_read = Some(c.id.clone());
                                }
                                if muted {
                                    ui.label(RichText::new("🔕 Muted").small().color(pal.muted));
                                }
                            });
                        }
                    }
                });
            });

        if let Some(id) = mark_read {
            self.mark_channel_read(&id);
        }
        let auto = if self.selected_channel.is_none() {
            first
        } else {
            None
        };
        if let Some(id) = clicked.or(auto) {
            let ctx = ui.ctx().clone();
            self.select_channel(id, &ctx, config);
        }
    }

    fn messages_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        // Header: channel name, 🔔 inbox on the right.
        let title = self.channel_title();
        if title.is_some() || config.modules.notifications {
            ui.horizontal(|ui| {
                if let Some(title) = &title {
                    ui.label(RichText::new(title).strong().size(15.0));
                }
                if config.modules.notifications {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.inbox_button(ui, config, pal);
                    });
                }
            });
            ui.separator();
        }
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Pick a channel").color(pal.muted))
            });
            return;
        };
        let Some(messages) = self.messages.get(&channel_id) else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        if messages.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No messages yet").color(pal.muted))
            });
            return;
        }

        let my_id = self.me.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
        let mut action = None;
        let mut new_after = self
            .new_marker
            .as_ref()
            .filter(|(c, _)| *c == channel_id)
            .map(|(_, acked)| *acked);

        egui::ScrollArea::vertical()
            .id_salt(("messages", &channel_id))
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let mut prev: Option<&Message> = None;
                for m in messages {
                    let mut grouped =
                        m.reply.is_none() && prev.is_some_and(|p| p.author.id == m.author.id);
                    if let Some(acked) = new_after
                        && m.id.parse::<u64>().is_ok_and(|id| id > acked)
                        && m.author.id != my_id
                    {
                        new_divider(ui, pal);
                        new_after = None;
                        grouped = false;
                    }
                    if let Some(a) = message_row(ui, m, grouped, m.author.id == my_id, config, pal)
                    {
                        action = Some(a);
                    }
                    prev = Some(m);
                }
            });

        match action {
            Some(MessageAction::Reply(id, author)) => {
                self.compose = Compose::Reply {
                    message_id: id,
                    author,
                }
            }
            Some(MessageAction::Edit(id, content)) => {
                self.draft = content;
                self.compose = Compose::Edit { message_id: id };
            }
            Some(MessageAction::Delete(id)) => self.confirm_delete = Some((channel_id, id)),
            Some(MessageAction::Copy(text)) => ui.ctx().copy_text(text),
            Some(MessageAction::MarkUnread(id)) => self.mark_unread(&channel_id, &id),
            None => {}
        }
    }

    /// 🔔 button with a red dot when there are unread mentions; opens the
    /// inbox popup. Clicking a mention jumps to its channel.
    fn inbox_button(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let unread = self.inbox.unread();
        ui.add_space(10.0);
        let bell = ui
            .add(egui::Button::new(RichText::new("🔔").size(16.0)).frame(false))
            .on_hover_text("Inbox: recent mentions");
        if unread > 0 {
            mention_badge(
                ui,
                bell.rect.right_top() + vec2(10.0, -2.0),
                egui::Align2::RIGHT_TOP,
                unread as u32,
                pal.chat,
                pal,
            );
        }
        let mut jump = None;
        let mut clear = false;
        egui::Popup::menu(&bell)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_width(340.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Inbox").strong().size(15.0));
                    ui.label(RichText::new("Mentions").small().color(pal.muted));
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
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.label(RichText::new("Pick a channel to start chatting").color(pal.muted));
            return;
        };

        let mut cancel = false;
        let mut send = false;
        let mut changed = false;
        let mut remove = None;

        // Built bottom-up (like Discord) so the input row always stays visible
        // and extra rows (files, reply banner) grow upwards.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            // Who's typing (under the box)
            let typing = config
                .modules
                .typing_indicators
                .then(|| self.typing.get(&channel_id))
                .flatten()
                .filter(|t| !t.is_empty());
            let typing_text =
                match typing.map(|t| t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()) {
                    Some(names) => match names.as_slice() {
                        [one] => format!("{one} is typing…"),
                        [a, b] => format!("{a} and {b} are typing…"),
                        _ => "Several people are typing…".into(),
                    },
                    None => String::new(),
                };
            ui.label(
                RichText::new(typing_text)
                    .small()
                    .italics()
                    .color(pal.muted),
            );

            // [+] [ message …               ] [➤]
            egui::Frame::new()
                .fill(pal.input)
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let plus = ui
                            .add(egui::Button::new(RichText::new("➕").size(16.0)).frame(false))
                            .on_hover_text("Upload files (or drag them onto the window)");
                        if plus.clicked() {
                            let _ = self.tx.send(ToBackend::PickFiles);
                        }

                        let hint = match self.channel_title() {
                            Some(t) => format!("Message {t}"),
                            None => "Message".into(),
                        };
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.draft)
                                .hint_text(hint)
                                .frame(egui::Frame::NONE)
                                .desired_width(ui.available_width() - 40.0),
                        );
                        changed = resp.changed();
                        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            cancel = true;
                        }
                        let enter =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let can_send =
                            !self.draft.trim().is_empty() || !self.pending_files.is_empty();
                        let send_btn = ui
                            .add_enabled(
                                can_send,
                                egui::Button::new(RichText::new("➤").color(Color32::WHITE))
                                    .fill(pal.accent)
                                    .corner_radius(6),
                            )
                            .on_hover_text("Send");
                        if enter || send_btn.clicked() {
                            send = true;
                            resp.request_focus();
                        }
                    });
                });

            // Files waiting to upload
            if !self.pending_files.is_empty() {
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        let mut total = 0;
                        for (i, p) in self.pending_files.iter().enumerate() {
                            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                            total += size;
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            egui::Frame::new()
                                .fill(pal.input)
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(format!("📄 {name}")).small());
                                    ui.label(
                                        RichText::new(human_size(size)).small().color(pal.muted),
                                    );
                                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                        }
                        if total > UPLOAD_LIMIT {
                            ui.label(
                                RichText::new("Over 10 MB, Discord may reject this")
                                    .small()
                                    .color(pal.danger),
                            );
                        }
                    },
                );
            }

            // Reply / edit banner
            let banner = match &self.compose {
                Compose::New => None,
                Compose::Reply { author, .. } => Some(format!("Replying to {author}")),
                Compose::Edit { .. } => Some("Editing message · Esc to cancel".to_string()),
            };
            if let Some(text) = banner {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).small().color(pal.muted));
                    cancel |= ui.small_button("✕").clicked();
                });
            }
        });

        if let Some(i) = remove {
            self.pending_files.remove(i);
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

    /// People in the current DM, or recently active in the current channel.
    fn members_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
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

        let (title, people): (&str, Vec<User>) = match self.dms.channel(&channel_id) {
            Some(dm) => {
                let mut people = dm.recipients.clone();
                people.extend(self.me.clone());
                ("MEMBERS", people)
            }
            None => {
                let mut seen: Vec<User> = Vec::new();
                if let Some(msgs) = self.messages.get(&channel_id) {
                    for m in msgs.iter().rev() {
                        if !seen.iter().any(|u| u.id == m.author.id) {
                            seen.push(m.author.clone());
                        }
                    }
                }
                ("RECENTLY ACTIVE", seen)
            }
        };

        ui.label(
            RichText::new(format!("{title} — {}", people.len()))
                .small()
                .strong()
                .color(pal.muted),
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("members")
            .auto_shrink(false)
            .show(ui, |ui| {
                for u in &people {
                    ui.horizontal(|ui| {
                        avatar(ui, &u.id, &u.name, &u.avatar_url, 28.0, config, pal);
                        ui.add(
                            egui::Label::new(RichText::new(&u.name).color(pal.muted)).truncate(),
                        );
                        if u.bot {
                            ui.label(
                                RichText::new(" BOT ")
                                    .small()
                                    .color(Color32::WHITE)
                                    .background_color(pal.accent),
                            );
                        }
                    });
                    ui.add_space(2.0);
                }
            });
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
                files,
            },
            Compose::Reply { message_id, .. } => ToBackend::Send {
                channel_id,
                content,
                reply_to: Some(message_id),
                files,
            },
        };
        let _ = self.tx.send(req);
        self.draft.clear();
        self.last_typing_sent = None;
    }

    /// Delete confirmation and the "drop files here" overlay.
    fn overlays(&mut self, ui: &mut egui::Ui, pal: &Palette) {
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

/// Free the GPU/CPU copies of attachment images for messages we no longer keep.
fn forget_message_images(ctx: &egui::Context, messages: &[Message]) {
    let (w, h) = IMAGE_MAX;
    for m in messages {
        for a in m.attachments.iter().filter(|a| a.is_image()) {
            ctx.forget_image(&a.thumbnail_url(w as u32, h as u32));
        }
    }
}

// ===========================================================================
// Widgets
// ===========================================================================

/// Round avatar. With the Images module off, a coloured circle with the
/// first letter instead (no download, no texture).
fn avatar(
    ui: &mut egui::Ui,
    user_id: &str,
    name: &str,
    url: &str,
    size: f32,
    config: &Config,
    pal: &Palette,
) -> egui::Rect {
    if config.modules.images && !url.is_empty() {
        ui.add(
            egui::Image::from_uri(url)
                .fit_to_exact_size(vec2(size, size))
                .corner_radius(size / 2.0),
        )
        .rect
    } else {
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
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
            egui::FontId::proportional(size * 0.45),
            pal.text,
        );
        rect
    }
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
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), egui::Sense::hover());
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
        egui::FontId::proportional(9.5),
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

/// A server entry: icon (or initials) plus name, with an unread pill, a red
/// mention badge, and greyed out when muted.
#[allow(clippy::too_many_arguments)]
fn server_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: Option<&str>,
    fallback: &str,
    name: &str,
    badge: Badge,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
    let mut icon_rect = egui::Rect::NOTHING;
    let resp = ui
        .horizontal(|ui| {
            let size = vec2(32.0, 32.0);
            match icon.filter(|_| config.modules.images) {
                Some(url) => {
                    let tint = if badge.muted {
                        Color32::from_gray(110)
                    } else {
                        Color32::WHITE
                    };
                    icon_rect = ui
                        .add(
                            egui::Image::from_uri(url)
                                .fit_to_exact_size(size)
                                .corner_radius(16)
                                .tint(tint),
                        )
                        .rect;
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    icon_rect = rect;
                    let fill = if selected { pal.accent } else { pal.sidebar };
                    ui.painter().circle_filled(rect.center(), 16.0, fill);
                    let col = if badge.muted && !selected {
                        pal.muted_text()
                    } else {
                        pal.text
                    };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        fallback,
                        egui::FontId::proportional(13.0),
                        col,
                    );
                }
            }
            if ui.available_width() > 30.0 {
                let mut text = RichText::new(name);
                if badge.muted && !selected {
                    text = text.color(pal.muted_text());
                } else if badge.unread && !selected {
                    text = text.color(pal.unread).strong();
                }
                ui.add(egui::Button::selectable(selected, text).truncate())
            } else {
                ui.allocate_response(vec2(0.0, 0.0), egui::Sense::click())
            }
        })
        .response;
    if selected {
        unread_pill(ui, icon_rect, 28.0, pal);
    } else if badge.unread {
        unread_pill(ui, icon_rect, 8.0, pal);
    }
    if badge.mentions > 0 {
        mention_badge(
            ui,
            icon_rect.right_bottom() + vec2(4.0, 2.0),
            egui::Align2::RIGHT_BOTTOM,
            badge.mentions,
            pal.servers,
            pal,
        );
    }
    let resp = ui.interact(resp.rect, resp.id.with("server"), egui::Sense::click());
    let hover = if badge.muted {
        format!("{name} (muted)")
    } else {
        name.to_string()
    };
    resp.on_hover_text(hover)
}

/// Draw one message. Returns an action if the user picked one from its menu.
fn message_row(
    ui: &mut egui::Ui,
    m: &Message,
    grouped: bool,
    mine: bool,
    config: &Config,
    pal: &Palette,
) -> Option<MessageAction> {
    let mut action = None;
    if !grouped {
        ui.add_space(8.0);
    }

    ui.horizontal_top(|ui| {
        let avatar_size = 36.0;
        if grouped {
            ui.add_space(avatar_size + ui.spacing().item_spacing.x);
        } else {
            avatar(
                ui,
                &m.author.id,
                &m.author.name,
                &m.author.avatar_url,
                avatar_size,
                config,
                pal,
            );
        }

        ui.vertical(|ui| {
            if let Some(reply) = &m.reply {
                ui.label(
                    RichText::new(format!("↪ {}: {}", reply.author, reply.content))
                        .small()
                        .color(pal.muted),
                );
            }
            if !grouped {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&m.author.name).strong());
                    if m.author.bot {
                        ui.label(
                            RichText::new(" BOT ")
                                .small()
                                .color(Color32::WHITE)
                                .background_color(pal.accent),
                        );
                    }
                    ui.label(RichText::new(m.short_time()).small().color(pal.muted));
                });
            }

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let mut text_responses = Vec::new();
                if !m.content.is_empty() {
                    if config.modules.emoji_images {
                        let segs = emoji::segments(&m.content);
                        let size = if emoji::is_jumbo(&segs) { 44.0 } else { 20.0 };
                        for seg in segs {
                            match seg {
                                Segment::Text(t) => text_responses
                                    .push(ui.add(egui::Label::new(t).sense(egui::Sense::click()))),
                                Segment::Emoji { url, alt } => {
                                    ui.add(
                                        egui::Image::from_uri(url)
                                            .fit_to_exact_size(vec2(size, size)),
                                    )
                                    .on_hover_text(alt);
                                }
                            }
                        }
                    } else {
                        text_responses
                            .push(ui.add(egui::Label::new(&m.content).sense(egui::Sense::click())));
                    }
                }
                ui.spacing_mut().item_spacing.x = 6.0;
                if m.edited {
                    ui.add_space(4.0);
                    ui.label(RichText::new("(edited)").small().color(pal.muted));
                }
                ui.add_space(4.0);
                let dots = ui
                    .add(egui::Button::new(RichText::new("⋯").color(pal.muted)).frame(false))
                    .on_hover_text("More");
                egui::Popup::menu(&dots).show(|ui| message_menu(ui, m, mine, &mut action));
                // Right-click the text for the same menu.
                for r in &text_responses {
                    egui::Popup::context_menu(r).show(|ui| message_menu(ui, m, mine, &mut action));
                }
            });

            for a in &m.attachments {
                attachment_ui(ui, a, config, pal);
            }
        });
    });
    action
}

fn message_menu(ui: &mut egui::Ui, m: &Message, mine: bool, action: &mut Option<MessageAction>) {
    if ui.button("↪ Reply").clicked() {
        *action = Some(MessageAction::Reply(m.id.clone(), m.author.name.clone()));
    }
    if mine && ui.button("✏ Edit").clicked() {
        *action = Some(MessageAction::Edit(m.id.clone(), m.content.clone()));
    }
    if ui.button("📋 Copy text").clicked() {
        *action = Some(MessageAction::Copy(m.content.clone()));
    }
    if ui.button("🆔 Copy message ID").clicked() {
        *action = Some(MessageAction::Copy(m.id.clone()));
    }
    if ui.button("👁 Mark unread").clicked() {
        *action = Some(MessageAction::MarkUnread(m.id.clone()));
    }
    if mine && ui.button("🗑 Delete").clicked() {
        *action = Some(MessageAction::Delete(m.id.clone()));
    }
}

fn attachment_ui(ui: &mut egui::Ui, a: &Attachment, config: &Config, pal: &Palette) {
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
            ui.ctx().open_url(egui::OpenUrl::new_tab(&a.url));
        }
    } else {
        egui::Frame::new()
            .fill(pal.sidebar)
            .corner_radius(6)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("📄").size(20.0));
                    ui.vertical(|ui| {
                        ui.hyperlink_to(&a.filename, &a.url);
                        ui.label(RichText::new(a.human_size()).small().color(pal.muted));
                    });
                });
            });
    }
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
