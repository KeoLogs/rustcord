//! Server settings → Overview: name, icon, banner, description, system
//! and AFK channels, AFK timeout, verification level, default
//! notifications and the explicit content filter. Edits go into a draft;
//! only "Save Changes" sends them (one PATCH with the changed fields).

use std::sync::Arc;

use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};

use super::{Cx, Kind, Method, Out, PickSlot, Picked, Request, n, s, ui as aui};
use crate::icons;
use crate::theme::Bold;

/// A picture field: keep what's there, remove it, or upload a new one.
#[derive(Clone, Default)]
pub enum ImageEdit {
    #[default]
    Keep,
    Remove,
    New(Arc<Picked>),
}

impl PartialEq for ImageEdit {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImageEdit::Keep, ImageEdit::Keep) | (ImageEdit::Remove, ImageEdit::Remove) => true,
            (ImageEdit::New(a), ImageEdit::New(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl ImageEdit {
    /// The JSON value to PATCH, if this is a change.
    pub fn patch(&self) -> Option<Value> {
        match self {
            ImageEdit::Keep => None,
            ImageEdit::Remove => Some(Value::Null),
            ImageEdit::New(p) => Some(Value::String(p.data_uri())),
        }
    }
}

/// The editable fields.
#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub name: String,
    pub description: String,
    pub system_channel: String,
    pub afk_channel: String,
    pub afk_timeout: u64,
    pub verification: u64,
    pub notifications: u64,
    pub explicit: u64,
    pub icon: ImageEdit,
    pub banner: ImageEdit,
}

impl Draft {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: s(v, "name").to_string(),
            description: s(v, "description").to_string(),
            system_channel: s(v, "system_channel_id").to_string(),
            afk_channel: s(v, "afk_channel_id").to_string(),
            afk_timeout: match n(v, "afk_timeout") {
                0 => 300,
                t => t,
            },
            verification: n(v, "verification_level"),
            notifications: n(v, "default_message_notifications"),
            explicit: n(v, "explicit_content_filter"),
            icon: ImageEdit::Keep,
            banner: ImageEdit::Keep,
        }
    }

    /// Only the fields that differ from `base`.
    pub fn patch(&self, base: &Draft) -> Map<String, Value> {
        let mut m = Map::new();
        let name = self.name.trim();
        if name != base.name {
            m.insert("name".into(), json!(name));
        }
        let desc = self.description.trim();
        if desc != base.description {
            m.insert(
                "description".into(),
                if desc.is_empty() {
                    Value::Null
                } else {
                    json!(desc)
                },
            );
        }
        if self.system_channel != base.system_channel {
            m.insert(
                "system_channel_id".into(),
                super::id_or_null(&self.system_channel),
            );
        }
        if self.afk_channel != base.afk_channel {
            m.insert(
                "afk_channel_id".into(),
                super::id_or_null(&self.afk_channel),
            );
        }
        if self.afk_timeout != base.afk_timeout {
            m.insert("afk_timeout".into(), json!(self.afk_timeout));
        }
        if self.verification != base.verification {
            m.insert("verification_level".into(), json!(self.verification));
        }
        if self.notifications != base.notifications {
            m.insert(
                "default_message_notifications".into(),
                json!(self.notifications),
            );
        }
        if self.explicit != base.explicit {
            m.insert("explicit_content_filter".into(), json!(self.explicit));
        }
        if let Some(v) = self.icon.patch() {
            m.insert("icon".into(), v);
        }
        if let Some(v) = self.banner.patch() {
            m.insert("banner".into(), v);
        }
        m
    }

    /// Valid enough to save? (Discord: 2–100 characters.)
    pub fn valid(&self) -> bool {
        (2..=100).contains(&self.name.trim().chars().count())
    }
}

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadGuild,
        Method::Get,
        format!("/guilds/{guild_id}"),
    )
}

/// The PATCH for a draft, or `None` when nothing changed or it's invalid.
pub fn save(guild_id: &str, base: &Draft, draft: &Draft) -> Option<Request> {
    let patch = draft.patch(base);
    if patch.is_empty() || !draft.valid() {
        return None;
    }
    Some(
        Request::new(
            guild_id,
            Kind::SaveGuild,
            Method::Patch,
            format!("/guilds/{guild_id}"),
        )
        .body(Value::Object(patch)),
    )
}

pub const AFK_TIMEOUTS: &[(u64, &str)] = &[
    (60, "1 minute"),
    (300, "5 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

pub const VERIFICATION: &[(u64, &str)] = &[
    (0, "None: unrestricted"),
    (1, "Low: verified email"),
    (2, "Medium: registered for 5 minutes"),
    (3, "High: a member for 10 minutes"),
    (4, "Highest: verified phone"),
];

pub const NOTIFICATIONS: &[(u64, &str)] = &[(0, "All Messages"), (1, "Only @mentions")];

pub const EXPLICIT: &[(u64, &str)] = &[
    (0, "Don't scan any media"),
    (1, "Scan media from members without a role"),
    (2, "Scan media from all members"),
];

#[derive(Default)]
pub struct State {
    /// The server as Discord last sent it.
    loaded: Option<Value>,
    base: Draft,
    draft: Draft,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, _key: &str, v: &Value) {
        match kind {
            Kind::LoadGuild => {
                self.base = Draft::from_json(v);
                self.draft = self.base.clone();
                self.loaded = Some(v.clone());
            }
            Kind::SaveGuild => {
                if let Some(g) = &mut self.loaded {
                    super::merge(g, v);
                    self.base = Draft::from_json(g);
                    self.draft = self.base.clone();
                }
            }
            _ => {}
        }
    }

    pub fn on_picked(&mut self, p: Arc<Picked>) {
        match p.slot {
            PickSlot::Icon => self.draft.icon = ImageEdit::New(p),
            PickSlot::Banner => self.draft.banner = ImageEdit::New(p),
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(loaded) = &self.loaded else {
            aui::loading(ui, pal);
            return;
        };
        let has_icon = !s(loaded, "icon").is_empty();
        let has_banner = !s(loaded, "banner").is_empty();
        let d = &mut self.draft;

        aui::label(ui, pal, "Server name");
        aui::text(ui, &mut d.name, "Server name", 100);
        if !d.valid() {
            aui::hint(ui, pal, "The name must be 2 to 100 characters long.");
        }

        aui::label(ui, pal, "Description");
        aui::text_area(
            ui,
            &mut d.description,
            "Tell people what this server is about",
            300,
            3,
        );

        aui::divider(ui, pal);
        ui.horizontal_wrapped(|ui| {
            image_field(
                ui,
                cx,
                "Server icon",
                "At least 512×512. PNG, JPG, GIF or WebP.",
                &mut d.icon,
                has_icon,
                PickSlot::Icon,
                out,
            );
            ui.add_space(24.0);
            image_field(
                ui,
                cx,
                "Server banner",
                "Shown above the channel list. 16:9 works best.",
                &mut d.banner,
                has_banner,
                PickSlot::Banner,
                out,
            );
        });

        aui::divider(ui, pal);
        aui::label(ui, pal, "System messages channel");
        aui::select(
            ui,
            pal,
            "ov_system",
            &mut d.system_channel,
            &cx.channel_options(&[0], Some("No system messages")),
        );
        aui::hint(ui, pal, "Where join and boost messages are posted.");

        aui::label(ui, pal, "Inactive (AFK) channel");
        aui::select(
            ui,
            pal,
            "ov_afk",
            &mut d.afk_channel,
            &cx.channel_options(&[2], Some("No inactive channel")),
        );
        aui::label(ui, pal, "Inactive timeout");
        aui::select(
            ui,
            pal,
            "ov_afk_timeout",
            &mut d.afk_timeout,
            &aui::opts(AFK_TIMEOUTS),
        );

        aui::divider(ui, pal);
        aui::label(ui, pal, "Default notification settings");
        aui::select(
            ui,
            pal,
            "ov_notif",
            &mut d.notifications,
            &aui::opts(NOTIFICATIONS),
        );
        aui::label(ui, pal, "Verification level");
        aui::select(
            ui,
            pal,
            "ov_verify",
            &mut d.verification,
            &aui::opts(VERIFICATION),
        );
        aui::hint(
            ui,
            pal,
            "What members need before they can send messages or DM other members.",
        );
        aui::label(ui, pal, "Explicit image filter");
        aui::select(
            ui,
            pal,
            "ov_explicit",
            &mut d.explicit,
            &aui::opts(EXPLICIT),
        );

        aui::divider(ui, pal);
        let dirty = self.draft != self.base;
        ui.horizontal(|ui| {
            let can_save = dirty && self.draft.valid();
            if aui::save(ui, pal, "Save Changes", can_save).clicked()
                && let Some(req) = save(&cx.guild.id, &self.base, &self.draft)
            {
                out.send.push(req);
            }
            if dirty && aui::plain(ui, "Reset").clicked() {
                self.draft = self.base.clone();
            }
            if dirty {
                ui.label(RichText::new("You have unsaved changes.").color(pal.muted));
            }
        });
    }
}

/// An icon / banner field: what's set now, Upload and Remove.
#[allow(clippy::too_many_arguments)]
fn image_field(
    ui: &mut egui::Ui,
    cx: &Cx,
    title: &str,
    help: &str,
    edit: &mut ImageEdit,
    has_current: bool,
    slot: PickSlot,
    out: &mut Out,
) {
    let pal = cx.pal;
    ui.vertical(|ui| {
        ui.set_max_width(300.0);
        ui.label(pal.section_label(title));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(super::ICON_BOX, egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                crate::theme::avatar_shape().radius(rect.width()),
                pal.input,
            );
            let (icon, col) = match edit {
                ImageEdit::New(_) => (icons::CHECK, pal.positive()),
                ImageEdit::Remove => (icons::PROHIBIT, pal.danger),
                ImageEdit::Keep if has_current => (icons::IMAGE, pal.text),
                ImageEdit::Keep => (icons::IMAGE, pal.muted_text()),
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                icons::font(28.0),
                col,
            );
            ui.vertical(|ui| {
                let status = match edit {
                    ImageEdit::New(p) => format!("{} (uploads when you save)", p.name),
                    ImageEdit::Remove => "Will be removed when you save".to_string(),
                    ImageEdit::Keep if has_current => "An image is set".to_string(),
                    ImageEdit::Keep => "No image".to_string(),
                };
                ui.label(RichText::new(status).color(pal.strong).bold());
                ui.horizontal(|ui| {
                    if aui::icon_text(ui, icons::UPLOAD, "Upload").clicked() {
                        out.pick = Some(slot);
                    }
                    let removable = match edit {
                        ImageEdit::Keep => has_current,
                        ImageEdit::New(_) => true,
                        ImageEdit::Remove => false,
                    };
                    if removable && aui::plain(ui, "Remove").clicked() {
                        *edit = if has_current && matches!(edit, ImageEdit::Keep) {
                            ImageEdit::Remove
                        } else {
                            ImageEdit::Keep
                        };
                    }
                    if matches!(edit, ImageEdit::Remove) && aui::plain(ui, "Undo").clicked() {
                        *edit = ImageEdit::Keep;
                    }
                });
            });
        });
        aui::hint(ui, pal, help);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Draft {
        Draft::from_json(&json!({
            "name": "Test", "description": "Hi", "system_channel_id": "10",
            "afk_channel_id": null, "afk_timeout": 300, "verification_level": 1,
            "default_message_notifications": 1, "explicit_content_filter": 2,
        }))
    }

    #[test]
    fn nothing_changed_means_no_request() {
        let b = base();
        assert!(save("1", &b, &b.clone()).is_none());
    }

    #[test]
    fn patch_holds_only_changed_fields() {
        let b = base();
        let mut d = b.clone();
        d.name = "  Renamed ".into();
        d.system_channel.clear();
        d.verification = 3;
        let r = save("1", &b, &d).unwrap();
        assert_eq!(r.method, Method::Patch);
        assert_eq!(r.path, "/guilds/1");
        assert_eq!(
            r.body,
            Some(json!({ "name": "Renamed", "system_channel_id": null,
                         "verification_level": 3 }))
        );
    }

    #[test]
    fn invalid_name_is_never_sent() {
        let b = base();
        let mut d = b.clone();
        d.name = "x".into();
        assert!(save("1", &b, &d).is_none());
    }

    #[test]
    fn images_patch_as_data_uri_or_null() {
        let b = base();
        let mut d = b.clone();
        d.icon = ImageEdit::Remove;
        d.banner = ImageEdit::New(Arc::new(Picked {
            slot: PickSlot::Banner,
            name: "b.png".into(),
            mime: "image/png".into(),
            bytes: Arc::new(vec![1, 2, 3]),
        }));
        let body = save("1", &b, &d).unwrap().body.unwrap();
        assert_eq!(body["icon"], Value::Null);
        assert_eq!(body["banner"], "data:image/png;base64,AQID");
    }

    #[test]
    fn a_save_reply_resets_the_draft() {
        let mut st = State::default();
        st.on_response(Kind::LoadGuild, "", &json!({ "name": "Test" }));
        st.draft.name = "New".into();
        st.on_response(Kind::SaveGuild, "", &json!({ "name": "New" }));
        assert!(st.draft == st.base);
        assert_eq!(st.base.name, "New");
    }
}
