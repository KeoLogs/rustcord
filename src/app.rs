//! The RustCord UI: login screen, main layout, settings page.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, vec2};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{CONFIG_KEY, Config};
use crate::emoji::{self, Segment};
use crate::layout::{self, LAYOUT_KEY, LayoutBehavior, Pane};
use crate::login::{LoginAction, LoginForm};
use crate::members::{self, Member, MemberCache, MemberList, Row};
use crate::notify::{self, Inbox, InboxItem, Mutes, NotifyCheck};
use crate::presence::{Presences, Status};
use crate::profile::{self, Popout, PopoutAction, ProfileCache};
use crate::read_state::ReadState;
use crate::settings::{SettingsAction, SettingsPage};
use crate::theme::{self, Bold, Palette, ThemeChoice};
use crate::timefmt;
use crate::types::{Attachment, CHANNEL_DM, DM_GUILD_ID, Guild, Message, ToBackend, ToUi, User};

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
    /// Status bar module's RAM reading.
    ram: RamSampler,
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
            ram: RamSampler::default(),
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
        let options = crate::backend::Options {
            bot_member_intents: self.config.bot_member_intents,
        };
        crate::backend::spawn(token, demo, options, ui_tx, be_rx, ctx.clone());
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
                        ui.label(RichText::new("✏ Editing layout").bold().color(pal.accent));
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
        state.drain_events(ctx, &self.config);
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
        self.top_bar(ui, &pal, &status, connected);

        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if self.config.modules.status_bar {
            let ram = self.ram.sample();
            state.status_bar(ui, &self.config, &pal, ram);
        }
        state.members_visible = layout::members_visible(&self.tree);
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
        if std::mem::take(&mut state.toggle_members) {
            layout::toggle_members(&mut self.tree);
        }

        state.overlays(ui, &self.config, &pal);
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
    /// Clicked an avatar or name: open their profile next to this rect.
    Profile(User, egui::Rect),
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
    /// The 👥 header button was clicked; the app toggles the panel.
    toggle_members: bool,
    /// Reply / Edit was picked: focus the message box next frame.
    focus_composer: bool,
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
            focus_composer: false,
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
                messages,
            } => {
                self.request_unknown_authors(&channel_id, &messages);
                self.messages.insert(channel_id, messages);
            }
            ToUi::FilesPicked(paths) => self.add_files(paths),
            ToUi::NewMessage(mut msg) => {
                if let (Some(gid), Some(member)) = (&msg.guild_id, msg.member.take()) {
                    self.member_cache
                        .insert_cached(gid, &msg.author.id, *member);
                }
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

    /// Discord's server rail: round icons, a white pill on the left for
    /// unread / hovered / selected, Home (DMs) at the top. Wider than
    /// `RAIL_NAMES_WIDTH` (the user dragged it), it shows names too.
    fn servers_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let mut clicked = None;
        let mut mark_read = None;
        let now = notify::now_iso();
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
                    clicked = Some(DM_GUILD_ID.to_string());
                }
                egui::Popup::context_menu(&resp).show(|ui| {
                    if ui.button("✔ Mark as read").clicked() {
                        mark_read = Some(DM_GUILD_ID.to_string());
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
                for g in &self.guilds {
                    let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
                    let initials: String = g
                        .name
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .take(3)
                        .collect();
                    let badge = self.guild_badge(g, &now);
                    let icon = match &g.icon_url {
                        Some(url) if config.modules.images => RailIcon::Image(url),
                        _ => RailIcon::Initials(&initials),
                    };
                    let resp = rail_item(ui, icon, &g.name, selected, badge, config, pal);
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

    /// Bottom-left user panel: avatar with status dot, bold name, muted
    /// status, ⚙ settings. In the narrow default rail it stacks the avatar
    /// over the gear.
    fn profile_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let (name, uid, url) = match &self.me {
            Some(me) => (me.name.clone(), me.id.clone(), me.avatar_url.clone()),
            None => ("…".into(), String::new(), String::new()),
        };
        let status = if self.connected { "Online" } else { "Offline" };
        let dot = if self.connected {
            pal.online
        } else {
            pal.offline
        };
        let draw_avatar = |ui: &mut egui::Ui| {
            let rect = avatar(ui, &uid, &name, &url, 32.0, config, pal);
            let c = rect.right_bottom() - vec2(4.0, 4.0);
            ui.painter().circle_filled(c, 6.0, pal.servers);
            ui.painter().circle_filled(c, 4.0, dot);
            ui.interact(rect, ui.scope_id().with("me_avatar"), egui::Sense::hover())
                .on_hover_text(format!("{name} · {status}"));
        };
        let mut open = false;
        if ui.available_width() < 120.0 {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                draw_avatar(ui);
                open = icon_button(ui, "⚙", "User Settings", false, true, pal).clicked();
            });
        } else {
            ui.horizontal_centered(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    open = icon_button(ui, "⚙", "User Settings", false, true, pal).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        draw_avatar(ui);
                        ui.add_space(2.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&name).bold().size(14.5).color(pal.strong),
                                )
                                .truncate()
                                .selectable(false),
                            );
                            ui.add(
                                egui::Label::new(RichText::new(status).size(12.0).color(pal.muted))
                                    .selectable(false),
                            );
                        });
                    });
                });
            });
        }
        if open {
            self.open_settings = true;
        }
    }

    fn channels_pane(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let Some(guild) = self.current_guild() else {
            let rect = header_bar(ui, pal);
            ui.painter().text(
                rect.left_center() + vec2(16.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "No server selected",
                egui::FontId::proportional(14.5),
                pal.muted,
            );
            return;
        };
        // Header: server name, with a border + soft shadow under it.
        let rect = header_bar(ui, pal);
        let galley = truncated(
            ui,
            &guild.name,
            theme::bold_font(15.0),
            pal.strong,
            rect.width() - 32.0,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0),
            galley,
            pal.strong,
        );

        let mut clicked = None;
        let mut first = None;
        let mut mark_read = None;
        let mut toggle_cat = None;
        let now = notify::now_iso();
        let guild_muted = self.mutes.guild_muted(&guild.id, &now);
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
                            ui.add_space(8.0);
                            ui.label(pal.section_label("Direct Messages"));
                            ui.add_space(2.0);
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
                                let selected =
                                    self.selected_channel.as_deref() == Some(c.id.as_str());
                                let muted = guild_muted || self.is_muted(&guild.id, c, &now);
                                let unread = !c.is_voice() && !muted && self.reads.is_unread(&c.id);
                                let mentions = if muted { 0 } else { self.reads.mentions(&c.id) };
                                // Collapsed categories still show the open
                                // channel and unread ones, like Discord.
                                if collapsed && !selected && !unread && mentions == 0 {
                                    continue;
                                }
                                if !c.is_voice() {
                                    first.get_or_insert_with(|| c.id.clone());
                                }
                                let status = (c.kind == CHANNEL_DM && config.modules.member_list)
                                    .then(|| c.recipients.first())
                                    .flatten()
                                    .and_then(|u| self.presences.status(&u.id));
                                let row = ChannelRow {
                                    selected,
                                    muted,
                                    unread,
                                    mentions,
                                    status,
                                };
                                let resp = channel_row(ui, c, &row, config, pal);
                                if resp.clicked() && !c.is_voice() {
                                    clicked = Some(c.id.clone());
                                }
                                if c.is_voice() {
                                    continue;
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
                                        ui.label(
                                            RichText::new("🔕 Muted").small().color(pal.muted),
                                        );
                                    }
                                });
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

    /// Chat header: channel icon + bold name, topic, and the 🔔 / 📌 / 👥
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
                "👥",
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
        icon_button(ui, "📌", "Pins — coming next", false, false, pal);
        if config.modules.notifications {
            self.inbox_button(ui, config, pal);
        }
        ui.add_space(8.0);
        let Some((icon, name, topic, dm)) = channel else {
            return;
        };
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(4.0);
            ui.label(
                RichText::new(icon)
                    .size(if icon == "#" { 20.0 } else { 18.0 })
                    .color(pal.muted),
            );
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
        self.chat_header(ui, config, pal);
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
        // Server nicknames and role colours for author names.
        let guild = self
            .find_channel(&channel_id)
            .map(|(g, _)| g)
            .filter(|g| !g.is_dms());

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
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(8.0);
                let mut prev: Option<&Message> = None;
                for m in messages {
                    let day = timefmt::local_day(m.time);
                    let new_day =
                        day.is_some() && prev.and_then(|p| timefmt::local_day(p.time)) != day;
                    // Discord groups messages by the same person sent within 7 minutes.
                    let mut grouped = m.reply.is_none()
                        && !new_day
                        && prev.is_some_and(|p| {
                            p.author.id == m.author.id && (m.time - p.time).abs() < 7 * 60
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
                    let author = self.author_style(guild, &m.author);
                    let mentions_me = !my_id.is_empty()
                        && m.author.id != my_id
                        && (m.mention_everyone || m.mentions.iter().any(|id| id == my_id));
                    let ctx = RowCtx {
                        grouped,
                        mine: m.author.id == my_id,
                        mentions_me,
                    };
                    if let Some(a) = message_row(ui, m, &author, &ctx, config, pal) {
                        action = Some(a);
                    }
                    prev = Some(m);
                }
                ui.add_space(20.0);
            });

        match action {
            Some(MessageAction::Reply(id, author)) => {
                self.compose = Compose::Reply {
                    message_id: id,
                    author,
                };
                self.focus_composer = true;
            }
            Some(MessageAction::Edit(id, content)) => {
                self.draft = content;
                self.compose = Compose::Edit { message_id: id };
                self.focus_composer = true;
            }
            Some(MessageAction::Delete(id)) => self.confirm_delete = Some((channel_id, id)),
            Some(MessageAction::Copy(text)) => ui.ctx().copy_text(text),
            Some(MessageAction::MarkUnread(id)) => self.mark_unread(&channel_id, &id),
            Some(MessageAction::Profile(user, rect)) => {
                let guild = self
                    .find_channel(&channel_id)
                    .filter(|(g, _)| !g.is_dms())
                    .map(|(g, _)| g.id.clone());
                let ctx = ui.ctx().clone();
                self.open_profile(user, guild, rect, &ctx);
            }
            None => {}
        }
    }

    /// 🔔 button with a red dot when there are unread mentions; opens the
    /// inbox popup. Clicking a mention jumps to its channel.
    fn inbox_button(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        let unread = self.inbox.unread();
        let bell = icon_button(ui, "🔔", "Inbox: recent mentions", false, true, pal);
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
            ui.add_space(12.0);
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
            box_frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if plus_button(ui, pal).clicked() {
                        let _ = self.tx.send(ToBackend::PickFiles);
                    }
                    ui.add_space(8.0);
                    let can_send = !self.draft.trim().is_empty() || !self.pending_files.is_empty();
                    let hint = match self
                        .find_channel(&channel_id)
                        .map(|(_, c)| (c.is_dm(), c.name.clone()))
                    {
                        Some((true, name)) => format!("Message @{name}"),
                        Some((false, name)) => format!("Message #{name}"),
                        None => "Message".into(),
                    };
                    let send_w = if can_send {
                        24.0 + ui.spacing().item_spacing.x
                    } else {
                        0.0
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.draft)
                            .hint_text(RichText::new(hint).color(pal.muted))
                            .font(egui::FontId::proportional(15.0))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_width(ui.available_width() - send_w),
                    );
                    if std::mem::take(&mut self.focus_composer) {
                        resp.request_focus();
                    }
                    changed = resp.changed();
                    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    // Discord has no send button; a subtle one shows only
                    // when there's something to send.
                    let send_btn = can_send && send_button(ui, pal).clicked();
                    if enter || send_btn {
                        send = true;
                        resp.request_focus();
                    }
                });
            });

            // Reply / edit banner, attached to the top of the box.
            let banner = match &self.compose {
                Compose::New => None,
                Compose::Reply { author, .. } => Some(("Replying to ", author.clone())),
                Compose::Edit { .. } => Some(("Editing message", String::new())),
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
                                            egui::Button::new(RichText::new("✕").color(pal.muted))
                                                .frame(false),
                                        )
                                        .on_hover_text("Cancel (Esc)");
                                    cancel |= x.clicked();
                                },
                            );
                        });
                    });
            }

            // Files waiting to upload
            if !self.pending_files.is_empty() {
                ui.add_space(6.0);
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
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
                                .inner_margin(egui::Margin::symmetric(8, 4))
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

    /// The member list: Discord's grouped list for servers (op 14 for user
    /// accounts, the full list for bots with the Server Members intent),
    /// people in the conversation for DMs, and "recently active" otherwise.
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
        if let Some(l) = list.filter(|l| l.member_count > 0) {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{} members · {} online",
                        l.member_count, l.online_count
                    ))
                    .size(11.5)
                    .color(pal.muted),
                );
            });
        }
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

    /// Status bar module: a thin bar at the bottom like Discord Messenger's
    /// (connection, where you are, who's typing, RAM).
    fn status_bar(&self, ui: &mut egui::Ui, config: &Config, pal: &Palette, ram: Option<u64>) {
        egui::Panel::bottom("status_bar")
            .exact_size(22.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.servers)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let small = |t: &str| RichText::new(t).size(11.5).color(pal.muted);
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    let dot = if self.connected { pal.online } else { pal.idle };
                    ui.painter().circle_filled(r.center(), 4.0, dot);
                    ui.label(small(&self.status));
                    if let Some(cid) = &self.selected_channel
                        && let Some((g, c)) = self.find_channel(cid)
                    {
                        ui.separator();
                        let place = if g.is_dms() {
                            format!("{} {}", c.icon(), c.name)
                        } else {
                            format!("{} {} · {}", c.icon(), c.name, g.name)
                        };
                        ui.add(egui::Label::new(small(&place)).truncate());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(bytes) = ram {
                            ui.label(small(&format!("RAM {:.0} MB", bytes as f64 / 1_048_576.0)));
                            ui.ctx().request_repaint_after(Duration::from_secs(2));
                        }
                        let typing = config
                            .modules
                            .typing_indicators
                            .then_some(self.selected_channel.as_ref())
                            .flatten()
                            .and_then(|c| self.typing.get(c))
                            .filter(|t| !t.is_empty());
                        if let Some(t) = typing {
                            ui.separator();
                            let text = match t.as_slice() {
                                [one] => format!("{} is typing…", one.name),
                                _ => format!("{} people are typing…", t.len()),
                            };
                            ui.label(small(&text));
                        }
                    });
                });
            });
    }

    /// Profile card, delete confirmation and the "drop files here" overlay.
    fn overlays(&mut self, ui: &mut egui::Ui, config: &Config, pal: &Palette) {
        self.popout_overlay(ui.ctx(), config, pal);

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
    let chevron = if collapsed { "▸" } else { "▾" };
    ui.painter().text(
        rect.left_center() + vec2(2.0, 0.0),
        egui::Align2::LEFT_CENTER,
        chevron,
        egui::FontId::proportional(11.0),
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
}

/// One channel in the list: "# name" (32 px) or, for DMs, avatar + name
/// (42 px); hover / selected backgrounds, white unread text and pill,
/// mention badge. Voice channels are listed greyed out.
fn channel_row(
    ui: &mut egui::Ui,
    c: &crate::types::Channel,
    row: &ChannelRow,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
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
                    "👥",
                    egui::FontId::proportional(15.0),
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
            egui::FontId::proportional(if c.icon() == "#" { 20.0 } else { 15.0 }),
            if row.selected || hovered || bold {
                color
            } else {
                pal.muted.gamma_multiply(0.85)
            },
        );
        x += 28.0;
    }
    let right_pad = if row.mentions > 0 { 34.0 } else { 8.0 };
    let galley = truncated(ui, &c.name, font, color, rect.right() - right_pad - x);
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
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
        return resp.on_hover_text("Voice channels are coming later");
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    match row.status.filter(|_| dm) {
        Some(st) => resp.on_hover_text(st.label()),
        None => resp,
    }
}

/// Discord's ⊕ upload button: a filled circle with a plus cut out.
fn plus_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
    let col = if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    let c = rect.center();
    ui.painter().circle_filled(c, 11.0, col);
    let stroke = egui::Stroke::new(2.0, pal.input);
    ui.painter().hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
    ui.painter().vline(c.x, c.y - 5.0..=c.y + 5.0, stroke);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Upload files (or drag them onto the window)")
}

/// The subtle ➤ at the right of the message box (only when there's
/// something to send).
fn send_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 20.0), egui::Sense::click());
    let col = if resp.hovered() {
        pal.accent
    } else {
        pal.muted
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "➤",
        egui::FontId::proportional(18.0),
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Send (Enter)")
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

/// A square icon button like Discord's header / user-panel icons: muted
/// glyph, brighter with a soft background on hover, white when active.
fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tooltip: &str,
    active: bool,
    enabled: bool,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::click());
    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect.shrink(2.0), 4, pal.hover);
    }
    let col = if !enabled {
        pal.muted_text()
    } else if active || hovered {
        pal.strong
    } else {
        pal.muted
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(18.0),
        col,
    );
    let resp = if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    resp.on_hover_text(tooltip)
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
    const SIZE: f32 = 48.0;
    let width = ui.available_width();
    let wide = width >= RAIL_NAMES_WIDTH;
    let (row, resp) = ui.allocate_exact_size(vec2(width, SIZE), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let ctx = ui.ctx().clone();

    // Circle (radius 24) → rounded square (radius 16).
    let t = ctx.animate_bool_with_time(resp.id.with("shape"), selected || hovered, 0.12);
    let radius = egui::lerp(SIZE / 2.0..=16.0, t);
    let x = if wide {
        row.left() + 12.0
    } else {
        row.center().x - SIZE / 2.0
    };
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), vec2(SIZE, SIZE));
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
                _ => ("💬", egui::FontId::proportional(20.0)),
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

/// Lay out one line of text, cut off with "…" past `max_width`.
fn truncated(
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

/// Per-message flags for `message_row`.
struct RowCtx {
    /// Follow-up from the same author: no avatar / name line.
    grouped: bool,
    mine: bool,
    /// Mentions me (or @everyone): Discord's yellow highlight.
    mentions_me: bool,
}

/// Message layout, in px (Discord's): 16 px left padding, 40 px avatar,
/// content starts at 72 px; groups start 17 px apart.
const MSG_PAD: f32 = 16.0;
const MSG_AVATAR: f32 = 40.0;
const MSG_GUTTER: f32 = 72.0;
const GROUP_GAP: f32 = 17.0;

/// Draw one message. Returns an action if the user picked one (hover bar,
/// right-click menu, avatar / name click).
fn message_row(
    ui: &mut egui::Ui,
    m: &Message,
    author: &AuthorStyle<'_>,
    rc: &RowCtx,
    config: &Config,
    pal: &Palette,
) -> Option<MessageAction> {
    let mut action = None;
    if !rc.grouped {
        ui.add_space(GROUP_GAP - 2.0);
    }
    let more_id = egui::Id::unique(("msg_more", &m.id));
    let menu_open = egui::Popup::is_id_open(ui.ctx(), more_id.with("popup"));
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
                    top: 2,
                    bottom: 2,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(reply) = &m.reply {
                        reply_preview(ui, reply, config, pal);
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
                            ui.add_space(0.0);
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
                                    let name = RichText::new(author.name)
                                        .bold()
                                        .size(15.0)
                                        .color(author.color.unwrap_or(pal.strong));
                                    let resp = ui
                                        .add(
                                            egui::Label::new(name)
                                                .sense(egui::Sense::click())
                                                .selectable(false),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if resp.clicked() {
                                        action = Some(MessageAction::Profile(
                                            m.author.clone(),
                                            resp.rect,
                                        ));
                                    }
                                    if m.author.bot {
                                        members::bot_tag(ui, pal);
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
                            let texts = message_body(ui, m, pal, config);
                            for r in &texts {
                                egui::Popup::context_menu(r)
                                    .show(|ui| message_menu(ui, m, rc.mine, &mut action));
                            }
                            for a in &m.attachments {
                                attachment_ui(ui, a, config, pal);
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

    // Backgrounds: mention highlight, then hover.
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

    egui::Popup::context_menu(&resp).show(|ui| message_menu(ui, m, rc.mine, &mut action));

    if hovered {
        hover_bar(ui, m, rc.mine, rect, more_id, bar_id, &mut action, pal);
    }
    action
}

/// Text, emoji and "(edited)". Returns the text labels (for right-click).
fn message_body(
    ui: &mut egui::Ui,
    m: &Message,
    pal: &Palette,
    config: &Config,
) -> Vec<egui::Response> {
    let mut text_responses = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if !m.content.is_empty() {
            if config.modules.emoji_images {
                let segs = emoji::segments(&m.content);
                let size = if emoji::is_jumbo(&segs) { 44.0 } else { 20.0 };
                for seg in segs {
                    match seg {
                        Segment::Text(t) => text_responses
                            .push(ui.add(egui::Label::new(RichText::new(t).color(pal.text)))),
                        Segment::Emoji { url, alt } => {
                            ui.add(egui::Image::from_uri(url).fit_to_exact_size(vec2(size, size)))
                                .on_hover_text(alt);
                        }
                    }
                }
            } else {
                text_responses
                    .push(ui.add(egui::Label::new(RichText::new(&m.content).color(pal.text))));
            }
        }
        if m.edited {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(RichText::new("(edited)").size(10.5).color(pal.muted))
                    .selectable(false),
            );
        }
    });
    text_responses
}

/// Discord's reply line above a message: a curved connector from the
/// avatar column, the replied-to author's small avatar and bold name, and
/// a muted one-line snippet.
fn reply_preview(
    ui: &mut egui::Ui,
    reply: &crate::types::ReplyPreview,
    config: &Config,
    pal: &Palette,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), egui::Sense::hover());
    let y = rect.center().y;
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
    let line = if pal.dark {
        Color32::from_gray(0x4e)
    } else {
        Color32::from_gray(0xc4)
    };
    ui.painter()
        .add(egui::Shape::line(pts, egui::Stroke::new(2.0, line)));

    let mut x = rect.left() + MSG_GUTTER - MSG_PAD;
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
        &reply.content,
        egui::FontId::proportional(13.0),
        pal.muted,
        rect.right() - x,
    );
    ui.painter().galley(
        egui::pos2(x, y - snippet.size().y / 2.0),
        snippet,
        pal.muted,
    );
}

/// The floating Reply / Edit / More bar at the top-right of a hovered
/// message.
#[allow(clippy::too_many_arguments)]
fn hover_bar(
    ui: &mut egui::Ui,
    m: &Message,
    mine: bool,
    row: egui::Rect,
    more_id: egui::Id,
    bar_id: egui::Id,
    action: &mut Option<MessageAction>,
    pal: &Palette,
) {
    let buttons = if mine { 3.0 } else { 2.0 };
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
                        if icon_button(ui, "↪", "Reply", false, true, pal).clicked() {
                            *action =
                                Some(MessageAction::Reply(m.id.clone(), m.author.name.clone()));
                        }
                        if mine && icon_button(ui, "✏", "Edit", false, true, pal).clicked() {
                            *action = Some(MessageAction::Edit(m.id.clone(), m.content.clone()));
                        }
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
                        let more = ui.interact(r, more_id, egui::Sense::click());
                        let open = egui::Popup::is_id_open(ui.ctx(), more_id.with("popup"));
                        if more.hovered() || open {
                            ui.painter().rect_filled(r.shrink(2.0), 4, pal.hover);
                        }
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            "⋯",
                            egui::FontId::proportional(18.0),
                            if more.hovered() || open {
                                pal.strong
                            } else {
                                pal.muted
                            },
                        );
                        let more = more.on_hover_text("More");
                        egui::Popup::menu(&more)
                            .align(egui::RectAlign::BOTTOM_END)
                            .show(|ui| message_menu(ui, m, mine, action));
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
    ui.painter().hline(
        line.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::bold_font(11.5), pal.muted);
    let text = egui::Rect::from_center_size(rect.center(), galley.size() + vec2(8.0, 0.0));
    ui.painter().rect_filled(text, 0, pal.chat);
    ui.painter()
        .galley(text.min + vec2(4.0, 0.0), galley, pal.muted);
    ui.add_space(4.0);
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
