//! Server settings → AutoMod: list, create, edit and delete rules
//! (keyword filters, spam and mention spam) with their actions (block
//! the message, send an alert, time the member out) and exemptions.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::{Confirm, Cx, Kind, Method, Out, Request, b, n, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

pub const KEYWORD: u64 = 1;
pub const SPAM: u64 = 3;
pub const KEYWORD_PRESET: u64 = 4;
pub const MENTION_SPAM: u64 = 5;

pub fn trigger_label(t: u64) -> &'static str {
    match t {
        KEYWORD => "Custom keywords",
        SPAM => "Suspected spam",
        KEYWORD_PRESET => "Commonly flagged words",
        MENTION_SPAM => "Mention spam",
        6 => "Member profile",
        _ => "Other",
    }
}

pub const RULE_TIMEOUTS: &[(u64, &str)] = &[
    (0, "Don't time out"),
    (60, "60 seconds"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (3600, "1 hour"),
    (86_400, "1 day"),
    (604_800, "1 week"),
];

/// Words from a text box: one per line or comma, trimmed, no duplicates,
/// at most 1000 (Discord's limit).
pub fn split_words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(['\n', ',']).map(str::trim) {
        if !w.is_empty() && !out.iter().any(|o| o == w) && out.len() < 1000 {
            out.push(w.to_string());
        }
    }
    out
}

fn join_words(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub name: String,
    pub enabled: bool,
    pub trigger: u64,
    pub keywords: String,
    pub allow: String,
    pub mention_limit: u64,
    pub block: bool,
    pub block_message: String,
    pub alert_channel: String,
    pub timeout: u64,
    pub exempt_roles: Vec<String>,
    pub exempt_channels: Vec<String>,
}

impl Draft {
    pub fn new(trigger: u64) -> Self {
        Self {
            name: match trigger {
                SPAM => "Block suspected spam",
                MENTION_SPAM => "Block mention spam",
                _ => "Block custom words",
            }
            .to_string(),
            enabled: true,
            trigger,
            mention_limit: 20,
            block: true,
            ..Default::default()
        }
    }

    pub fn from_json(v: &Value) -> Self {
        let meta = &v["trigger_metadata"];
        let mut d = Self {
            name: s(v, "name").to_string(),
            enabled: b(v, "enabled"),
            trigger: n(v, "trigger_type"),
            keywords: join_words(&meta["keyword_filter"]),
            allow: join_words(&meta["allow_list"]),
            mention_limit: n(meta, "mention_total_limit"),
            exempt_roles: ids(&v["exempt_roles"]),
            exempt_channels: ids(&v["exempt_channels"]),
            ..Default::default()
        };
        for a in v["actions"].as_array().into_iter().flatten() {
            let m = &a["metadata"];
            match n(a, "type") {
                1 => {
                    d.block = true;
                    d.block_message = s(m, "custom_message").to_string();
                }
                2 => d.alert_channel = s(m, "channel_id").to_string(),
                3 => d.timeout = n(m, "duration_seconds"),
                _ => {}
            }
        }
        d
    }

    /// Timeouts are only allowed on keyword and mention-spam rules.
    pub fn can_timeout(&self) -> bool {
        matches!(self.trigger, KEYWORD | MENTION_SPAM)
    }

    fn actions(&self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.block {
            let msg = self.block_message.trim();
            out.push(if msg.is_empty() {
                json!({ "type": 1 })
            } else {
                json!({ "type": 1, "metadata": { "custom_message": msg } })
            });
        }
        if !self.alert_channel.is_empty() {
            out.push(json!({ "type": 2, "metadata": { "channel_id": self.alert_channel } }));
        }
        if self.timeout > 0 && self.can_timeout() {
            out.push(json!({ "type": 3, "metadata": { "duration_seconds": self.timeout } }));
        }
        out
    }

    pub fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && !self.actions().is_empty()
            && (self.trigger != KEYWORD || !split_words(&self.keywords).is_empty())
            && (self.trigger != MENTION_SPAM || (1..=50).contains(&self.mention_limit))
    }

    /// The rule body. `trigger_type` is only sent when creating.
    pub fn body(&self, create: bool) -> Value {
        let mut v = json!({
            "name": self.name.trim(),
            "enabled": self.enabled,
            "event_type": 1,
            "actions": self.actions(),
            "exempt_roles": self.exempt_roles,
            "exempt_channels": self.exempt_channels,
        });
        match self.trigger {
            KEYWORD => {
                v["trigger_metadata"] = json!({
                    "keyword_filter": split_words(&self.keywords),
                    "allow_list": split_words(&self.allow),
                });
            }
            MENTION_SPAM => {
                v["trigger_metadata"] = json!({ "mention_total_limit": self.mention_limit });
            }
            _ => {}
        }
        if create {
            v["trigger_type"] = json!(self.trigger);
        }
        v
    }
}

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadRules,
        Method::Get,
        format!("/guilds/{guild_id}/auto-moderation/rules"),
    )
}

pub fn create(guild_id: &str, d: &Draft) -> Option<Request> {
    d.valid().then(|| {
        Request::new(
            guild_id,
            Kind::CreateRule,
            Method::Post,
            format!("/guilds/{guild_id}/auto-moderation/rules"),
        )
        .body(d.body(true))
    })
}

pub fn save(guild_id: &str, id: &str, base: &Draft, d: &Draft) -> Option<Request> {
    (d.valid() && d != base).then(|| {
        Request::new(
            guild_id,
            Kind::SaveRule,
            Method::Patch,
            format!("/guilds/{guild_id}/auto-moderation/rules/{id}"),
        )
        .key(id)
        .body(d.body(false))
    })
}

pub fn delete(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteRule,
        Method::Delete,
        format!("/guilds/{guild_id}/auto-moderation/rules/{id}"),
    )
    .key(id)
}

struct Edit {
    id: Option<String>,
    base: Draft,
    draft: Draft,
}

#[derive(Default)]
pub struct State {
    rules: Option<Vec<Value>>,
    edit: Option<Edit>,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadRules => self.rules = Some(super::list(v)),
            Kind::CreateRule | Kind::SaveRule => {
                if let Some(list) = &mut self.rules {
                    super::upsert(list, v);
                }
                self.edit = None;
            }
            Kind::DeleteRule => {
                if let Some(list) = &mut self.rules {
                    super::remove(list, key);
                }
                self.edit = None;
            }
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(rules) = &self.rules else {
            aui::loading(ui, pal);
            return;
        };
        if let Some(e) = &mut self.edit {
            let mut back = false;
            ui.horizontal(|ui| {
                if aui::icon_text(ui, icons::CARET_LEFT, "AutoMod").clicked() {
                    back = true;
                }
                ui.label(
                    RichText::new(trigger_label(e.draft.trigger))
                        .size(16.0)
                        .color(pal.strong)
                        .bold(),
                );
            });
            aui::divider(ui, pal);
            edit_ui(ui, cx, e, out);
            if back {
                self.edit = None;
            }
            return;
        }

        aui::hint(
            ui,
            pal,
            "AutoMod checks messages before they are posted. Members with Manage Server or Administrator are never filtered.",
        );
        ui.add_space(4.0);
        let mut new = None;
        ui.horizontal_wrapped(|ui| {
            if aui::primary(ui, pal, "Create Keyword Rule", true).clicked() {
                new = Some(KEYWORD);
            }
            // Discord allows one spam rule and one mention-spam rule.
            let has = |t: u64| rules.iter().any(|r| n(r, "trigger_type") == t);
            if !has(SPAM) && aui::plain(ui, "Create Spam Rule").clicked() {
                new = Some(SPAM);
            }
            if !has(MENTION_SPAM) && aui::plain(ui, "Create Mention Spam Rule").clicked() {
                new = Some(MENTION_SPAM);
            }
        });
        ui.add_space(6.0);
        if rules.is_empty() {
            aui::empty(ui, pal, icons::SHIELD, "No AutoMod rules yet.");
        }
        let mut open = None;
        for r in rules {
            let (_, clicked) = aui::row(ui, pal, false, |ui| {
                ui.label(RichText::new(icons::SHIELD).font(icons::font(18.0)).color(
                    if b(r, "enabled") {
                        pal.positive()
                    } else {
                        pal.muted
                    },
                ));
                ui.vertical(|ui| {
                    ui.label(RichText::new(s(r, "name")).color(pal.strong).bold());
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            trigger_label(n(r, "trigger_type")),
                            if b(r, "enabled") { "on" } else { "off" }
                        ))
                        .size(13.0)
                        .color(pal.muted),
                    );
                });
                aui::right(ui, |ui| {
                    crate::widgets::icon_button(ui, icons::PENCIL, "Edit rule", false, true, pal)
                        .clicked()
                })
            });
            if clicked {
                open = Some(r.clone());
            }
        }
        if let Some(r) = open {
            let draft = Draft::from_json(&r);
            self.edit = Some(Edit {
                id: Some(s(&r, "id").to_string()),
                base: draft.clone(),
                draft,
            });
        } else if let Some(t) = new {
            let draft = Draft::new(t);
            self.edit = Some(Edit {
                id: None,
                base: draft.clone(),
                draft,
            });
        }
    }
}

fn edit_ui(ui: &mut egui::Ui, cx: &Cx, e: &mut Edit, out: &mut Out) {
    let pal = cx.pal;
    let d = &mut e.draft;
    aui::label(ui, pal, "Rule name");
    aui::text(ui, &mut d.name, "Rule name", 100);
    ui.add_space(6.0);
    crate::settings::switch_row(
        ui,
        &mut d.enabled,
        "Rule enabled",
        "When off, the rule is kept but does nothing.",
        pal,
    );

    match d.trigger {
        KEYWORD => {
            aui::label(ui, pal, "Blocked words or phrases");
            aui::text_area(
                ui,
                &mut d.keywords,
                "One per line (or separated by commas)",
                20_000,
                5,
            );
            aui::hint(
                ui,
                pal,
                "Use * at the start or end of a word to match partial words, e.g. spam*.",
            );
            aui::label(ui, pal, "Allowed words (never blocked)");
            aui::text_area(ui, &mut d.allow, "One per line", 5000, 2);
        }
        MENTION_SPAM => {
            aui::label(ui, pal, "Mentions allowed in one message");
            d.mention_limit = d.mention_limit.clamp(1, 50);
            ui.add(egui::Slider::new(&mut d.mention_limit, 1..=50));
        }
        SPAM => aui::hint(
            ui,
            pal,
            "Discord decides what looks like spam. There is nothing to configure for this rule.",
        ),
        _ => aui::hint(
            ui,
            pal,
            "This kind of rule's filter can't be edited here yet; its name, actions and exemptions can.",
        ),
    }

    aui::divider(ui, pal);
    aui::heading(ui, pal, "When a message matches");
    ui.checkbox(&mut d.block, "Block the message");
    if d.block {
        aui::text(
            ui,
            &mut d.block_message,
            "Optional: a custom note shown to the member",
            150,
        );
    }
    aui::label(ui, pal, "Send an alert to");
    aui::select(
        ui,
        pal,
        "am_alert",
        &mut d.alert_channel,
        &cx.channel_options(&[0], Some("No alert")),
    );
    if d.can_timeout() && cx.can(perms::MODERATE_MEMBERS) {
        aui::label(ui, pal, "Time out the member");
        aui::select(
            ui,
            pal,
            "am_timeout",
            &mut d.timeout,
            &aui::opts(RULE_TIMEOUTS),
        );
    }

    aui::divider(ui, pal);
    aui::heading(ui, pal, "Exemptions");
    aui::label(ui, pal, "Roles this rule ignores");
    ui.horizontal_wrapped(|ui| {
        for r in cx.guild.roles.iter().filter(|r| r.id != cx.guild.id) {
            let mut on = d.exempt_roles.contains(&r.id);
            if ui
                .checkbox(
                    &mut on,
                    RichText::new(&r.name).color(aui::rgb_color(pal, r.color)),
                )
                .changed()
            {
                d.exempt_roles.retain(|x| x != &r.id);
                if on && d.exempt_roles.len() < 20 {
                    d.exempt_roles.push(r.id.clone());
                }
            }
        }
    });
    aui::label(ui, pal, "Channels this rule ignores");
    ui.horizontal_wrapped(|ui| {
        for (id, label) in cx.channel_options(&[0, 2, 5, 13, 15], None) {
            let mut on = d.exempt_channels.contains(&id);
            if ui.checkbox(&mut on, label).changed() {
                d.exempt_channels.retain(|x| x != &id);
                if on && d.exempt_channels.len() < 50 {
                    d.exempt_channels.push(id);
                }
            }
        }
    });

    aui::divider(ui, pal);
    if !e.draft.valid() {
        aui::hint(
            ui,
            pal,
            "A rule needs a name, at least one action, and (for keyword rules) at least one word.",
        );
    }
    ui.horizontal(|ui| {
        let (label, req) = match &e.id {
            Some(id) => ("Save Changes", save(&cx.guild.id, id, &e.base, &e.draft)),
            None => ("Create Rule", create(&cx.guild.id, &e.draft)),
        };
        if aui::save(ui, pal, label, req.is_some()).clicked() {
            out.send.extend(req);
        }
        if e.id.is_some() && e.draft != e.base && aui::plain(ui, "Reset").clicked() {
            e.draft = e.base.clone();
        }
        if let Some(id) = &e.id {
            aui::right(ui, |ui| {
                if aui::danger(ui, pal, "Delete Rule", true).clicked() {
                    out.confirm = Some(
                        Confirm::new(
                            "Delete rule",
                            &format!("Delete the AutoMod rule \"{}\"?", e.base.name),
                            "Delete Rule",
                            delete(&cx.guild.id, id),
                        )
                        .reason(),
                    );
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_split_trimmed_and_deduped() {
        assert_eq!(
            split_words(" spam*, scam\nspam* ,, \n free nitro "),
            ["spam*", "scam", "free nitro"]
        );
        assert!(split_words(" ,\n").is_empty());
    }

    #[test]
    fn keyword_rule_body() {
        let mut d = Draft::new(KEYWORD);
        assert!(!d.valid(), "no keywords yet");
        assert!(create("1", &d).is_none());
        d.keywords = "spam*\nscam".into();
        d.alert_channel = "10".into();
        d.timeout = 60;
        let r = create("1", &d).unwrap();
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.path, "/guilds/1/auto-moderation/rules");
        assert_eq!(
            r.body,
            Some(json!({
                "name": "Block custom words", "enabled": true, "event_type": 1,
                "trigger_type": 1,
                "trigger_metadata": { "keyword_filter": ["spam*", "scam"], "allow_list": [] },
                "actions": [
                    { "type": 1 },
                    { "type": 2, "metadata": { "channel_id": "10" } },
                    { "type": 3, "metadata": { "duration_seconds": 60 } },
                ],
                "exempt_roles": [], "exempt_channels": [],
            }))
        );
    }

    #[test]
    fn spam_rules_never_time_out_and_need_an_action() {
        let mut d = Draft::new(SPAM);
        d.timeout = 60;
        let body = d.body(true);
        assert_eq!(body["actions"], json!([{ "type": 1 }]));
        assert!(body.get("trigger_metadata").is_none());
        d.block = false;
        assert!(!d.valid());
    }

    #[test]
    fn round_trip_and_save_only_when_changed() {
        let rule = json!({
            "id": "7", "name": "Words", "enabled": true, "trigger_type": 1,
            "trigger_metadata": { "keyword_filter": ["a", "b"], "allow_list": ["c"] },
            "actions": [{ "type": 1, "metadata": { "custom_message": "no" } },
                        { "type": 2, "metadata": { "channel_id": "10" } }],
            "exempt_roles": ["r"], "exempt_channels": [],
        });
        let base = Draft::from_json(&rule);
        assert_eq!(base.keywords, "a\nb");
        assert_eq!(base.block_message, "no");
        assert_eq!(base.alert_channel, "10");
        assert!(save("1", "7", &base, &base.clone()).is_none());
        let mut d = base.clone();
        d.enabled = false;
        let r = save("1", "7", &base, &d).unwrap();
        assert_eq!(r.method, Method::Patch);
        assert_eq!(r.path, "/guilds/1/auto-moderation/rules/7");
        let body = r.body.unwrap();
        assert_eq!(body["enabled"], false);
        assert!(body.get("trigger_type").is_none());
        let del = delete("1", "7");
        assert_eq!((del.method, del.key.as_str()), (Method::Delete, "7"));
    }
}
