//! Settings → Profiles: display name, pronouns, about me, avatar, banner and
//! banner colour with a live preview card, plus per-server profiles
//! (nickname, server avatar).
//!
//! Protocol (user accounts; checked against Discord's client behaviour and
//! the community docs at docs.discord.food, not an official API):
//! - Load: `GET users/@me` (`global_name`, `avatar`, `banner`,
//!   `accent_color`, `premium_type`) + `GET users/{me}/profile`
//!   (`user_profile: { bio, pronouns, accent_color }`).
//! - `PATCH users/@me` with `global_name` (1–32 chars, `null` resets to the
//!   username), `avatar` and `banner` as image data URIs
//!   (`data:image/png;base64,…`, `null` removes). No password needed for
//!   these three (it is for username / email / password changes).
//! - `PATCH users/@me/profile` with `bio` (≤ 190), `pronouns` (≤ 40) and
//!   `accent_color` (integer 0xRRGGBB, `null` resets). UNCERTAIN: older
//!   clients sent `bio` / `accent_color` to `PATCH users/@me`; the backend
//!   falls back to that if `users/@me/profile` answers 404/405.
//! - Banners and animated avatars (GIF, animated WebP) need Nitro; Discord
//!   answers 400 otherwise, so the page blocks them when `premium_type` is 0.
//!   UNCERTAIN: the upload size cap. Discord's client refuses files over
//!   10 MB for avatars and banners; we use 10 MiB.
//! - Per server: `GET users/{me}/profile?guild_id=` → `guild_member: { nick,
//!   avatar }`; `PATCH guilds/{id}/members/@me` with `nick` (≤ 32, `null`
//!   resets) and `avatar` (data URI / `null`; server avatars need Nitro).
//!   UNCERTAIN: server bio / pronouns / banner go to a separate guild-profile
//!   endpoint in Discord's client and are not implemented.
//! - Rate limits: avatar changes are heavily rate limited (a few per ~10
//!   minutes); the error text from Discord is shown as is.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use base64::Engine;
use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::{Map, Value, json};

use super::widgets::{self, Icon};
use super::{Action, Ctx, Loadable, Outcome, Request};
use crate::theme::Bold;

pub const MAX_DISPLAY_NAME: usize = 32;
pub const MAX_PRONOUNS: usize = 40;
pub const MAX_BIO: usize = 190;
pub const MAX_NICK: usize = 32;
/// Largest avatar / banner we upload (Discord's client limit is 10 MB).
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const CDN: &str = "https://cdn.discordapp.com";
const CARD_WIDTH: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageSlot {
    Avatar,
    Banner,
    /// Per-server avatar (Server Profiles tab).
    ServerAvatar,
}

impl ImageSlot {
    pub fn label(self) -> &'static str {
        match self {
            ImageSlot::Avatar => "avatar",
            ImageSlot::Banner => "banner",
            ImageSlot::ServerAvatar => "server avatar",
        }
    }
}

/// A checked image ready to upload.
#[derive(Clone)]
pub struct PickedImage {
    pub name: String,
    pub mime: &'static str,
    pub bytes: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
    /// GIF or animated WebP (needs Nitro).
    pub animated: bool,
}

impl fmt::Debug for PickedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickedImage")
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("len", &self.bytes.len())
            .field("size", &(self.width, self.height))
            .field("animated", &self.animated)
            .finish()
    }
}

impl PartialEq for PickedImage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
    }
}

impl PickedImage {
    /// `data:<mime>;base64,<…>` for the PATCH body.
    pub fn data_uri(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mime,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }
}

/// Image type from the first bytes (the extension can lie).
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Animated WebP: extended format (`VP8X`) with the animation flag.
fn webp_animated(bytes: &[u8]) -> bool {
    bytes.len() > 20 && &bytes[12..16] == b"VP8X" && bytes[20] & 0x02 != 0
}

/// Size / type checks for an avatar or banner file.
pub fn check_image(name: &str, bytes: Vec<u8>) -> Result<PickedImage, String> {
    if bytes.is_empty() {
        return Err("That file is empty.".into());
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "That file is {:.1} MB. Images must be under 10 MB.",
            bytes.len() as f64 / 1_048_576.0
        ));
    }
    let mime = sniff_mime(&bytes).ok_or("Use a PNG, JPEG, GIF or WebP image.")?;
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .ok_or("That image couldn't be read.")?;
    if width == 0 || height == 0 {
        return Err("That image is empty.".into());
    }
    let animated = mime == "image/gif" || (mime == "image/webp" && webp_animated(&bytes));
    Ok(PickedImage {
        name: name.to_string(),
        mime,
        bytes: bytes.into(),
        width,
        height,
        animated,
    })
}

/// Change to an image field.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ImageEdit {
    #[default]
    Keep,
    Remove,
    New(PickedImage),
}

impl ImageEdit {
    /// The PATCH value, or None if unchanged.
    fn patch_value(&self) -> Option<Value> {
        match self {
            ImageEdit::Keep => None,
            ImageEdit::Remove => Some(Value::Null),
            ImageEdit::New(img) => Some(Value::String(img.data_uri())),
        }
    }
}

/// My profile as loaded from Discord.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MyProfile {
    pub id: String,
    pub username: String,
    /// Empty = none (shows the username).
    pub global_name: String,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    pub bio: String,
    pub pronouns: String,
    /// Has Nitro (banner, animated avatar, server avatar).
    pub premium: bool,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

fn color_of(v: &Value, k: &str) -> Option<u32> {
    v.get(k)
        .and_then(Value::as_u64)
        .map(|c| (c & 0xff_ffff) as u32)
}

impl MyProfile {
    /// From `GET users/@me` and (optionally) `GET users/{me}/profile`.
    pub fn from_json(user: &Value, profile: Option<&Value>) -> Self {
        let meta = profile
            .map(|p| &p["user_profile"])
            .filter(|m| m.is_object());
        let pick = |k: &str| {
            meta.map(|m| s(m, k))
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| s(user, k))
        };
        Self {
            id: s(user, "id"),
            username: s(user, "username"),
            global_name: s(user, "global_name"),
            avatar: opt_s(user, "avatar"),
            banner: opt_s(user, "banner"),
            accent_color: meta
                .and_then(|m| color_of(m, "accent_color"))
                .or_else(|| color_of(user, "accent_color")),
            bio: pick("bio"),
            pronouns: pick("pronouns"),
            premium: user
                .get("premium_type")
                .and_then(Value::as_u64)
                .is_some_and(|t| t > 0),
        }
    }

    pub fn avatar_url(&self) -> String {
        crate::types::avatar_url(&self.id, self.avatar.as_deref()).replace("size=64", "size=128")
    }

    pub fn banner_url(&self) -> Option<String> {
        self.banner.as_ref().map(|h| {
            let ext = if h.starts_with("a_") { "gif" } else { "png" };
            format!("{CDN}/banners/{}/{h}.{ext}?size=600", self.id)
        })
    }
}

/// The editable copy of [`MyProfile`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileDraft {
    pub global_name: String,
    pub pronouns: String,
    pub bio: String,
    pub accent_color: Option<u32>,
    pub avatar: ImageEdit,
    pub banner: ImageEdit,
}

impl ProfileDraft {
    pub fn from_profile(p: &MyProfile) -> Self {
        Self {
            global_name: p.global_name.clone(),
            pronouns: p.pronouns.clone(),
            bio: p.bio.clone(),
            accent_color: p.accent_color,
            avatar: ImageEdit::Keep,
            banner: ImageEdit::Keep,
        }
    }
}

/// Problems that stop saving (shown under the form).
pub fn validate(d: &ProfileDraft, premium: bool) -> Vec<String> {
    let mut errs = Vec::new();
    let name = d.global_name.trim();
    if name.chars().count() > MAX_DISPLAY_NAME {
        errs.push(format!(
            "Display name must be {MAX_DISPLAY_NAME} characters or fewer."
        ));
    }
    if name.to_lowercase().contains("discord") {
        errs.push("Display names can't contain \"discord\".".into());
    }
    if d.pronouns.chars().count() > MAX_PRONOUNS {
        errs.push(format!(
            "Pronouns must be {MAX_PRONOUNS} characters or fewer."
        ));
    }
    if d.bio.chars().count() > MAX_BIO {
        errs.push(format!("About me must be {MAX_BIO} characters or fewer."));
    }
    if !premium {
        if matches!(&d.avatar, ImageEdit::New(i) if i.animated) {
            errs.push("Animated avatars need Nitro.".into());
        }
        if matches!(d.banner, ImageEdit::New(_)) {
            errs.push("Profile banners need Nitro.".into());
        }
    }
    errs
}

/// Body for `PATCH users/@me` (display name, avatar, banner), or None if
/// none of them changed.
pub fn user_patch(orig: &MyProfile, d: &ProfileDraft) -> Option<Value> {
    let mut body = Map::new();
    let name = d.global_name.trim();
    if name != orig.global_name {
        body.insert(
            "global_name".into(),
            if name.is_empty() {
                Value::Null
            } else {
                Value::String(name.into())
            },
        );
    }
    if let Some(v) = d.avatar.patch_value() {
        body.insert("avatar".into(), v);
    }
    if let Some(v) = d.banner.patch_value() {
        body.insert("banner".into(), v);
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

/// Body for `PATCH users/@me/profile` (bio, pronouns, banner colour), or
/// None if unchanged.
pub fn profile_patch(orig: &MyProfile, d: &ProfileDraft) -> Option<Value> {
    let mut body = Map::new();
    if d.bio != orig.bio {
        body.insert("bio".into(), json!(d.bio));
    }
    let pronouns = d.pronouns.trim();
    if pronouns != orig.pronouns {
        body.insert("pronouns".into(), json!(pronouns));
    }
    if d.accent_color != orig.accent_color {
        body.insert("accent_color".into(), json!(d.accent_color));
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

/// My member data in one server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemberProfile {
    pub nick: String,
    pub avatar: Option<String>,
}

impl MemberProfile {
    /// From a profile response (`guild_member`) or a member object.
    pub fn from_json(v: &Value) -> Self {
        let m = v.get("guild_member").filter(|m| m.is_object()).unwrap_or(v);
        Self {
            nick: s(m, "nick"),
            avatar: opt_s(m, "avatar"),
        }
    }
}

/// Body for `PATCH guilds/{id}/members/@me`, or None if unchanged.
pub fn member_patch(orig: &MemberProfile, nick: &str, avatar: &ImageEdit) -> Option<Value> {
    let mut body = Map::new();
    let nick = nick.trim();
    if nick != orig.nick {
        body.insert(
            "nick".into(),
            if nick.is_empty() {
                Value::Null
            } else {
                json!(nick)
            },
        );
    }
    if let Some(v) = avatar.patch_value() {
        body.insert("avatar".into(), v);
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

pub fn validate_member(nick: &str, avatar: &ImageEdit, premium: bool) -> Vec<String> {
    let mut errs = Vec::new();
    if nick.trim().chars().count() > MAX_NICK {
        errs.push(format!("Nicknames must be {MAX_NICK} characters or fewer."));
    }
    if !premium && matches!(avatar, ImageEdit::New(_)) {
        errs.push("Server avatars need Nitro.".into());
    }
    errs
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    User,
    Server,
}

#[derive(Default)]
struct ServerTab {
    guild_id: Option<String>,
    member: Loadable<MemberProfile>,
    nick: String,
    avatar: ImageEdit,
    saving: bool,
    status: Option<Result<String, String>>,
}

#[derive(Default)]
pub struct ProfilesPage {
    pub tab: Tab,
    pub profile: Loadable<MyProfile>,
    draft: Option<ProfileDraft>,
    saving: bool,
    status: Option<Result<String, String>>,
    server: ServerTab,
    /// `bytes://` URIs of picked images, to free them.
    previews: HashMap<ImageSlot, String>,
    counter: u32,
}

impl ProfilesPage {
    pub fn apply(&mut self, ctx: &egui::Context, outcome: Outcome) {
        match outcome {
            Outcome::Profile(r) => {
                if let Ok(p) = &r {
                    self.draft = Some(ProfileDraft::from_profile(p));
                }
                self.profile.set(r);
            }
            Outcome::ProfileSaved(r) => {
                self.saving = false;
                match r {
                    Ok(p) => {
                        self.forget(ctx, ImageSlot::Avatar);
                        self.forget(ctx, ImageSlot::Banner);
                        self.draft = Some(ProfileDraft::from_profile(&p));
                        self.profile = Loadable::Loaded(p);
                        self.status = Some(Ok("Profile saved.".into()));
                    }
                    Err(e) => self.status = Some(Err(e)),
                }
            }
            Outcome::ImagePicked { slot, result } => match result {
                Ok(None) => {}
                Ok(Some(img)) => {
                    self.forget(ctx, slot);
                    match slot {
                        ImageSlot::ServerAvatar => self.server.avatar = ImageEdit::New(img),
                        _ => {
                            if let Some(d) = &mut self.draft {
                                let edit = ImageEdit::New(img);
                                if slot == ImageSlot::Avatar {
                                    d.avatar = edit;
                                } else {
                                    d.banner = edit;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let st = Some(Err(e));
                    if slot == ImageSlot::ServerAvatar {
                        self.server.status = st;
                    } else {
                        self.status = st;
                    }
                }
            },
            Outcome::Member { guild_id, result } => {
                if self.server.guild_id.as_deref() == Some(&guild_id) {
                    if let Ok(m) = &result {
                        self.server.nick = m.nick.clone();
                    }
                    self.server.member.set(result);
                }
            }
            Outcome::MemberSaved { guild_id, result } => {
                if self.server.guild_id.as_deref() != Some(&guild_id) {
                    return;
                }
                self.server.saving = false;
                match result {
                    Ok(m) => {
                        self.forget(ctx, ImageSlot::ServerAvatar);
                        self.server.avatar = ImageEdit::Keep;
                        self.server.nick = m.nick.clone();
                        self.server.member = Loadable::Loaded(m);
                        self.server.status = Some(Ok("Server profile saved.".into()));
                    }
                    Err(e) => self.server.status = Some(Err(e)),
                }
            }
            _ => {}
        }
    }

    fn forget(&mut self, ctx: &egui::Context, slot: ImageSlot) {
        if let Some(uri) = self.previews.remove(&slot) {
            ctx.forget_image(&uri);
        }
    }

    pub fn forget_images(&mut self, ctx: &egui::Context) {
        for (_, uri) in self.previews.drain() {
            ctx.forget_image(&uri);
        }
    }

    /// `bytes://` URI for a picked image (stable while it is shown).
    fn preview_uri(&mut self, slot: ImageSlot, img: &PickedImage) -> String {
        let counter = &mut self.counter;
        self.previews
            .entry(slot)
            .or_insert_with(|| {
                *counter += 1;
                let ext = img.mime.trim_start_matches("image/");
                format!(
                    "bytes://rc-us-{}-{}.{ext}",
                    slot.label().replace(' ', "-"),
                    counter
                )
            })
            .clone()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        if c.is_bot {
            widgets::hint(
                ui,
                pal,
                "Bot profiles are edited in the Discord Developer Portal \
                 (Applications → your bot → Bot).",
            );
            return None;
        }
        let mut action = None;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::User, "User Profile");
            ui.selectable_value(&mut self.tab, Tab::Server, "Server Profiles");
        });
        widgets::divider(ui, pal);
        ui.add_space(12.0);
        if self.profile.start() {
            return Some(Action::Request(Request::LoadProfile));
        }
        match &self.profile {
            Loadable::Loading | Loadable::NotLoaded => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    widgets::hint(ui, pal, "Loading your profile…");
                });
                return None;
            }
            Loadable::Failed(e) => {
                widgets::error(ui, pal, &format!("Couldn't load your profile: {e}"));
                if ui.button("Try again").clicked() {
                    self.profile = Loadable::Loading;
                    return Some(Action::Request(Request::LoadProfile));
                }
                return None;
            }
            Loadable::Loaded(_) => {}
        }
        match self.tab {
            Tab::User => {
                if let Some(a) = self.user_tab(ui, c) {
                    action = Some(a);
                }
            }
            Tab::Server => {
                if let Some(a) = self.server_tab(ui, c) {
                    action = Some(a);
                }
            }
        }
        action
    }

    fn user_tab(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        let mut action = None;
        let profile = self.profile.get()?.clone();
        let mut draft = self
            .draft
            .take()
            .unwrap_or_else(|| ProfileDraft::from_profile(&profile));
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - CARD_WIDTH - 24.0).clamp(240.0, 360.0));
                widgets::label(ui, pal, "DISPLAY NAME");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.global_name)
                        .hint_text(&profile.username)
                        .char_limit(MAX_DISPLAY_NAME)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);
                widgets::label(ui, pal, "PRONOUNS");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.pronouns)
                        .hint_text("Add your pronouns")
                        .char_limit(MAX_PRONOUNS)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);
                widgets::label(ui, pal, "AVATAR");
                ui.horizontal(|ui| {
                    if ui.add(pal.primary_button("Change Avatar")).clicked() {
                        action = Some(Action::Request(Request::PickImage(ImageSlot::Avatar)));
                    }
                    let has = match &draft.avatar {
                        ImageEdit::Keep => profile.avatar.is_some(),
                        ImageEdit::Remove => false,
                        ImageEdit::New(_) => true,
                    };
                    if has && ui.button("Remove Avatar").clicked() {
                        draft.avatar = ImageEdit::Remove;
                    }
                });
                ui.add_space(10.0);
                widgets::label(ui, pal, "PROFILE BANNER");
                ui.horizontal(|ui| {
                    let pick = ui.add_enabled(profile.premium, pal.primary_button("Change Banner"));
                    if pick.clicked() {
                        action = Some(Action::Request(Request::PickImage(ImageSlot::Banner)));
                    }
                    let has = match &draft.banner {
                        ImageEdit::Keep => profile.banner.is_some(),
                        ImageEdit::Remove => false,
                        ImageEdit::New(_) => true,
                    };
                    if has && ui.button("Remove Banner").clicked() {
                        draft.banner = ImageEdit::Remove;
                    }
                });
                if !profile.premium {
                    widgets::hint(ui, pal, "Banner images need Nitro. Banner colour is free.");
                }
                ui.add_space(10.0);
                widgets::label(ui, pal, "BANNER COLOUR");
                ui.horizontal(|ui| {
                    let cur = draft.accent_color.unwrap_or(0x5865f2);
                    let mut arr = [(cur >> 16) as u8, (cur >> 8) as u8, cur as u8];
                    if egui::color_picker::color_edit_button_srgb(ui, &mut arr).changed() {
                        draft.accent_color = Some(
                            (u32::from(arr[0]) << 16)
                                | (u32::from(arr[1]) << 8)
                                | u32::from(arr[2]),
                        );
                    }
                    if let Some(c) = draft.accent_color {
                        ui.label(RichText::new(format!("#{c:06X}")).color(pal.muted));
                        if ui.button("Reset").clicked() {
                            draft.accent_color = None;
                        }
                    } else {
                        widgets::hint(ui, pal, "Default");
                    }
                });
                ui.add_space(10.0);
                widgets::label(ui, pal, "ABOUT ME");
                widgets::hint(ui, pal, "You can use markdown and links if you'd like.");
                ui.add(
                    egui::TextEdit::multiline(&mut draft.bio)
                        .char_limit(MAX_BIO)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                let n = draft.bio.chars().count();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(
                        RichText::new(format!("{}", MAX_BIO - n.min(MAX_BIO)))
                            .size(12.0)
                            .color(pal.muted),
                    );
                });
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                widgets::label(ui, pal, "PREVIEW");
                self.preview_card(ui, c, &profile, &draft);
            });
        });

        ui.add_space(12.0);
        let errs = validate(&draft, profile.premium);
        for e in &errs {
            widgets::error(ui, pal, e);
        }
        let pristine = ProfileDraft::from_profile(&profile);
        let dirty = draft != pristine;
        if let Some(st) = &self.status {
            match st {
                Ok(m) => {
                    ui.label(RichText::new(m).color(pal.online));
                }
                Err(e) => widgets::error(ui, pal, e),
            }
        }
        if dirty || self.saving {
            unsaved_bar(ui, c, self.saving, errs.is_empty(), |save| {
                if save {
                    let user = user_patch(&profile, &draft);
                    let prof = profile_patch(&profile, &draft);
                    if user.is_some() || prof.is_some() {
                        self.saving = true;
                        self.status = None;
                        action = Some(Action::Request(Request::SaveProfile {
                            user,
                            profile: prof,
                        }));
                    }
                } else {
                    draft = pristine.clone();
                    self.status = None;
                }
            });
        }
        if !matches!(draft.avatar, ImageEdit::New(_)) {
            self.forget(ui.ctx(), ImageSlot::Avatar);
        }
        if !matches!(draft.banner, ImageEdit::New(_)) {
            self.forget(ui.ctx(), ImageSlot::Banner);
        }
        self.draft = Some(draft);
        action
    }

    fn preview_card(&mut self, ui: &mut egui::Ui, c: &Ctx, p: &MyProfile, d: &ProfileDraft) {
        let pal = c.pal;
        let avatar = match &d.avatar {
            ImageEdit::Keep => Some(p.avatar_url()),
            ImageEdit::Remove => Some(crate::types::avatar_url(&p.id, None)),
            ImageEdit::New(img) => {
                let uri = self.preview_uri(ImageSlot::Avatar, img);
                bytes_image(ui.ctx(), &uri, img);
                Some(uri)
            }
        };
        let banner = match &d.banner {
            ImageEdit::Keep => p.banner_url(),
            ImageEdit::Remove => None,
            ImageEdit::New(img) => {
                let uri = self.preview_uri(ImageSlot::Banner, img);
                bytes_image(ui.ctx(), &uri, img);
                Some(uri)
            }
        };
        let name = if d.global_name.trim().is_empty() {
            p.username.clone()
        } else {
            d.global_name.trim().to_string()
        };
        draw_card(
            ui,
            c,
            CardData {
                banner_color: d.accent_color.map(rgb).unwrap_or(pal.accent),
                banner_uri: banner.as_deref(),
                avatar_uri: avatar.as_deref(),
                name: &name,
                username: &p.username,
                pronouns: d.pronouns.trim(),
                bio: &d.bio,
                server: None,
            },
        );
    }

    fn server_tab(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        let profile = self.profile.get()?.clone();
        let mut action = None;
        widgets::hint(
            ui,
            pal,
            "Show who you are in each of your servers. Changes here only apply to the \
             server you pick.",
        );
        ui.add_space(6.0);
        widgets::label(ui, pal, "CHOOSE A SERVER");
        let selected_name = self
            .server
            .guild_id
            .as_ref()
            .and_then(|id| c.guilds.iter().find(|g| &g.id == id))
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "Select a server".into());
        let mut picked = None;
        egui::ComboBox::from_id_salt("rc_us_server_pick")
            .selected_text(selected_name)
            .width(300.0)
            .show_ui(ui, |ui| {
                for g in c.guilds {
                    let sel = self.server.guild_id.as_deref() == Some(g.id.as_str());
                    if ui.selectable_label(sel, &g.name).clicked() && !sel {
                        picked = Some(g.id.clone());
                    }
                }
            });
        if let Some(id) = picked {
            if let ImageEdit::New(_) = self.server.avatar {
                self.forget(ui.ctx(), ImageSlot::ServerAvatar);
            }
            self.server = ServerTab {
                guild_id: Some(id.clone()),
                member: Loadable::Loading,
                ..Default::default()
            };
            return Some(Action::Request(Request::LoadMember {
                guild_id: id,
                user_id: profile.id.clone(),
            }));
        }
        let gid = self.server.guild_id.clone()?;
        ui.add_space(12.0);
        let member = match &self.server.member {
            Loadable::Loaded(m) => m.clone(),
            Loadable::Failed(e) => {
                widgets::error(ui, pal, &format!("Couldn't load your server profile: {e}"));
                if ui.button("Try again").clicked() {
                    self.server.member = Loadable::Loading;
                    return Some(Action::Request(Request::LoadMember {
                        guild_id: gid,
                        user_id: profile.id.clone(),
                    }));
                }
                return None;
            }
            _ => {
                ui.spinner();
                return None;
            }
        };
        let guild_name = c
            .guilds
            .iter()
            .find(|g| g.id == gid)
            .map(|g| g.name.as_str())
            .unwrap_or("");
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - CARD_WIDTH - 24.0).clamp(240.0, 360.0));
                widgets::label(ui, pal, "SERVER NICKNAME");
                let hint = if profile.global_name.is_empty() {
                    profile.username.clone()
                } else {
                    profile.global_name.clone()
                };
                ui.add(
                    egui::TextEdit::singleline(&mut self.server.nick)
                        .hint_text(hint)
                        .char_limit(MAX_NICK)
                        .desired_width(f32::INFINITY),
                );
                widgets::hint(
                    ui,
                    pal,
                    "Leave empty to use your display name. Some servers don't allow \
                     changing your nickname.",
                );
                ui.add_space(10.0);
                widgets::label(ui, pal, "SERVER AVATAR");
                ui.horizontal(|ui| {
                    let b = ui.add_enabled(profile.premium, pal.primary_button("Change Avatar"));
                    if b.clicked() {
                        action = Some(Action::Request(Request::PickImage(ImageSlot::ServerAvatar)));
                    }
                    let has = match &self.server.avatar {
                        ImageEdit::Keep => member.avatar.is_some(),
                        ImageEdit::Remove => false,
                        ImageEdit::New(_) => true,
                    };
                    if has && ui.button("Remove Avatar").clicked() {
                        self.server.avatar = ImageEdit::Remove;
                    }
                });
                if !profile.premium {
                    widgets::hint(ui, pal, "Per-server avatars need Nitro.");
                }
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                widgets::label(ui, pal, "PREVIEW");
                let avatar = match &self.server.avatar {
                    ImageEdit::New(img) => {
                        let img = img.clone();
                        let uri = self.preview_uri(ImageSlot::ServerAvatar, &img);
                        bytes_image(ui.ctx(), &uri, &img);
                        uri
                    }
                    ImageEdit::Keep if member.avatar.is_some() => format!(
                        "{CDN}/guilds/{gid}/users/{}/avatars/{}.png?size=128",
                        profile.id,
                        member.avatar.as_deref().unwrap_or_default()
                    ),
                    _ => profile.avatar_url(),
                };
                let name = [&self.server.nick, &profile.global_name, &profile.username]
                    .into_iter()
                    .map(|s| s.trim())
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string();
                draw_card(
                    ui,
                    c,
                    CardData {
                        banner_color: profile.accent_color.map(rgb).unwrap_or(pal.accent),
                        banner_uri: profile.banner_url().as_deref(),
                        avatar_uri: Some(&avatar),
                        name: &name,
                        username: &profile.username,
                        pronouns: &profile.pronouns,
                        bio: &profile.bio,
                        server: Some(guild_name),
                    },
                );
            });
        });
        ui.add_space(12.0);
        let errs = validate_member(&self.server.nick, &self.server.avatar, profile.premium);
        for e in &errs {
            widgets::error(ui, pal, e);
        }
        if let Some(st) = &self.server.status {
            match st {
                Ok(m) => {
                    ui.label(RichText::new(m).color(pal.online));
                }
                Err(e) => widgets::error(ui, pal, e),
            }
        }
        let body = member_patch(&member, &self.server.nick, &self.server.avatar);
        if body.is_some() || self.server.saving {
            let saving = self.server.saving;
            let mut reset = false;
            unsaved_bar(ui, c, saving, errs.is_empty(), |save| {
                if save {
                    if let Some(body) = body.clone() {
                        action = Some(Action::Request(Request::SaveMember {
                            guild_id: gid.clone(),
                            body,
                        }));
                    }
                } else {
                    reset = true;
                }
            });
            if matches!(action, Some(Action::Request(Request::SaveMember { .. }))) {
                self.server.saving = true;
                self.server.status = None;
            }
            if reset {
                self.server.nick = member.nick.clone();
                self.server.avatar = ImageEdit::Keep;
                self.server.status = None;
            }
        }
        if !matches!(self.server.avatar, ImageEdit::New(_)) {
            self.forget(ui.ctx(), ImageSlot::ServerAvatar);
        }
        action
    }
}

/// Register a picked image's bytes with egui under `uri` (no-op if already).
fn bytes_image(ctx: &egui::Context, uri: &str, img: &PickedImage) {
    ctx.include_bytes(
        uri.to_string(),
        egui::load::Bytes::Shared(img.bytes.clone()),
    );
}

/// Discord's "Careful — you have unsaved changes!" bar. `on(true)` = save,
/// `on(false)` = reset.
fn unsaved_bar(ui: &mut egui::Ui, c: &Ctx, saving: bool, valid: bool, on: impl FnOnce(bool)) {
    let pal = c.pal;
    let mut choice = None;
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Careful — you have unsaved changes!")
                        .bold()
                        .color(pal.strong),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if saving {
                        ui.spinner();
                        widgets::hint(ui, pal, "Saving…");
                    } else {
                        if ui
                            .add_enabled(valid, pal.primary_button("Save Changes"))
                            .clicked()
                        {
                            choice = Some(true);
                        }
                        if ui.button("Reset").clicked() {
                            choice = Some(false);
                        }
                    }
                });
            });
        });
    if let Some(ch) = choice {
        on(ch);
    }
}

struct CardData<'a> {
    banner_color: Color32,
    banner_uri: Option<&'a str>,
    avatar_uri: Option<&'a str>,
    name: &'a str,
    username: &'a str,
    pronouns: &'a str,
    bio: &'a str,
    /// Server name for the per-server preview.
    server: Option<&'a str>,
}

/// The profile popout look: banner, avatar overlapping it, names, bio.
fn draw_card(ui: &mut egui::Ui, c: &Ctx, d: CardData) {
    let pal = c.pal;
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(CARD_WIDTH);
            let banner_h = 105.0;
            let (banner, _) =
                ui.allocate_exact_size(vec2(CARD_WIDTH, banner_h), egui::Sense::hover());
            let top = egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            };
            ui.painter().rect_filled(banner, top, d.banner_color);
            if let Some(uri) = d.banner_uri {
                egui::Image::new(uri)
                    .corner_radius(top)
                    .fit_to_exact_size(banner.size())
                    .paint_at(ui, banner);
            }
            let av = 80.0;
            let centre = egui::pos2(banner.left() + 16.0 + av / 2.0, banner.bottom());
            ui.painter()
                .circle_filled(centre, av / 2.0 + 5.0, pal.servers);
            let av_rect = egui::Rect::from_center_size(centre, vec2(av, av));
            ui.painter().circle_filled(centre, av / 2.0, pal.rail_icon);
            if let Some(uri) = d.avatar_uri {
                egui::Image::new(uri)
                    .corner_radius(av / 2.0)
                    .fit_to_exact_size(av_rect.size())
                    .paint_at(ui, av_rect);
            }
            ui.add_space(av / 2.0 + 10.0);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 0,
                    bottom: 16,
                })
                .show(ui, |ui| {
                    ui.set_width(CARD_WIDTH - 32.0);
                    ui.label(RichText::new(d.name).bold().size(20.0).color(pal.strong));
                    let mut line = d.username.to_string();
                    if !d.pronouns.is_empty() {
                        line.push_str(" • ");
                        line.push_str(d.pronouns);
                    }
                    ui.label(RichText::new(line).size(13.0).color(pal.text));
                    if let Some(server) = d.server {
                        ui.label(
                            RichText::new(format!("in {server}"))
                                .size(12.0)
                                .color(pal.muted),
                        );
                    }
                    if !d.bio.trim().is_empty() {
                        ui.add_space(8.0);
                        widgets::divider(ui, pal);
                        ui.add_space(8.0);
                        ui.label(pal.section_label("ABOUT ME"));
                        ui.label(RichText::new(d.bio.trim()).size(13.0).color(pal.text));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        widgets::icon(ui, Icon::Check, 14.0, pal.muted);
                        widgets::hint(ui, pal, "This is how others see you");
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_1x1() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(2, 3)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn me() -> MyProfile {
        MyProfile {
            id: "42".into(),
            username: "leo".into(),
            global_name: "Leo".into(),
            avatar: Some("abc".into()),
            banner: None,
            accent_color: None,
            bio: "hi".into(),
            pronouns: "he/him".into(),
            premium: false,
        }
    }

    #[test]
    fn sniffing_and_checks() {
        assert_eq!(sniff_mime(&png_1x1()), Some("image/png"));
        assert_eq!(sniff_mime(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89a....."), Some("image/gif"));
        assert_eq!(sniff_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_mime(b"%PDF-1.4"), None);

        let img = check_image("a.png", png_1x1()).unwrap();
        assert_eq!((img.width, img.height), (2, 3));
        assert!(!img.animated);
        assert!(
            img.data_uri()
                .starts_with("data:image/png;base64,iVBORw0KGgo")
        );

        assert!(check_image("x", vec![]).is_err());
        assert!(check_image("x.png", b"not an image".to_vec()).is_err());
        let mut big = png_1x1();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(check_image("big.png", big).unwrap_err().contains("10 MB"));
        // Truncated PNG header: right magic, unreadable.
        assert!(check_image("t.png", png_1x1()[..10].to_vec()).is_err());
    }

    #[test]
    fn animated_webp_flag() {
        let mut b = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        b.extend_from_slice(&[0, 0, 0, 0, 0x02, 0, 0, 0]);
        assert!(webp_animated(&b));
        b[20] = 0;
        assert!(!webp_animated(&b));
    }

    #[test]
    fn parse_profile() {
        let user = json!({
            "id": "42", "username": "leo", "global_name": "Leo", "avatar": "abc",
            "banner": "a_ban", "accent_color": 255, "bio": "old", "premium_type": 2
        });
        let prof = json!({ "user_profile": { "bio": "new bio", "pronouns": "he/him", "accent_color": 16711680 } });
        let p = MyProfile::from_json(&user, Some(&prof));
        assert_eq!(p.bio, "new bio");
        assert_eq!(p.pronouns, "he/him");
        assert_eq!(p.accent_color, Some(0xff0000));
        assert!(p.premium);
        assert!(
            p.banner_url()
                .unwrap()
                .ends_with("/banners/42/a_ban.gif?size=600")
        );
        assert!(p.avatar_url().contains("size=128"));
        // Without the profile: user fields; null global_name / premium_type.
        let p = MyProfile::from_json(
            &json!({ "id": "1", "username": "u", "global_name": null }),
            None,
        );
        assert_eq!(p.global_name, "");
        assert!(!p.premium && p.accent_color.is_none() && p.banner_url().is_none());
    }

    #[test]
    fn user_and_profile_patches() {
        let orig = me();
        let mut d = ProfileDraft::from_profile(&orig);
        assert_eq!(user_patch(&orig, &d), None);
        assert_eq!(profile_patch(&orig, &d), None);

        d.global_name = "  ".into();
        d.avatar = ImageEdit::Remove;
        assert_eq!(
            user_patch(&orig, &d),
            Some(json!({ "global_name": null, "avatar": null }))
        );
        d.global_name = " Leo K ".into();
        d.avatar = ImageEdit::New(check_image("a.png", png_1x1()).unwrap());
        let body = user_patch(&orig, &d).unwrap();
        assert_eq!(body["global_name"], "Leo K");
        assert!(
            body["avatar"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(body.get("banner").is_none());

        d.bio = "new".into();
        d.accent_color = Some(0x123456);
        assert_eq!(
            profile_patch(&orig, &d),
            Some(json!({ "bio": "new", "accent_color": 0x123456 }))
        );
        d.accent_color = None;
        d.pronouns = "they/them".into();
        assert_eq!(
            profile_patch(&orig, &d),
            Some(json!({ "bio": "new", "pronouns": "they/them" }))
        );
    }

    #[test]
    fn validation() {
        let orig = me();
        let mut d = ProfileDraft::from_profile(&orig);
        assert!(validate(&d, false).is_empty());
        d.bio = "x".repeat(MAX_BIO + 1);
        d.pronouns = "p".repeat(MAX_PRONOUNS + 1);
        d.global_name = "my Discord name".into();
        assert_eq!(validate(&d, true).len(), 3);
        // Bio limit counts characters, not bytes.
        d = ProfileDraft::from_profile(&orig);
        d.bio = "é".repeat(MAX_BIO);
        assert!(validate(&d, false).is_empty());

        let mut gif = check_image("a.png", png_1x1()).unwrap();
        gif.animated = true;
        d.avatar = ImageEdit::New(gif);
        d.banner = ImageEdit::New(check_image("b.png", png_1x1()).unwrap());
        assert_eq!(validate(&d, false).len(), 2);
        assert!(validate(&d, true).is_empty());
    }

    #[test]
    fn member_patches() {
        let orig =
            MemberProfile::from_json(&json!({ "guild_member": { "nick": "Keo", "avatar": null } }));
        assert_eq!(orig.nick, "Keo");
        assert_eq!(member_patch(&orig, " Keo ", &ImageEdit::Keep), None);
        assert_eq!(
            member_patch(&orig, "", &ImageEdit::Keep),
            Some(json!({ "nick": null }))
        );
        assert_eq!(
            member_patch(&orig, "Leo", &ImageEdit::Remove),
            Some(json!({ "nick": "Leo", "avatar": null }))
        );
        assert_eq!(
            validate_member(&"n".repeat(33), &ImageEdit::Keep, true).len(),
            1
        );
        let img = check_image("a.png", png_1x1()).unwrap();
        assert_eq!(validate_member("ok", &ImageEdit::New(img), false).len(), 1);
        // Plain member object.
        assert_eq!(
            MemberProfile::from_json(&json!({ "nick": "n", "avatar": "h" }))
                .avatar
                .as_deref(),
            Some("h")
        );
    }
}
