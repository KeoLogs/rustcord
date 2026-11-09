//! The RustCord UI.
//!
//! Layout (top to bottom):
//!   ┌──────────────────────────────────────────────┐
//!   │ RustCord   [Server A] [Server B] …    status │  ← server bar
//!   │ # general  # memes  # dev …                  │  ← channel tabs
//!   ├──────────────────────────────────────────────┤
//!   │ messages                                     │
//!   ├──────────────────────────────────────────────┤
//!   │ [ message box                        ] Send  │
//!   └──────────────────────────────────────────────┘

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

use eframe::egui::{self, RichText};
use tokio::sync::mpsc::UnboundedSender;

use crate::types::{Guild, Message, ToBackend, ToUi};

pub struct RustCordApp {
    rx: Receiver<ToUi>,
    tx: UnboundedSender<ToBackend>,

    status: String,
    username: Option<String>,
    guilds: Vec<Guild>,
    selected_guild: Option<String>,
    selected_channel: Option<String>,

    /// channel_id -> messages (oldest first)
    messages: HashMap<String, Vec<Message>>,
    /// Channels we've already asked the backend to load.
    requested: HashSet<String>,

    draft: String,
}

impl RustCordApp {
    pub fn new(rx: Receiver<ToUi>, tx: UnboundedSender<ToBackend>) -> Self {
        Self {
            rx,
            tx,
            status: "Starting…".into(),
            username: None,
            guilds: Vec::new(),
            selected_guild: None,
            selected_channel: None,
            messages: HashMap::new(),
            requested: HashSet::new(),
            draft: String::new(),
        }
    }

    fn handle_event(&mut self, ev: ToUi) {
        match ev {
            ToUi::Status(s) => self.status = s,
            ToUi::Ready { username, guilds } => {
                self.status = format!("Connected as {username}");
                self.username = Some(username);
                self.guilds = guilds;
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
            ToUi::History {
                channel_id,
                messages,
            } => {
                self.messages.insert(channel_id, messages);
            }
            ToUi::NewMessage(msg) => {
                // Only keep live messages for channels we've opened; the rest
                // get loaded fresh from history when opened.
                if let Some(list) = self.messages.get_mut(&msg.channel_id)
                    && !list.iter().any(|m| m.id == msg.id)
                {
                    list.push(msg);
                }
            }
        }
    }

    fn current_guild(&self) -> Option<&Guild> {
        let id = self.selected_guild.as_ref()?;
        self.guilds.iter().find(|g| &g.id == id)
    }

    fn select_channel(&mut self, channel_id: String) {
        if self.requested.insert(channel_id.clone()) {
            let _ = self.tx.send(ToBackend::LoadHistory {
                channel_id: channel_id.clone(),
            });
        }
        self.selected_channel = Some(channel_id);
    }

    fn send_draft(&mut self) {
        let content = self.draft.trim().to_string();
        let Some(channel_id) = self.selected_channel.clone() else {
            return;
        };
        if content.is_empty() {
            return;
        }
        let _ = self.tx.send(ToBackend::Send {
            channel_id,
            content,
        });
        self.draft.clear();
    }

    fn server_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("RustCord").strong().size(16.0));
            ui.separator();

            let mut clicked_guild = None;
            egui::ScrollArea::horizontal()
                .id_salt("guild_scroll")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for g in &self.guilds {
                            let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
                            if ui.selectable_label(selected, &g.name).clicked() {
                                clicked_guild = Some(g.id.clone());
                            }
                        }
                    });
                });
            if let Some(id) = clicked_guild
                && self.selected_guild.as_ref() != Some(&id)
            {
                self.selected_guild = Some(id);
                self.selected_channel = None;
            }
        });
    }

    fn channel_tabs(&mut self, ui: &mut egui::Ui) {
        let mut clicked_channel = None;
        let mut first_channel = None;

        egui::ScrollArea::horizontal()
            .id_salt("channel_scroll")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let Some(guild) = self.current_guild() else {
                        ui.weak("No server selected");
                        return;
                    };
                    for c in guild.text_channels() {
                        first_channel.get_or_insert_with(|| c.id.clone());
                        let selected = self.selected_channel.as_deref() == Some(c.id.as_str());
                        if ui
                            .selectable_label(selected, format!("# {}", c.name))
                            .clicked()
                        {
                            clicked_channel = Some(c.id.clone());
                        }
                    }
                });
            });

        // Auto-open the first channel when switching servers.
        if let Some(id) = clicked_channel.or(if self.selected_channel.is_none() {
            first_channel
        } else {
            None
        }) {
            self.select_channel(id);
        }
    }

    fn message_list(&self, ui: &mut egui::Ui) {
        let Some(channel_id) = &self.selected_channel else {
            ui.centered_and_justified(|ui| ui.weak("Pick a channel"));
            return;
        };
        let Some(messages) = self.messages.get(channel_id) else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };

        egui::ScrollArea::vertical()
            .id_salt(("messages", channel_id))
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for m in messages {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(m.short_time());
                        ui.label(RichText::new(&m.author).strong());
                        ui.label(&m.content);
                    });
                    ui.add_space(2.0);
                }
            });
    }

    fn composer(&mut self, ui: &mut egui::Ui) {
        let enabled = self.selected_channel.is_some();
        ui.horizontal(|ui| {
            let send_width = 60.0;
            let edit = egui::TextEdit::singleline(&mut self.draft)
                .hint_text("Message")
                .desired_width(ui.available_width() - send_width);
            let resp = ui.add_enabled(enabled, edit);

            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui.add_enabled(enabled, egui::Button::new("Send")).clicked();
            if enter || clicked {
                self.send_draft();
                resp.request_focus();
            }
        });
    }
}

impl eframe::App for RustCordApp {
    /// Drain backend events before each frame.
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.rx.try_recv() {
            self.handle_event(ev);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("server_bar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(&self.status);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        self.server_bar(ui);
                    });
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::top("channel_tabs").show(ui, |ui| {
            ui.add_space(2.0);
            self.channel_tabs(ui);
            ui.add_space(2.0);
        });

        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.add_space(6.0);
            self.composer(ui);
            ui.add_space(6.0);
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.message_list(ui);
        });
    }
}
