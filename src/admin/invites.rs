//! Server settings → Invites: list the server's invites, create one (max
//! uses and expiry) and revoke. Listing needs Manage Server; creating
//! needs Create Invite in the chosen channel.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::{Confirm, Cx, Kind, Method, Out, Request, b, n, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

pub const MAX_AGE: &[(u64, &str)] = &[
    (1800, "30 minutes"),
    (3600, "1 hour"),
    (21_600, "6 hours"),
    (43_200, "12 hours"),
    (86_400, "1 day"),
    (604_800, "7 days"),
    (0, "Never"),
];

pub const MAX_USES: &[(u64, &str)] = &[
    (0, "No limit"),
    (1, "1 use"),
    (5, "5 uses"),
    (10, "10 uses"),
    (25, "25 uses"),
    (50, "50 uses"),
    (100, "100 uses"),
];

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadInvites,
        Method::Get,
        format!("/guilds/{guild_id}/invites"),
    )
}

/// A new, unique invite to a channel. `None` without a channel.
pub fn create(
    guild_id: &str,
    channel_id: &str,
    max_age: u64,
    max_uses: u64,
    temporary: bool,
) -> Option<Request> {
    (!channel_id.is_empty()).then(|| {
        Request::new(
            guild_id,
            Kind::CreateInvite,
            Method::Post,
            format!("/channels/{channel_id}/invites"),
        )
        .body(json!({ "max_age": max_age, "max_uses": max_uses,
                      "temporary": temporary, "unique": true }))
    })
}

pub fn revoke(guild_id: &str, code: &str) -> Request {
    Request::new(
        guild_id,
        Kind::RevokeInvite,
        Method::Delete,
        format!("/invites/{code}"),
    )
    .key(code)
}

pub fn link(code: &str) -> String {
    format!("https://discord.gg/{code}")
}

pub struct State {
    invites: Option<Vec<Value>>,
    channel: String,
    max_age: u64,
    max_uses: u64,
    temporary: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            invites: None,
            channel: String::new(),
            max_age: 604_800,
            max_uses: 0,
            temporary: false,
        }
    }
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadInvites => self.invites = Some(super::list(v)),
            Kind::CreateInvite => {
                let code = s(v, "code");
                if let Some(list) = &mut self.invites
                    && !code.is_empty()
                    && list.len() < super::LIST_CAP
                    && !list.iter().any(|i| s(i, "code") == code)
                {
                    list.insert(0, v.clone());
                }
            }
            Kind::RevokeInvite => {
                if let Some(list) = &mut self.invites {
                    list.retain(|i| s(i, "code") != key);
                }
            }
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        // Channels I may invite to (overwrites included).
        let channels: Vec<(String, String)> = cx
            .channel_options(&[0, 2, 5, 13, 15], None)
            .into_iter()
            .filter(|(id, _)| cx.channel_perms(id) & perms::CREATE_INSTANT_INVITE != 0)
            .collect();
        if !channels.is_empty() {
            if !channels.iter().any(|(id, _)| *id == self.channel) {
                self.channel = channels[0].0.clone();
            }
            aui::card(ui, pal, |ui| {
                aui::heading(ui, pal, "Create an invite");
                ui.horizontal_wrapped(|ui| {
                    ui.vertical(|ui| {
                        ui.label(pal.section_label("Channel"));
                        ui.set_max_width(200.0);
                        aui::select(ui, pal, "inv_channel", &mut self.channel, &channels);
                    });
                    ui.vertical(|ui| {
                        ui.label(pal.section_label("Expire after"));
                        ui.set_max_width(160.0);
                        aui::select(ui, pal, "inv_age", &mut self.max_age, &aui::opts(MAX_AGE));
                    });
                    ui.vertical(|ui| {
                        ui.label(pal.section_label("Max number of uses"));
                        ui.set_max_width(160.0);
                        aui::select(
                            ui,
                            pal,
                            "inv_uses",
                            &mut self.max_uses,
                            &aui::opts(MAX_USES),
                        );
                    });
                });
                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.temporary,
                    "Temporary membership (members are kicked when they go offline, unless they get a role)",
                );
                ui.add_space(6.0);
                if aui::primary(ui, pal, "Create Invite", true).clicked() {
                    out.send.extend(create(
                        &cx.guild.id,
                        &self.channel,
                        self.max_age,
                        self.max_uses,
                        self.temporary,
                    ));
                }
            });
            ui.add_space(10.0);
        }

        let Some(invites) = &self.invites else {
            aui::loading(ui, pal);
            return;
        };
        if invites.is_empty() {
            aui::empty(ui, pal, icons::ENVELOPE, "This server has no invites.");
            return;
        }
        aui::label(ui, pal, &format!("Active invites ({})", invites.len()));
        for inv in invites {
            let code = s(inv, "code");
            aui::row(ui, pal, false, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(code).color(pal.strong).bold());
                        ui.label(
                            RichText::new(super::channel_label(
                                n(&inv["channel"], "type"),
                                s(&inv["channel"], "name"),
                            ))
                            .color(pal.muted),
                        );
                        if b(inv, "temporary") {
                            ui.label(RichText::new("temporary").size(12.0).color(pal.muted));
                        }
                    });
                    let by = s(&inv["inviter"], "username");
                    let uses = match n(inv, "max_uses") {
                        0 => format!("{} uses", n(inv, "uses")),
                        max => format!("{} / {max} uses", n(inv, "uses")),
                    };
                    let expires = match s(inv, "expires_at") {
                        "" => "never expires".to_string(),
                        at => format!("expires {}", crate::timefmt::full_iso(at)),
                    };
                    let by = if by.is_empty() {
                        String::new()
                    } else {
                        format!("by @{by} · ")
                    };
                    ui.label(
                        RichText::new(format!("{by}{uses} · {expires}"))
                            .size(13.0)
                            .color(pal.muted),
                    );
                });
                aui::right(ui, |ui| {
                    if aui::danger(ui, pal, "Revoke", true).clicked() {
                        out.confirm = Some(Confirm::new(
                            "Revoke invite",
                            &format!("Revoke the invite {code}? The link stops working."),
                            "Revoke",
                            revoke(&cx.guild.id, code),
                        ));
                    }
                    if crate::widgets::icon_button(ui, icons::COPY, "Copy link", false, true, pal)
                        .clicked()
                    {
                        ui.ctx().copy_text(link(code));
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_needs_a_channel() {
        assert!(create("1", "", 0, 0, false).is_none());
        let r = create("1", "10", 3600, 5, true).unwrap();
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.path, "/channels/10/invites");
        assert_eq!(
            r.body,
            Some(json!({ "max_age": 3600, "max_uses": 5, "temporary": true, "unique": true }))
        );
    }

    #[test]
    fn revoke_and_list_updates() {
        let r = revoke("1", "abc");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Delete, "/invites/abc")
        );
        let mut st = State::default();
        st.on_response(Kind::LoadInvites, "", &json!([{ "code": "abc" }]));
        st.on_response(Kind::CreateInvite, "", &json!({ "code": "new" }));
        st.on_response(Kind::CreateInvite, "", &json!({ "code": "new" }));
        assert_eq!(st.invites.as_ref().unwrap().len(), 2);
        assert_eq!(st.invites.as_ref().unwrap()[0]["code"], "new");
        st.on_response(Kind::RevokeInvite, "abc", &Value::Null);
        assert_eq!(st.invites.as_ref().unwrap().len(), 1);
        assert_eq!(link("abc"), "https://discord.gg/abc");
    }
}
