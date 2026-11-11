//! Server settings → Emoji and Stickers: upload, rename and delete.
//! Uploads are picked and read by the backend (`PickSlot`), named here,
//! and only sent by the "Add" button.

use std::sync::Arc;

use eframe::egui::{self, RichText, vec2};
use serde_json::{Value, json};

use super::{
    Confirm, Cx, Kind, Method, Out, PickSlot, Picked, Request, Upload, b, n, s, ui as aui,
};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

/// Discord's upload limits.
pub const EMOJI_MAX_BYTES: usize = 256 * 1024;
pub const STICKER_MAX_BYTES: usize = 512 * 1024;

/// Create Expressions (`1 << 43`): upload, and manage my own uploads.
const CREATE_EXPRESSIONS: u64 = 1 << 43;

/// 2–32 letters, digits or underscores.
pub fn valid_emoji_name(name: &str) -> bool {
    (2..=32).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A starting name from a file name: "Party Cat.png" → "Party_Cat".
pub fn name_from_file(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(a, _)| a);
    let mut name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(32)
        .collect();
    while name.chars().count() < 2 {
        name.push('_');
    }
    name
}

pub fn load_emojis(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadEmojis,
        Method::Get,
        format!("/guilds/{guild_id}/emojis"),
    )
}

pub fn load_stickers(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadStickers,
        Method::Get,
        format!("/guilds/{guild_id}/stickers"),
    )
}

pub fn create_emoji(guild_id: &str, name: &str, image: &Picked) -> Option<Request> {
    (valid_emoji_name(name) && image.bytes.len() <= EMOJI_MAX_BYTES).then(|| {
        Request::new(
            guild_id,
            Kind::CreateEmoji,
            Method::Post,
            format!("/guilds/{guild_id}/emojis"),
        )
        .body(json!({ "name": name, "image": image.data_uri() }))
    })
}

pub fn rename_emoji(guild_id: &str, id: &str, name: &str) -> Option<Request> {
    valid_emoji_name(name).then(|| {
        Request::new(
            guild_id,
            Kind::RenameEmoji,
            Method::Patch,
            format!("/guilds/{guild_id}/emojis/{id}"),
        )
        .key(id)
        .body(json!({ "name": name }))
    })
}

pub fn delete_emoji(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteEmoji,
        Method::Delete,
        format!("/guilds/{guild_id}/emojis/{id}"),
    )
    .key(id)
}

/// A sticker's text fields.
#[derive(Clone, PartialEq, Default)]
pub struct StickerDraft {
    pub name: String,
    pub description: String,
    /// The related emoji (Discord's `tags`).
    pub tags: String,
}

impl StickerDraft {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: s(v, "name").to_string(),
            description: s(v, "description").to_string(),
            tags: s(v, "tags").to_string(),
        }
    }

    pub fn valid(&self) -> bool {
        let desc = self.description.trim().chars().count();
        (2..=30).contains(&self.name.trim().chars().count())
            && (desc == 0 || (2..=100).contains(&desc))
            && (1..=200).contains(&self.tags.trim().chars().count())
    }

    fn body(&self) -> Value {
        json!({ "name": self.name.trim(), "description": self.description.trim(),
                "tags": self.tags.trim() })
    }
}

/// Upload a sticker (multipart: the three text fields plus the file).
pub fn create_sticker(guild_id: &str, d: &StickerDraft, file: &Picked) -> Option<Request> {
    if !d.valid() || file.bytes.len() > STICKER_MAX_BYTES {
        return None;
    }
    let mut r = Request::new(
        guild_id,
        Kind::CreateSticker,
        Method::Post,
        format!("/guilds/{guild_id}/stickers"),
    )
    .body(d.body());
    r.file = Some(Upload {
        name: file.name.clone(),
        mime: file.mime.clone(),
        bytes: file.bytes.clone(),
    });
    Some(r)
}

pub fn save_sticker(
    guild_id: &str,
    id: &str,
    base: &StickerDraft,
    d: &StickerDraft,
) -> Option<Request> {
    (d.valid() && d != base).then(|| {
        Request::new(
            guild_id,
            Kind::SaveSticker,
            Method::Patch,
            format!("/guilds/{guild_id}/stickers/{id}"),
        )
        .key(id)
        .body(d.body())
    })
}

pub fn delete_sticker(guild_id: &str, id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteSticker,
        Method::Delete,
        format!("/guilds/{guild_id}/stickers/{id}"),
    )
    .key(id)
}

fn sticker_url(v: &Value) -> Option<String> {
    let id = s(v, "id");
    match n(v, "format_type") {
        // Lottie stickers are JSON animations, not pictures.
        3 => None,
        4 => Some(format!(
            "https://media.discordapp.net/stickers/{id}.gif?size=160"
        )),
        _ => Some(format!(
            "https://media.discordapp.net/stickers/{id}.png?size=160"
        )),
    }
}

#[derive(Default)]
pub struct State {
    emojis: Option<Vec<Value>>,
    stickers: Option<Vec<Value>>,
    new_emoji: Option<(Arc<Picked>, String)>,
    rename: Option<(String, String)>,
    new_sticker: Option<(Arc<Picked>, StickerDraft)>,
    edit_sticker: Option<(String, StickerDraft)>,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadEmojis => self.emojis = Some(super::list(v)),
            Kind::LoadStickers => self.stickers = Some(super::list(v)),
            Kind::CreateEmoji | Kind::RenameEmoji => {
                if let Some(list) = &mut self.emojis {
                    let mut item = v.clone();
                    // The reply echoes the image in the demo: not kept.
                    if let Some(o) = item.as_object_mut() {
                        o.remove("image");
                    }
                    super::upsert(list, &item);
                }
                if kind == Kind::CreateEmoji {
                    self.new_emoji = None;
                } else {
                    self.rename = None;
                }
            }
            Kind::DeleteEmoji => {
                if let Some(list) = &mut self.emojis {
                    super::remove(list, key);
                }
            }
            Kind::CreateSticker | Kind::SaveSticker => {
                if let Some(list) = &mut self.stickers {
                    super::upsert(list, v);
                }
                if kind == Kind::CreateSticker {
                    self.new_sticker = None;
                } else {
                    self.edit_sticker = None;
                }
            }
            Kind::DeleteSticker => {
                if let Some(list) = &mut self.stickers {
                    super::remove(list, key);
                }
            }
            _ => {}
        }
    }

    pub fn on_picked(&mut self, p: Arc<Picked>) {
        match p.slot {
            PickSlot::Emoji => {
                let name = name_from_file(&p.name);
                self.new_emoji = Some((p, name));
            }
            PickSlot::Sticker => {
                let name: String = name_from_file(&p.name).chars().take(30).collect();
                self.new_sticker = Some((
                    p,
                    StickerDraft {
                        name,
                        ..Default::default()
                    },
                ));
            }
            _ => {}
        }
    }

    /// Image URIs this page may have loaded (freed when it closes).
    pub fn image_uris(&self) -> Vec<String> {
        let emojis = self
            .emojis
            .iter()
            .flatten()
            .map(|e| crate::emoji::custom_emoji_url(s(e, "id"), b(e, "animated")));
        let stickers = self.stickers.iter().flatten().filter_map(sticker_url);
        emojis.chain(stickers).collect()
    }

    pub fn emoji_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let can_manage = cx.can(perms::MANAGE_EXPRESSIONS);
        let can_create = can_manage || cx.can(CREATE_EXPRESSIONS);
        if can_create {
            aui::card(ui, pal, |ui| match &mut self.new_emoji {
                None => {
                    ui.horizontal(|ui| {
                        if aui::primary(ui, pal, "Upload Emoji", true).clicked() {
                            out.pick = Some(PickSlot::Emoji);
                        }
                        aui::hint(ui, pal, "PNG, JPG, GIF or WebP, 256 KB at most.");
                    });
                }
                Some((file, name)) => {
                    let too_big = file.bytes.len() > EMOJI_MAX_BYTES;
                    ui.label(RichText::new(&file.name).color(pal.strong).bold());
                    if too_big {
                        ui.label(
                            RichText::new("This file is over 256 KB. Pick a smaller one.")
                                .color(pal.danger),
                        );
                    }
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(":").color(pal.muted));
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .desired_width(200.0)
                                .char_limit(32),
                        );
                        ui.label(RichText::new(":").color(pal.muted));
                        let ok = valid_emoji_name(name) && !too_big;
                        if aui::save(ui, pal, "Add Emoji", ok).clicked() {
                            out.send.extend(create_emoji(&cx.guild.id, name, file));
                        }
                        if aui::plain(ui, "Cancel").clicked() {
                            cancel = true;
                        }
                    });
                    if !valid_emoji_name(name) {
                        aui::hint(ui, pal, "2 to 32 letters, numbers or underscores.");
                    }
                    if cancel {
                        self.new_emoji = None;
                    }
                }
            });
            ui.add_space(10.0);
        }
        let Some(emojis) = &self.emojis else {
            aui::loading(ui, pal);
            return;
        };
        if emojis.is_empty() {
            aui::empty(
                ui,
                pal,
                icons::SMILEY,
                "This server has no custom emoji yet.",
            );
            return;
        }
        aui::label(ui, pal, &format!("Emoji ({})", emojis.len()));
        let mut start_rename = None;
        for e in emojis {
            let id = s(e, "id");
            let mine = s(&e["user"], "id") == cx.me_id;
            let editable = can_manage || (mine && cx.can(CREATE_EXPRESSIONS));
            aui::row(ui, pal, false, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::hover());
                if cx.images {
                    egui::Image::from_uri(crate::emoji::custom_emoji_url(id, b(e, "animated")))
                        .paint_at(ui, rect);
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icons::SMILEY,
                        icons::font(20.0),
                        pal.muted,
                    );
                }
                match &mut self.rename {
                    Some((rid, name)) if rid == id => {
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .desired_width(180.0)
                                .char_limit(32),
                        );
                        if aui::save(
                            ui,
                            pal,
                            "Save",
                            valid_emoji_name(name) && name != s(e, "name"),
                        )
                        .clicked()
                        {
                            out.send.extend(rename_emoji(&cx.guild.id, id, name));
                        }
                        if aui::plain(ui, "Cancel").clicked() {
                            start_rename = Some(None);
                        }
                    }
                    _ => {
                        ui.label(
                            RichText::new(format!(":{}:", s(e, "name")))
                                .color(pal.strong)
                                .bold(),
                        );
                        if b(e, "animated") {
                            ui.label(RichText::new("animated").size(12.0).color(pal.muted));
                        }
                        let by = s(&e["user"], "username");
                        if !by.is_empty() {
                            ui.label(
                                RichText::new(format!("by @{by}"))
                                    .size(12.0)
                                    .color(pal.muted),
                            );
                        }
                        if editable {
                            aui::right(ui, |ui| {
                                if crate::widgets::icon_button(
                                    ui,
                                    icons::TRASH,
                                    "Delete emoji",
                                    false,
                                    true,
                                    pal,
                                )
                                .clicked()
                                {
                                    out.confirm = Some(Confirm::new(
                                        "Delete emoji",
                                        &format!(
                                            "Delete :{}:? Messages that used it show its name instead.",
                                            s(e, "name")
                                        ),
                                        "Delete",
                                        delete_emoji(&cx.guild.id, id),
                                    ));
                                }
                                if crate::widgets::icon_button(
                                    ui,
                                    icons::PENCIL,
                                    "Rename",
                                    false,
                                    true,
                                    pal,
                                )
                                .clicked()
                                {
                                    start_rename =
                                        Some(Some((id.to_string(), s(e, "name").to_string())));
                                }
                            });
                        }
                    }
                }
            });
        }
        if let Some(r) = start_rename {
            self.rename = r;
        }
    }

    pub fn sticker_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let can_manage = cx.can(perms::MANAGE_EXPRESSIONS);
        let can_create = can_manage || cx.can(CREATE_EXPRESSIONS);
        if can_create {
            aui::card(ui, pal, |ui| match &mut self.new_sticker {
                None => {
                    ui.horizontal(|ui| {
                        if aui::primary(ui, pal, "Upload Sticker", true).clicked() {
                            out.pick = Some(PickSlot::Sticker);
                        }
                        aui::hint(ui, pal, "PNG, APNG or GIF, 320×320, 512 KB at most.");
                    });
                }
                Some((file, draft)) => {
                    let too_big = file.bytes.len() > STICKER_MAX_BYTES;
                    ui.label(RichText::new(&file.name).color(pal.strong).bold());
                    if too_big {
                        ui.label(
                            RichText::new("This file is over 512 KB. Pick a smaller one.")
                                .color(pal.danger),
                        );
                    }
                    sticker_fields(ui, cx, draft);
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        if aui::save(ui, pal, "Add Sticker", draft.valid() && !too_big).clicked() {
                            out.send.extend(create_sticker(&cx.guild.id, draft, file));
                        }
                        if aui::plain(ui, "Cancel").clicked() {
                            cancel = true;
                        }
                    });
                    if cancel {
                        self.new_sticker = None;
                    }
                }
            });
            ui.add_space(10.0);
        }
        let Some(stickers) = &self.stickers else {
            aui::loading(ui, pal);
            return;
        };
        if stickers.is_empty() {
            aui::empty(ui, pal, icons::STICKER, "This server has no stickers yet.");
            return;
        }
        aui::label(ui, pal, &format!("Stickers ({})", stickers.len()));
        let mut set_edit = None;
        for st in stickers {
            let id = s(st, "id");
            let mine = s(&st["user"], "id") == cx.me_id;
            let editable = can_manage || (mine && cx.can(CREATE_EXPRESSIONS));
            let editing = self.edit_sticker.as_ref().is_some_and(|(e, _)| e == id);
            aui::row(ui, pal, editing, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
                match sticker_url(st).filter(|_| cx.images) {
                    Some(url) => egui::Image::from_uri(url).paint_at(ui, rect),
                    None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            icons::STICKER,
                            icons::font(28.0),
                            pal.muted,
                        );
                    }
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new(s(st, "name")).color(pal.strong).bold());
                    let desc = s(st, "description");
                    let tags = s(st, "tags");
                    ui.label(
                        RichText::new(if desc.is_empty() {
                            format!("Related emoji: {tags}")
                        } else {
                            format!("{desc} · related emoji: {tags}")
                        })
                        .size(13.0)
                        .color(pal.muted),
                    );
                });
                if editable && !editing {
                    aui::right(ui, |ui| {
                        if crate::widgets::icon_button(
                            ui,
                            icons::TRASH,
                            "Delete sticker",
                            false,
                            true,
                            pal,
                        )
                        .clicked()
                        {
                            out.confirm = Some(Confirm::new(
                                "Delete sticker",
                                &format!("Delete the sticker \"{}\"?", s(st, "name")),
                                "Delete",
                                delete_sticker(&cx.guild.id, id),
                            ));
                        }
                        if crate::widgets::icon_button(ui, icons::PENCIL, "Edit", false, true, pal)
                            .clicked()
                        {
                            set_edit = Some(Some((id.to_string(), StickerDraft::from_json(st))));
                        }
                    });
                }
            });
            if editing && let Some((_, draft)) = &mut self.edit_sticker {
                let base = StickerDraft::from_json(st);
                aui::card(ui, pal, |ui| {
                    sticker_fields(ui, cx, draft);
                    ui.horizontal(|ui| {
                        let req = save_sticker(&cx.guild.id, id, &base, draft);
                        if aui::save(ui, pal, "Save Changes", req.is_some()).clicked() {
                            out.send.extend(req);
                        }
                        if aui::plain(ui, "Cancel").clicked() {
                            set_edit = Some(None);
                        }
                    });
                });
            }
        }
        if let Some(e) = set_edit {
            self.edit_sticker = e;
        }
    }
}

fn sticker_fields(ui: &mut egui::Ui, cx: &Cx, d: &mut StickerDraft) {
    let pal = cx.pal;
    aui::label(ui, pal, "Sticker name");
    aui::text(ui, &mut d.name, "2 to 30 characters", 30);
    aui::label(ui, pal, "Related emoji");
    aui::text(ui, &mut d.tags, "An emoji name, e.g. wave", 200);
    aui::label(ui, pal, "Description");
    aui::text(
        ui,
        &mut d.description,
        "Optional; helps people using screen readers",
        100,
    );
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pic(len: usize) -> Picked {
        Picked {
            slot: PickSlot::Emoji,
            name: "Party Cat!.png".into(),
            mime: "image/png".into(),
            bytes: Arc::new(vec![0; len]),
        }
    }

    #[test]
    fn emoji_names() {
        assert!(valid_emoji_name("party_cat2"));
        assert!(!valid_emoji_name("a"));
        assert!(!valid_emoji_name("party cat"));
        assert!(!valid_emoji_name(&"x".repeat(33)));
        assert_eq!(name_from_file("Party Cat!.png"), "Party_Cat_");
        assert!(valid_emoji_name(&name_from_file("a.png")));
    }

    #[test]
    fn emoji_requests() {
        let c = create_emoji("1", "cat", &pic(3)).unwrap();
        assert_eq!(
            (c.method, c.path.as_str()),
            (Method::Post, "/guilds/1/emojis")
        );
        assert_eq!(c.body.unwrap()["image"], "data:image/png;base64,AAAA");
        assert!(create_emoji("1", "bad name", &pic(3)).is_none());
        assert!(create_emoji("1", "cat", &pic(EMOJI_MAX_BYTES + 1)).is_none());
        let r = rename_emoji("1", "9", "dog").unwrap();
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Patch, "/guilds/1/emojis/9")
        );
        assert!(rename_emoji("1", "9", "!").is_none());
        let d = delete_emoji("1", "9");
        assert_eq!((d.method, d.key.as_str()), (Method::Delete, "9"));
    }

    #[test]
    fn sticker_requests() {
        let mut d = StickerDraft {
            name: "Wave".into(),
            description: String::new(),
            tags: "wave".into(),
        };
        let c = create_sticker("1", &d, &pic(10)).unwrap();
        assert!(c.file.is_some());
        assert_eq!(
            c.body,
            Some(json!({ "name": "Wave", "description": "", "tags": "wave" }))
        );
        assert!(create_sticker("1", &d, &pic(STICKER_MAX_BYTES + 1)).is_none());
        let base = d.clone();
        assert!(save_sticker("1", "5", &base, &d).is_none());
        d.description = "A friendly wave".into();
        let p = save_sticker("1", "5", &base, &d).unwrap();
        assert_eq!(p.path, "/guilds/1/stickers/5");
        d.tags.clear();
        assert!(save_sticker("1", "5", &base, &d).is_none());
    }

    #[test]
    fn replies_update_lists_and_drop_image_data() {
        let mut st = State::default();
        st.on_response(Kind::LoadEmojis, "", &json!([{ "id": "1", "name": "a" }]));
        st.on_response(
            Kind::CreateEmoji,
            "",
            &json!({ "id": "2", "name": "b", "image": "data:..." }),
        );
        let list = st.emojis.as_ref().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[1].get("image").is_none());
        st.on_response(Kind::DeleteEmoji, "1", &Value::Null);
        assert_eq!(st.emojis.as_ref().unwrap().len(), 1);
        assert_eq!(st.image_uris().len(), 1);
    }
}
