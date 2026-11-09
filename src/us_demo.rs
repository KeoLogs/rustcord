// THROWAWAY visual test harness for user_settings. Not committed.
use eframe::egui;
use serde_json::json;

use crate::notify::GuildSettings;
use crate::theme::{Palette, ThemeChoice};
use crate::types::{Guild, User};
use crate::user_settings::*;

pub struct Demo {
    pal: Palette,
    pages: Pages,
    opts: AppOptions,
    page: String,
    me: User,
    guilds: Vec<Guild>,
    dialog: Option<NotificationDialog>,
    started: bool,
}

impl Demo {
    pub fn new(ctx: &egui::Context) -> Self {
        let pal = ThemeChoice::DiscordDark.palette();
        pal.apply(ctx);
        let guild = Guild::from_json(&json!({
            "id": "100", "name": "Pegasi Peripherals", "default_message_notifications": 1,
            "channels": [
                { "id": "1", "type": 4, "name": "Community", "position": 0 },
                { "id": "2", "type": 0, "name": "general", "parent_id": "1", "position": 0 },
                { "id": "3", "type": 0, "name": "announcements", "parent_id": "1", "position": 1 },
                { "id": "4", "type": 0, "name": "memes", "parent_id": "1", "position": 2 }
            ]
        }));
        Self {
            pal,
            pages: Pages::default(),
            opts: AppOptions::default(),
            page: std::env::var("RC_US_PAGE").unwrap_or_else(|_| "profiles".into()),
            me: User::from_json(&json!({ "id": "80351110224678912", "username": "leo", "global_name": "Leo" })),
            guilds: vec![guild],
            dialog: None,
            started: false,
        }
    }

    fn fake(&mut self, ctx: &egui::Context, req: Request) {
        let out = match req {
            Request::LoadProfile => Outcome::Profile(Ok(profiles::MyProfile {
                id: "80351110224678912".into(),
                username: "leo".into(),
                global_name: "Leo".into(),
                avatar: None,
                banner: None,
                accent_color: Some(0x8e44ad),
                bio: "Discord admin at **Pegasi Peripherals**. Building RustCord.".into(),
                pronouns: "he/him".into(),
                premium: false,
            })),
            Request::LoadSessions => Outcome::Sessions(Ok(sessions::AuthSession::parse_list(&json!({
                "user_sessions": [
                    { "id_hash": "me", "approx_last_used_time": "2026-11-09T12:00:00Z", "client_info": { "os": "Linux", "platform": "RustCord", "location": "London, United Kingdom" } },
                    { "id_hash": "b", "approx_last_used_time": "2026-11-09T08:00:00Z", "client_info": { "os": "Android", "platform": "Discord Android", "location": "London, United Kingdom" } },
                    { "id_hash": "c", "approx_last_used_time": "2026-11-02T08:00:00Z", "client_info": { "os": "Windows", "platform": "Chrome", "location": "Pristina, Kosovo" } }
                ]})))),
            Request::LoadConnections => Outcome::Connections(Ok(connections::Connection::parse_list(&json!([
                { "type": "github", "id": "1", "name": "KeoLogs", "verified": true, "visibility": 1 },
                { "type": "spotify", "id": "leo123", "name": "Leo", "verified": true, "visibility": 0 },
                { "type": "twitch", "id": "2", "name": "pegasi_live", "verified": true, "visibility": 1, "revoked": true }
            ])))),
            Request::CheckUpdates => Outcome::UpdateCheck(Ok(options::UpdateInfo {
                latest: "v0.2.0".into(),
                newer: true,
                url: options::RELEASES_PAGE.into(),
            })),
            Request::LoadMember { guild_id, .. } => Outcome::Member {
                guild_id,
                result: Ok(profiles::MemberProfile { nick: "Leo | Admin".into(), avatar: None }),
            },
            _ => return,
        };
        self.pages.apply(ctx, out);
    }
}

impl eframe::App for Demo {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {}

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            if self.page == "notif" {
                let s = GuildSettings::from_json(&json!({
                    "guild_id": "100", "muted": false, "message_notifications": 3,
                    "suppress_everyone": true,
                    "channel_overrides": [ { "channel_id": "4", "muted": true, "message_notifications": 2, "mute_config": null } ]
                }));
                self.dialog = Some(NotificationDialog::open("100", Some(&s)));
            } else if self.page == "chnotif" {
                self.dialog = Some(NotificationDialog::open_channel("100", "3", None));
            }
            if self.page == "server" {
                self.pages.profiles.tab = profiles::Tab::Server;
            }
        }
        let guilds = std::mem::take(&mut self.guilds);
        let pal = self.pal;
        let me = self.me.clone();
        let c = Ctx {
            pal: &pal,
            me: Some(&me),
            is_bot: false,
            guilds: &guilds,
            current_session_hash: Some("me"),
        };
        let mut reqs = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.chat).inner_margin(30))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for p in ["profiles", "server", "notif", "chnotif", "devices", "connections", "keybinds", "options", "language"] {
                        if ui.selectable_label(self.page == p, p).clicked() {
                            self.page = p.into();
                        }
                    }
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.set_max_width(680.0);
                    let a = match self.page.as_str() {
                        "profiles" | "server" => self.pages.profiles.show(ui, &c),
                        "devices" => self.pages.sessions.show(ui, &c),
                        "connections" => self.pages.connections.show(ui, &c),
                        "keybinds" => self.pages.keybinds.show(ui, &c),
                        "options" => self.pages.options.show(ui, &c, &mut self.opts),
                        "language" => self.pages.language.show(ui, &c, &mut self.opts),
                        _ => None,
                    };
                    if let Some(Action::Request(r)) = a {
                        reqs.push(r);
                    }
                    if let Some(d) = &mut self.dialog {
                        match d.show(ui, &c) {
                            Some(Action::CloseDialog) => self.dialog = None,
                            Some(a) => eprintln!("{a:?}"),
                            None => {}
                        }
                    }
                });
            });
        self.guilds = guilds;
        for r in reqs {
            eprintln!("request: {r:?}");
            self.fake(&ctx, r);
        }
    }
}
