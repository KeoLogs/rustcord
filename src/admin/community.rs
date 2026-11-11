//! Server settings → Community (Community servers only): the community
//! channels and language, the welcome screen, and onboarding (default
//! channels and the questions new members answer). Each section has its
//! own Save button; nothing is sent before it is pressed.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::{Cx, Kind, Method, Out, Request, b, n, s, ui as aui};
use crate::icons;
use crate::theme::Bold;

pub const FEATURE: &str = "COMMUNITY";

pub const LOCALES: &[(&str, &str)] = &[
    ("en-US", "English (US)"),
    ("en-GB", "English (UK)"),
    ("de", "Deutsch"),
    ("es-ES", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("pt-BR", "Português (Brasil)"),
    ("ru", "Русский"),
    ("tr", "Türkçe"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh-CN", "中文"),
];

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---- community settings ---------------------------------------------------

#[derive(Clone, PartialEq, Default)]
pub struct Settings {
    pub rules_channel: String,
    pub updates_channel: String,
    pub locale: String,
}

impl Settings {
    pub fn from_json(v: &Value) -> Self {
        Self {
            rules_channel: s(v, "rules_channel_id").to_string(),
            updates_channel: s(v, "public_updates_channel_id").to_string(),
            locale: match s(v, "preferred_locale") {
                "" => "en-US".to_string(),
                l => l.to_string(),
            },
        }
    }
}

pub fn load_settings(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadCommunity,
        Method::Get,
        format!("/guilds/{guild_id}"),
    )
}

/// A Community server must keep both channels: empty ones are not sent.
pub fn save_settings(guild_id: &str, base: &Settings, d: &Settings) -> Option<Request> {
    let mut m = serde_json::Map::new();
    if d.rules_channel != base.rules_channel && !d.rules_channel.is_empty() {
        m.insert("rules_channel_id".into(), json!(d.rules_channel));
    }
    if d.updates_channel != base.updates_channel && !d.updates_channel.is_empty() {
        m.insert("public_updates_channel_id".into(), json!(d.updates_channel));
    }
    if d.locale != base.locale {
        m.insert("preferred_locale".into(), json!(d.locale));
    }
    (!m.is_empty()).then(|| {
        Request::new(
            guild_id,
            Kind::SaveCommunity,
            Method::Patch,
            format!("/guilds/{guild_id}"),
        )
        .body(Value::Object(m))
    })
}

// ---- welcome screen -------------------------------------------------------

#[derive(Clone, PartialEq, Default)]
pub struct WelcomeChannel {
    pub channel: String,
    pub description: String,
    pub emoji: String,
}

#[derive(Clone, PartialEq, Default)]
pub struct Welcome {
    pub enabled: bool,
    pub description: String,
    pub channels: Vec<WelcomeChannel>,
}

impl Welcome {
    pub fn from_json(v: &Value) -> Self {
        Self {
            enabled: b(v, "enabled"),
            description: s(v, "description").to_string(),
            channels: v["welcome_channels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .take(5)
                        .map(|c| WelcomeChannel {
                            channel: s(c, "channel_id").to_string(),
                            description: s(c, "description").to_string(),
                            emoji: s(c, "emoji_name").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn valid(&self) -> bool {
        self.channels.len() <= 5
            && self
                .channels
                .iter()
                .all(|c| !c.channel.is_empty() && !c.description.trim().is_empty())
    }

    pub fn body(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "description": self.description.trim(),
            "welcome_channels": self.channels.iter().map(|c| json!({
                "channel_id": c.channel,
                "description": c.description.trim(),
                "emoji_id": Value::Null,
                "emoji_name": if c.emoji.trim().is_empty() { Value::Null } else { json!(c.emoji.trim()) },
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn load_welcome(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadWelcome,
        Method::Get,
        format!("/guilds/{guild_id}/welcome-screen"),
    )
}

pub fn save_welcome(guild_id: &str, base: &Welcome, d: &Welcome) -> Option<Request> {
    (d != base && d.valid()).then(|| {
        Request::new(
            guild_id,
            Kind::SaveWelcome,
            Method::Patch,
            format!("/guilds/{guild_id}/welcome-screen"),
        )
        .body(d.body())
    })
}

// ---- onboarding -----------------------------------------------------------

#[derive(Clone, PartialEq, Default)]
pub struct OnbOption {
    pub id: String,
    pub title: String,
    pub description: String,
    pub roles: Vec<String>,
    pub channels: Vec<String>,
    /// Kept as sent: id, name, animated.
    pub emoji: (String, String, bool),
}

#[derive(Clone, PartialEq, Default)]
pub struct Prompt {
    pub id: String,
    pub title: String,
    pub kind: u64,
    pub single_select: bool,
    pub required: bool,
    pub in_onboarding: bool,
    pub options: Vec<OnbOption>,
}

#[derive(Clone, PartialEq, Default)]
pub struct Onboarding {
    pub enabled: bool,
    pub mode: u64,
    pub default_channels: Vec<String>,
    pub prompts: Vec<Prompt>,
}

impl Onboarding {
    pub fn from_json(v: &Value) -> Self {
        let option = |o: &Value| OnbOption {
            id: s(o, "id").to_string(),
            title: s(o, "title").to_string(),
            description: s(o, "description").to_string(),
            roles: ids(&o["role_ids"]),
            channels: ids(&o["channel_ids"]),
            emoji: (
                s(&o["emoji"], "id").to_string(),
                s(&o["emoji"], "name").to_string(),
                b(&o["emoji"], "animated"),
            ),
        };
        Self {
            enabled: b(v, "enabled"),
            mode: n(v, "mode"),
            default_channels: ids(&v["default_channel_ids"]),
            prompts: v["prompts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| Prompt {
                            id: s(p, "id").to_string(),
                            title: s(p, "title").to_string(),
                            kind: n(p, "type"),
                            single_select: b(p, "single_select"),
                            required: b(p, "required"),
                            in_onboarding: b(p, "in_onboarding"),
                            options: p["options"]
                                .as_array()
                                .map(|o| o.iter().map(option).collect())
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Every question needs a title and at least one answer, and every
    /// answer a title and something to give (a role or a channel).
    pub fn valid(&self) -> bool {
        self.prompts.iter().all(|p| {
            !p.title.trim().is_empty()
                && !p.options.is_empty()
                && p.options.iter().all(|o| {
                    !o.title.trim().is_empty() && (!o.roles.is_empty() || !o.channels.is_empty())
                })
        })
    }

    pub fn body(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "mode": self.mode,
            "default_channel_ids": self.default_channels,
            "prompts": self.prompts.iter().map(|p| json!({
                "id": p.id,
                "title": p.title.trim(),
                "type": p.kind,
                "single_select": p.single_select,
                "required": p.required,
                "in_onboarding": p.in_onboarding,
                "options": p.options.iter().map(|o| json!({
                    "id": o.id,
                    "title": o.title.trim(),
                    "description": o.description.trim(),
                    "role_ids": o.roles,
                    "channel_ids": o.channels,
                    "emoji_id": super::id_or_null(&o.emoji.0),
                    "emoji_name": super::id_or_null(&o.emoji.1),
                    "emoji_animated": o.emoji.2,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// A placeholder snowflake for a new question / answer (Discord assigns
/// the real id on save).
pub fn placeholder_id(now_ms: i64, seq: u64) -> String {
    let ms = (now_ms - 1_420_070_400_000).max(0) as u64;
    ((ms << 22) | (seq & 0x3f_ffff)).to_string()
}

pub fn load_onboarding(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadOnboarding,
        Method::Get,
        format!("/guilds/{guild_id}/onboarding"),
    )
}

pub fn save_onboarding(guild_id: &str, base: &Onboarding, d: &Onboarding) -> Option<Request> {
    (d != base && d.valid()).then(|| {
        Request::new(
            guild_id,
            Kind::SaveOnboarding,
            Method::Put,
            format!("/guilds/{guild_id}/onboarding"),
        )
        .body(d.body())
    })
}

// ---- page -----------------------------------------------------------------

#[derive(Default)]
pub struct State {
    settings: Option<(Settings, Settings)>,
    welcome: Option<(Welcome, Welcome)>,
    onboarding: Option<(Onboarding, Onboarding)>,
    seq: u64,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, _key: &str, v: &Value) {
        match kind {
            Kind::LoadCommunity => {
                let d = Settings::from_json(v);
                self.settings = Some((d.clone(), d));
            }
            Kind::SaveCommunity => {
                // The reply is the server (the demo echoes the change).
                if let Some((base, draft)) = &mut self.settings {
                    *base = draft.clone();
                }
            }
            Kind::LoadWelcome | Kind::SaveWelcome => {
                let d = Welcome::from_json(v);
                self.welcome = Some((d.clone(), d));
            }
            Kind::LoadOnboarding | Kind::SaveOnboarding => {
                let d = Onboarding::from_json(v);
                self.onboarding = Some((d.clone(), d));
            }
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let text_channels = cx.channel_options(&[0, 5], None);

        aui::heading(ui, pal, "Community settings");
        match &mut self.settings {
            None => aui::loading(ui, pal),
            Some((base, d)) => {
                aui::label(ui, pal, "Rules or guidelines channel");
                aui::select(ui, pal, "co_rules", &mut d.rules_channel, &text_channels);
                aui::label(ui, pal, "Community updates channel");
                aui::select(
                    ui,
                    pal,
                    "co_updates",
                    &mut d.updates_channel,
                    &text_channels,
                );
                aui::hint(
                    ui,
                    pal,
                    "Discord posts announcements for community admins and moderators here.",
                );
                aui::label(ui, pal, "Server primary language");
                let locales: Vec<(String, String)> = LOCALES
                    .iter()
                    .map(|(k, l)| (k.to_string(), l.to_string()))
                    .collect();
                aui::select(ui, pal, "co_locale", &mut d.locale, &locales);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let req = save_settings(&cx.guild.id, base, d);
                    if aui::save(ui, pal, "Save Community Settings", req.is_some()).clicked() {
                        out.send.extend(req);
                    }
                    if d != base && aui::plain(ui, "Reset").clicked() {
                        *d = base.clone();
                    }
                });
            }
        }

        aui::divider(ui, pal);
        aui::heading(ui, pal, "Welcome screen");
        match &mut self.welcome {
            None => aui::loading(ui, pal),
            Some((base, d)) => {
                crate::settings::switch_row(
                    ui,
                    &mut d.enabled,
                    "Show the welcome screen",
                    "New members see a short description and up to five recommended channels.",
                    pal,
                );
                aui::label(ui, pal, "Server description");
                aui::text_area(ui, &mut d.description, "What is this server about?", 140, 2);
                aui::label(ui, pal, "Recommended channels");
                let mut remove = None;
                for (i, c) in d.channels.iter_mut().enumerate() {
                    aui::card(ui, pal, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.scope(|ui| {
                                ui.set_max_width(180.0);
                                aui::select(ui, pal, ("co_wc", i), &mut c.channel, &text_channels);
                            });
                            ui.add(
                                egui::TextEdit::singleline(&mut c.emoji)
                                    .hint_text("Emoji")
                                    .desired_width(48.0)
                                    .char_limit(16),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut c.description)
                                    .hint_text("What people do there")
                                    .desired_width(220.0)
                                    .char_limit(50),
                            );
                            if crate::widgets::icon_button(
                                ui,
                                icons::TRASH,
                                "Remove",
                                false,
                                true,
                                pal,
                            )
                            .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    d.channels.remove(i);
                }
                if d.channels.len() < 5
                    && aui::icon_text(ui, icons::PLUS, "Add a channel").clicked()
                {
                    d.channels.push(WelcomeChannel {
                        channel: text_channels
                            .first()
                            .map(|c| c.0.clone())
                            .unwrap_or_default(),
                        ..Default::default()
                    });
                }
                if !d.valid() {
                    aui::hint(ui, pal, "Every recommended channel needs a description.");
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let req = save_welcome(&cx.guild.id, base, d);
                    if aui::save(ui, pal, "Save Welcome Screen", req.is_some()).clicked() {
                        out.send.extend(req);
                    }
                    if d != base && aui::plain(ui, "Reset").clicked() {
                        *d = base.clone();
                    }
                });
            }
        }

        aui::divider(ui, pal);
        aui::heading(ui, pal, "Onboarding");
        let Some((base, d)) = &mut self.onboarding else {
            aui::loading(ui, pal);
            return;
        };
        crate::settings::switch_row(
            ui,
            &mut d.enabled,
            "Onboarding enabled",
            "New members pick their interests first. Discord requires at least 7 default channels, 5 of them open to @everyone.",
            pal,
        );
        aui::label(ui, pal, "Mode");
        aui::select(
            ui,
            pal,
            "co_onb_mode",
            &mut d.mode,
            &aui::opts(&[
                (0, "Default: only default channels count"),
                (1, "Advanced: questions count too"),
            ]),
        );
        aui::label(ui, pal, "Default channels (everyone gets these)");
        let all_channels = cx.channel_options(&[0, 2, 5, 13, 15], None);
        ui.horizontal_wrapped(|ui| {
            check_list(ui, &all_channels, &mut d.default_channels);
        });

        aui::label(ui, pal, "Questions");
        let roles: Vec<(String, String)> = cx
            .guild
            .roles
            .iter()
            .filter(|r| r.id != cx.guild.id)
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut remove_prompt = None;
        for (pi, p) in d.prompts.iter_mut().enumerate() {
            let title = if p.title.trim().is_empty() {
                "New question".to_string()
            } else {
                p.title.clone()
            };
            egui::CollapsingHeader::new(RichText::new(title).color(pal.strong).bold())
                .id_salt(("co_prompt", &p.id))
                .show(ui, |ui| {
                    aui::text(
                        ui,
                        &mut p.title,
                        "Question, e.g. What brings you here?",
                        100,
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut p.single_select, "One answer only");
                        ui.checkbox(&mut p.required, "Required");
                        ui.checkbox(&mut p.in_onboarding, "Ask when joining");
                    });
                    let mut remove_option = None;
                    for (oi, o) in p.options.iter_mut().enumerate() {
                        aui::card(ui, pal, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.title)
                                        .hint_text("Answer")
                                        .desired_width(180.0)
                                        .char_limit(50),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.description)
                                        .hint_text("Description (optional)")
                                        .desired_width(220.0)
                                        .char_limit(100),
                                );
                                if crate::widgets::icon_button(
                                    ui,
                                    icons::TRASH,
                                    "Remove answer",
                                    false,
                                    true,
                                    pal,
                                )
                                .clicked()
                                {
                                    remove_option = Some(oi);
                                }
                            });
                            ui.label(pal.section_label("Gives roles"));
                            ui.horizontal_wrapped(|ui| check_list(ui, &roles, &mut o.roles));
                            ui.label(pal.section_label("Shows channels"));
                            ui.horizontal_wrapped(|ui| {
                                check_list(ui, &all_channels, &mut o.channels)
                            });
                        });
                    }
                    if let Some(oi) = remove_option {
                        p.options.remove(oi);
                    }
                    ui.horizontal(|ui| {
                        if aui::icon_text(ui, icons::PLUS, "Add an answer").clicked() {
                            self.seq += 1;
                            p.options.push(OnbOption {
                                id: placeholder_id(now_ms, self.seq),
                                ..Default::default()
                            });
                        }
                        if aui::plain(ui, "Remove question").clicked() {
                            remove_prompt = Some(pi);
                        }
                    });
                });
        }
        if let Some(pi) = remove_prompt {
            d.prompts.remove(pi);
        }
        if aui::icon_text(ui, icons::PLUS, "Add a question").clicked() {
            self.seq += 1;
            d.prompts.push(Prompt {
                id: placeholder_id(now_ms, self.seq),
                in_onboarding: true,
                ..Default::default()
            });
        }
        if !d.valid() {
            aui::hint(
                ui,
                pal,
                "Every question needs a title and an answer; every answer needs a title and at least one role or channel.",
            );
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let req = save_onboarding(&cx.guild.id, base, d);
            if aui::save(ui, pal, "Save Onboarding", req.is_some()).clicked() {
                out.send.extend(req);
            }
            if d != base && aui::plain(ui, "Reset").clicked() {
                *d = base.clone();
            }
        });
    }
}

/// Check boxes for picking ids out of `(id, label)` options.
fn check_list(ui: &mut egui::Ui, options: &[(String, String)], picked: &mut Vec<String>) {
    for (id, label) in options {
        let mut on = picked.contains(id);
        if ui.checkbox(&mut on, label.as_str()).changed() {
            picked.retain(|x| x != id);
            if on {
                picked.push(id.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_patch_changes_and_never_clears_required_channels() {
        let base = Settings::from_json(&json!({
            "rules_channel_id": "1", "public_updates_channel_id": "2",
            "preferred_locale": "en-US",
        }));
        assert!(save_settings("9", &base, &base.clone()).is_none());
        let mut d = base.clone();
        d.rules_channel.clear();
        assert!(save_settings("9", &base, &d).is_none());
        d.rules_channel = "3".into();
        d.locale = "de".into();
        let r = save_settings("9", &base, &d).unwrap();
        assert_eq!((r.method, r.path.as_str()), (Method::Patch, "/guilds/9"));
        assert_eq!(
            r.body,
            Some(json!({ "rules_channel_id": "3", "preferred_locale": "de" }))
        );
    }

    #[test]
    fn welcome_screen_round_trip() {
        let base = Welcome::from_json(&json!({
            "enabled": true, "description": "Hi",
            "welcome_channels": [{ "channel_id": "1", "description": "Rules",
                                   "emoji_id": null, "emoji_name": "📜" }],
        }));
        assert_eq!(base.channels.len(), 1);
        assert!(save_welcome("9", &base, &base.clone()).is_none());
        let mut d = base.clone();
        d.channels.push(WelcomeChannel::default());
        assert!(
            save_welcome("9", &base, &d).is_none(),
            "needs a description"
        );
        d.channels[1] = WelcomeChannel {
            channel: "2".into(),
            description: " Chat ".into(),
            emoji: String::new(),
        };
        let r = save_welcome("9", &base, &d).unwrap();
        assert_eq!(r.path, "/guilds/9/welcome-screen");
        let body = r.body.unwrap();
        assert_eq!(body["welcome_channels"][1]["description"], "Chat");
        assert_eq!(body["welcome_channels"][1]["emoji_name"], Value::Null);
        assert_eq!(body["welcome_channels"][0]["emoji_name"], "📜");
    }

    #[test]
    fn onboarding_round_trip_and_validation() {
        let base = Onboarding::from_json(&json!({
            "enabled": true, "mode": 0, "default_channel_ids": ["1"],
            "prompts": [{ "id": "5", "title": "Why?", "type": 0, "single_select": true,
                "required": false, "in_onboarding": true,
                "options": [{ "id": "6", "title": "Rust", "description": "",
                    "role_ids": ["r"], "channel_ids": [],
                    "emoji": { "id": null, "name": "🦀", "animated": false } }] }],
        }));
        assert!(base.valid());
        assert!(save_onboarding("9", &base, &base.clone()).is_none());
        let mut d = base.clone();
        d.default_channels.push("2".into());
        let r = save_onboarding("9", &base, &d).unwrap();
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Put, "/guilds/9/onboarding")
        );
        let body = r.body.unwrap();
        assert_eq!(body["default_channel_ids"], json!(["1", "2"]));
        let opt = &body["prompts"][0]["options"][0];
        assert_eq!(opt["emoji_name"], "🦀");
        assert_eq!(opt["emoji_id"], Value::Null);
        assert_eq!(opt["role_ids"], json!(["r"]));
        // An answer that gives nothing is refused.
        d.prompts[0].options[0].roles.clear();
        assert!(save_onboarding("9", &base, &d).is_none());
        // So is a question without answers.
        d.prompts[0].options.clear();
        assert!(!d.valid());
    }

    #[test]
    fn placeholder_ids_are_snowflakes_that_differ() {
        let a = placeholder_id(1_762_000_000_000, 1);
        let b = placeholder_id(1_762_000_000_000, 2);
        assert_ne!(a, b);
        assert_eq!(super::super::audit::snowflake_time(&a), 1_762_000_000);
    }
}
