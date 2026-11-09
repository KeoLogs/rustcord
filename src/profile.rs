//! User profiles and the profile popout card.
//!
//! Protocol notes (from Discord Messenger, `core/state/ProfileCache.cpp`):
//! - `GET users/{id}/profile?with_mutual_guilds=false&with_mutual_friends=false
//!   &with_mutual_friends_count=false[&guild_id=…]` (user accounts only;
//!   bots get 403/404). Answer: `{ user: { id, username, global_name,
//!   avatar, banner, accent_color, banner_color, bio }, user_profile: { bio,
//!   pronouns, accent_color, theme_colors }, guild_member?: { nick, roles,
//!   joined_at }, premium_since?, … }`.
//! - Notes: `GET users/@me/notes/{id}` → `{ note }` (404 = no note);
//!   `PUT users/@me/notes/{id}` with `{ note }` sets it (an empty note
//!   clears it). `USER_NOTE_UPDATE { id, note }` arrives on change.
//! - "Member since" for the Discord account comes from the user id
//!   (snowflake timestamp), so it works without the profile request.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::Value;

use crate::config::Config;
use crate::members::{self, CachedMember, Role};
use crate::presence::Presence;
use crate::theme::Palette;
use crate::types::User;

/// Profiles kept in memory (most recently opened).
const MAX_PROFILES: usize = 20;
const CARD_WIDTH: f32 = 300.0;
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub user_id: String,
    /// Server the profile was loaded for (None = global profile).
    pub guild_id: Option<String>,
    pub bio: String,
    pub pronouns: String,
    /// Banner colour (0xRRGGBB): theme colour, banner colour or accent.
    pub banner_color: Option<u32>,
    /// CDN URL of the banner image, if they have one.
    pub banner_url: Option<String>,
    /// Server member data from the profile (if loaded with a guild_id).
    pub member: Option<CachedMember>,
}

fn hex_color(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim_start_matches('#'), 16).ok()
}

impl Profile {
    pub fn from_json(v: &Value, guild_id: Option<String>) -> Self {
        let user = &v["user"];
        let profile = &v["user_profile"];
        let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let bio = Some(s(profile, "bio"))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| s(user, "bio"));
        let banner_color = profile
            .pointer("/theme_colors/0")
            .and_then(Value::as_u64)
            .map(|c| c as u32)
            .or_else(|| {
                user.get("banner_color")
                    .and_then(Value::as_str)
                    .and_then(hex_color)
            })
            .or_else(|| {
                profile
                    .get("accent_color")
                    .and_then(Value::as_u64)
                    .map(|c| c as u32)
            })
            .or_else(|| {
                user.get("accent_color")
                    .and_then(Value::as_u64)
                    .map(|c| c as u32)
            });
        let id = s(user, "id");
        let banner_url = user
            .get("banner")
            .and_then(Value::as_str)
            .map(|h| format!("https://cdn.discordapp.com/banners/{id}/{h}.png?size=600"));
        let member = v.get("guild_member").filter(|m| m.is_object()).map(|m| {
            let m = members::Member::from_json(m);
            CachedMember {
                nick: m.nick,
                roles: m.roles,
                joined_at: m.joined_at,
            }
        });
        Self {
            user_id: id,
            guild_id,
            bio,
            pronouns: s(profile, "pronouns"),
            banner_color,
            banner_url,
            member,
        }
    }
}

/// Small most-recently-used cache of profiles and notes.
#[derive(Default)]
pub struct ProfileCache {
    profiles: VecDeque<Profile>,
    notes: VecDeque<(String, String)>,
    /// Users whose profile can't be loaded (bots asking, 403/404).
    unavailable: VecDeque<String>,
}

impl ProfileCache {
    pub fn get(&self, user_id: &str, guild_id: Option<&str>) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.user_id == user_id && p.guild_id.as_deref() == guild_id)
    }

    pub fn insert(&mut self, p: Profile) {
        self.profiles
            .retain(|o| !(o.user_id == p.user_id && o.guild_id == p.guild_id));
        self.unavailable.retain(|u| *u != p.user_id);
        self.profiles.push_front(p);
        self.profiles.truncate(MAX_PROFILES);
    }

    pub fn mark_unavailable(&mut self, user_id: &str) {
        if !self.unavailable.iter().any(|u| u == user_id) {
            self.unavailable.push_front(user_id.to_string());
            self.unavailable.truncate(MAX_PROFILES);
        }
    }

    pub fn is_unavailable(&self, user_id: &str) -> bool {
        self.unavailable.iter().any(|u| u == user_id)
    }

    pub fn note(&self, user_id: &str) -> Option<&str> {
        self.notes
            .iter()
            .find(|(u, _)| u == user_id)
            .map(|(_, n)| n.as_str())
    }

    pub fn set_note(&mut self, user_id: &str, note: String) {
        self.notes.retain(|(u, _)| u != user_id);
        self.notes.push_front((user_id.to_string(), note));
        self.notes.truncate(MAX_PROFILES);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.profiles.len()
    }
}

/// "9 Nov 2026" from an ISO-8601 timestamp.
pub fn date_from_iso(iso: &str) -> Option<String> {
    let y: u32 = iso.get(0..4)?.parse().ok()?;
    let m: usize = iso.get(5..7)?.parse().ok()?;
    let d: u32 = iso.get(8..10)?.parse().ok()?;
    Some(format!("{d} {} {y}", MONTHS.get(m.checked_sub(1)?)?))
}

/// When a Discord account (or anything with a snowflake id) was created.
pub fn snowflake_date(id: &str) -> Option<String> {
    let id: u64 = id.parse().ok()?;
    let ms = (id >> 22) + DISCORD_EPOCH_MS;
    date_from_iso(&crate::notify::iso_from_unix(ms / 1000))
}

// ---------------------------------------------------------------------------
// Popout
// ---------------------------------------------------------------------------

/// An open profile card.
pub struct Popout {
    pub user: User,
    pub guild_id: Option<String>,
    /// Screen position of what was clicked; the card opens beside it.
    pub anchor: egui::Rect,
    /// Note text being edited (None until the note is loaded).
    pub note_draft: Option<String>,
    /// Frame the card opened on, so the opening click doesn't close it.
    pub opened_frame: u64,
}

/// Everything the card shows, gathered by the app.
pub struct CardData<'a> {
    pub profile: Option<&'a Profile>,
    pub loading: bool,
    pub presence: Option<&'a Presence>,
    /// Member info from the member cache (or the profile).
    pub member: Option<&'a CachedMember>,
    /// The server's roles, sorted (empty outside servers).
    pub roles: &'a [Role],
    /// Saved note (None = not loaded / not available).
    pub note: Option<&'a str>,
    /// Notes and profiles need a user account.
    pub is_bot_session: bool,
}

pub enum PopoutAction {
    Close,
    SaveNote(String),
}

/// Draw the card. Returns Close when clicked outside / Esc, or SaveNote
/// when the user presses Save.
pub fn popout_ui(
    ctx: &egui::Context,
    pop: &mut Popout,
    data: CardData<'_>,
    config: &Config,
    pal: &Palette,
) -> Option<PopoutAction> {
    let mut action = None;
    let screen = ctx.content_rect();
    // Open to the right of the anchor if there's room, else to the left.
    let right_room = screen.right() - pop.anchor.right();
    let (pos, pivot) = if right_room > CARD_WIDTH + 16.0 {
        (
            pop.anchor.right_top() + vec2(8.0, 0.0),
            egui::Align2::LEFT_TOP,
        )
    } else {
        (
            pop.anchor.left_top() - vec2(8.0, 0.0),
            egui::Align2::RIGHT_TOP,
        )
    };
    let card_bg = if pal.dark { pal.servers } else { pal.chat };
    let inner_bg = if pal.dark { pal.chat } else { pal.sidebar };

    let area = egui::Area::new(egui::Id::unique("profile_popout"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(card_bg)
                .corner_radius(8)
                .shadow(egui::Shadow {
                    offset: [0, 4],
                    blur: 16,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                })
                .show(ui, |ui| {
                    ui.set_width(CARD_WIDTH);
                    card_contents(ui, pop, &data, card_bg, inner_bg, config, pal, &mut action);
                });
        });

    let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx.cumulative_frame_nr() > pop.opened_frame + 1
        && !ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| area.response.rect.contains(p));
    if action.is_none() && (clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        action = Some(PopoutAction::Close);
    }
    action
}

#[allow(clippy::too_many_arguments)]
fn card_contents(
    ui: &mut egui::Ui,
    pop: &mut Popout,
    data: &CardData<'_>,
    card_bg: Color32,
    inner_bg: Color32,
    config: &Config,
    pal: &Palette,
    action: &mut Option<PopoutAction>,
) {
    let user = &pop.user;
    // ---- Banner: image, profile colour, or a colour from the user id.
    let banner_h = 64.0;
    let (banner, _) = ui.allocate_exact_size(vec2(CARD_WIDTH, banner_h), egui::Sense::hover());
    let banner_col = data
        .profile
        .and_then(|p| p.banner_color)
        .map(members::rgb)
        .unwrap_or_else(|| fallback_banner(&user.id, pal));
    ui.painter().rect_filled(
        banner,
        egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        banner_col,
    );
    if config.modules.images
        && let Some(url) = data.profile.and_then(|p| p.banner_url.as_deref())
    {
        egui::Image::from_uri(url)
            .corner_radius(egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            })
            .paint_at(ui, banner);
    }

    // ---- Big avatar overlapping the banner, with a status dot.
    let av_size = 80.0;
    let av_rect = egui::Rect::from_min_size(
        egui::pos2(banner.left() + 16.0, banner.bottom() - av_size / 2.0),
        vec2(av_size, av_size),
    );
    ui.painter()
        .circle_filled(av_rect.center(), av_size / 2.0 + 5.0, card_bg);
    let mut av_ui = ui.new_child(egui::UiBuilder::new().max_rect(av_rect));
    let av = crate::app::avatar(
        &mut av_ui,
        &user.id,
        &user.name,
        &user.avatar_url.replace("size=64", "size=128"),
        av_size,
        config,
        pal,
    );
    if let Some(s) = data.presence.map(|p| p.status) {
        members::status_dot(ui, av, s, card_bg, pal);
    }
    ui.add_space(av_size / 2.0 + 10.0);

    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 12,
        })
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(inner_bg)
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    inner(ui, pop, data, pal, action);
                });
        });
}

fn inner(
    ui: &mut egui::Ui,
    pop: &mut Popout,
    data: &CardData<'_>,
    pal: &Palette,
    action: &mut Option<PopoutAction>,
) {
    let user = &pop.user;
    let member = data
        .member
        .or_else(|| data.profile.and_then(|p| p.member.as_ref()));
    let display = member.and_then(|m| m.nick.as_deref()).unwrap_or(&user.name);

    // ---- Names
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(display).heading().strong()).truncate());
        if user.bot {
            members::bot_tag(ui, pal);
        }
    });
    let mut handle = if user.username.is_empty() {
        String::new()
    } else {
        user.username.clone()
    };
    if let Some(p) = data.profile.filter(|p| !p.pronouns.is_empty()) {
        if !handle.is_empty() {
            handle.push_str(" • ");
        }
        handle.push_str(&p.pronouns);
    }
    if !handle.is_empty() {
        ui.label(RichText::new(handle).color(pal.text));
    }

    // ---- Status / custom status (nothing if unknown, rather than "offline")
    if let Some(p) = data.presence {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(r.center(), 4.5, p.status.color(pal));
            let text = p.activity.as_deref().unwrap_or(p.status.label());
            ui.label(RichText::new(text));
        });
    }

    ui.add_space(4.0);
    divider(ui, pal);

    // ---- About me
    if let Some(bio) = data.profile.map(|p| p.bio.trim()).filter(|b| !b.is_empty()) {
        heading(ui, pal, "ABOUT ME");
        ui.add(egui::Label::new(RichText::new(bio)).wrap());
        ui.add_space(8.0);
    } else if data.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Loading profile…").small().color(pal.muted));
        });
        ui.add_space(4.0);
    }

    // ---- Member since
    let since = snowflake_date(&user.id);
    let joined = member.and_then(|m| date_from_iso(&m.joined_at));
    if since.is_some() || joined.is_some() {
        heading(ui, pal, "MEMBER SINCE");
        ui.horizontal(|ui| {
            if let Some(d) = &since {
                ui.label(RichText::new("Discord").small().strong().color(pal.accent))
                    .on_hover_text("Discord account created");
                ui.label(RichText::new(d));
            }
            if let Some(d) = &joined {
                ui.label(RichText::new("•").color(pal.muted));
                ui.label(RichText::new("Server").small().strong().color(pal.muted))
                    .on_hover_text("Joined this server");
                ui.label(RichText::new(d));
            }
        });
        ui.add_space(8.0);
    }

    // ---- Roles
    if let Some(m) = member
        && pop.guild_id.is_some()
    {
        let roles = members::member_roles(data.roles, &m.roles);
        heading(
            ui,
            pal,
            if roles.is_empty() {
                "NO ROLES"
            } else {
                "ROLES"
            },
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for r in roles {
                role_chip(ui, r, pal);
            }
        });
        ui.add_space(8.0);
    }

    // ---- Note (user accounts)
    if data.is_bot_session {
        return;
    }
    heading(ui, pal, "NOTE");
    match (&mut pop.note_draft, data.note) {
        (Some(draft), saved) => {
            let resp = ui.add(
                egui::TextEdit::multiline(draft)
                    .hint_text("Click to add a note")
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE),
            );
            let dirty = saved.unwrap_or("") != draft.as_str();
            if dirty {
                ui.horizontal(|ui| {
                    let save = ui.add(pal.primary_button("Save note"));
                    let enter = resp.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                    if save.clicked() || enter {
                        *action = Some(PopoutAction::SaveNote(draft.trim().to_string()));
                    }
                    ui.label(RichText::new("Ctrl+Enter").small().color(pal.muted));
                });
            }
        }
        (None, _) => {
            ui.label(RichText::new("Loading…").small().color(pal.muted));
        }
    }
}

fn heading(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).small().strong().color(pal.muted));
    ui.add_space(2.0);
}

fn divider(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.center().y, egui::Stroke::new(1.0, pal.hover));
}

/// A role "chip": coloured dot + name.
fn role_chip(ui: &mut egui::Ui, r: &Role, pal: &Palette) {
    egui::Frame::new()
        .fill(pal.input)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 14.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 5.0, r.color32().unwrap_or(pal.offline));
                ui.label(RichText::new(&r.name));
            });
        });
}

/// Banner colour when the profile has none: Discord uses the avatar's main
/// colour; we use a stable colour from the user id.
fn fallback_banner(user_id: &str, pal: &Palette) -> Color32 {
    let hue = user_id
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let c: Color32 = egui::ecolor::Hsva::new((hue % 360) as f32 / 360.0, 0.45, 0.55, 1.0).into();
    if pal.dark { c } else { c.gamma_multiply(0.9) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_profile() {
        let p = Profile::from_json(
            &json!({
                "user": { "id": "5", "bio": "old bio", "accent_color": 1, "banner": "abc" },
                "user_profile": { "bio": "hello", "pronouns": "they/them", "theme_colors": [255, 0] },
                "guild_member": { "nick": "Nick", "roles": ["r1"], "joined_at": "2024-03-05T10:00:00Z" },
            }),
            Some("g".into()),
        );
        assert_eq!(p.bio, "hello");
        assert_eq!(p.pronouns, "they/them");
        assert_eq!(p.banner_color, Some(255));
        assert!(p.banner_url.unwrap().contains("/banners/5/abc.png"));
        let m = p.member.unwrap();
        assert_eq!(m.nick.as_deref(), Some("Nick"));
        assert_eq!(m.roles, ["r1"]);
    }

    #[test]
    fn banner_colour_fallbacks() {
        let p = Profile::from_json(
            &json!({ "user": { "id": "1", "banner_color": "#ff0000" } }),
            None,
        );
        assert_eq!(p.banner_color, Some(0xff0000));
        let p = Profile::from_json(&json!({ "user": { "id": "1", "accent_color": 42 } }), None);
        assert_eq!(p.banner_color, Some(42));
        let p = Profile::from_json(&json!({ "user": { "id": "1" } }), None);
        assert_eq!(p.banner_color, None);
        assert!(p.member.is_none());
    }

    #[test]
    fn dates() {
        assert_eq!(
            date_from_iso("2024-03-05T10:00:00.000000+00:00").as_deref(),
            Some("5 Mar 2024")
        );
        assert_eq!(date_from_iso("garbage"), None);
        // Discord's own example snowflake: 2016-04-30.
        assert_eq!(
            snowflake_date("175928847299117063").as_deref(),
            Some("30 Apr 2016")
        );
    }

    #[test]
    fn cache_cap_and_notes() {
        let mut c = ProfileCache::default();
        for i in 0..MAX_PROFILES + 5 {
            c.insert(Profile {
                user_id: i.to_string(),
                ..Profile::default()
            });
        }
        assert_eq!(c.len(), MAX_PROFILES);
        assert!(c.get("0", None).is_none());
        assert!(c.get(&(MAX_PROFILES + 4).to_string(), None).is_some());
        c.set_note("1", "hi".into());
        c.set_note("1", "bye".into());
        assert_eq!(c.note("1"), Some("bye"));
        c.mark_unavailable("9");
        assert!(c.is_unavailable("9"));
    }
}
