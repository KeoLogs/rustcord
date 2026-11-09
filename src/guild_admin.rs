//! Server administration: the shared plumbing for the Roles, Members, Bans,
//! Audit Log and Emoji pages (`server_roles`, `moderation`, `audit_log`,
//! `emoji_manage`).
//!
//! Data in, actions out. The pages never touch the network: each `show`
//! returns an [`AdminAction`]; the caller sends it to the backend
//! (`ToBackend::GuildAdmin(action)`), the backend runs [`execute`] and sends
//! back `ToUi::GuildAdmin { action, result }`, which goes into
//! [`ServerAdmin::on_result`] (or the page's own `on_result`).
//!
//! Reads (lists) are requested when a page is first shown. Every write is a
//! direct user click, and destructive ones (delete role, kick, ban, unban,
//! delete emoji) only after a confirm dialog. Nothing is automatic.
//!
//! REST endpoints (Discord API v10):
//! - roles: `GET/POST /guilds/{g}/roles`, `PATCH /guilds/{g}/roles` (positions),
//!   `PATCH/DELETE /guilds/{g}/roles/{r}`,
//!   `PUT/DELETE /guilds/{g}/members/{u}/roles/{r}`;
//!   `GET /guilds/{g}/roles/{r}/member-ids` is what Discord's own client uses
//!   for "Manage Members" (undocumented, user accounts; up to 100 ids).
//! - members: `GET /guilds/{g}/members?limit=` (bots with the Server Members
//!   intent; user accounts get 403), `GET /guilds/{g}/members/search?query=`
//!   (documented for bots; user accounts may get 403 — then feed results of
//!   an op 8 `query` request into `MembersPage::set_members` instead),
//!   `PATCH /guilds/{g}/members/{u}` (`nick`, `communication_disabled_until`),
//!   `PATCH /guilds/{g}/members/@me` (own nick), `DELETE /guilds/{g}/members/{u}`.
//! - bans: `GET /guilds/{g}/bans?limit=&after=`, `PUT/DELETE /guilds/{g}/bans/{u}`.
//! - audit log: `GET /guilds/{g}/audit-logs?limit=&user_id=&action_type=&before=`.
//! - emoji: `GET/POST /guilds/{g}/emojis`, `PATCH/DELETE /guilds/{g}/emojis/{e}`.
//!
//! Moderation reasons go in the `X-Audit-Log-Reason` header, percent-encoded
//! (max 512 characters).

use std::collections::HashSet;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, vec2};
use serde_json::{Value, json};

use crate::audit_log::{AuditFilter, AuditLogPage};
use crate::emoji_manage::EmojiPage;
use crate::moderation::{BansPage, MembersPage, ModDialogs};
use crate::permissions_table::{self as pt, Actor};
use crate::server_roles::RolesPage;
use crate::theme::Palette;
use crate::types::Guild;

/// Longest audit-log reason Discord accepts.
pub const MAX_REASON: usize = 512;
/// Discord's epoch (2015-01-01) in unix milliseconds.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Looks up a display name (user or channel) for an id.
pub type NameFn<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Everything a page needs to know about the server and me.
pub struct AdminCtx<'a> {
    pub pal: &'a Palette,
    pub actor: Actor<'a>,
    pub guild_name: &'a str,
    /// Images module on: draw avatars / emoji (else initials / names).
    pub show_images: bool,
    pub is_bot: bool,
    /// Server boost tier 0-3 (emoji slots). None: unknown, assume 0.
    pub premium_tier: Option<u8>,
    /// Display name of a user id we already know (member cache), for ids
    /// the pages haven't seen in their own results.
    pub name_of: Option<NameFn<'a>>,
    /// Channel name for an id (audit log targets).
    pub channel_name: Option<NameFn<'a>>,
}

impl<'a> AdminCtx<'a> {
    pub fn from_guild(pal: &'a Palette, guild: &'a Guild, my_user_id: &'a str) -> Self {
        Self {
            pal,
            actor: Actor {
                guild_id: &guild.id,
                owner_id: &guild.owner_id,
                user_id: my_user_id,
                roles: &guild.roles,
                my_roles: &guild.my_roles,
            },
            guild_name: &guild.name,
            show_images: true,
            is_bot: false,
            premium_tier: None,
            name_of: None,
            channel_name: None,
        }
    }

    pub fn guild_id(&self) -> &'a str {
        self.actor.guild_id
    }

    pub fn known_name(&self, user_id: &str) -> Option<String> {
        self.name_of.and_then(|f| f(user_id))
    }
}

/// Which list a member search fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFor {
    MembersPage,
    RoleMembers,
}

/// One REST operation. Built only from user clicks (writes) or from a page
/// being opened (reads).
#[derive(Debug, Clone, PartialEq)]
pub enum AdminAction {
    // Roles
    LoadRoles {
        guild_id: String,
    },
    CreateRole {
        guild_id: String,
        name: String,
        color: u32,
    },
    /// Only the changed fields (`server_roles::RoleDraft::patch`).
    EditRole {
        guild_id: String,
        role_id: String,
        patch: Value,
    },
    /// `(role id, new position)` for the roles that moved.
    ReorderRoles {
        guild_id: String,
        positions: Vec<(String, i64)>,
    },
    DeleteRole {
        guild_id: String,
        role_id: String,
    },
    LoadRoleMemberIds {
        guild_id: String,
        role_id: String,
    },
    AddMemberRole {
        guild_id: String,
        user_id: String,
        role_id: String,
    },
    RemoveMemberRole {
        guild_id: String,
        user_id: String,
        role_id: String,
    },
    // Members
    ListMembers {
        guild_id: String,
        after: Option<String>,
    },
    SearchMembers {
        guild_id: String,
        query: String,
        purpose: SearchFor,
    },
    /// `None` resets the nickname.
    SetNickname {
        guild_id: String,
        user_id: String,
        nick: Option<String>,
        is_me: bool,
    },
    /// `until`: ISO-8601 end of the timeout; `None` removes it.
    Timeout {
        guild_id: String,
        user_id: String,
        until: Option<String>,
        reason: String,
    },
    Kick {
        guild_id: String,
        user_id: String,
        reason: String,
    },
    Ban {
        guild_id: String,
        user_id: String,
        delete_message_seconds: u32,
        reason: String,
    },
    // Bans
    LoadBans {
        guild_id: String,
        after: Option<String>,
    },
    Unban {
        guild_id: String,
        user_id: String,
    },
    // Audit log
    LoadAuditLog {
        guild_id: String,
        filter: AuditFilter,
        before: Option<String>,
    },
    // Emoji
    LoadEmojis {
        guild_id: String,
    },
    /// Native file picker (backend, off the UI thread); answers with
    /// `AdminOutcome::File`.
    PickEmojiFile,
    /// `image` is a `data:image/...;base64,` URI.
    CreateEmoji {
        guild_id: String,
        name: String,
        image: String,
    },
    RenameEmoji {
        guild_id: String,
        emoji_id: String,
        name: String,
    },
    DeleteEmoji {
        guild_id: String,
        emoji_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    fn reqwest(self) -> reqwest::Method {
        match self {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
        }
    }
}

/// What [`execute`] sends (pure, so it can be tested).
#[derive(Debug, Clone, PartialEq)]
pub struct RestRequest {
    pub method: Method,
    /// Relative to `https://discord.com/api/v10`, query included.
    pub path: String,
    pub body: Option<Value>,
    /// Already percent-encoded for `X-Audit-Log-Reason`.
    pub reason: Option<String>,
}

fn req(method: Method, path: String, body: Option<Value>) -> RestRequest {
    RestRequest {
        method,
        path,
        body,
        reason: None,
    }
}

impl AdminAction {
    /// The REST call for this action (`PickEmojiFile`: none).
    pub fn request(&self) -> Option<RestRequest> {
        use AdminAction::*;
        use Method::*;
        Some(match self {
            LoadRoles { guild_id } => req(Get, format!("/guilds/{guild_id}/roles"), None),
            CreateRole {
                guild_id,
                name,
                color,
            } => req(
                Post,
                format!("/guilds/{guild_id}/roles"),
                Some(json!({ "name": name, "color": color })),
            ),
            EditRole {
                guild_id,
                role_id,
                patch,
            } => req(
                Patch,
                format!("/guilds/{guild_id}/roles/{role_id}"),
                Some(patch.clone()),
            ),
            ReorderRoles {
                guild_id,
                positions,
            } => req(
                Patch,
                format!("/guilds/{guild_id}/roles"),
                Some(Value::Array(
                    positions
                        .iter()
                        .map(|(id, pos)| json!({ "id": id, "position": pos }))
                        .collect(),
                )),
            ),
            DeleteRole { guild_id, role_id } => {
                req(Delete, format!("/guilds/{guild_id}/roles/{role_id}"), None)
            }
            LoadRoleMemberIds { guild_id, role_id } => req(
                Get,
                format!("/guilds/{guild_id}/roles/{role_id}/member-ids"),
                None,
            ),
            AddMemberRole {
                guild_id,
                user_id,
                role_id,
            } => req(
                Put,
                format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"),
                None,
            ),
            RemoveMemberRole {
                guild_id,
                user_id,
                role_id,
            } => req(
                Delete,
                format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"),
                None,
            ),
            ListMembers { guild_id, after } => {
                let mut path = format!("/guilds/{guild_id}/members?limit=100");
                if let Some(a) = after {
                    path.push_str(&format!("&after={}", encode_component(a)));
                }
                req(Get, path, None)
            }
            SearchMembers {
                guild_id, query, ..
            } => req(
                Get,
                format!(
                    "/guilds/{guild_id}/members/search?query={}&limit=100",
                    encode_component(query.trim())
                ),
                None,
            ),
            SetNickname {
                guild_id,
                user_id,
                nick,
                is_me,
            } => {
                let who = if *is_me { "@me" } else { user_id.as_str() };
                let nick = nick.as_deref().map(str::trim).filter(|n| !n.is_empty());
                req(
                    Patch,
                    format!("/guilds/{guild_id}/members/{who}"),
                    Some(json!({ "nick": nick })),
                )
            }
            Timeout {
                guild_id,
                user_id,
                until,
                reason,
            } => RestRequest {
                reason: encode_reason(reason),
                ..req(
                    Patch,
                    format!("/guilds/{guild_id}/members/{user_id}"),
                    Some(json!({ "communication_disabled_until": until })),
                )
            },
            Kick {
                guild_id,
                user_id,
                reason,
            } => RestRequest {
                reason: encode_reason(reason),
                ..req(
                    Delete,
                    format!("/guilds/{guild_id}/members/{user_id}"),
                    None,
                )
            },
            Ban {
                guild_id,
                user_id,
                delete_message_seconds,
                reason,
            } => RestRequest {
                reason: encode_reason(reason),
                ..req(
                    Put,
                    format!("/guilds/{guild_id}/bans/{user_id}"),
                    Some(json!({ "delete_message_seconds": delete_message_seconds })),
                )
            },
            LoadBans { guild_id, after } => {
                let mut path = format!("/guilds/{guild_id}/bans?limit=1000");
                if let Some(a) = after {
                    path.push_str(&format!("&after={}", encode_component(a)));
                }
                req(Get, path, None)
            }
            Unban { guild_id, user_id } => {
                req(Delete, format!("/guilds/{guild_id}/bans/{user_id}"), None)
            }
            LoadAuditLog {
                guild_id,
                filter,
                before,
            } => {
                let mut path = format!("/guilds/{guild_id}/audit-logs?limit=50");
                if let Some(u) = &filter.user_id {
                    path.push_str(&format!("&user_id={}", encode_component(u)));
                }
                if let Some(t) = filter.action_type {
                    path.push_str(&format!("&action_type={t}"));
                }
                if let Some(b) = before {
                    path.push_str(&format!("&before={}", encode_component(b)));
                }
                req(Get, path, None)
            }
            LoadEmojis { guild_id } => req(Get, format!("/guilds/{guild_id}/emojis"), None),
            PickEmojiFile => return None,
            CreateEmoji {
                guild_id,
                name,
                image,
            } => req(
                Post,
                format!("/guilds/{guild_id}/emojis"),
                Some(json!({ "name": name, "image": image, "roles": [] })),
            ),
            RenameEmoji {
                guild_id,
                emoji_id,
                name,
            } => req(
                Patch,
                format!("/guilds/{guild_id}/emojis/{emoji_id}"),
                Some(json!({ "name": name })),
            ),
            DeleteEmoji { guild_id, emoji_id } => req(
                Delete,
                format!("/guilds/{guild_id}/emojis/{emoji_id}"),
                None,
            ),
        })
    }

    /// For error messages: "Banning member failed: …".
    pub fn label(&self) -> &'static str {
        use AdminAction::*;
        match self {
            LoadRoles { .. } => "Loading roles",
            CreateRole { .. } => "Creating role",
            EditRole { .. } => "Saving role",
            ReorderRoles { .. } => "Reordering roles",
            DeleteRole { .. } => "Deleting role",
            LoadRoleMemberIds { .. } => "Loading role members",
            AddMemberRole { .. } => "Adding role",
            RemoveMemberRole { .. } => "Removing role",
            ListMembers { .. } => "Loading members",
            SearchMembers { .. } => "Searching members",
            SetNickname { .. } => "Changing nickname",
            Timeout { .. } => "Timing out member",
            Kick { .. } => "Kicking member",
            Ban { .. } => "Banning member",
            LoadBans { .. } => "Loading bans",
            Unban { .. } => "Revoking ban",
            LoadAuditLog { .. } => "Loading audit log",
            LoadEmojis { .. } => "Loading emoji",
            PickEmojiFile => "Choosing a file",
            CreateEmoji { .. } => "Uploading emoji",
            RenameEmoji { .. } => "Renaming emoji",
            DeleteEmoji { .. } => "Deleting emoji",
        }
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq)]
pub enum AdminOutcome {
    /// The response body (`Null` for 204).
    Json(Value),
    /// `PickEmojiFile`: the chosen file (already checked against the size
    /// limit).
    File { name: String, bytes: Vec<u8> },
    /// File picker closed without a choice.
    Cancelled,
}

/// Run an action (backend side). Results go back as
/// `ToUi::GuildAdmin { action, result: result.map_err(|e| e.to_string()) }`.
pub async fn execute(api: &crate::api::Api, action: &AdminAction) -> anyhow::Result<AdminOutcome> {
    if let AdminAction::PickEmojiFile = action {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Upload emoji")
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "avif"])
            .pick_file()
            .await
        else {
            return Ok(AdminOutcome::Cancelled);
        };
        let path = file.path().to_path_buf();
        let len = tokio::fs::metadata(&path).await?.len();
        if len > crate::emoji_manage::MAX_EMOJI_BYTES as u64 {
            anyhow::bail!("{}", crate::emoji_manage::too_big_message(len as usize));
        }
        let bytes = tokio::fs::read(&path).await?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok(AdminOutcome::File { name, bytes });
    }
    let Some(r) = action.request() else {
        return Ok(AdminOutcome::Cancelled);
    };
    let value = api
        .admin_request(
            r.method.reqwest(),
            &r.path,
            r.body.as_ref(),
            r.reason.as_deref(),
            action.label(),
        )
        .await?;
    Ok(AdminOutcome::Json(value))
}

/// Percent-encode for a URL query value or header (RFC 3986 unreserved
/// characters stay as they are).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Trimmed, at most 512 characters, encoded; `None` if empty.
pub fn encode_reason(reason: &str) -> Option<String> {
    let r: String = reason.trim().chars().take(MAX_REASON).collect();
    (!r.is_empty()).then(|| encode_component(&r))
}

/// Unix seconds a snowflake was created at.
pub fn snowflake_time(id: &str) -> i64 {
    id.parse::<u64>()
        .map(|n| (((n >> 22) + DISCORD_EPOCH_MS) / 1000) as i64)
        .unwrap_or(0)
}

/// A list a page loads from the API.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn get(&self) -> Option<&T> {
        match self {
            Load::Loaded(t) => Some(t),
            _ => None,
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        match self {
            Load::Loaded(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Load::Idle)
    }
}

/// Image URIs a page has drawn, freed with `forget` when the page closes.
#[derive(Debug, Default, Clone)]
pub struct ImageUris(HashSet<String>);

impl ImageUris {
    pub fn note(&mut self, uri: &str) {
        if !self.0.contains(uri) {
            self.0.insert(uri.to_string());
        }
    }

    pub fn forget(&mut self, ctx: &egui::Context) {
        for uri in self.0.drain() {
            ctx.forget_image(&uri);
        }
    }
}

// ---------------------------------------------------------------------------
// The bundle the settings window holds
// ---------------------------------------------------------------------------

/// The admin sections, in Discord's sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Roles,
    Emoji,
    AuditLog,
    Bans,
    Members,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Roles,
        Section::Emoji,
        Section::AuditLog,
        Section::Bans,
        Section::Members,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Roles => "Roles",
            Section::Emoji => "Emoji",
            Section::AuditLog => "Audit Log",
            Section::Bans => "Bans",
            Section::Members => "Members",
        }
    }

    /// Show this section in the sidebar for me?
    pub fn visible(self, actor: &Actor) -> bool {
        match self {
            Section::Roles => actor.has(pt::MANAGE_ROLES),
            Section::Emoji => actor.can_upload_emoji(),
            Section::AuditLog => actor.has(pt::VIEW_AUDIT_LOG),
            Section::Bans => actor.has(pt::BAN_MEMBERS),
            Section::Members => {
                actor.has(pt::KICK_MEMBERS)
                    || actor.has(pt::BAN_MEMBERS)
                    || actor.has(pt::MODERATE_MEMBERS)
                    || actor.has(pt::MANAGE_ROLES)
                    || actor.has(pt::MANAGE_NICKNAMES)
            }
        }
    }
}

/// All admin pages for one server, plus the moderation dialogs. Create a
/// new one when the server settings window opens; call `close` when it
/// closes (frees images).
#[derive(Default)]
pub struct ServerAdmin {
    pub roles: RolesPage,
    pub members: MembersPage,
    pub bans: BansPage,
    pub audit: AuditLogPage,
    pub emoji: EmojiPage,
    /// Nickname / timeout / kick / ban dialogs (also used by the member
    /// context menu outside the settings window).
    pub dialogs: ModDialogs,
}

impl ServerAdmin {
    /// Draw one section (and any open moderation dialog).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &AdminCtx,
        section: Section,
    ) -> Option<AdminAction> {
        let action = match section {
            Section::Roles => self.roles.show(ui, ctx),
            Section::Members => {
                let (action, mod_action) = self.members.show(ui, ctx);
                match mod_action {
                    Some(m) => self.dialogs.handle(m, ctx.guild_id()).or(action),
                    None => action,
                }
            }
            Section::Bans => self.bans.show(ui, ctx),
            Section::AuditLog => self.audit.show(ui, ctx),
            Section::Emoji => self.emoji.show(ui, ctx),
        };
        let dialog = self.dialogs.show(ui.ctx(), ctx);
        action.or(dialog)
    }

    /// Feed a backend answer to every page that cares.
    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        self.roles.on_result(action, result);
        self.members.on_result(action, result);
        self.bans.on_result(action, result);
        self.audit.on_result(action, result);
        self.emoji.on_result(action, result);
    }

    /// Free the images the pages drew.
    pub fn close(&mut self, ctx: &egui::Context) {
        self.members.forget_images(ctx);
        self.bans.forget_images(ctx);
        self.emoji.forget_images(ctx);
        self.roles.forget_images(ctx);
    }
}

// ---------------------------------------------------------------------------
// Shared widgets
// ---------------------------------------------------------------------------

/// Small vector icons (no emoji as icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Close,
    Up,
    Down,
    Grip,
    More,
    Search,
    Plus,
    Shield,
    Back,
}

/// Paint `icon` centred in `rect`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke = Stroke::new(1.6, color);
    let p = |x: f32, y: f32| Pos2::new(c.x + x * s, c.y + y * s);
    match icon {
        Icon::Close => {
            painter.line_segment([p(-0.55, -0.55), p(0.55, 0.55)], stroke);
            painter.line_segment([p(0.55, -0.55), p(-0.55, 0.55)], stroke);
        }
        Icon::Up => {
            painter.line_segment([p(-0.6, 0.3), p(0.0, -0.35)], stroke);
            painter.line_segment([p(0.0, -0.35), p(0.6, 0.3)], stroke);
        }
        Icon::Down => {
            painter.line_segment([p(-0.6, -0.3), p(0.0, 0.35)], stroke);
            painter.line_segment([p(0.0, 0.35), p(0.6, -0.3)], stroke);
        }
        Icon::Back => {
            painter.line_segment([p(0.3, -0.6), p(-0.35, 0.0)], stroke);
            painter.line_segment([p(-0.35, 0.0), p(0.3, 0.6)], stroke);
        }
        Icon::Grip => {
            for dx in [-0.25, 0.25] {
                for dy in [-0.5, 0.0, 0.5] {
                    painter.circle_filled(p(dx, dy), 1.4, color);
                }
            }
        }
        Icon::More => {
            for dx in [-0.55, 0.0, 0.55] {
                painter.circle_filled(p(dx, 0.0), 1.7, color);
            }
        }
        Icon::Search => {
            painter.circle_stroke(p(-0.15, -0.15), s * 0.45, stroke);
            painter.line_segment([p(0.2, 0.2), p(0.65, 0.65)], stroke);
        }
        Icon::Plus => {
            painter.line_segment([p(-0.6, 0.0), p(0.6, 0.0)], stroke);
            painter.line_segment([p(0.0, -0.6), p(0.0, 0.6)], stroke);
        }
        Icon::Shield => {
            let pts = vec![
                p(0.0, -0.7),
                p(0.6, -0.45),
                p(0.5, 0.25),
                p(0.0, 0.7),
                p(-0.5, 0.25),
                p(-0.6, -0.45),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
    }
}

/// A square icon button with a hover background and tooltip.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    tip: &str,
    pal: &Palette,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), sense);
    if ui.is_rect_visible(rect) {
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, 4, pal.hover);
        }
        let col = if !enabled {
            pal.muted_text()
        } else if resp.hovered() {
            pal.strong
        } else {
            pal.muted
        };
        paint_icon(ui.painter(), rect.shrink(5.0), icon, col);
    }
    resp.on_hover_text(tip)
}

/// Discord's toggle switch. Returns true when flipped.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, enabled: bool, pal: &Palette) -> egui::Response {
    let size = vec2(40.0, 24.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let off_col = pal.offline;
        let track = egui::lerp(egui::Rgba::from(off_col)..=egui::Rgba::from(pal.online), t);
        let mut track = Color32::from(track);
        if !enabled {
            track = track.gamma_multiply(0.5);
        }
        ui.painter()
            .rect_filled(rect, CornerRadius::same(12), track);
        let x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
        ui.painter()
            .circle_filled(Pos2::new(x, rect.center().y), 9.0, Color32::WHITE);
    }
    resp
}

/// Single-line search field with a magnifier.
pub fn search_box(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    pal: &Palette,
) -> egui::Response {
    egui::Frame::new()
        .fill(pal.servers)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .frame(egui::Frame::NONE)
                        .desired_width(width - 34.0),
                );
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                paint_icon(ui.painter(), r.shrink(2.0), Icon::Search, pal.muted);
                resp
            })
            .inner
        })
        .inner
}

/// A red filled button.
pub fn danger_button(pal: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE))
        .fill(pal.danger)
        .corner_radius(4)
}

/// A red-tinted error line.
pub fn error_line(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    egui::Frame::new()
        .fill(pal.danger.gamma_multiply(0.15))
        .stroke(Stroke::new(1.0, pal.danger))
        .corner_radius(4)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(pal.text));
        });
}

/// A confirm dialog. `Some(true)`: confirmed, `Some(false)`: cancelled.
pub fn confirm(
    ctx: &egui::Context,
    pal: &Palette,
    id: &str,
    title: &str,
    body: &str,
    confirm_label: &str,
) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique(("rc_admin_confirm", id))).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(title);
        ui.add_space(4.0);
        ui.label(body);
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(danger_button(pal, confirm_label)).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    answer
}

/// A round avatar (or coloured initial when images are off / no URL).
pub fn avatar(
    ui: &mut egui::Ui,
    size: f32,
    user_id: &str,
    name: &str,
    url: &str,
    ctx: &AdminCtx,
    uris: &mut ImageUris,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    if ctx.show_images && !url.is_empty() {
        ui.painter()
            .circle_filled(rect.center(), size / 2.0, ctx.pal.hover);
        uris.note(url);
        egui::Image::from_uri(url.to_string())
            .corner_radius(size / 2.0)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
    } else {
        let hue = user_id
            .bytes()
            .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let col = egui::ecolor::Hsva::new((hue % 360) as f32 / 360.0, 0.45, 0.6, 1.0);
        ui.painter().circle_filled(rect.center(), size / 2.0, col);
        let letter = name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            egui::FontId::proportional(size * 0.45),
            Color32::WHITE,
        );
    }
}

/// A small filled circle in a role's colour (muted grey for no colour).
pub fn role_dot(ui: &mut egui::Ui, color: u32, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    let col = if color == 0 {
        pal.muted
    } else {
        crate::members::rgb(color)
    };
    ui.painter().circle_filled(r.center(), 6.0, col);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> String {
        "1".to_string()
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_component("a b/ü?&"), "a%20b%2F%C3%BC%3F%26");
        assert_eq!(encode_component("abc-_.~"), "abc-_.~");
        assert_eq!(encode_reason("  "), None);
        assert_eq!(encode_reason(" spam "), Some("spam".into()));
        let long = "x".repeat(600);
        assert_eq!(encode_reason(&long).unwrap().len(), MAX_REASON);
    }

    #[test]
    fn snowflakes() {
        // Discord's docs example: 175928847299117063 → 2016-04-30 11:18:25.796 UTC.
        assert_eq!(snowflake_time("175928847299117063"), 1_462_015_105);
        assert_eq!(snowflake_time("junk"), 0);
    }

    #[test]
    fn requests() {
        let r = AdminAction::Kick {
            guild_id: g(),
            user_id: "5".into(),
            reason: "spam & ads".into(),
        }
        .request()
        .unwrap();
        assert_eq!(r.method, Method::Delete);
        assert_eq!(r.path, "/guilds/1/members/5");
        assert_eq!(r.reason.as_deref(), Some("spam%20%26%20ads"));
        assert_eq!(r.body, None);

        let r = AdminAction::Ban {
            guild_id: g(),
            user_id: "5".into(),
            delete_message_seconds: 3600,
            reason: String::new(),
        }
        .request()
        .unwrap();
        assert_eq!(r.method, Method::Put);
        assert_eq!(r.path, "/guilds/1/bans/5");
        assert_eq!(r.body, Some(json!({ "delete_message_seconds": 3600 })));
        assert_eq!(r.reason, None);

        let r = AdminAction::SetNickname {
            guild_id: g(),
            user_id: "5".into(),
            nick: Some("  ".into()),
            is_me: true,
        }
        .request()
        .unwrap();
        assert_eq!(r.path, "/guilds/1/members/@me");
        assert_eq!(r.body, Some(json!({ "nick": null })));

        let r = AdminAction::Timeout {
            guild_id: g(),
            user_id: "5".into(),
            until: None,
            reason: String::new(),
        }
        .request()
        .unwrap();
        assert_eq!(r.method, Method::Patch);
        assert_eq!(
            r.body,
            Some(json!({ "communication_disabled_until": null }))
        );

        let r = AdminAction::ReorderRoles {
            guild_id: g(),
            positions: vec![("a".into(), 2), ("b".into(), 1)],
        }
        .request()
        .unwrap();
        assert_eq!(r.path, "/guilds/1/roles");
        assert_eq!(
            r.body,
            Some(json!([{ "id": "a", "position": 2 }, { "id": "b", "position": 1 }]))
        );

        let r = AdminAction::SearchMembers {
            guild_id: g(),
            query: " sam k ".into(),
            purpose: SearchFor::MembersPage,
        }
        .request()
        .unwrap();
        assert_eq!(r.path, "/guilds/1/members/search?query=sam%20k&limit=100");

        let r = AdminAction::LoadAuditLog {
            guild_id: g(),
            filter: AuditFilter {
                user_id: Some("9".into()),
                action_type: Some(22),
            },
            before: Some("100".into()),
        }
        .request()
        .unwrap();
        assert_eq!(
            r.path,
            "/guilds/1/audit-logs?limit=50&user_id=9&action_type=22&before=100"
        );
        assert!(AdminAction::PickEmojiFile.request().is_none());

        let r = AdminAction::AddMemberRole {
            guild_id: g(),
            user_id: "5".into(),
            role_id: "7".into(),
        }
        .request()
        .unwrap();
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Put, "/guilds/1/members/5/roles/7")
        );
    }
}
