//! Server settings → Webhooks and Integrations: list, create, edit and
//! delete webhooks, copy a webhook URL, and list / remove integrations.
//!
//! A webhook URL is a secret (anyone holding it can post as the webhook):
//! it is drawn masked until the user clicks Reveal, copied only by the
//! Copy button, never logged (`Request` / `Response` print no bodies) and
//! dropped with the page when Server Settings closes.

use std::sync::Arc;

use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};

use super::overview::ImageEdit;
use super::{Confirm, Cx, Kind, Method, Out, PickSlot, Picked, Request, b, n, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

/// What is shown in place of a hidden URL.
pub const MASKED: &str = "https://discord.com/api/webhooks/••••••••••••";

/// 1–80 characters; Discord refuses names containing "clyde" or "discord".
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    let lower = n.to_lowercase();
    (1..=80).contains(&n.chars().count()) && !lower.contains("clyde") && !lower.contains("discord")
}

/// The secret URL of an incoming webhook (`None` for follower / app
/// webhooks, which have no token).
pub fn secret_url(hook: &Value) -> Option<String> {
    let url = s(hook, "url");
    if !url.is_empty() {
        return Some(url.to_string());
    }
    let token = s(hook, "token");
    (!token.is_empty())
        .then(|| format!("https://discord.com/api/webhooks/{}/{token}", s(hook, "id")))
}

/// What to draw for a webhook's URL.
pub fn shown_url(hook: &Value, revealed: bool) -> Option<String> {
    let url = secret_url(hook)?;
    Some(if revealed { url } else { MASKED.to_string() })
}

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadWebhooks,
        Method::Get,
        format!("/guilds/{guild_id}/webhooks"),
    )
}

pub fn create(guild_id: &str, channel_id: &str, name: &str) -> Option<Request> {
    (valid_name(name) && !channel_id.is_empty()).then(|| {
        Request::new(
            guild_id,
            Kind::CreateWebhook,
            Method::Post,
            format!("/channels/{channel_id}/webhooks"),
        )
        .body(json!({ "name": name.trim() }))
    })
}

#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub name: String,
    pub channel: String,
    pub avatar: ImageEdit,
}

impl Draft {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: s(v, "name").to_string(),
            channel: s(v, "channel_id").to_string(),
            avatar: ImageEdit::Keep,
        }
    }
}

/// The path never holds the token: webhooks are edited by id, with my
/// own authorisation.
pub fn save(guild_id: &str, id: &str, base: &Draft, d: &Draft) -> Option<Request> {
    if !valid_name(&d.name) {
        return None;
    }
    let mut m = Map::new();
    if d.name.trim() != base.name {
        m.insert("name".into(), json!(d.name.trim()));
    }
    if d.channel != base.channel && !d.channel.is_empty() {
        m.insert("channel_id".into(), json!(d.channel));
    }
    if let Some(v) = d.avatar.patch() {
        m.insert("avatar".into(), v);
    }
    (!m.is_empty()).then(|| {
        Request::new(
            guild_id,
            Kind::SaveWebhook,
            Method::Patch,
            format!("/webhooks/{id}"),
        )
        .key(id)
        .body(Value::Object(m))
    })
}

pub fn delete(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteWebhook,
        Method::Delete,
        format!("/webhooks/{id}"),
    )
    .key(id)
}

pub fn load_integrations(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadIntegrations,
        Method::Get,
        format!("/guilds/{guild_id}/integrations"),
    )
}

pub fn delete_integration(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteIntegration,
        Method::Delete,
        format!("/guilds/{guild_id}/integrations/{id}"),
    )
    .key(id)
}

struct Edit {
    id: String,
    base: Draft,
    draft: Draft,
    has_avatar: bool,
}

#[derive(Default)]
pub struct State {
    hooks: Option<Vec<Value>>,
    integrations: Option<Vec<Value>>,
    /// Webhooks whose URL the user chose to reveal.
    revealed: Vec<String>,
    edit: Option<Edit>,
    new_name: String,
    new_channel: String,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadWebhooks => {
                self.hooks = Some(super::list(v));
                self.revealed.clear();
            }
            Kind::CreateWebhook | Kind::SaveWebhook => {
                if let Some(list) = &mut self.hooks {
                    let mut item = v.clone();
                    // The demo echoes the uploaded picture: not kept.
                    if let Some(o) = item.as_object_mut()
                        && o.get("avatar")
                            .and_then(Value::as_str)
                            .is_some_and(|a| a.starts_with("data:"))
                    {
                        o.insert("avatar".into(), json!("uploaded"));
                    }
                    super::upsert(list, &item);
                }
                if kind == Kind::CreateWebhook {
                    self.new_name.clear();
                } else {
                    self.edit = None;
                }
            }
            Kind::DeleteWebhook => {
                if let Some(list) = &mut self.hooks {
                    super::remove(list, key);
                }
                self.revealed.retain(|r| r != key);
                self.edit = None;
            }
            Kind::LoadIntegrations => self.integrations = Some(super::list(v)),
            Kind::DeleteIntegration => {
                if let Some(list) = &mut self.integrations {
                    super::remove(list, key);
                }
            }
            _ => {}
        }
    }

    pub fn on_picked(&mut self, p: Arc<Picked>) {
        if let Some(e) = &mut self.edit {
            e.draft.avatar = ImageEdit::New(p);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        // Channels where I may manage webhooks (overwrites included).
        let channels: Vec<(String, String)> = cx
            .channel_options(&[0, 5, 15], None)
            .into_iter()
            .filter(|(id, _)| cx.channel_perms(id) & perms::MANAGE_WEBHOOKS != 0)
            .collect();
        aui::hint(
            ui,
            pal,
            "Webhooks let other apps post messages into a channel. Treat a webhook URL like a password: anyone who has it can post.",
        );
        ui.add_space(4.0);
        if !channels.is_empty() {
            if !channels.iter().any(|(id, _)| *id == self.new_channel) {
                self.new_channel = channels[0].0.clone();
            }
            aui::card(ui, pal, |ui| {
                aui::heading(ui, pal, "New webhook");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .hint_text("Name, e.g. Build Bot")
                            .desired_width(200.0)
                            .char_limit(80),
                    );
                    ui.scope(|ui| {
                        ui.set_max_width(200.0);
                        aui::select(ui, pal, "wh_new_channel", &mut self.new_channel, &channels);
                    });
                    if aui::primary(ui, pal, "Create Webhook", valid_name(&self.new_name)).clicked()
                    {
                        out.send
                            .extend(create(&cx.guild.id, &self.new_channel, &self.new_name));
                    }
                });
            });
            ui.add_space(10.0);
        }

        let Some(hooks) = &self.hooks else {
            aui::loading(ui, pal);
            return;
        };
        if hooks.is_empty() {
            aui::empty(ui, pal, icons::LINK, "This server has no webhooks.");
            return;
        }
        let mut toggle_reveal = None;
        let mut set_edit: Option<Option<Edit>> = None;
        for h in hooks {
            let id = s(h, "id");
            let editing = self.edit.as_ref().is_some_and(|e| e.id == id);
            let revealed = self.revealed.iter().any(|r| r == id);
            let channel = cx
                .guild
                .channels
                .iter()
                .find(|c| c.id == s(h, "channel_id"))
                .map_or_else(
                    || "unknown channel".to_string(),
                    |c| super::channel_label(c.kind, &c.name),
                );
            aui::card(ui, pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(icons::LINK)
                            .font(icons::font(18.0))
                            .color(pal.muted),
                    );
                    ui.label(RichText::new(s(h, "name")).color(pal.strong).bold());
                    ui.label(RichText::new(format!("posts in {channel}")).color(pal.muted));
                    let by = s(&h["user"], "username");
                    if !by.is_empty() {
                        ui.label(
                            RichText::new(format!("· created by @{by}"))
                                .size(12.0)
                                .color(pal.muted),
                        );
                    }
                    aui::right(ui, |ui| {
                        if crate::widgets::icon_button(
                            ui,
                            icons::TRASH,
                            "Delete webhook",
                            false,
                            true,
                            pal,
                        )
                        .clicked()
                        {
                            out.confirm = Some(
                                Confirm::new(
                                    "Delete webhook",
                                    &format!(
                                        "Delete the webhook \"{}\"? Anything still posting to its URL stops working.",
                                        s(h, "name")
                                    ),
                                    "Delete",
                                    delete(&cx.guild.id, id),
                                )
                                .reason(),
                            );
                        }
                        if crate::widgets::icon_button(
                            ui,
                            icons::PENCIL,
                            "Edit webhook",
                            editing,
                            true,
                            pal,
                        )
                        .clicked()
                        {
                            set_edit = Some((!editing).then(|| {
                                let draft = Draft::from_json(h);
                                Edit {
                                    id: id.to_string(),
                                    base: draft.clone(),
                                    draft,
                                    has_avatar: !s(h, "avatar").is_empty(),
                                }
                            }));
                        }
                    });
                });
                match shown_url(h, revealed) {
                    Some(text) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(text)
                                    .monospace()
                                    .size(12.5)
                                    .color(if revealed { pal.text } else { pal.muted }),
                            );
                        });
                        ui.horizontal(|ui| {
                            if aui::icon_text(ui, icons::COPY, "Copy URL").clicked()
                                && let Some(url) = secret_url(h)
                            {
                                ui.ctx().copy_text(url);
                            }
                            let (icon, label) = if revealed {
                                (icons::EYE_SLASH, "Hide")
                            } else {
                                (icons::EYE, "Reveal")
                            };
                            if aui::icon_text(ui, icon, label).clicked() {
                                toggle_reveal = Some(id.to_string());
                            }
                        });
                    }
                    None => aui::hint(
                        ui,
                        pal,
                        match n(h, "type") {
                            2 => "Follows an announcement channel from another server (no URL).",
                            _ => "Managed by an app (no URL).",
                        },
                    ),
                }
                if editing && let Some(e) = &mut self.edit {
                    aui::divider(ui, pal);
                    aui::label(ui, pal, "Name");
                    aui::text(ui, &mut e.draft.name, "Webhook name", 80);
                    if !valid_name(&e.draft.name) {
                        aui::hint(
                            ui,
                            pal,
                            "1 to 80 characters, without \"clyde\" or \"discord\".",
                        );
                    }
                    aui::label(ui, pal, "Channel");
                    aui::select(ui, pal, ("wh_channel", id), &mut e.draft.channel, &channels);
                    aui::label(ui, pal, "Avatar");
                    ui.horizontal(|ui| {
                        let status = match &e.draft.avatar {
                            ImageEdit::New(p) => format!("{} (uploads when you save)", p.name),
                            ImageEdit::Remove => "Will be removed when you save".into(),
                            ImageEdit::Keep if e.has_avatar => "An avatar is set".into(),
                            ImageEdit::Keep => "Default avatar".to_string(),
                        };
                        ui.label(RichText::new(status).color(pal.text));
                        if aui::icon_text(ui, icons::UPLOAD, "Upload").clicked() {
                            out.pick = Some(PickSlot::WebhookAvatar);
                        }
                        if e.has_avatar
                            && matches!(e.draft.avatar, ImageEdit::Keep)
                            && aui::plain(ui, "Remove").clicked()
                        {
                            e.draft.avatar = ImageEdit::Remove;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let req = save(&cx.guild.id, id, &e.base, &e.draft);
                        if aui::save(ui, pal, "Save Changes", req.is_some()).clicked() {
                            out.send.extend(req);
                        }
                        if aui::plain(ui, "Cancel").clicked() {
                            set_edit = Some(None);
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
        if let Some(id) = toggle_reveal {
            if self.revealed.contains(&id) {
                self.revealed.retain(|r| *r != id);
            } else {
                self.revealed.push(id);
            }
        }
        if let Some(e) = set_edit {
            self.edit = e;
        }
    }

    pub fn integrations_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(list) = &self.integrations else {
            aui::loading(ui, pal);
            return;
        };
        aui::hint(
            ui,
            pal,
            "Apps and services connected to this server. Removing one also removes its bot, its webhooks and its managed role.",
        );
        ui.add_space(4.0);
        if list.is_empty() {
            aui::empty(ui, pal, icons::PUZZLE, "No integrations.");
            return;
        }
        for i in list {
            let name = s(i, "name");
            aui::row(ui, pal, false, |ui| {
                ui.label(
                    RichText::new(icons::PUZZLE)
                        .font(icons::font(18.0))
                        .color(pal.muted),
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new(name).color(pal.strong).bold());
                    let kind = match s(i, "type") {
                        "discord" => "App",
                        "twitch" => "Twitch",
                        "youtube" => "YouTube",
                        "guild_subscription" => "Server subscription",
                        other => other,
                    };
                    let by = s(&i["user"], "username");
                    let state = if b(i, "enabled") || i.get("enabled").is_none() {
                        ""
                    } else {
                        " · disabled"
                    };
                    ui.label(
                        RichText::new(if by.is_empty() {
                            format!("{kind}{state}")
                        } else {
                            format!("{kind} · added by @{by}{state}")
                        })
                        .size(13.0)
                        .color(pal.muted),
                    );
                });
                aui::right(ui, |ui| {
                    if aui::danger(ui, pal, "Remove", true).clicked() {
                        out.confirm = Some(
                            Confirm::new(
                                "Remove integration",
                                &format!(
                                    "Remove {name} from this server? Its bot is kicked and its webhooks and managed role are deleted."
                                ),
                                "Remove",
                                delete_integration(&cx.guild.id, s(i, "id")),
                            )
                            .typed(name)
                            .reason(),
                        );
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> Value {
        json!({ "id": "9", "type": 1, "name": "CI", "channel_id": "10", "token": "SECRET" })
    }

    #[test]
    fn the_url_is_masked_until_revealed() {
        let h = hook();
        assert_eq!(
            secret_url(&h).unwrap(),
            "https://discord.com/api/webhooks/9/SECRET"
        );
        let hidden = shown_url(&h, false).unwrap();
        assert!(!hidden.contains("SECRET"));
        assert!(shown_url(&h, true).unwrap().contains("SECRET"));
        // Nothing is revealed by default, and a reload hides everything again.
        let mut st = State::default();
        st.on_response(Kind::LoadWebhooks, "", &json!([h]));
        assert!(st.revealed.is_empty());
        st.revealed.push("9".into());
        st.on_response(Kind::LoadWebhooks, "", &json!([hook()]));
        assert!(st.revealed.is_empty());
        // App / follower webhooks have no URL at all.
        assert!(shown_url(&json!({ "id": "1", "type": 3 }), true).is_none());
    }

    #[test]
    fn requests_never_carry_the_token() {
        let base = Draft::from_json(&hook());
        let mut d = base.clone();
        d.name = "Deploys".into();
        d.channel = "11".into();
        for r in [
            save("1", "9", &base, &d).unwrap(),
            delete("1", "9"),
            create("1", "10", "CI").unwrap(),
            load("1"),
        ] {
            assert!(!r.path.contains("SECRET"));
            assert!(!format!("{r:?}").contains("SECRET"));
            let body = r.body.map(|b| b.to_string()).unwrap_or_default();
            assert!(!body.contains("SECRET"));
        }
        let r = save("1", "9", &base, &d).unwrap();
        assert_eq!((r.method, r.path.as_str()), (Method::Patch, "/webhooks/9"));
        assert_eq!(
            r.body,
            Some(json!({ "name": "Deploys", "channel_id": "11" }))
        );
        assert!(save("1", "9", &base, &base.clone()).is_none());
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("Build Bot"));
        assert!(!valid_name("  "));
        assert!(!valid_name("Discord Updates"));
        assert!(!valid_name("clyde"));
        assert!(create("1", "", "CI").is_none());
        assert!(create("1", "10", "").is_none());
    }

    #[test]
    fn integrations() {
        assert!(!load_integrations("1").is_write());
        let d = delete_integration("1", "5");
        assert_eq!(
            (d.method, d.path.as_str()),
            (Method::Delete, "/guilds/1/integrations/5")
        );
        let mut st = State::default();
        st.on_response(Kind::LoadIntegrations, "", &json!([{ "id": "5" }]));
        st.on_response(Kind::DeleteIntegration, "5", &Value::Null);
        assert!(st.integrations.unwrap().is_empty());
    }
}
