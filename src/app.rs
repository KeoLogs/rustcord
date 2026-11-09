//! The RustCord UI.
//!
//! Default layout (Discord-style), every part movable and resizable:
//!   ┌────────┬───────────┬──────────────────────────────┐
//!   │Servers │ Channels  │ Messages                     │
//!   │        │           │                              │
//!   │        │           ├──────────────────────────────┤
//!   │        │           │ Message box                  │
//!   └────────┴───────────┴──────────────────────────────┘

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, vec2};
use tokio::sync::mpsc::UnboundedSender;

use crate::layout::{self, LayoutBehavior, Pane};
use crate::types::{Attachment, DM_GUILD_ID, Guild, Message, ToBackend, ToUi, User};

const LAYOUT_KEY: &str = "layout";
const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
const TYPING_RESEND: Duration = Duration::from_secs(8);
const IMAGE_MAX: (f32, f32) = (400.0, 300.0);

// ---------------------------------------------------------------------------
// App shell: layout + top bar
// ---------------------------------------------------------------------------

pub struct RustCordApp {
    state: State,
    tree: egui_tiles::Tree<Pane>,
    editing_layout: bool,
}

impl RustCordApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rx: Receiver<ToUi>,
        tx: UnboundedSender<ToBackend>,
    ) -> Self {
        let tree = cc
            .storage
            .and_then(|s| eframe::get_value(s, LAYOUT_KEY))
            .unwrap_or_else(layout::default_tree);
        Self {
            state: State::new(rx, tx),
            tree,
            editing_layout: false,
        }
    }
}

impl eframe::App for RustCordApp {
    /// Drain backend events before each frame.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.drain_events();
        if self.state.expire_typing() {
            // Keep ticking so "is typing…" disappears on time.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("RustCord").strong());
                ui.separator();
                ui.toggle_value(&mut self.editing_layout, "✏ Edit layout")
                    .on_hover_text(
                        "Drag a panel's title bar to move it.\n\
                         Drag the lines between panels to resize.",
                    );
                if self.editing_layout && ui.button("↺ Reset layout").clicked() {
                    self.tree = layout::default_tree();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(&self.state.status);
                });
            });
        });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut behavior = LayoutBehavior {
                state: &mut self.state,
                editing: self.editing_layout,
            };
            self.tree.ui(&mut behavior, ui);
        });

        self.state.modals(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, LAYOUT_KEY, &self.tree);
    }
}

// ---------------------------------------------------------------------------
// Chat state
// ---------------------------------------------------------------------------

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
    me: Option<User>,
    guilds: Vec<Guild>,
    dms: Guild,
    selected_guild: Option<String>,
    selected_channel: Option<String>,

    /// channel_id -> messages (oldest first)
    messages: HashMap<String, Vec<Message>>,
    /// channel_id -> people typing there
    typing: HashMap<String, Vec<Typer>>,

    draft: String,
    compose: Compose,
    last_typing_sent: Option<(String, Instant)>,
    confirm_delete: Option<(String, String)>,
}

impl State {
    fn new(rx: Receiver<ToUi>, tx: UnboundedSender<ToBackend>) -> Self {
        Self {
            rx,
            tx,
            status: "Starting…".into(),
            me: None,
            guilds: Vec::new(),
            dms: Guild::dms(Vec::new()),
            selected_guild: None,
            selected_channel: None,
            messages: HashMap::new(),
            typing: HashMap::new(),
            draft: String::new(),
            compose: Compose::New,
            last_typing_sent: None,
            confirm_delete: None,
        }
    }

    // ----- events --------------------------------------------------------

    fn drain_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.handle_event(ev);
        }
    }

    fn handle_event(&mut self, ev: ToUi) {
        match ev {
            ToUi::Status(s) => self.status = s,
            ToUi::Ready { me, guilds, dms } => {
                self.status = format!("Connected as {}", me.name);
                self.me = Some(me);
                self.guilds = guilds;
                self.dms = Guild::dms(dms);
                if self.selected_guild.is_none() {
                    self.selected_guild = self.guilds.first().map(|g| g.id.clone());
                }
            }
            ToUi::GuildAdded(guild) => {
                match self.guilds.iter_mut().find(|g| g.id == guild.id) {
                    Some(existing) => *existing = guild,
                    None => self.guilds.push(guild),
                }
                if self.selected_guild.is_none() {
                    self.selected_guild = self.guilds.first().map(|g| g.id.clone());
                }
            }
            ToUi::ChannelUpserted { guild_id, channel } => match guild_id {
                Some(gid) => {
                    if let Some(g) = self.guilds.iter_mut().find(|g| g.id == gid) {
                        g.upsert_channel(channel);
                    }
                }
                None if channel.is_dm() => self.dms.upsert_channel(channel),
                None => {}
            },
            ToUi::History {
                channel_id,
                messages,
            } => {
                self.messages.insert(channel_id, messages);
            }
            ToUi::NewMessage(msg) => {
                // They sent it, so they've stopped typing.
                if let Some(typers) = self.typing.get_mut(&msg.channel_id) {
                    typers.retain(|t| t.user_id != msg.author.id);
                }
                // Bump DMs to the top of the list.
                if let Some(c) = self
                    .dms
                    .channels
                    .iter_mut()
                    .find(|c| c.id == msg.channel_id)
                {
                    c.last_message_id = msg.id.parse().unwrap_or(c.last_message_id);
                    self.dms.sort_channels();
                }
                // Only keep live messages for channels we've opened; the rest
                // load fresh from history when opened.
                if let Some(list) = self.messages.get_mut(&msg.channel_id)
                    && !list.iter().any(|m| m.id == msg.id)
                {
                    list.push(msg);
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
                if self.me.as_ref().is_some_and(|me| me.id == user_id) {
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

    /// Best-effort display name for a user in a channel (used for typing in
    /// DMs, where Discord doesn't send the name).
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

    /// Drop typing entries older than 10s. Returns true if anyone is still typing.
    fn expire_typing(&mut self) -> bool {
        for typers in self.typing.values_mut() {
            typers.retain(|t| t.since.elapsed() < TYPING_TIMEOUT);
        }
        self.typing.retain(|_, t| !t.is_empty());
        !self.typing.is_empty()
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

    fn select_channel(&mut self, channel_id: String) {
        if !self.messages.contains_key(&channel_id) {
            let _ = self.tx.send(ToBackend::LoadHistory {
                channel_id: channel_id.clone(),
            });
        }
        if self.selected_channel.as_ref() != Some(&channel_id) {
            self.compose = Compose::New;
        }
        self.selected_channel = Some(channel_id);
    }

    fn channel_title(&self) -> Option<String> {
        let cid = self.selected_channel.as_deref()?;
        self.current_guild()?.channel(cid).map(|c| c.label())
    }

    // ----- panes ---------------------------------------------------------

    pub fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Servers => self.servers_pane(ui),
            Pane::Channels => self.channels_pane(ui),
            Pane::Messages => self.messages_pane(ui),
            Pane::Composer => self.composer_pane(ui),
        }
    }

    fn servers_pane(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("servers")
            .auto_shrink(false)
            .show(ui, |ui| {
                let dm_selected = self.selected_guild.as_deref() == Some(DM_GUILD_ID);
                if server_button(ui, dm_selected, None, "💬", "Direct Messages").clicked() {
                    clicked = Some(DM_GUILD_ID.to_string());
                }
                ui.separator();
                for g in &self.guilds {
                    let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
                    let initials: String = g
                        .name
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .take(2)
                        .collect();
                    if server_button(ui, selected, g.icon_url.as_deref(), &initials, &g.name)
                        .clicked()
                    {
                        clicked = Some(g.id.clone());
                    }
                }
            });
        if let Some(id) = clicked {
            self.select_guild(id);
        }
    }

    fn channels_pane(&mut self, ui: &mut egui::Ui) {
        let Some(guild) = self.current_guild() else {
            ui.weak("No server selected");
            return;
        };
        ui.label(RichText::new(&guild.name).strong().size(15.0));
        ui.separator();

        let mut clicked = None;
        let mut first = None;
        egui::ScrollArea::vertical()
            .id_salt("channels")
            .auto_shrink(false)
            .show(ui, |ui| {
                let groups = guild.channel_groups();
                if groups.is_empty() {
                    ui.weak(if guild.is_dms() {
                        "No DMs"
                    } else {
                        "No text channels"
                    });
                }
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for group in groups {
                        if let Some(cat) = group.category {
                            ui.add_space(6.0);
                            ui.label(RichText::new(cat.name.to_uppercase()).small().weak());
                        }
                        for c in group.channels {
                            first.get_or_insert_with(|| c.id.clone());
                            let selected = self.selected_channel.as_deref() == Some(c.id.as_str());
                            let resp =
                                ui.add(egui::Button::selectable(selected, c.label()).truncate());
                            if resp.clicked() {
                                clicked = Some(c.id.clone());
                            }
                        }
                    }
                });
            });

        // Auto-open the first channel when switching servers.
        let auto = if self.selected_channel.is_none() {
            first
        } else {
            None
        };
        if let Some(id) = clicked.or(auto) {
            self.select_channel(id);
        }
    }

    fn messages_pane(&mut self, ui: &mut egui::Ui) {
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.centered_and_justified(|ui| ui.weak("Pick a channel"));
            return;
        };
        if let Some(title) = self.channel_title() {
            ui.label(RichText::new(title).strong().size(15.0));
            ui.separator();
        }
        let Some(messages) = self.messages.get(&channel_id) else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        if messages.is_empty() {
            ui.centered_and_justified(|ui| ui.weak("No messages yet"));
            return;
        }

        let my_id = self.me.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
        let mut action = None;

        egui::ScrollArea::vertical()
            .id_salt(("messages", &channel_id))
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let mut prev: Option<&Message> = None;
                for m in messages {
                    // Collapse consecutive messages from the same person.
                    let grouped =
                        m.reply.is_none() && prev.is_some_and(|p| p.author.id == m.author.id);
                    if let Some(a) = message_row(ui, m, grouped, m.author.id == my_id) {
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
            None => {}
        }
    }

    fn composer_pane(&mut self, ui: &mut egui::Ui) {
        let Some(channel_id) = self.selected_channel.clone() else {
            ui.weak("Pick a channel to start chatting");
            return;
        };

        // Who's typing
        if let Some(typers) = self.typing.get(&channel_id).filter(|t| !t.is_empty()) {
            let names: Vec<&str> = typers.iter().map(|t| t.name.as_str()).collect();
            let text = match names.as_slice() {
                [one] => format!("{one} is typing…"),
                [a, b] => format!("{a} and {b} are typing…"),
                _ => "Several people are typing…".into(),
            };
            ui.label(RichText::new(text).small().italics().weak());
        }

        // Reply / edit banner
        let mut cancel = false;
        match &self.compose {
            Compose::New => {}
            Compose::Reply { author, .. } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Replying to {author}")).small());
                    cancel = ui.small_button("✕").clicked();
                });
            }
            Compose::Edit { .. } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Editing message — Esc to cancel").small());
                    cancel = ui.small_button("✕").clicked();
                });
            }
        }

        let mut send = false;
        let mut changed = false;
        ui.horizontal(|ui| {
            let send_width = 60.0;
            let edit = egui::TextEdit::singleline(&mut self.draft)
                .hint_text("Message")
                .desired_width(ui.available_width() - send_width);
            let resp = ui.add(edit);
            changed = resp.changed();

            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui.button("Send").clicked();
            if enter || clicked {
                send = true;
                resp.request_focus();
            }
        });

        if cancel {
            if matches!(self.compose, Compose::Edit { .. }) {
                self.draft.clear();
            }
            self.compose = Compose::New;
        }
        if changed && !self.draft.is_empty() {
            self.maybe_send_typing(&channel_id);
        }
        if send {
            self.submit(channel_id);
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
        if content.is_empty() {
            return;
        }
        let req = match std::mem::replace(&mut self.compose, Compose::New) {
            Compose::New => ToBackend::Send {
                channel_id,
                content,
                reply_to: None,
            },
            Compose::Reply { message_id, .. } => ToBackend::Send {
                channel_id,
                content,
                reply_to: Some(message_id),
            },
            Compose::Edit { message_id } => ToBackend::Edit {
                channel_id,
                message_id,
                content,
            },
        };
        let _ = self.tx.send(req);
        self.draft.clear();
        self.last_typing_sent = None;
    }

    fn modals(&mut self, ui: &mut egui::Ui) {
        let Some((channel_id, message_id)) = self.confirm_delete.clone() else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("confirm_delete")).show(ui.ctx(), |ui| {
            ui.heading("Delete message?");
            ui.label("This can't be undone.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    let _ = self.tx.send(ToBackend::Delete {
                        channel_id: channel_id.clone(),
                        message_id: message_id.clone(),
                    });
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_delete = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

/// A server entry: icon (or initials) plus name.
fn server_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: Option<&str>,
    fallback: &str,
    name: &str,
) -> egui::Response {
    let resp = ui
        .horizontal(|ui| {
            let size = vec2(28.0, 28.0);
            match icon {
                Some(url) => {
                    ui.add(
                        egui::Image::from_uri(url)
                            .fit_to_exact_size(size)
                            .corner_radius(14),
                    );
                }
                None => {
                    ui.add_sized(size, egui::Label::new(RichText::new(fallback).strong()));
                }
            }
            ui.add(egui::Button::selectable(selected, name).truncate())
        })
        .inner;
    resp.on_hover_text(name)
}

/// Draw one message. Returns an action if the user picked one from its menu.
fn message_row(ui: &mut egui::Ui, m: &Message, grouped: bool, mine: bool) -> Option<MessageAction> {
    let mut action = None;
    if !grouped {
        ui.add_space(6.0);
    }

    ui.horizontal_top(|ui| {
        let avatar = vec2(32.0, 32.0);
        if grouped {
            ui.add_space(avatar.x + ui.spacing().item_spacing.x);
        } else {
            ui.add(
                egui::Image::from_uri(&m.author.avatar_url)
                    .fit_to_exact_size(avatar)
                    .corner_radius(16),
            );
        }

        ui.vertical(|ui| {
            if let Some(reply) = &m.reply {
                ui.label(
                    RichText::new(format!("↪ {}: {}", reply.author, reply.content))
                        .small()
                        .weak(),
                );
            }
            if !grouped {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&m.author.name).strong());
                    ui.label(RichText::new(m.short_time()).small().weak());
                });
            }

            ui.horizontal_wrapped(|ui| {
                if !m.content.is_empty() {
                    // Right-click the text for the same menu as "⋯".
                    let text = ui.add(egui::Label::new(&m.content).sense(egui::Sense::click()));
                    egui::Popup::context_menu(&text).show(|ui| {
                        message_menu(ui, m, mine, &mut action);
                    });
                }
                if m.edited {
                    ui.label(RichText::new("(edited)").small().weak());
                }
                let dots = ui
                    .add(egui::Button::new(RichText::new("⋯").weak()).frame(false))
                    .on_hover_text("More");
                egui::Popup::menu(&dots).show(|ui| {
                    message_menu(ui, m, mine, &mut action);
                });
            });

            for a in &m.attachments {
                attachment_ui(ui, a);
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
    if mine && ui.button("🗑 Delete").clicked() {
        *action = Some(MessageAction::Delete(m.id.clone()));
    }
}

fn attachment_ui(ui: &mut egui::Ui, a: &Attachment) {
    if a.is_image() {
        let (max_w, max_h) = IMAGE_MAX;
        let url = a.thumbnail_url(max_w as u32, max_h as u32);
        let mut image = egui::Image::from_uri(url)
            .corner_radius(4)
            .sense(egui::Sense::click());
        image = match a.display_size(max_w, max_h) {
            Some((w, h)) => image.fit_to_exact_size(vec2(w, h)),
            None => image.max_size(vec2(max_w, max_h)),
        };
        let resp = ui.add(image).on_hover_text(format!(
            "{} ({}) — click to open full size",
            a.filename,
            a.human_size()
        ));
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&a.url));
        }
    } else {
        ui.horizontal(|ui| {
            ui.label("📎");
            ui.hyperlink_to(&a.filename, &a.url);
            ui.label(RichText::new(a.human_size()).small().weak());
        });
    }
}
