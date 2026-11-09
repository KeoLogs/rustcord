//! Server Settings → Emoji: list (static / animated), upload, rename,
//! delete, and the slot counts by boost tier.
//!
//! Upload: "Upload Emoji" sends `AdminAction::PickEmojiFile` (native picker
//! in the backend, size checked before reading); the bytes come back as
//! `AdminOutcome::File`, are checked here ([`prepare_upload`]: type by magic
//! bytes, 256 KB) and turned into a `data:image/…;base64,` URI; the user
//! names it and presses Upload → `POST /guilds/{g}/emojis { name, image }`.
//!
//! Rules (Discord's docs and client): names 2-32 characters of letters,
//! digits and underscores; at most 256 KB; PNG / JPEG / GIF / WEBP / AVIF.
//! Slots per kind (static and animated counted separately): 50 / 100 / 150
//! / 250 for boost tiers 0-3. Servers with the MORE_EMOJI feature get more
//! (not handled; Discord will just refuse when full).
//!
//! Permissions: Create Expressions lets you upload and edit / delete your
//! own; Manage Expressions anyone's. The uploader (`user`) is only sent to
//! members with one of those.

use base64::Engine;
use eframe::egui::{self, RichText, Sense, vec2};
use serde_json::Value;

use crate::guild_admin::{self as ga, AdminAction, AdminCtx, AdminOutcome, Icon, ImageUris, Load};
use crate::types::User;

pub const MAX_EMOJI_BYTES: usize = 256 * 1024;
pub const MIN_NAME: usize = 2;
pub const MAX_NAME: usize = 32;

pub fn too_big_message(len: usize) -> String {
    format!(
        "That file is {} KB. Emoji must be 256 KB or smaller.",
        len.div_ceil(1024)
    )
}

/// Emoji slots of each kind (static, animated) for a boost tier.
pub fn slots(tier: u8) -> usize {
    match tier {
        0 => 50,
        1 => 100,
        2 => 150,
        _ => 250,
    }
}

/// Why a name isn't allowed, if it isn't.
pub fn name_problem(name: &str) -> Option<&'static str> {
    let n = name.chars().count();
    if n < MIN_NAME {
        Some("Emoji names need at least 2 characters.")
    } else if n > MAX_NAME {
        Some("Emoji names can be at most 32 characters.")
    } else if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some("Emoji names can only use letters, numbers and underscores.")
    } else {
        None
    }
}

/// A valid name from a file name ("Party Parrot!.gif" → "Party_Parrot_").
pub fn name_from_file(file_name: &str) -> String {
    let stem = match file_name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => file_name,
    };
    let mut name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME)
        .collect();
    while name.chars().count() < MIN_NAME {
        name.push('_');
    }
    name
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Avif,
}

impl ImageKind {
    pub fn detect(b: &[u8]) -> Option<ImageKind> {
        if b.starts_with(&[0x89, b'P', b'N', b'G']) {
            Some(ImageKind::Png)
        } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(ImageKind::Jpeg)
        } else if b.starts_with(b"GIF8") {
            Some(ImageKind::Gif)
        } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
            Some(ImageKind::Webp)
        } else if b.len() >= 12
            && &b[4..8] == b"ftyp"
            && (&b[8..12] == b"avif" || &b[8..12] == b"avis")
        {
            Some(ImageKind::Avif)
        } else {
            None
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageKind::Png => "image/png",
            ImageKind::Jpeg => "image/jpeg",
            ImageKind::Gif => "image/gif",
            ImageKind::Webp => "image/webp",
            ImageKind::Avif => "image/avif",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            ImageKind::Png => "png",
            ImageKind::Jpeg => "jpg",
            ImageKind::Gif => "gif",
            ImageKind::Webp => "webp",
            ImageKind::Avif => "avif",
        }
    }
}

/// Will Discord store this as an animated emoji? GIFs always; WEBP with
/// the VP8X animation flag; AVIF image sequences.
pub fn is_animated(kind: ImageKind, b: &[u8]) -> bool {
    match kind {
        ImageKind::Gif => true,
        ImageKind::Webp => b.len() > 20 && &b[12..16] == b"VP8X" && b[20] & 0x02 != 0,
        ImageKind::Avif => b.len() >= 12 && &b[8..12] == b"avis",
        _ => false,
    }
}

/// A checked file waiting for a name.
#[derive(Debug, Clone, PartialEq)]
pub struct EmojiUpload {
    pub name: String,
    pub kind: ImageKind,
    pub animated: bool,
    pub size: usize,
    /// `data:image/png;base64,…`
    pub data_uri: String,
    /// For the preview (`bytes://…`).
    pub preview_uri: String,
    pub bytes: std::sync::Arc<[u8]>,
}

/// Check a picked file and build the upload.
pub fn prepare_upload(file_name: &str, bytes: &[u8]) -> Result<EmojiUpload, String> {
    if bytes.len() > MAX_EMOJI_BYTES {
        return Err(too_big_message(bytes.len()));
    }
    let kind = ImageKind::detect(bytes)
        .ok_or_else(|| "That isn't a PNG, JPEG, GIF, WEBP or AVIF image.".to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let name = name_from_file(file_name);
    Ok(EmojiUpload {
        preview_uri: format!("bytes://rc-emoji-upload/{name}.{}", kind.ext()),
        name,
        kind,
        animated: is_animated(kind, bytes),
        size: bytes.len(),
        data_uri: format!("data:{};base64,{b64}", kind.mime()),
        bytes: bytes.into(),
    })
}

/// A server emoji as the page shows it.
#[derive(Debug, Clone)]
pub struct EmojiRow {
    pub id: String,
    pub name: String,
    pub animated: bool,
    /// False when the server lost the boost tier it needs.
    pub available: bool,
    pub uploader: Option<User>,
}

impl EmojiRow {
    pub fn from_json(v: &Value) -> Self {
        let user = &v["user"];
        Self {
            id: v["id"].as_str().unwrap_or_default().to_string(),
            name: v["name"].as_str().unwrap_or_default().to_string(),
            animated: v["animated"].as_bool().unwrap_or(false),
            available: v["available"].as_bool().unwrap_or(true),
            uploader: user.is_object().then(|| User::from_json(user)),
        }
    }

    pub fn parse_list(v: &Value) -> Vec<EmojiRow> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(EmojiRow::from_json)
                    .filter(|e| !e.id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Same URL as the chat uses, so the texture is shared.
    pub fn url(&self) -> String {
        let ext = if self.animated { "gif" } else { "png" };
        format!(
            "https://cdn.discordapp.com/emojis/{}.{ext}?size=48",
            self.id
        )
    }
}

/// (static, animated) emoji in use.
pub fn used(list: &[EmojiRow]) -> (usize, usize) {
    let animated = list.iter().filter(|e| e.animated).count();
    (list.len() - animated, animated)
}

/// Server Settings → Emoji.
#[derive(Default)]
pub struct EmojiPage {
    list: Load<Vec<EmojiRow>>,
    upload: Option<EmojiUpload>,
    uploading: bool,
    renaming: Option<(String, String)>,
    confirm_delete: Option<(String, String)>,
    error: Option<String>,
    uris: ImageUris,
}

impl EmojiPage {
    pub fn forget_images(&mut self, ctx: &egui::Context) {
        if let Some(u) = self.upload.take() {
            ctx.forget_image(&u.preview_uri);
        }
        self.uris.forget(ctx);
    }

    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        use AdminAction::*;
        match (action, result) {
            (LoadEmojis { .. }, Ok(AdminOutcome::Json(v))) => {
                self.list = Load::Loaded(EmojiRow::parse_list(v));
            }
            (LoadEmojis { .. }, Err(e)) => self.list = Load::Failed(e.clone()),
            (PickEmojiFile, Ok(AdminOutcome::File { name, bytes })) => {
                match prepare_upload(name, bytes) {
                    Ok(u) => {
                        self.error = None;
                        self.upload = Some(u);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            (CreateEmoji { .. }, Ok(AdminOutcome::Json(v))) => {
                self.uploading = false;
                // The preview texture stays until the page closes
                // (`forget_images`); the bytes go now.
                self.upload = None;
                self.error = None;
                let row = EmojiRow::from_json(v);
                if let (Load::Loaded(list), false) = (&mut self.list, row.id.is_empty()) {
                    list.push(row);
                }
            }
            (CreateEmoji { .. }, Err(e)) => {
                self.uploading = false;
                self.error = Some(e.clone());
            }
            (RenameEmoji { emoji_id, name, .. }, Ok(_)) => {
                self.error = None;
                if let Load::Loaded(list) = &mut self.list {
                    for e in list.iter_mut().filter(|e| &e.id == emoji_id) {
                        e.name = name.clone();
                    }
                }
            }
            (DeleteEmoji { emoji_id, .. }, Ok(_)) => {
                self.error = None;
                if let Load::Loaded(list) = &mut self.list {
                    list.retain(|e| &e.id != emoji_id);
                }
            }
            (PickEmojiFile | RenameEmoji { .. } | DeleteEmoji { .. }, Err(e)) => {
                self.error = Some(e.clone());
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        if self.list.is_idle() {
            self.list = Load::Loading;
            return Some(AdminAction::LoadEmojis { guild_id: gid });
        }
        let mut action = None;
        let tier = ctx.premium_tier.unwrap_or(0);
        let max = slots(tier);
        let list = self.list.get().cloned().unwrap_or_default();
        let (st, an) = used(&list);

        ui.heading("Emoji");
        ui.label(
            RichText::new(format!(
                "Add up to {max} custom emoji that anyone can use in this server. \
                 Animated GIF emoji may be used by members with Discord Nitro."
            ))
            .color(pal.muted),
        );
        ui.add_space(8.0);
        ui.label(pal.section_label("Upload requirements"));
        for line in [
            "File type: JPEG, PNG, GIF, WEBP, AVIF",
            "Recommended file size: 256 KB (we'll compress for you)",
            "Recommended dimensions: 128x128",
            "Naming: emoji names must be at least 2 characters long and can only contain alphanumeric characters and underscores",
        ] {
            ui.label(RichText::new(format!("•  {line}")).small().color(pal.muted));
        }
        ui.add_space(8.0);
        let can_upload = ctx.actor.can_upload_emoji();
        ui.horizontal(|ui| {
            let full = st >= max && an >= max;
            if ui
                .add_enabled(
                    can_upload && !full && self.upload.is_none(),
                    pal.primary_button("Upload Emoji"),
                )
                .clicked()
            {
                action = Some(AdminAction::PickEmojiFile);
            }
            ui.label(
                RichText::new(format!(
                    "Static: {} of {max} slots free · Animated: {} of {max} slots free",
                    max.saturating_sub(st),
                    max.saturating_sub(an)
                ))
                .small()
                .color(pal.muted),
            );
        });
        if let Some(e) = &self.error {
            ui.add_space(4.0);
            ga::error_line(ui, pal, e);
        }

        // Waiting upload.
        if let Some(u) = &mut self.upload {
            ui.add_space(8.0);
            let mut cancel = false;
            egui::Frame::new()
                .fill(pal.sidebar)
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if ctx.show_images {
                            ui.add(
                                egui::Image::from_bytes(u.preview_uri.clone(), u.bytes.to_vec())
                                    .fit_to_exact_size(vec2(48.0, 48.0)),
                            );
                        }
                        ui.vertical(|ui| {
                            ui.label(pal.section_label("Name"));
                            ui.add(
                                egui::TextEdit::singleline(&mut u.name)
                                    .char_limit(MAX_NAME)
                                    .desired_width(220.0),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} KB{}",
                                    u.kind.mime(),
                                    u.size.div_ceil(1024),
                                    if u.animated { " · animated" } else { "" }
                                ))
                                .small()
                                .color(pal.muted),
                            );
                        });
                    });
                    let full = if u.animated { an >= max } else { st >= max };
                    let problem = name_problem(&u.name).or(full.then_some(if u.animated {
                        "No animated emoji slots left."
                    } else {
                        "No static emoji slots left."
                    }));
                    if let Some(p) = problem {
                        ui.label(RichText::new(p).small().color(pal.danger));
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                problem.is_none() && !self.uploading,
                                pal.primary_button("Upload"),
                            )
                            .clicked()
                        {
                            self.uploading = true;
                            action = Some(AdminAction::CreateEmoji {
                                guild_id: gid.clone(),
                                name: u.name.clone(),
                                image: u.data_uri.clone(),
                            });
                        }
                        if self.uploading {
                            ui.spinner();
                        } else if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if cancel && let Some(u) = self.upload.take() {
                ui.ctx().forget_image(&u.preview_uri);
            }
        }

        ui.add_space(10.0);
        match &self.list {
            Load::Loading | Load::Idle => {
                ui.spinner();
                return action;
            }
            Load::Failed(e) => {
                ga::error_line(ui, pal, e);
                if ui.button("Try again").clicked() {
                    self.list = Load::Idle;
                }
                return action;
            }
            Load::Loaded(_) => {}
        }
        egui::ScrollArea::vertical()
            .id_salt("rc_admin_emoji")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for animated in [false, true] {
                    let rows: Vec<&EmojiRow> =
                        list.iter().filter(|e| e.animated == animated).collect();
                    let n = if animated { an } else { st };
                    ui.add_space(8.0);
                    ui.label(pal.section_label(&format!(
                        "{} emoji — {} slots available",
                        if animated { "Animated" } else { "Static" },
                        max.saturating_sub(n)
                    )));
                    if rows.is_empty() {
                        ui.label(RichText::new("None yet.").small().color(pal.muted));
                    }
                    for e in rows {
                        if let Some(a) = self.emoji_row(ui, ctx, e) {
                            action = Some(a);
                        }
                    }
                }
            });

        if let Some((id, name)) = self.confirm_delete.clone() {
            match ga::confirm(
                ui.ctx(),
                pal,
                "delete_emoji",
                "Delete Emoji",
                &format!("Are you sure you want to delete :{name}:? This cannot be undone."),
                "Delete",
            ) {
                Some(true) => {
                    self.confirm_delete = None;
                    action = Some(AdminAction::DeleteEmoji {
                        guild_id: gid,
                        emoji_id: id,
                    });
                }
                Some(false) => self.confirm_delete = None,
                None => {}
            }
        }
        action
    }

    fn emoji_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &AdminCtx,
        e: &EmojiRow,
    ) -> Option<AdminAction> {
        let pal = ctx.pal;
        let mut action = None;
        let can = ctx
            .actor
            .can_manage_emoji(e.uploader.as_ref().map(|u| u.id.as_str()));
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4, pal.hover);
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(8.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        let (img, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        if ctx.show_images {
            let url = e.url();
            self.uris.note(&url);
            egui::Image::from_uri(url)
                .show_loading_spinner(false)
                .paint_at(ui, img);
        }
        ui.add_space(8.0);
        let editing = self.renaming.as_ref().is_some_and(|(id, _)| id == &e.id);
        if editing {
            let (_, text) = self.renaming.as_mut().expect("checked");
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .char_limit(MAX_NAME)
                    .desired_width(200.0),
            );
            r.request_focus();
            let problem = name_problem(text);
            if let Some(p) = problem {
                ui.label(RichText::new(p).small().color(pal.danger));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.renaming = None;
            } else if r.lost_focus() {
                let name = text.clone();
                self.renaming = None;
                if problem.is_none() && name != e.name {
                    action = Some(AdminAction::RenameEmoji {
                        guild_id: ctx.guild_id().to_string(),
                        emoji_id: e.id.clone(),
                        name,
                    });
                }
            }
        } else {
            let label = ui.add(
                egui::Label::new(RichText::new(format!(":{}:", e.name)).color(pal.text))
                    .sense(if can { Sense::click() } else { Sense::hover() }),
            );
            if can && label.on_hover_text("Click to rename").clicked() {
                self.renaming = Some((e.id.clone(), e.name.clone()));
            }
            if !e.available {
                ui.label(RichText::new("unavailable").small().color(pal.idle));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if can && ga::icon_button(ui, Icon::Close, "Delete emoji", pal, true).clicked() {
                self.confirm_delete = Some((e.id.clone(), e.name.clone()));
            }
            if let Some(u) = &e.uploader {
                ui.label(RichText::new(&u.name).small().color(pal.muted));
                ga::avatar(ui, 20.0, &u.id, &u.name, &u.avatar_url, ctx, &mut self.uris);
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names() {
        assert_eq!(name_problem("ok"), None);
        assert_eq!(name_problem("Party_Parrot_2"), None);
        assert!(name_problem("a").is_some());
        assert!(name_problem(&"a".repeat(33)).is_some());
        assert!(name_problem("has space").is_some());
        assert!(name_problem("emoji-dash").is_some());
        assert!(name_problem("héllo").is_some());
        assert_eq!(name_from_file("Party Parrot!.gif"), "Party_Parrot_");
        assert_eq!(name_from_file("a.png"), "a_");
        assert_eq!(name_from_file(".png"), "_png");
        assert_eq!(name_from_file(&format!("{}.png", "x".repeat(40))).len(), 32);
        assert_eq!(name_problem(&name_from_file("weird name ü.webp")), None);
    }

    #[test]
    fn files() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];
        let u = prepare_upload("cat.png", &png).unwrap();
        assert_eq!(u.kind, ImageKind::Png);
        assert!(!u.animated);
        assert_eq!(u.name, "cat");
        assert!(u.data_uri.starts_with("data:image/png;base64,iVBORw0KGgo"));

        let gif = b"GIF89a....";
        let u = prepare_upload("dance.gif", gif).unwrap();
        assert!(u.animated);
        assert!(u.data_uri.starts_with("data:image/gif;base64,"));

        // Animated WEBP: VP8X chunk with the animation flag.
        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF\0\0\0\0WEBPVP8X");
        webp.extend_from_slice(&[0, 0, 0, 0, 0x02, 0, 0, 0]);
        assert_eq!(ImageKind::detect(&webp), Some(ImageKind::Webp));
        assert!(is_animated(ImageKind::Webp, &webp));
        webp[20] = 0;
        assert!(!is_animated(ImageKind::Webp, &webp));

        assert_eq!(
            ImageKind::detect(b"\0\0\0\x1cftypavif"),
            Some(ImageKind::Avif)
        );
        assert!(prepare_upload("x.txt", b"hello world").is_err());

        // 256 KB exactly is fine, one byte more isn't.
        let mut big = png.to_vec();
        big.resize(MAX_EMOJI_BYTES, 0);
        assert!(prepare_upload("big.png", &big).is_ok());
        big.push(0);
        let err = prepare_upload("big.png", &big).unwrap_err();
        assert_eq!(err, "That file is 257 KB. Emoji must be 256 KB or smaller.");
    }

    #[test]
    fn slots_and_list() {
        assert_eq!(
            [slots(0), slots(1), slots(2), slots(3)],
            [50, 100, 150, 250]
        );
        let v = json!([
            { "id": "1", "name": "pog", "animated": false, "user": { "id": "9", "username": "leo" } },
            { "id": "2", "name": "dance", "animated": true, "available": false },
            { "name": "no id" }
        ]);
        let list = EmojiRow::parse_list(&v);
        assert_eq!(list.len(), 2);
        assert_eq!(used(&list), (1, 1));
        assert_eq!(list[0].uploader.as_ref().unwrap().id, "9");
        assert!(!list[1].available);
        assert_eq!(
            list[1].url(),
            "https://cdn.discordapp.com/emojis/2.gif?size=48"
        );

        let mut page = EmojiPage::default();
        let g = "g".to_string();
        page.on_result(
            &AdminAction::LoadEmojis {
                guild_id: g.clone(),
            },
            &Ok(AdminOutcome::Json(v)),
        );
        page.on_result(
            &AdminAction::RenameEmoji {
                guild_id: g.clone(),
                emoji_id: "1".into(),
                name: "poggers".into(),
            },
            &Ok(AdminOutcome::Json(Value::Null)),
        );
        page.on_result(
            &AdminAction::DeleteEmoji {
                guild_id: g.clone(),
                emoji_id: "2".into(),
            },
            &Ok(AdminOutcome::Json(Value::Null)),
        );
        page.on_result(
            &AdminAction::PickEmojiFile,
            &Ok(AdminOutcome::File {
                name: "x.txt".into(),
                bytes: b"nope".to_vec(),
            }),
        );
        assert!(page.error.is_some());
        let list = page.list.get().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "poggers");
    }
}
