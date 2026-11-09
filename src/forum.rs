//! Forum and media channels (types 15 / 16): channel settings (tags,
//! default sort / layout / reaction), the post-list query and pagination
//! state, and the UI: [`ForumView`] (header with "New Post", tag filter
//! chips, sort toggle, post cards, "Load more") and the "New Post" dialog.
//!
//! Posts are public threads (see [`crate::threads`]); they live in the same
//! [`ThreadStore`]. Data in, actions out: widgets return a [`ThreadAction`].
//!
//! # Listing posts (protocol notes)
//!
//! - **User accounts**: Discord's own client lists forum posts with
//!   `GET /channels/{forum}/threads/search?archived=…&sort_by=…&sort_order=desc
//!   &limit=25&offset=…[&tag=…&tag_setting=match_some]`. This endpoint is not
//!   in Discord's public docs; the answer is
//!   `{threads, members, has_more, total_results, first_messages}`. Whether
//!   `archived=false` lists active posts reliably is unverified (the official
//!   client also gets active posts from the gateway's THREAD_LIST_SYNC). We
//!   page active posts first, then archived ones (see
//!   [`ForumListing::next_query`]).
//! - **Bots**: the search endpoint isn't for bots. Active posts arrive in
//!   GUILD_CREATE `threads` (or `GET /guilds/{id}/threads/active`), archived
//!   ones from `GET /channels/{forum}/threads/archived/public?before=…`
//!   ([`ThreadStore::oldest_archived_iso`] gives `before`).

use eframe::egui::{self, Color32, RichText, pos2, vec2};
use serde_json::Value;

use crate::icons;
use crate::theme::{Bold, Palette};
use crate::threads::{
    Ctx, DialogOutcome, Thread, ThreadAction, ThreadStore, open_action, plural, thread_menu,
};

/// Forum channel: a list of posts (threads).
pub const CHANNEL_FORUM: u64 = 15;
/// Media channel: a forum whose posts need an attachment.
pub const CHANNEL_MEDIA: u64 = 16;

/// Channel `flags` bit: posts must have at least one tag.
pub const FLAG_REQUIRE_TAG: u64 = 1 << 4;

/// Most tags a post can have.
pub const MAX_APPLIED_TAGS: usize = 5;
/// Posts per page (Discord's client uses 25).
pub const PAGE_SIZE: u8 = 25;

const THUMB: f32 = 72.0;

pub fn is_forum_kind(kind: u64) -> bool {
    kind == CHANNEL_FORUM || kind == CHANNEL_MEDIA
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// A tag from a forum's `available_tags`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumTag {
    pub id: String,
    pub name: String,
    /// Only people with Manage Threads can apply it.
    pub moderated: bool,
    /// Custom server emoji id, or…
    pub emoji_id: Option<String>,
    /// …a Unicode emoji.
    pub emoji_name: Option<String>,
}

impl ForumTag {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: str_of(v, "id"),
            name: str_of(v, "name"),
            moderated: v.get("moderated").and_then(Value::as_bool).unwrap_or(false),
            emoji_id: opt_str(v, "emoji_id"),
            emoji_name: opt_str(v, "emoji_name"),
        }
    }

    /// Image URL for the tag's emoji (custom: Discord CDN, Unicode: Twemoji).
    pub fn emoji_url(&self) -> Option<String> {
        emoji_url(self.emoji_id.as_deref(), self.emoji_name.as_deref())
    }
}

/// Emoji image URL from an (emoji_id, emoji_name) pair.
pub fn emoji_url(id: Option<&str>, name: Option<&str>) -> Option<String> {
    if let Some(id) = id {
        return Some(crate::emoji::custom_emoji_url(id, false));
    }
    name.filter(|n| !n.is_empty())
        .map(crate::emoji::twemoji_url)
}

/// `default_reaction_emoji`: the reaction button shown on post cards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultReaction {
    pub emoji_id: Option<String>,
    pub emoji_name: Option<String>,
}

/// How posts are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SortOrder {
    /// Newest message first (Discord's default).
    #[default]
    LatestActivity,
    /// Newest post first.
    CreationDate,
}

impl SortOrder {
    /// `default_sort_order`: 0 latest activity, 1 creation date, null unset.
    pub fn from_api(v: Option<u64>) -> Self {
        match v {
            Some(1) => Self::CreationDate,
            _ => Self::LatestActivity,
        }
    }

    /// `sort_by` value for the search endpoint.
    pub fn search_param(self) -> &'static str {
        match self {
            Self::LatestActivity => "last_message_time",
            Self::CreationDate => "creation_time",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LatestActivity => "Recently Active",
            Self::CreationDate => "Date Posted",
        }
    }
}

/// `default_forum_layout`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ForumLayout {
    #[default]
    NotSet,
    List,
    Gallery,
}

impl ForumLayout {
    pub fn from_api(v: Option<u64>) -> Self {
        match v {
            Some(1) => Self::List,
            Some(2) => Self::Gallery,
            _ => Self::NotSet,
        }
    }
}

/// A forum or media channel's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumChannel {
    pub id: String,
    pub guild_id: String,
    pub name: String,
    /// 15 forum, 16 media.
    pub kind: u64,
    /// Post guidelines (the channel topic).
    pub guidelines: String,
    pub available_tags: Vec<ForumTag>,
    pub default_reaction: Option<DefaultReaction>,
    pub default_sort_order: SortOrder,
    pub default_layout: ForumLayout,
    pub flags: u64,
    /// Slowmode for new posts, seconds.
    pub rate_limit_per_user: u32,
}

impl ForumChannel {
    /// Parse a channel object (GUILD_CREATE `channels`, CHANNEL_UPDATE…).
    /// `guild_id` fills in the guild when the payload has none.
    pub fn from_json(v: &Value, guild_id: Option<&str>) -> Self {
        let own_guild = str_of(v, "guild_id");
        Self {
            id: str_of(v, "id"),
            guild_id: if own_guild.is_empty() {
                guild_id.unwrap_or_default().to_string()
            } else {
                own_guild
            },
            name: str_of(v, "name"),
            kind: v
                .get("type")
                .and_then(Value::as_u64)
                .unwrap_or(CHANNEL_FORUM),
            guidelines: str_of(v, "topic").trim().to_string(),
            available_tags: v
                .get("available_tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(ForumTag::from_json).collect())
                .unwrap_or_default(),
            default_reaction: v
                .get("default_reaction_emoji")
                .filter(|r| r.is_object())
                .map(|r| DefaultReaction {
                    emoji_id: opt_str(r, "emoji_id"),
                    emoji_name: opt_str(r, "emoji_name"),
                })
                .filter(|r| r.emoji_id.is_some() || r.emoji_name.is_some()),
            default_sort_order: SortOrder::from_api(
                v.get("default_sort_order").and_then(Value::as_u64),
            ),
            default_layout: ForumLayout::from_api(
                v.get("default_forum_layout").and_then(Value::as_u64),
            ),
            flags: v.get("flags").and_then(Value::as_u64).unwrap_or(0),
            rate_limit_per_user: v
                .get("default_thread_rate_limit_per_user")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
        }
    }

    pub fn require_tag(&self) -> bool {
        self.flags & FLAG_REQUIRE_TAG != 0
    }

    pub fn is_media(&self) -> bool {
        self.kind == CHANNEL_MEDIA
    }

    pub fn tag(&self, id: &str) -> Option<&ForumTag> {
        self.available_tags.iter().find(|t| t.id == id)
    }
}

/// One forum list request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForumQuery {
    /// false: active posts, true: archived posts.
    pub archived: bool,
    pub sort: SortOrder,
    /// Only posts with at least one of these tags (empty = all).
    pub tags: Vec<String>,
    pub offset: u32,
    pub limit: u8,
}

impl ForumQuery {
    pub fn first(sort: SortOrder, tags: Vec<String>) -> Self {
        Self {
            archived: false,
            sort,
            tags,
            offset: 0,
            limit: PAGE_SIZE,
        }
    }

    /// Query string for `GET /channels/{forum}/threads/search`.
    pub fn search_params(&self) -> String {
        let mut s = format!(
            "archived={}&sort_by={}&sort_order=desc&limit={}&offset={}",
            self.archived,
            self.sort.search_param(),
            self.limit.clamp(1, 25),
            self.offset
        );
        if !self.tags.is_empty() {
            for t in &self.tags {
                s.push_str("&tag=");
                s.push_str(t);
            }
            s.push_str("&tag_setting=match_some");
        }
        s
    }

    /// Same list (filters and phase), ignoring the page.
    fn same_list(&self, other: &Self) -> bool {
        self.archived == other.archived && self.sort == other.sort && self.tags == other.tags
    }
}

/// Pagination state of one forum's post list (kept in the [`ThreadStore`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForumListing {
    /// The request in flight, or the last one answered.
    pub query: Option<ForumQuery>,
    pub loading: bool,
    pub next_offset: u32,
    pub has_more: bool,
    pub total: Option<u32>,
    pub error: Option<String>,
}

impl ForumListing {
    pub(crate) fn begin(&mut self, q: &ForumQuery) {
        if !self.query.as_ref().is_some_and(|old| old.same_list(q)) {
            self.next_offset = 0;
            self.has_more = true;
            self.total = None;
        }
        self.query = Some(q.clone());
        self.loading = true;
        self.error = None;
    }

    pub(crate) fn finish(
        &mut self,
        q: &ForumQuery,
        count: usize,
        has_more: bool,
        total: Option<u32>,
    ) {
        if self.query.as_ref() != Some(q) {
            return; // an older request; a newer one is pending
        }
        self.loading = false;
        self.next_offset = q.offset + count as u32;
        self.has_more = has_more;
        if total.is_some() {
            self.total = total;
        }
    }

    /// The next request for this sort / tag filter: the first page if the
    /// filter changed, the next page, then archived posts after the last
    /// active page. None while loading, after an error, or at the end.
    pub fn next_query(&self, sort: SortOrder, tags: &[String]) -> Option<ForumQuery> {
        let Some(q) = &self.query else {
            return Some(ForumQuery::first(sort, tags.to_vec()));
        };
        if q.sort != sort || q.tags != tags {
            return Some(ForumQuery::first(sort, tags.to_vec()));
        }
        if self.loading || self.error.is_some() {
            return None;
        }
        if self.has_more {
            return Some(ForumQuery {
                offset: self.next_offset,
                ..q.clone()
            });
        }
        (!q.archived).then(|| ForumQuery {
            archived: true,
            offset: 0,
            ..q.clone()
        })
    }
}

// ---------------------------------------------------------------------------
// Forum view
// ---------------------------------------------------------------------------

/// UI state of an open forum channel. Keep one per open forum; it resets
/// itself when shown with a different forum.
#[derive(Debug, Clone, Default)]
pub struct ForumView {
    forum_id: String,
    /// None: the forum's default sort.
    sort: Option<SortOrder>,
    tags: Vec<String>,
    /// Last LoadMore emitted (so a request isn't repeated every frame).
    last_request: Option<ForumQuery>,
    dialog: Option<PostDialog>,
}

impl ForumView {
    pub fn new(forum_id: &str) -> Self {
        Self {
            forum_id: forum_id.to_string(),
            ..Default::default()
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        forum: &ForumChannel,
        store: &ThreadStore,
        ctx: &Ctx,
    ) -> Option<ThreadAction> {
        if self.forum_id != forum.id {
            *self = Self::new(&forum.id);
        }
        let pal = ctx.pal;
        let mut action = None;

        if let Some(d) = &mut self.dialog {
            match d.show(ui.ctx(), pal, forum, ctx.can_manage_threads) {
                DialogOutcome::Open => {}
                DialogOutcome::Cancelled => self.dialog = None,
                DialogOutcome::Submit(a) => {
                    action = Some(a);
                    self.dialog = None;
                }
            }
        }

        let sort = self.sort.unwrap_or(forum.default_sort_order);
        let default_listing = ForumListing::default();
        let listing = store.listing(&forum.id).unwrap_or(&default_listing);

        // First page, or the filter changed: fetch right away.
        let filter_changed = listing
            .query
            .as_ref()
            .is_none_or(|q| q.sort != sort || q.tags != self.tags);
        if filter_changed {
            self.request(&forum.id, listing.next_query(sort, &self.tags), &mut action);
        }

        // Header: name, guidelines, New Post.
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::CHATS).size(20.0).color(pal.muted));
            ui.label(
                RichText::new(&forum.name)
                    .size(20.0)
                    .bold()
                    .color(pal.strong),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let media = forum.is_media();
                let b = ui
                    .add_enabled(!media, pal.primary_button("New Post"))
                    .on_disabled_hover_text("Media posts need an attachment (not supported yet)");
                if b.clicked() {
                    self.dialog = Some(PostDialog::new(forum));
                }
            });
        });
        if !forum.guidelines.is_empty() {
            ui.add(
                egui::Label::new(wrapped_job(
                    ui,
                    &forum.guidelines,
                    pal.muted,
                    ui.available_width(),
                    2,
                ))
                .selectable(false),
            )
            .on_hover_text(&forum.guidelines);
        }
        ui.add_space(6.0);

        // Toolbar: sort toggle, tag chips.
        ui.horizontal_wrapped(|ui| {
            // Sort dropdown, like Discord's "Sort & View".
            let mut picked = sort;
            crate::widgets::field_style(ui, pal, |ui| {
                egui::ComboBox::from_id_salt(("forum_sort", &forum.id))
                    .selected_text(format!("{}  {}", icons::SORT_ASCENDING, sort.label()))
                    .show_ui(ui, |ui| {
                        for o in [SortOrder::LatestActivity, SortOrder::CreationDate] {
                            ui.selectable_value(&mut picked, o, o.label());
                        }
                    })
                    .response
                    .on_hover_text("Sort posts by");
            });
            if picked != sort {
                self.sort = Some(picked);
            }
            for tag in &forum.available_tags {
                let on = self.tags.contains(&tag.id);
                if tag_chip(ui, tag, ctx, on, true).clicked() {
                    if on {
                        self.tags.retain(|t| t != &tag.id);
                    } else {
                        self.tags.push(tag.id.clone());
                    }
                }
            }
            if !self.tags.is_empty() && ui.small_button("Clear").clicked() {
                self.tags.clear();
            }
        });
        ui.add_space(6.0);

        // Cards.
        let posts = store.forum_posts(&forum.id, sort, &self.tags);
        egui::ScrollArea::vertical()
            .id_salt(("forum_posts", &forum.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if posts.is_empty() && !listing.loading {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        let text = if self.tags.is_empty() {
                            "No posts yet. Be the first to start a conversation!"
                        } else {
                            "No posts match these tags."
                        };
                        ui.label(RichText::new(text).color(pal.muted));
                    });
                }
                for t in &posts {
                    post_card(ui, t, forum, store, ctx, &mut action);
                    ui.add_space(8.0);
                }

                // Pagination footer.
                if listing.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading posts…").color(pal.muted));
                    });
                } else if let Some(err) = &listing.error {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(err).color(pal.danger));
                        if ui.button("Retry").clicked() {
                            self.last_request = None;
                            let q = listing.query.clone();
                            self.request(&forum.id, q, &mut action);
                        }
                    });
                } else if let Some(q) = listing.next_query(sort, &self.tags) {
                    let label = if q.archived && q.offset == 0 {
                        "Show older posts"
                    } else {
                        "Load more"
                    };
                    let r = ui.button(label);
                    // Infinite scroll: fetch when the button scrolls into view.
                    if r.clicked() || ui.is_rect_visible(r.rect) {
                        self.request(&forum.id, Some(q), &mut action);
                    }
                }
            });
        action
    }

    fn request(
        &mut self,
        forum_id: &str,
        q: Option<ForumQuery>,
        action: &mut Option<ThreadAction>,
    ) {
        let Some(q) = q else { return };
        if action.is_some() || self.last_request.as_ref() == Some(&q) {
            return;
        }
        self.last_request = Some(q.clone());
        *action = Some(ThreadAction::LoadMore {
            forum_id: forum_id.to_string(),
            query: q,
        });
    }
}

/// Text wrapped to at most `rows` lines, ending in "…" if cut.
fn wrapped_job(
    ui: &egui::Ui,
    text: &str,
    color: Color32,
    width: f32,
    rows: usize,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(egui::TextStyle::Body.resolve(ui.style()), color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(20.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job
}

/// A tag pill. `interactive`: clickable filter chip (highlighted when `on`).
fn tag_chip(
    ui: &mut egui::Ui,
    tag: &ForumTag,
    ctx: &Ctx,
    on: bool,
    interactive: bool,
) -> egui::Response {
    let pal = ctx.pal;
    let fill = if on { pal.accent } else { pal.hover };
    let text = if on { Color32::WHITE } else { pal.text };
    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                match tag.emoji_url().filter(|_| ctx.load_images) {
                    Some(url) => {
                        ui.add(
                            egui::Image::from_uri(url)
                                .fit_to_exact_size(vec2(14.0, 14.0))
                                .show_loading_spinner(false),
                        );
                    }
                    None => {
                        if let Some(e) = &tag.emoji_name {
                            ui.label(RichText::new(e).size(11.5).color(text));
                        }
                    }
                }
                ui.add(
                    egui::Label::new(RichText::new(&tag.name).size(11.5).color(text).bold())
                        .selectable(false),
                );
            });
        });
    if interactive {
        let r = ui.interact(
            inner.response.rect,
            ui.make_persistent_id(("tag_chip", &tag.id)),
            egui::Sense::click(),
        );
        let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
        if tag.moderated {
            r.on_hover_text("Moderators only")
        } else {
            r
        }
    } else {
        inner.response
    }
}

fn post_card(
    ui: &mut egui::Ui,
    t: &Thread,
    forum: &ForumChannel,
    store: &ThreadStore,
    ctx: &Ctx,
    action: &mut Option<ThreadAction>,
) {
    let pal = ctx.pal;
    let preview = store.preview(&t.id);
    let thumb = preview
        .and_then(|p| p.image.as_ref().map(|u| (u, p.image_size)))
        .filter(|_| ctx.load_images);
    // Background painted after we know the size and hover state.
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new().inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let text_w = ui.available_width() - if thumb.is_some() { THUMB + 12.0 } else { 0.0 };
            ui.vertical(|ui| {
                ui.set_width(text_w.max(60.0));
                ui.spacing_mut().item_spacing.y = 4.0;
                let tags: Vec<&ForumTag> = t
                    .applied_tags
                    .iter()
                    .filter_map(|id| forum.tag(id))
                    .collect();
                if !tags.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for tag in tags {
                            tag_chip(ui, tag, ctx, false, false);
                        }
                    });
                }
                let mut title = String::new();
                if t.is_pinned() {
                    title.push_str(icons::PUSH_PIN);
                    title.push(' ');
                }
                if t.meta.locked {
                    title.push_str(icons::LOCK);
                    title.push(' ');
                }
                title.push_str(&t.name);
                ui.add(
                    egui::Label::new(RichText::new(title).size(15.0).bold().color(pal.strong))
                        .truncate()
                        .selectable(false),
                );

                // "author: first message…"
                let author = (ctx.user_name)(&t.owner_id)
                    .or_else(|| preview.map(|p| p.author.name.clone()))
                    .filter(|n| !n.is_empty() && n != "unknown");
                let mut job = egui::text::LayoutJob::default();
                let body = egui::TextStyle::Body.resolve(ui.style());
                let content = preview.map_or("", |p| p.content.as_str());
                if let Some(a) = author {
                    // "author: message", or just the author until the
                    // starter message is known.
                    let sep = if content.is_empty() { "" } else { ": " };
                    job.append(
                        &format!("{a}{sep}"),
                        0.0,
                        egui::TextFormat::simple(body.clone(), pal.text),
                    );
                }
                if !content.is_empty() {
                    job.append(content, 0.0, egui::TextFormat::simple(body, pal.muted));
                }
                if !job.is_empty() {
                    job.wrap = egui::text::TextWrapping {
                        max_width: ui.available_width(),
                        max_rows: 2,
                        break_anywhere: false,
                        overflow_character: Some('…'),
                    };
                    ui.add(egui::Label::new(job).selectable(false));
                }

                // Footer: replies, reactions, last activity.
                let mut footer = format!("{} {}", icons::CHAT_CIRCLE, t.message_count);
                if let Some(p) = preview.filter(|p| p.reactions > 0) {
                    footer.push_str(&format!("   {} {}", icons::SMILEY, p.reactions));
                }
                let when = crate::timefmt::full(t.last_activity());
                if !when.is_empty() {
                    footer.push_str("   ·   ");
                    footer.push_str(&when);
                }
                if t.meta.archived {
                    footer.push_str("   ·   Archived");
                }
                ui.label(RichText::new(footer).size(11.5).color(pal.muted));
            });
            if let Some((url, size)) = thumb {
                let (rect, _) = ui.allocate_exact_size(vec2(THUMB, THUMB), egui::Sense::hover());
                ui.painter().rect_filled(rect, 6, pal.hover);
                egui::Image::from_uri(url.as_str())
                    .uv(square_crop(size))
                    .corner_radius(6)
                    .show_loading_spinner(false)
                    .paint_at(ui, rect);
            }
        });
    });
    let rect = inner.response.rect;
    let resp = ui
        .interact(
            rect,
            ui.make_persistent_id(("post", &t.id)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if ctx.selected == Some(t.id.as_str()) {
        pal.selected
    } else if resp.hovered() {
        pal.hover
    } else {
        pal.sidebar
    };
    ui.painter().set(
        bg,
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::same(8), fill),
    );
    if (ctx.is_unread)(&t.id) {
        let dot = pos2(rect.right() - 10.0, rect.top() + 10.0);
        ui.painter().circle_filled(dot, 4.0, pal.unread);
    }
    if resp.clicked() {
        *action = Some(open_action(t));
    }
    resp.context_menu(|ui| thread_menu(ui, t, action));
    resp.on_hover_text(plural(t.message_count, "message"));
}

/// UV rect cropping an image of `size` to its centre square.
fn square_crop(size: Option<[u32; 2]>) -> egui::Rect {
    let full = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let Some([w, h]) = size else { return full };
    let (w, h) = (w as f32, h as f32);
    if w > h {
        let f = h / w;
        egui::Rect::from_min_max(pos2((1.0 - f) / 2.0, 0.0), pos2((1.0 + f) / 2.0, 1.0))
    } else {
        let f = w / h;
        egui::Rect::from_min_max(pos2(0.0, (1.0 - f) / 2.0), pos2(1.0, (1.0 + f) / 2.0))
    }
}

// ---------------------------------------------------------------------------
// New Post dialog
// ---------------------------------------------------------------------------

/// Discord's "New Post" form: title, message, tags.
#[derive(Debug, Clone)]
pub struct PostDialog {
    pub forum_id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
}

impl PostDialog {
    pub fn new(forum: &ForumChannel) -> Self {
        Self {
            forum_id: forum.id.clone(),
            title: String::new(),
            content: String::new(),
            tags: Vec::new(),
        }
    }

    /// Why the post can't be sent yet (None = ready).
    pub fn problem(&self, forum: &ForumChannel) -> Option<&'static str> {
        if self.title.trim().is_empty() {
            Some("Add a title")
        } else if self.content.trim().is_empty() {
            Some("Write a message")
        } else if forum.require_tag() && self.tags.is_empty() {
            Some("This forum requires a tag")
        } else {
            None
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pal: &Palette,
        forum: &ForumChannel,
        can_moderate: bool,
    ) -> DialogOutcome {
        let mut out = DialogOutcome::Open;
        let modal = egui::Modal::new(egui::Id::unique(("new_post", &self.forum_id)))
            .frame(
                egui::Frame::new()
                    .fill(pal.chat)
                    .corner_radius(8)
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    RichText::new(format!("New post in {}", forum.name))
                        .heading()
                        .color(pal.strong),
                );
                if !forum.guidelines.is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(&forum.guidelines).color(pal.muted)).wrap(),
                    );
                }
                ui.add_space(8.0);
                ui.label(pal.section_label("Post title"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.title)
                        .hint_text("Title")
                        .char_limit(100)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.label(pal.section_label("Message"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.content)
                        .hint_text("Enter a message")
                        .char_limit(2000)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                if !forum.available_tags.is_empty() {
                    ui.add_space(6.0);
                    let label = if forum.require_tag() {
                        "Tags (required)"
                    } else {
                        "Tags"
                    };
                    ui.label(pal.section_label(label));
                    ui.horizontal_wrapped(|ui| {
                        for tag in &forum.available_tags {
                            let on = self.tags.contains(&tag.id);
                            let allowed = (!tag.moderated || can_moderate)
                                && (on || self.tags.len() < MAX_APPLIED_TAGS);
                            let r = ui
                                .add_enabled(allowed, egui::Button::selectable(on, &tag.name))
                                .on_disabled_hover_text(if tag.moderated {
                                    "Only moderators can use this tag"
                                } else {
                                    "Up to 5 tags"
                                });
                            if r.clicked() {
                                if on {
                                    self.tags.retain(|t| t != &tag.id);
                                } else {
                                    self.tags.push(tag.id.clone());
                                }
                            }
                        }
                    });
                }
                ui.add_space(12.0);
                let problem = self.problem(forum);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        out = DialogOutcome::Cancelled;
                    }
                    let post = ui
                        .add_enabled(problem.is_none(), pal.primary_button("Post"))
                        .on_disabled_hover_text(problem.unwrap_or_default());
                    if post.clicked() {
                        out = DialogOutcome::Submit(self.action());
                    }
                });
            });
        if out == DialogOutcome::Open && modal.should_close() {
            out = DialogOutcome::Cancelled;
        }
        out
    }

    fn action(&self) -> ThreadAction {
        ThreadAction::CreatePost {
            forum_id: self.forum_id.clone(),
            name: self.title.trim().to_string(),
            content: self.content.trim().to_string(),
            applied_tags: self.tags.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::ThreadPage;
    use serde_json::json;

    fn forum_json() -> Value {
        json!({
            "id": "f", "guild_id": "g", "type": 15, "name": "help",
            "topic": " Be nice ", "flags": 16,
            "available_tags": [
                { "id": "t1", "name": "Bug", "moderated": false, "emoji_id": null, "emoji_name": "🐛" },
                { "id": "t2", "name": "Staff", "moderated": true, "emoji_id": "42", "emoji_name": null }
            ],
            "default_reaction_emoji": { "emoji_id": null, "emoji_name": "👍" },
            "default_sort_order": 1,
            "default_forum_layout": 2,
            "default_thread_rate_limit_per_user": 30
        })
    }

    #[test]
    fn parses_forum_channel() {
        let f = ForumChannel::from_json(&forum_json(), None);
        assert_eq!(f.guild_id, "g");
        assert_eq!(f.guidelines, "Be nice");
        assert!(f.require_tag());
        assert!(!f.is_media());
        assert_eq!(f.available_tags.len(), 2);
        assert!(f.tag("t2").unwrap().moderated);
        assert_eq!(f.default_sort_order, SortOrder::CreationDate);
        assert_eq!(f.default_layout, ForumLayout::Gallery);
        assert_eq!(f.rate_limit_per_user, 30);
        assert_eq!(
            f.default_reaction.as_ref().unwrap().emoji_name.as_deref(),
            Some("👍")
        );
        assert_eq!(
            f.tag("t2").unwrap().emoji_url().as_deref(),
            Some("https://cdn.discordapp.com/emojis/42.png?size=48")
        );
        assert!(f.tag("t1").unwrap().emoji_url().unwrap().ends_with(".png"));
    }

    #[test]
    fn parses_minimal_forum() {
        let f = ForumChannel::from_json(
            &json!({ "id": "f", "default_sort_order": null, "default_reaction_emoji": null }),
            Some("g"),
        );
        assert_eq!(f.guild_id, "g");
        assert_eq!(f.kind, CHANNEL_FORUM);
        assert_eq!(f.default_sort_order, SortOrder::LatestActivity);
        assert_eq!(f.default_layout, ForumLayout::NotSet);
        assert!(f.default_reaction.is_none());
        assert!(!f.require_tag());
        assert!(is_forum_kind(16) && !is_forum_kind(0));
    }

    #[test]
    fn search_params() {
        let mut q = ForumQuery::first(SortOrder::LatestActivity, vec![]);
        assert_eq!(
            q.search_params(),
            "archived=false&sort_by=last_message_time&sort_order=desc&limit=25&offset=0"
        );
        q.archived = true;
        q.sort = SortOrder::CreationDate;
        q.tags = vec!["1".into(), "2".into()];
        q.offset = 50;
        assert_eq!(
            q.search_params(),
            "archived=true&sort_by=creation_time&sort_order=desc&limit=25&offset=50\
             &tag=1&tag=2&tag_setting=match_some"
        );
    }

    #[test]
    fn listing_pages_active_then_archived() {
        let sort = SortOrder::LatestActivity;
        let mut l = ForumListing::default();
        let q0 = l.next_query(sort, &[]).unwrap();
        assert_eq!(q0, ForumQuery::first(sort, vec![]));
        l.begin(&q0);
        assert!(l.next_query(sort, &[]).is_none(), "nothing while loading");
        l.finish(&q0, 25, true, Some(30));
        let q1 = l.next_query(sort, &[]).unwrap();
        assert_eq!((q1.archived, q1.offset), (false, 25));
        l.begin(&q1);
        l.finish(&q1, 5, false, None);
        assert_eq!(l.total, Some(30));
        let q2 = l.next_query(sort, &[]).unwrap();
        assert_eq!((q2.archived, q2.offset), (true, 0));
        l.begin(&q2);
        assert_eq!(l.next_offset, 0, "new phase starts over");
        l.finish(&q2, 3, false, None);
        assert!(l.next_query(sort, &[]).is_none(), "end of list");
        // Changing the filter starts again from the first page.
        let q3 = l.next_query(sort, &["t".into()]).unwrap();
        assert_eq!(q3, ForumQuery::first(sort, vec!["t".into()]));
        assert_eq!(
            l.next_query(SortOrder::CreationDate, &[]).unwrap().sort,
            SortOrder::CreationDate
        );
    }

    #[test]
    fn stale_pages_and_errors() {
        let sort = SortOrder::LatestActivity;
        let mut l = ForumListing::default();
        let old = ForumQuery::first(sort, vec![]);
        let new = ForumQuery::first(sort, vec!["t".into()]);
        l.begin(&old);
        l.begin(&new);
        l.finish(&old, 25, true, None);
        assert!(l.loading, "stale answer ignored");
        l.finish(&new, 2, false, None);
        assert!(!l.loading);
        assert_eq!(l.next_offset, 2);

        let mut store = ThreadStore::new();
        store.begin_load("f", &old);
        store.fail_load("f", "HTTP 403".into());
        let listing = store.listing("f").unwrap();
        assert_eq!(listing.error.as_deref(), Some("HTTP 403"));
        assert!(listing.next_query(sort, &[]).is_none());
        // Retry clears the error.
        store.begin_load("f", &old);
        assert!(store.listing("f").unwrap().error.is_none());
    }

    #[test]
    fn store_applies_forum_pages() {
        let mut store = ThreadStore::new();
        let q = ForumQuery::first(SortOrder::LatestActivity, vec![]);
        store.begin_load("f", &q);
        let page = ThreadPage::from_json(
            &json!({
                "threads": [{ "id": "1", "parent_id": "f", "guild_id": "g", "type": 11 }],
                "members": [],
                "has_more": false,
                "first_messages": [{ "id": "1", "channel_id": "1", "content": "hi",
                    "author": { "id": "u", "username": "u" } }]
            }),
            None,
        );
        store.apply_forum_page("f", &q, page);
        assert_eq!(
            store.forum_posts("f", SortOrder::LatestActivity, &[]).len(),
            1
        );
        assert_eq!(store.preview("1").unwrap().content, "hi");
        let l = store.listing("f").unwrap();
        assert!(!l.loading && !l.has_more);
    }

    #[test]
    fn view_requests_once() {
        let mut v = ForumView::new("f");
        let q = ForumQuery::first(SortOrder::LatestActivity, vec![]);
        let mut a = None;
        v.request("f", Some(q.clone()), &mut a);
        assert!(matches!(a, Some(ThreadAction::LoadMore { .. })));
        let mut b = None;
        v.request("f", Some(q), &mut b);
        assert!(b.is_none(), "same request isn't repeated");
    }

    #[test]
    fn post_dialog_validation() {
        let f = ForumChannel::from_json(&forum_json(), None);
        let mut d = PostDialog::new(&f);
        assert_eq!(d.problem(&f), Some("Add a title"));
        d.title = " Crash on start ".into();
        assert_eq!(d.problem(&f), Some("Write a message"));
        d.content = "It crashes".into();
        assert_eq!(d.problem(&f), Some("This forum requires a tag"));
        d.tags.push("t1".into());
        assert_eq!(d.problem(&f), None);
        assert_eq!(
            d.action(),
            ThreadAction::CreatePost {
                forum_id: "f".into(),
                name: "Crash on start".into(),
                content: "It crashes".into(),
                applied_tags: vec!["t1".into()],
            }
        );
    }

    #[test]
    fn crop_is_centred_square() {
        let r = square_crop(Some([200, 100]));
        assert_eq!((r.min.x, r.max.x, r.min.y, r.max.y), (0.25, 0.75, 0.0, 1.0));
        let r = square_crop(None);
        assert_eq!(r.max, pos2(1.0, 1.0));
    }
}
