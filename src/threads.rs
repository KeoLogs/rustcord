//! Threads: thread channels (types 10 / 11 / 12), the gateway events that
//! keep them current, a memory-capped [`ThreadStore`], and the UI pieces that
//! show them: thread rows nested under their parent in the channel list and
//! the "Create Thread" dialog. Forum channels (types 15 / 16) and their post
//! list live in [`crate::forum`].
//!
//! Everything here is "data in, actions out": parsing and state updates are
//! pure functions (unit-tested below), and widgets return a [`ThreadAction`]
//! instead of doing I/O. The backend turns actions into REST calls (see the
//! thread methods in `api.rs`). Nothing here joins, creates or leaves a
//! thread on its own: every action comes from a click.
//!
//! Lenient parsing, like `types.rs`: missing fields give empty values, so
//! bot and user payloads (which differ) both work.

use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText, Stroke, pos2, vec2};
use serde_json::Value;

use crate::forum::{ForumListing, ForumQuery, SortOrder};
use crate::theme::{self, Palette};
use crate::types::{Attachment, User};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Thread in an announcement channel.
pub const CHANNEL_ANNOUNCEMENT_THREAD: u64 = 10;
/// Public thread (also every forum / media post).
pub const CHANNEL_PUBLIC_THREAD: u64 = 11;
/// Private thread (invite only).
pub const CHANNEL_PRIVATE_THREAD: u64 = 12;

/// Thread `flags` bit: pinned post in a forum / media channel.
pub const FLAG_PINNED: u64 = 1 << 1;

/// Auto-archive choices Discord accepts, in minutes (1 hour .. 1 week).
pub const AUTO_ARCHIVE_MINUTES: [u32; 4] = [60, 1440, 4320, 10080];

/// Most threads kept in memory. Archived threads you haven't joined go first.
pub const MAX_THREADS: usize = 2000;
/// Most forum post previews (first message text + thumbnail URL) kept.
pub const MAX_PREVIEWS: usize = 300;
/// Longest preview text kept per post (characters).
const PREVIEW_CHARS: usize = 200;

/// Discord's epoch (2015-01-01) in unix milliseconds.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

pub fn is_thread_kind(kind: u64) -> bool {
    matches!(
        kind,
        CHANNEL_ANNOUNCEMENT_THREAD | CHANNEL_PUBLIC_THREAD | CHANNEL_PRIVATE_THREAD
    )
}

/// Unix seconds encoded in a snowflake (0 for 0).
pub fn snowflake_time(id: u64) -> i64 {
    if id == 0 {
        return 0;
    }
    (((id >> 22) + DISCORD_EPOCH_MS) / 1000) as i64
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn snowflake(v: &Value, key: &str) -> u64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn u32_of(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// `thread_metadata` of a thread channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadMetadata {
    pub archived: bool,
    pub locked: bool,
    /// Private threads: non-moderators can add people.
    pub invitable: bool,
    /// Minutes of inactivity before auto-archiving (60, 1440, 4320, 10080).
    pub auto_archive_duration: u32,
    /// When `archived` last changed, unix seconds (0 if unknown).
    pub archive_timestamp: i64,
    /// Creation time, unix seconds. Only on threads made after 2022-01-09;
    /// 0 otherwise (use [`Thread::created`]).
    pub create_timestamp: i64,
}

impl ThreadMetadata {
    pub fn from_json(v: &Value) -> Self {
        let iso =
            |k: &str| crate::timefmt::parse_iso(v.get(k).and_then(Value::as_str).unwrap_or(""));
        Self {
            archived: v.get("archived").and_then(Value::as_bool).unwrap_or(false),
            locked: v.get("locked").and_then(Value::as_bool).unwrap_or(false),
            invitable: v.get("invitable").and_then(Value::as_bool).unwrap_or(true),
            auto_archive_duration: u32_of(v, "auto_archive_duration"),
            archive_timestamp: iso("archive_timestamp"),
            create_timestamp: iso("create_timestamp"),
        }
    }
}

/// A thread channel (or a forum / media post, which is a public thread).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub guild_id: String,
    /// The text, announcement, forum or media channel it belongs to.
    pub parent_id: String,
    /// 10, 11 or 12.
    pub kind: u64,
    pub name: String,
    /// Who started it (the forum post's author).
    pub owner_id: String,
    /// Messages in the thread, not counting the starter message (Discord
    /// stops counting at 50 on very old threads).
    pub message_count: u32,
    /// Members, capped at 50 by Discord.
    pub member_count: u32,
    /// Messages ever sent (doesn't go down on delete).
    pub total_message_sent: u32,
    pub last_message_id: u64,
    pub meta: ThreadMetadata,
    /// Forum tag ids (from the parent's `available_tags`).
    pub applied_tags: Vec<String>,
    pub flags: u64,
    /// Slowmode in seconds.
    pub rate_limit_per_user: u32,
    /// The current user is a member (has joined / was added).
    pub joined: bool,
}

impl Thread {
    /// Parse a thread channel object. `guild_id` fills in the guild when the
    /// payload doesn't carry it (READY / GUILD_CREATE `threads` arrays).
    pub fn from_json(v: &Value, guild_id: Option<&str>) -> Self {
        let own_guild = str_of(v, "guild_id");
        Self {
            id: str_of(v, "id"),
            guild_id: if own_guild.is_empty() {
                guild_id.unwrap_or_default().to_string()
            } else {
                own_guild
            },
            parent_id: str_of(v, "parent_id"),
            kind: v
                .get("type")
                .and_then(Value::as_u64)
                .unwrap_or(CHANNEL_PUBLIC_THREAD),
            name: str_of(v, "name"),
            owner_id: str_of(v, "owner_id"),
            message_count: u32_of(v, "message_count"),
            member_count: u32_of(v, "member_count"),
            total_message_sent: u32_of(v, "total_message_sent"),
            last_message_id: snowflake(v, "last_message_id"),
            meta: v
                .get("thread_metadata")
                .map(ThreadMetadata::from_json)
                .unwrap_or_default(),
            applied_tags: str_list(v, "applied_tags"),
            flags: v.get("flags").and_then(Value::as_u64).unwrap_or(0),
            rate_limit_per_user: u32_of(v, "rate_limit_per_user"),
            joined: v.get("member").is_some_and(Value::is_object),
        }
    }

    pub fn is_private(&self) -> bool {
        self.kind == CHANNEL_PRIVATE_THREAD
    }

    pub fn is_pinned(&self) -> bool {
        self.flags & FLAG_PINNED != 0
    }

    pub fn is_active(&self) -> bool {
        !self.meta.archived
    }

    /// Creation time, unix seconds.
    pub fn created(&self) -> i64 {
        if self.meta.create_timestamp > 0 {
            self.meta.create_timestamp
        } else {
            snowflake_time(self.id.parse().unwrap_or(0))
        }
    }

    /// Time of the newest message (or creation), unix seconds.
    pub fn last_activity(&self) -> i64 {
        snowflake_time(self.last_message_id).max(self.created())
    }

    fn id_num(&self) -> u64 {
        self.id.parse().unwrap_or(0)
    }
}

/// A forum post's first message, shown on its card.
#[derive(Debug, Clone, Default)]
pub struct PostPreview {
    pub author: User,
    /// First `PREVIEW_CHARS` characters, newlines turned into spaces.
    pub content: String,
    /// Thumbnail (Discord media proxy, resized) of the first image, if any.
    pub image: Option<String>,
    /// The image's size in pixels if Discord sent it (for a square crop).
    pub image_size: Option<[u32; 2]>,
    /// Total reactions on the first message.
    pub reactions: u32,
}

impl PostPreview {
    pub fn from_message_json(v: &Value) -> Self {
        let mut content: String = str_of(v, "content")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if content.chars().count() > PREVIEW_CHARS {
            content = content.chars().take(PREVIEW_CHARS).collect::<String>() + "…";
        }
        let size_of = |w: Option<u64>, h: Option<u64>| match (w, h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some([w as u32, h as u32]),
            _ => None,
        };
        let from_attachment = v
            .get("attachments")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .map(Attachment::from_json)
                    .find(Attachment::is_image)
                    .map(|a| {
                        let size = size_of(a.width.map(u64::from), a.height.map(u64::from));
                        (a.thumbnail_url(160, 160), size)
                    })
            });
        let from_embed = || {
            v.get("embeds").and_then(Value::as_array).and_then(|e| {
                e.iter().find_map(|e| {
                    ["image", "thumbnail"].iter().find_map(|k| {
                        let img = e.get(*k)?;
                        let url = img
                            .get("proxy_url")
                            .or_else(|| img.get("url"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())?;
                        let size = size_of(
                            img.get("width").and_then(Value::as_u64),
                            img.get("height").and_then(Value::as_u64),
                        );
                        Some((url.to_string(), size))
                    })
                })
            })
        };
        let (image, image_size) = match from_attachment.or_else(from_embed) {
            Some((url, size)) => (Some(url), size),
            None => (None, None),
        };
        let reactions = v
            .get("reactions")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|r| u32_of(r, "count")).sum())
            .unwrap_or(0);
        Self {
            author: User::from_json(v.get("author").unwrap_or(&Value::Null)),
            content,
            image,
            image_size,
            reactions,
        }
    }
}

/// One page of threads from a REST listing (active guild threads, archived
/// threads, forum search).
#[derive(Debug, Clone, Default)]
pub struct ThreadPage {
    pub threads: Vec<Thread>,
    /// Threads in this page that the current user has joined (from the
    /// `members` array: one thread member object per joined thread).
    pub joined: Vec<String>,
    pub has_more: bool,
    /// Forum search only: total matching posts.
    pub total: Option<u32>,
    /// Forum search only: (thread id, preview of its first message).
    pub previews: Vec<(String, PostPreview)>,
}

impl ThreadPage {
    /// Parses `{threads, members, has_more, total_results?, first_messages?}`.
    pub fn from_json(v: &Value, guild_id: Option<&str>) -> Self {
        let threads: Vec<Thread> = v
            .get("threads")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|t| Thread::from_json(t, guild_id)).collect())
            .unwrap_or_default();
        let joined = v
            .get("members")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|m| str_of(m, "id"))
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        // The first message of a forum post has the post's id as channel_id
        // (and, for posts, the same id as the thread itself).
        let previews = v
            .get("first_messages")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        let ch = str_of(m, "channel_id");
                        let id = if ch.is_empty() { str_of(m, "id") } else { ch };
                        (!id.is_empty()).then(|| (id, PostPreview::from_message_json(m)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            threads,
            joined,
            has_more: v.get("has_more").and_then(Value::as_bool).unwrap_or(false),
            total: v
                .get("total_results")
                .and_then(Value::as_u64)
                .map(|n| n.min(u32::MAX as u64) as u32),
            previews,
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway events
// ---------------------------------------------------------------------------

/// A thread gateway event, parsed. Send these from the backend to the UI
/// (e.g. `ToUi::Thread(ThreadEvent)`) and apply with [`ThreadStore::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    /// THREAD_CREATE / THREAD_UPDATE: the full thread object.
    Upsert(Thread),
    /// THREAD_DELETE.
    Delete { id: String, parent_id: String },
    /// THREAD_LIST_SYNC: replaces the active threads of `channel_ids` (or of
    /// the whole guild when None).
    ListSync {
        guild_id: String,
        channel_ids: Option<Vec<String>>,
        threads: Vec<Thread>,
        /// Threads in `threads` the current user is in.
        joined: Vec<String>,
    },
    /// READY (user accounts: `guilds[].threads`) / GUILD_CREATE `threads`:
    /// every active thread the user can see in the guild.
    GuildThreads {
        guild_id: String,
        threads: Vec<Thread>,
    },
    /// THREAD_MEMBER_UPDATE: the current user's membership of a thread
    /// (sent when you join).
    MemberUpdate { thread_id: String },
    /// THREAD_MEMBERS_UPDATE: people added to / removed from a thread.
    MembersUpdate {
        thread_id: String,
        member_count: u32,
        added: Vec<String>,
        removed: Vec<String>,
    },
}

impl ThreadEvent {
    /// Parse a dispatch (`t`, `d`). None for other event names.
    pub fn from_dispatch(t: &str, d: &Value) -> Option<Self> {
        Some(match t {
            "THREAD_CREATE" | "THREAD_UPDATE" => Self::Upsert(Thread::from_json(d, None)),
            "THREAD_DELETE" => Self::Delete {
                id: str_of(d, "id"),
                parent_id: str_of(d, "parent_id"),
            },
            "THREAD_LIST_SYNC" => {
                let guild_id = str_of(d, "guild_id");
                let page = ThreadPage::from_json(d, Some(&guild_id));
                Self::ListSync {
                    channel_ids: d
                        .get("channel_ids")
                        .filter(|c| c.is_array())
                        .map(|_| str_list(d, "channel_ids")),
                    guild_id,
                    threads: page.threads,
                    joined: page.joined,
                }
            }
            "THREAD_MEMBER_UPDATE" => Self::MemberUpdate {
                thread_id: str_of(d, "id"),
            },
            "THREAD_MEMBERS_UPDATE" => Self::MembersUpdate {
                thread_id: str_of(d, "id"),
                member_count: u32_of(d, "member_count"),
                added: d
                    .get("added_members")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|m| str_of(m, "user_id")).collect())
                    .unwrap_or_default(),
                removed: str_list(d, "removed_member_ids"),
            },
            _ => return None,
        })
    }

    /// The `threads` array of a READY guild or GUILD_CREATE payload. None if
    /// the guild object has no `threads` (e.g. unavailable guilds).
    pub fn from_guild(guild: &Value) -> Option<Self> {
        let arr = guild.get("threads")?.as_array()?;
        let guild_id = str_of(guild, "id");
        Some(Self::GuildThreads {
            threads: arr
                .iter()
                .map(|t| Thread::from_json(t, Some(&guild_id)))
                .collect(),
            guild_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// All known threads, indexed by parent channel, plus forum post previews
/// and forum pagination state. Capped at [`MAX_THREADS`] threads and
/// [`MAX_PREVIEWS`] previews.
#[derive(Debug)]
pub struct ThreadStore {
    me: String,
    threads: HashMap<String, Thread>,
    /// parent channel id -> thread ids (unordered).
    by_parent: HashMap<String, Vec<String>>,
    previews: HashMap<String, PostPreview>,
    listings: HashMap<String, ForumListing>,
    max_threads: usize,
    max_previews: usize,
}

impl Default for ThreadStore {
    fn default() -> Self {
        Self::with_caps(MAX_THREADS, MAX_PREVIEWS)
    }
}

impl ThreadStore {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_caps(max_threads: usize, max_previews: usize) -> Self {
        Self {
            me: String::new(),
            threads: HashMap::new(),
            by_parent: HashMap::new(),
            previews: HashMap::new(),
            listings: HashMap::new(),
            max_threads: max_threads.max(1),
            max_previews: max_previews.max(1),
        }
    }

    /// The current user's id (needed for THREAD_MEMBERS_UPDATE).
    pub fn set_me(&mut self, user_id: &str) {
        self.me = user_id.to_string();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.threads.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Thread> {
        self.threads.get(id)
    }

    pub fn preview(&self, thread_id: &str) -> Option<&PostPreview> {
        self.previews.get(thread_id)
    }

    /// Pagination state of a forum's post list.
    pub fn listing(&self, forum_id: &str) -> Option<&ForumListing> {
        self.listings.get(forum_id)
    }

    /// Every known thread of a parent channel, in no particular order.
    pub fn children<'a>(&'a self, parent_id: &str) -> impl Iterator<Item = &'a Thread> + 'a {
        self.by_parent
            .get(parent_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.threads.get(id))
    }

    /// Threads shown under a channel in the channel list, like Discord:
    /// active threads you've joined, plus the open one. Oldest first.
    pub fn sidebar_threads(&self, parent_id: &str, selected: Option<&str>) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self
            .children(parent_id)
            .filter(|t| (t.joined && t.is_active()) || selected == Some(t.id.as_str()))
            .collect();
        v.sort_by_key(|t| t.id_num());
        v
    }

    /// Active threads of a channel, most recent activity first (thread
    /// browser).
    pub fn active_threads(&self, parent_id: &str) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self.children(parent_id).filter(|t| t.is_active()).collect();
        v.sort_by_key(|t| std::cmp::Reverse((t.last_activity(), t.id_num())));
        v
    }

    /// Archived threads of a channel, most recently archived first.
    pub fn archived_threads(&self, parent_id: &str) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self
            .children(parent_id)
            .filter(|t| !t.is_active())
            .collect();
        v.sort_by_key(|t| std::cmp::Reverse((t.meta.archive_timestamp, t.id_num())));
        v
    }

    /// ISO-8601 `archive_timestamp` of the oldest archived thread of a
    /// channel: the `before=` for the next archived-threads page.
    pub fn oldest_archived_iso(&self, parent_id: &str) -> Option<String> {
        self.children(parent_id)
            .filter(|t| !t.is_active() && t.meta.archive_timestamp > 0)
            .map(|t| t.meta.archive_timestamp)
            .min()
            .and_then(iso)
    }

    /// Posts of a forum for its post list: pinned first, then by `sort`
    /// (newest first). With `tags`, only posts that have at least one of
    /// them (Discord's "match some").
    pub fn forum_posts(&self, forum_id: &str, sort: SortOrder, tags: &[String]) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self
            .children(forum_id)
            .filter(|t| tags.is_empty() || t.applied_tags.iter().any(|a| tags.contains(a)))
            .collect();
        v.sort_by_key(|t| {
            let key = match sort {
                SortOrder::LatestActivity => t.last_activity(),
                SortOrder::CreationDate => t.created(),
            };
            std::cmp::Reverse((t.is_pinned() && t.is_active(), key, t.id_num()))
        });
        v
    }

    /// Apply a gateway event.
    pub fn apply(&mut self, event: ThreadEvent) {
        match event {
            ThreadEvent::Upsert(t) => self.upsert(t, false),
            ThreadEvent::Delete { id, .. } => self.remove(&id),
            ThreadEvent::ListSync {
                guild_id,
                channel_ids,
                threads,
                joined,
            } => {
                // Drop the active threads of the synced channels (or the
                // whole guild), then take the event's list as the truth.
                let stale: Vec<String> = self
                    .threads
                    .values()
                    .filter(|t| t.guild_id == guild_id && t.is_active())
                    .filter(|t| match &channel_ids {
                        Some(ids) => ids.contains(&t.parent_id),
                        None => true,
                    })
                    .map(|t| t.id.clone())
                    .collect();
                for id in stale {
                    self.remove(&id);
                }
                for mut t in threads {
                    t.joined = joined.contains(&t.id);
                    self.upsert(t, true);
                }
            }
            ThreadEvent::GuildThreads { guild_id, threads } => {
                let stale: Vec<String> = self
                    .threads
                    .values()
                    .filter(|t| t.guild_id == guild_id && t.is_active())
                    .map(|t| t.id.clone())
                    .collect();
                for id in stale {
                    self.remove(&id);
                }
                for t in threads {
                    self.upsert(t, false);
                }
            }
            ThreadEvent::MemberUpdate { thread_id } => self.set_joined(&thread_id, true),
            ThreadEvent::MembersUpdate {
                thread_id,
                member_count,
                added,
                removed,
            } => {
                let me = self.me.clone();
                if let Some(t) = self.threads.get_mut(&thread_id) {
                    t.member_count = member_count;
                    if !me.is_empty() {
                        if added.contains(&me) {
                            t.joined = true;
                        }
                        if removed.contains(&me) {
                            t.joined = false;
                        }
                    }
                }
            }
        }
    }

    /// A message arrived (MESSAGE_CREATE). Bumps a thread's activity and
    /// counters; does nothing for non-thread channels.
    pub fn on_message(&mut self, channel_id: &str, message_id: &str) {
        let Some(t) = self.threads.get_mut(channel_id) else {
            return;
        };
        let id: u64 = message_id.parse().unwrap_or(0);
        if id > t.last_message_id {
            t.last_message_id = id;
            t.message_count = t.message_count.saturating_add(1);
            t.total_message_sent = t.total_message_sent.saturating_add(1);
        }
    }

    /// After a successful join / leave request (the gateway confirms later).
    pub fn set_joined(&mut self, thread_id: &str, joined: bool) {
        if let Some(t) = self.threads.get_mut(thread_id) {
            t.joined = joined;
        }
    }

    /// Add a REST page (active guild threads, archived threads, a created
    /// thread). Membership in the page is authoritative.
    pub fn apply_page(&mut self, page: ThreadPage) {
        for (id, p) in page.previews {
            self.previews.insert(id, p);
        }
        for mut t in page.threads {
            t.joined = page.joined.contains(&t.id);
            self.upsert(t, true);
        }
        self.cap_previews();
    }

    /// A forum list request is about to be sent.
    pub fn begin_load(&mut self, forum_id: &str, query: &ForumQuery) {
        self.listings
            .entry(forum_id.to_string())
            .or_default()
            .begin(query);
    }

    /// A forum page arrived for `query`.
    pub fn apply_forum_page(&mut self, forum_id: &str, query: &ForumQuery, page: ThreadPage) {
        let (count, has_more, total) = (page.threads.len(), page.has_more, page.total);
        self.apply_page(page);
        self.listings
            .entry(forum_id.to_string())
            .or_default()
            .finish(query, count, has_more, total);
    }

    /// A forum list request failed.
    pub fn fail_load(&mut self, forum_id: &str, error: String) {
        if let Some(l) = self.listings.get_mut(forum_id) {
            l.loading = false;
            l.error = Some(error);
        }
    }

    /// A guild went away (left / GUILD_DELETE): forget its threads.
    pub fn remove_guild(&mut self, guild_id: &str) {
        let ids: Vec<String> = self
            .threads
            .values()
            .filter(|t| t.guild_id == guild_id)
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            self.listings.remove(&id);
            self.remove(&id);
        }
    }

    /// A channel was deleted: forget its threads and forum state.
    pub fn remove_parent(&mut self, parent_id: &str) {
        let ids: Vec<String> = self.children(parent_id).map(|t| t.id.clone()).collect();
        for id in ids {
            self.remove(&id);
        }
        self.listings.remove(parent_id);
    }

    /// A thread of `parent_id` called `name` (the newest if several are),
    /// for thread-created messages whose payload has no thread id.
    pub fn find_by_name(&self, parent_id: &str, name: &str) -> Option<&Thread> {
        if name.is_empty() {
            return None;
        }
        self.children(parent_id)
            .filter(|t| t.name == name)
            .max_by_key(|t| t.id_num())
    }

    /// Insert or replace. With `joined_known` false (THREAD_UPDATE, which has
    /// no member object), a membership we already knew about is kept.
    fn upsert(&mut self, mut t: Thread, joined_known: bool) {
        if t.id.is_empty() {
            return;
        }
        if let Some(old) = self.threads.get(&t.id) {
            if !joined_known {
                t.joined |= old.joined;
            }
            // Gateway updates can be older than what MESSAGE_CREATE told us.
            if old.last_message_id > t.last_message_id {
                t.last_message_id = old.last_message_id;
                t.message_count = t.message_count.max(old.message_count);
            }
            if old.parent_id != t.parent_id {
                let old_parent = old.parent_id.clone();
                self.unindex(&old_parent, &t.id);
            } else {
                self.threads.insert(t.id.clone(), t);
                return;
            }
        }
        self.by_parent
            .entry(t.parent_id.clone())
            .or_default()
            .push(t.id.clone());
        self.threads.insert(t.id.clone(), t);
        self.cap_threads();
    }

    fn remove(&mut self, id: &str) {
        if let Some(t) = self.threads.remove(id) {
            self.unindex(&t.parent_id, id);
        }
        self.previews.remove(id);
    }

    fn unindex(&mut self, parent_id: &str, id: &str) {
        if let Some(v) = self.by_parent.get_mut(parent_id) {
            v.retain(|x| x != id);
            if v.is_empty() {
                self.by_parent.remove(parent_id);
            }
        }
    }

    /// Over the cap: drop down to 90 % so we don't sort on every insert.
    /// Archived threads you haven't joined go first, then archived joined,
    /// then active ones; oldest activity first within each.
    fn cap_threads(&mut self) {
        if self.threads.len() <= self.max_threads {
            return;
        }
        let target = self.max_threads * 9 / 10;
        let mut order: Vec<(bool, bool, i64, String)> = self
            .threads
            .values()
            .map(|t| (t.is_active(), t.joined, t.last_activity(), t.id.clone()))
            .collect();
        order.sort();
        let drop = self.threads.len() - target;
        for (_, _, _, id) in order.into_iter().take(drop) {
            self.remove(&id);
        }
    }

    fn cap_previews(&mut self) {
        if self.previews.len() <= self.max_previews {
            return;
        }
        let threads = &self.threads;
        self.previews.retain(|id, _| threads.contains_key(id));
        if self.previews.len() <= self.max_previews {
            return;
        }
        let mut order: Vec<(i64, String)> = self
            .previews
            .keys()
            .map(|id| (threads.get(id).map_or(0, Thread::last_activity), id.clone()))
            .collect();
        order.sort();
        let drop = self.previews.len() - self.max_previews * 9 / 10;
        for (_, id) in order.into_iter().take(drop) {
            self.previews.remove(&id);
        }
    }
}

// ---------------------------------------------------------------------------
// Thread cards (a thread-created message in the chat)
// ---------------------------------------------------------------------------

/// The thread a message's card opens: the store's copy when we know it
/// (live name and counts from THREAD_UPDATE), else one built from the
/// message (`thread` object or `message_reference`), else the store's
/// thread of this channel with the message's name (type 18 content is the
/// thread name). None if nothing names a thread. `guild_id` fills in the
/// server for a built one (history messages carry no `guild_id`).
pub fn card_thread<'a>(
    m: &crate::types::Message,
    store: &'a ThreadStore,
    guild_id: Option<&str>,
) -> Option<std::borrow::Cow<'a, Thread>> {
    use std::borrow::Cow;
    if let Some(id) = m.thread_id() {
        if let Some(t) = store.get(id) {
            return Some(Cow::Borrowed(t));
        }
        let r = m.thread.as_deref();
        let name = r
            .map(|r| r.name.as_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(&m.content);
        return Some(Cow::Owned(Thread {
            id: id.to_string(),
            guild_id: m
                .guild_id
                .as_deref()
                .or(guild_id)
                .unwrap_or_default()
                .to_string(),
            parent_id: m.channel_id.clone(),
            kind: CHANNEL_PUBLIC_THREAD,
            name: name.to_string(),
            message_count: r.and_then(|r| r.message_count).unwrap_or(0),
            last_message_id: r.map_or(0, |r| r.last_message_id),
            ..Thread::default()
        }));
    }
    let name = m
        .thread
        .as_deref()
        .map(|r| r.name.as_str())
        .unwrap_or(&m.content);
    store.find_by_name(&m.channel_id, name).map(Cow::Borrowed)
}

/// Longest snippet on a thread card (characters; the UI also truncates to
/// the card's width).
const CARD_SNIPPET_CHARS: usize = 120;

/// The newest message of a thread, as its card shows it.
#[derive(Debug, Clone)]
pub struct CardLast {
    pub author: User,
    /// One line of plain text (never empty).
    pub snippet: String,
}

/// A thread card's preview line: the newest message when it's loaded,
/// and the message count.
#[derive(Debug, Clone, Default)]
pub struct CardPreview {
    pub last: Option<CardLast>,
    /// Messages in the thread (0 = unknown or none).
    pub count: u32,
}

impl CardPreview {
    /// "1 message" / "N messages", None when the count is unknown.
    pub fn count_text(&self) -> Option<String> {
        match self.count {
            0 => None,
            1 => Some("1 message".to_string()),
            n => Some(format!("{n} messages")),
        }
    }

    /// The muted line when the newest message isn't loaded.
    pub fn summary(&self) -> String {
        match self.count_text() {
            Some(c) => format!("{c} · Open to see messages"),
            None => "Open to see messages".to_string(),
        }
    }
}

/// Build a card's preview from what's already here: the thread (store or
/// THREAD_UPDATE count) and the thread channel's cached messages (oldest
/// first), if it was opened or got messages this session. Never asks
/// Discord for anything. `plain` turns a message into one line of text
/// (`markdown::to_plain_text` through the parse cache).
pub fn card_preview(
    thread: &Thread,
    cached: Option<&[crate::types::Message]>,
    plain: &mut dyn FnMut(&crate::types::Message) -> String,
) -> CardPreview {
    let cached = cached.unwrap_or_default();
    // Discord doesn't count the starter message (type 21).
    let loaded = cached.iter().filter(|m| m.kind != 21).count();
    let count = thread
        .message_count
        .max(u32::try_from(loaded).unwrap_or(u32::MAX));
    let newest = cached
        .iter()
        .rev()
        .find(|m| crate::sysmsg::is_normal(m.kind));
    // The cache is behind the thread (it got newer messages we don't have):
    // don't show an old message as the last one.
    let behind = |m: &crate::types::Message| {
        thread.last_message_id > 0 && m.id.parse::<u64>().unwrap_or(0) < thread.last_message_id
    };
    let last = newest
        .filter(|_| !cached.last().is_some_and(behind))
        .map(|m| CardLast {
            author: m.author.clone(),
            snippet: card_snippet(&plain(m), m),
        });
    CardPreview { last, count }
}

/// One line, at most `CARD_SNIPPET_CHARS`; a word for messages without
/// text.
fn card_snippet(text: &str, m: &crate::types::Message) -> String {
    let mut s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > CARD_SNIPPET_CHARS {
        s = s.chars().take(CARD_SNIPPET_CHARS).collect::<String>() + "…";
    }
    if !s.is_empty() {
        s
    } else if !m.attachments.is_empty() {
        "Sent an attachment".to_string()
    } else if !m.stickers.is_empty() {
        "Sent a sticker".to_string()
    } else if !m.embeds.is_empty() {
        "Sent an embed".to_string()
    } else {
        "Sent a message".to_string()
    }
}

// ---------------------------------------------------------------------------
// Actions and UI context
// ---------------------------------------------------------------------------

/// What the user asked for in a thread / forum widget. The integrator turns
/// these into `ToBackend` requests (or navigation, for `OpenThread`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadAction {
    /// Show the thread's messages (it's a channel: load history as usual).
    OpenThread {
        guild_id: String,
        parent_id: String,
        thread_id: String,
    },
    /// PUT /channels/{thread}/thread-members/@me.
    JoinThread { thread_id: String },
    /// DELETE /channels/{thread}/thread-members/@me.
    LeaveThread { thread_id: String },
    /// New thread in a text / announcement channel: from a message
    /// (POST .../messages/{m}/threads) or standalone (POST .../threads).
    CreateThread {
        channel_id: String,
        message_id: Option<String>,
        name: String,
        /// Standalone threads only: private (type 12) instead of public.
        private: bool,
        auto_archive_minutes: u32,
    },
    /// New forum / media post (POST /channels/{forum}/threads).
    CreatePost {
        forum_id: String,
        name: String,
        content: String,
        applied_tags: Vec<String>,
    },
    /// Fetch (more of) a forum's post list. Call
    /// [`ThreadStore::begin_load`] when sending it.
    LoadMore { forum_id: String, query: ForumQuery },
    /// Fetch a text channel's archived threads (thread browser).
    LoadArchived {
        channel_id: String,
        /// ISO-8601 `archive_timestamp` of the oldest archived thread shown.
        before: Option<String>,
    },
}

/// What thread widgets need from the app.
pub struct Ctx<'a> {
    pub pal: &'a Palette,
    /// The open channel / thread id.
    pub selected: Option<&'a str>,
    /// Unread state of a channel / thread id (read_state).
    pub is_unread: &'a dyn Fn(&str) -> bool,
    /// Display name for a user id (member cache), if known.
    pub user_name: &'a dyn Fn(&str) -> Option<String>,
    /// Images module on: show thumbnails and custom tag emoji.
    pub load_images: bool,
    /// The user has Manage Threads in this channel (moderated forum tags).
    pub can_manage_threads: bool,
}

pub(crate) fn truncated(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(10.0));
    ui.painter().layout_job(job)
}

/// "Copy ID" / Join / Leave items shared by thread rows and forum cards.
pub(crate) fn thread_menu(ui: &mut egui::Ui, t: &Thread, action: &mut Option<ThreadAction>) {
    if ui.button("Open").clicked() {
        *action = Some(open_action(t));
        ui.close();
    }
    if t.joined {
        if ui.button("Leave Thread").clicked() {
            *action = Some(ThreadAction::LeaveThread {
                thread_id: t.id.clone(),
            });
            ui.close();
        }
    } else if ui.button("Join Thread").clicked() {
        *action = Some(ThreadAction::JoinThread {
            thread_id: t.id.clone(),
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Copy Thread ID").clicked() {
        ui.ctx().copy_text(t.id.clone());
        ui.close();
    }
}

pub(crate) fn open_action(t: &Thread) -> ThreadAction {
    ThreadAction::OpenThread {
        guild_id: t.guild_id.clone(),
        parent_id: t.parent_id.clone(),
        thread_id: t.id.clone(),
    }
}

// ---------------------------------------------------------------------------
// Channel list rows
// ---------------------------------------------------------------------------

const ROW_H: f32 = 30.0;
/// x of the tree line, under the parent row's channel icon (app.rs draws the
/// icon centred 18 px from the row's left edge).
const TREE_X: f32 = 18.0;
const TREE_RADIUS: f32 = 7.0;
const TEXT_X: f32 = 36.0;

/// Thread rows under `parent_id` in the channel list, with Discord's curved
/// tree line. Call right after drawing the parent channel's row. Returns the
/// clicked action, if any.
pub fn thread_rows(
    ui: &mut egui::Ui,
    parent_id: &str,
    store: &ThreadStore,
    ctx: &Ctx,
) -> Option<ThreadAction> {
    let list = store.sidebar_threads(parent_id, ctx.selected);
    let mut action = None;
    let pal = ctx.pal;
    let gap = ui.spacing().item_spacing.y;
    let n = list.len();
    for (i, t) in list.into_iter().enumerate() {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), egui::Sense::click());
        let selected = ctx.selected == Some(t.id.as_str());
        let hovered = resp.hovered();
        let unread = !selected && (ctx.is_unread)(&t.id);

        let bg = egui::Rect::from_min_max(pos2(rect.left() + TEXT_X - 8.0, rect.top()), rect.max);
        if selected {
            ui.painter().rect_filled(bg, 4, pal.selected);
        } else if hovered {
            ui.painter().rect_filled(bg, 4, pal.hover);
        }

        // Tree line: a vertical line from the parent, then a quarter curve
        // into this row. It carries on down if more rows follow.
        let stroke = Stroke::new(1.5, pal.divider.gamma_multiply(1.6));
        let x = rect.left() + TREE_X;
        let cy = rect.center().y;
        let top = if i == 0 {
            rect.top() - gap - 6.0
        } else {
            rect.top() - gap
        };
        let p = ui.painter();
        p.line_segment([pos2(x, top), pos2(x, cy - TREE_RADIUS)], stroke);
        let centre = pos2(x + TREE_RADIUS, cy - TREE_RADIUS);
        let arc: Vec<egui::Pos2> = (0..=8)
            .map(|k| {
                let a = std::f32::consts::PI - (k as f32 / 8.0) * std::f32::consts::FRAC_PI_2;
                centre + TREE_RADIUS * vec2(a.cos(), a.sin())
            })
            .collect();
        p.add(egui::Shape::line(arc, stroke));
        p.line_segment(
            [
                pos2(x + TREE_RADIUS, cy),
                pos2(rect.left() + TEXT_X - 10.0, cy),
            ],
            stroke,
        );
        if i + 1 < n {
            p.line_segment([pos2(x, cy - TREE_RADIUS), pos2(x, rect.bottom())], stroke);
        }

        let color = if selected {
            pal.strong
        } else if unread {
            pal.unread
        } else if !t.is_active() {
            pal.muted_text()
        } else if hovered {
            pal.text
        } else {
            pal.muted
        };
        let font = if selected || unread {
            theme::bold_font(14.5)
        } else {
            egui::FontId::proportional(14.5)
        };
        let tx = rect.left() + TEXT_X;
        let galley = truncated(ui, &t.name, font, color, rect.right() - 8.0 - tx);
        ui.painter()
            .galley(pos2(tx, cy - galley.size().y / 2.0), galley, color);
        if unread {
            // Discord's small unread pill on the far left.
            let pill = egui::Rect::from_min_size(pos2(rect.left() - 8.0, cy - 4.0), vec2(4.0, 8.0));
            ui.painter().rect_filled(pill, 2, pal.unread);
        }

        if resp.clicked() {
            action = Some(open_action(t));
        }
        resp.context_menu(|ui| thread_menu(ui, t, &mut action));
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(thread_tooltip(t));
    }
    action
}

fn thread_tooltip(t: &Thread) -> String {
    let mut s = t.name.clone();
    if t.is_private() {
        s.push_str("\nPrivate thread");
    }
    if t.meta.locked {
        s.push_str("\nLocked");
    }
    if t.meta.archived {
        s.push_str("\nArchived");
    }
    s
}

// ---------------------------------------------------------------------------
// Thread browser (all threads of a text channel)
// ---------------------------------------------------------------------------

/// Compact list of a text channel's threads (active, then archived) with
/// Join buttons, for a "Threads" popout. `has_more_archived`: show "Load
/// older" (from the last archived page's `has_more`).
pub fn thread_browser(
    ui: &mut egui::Ui,
    channel_id: &str,
    store: &ThreadStore,
    ctx: &Ctx,
    has_more_archived: bool,
) -> Option<ThreadAction> {
    let pal = ctx.pal;
    let mut action = None;
    let active = store.active_threads(channel_id);
    let archived = store.archived_threads(channel_id);
    egui::ScrollArea::vertical()
        .id_salt(("thread_browser", channel_id))
        .min_scrolled_height(320.0)
        .max_height(420.0)
        .show(ui, |ui| {
            ui.label(pal.section_label(&format!("Active threads — {}", active.len())));
            if active.is_empty() {
                ui.label(RichText::new("No active threads.").color(pal.muted));
            }
            for t in &active {
                browser_row(ui, t, ctx, &mut action);
            }
            ui.add_space(8.0);
            ui.label(pal.section_label("Older threads"));
            for t in &archived {
                browser_row(ui, t, ctx, &mut action);
            }
            if has_more_archived || archived.is_empty() {
                let label = if archived.is_empty() {
                    "Load archived threads"
                } else {
                    "Load older"
                };
                if ui.button(label).clicked() {
                    action = Some(ThreadAction::LoadArchived {
                        channel_id: channel_id.to_string(),
                        before: store.oldest_archived_iso(channel_id),
                    });
                }
            }
        });
    action
}

fn browser_row(ui: &mut egui::Ui, t: &Thread, ctx: &Ctx, action: &mut Option<ThreadAction>) {
    let pal = ctx.pal;
    let can_join = !t.joined && !(t.meta.locked && !ctx.can_manage_threads);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 44.0), egui::Sense::click());
    // The Join button sits on top of the row.
    let join_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 58.0, rect.center().y - 13.0),
        vec2(52.0, 26.0),
    );
    let join = can_join.then(|| {
        ui.put(
            join_rect,
            egui::Button::new(RichText::new("Join").color(pal.text))
                .fill(pal.input)
                .corner_radius(4),
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() || join.as_ref().is_some_and(egui::Response::hovered) {
            painter.rect_filled(rect, 4, pal.hover);
        }
        painter.text(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            crate::icons::CHATS,
            crate::icons::font(18.0),
            pal.muted,
        );
        let text_w = rect.width() - 36.0 - if can_join { 64.0 } else { 8.0 };
        painter.galley(
            egui::pos2(rect.left() + 36.0, rect.top() + 5.0),
            one_line(
                ui,
                &t.name,
                crate::theme::bold_font(14.0),
                pal.strong,
                text_w,
            ),
            pal.strong,
        );
        painter.text(
            egui::pos2(rect.left() + 36.0, rect.bottom() - 5.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} · {}",
                plural(t.message_count, "message"),
                crate::timefmt::full(t.last_activity())
            ),
            egui::FontId::proportional(12.0),
            pal.muted,
        );
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *action = Some(open_action(t));
    }
    resp.context_menu(|ui| thread_menu(ui, t, action));
    if join.is_some_and(|j| j.clicked()) {
        *action = Some(ThreadAction::JoinThread {
            thread_id: t.id.clone(),
        });
    }
}

/// `text` on one row, cut with "…" to `width`.
fn one_line(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// "1 message" / "3 messages".
pub(crate) fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Unix seconds -> ISO-8601 (for `before=` paging).
fn iso(unix: i64) -> Option<String> {
    use chrono::{TimeZone, Utc};
    (unix > 0)
        .then(|| Utc.timestamp_opt(unix, 0).single())
        .flatten()
        .map(|t| t.to_rfc3339())
}

// ---------------------------------------------------------------------------
// Create Thread dialog
// ---------------------------------------------------------------------------

/// Result of showing a dialog for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome {
    /// Still open.
    Open,
    /// Closed without doing anything.
    Cancelled,
    /// The user pressed the create button.
    Submit(ThreadAction),
}

/// Discord's "Create Thread" dialog: name, auto-archive time, and (for a
/// thread not started from a message) public / private.
#[derive(Debug, Clone)]
pub struct ThreadDialog {
    pub channel_id: String,
    pub message_id: Option<String>,
    /// Starter message excerpt shown above the form.
    pub starter: Option<String>,
    pub name: String,
    pub private: bool,
    pub auto_archive_minutes: u32,
}

impl ThreadDialog {
    /// Thread from a message ("Create Thread" in the message menu). The name
    /// defaults to the start of the message, like Discord.
    pub fn from_message(channel_id: &str, message_id: &str, content: &str) -> Self {
        let excerpt: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            channel_id: channel_id.to_string(),
            message_id: Some(message_id.to_string()),
            name: excerpt.chars().take(40).collect(),
            starter: (!excerpt.is_empty()).then(|| excerpt.chars().take(140).collect()),
            private: false,
            auto_archive_minutes: 4320,
        }
    }

    /// Standalone thread in a text channel.
    pub fn standalone(channel_id: &str) -> Self {
        Self {
            channel_id: channel_id.to_string(),
            message_id: None,
            starter: None,
            name: String::new(),
            private: false,
            auto_archive_minutes: 4320,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> DialogOutcome {
        let mut out = DialogOutcome::Open;
        let modal = egui::Modal::new(egui::Id::unique(("create_thread", &self.channel_id)))
            .frame(
                egui::Frame::new()
                    .fill(pal.chat)
                    .corner_radius(8)
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("Create Thread").heading().color(pal.strong));
                if let Some(s) = &self.starter {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(pal.sidebar)
                        .corner_radius(4)
                        .inner_margin(8)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(s).color(pal.muted)).wrap());
                        });
                }
                ui.add_space(10.0);
                ui.label(pal.section_label("Thread name"));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("New Thread")
                        .char_limit(100)
                        .desired_width(f32::INFINITY),
                );
                if self.name.is_empty() && !edit.has_focus() {
                    edit.request_focus();
                }
                ui.add_space(8.0);
                ui.label(pal.section_label("Hide after inactivity"));
                ui.horizontal(|ui| {
                    for m in AUTO_ARCHIVE_MINUTES {
                        ui.selectable_value(&mut self.auto_archive_minutes, m, archive_label(m));
                    }
                });
                if self.message_id.is_none() {
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.private, "Private thread")
                        .on_hover_text("Only people you invite and moderators can see it");
                }
                ui.add_space(14.0);
                let valid = !self.name.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        out = DialogOutcome::Cancelled;
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let create = ui.add_enabled(valid, pal.primary_button("Create Thread"));
                    if valid && (create.clicked() || enter) {
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
        ThreadAction::CreateThread {
            channel_id: self.channel_id.clone(),
            message_id: self.message_id.clone(),
            name: self.name.trim().to_string(),
            private: self.private && self.message_id.is_none(),
            auto_archive_minutes: self.auto_archive_minutes,
        }
    }
}

/// "1 Hour" / "24 Hours" / "3 Days" / "1 Week".
pub fn archive_label(minutes: u32) -> &'static str {
    match minutes {
        60 => "1 Hour",
        1440 => "24 Hours",
        4320 => "3 Days",
        10080 => "1 Week",
        _ => "Custom",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread_json(id: &str, parent: &str) -> Value {
        json!({
            "id": id, "guild_id": "g1", "parent_id": parent, "type": 11,
            "name": format!("thread {id}"), "owner_id": "u1",
            "message_count": 3, "member_count": 2, "last_message_id": id,
            "thread_metadata": {
                "archived": false, "locked": false, "auto_archive_duration": 1440,
                "archive_timestamp": "2026-11-09T12:00:00+00:00",
                "create_timestamp": "2026-11-08T12:00:00+00:00"
            },
            "applied_tags": ["t1"], "flags": 2
        })
    }

    /// Snowflake for a unix time (seconds).
    fn sf(unix: u64) -> u64 {
        (unix * 1000 - DISCORD_EPOCH_MS) << 22
    }

    fn t(id: &str, parent: &str) -> Thread {
        Thread::from_json(&thread_json(id, parent), None)
    }

    #[test]
    fn parses_thread() {
        let th = t("1100000000000000000", "p1");
        assert_eq!(th.guild_id, "g1");
        assert_eq!(th.parent_id, "p1");
        assert_eq!(th.kind, CHANNEL_PUBLIC_THREAD);
        assert_eq!(th.message_count, 3);
        assert_eq!(th.meta.auto_archive_duration, 1440);
        assert_eq!(th.meta.create_timestamp, 1_794_139_200);
        assert_eq!(th.applied_tags, vec!["t1"]);
        assert!(th.is_pinned());
        assert!(!th.joined);
        assert!(th.is_active());
    }

    #[test]
    fn parses_leniently() {
        let th = Thread::from_json(&json!({"id": "5", "member": {"id": "5"}}), Some("g9"));
        assert_eq!(th.guild_id, "g9");
        assert!(th.joined);
        assert_eq!(th.meta, ThreadMetadata::default());
        // Old threads without create_timestamp: from the snowflake.
        let id: u64 = 175928847299117063;
        let old = Thread::from_json(&json!({ "id": id.to_string() }), None);
        assert_eq!(old.created(), 1_462_015_105);
        assert_eq!(snowflake_time(0), 0);
    }

    #[test]
    fn parses_page_with_previews() {
        let v = json!({
            "threads": [thread_json("10", "f"), thread_json("11", "f")],
            "members": [{ "id": "11", "user_id": "me" }],
            "has_more": true,
            "total_results": 40,
            "first_messages": [{
                "id": "10", "channel_id": "10",
                "author": { "id": "u1", "username": "leo" },
                "content": "hello\n\nworld",
                "attachments": [{ "url": "https://x/a.png", "proxy_url": "https://p/a.png",
                    "filename": "a.png", "width": 320, "height": 160 }],
                "reactions": [{ "count": 2 }, { "count": 3 }]
            }]
        });
        let page = ThreadPage::from_json(&v, None);
        assert_eq!(page.threads.len(), 2);
        assert_eq!(page.joined, vec!["11"]);
        assert!(page.has_more);
        assert_eq!(page.total, Some(40));
        let (id, p) = &page.previews[0];
        assert_eq!(id, "10");
        assert_eq!(p.content, "hello world");
        assert_eq!(p.author.name, "leo");
        assert_eq!(p.reactions, 5);
        assert_eq!(
            p.image.as_deref(),
            Some("https://p/a.png?width=160&height=80")
        );
        assert_eq!(p.image_size, Some([320, 160]));
    }

    #[test]
    fn preview_uses_embed_image_and_truncates() {
        let long = "x".repeat(500);
        let p = PostPreview::from_message_json(&json!({
            "content": long,
            "embeds": [{ "thumbnail": { "url": "https://e/t.png" } }]
        }));
        assert_eq!(p.content.chars().count(), PREVIEW_CHARS + 1);
        assert_eq!(p.image.as_deref(), Some("https://e/t.png"));
    }

    #[test]
    fn dispatch_parsing() {
        assert!(ThreadEvent::from_dispatch("MESSAGE_CREATE", &json!({})).is_none());
        let del = ThreadEvent::from_dispatch(
            "THREAD_DELETE",
            &json!({ "id": "1", "guild_id": "g", "parent_id": "p", "type": 11 }),
        );
        assert_eq!(
            del,
            Some(ThreadEvent::Delete {
                id: "1".into(),
                parent_id: "p".into()
            })
        );
        let sync = ThreadEvent::from_dispatch(
            "THREAD_LIST_SYNC",
            &json!({ "guild_id": "g", "threads": [], "members": [] }),
        );
        assert!(matches!(
            sync,
            Some(ThreadEvent::ListSync {
                channel_ids: None,
                ..
            })
        ));
        let mu = ThreadEvent::from_dispatch(
            "THREAD_MEMBERS_UPDATE",
            &json!({ "id": "1", "guild_id": "g", "member_count": 4,
                "added_members": [{ "id": "1", "user_id": "me" }],
                "removed_member_ids": ["x"] }),
        );
        assert_eq!(
            mu,
            Some(ThreadEvent::MembersUpdate {
                thread_id: "1".into(),
                member_count: 4,
                added: vec!["me".into()],
                removed: vec!["x".into()]
            })
        );
        let g = ThreadEvent::from_guild(&json!({ "id": "g", "threads": [{ "id": "1" }] }));
        match g {
            Some(ThreadEvent::GuildThreads { guild_id, threads }) => {
                assert_eq!(guild_id, "g");
                assert_eq!(threads[0].guild_id, "g");
            }
            other => panic!("{other:?}"),
        }
        assert!(ThreadEvent::from_guild(&json!({ "id": "g" })).is_none());
    }

    #[test]
    fn create_update_delete() {
        let mut s = ThreadStore::new();
        let mut a = t("100", "p");
        a.joined = true;
        s.apply(ThreadEvent::Upsert(a));
        // THREAD_UPDATE has no member object: membership is kept.
        let mut upd = t("100", "p");
        upd.name = "renamed".into();
        s.apply(ThreadEvent::Upsert(upd));
        assert_eq!(s.get("100").unwrap().name, "renamed");
        assert!(s.get("100").unwrap().joined);
        assert_eq!(s.sidebar_threads("p", None).len(), 1);
        // Moved to another parent: re-indexed.
        s.apply(ThreadEvent::Upsert(t("100", "q")));
        assert_eq!(s.children("p").count(), 0);
        assert_eq!(s.children("q").count(), 1);
        s.apply(ThreadEvent::Delete {
            id: "100".into(),
            parent_id: "q".into(),
        });
        assert!(s.is_empty());
        assert_eq!(s.children("q").count(), 0);
    }

    #[test]
    fn list_sync_replaces_active_threads_of_listed_channels() {
        let mut s = ThreadStore::new();
        s.apply(ThreadEvent::Upsert(t("1", "a")));
        s.apply(ThreadEvent::Upsert(t("2", "b")));
        let mut archived = t("3", "a");
        archived.meta.archived = true;
        s.apply(ThreadEvent::Upsert(archived));
        s.apply(ThreadEvent::ListSync {
            guild_id: "g1".into(),
            channel_ids: Some(vec!["a".into()]),
            threads: vec![t("4", "a")],
            joined: vec!["4".into()],
        });
        assert!(s.get("1").is_none(), "stale active thread in a dropped");
        assert!(s.get("2").is_some(), "other channel untouched");
        assert!(s.get("3").is_some(), "archived threads kept");
        assert!(s.get("4").unwrap().joined);
        // Without channel_ids the whole guild is synced.
        s.apply(ThreadEvent::ListSync {
            guild_id: "g1".into(),
            channel_ids: None,
            threads: vec![],
            joined: vec![],
        });
        assert!(s.get("2").is_none() && s.get("4").is_none());
        assert!(s.get("3").is_some());
    }

    #[test]
    fn guild_threads_replace_active_set() {
        let mut s = ThreadStore::new();
        s.apply(ThreadEvent::Upsert(t("1", "a")));
        s.apply(ThreadEvent::GuildThreads {
            guild_id: "g1".into(),
            threads: vec![t("2", "a")],
        });
        assert!(s.get("1").is_none());
        assert!(s.get("2").is_some());
    }

    #[test]
    fn membership_events() {
        let mut s = ThreadStore::new();
        s.set_me("me");
        s.apply(ThreadEvent::Upsert(t("1", "a")));
        s.apply(ThreadEvent::MemberUpdate {
            thread_id: "1".into(),
        });
        assert!(s.get("1").unwrap().joined);
        s.apply(ThreadEvent::MembersUpdate {
            thread_id: "1".into(),
            member_count: 7,
            added: vec![],
            removed: vec!["me".into()],
        });
        let th = s.get("1").unwrap();
        assert!(!th.joined);
        assert_eq!(th.member_count, 7);
        s.apply(ThreadEvent::MembersUpdate {
            thread_id: "1".into(),
            member_count: 8,
            added: vec!["me".into()],
            removed: vec![],
        });
        assert!(s.get("1").unwrap().joined);
    }

    #[test]
    fn messages_bump_activity() {
        let mut s = ThreadStore::new();
        s.apply(ThreadEvent::Upsert(t("100", "a")));
        s.on_message("100", "200");
        s.on_message("100", "150"); // older: ignored
        s.on_message("999", "300"); // not a thread
        let th = s.get("100").unwrap();
        assert_eq!(th.last_message_id, 200);
        assert_eq!(th.message_count, 4);
        // A stale THREAD_UPDATE doesn't roll it back.
        s.apply(ThreadEvent::Upsert(t("100", "a")));
        assert_eq!(s.get("100").unwrap().last_message_id, 200);
    }

    #[test]
    fn sidebar_shows_joined_active_and_selected() {
        let mut s = ThreadStore::new();
        let mut joined = t("2", "a");
        joined.joined = true;
        let mut joined_old = t("1", "a");
        joined_old.joined = true;
        let mut joined_archived = t("3", "a");
        joined_archived.joined = true;
        joined_archived.meta.archived = true;
        for th in [joined, joined_old, joined_archived, t("4", "a")] {
            s.apply(ThreadEvent::Upsert(th));
        }
        let ids = |v: Vec<&Thread>| v.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(s.sidebar_threads("a", None)), ["1", "2"]);
        assert_eq!(ids(s.sidebar_threads("a", Some("4"))), ["1", "2", "4"]);
        assert_eq!(ids(s.active_threads("a")).len(), 3);
        assert_eq!(ids(s.archived_threads("a")), ["3"]);
    }

    #[test]
    fn store_caps_threads_and_prefers_archived_unjoined() {
        let mut s = ThreadStore::with_caps(10, 4);
        let mut keep = t("1", "a");
        keep.joined = true;
        s.apply(ThreadEvent::Upsert(keep));
        for i in 2..=20u64 {
            let mut th = t(&(i * 1000).to_string(), "a");
            th.meta.archived = i % 2 == 0;
            th.last_message_id = i << 22;
            s.apply(ThreadEvent::Upsert(th));
        }
        assert!(s.len() <= 10);
        assert!(s.get("1").is_some(), "joined active thread survives");
        // Index stays consistent with the map.
        assert_eq!(s.children("a").count(), s.len());
        assert!(s.children("a").all(|th| s.get(&th.id).is_some()));
    }

    #[test]
    fn previews_are_capped() {
        let mut s = ThreadStore::with_caps(100, 4);
        let page = ThreadPage {
            threads: (1..=8).map(|i| t(&i.to_string(), "f")).collect(),
            previews: (1..=8)
                .map(|i| (i.to_string(), PostPreview::default()))
                .collect(),
            ..Default::default()
        };
        s.apply_page(page);
        assert_eq!(s.len(), 8);
        assert!(s.previews.len() <= 4);
        s.remove_parent("f");
        assert!(s.is_empty());
        assert!(s.previews.is_empty());
    }

    #[test]
    fn forum_posts_filter_and_sort() {
        let mut s = ThreadStore::new();
        let mut a = t("1", "f");
        a.flags = 0;
        a.applied_tags = vec!["x".into()];
        a.last_message_id = sf(1_794_300_000);
        let mut b = t("2", "f");
        b.flags = 0;
        b.applied_tags = vec!["y".into()];
        b.last_message_id = sf(1_794_200_000);
        let mut pinned = t("3", "f");
        pinned.applied_tags = vec![];
        for th in [a, b, pinned] {
            s.apply(ThreadEvent::Upsert(th));
        }
        let ids = |v: Vec<&Thread>| v.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(s.forum_posts("f", SortOrder::LatestActivity, &[])),
            ["3", "1", "2"]
        );
        assert_eq!(
            ids(s.forum_posts("f", SortOrder::LatestActivity, &["y".into()])),
            ["2"]
        );
    }

    #[test]
    fn dialog_actions() {
        let d = ThreadDialog::from_message("c", "m", "  hello   there  ");
        assert_eq!(d.name, "hello there");
        assert_eq!(
            d.action(),
            ThreadAction::CreateThread {
                channel_id: "c".into(),
                message_id: Some("m".into()),
                name: "hello there".into(),
                private: false,
                auto_archive_minutes: 4320,
            }
        );
        let mut s = ThreadDialog::standalone("c");
        s.private = true;
        s.name = " x ".into();
        assert!(matches!(
            s.action(),
            ThreadAction::CreateThread { private: true, ref name, .. } if name == "x"
        ));
        assert_eq!(
            iso(1_794_225_600).as_deref(),
            Some("2026-11-09T12:00:00+00:00")
        );
    }

    fn card_msg(v: Value) -> crate::types::Message {
        crate::types::Message::from_json(&v)
    }

    #[test]
    fn card_thread_prefers_the_store() {
        let mut store = ThreadStore::new();
        let mut live = t("77", "p1");
        live.name = "renamed".into();
        live.message_count = 9;
        store.apply(ThreadEvent::Upsert(live));
        let m = card_msg(json!({
            "id": "5", "channel_id": "p1", "type": 18, "content": "ideas",
            "message_reference": { "channel_id": "77" },
        }));
        let th = card_thread(&m, &store, Some("g1")).unwrap();
        assert!(matches!(th, std::borrow::Cow::Borrowed(_)));
        assert_eq!(th.name, "renamed");
        assert_eq!(th.message_count, 9);
    }

    #[test]
    fn card_thread_built_from_the_message() {
        let store = ThreadStore::new();
        // Only the reference: name from the content, server from the caller.
        let m = card_msg(json!({
            "id": "5", "channel_id": "p1", "type": 18, "content": "ideas",
            "message_reference": { "channel_id": "77" },
        }));
        let th = card_thread(&m, &store, Some("g1")).unwrap();
        assert_eq!(
            (th.id.as_str(), th.name.as_str(), th.parent_id.as_str()),
            ("77", "ideas", "p1")
        );
        assert_eq!(th.guild_id, "g1");
        assert_eq!(th.message_count, 0);
        // The thread object wins for name and count.
        let m = card_msg(json!({
            "id": "5", "channel_id": "p1", "guild_id": "g2", "type": 18, "content": "old",
            "thread": { "id": "78", "name": "new name", "message_count": 4,
                        "last_message_id": "99" },
        }));
        let th = card_thread(&m, &store, Some("g1")).unwrap();
        assert_eq!(th.name, "new name");
        assert_eq!(th.guild_id, "g2");
        assert_eq!((th.message_count, th.last_message_id), (4, 99));
    }

    #[test]
    fn card_thread_falls_back_to_name() {
        let mut store = ThreadStore::new();
        let mut a = t("70", "p1");
        a.name = "ideas".into();
        let mut b = t("71", "p1");
        b.name = "ideas".into();
        let mut other = t("72", "p2");
        other.name = "ideas".into();
        for th in [a, b, other] {
            store.apply(ThreadEvent::Upsert(th));
        }
        let m = card_msg(json!({ "id": "5", "channel_id": "p1", "type": 18, "content": "ideas" }));
        // Newest thread of this channel with that name.
        assert_eq!(card_thread(&m, &store, None).unwrap().id, "71");
        let unknown =
            card_msg(json!({ "id": "5", "channel_id": "p1", "type": 18, "content": "nope" }));
        assert!(card_thread(&unknown, &store, None).is_none());
        let empty = card_msg(json!({ "id": "5", "channel_id": "p1", "type": 18 }));
        assert!(card_thread(&empty, &store, None).is_none());
    }

    fn plain(m: &crate::types::Message) -> String {
        crate::markdown::to_plain_text(
            &crate::markdown::parse(&m.content),
            &crate::markdown::NoLookup,
        )
    }

    fn thread_msgs(ids: &[(&str, &str)]) -> Vec<crate::types::Message> {
        ids.iter()
            .map(|(id, content)| {
                card_msg(json!({
                    "id": id, "channel_id": "77", "content": content,
                    "author": { "id": "9", "username": "sam", "global_name": "Sam" },
                }))
            })
            .collect()
    }

    #[test]
    fn card_preview_from_cached_messages() {
        let mut th = t("77", "p1");
        th.message_count = 2;
        th.last_message_id = 92;
        let msgs = thread_msgs(&[
            ("90", "first"),
            ("91", "hi"),
            (
                "92",
                "**big**
news  here",
            ),
        ]);
        let p = card_preview(&th, Some(&msgs), &mut plain);
        let last = p.last.unwrap();
        assert_eq!(last.author.name, "Sam");
        assert_eq!(last.snippet, "big news here");
        // More loaded than the stored count: the loaded number wins.
        assert_eq!(p.count, 3);
    }

    #[test]
    fn card_preview_without_messages() {
        let mut th = t("77", "p1");
        th.message_count = 4;
        let p = card_preview(&th, None, &mut plain);
        assert!(p.last.is_none());
        assert_eq!(p.count_text().as_deref(), Some("4 messages"));
        assert_eq!(p.summary(), "4 messages · Open to see messages");
        th.message_count = 1;
        let p = card_preview(&th, Some(&[]), &mut plain);
        assert_eq!(p.summary(), "1 message · Open to see messages");
        th.message_count = 0;
        let p = card_preview(&th, None, &mut plain);
        assert_eq!(p.count_text(), None);
        assert_eq!(p.summary(), "Open to see messages");
    }

    #[test]
    fn card_preview_skips_stale_and_system_messages() {
        // The cache stops before the thread's newest message: no last line.
        let mut th = t("77", "p1");
        th.message_count = 5;
        th.last_message_id = 99;
        let msgs = thread_msgs(&[("90", "a"), ("91", "b")]);
        let p = card_preview(&th, Some(&msgs), &mut plain);
        assert!(p.last.is_none());
        assert_eq!(p.count, 5);
        // System lines (a pin) are skipped; empty text gets a word.
        th.last_message_id = 0;
        let mut msgs = thread_msgs(&[("90", "")]);
        msgs[0]
            .attachments
            .push(crate::types::Attachment::from_json(
                &json!({ "url": "u", "filename": "a.png" }),
            ));
        msgs.push(card_msg(
            json!({ "id": "91", "channel_id": "77", "type": 6 }),
        ));
        let p = card_preview(&th, Some(&msgs), &mut plain);
        assert_eq!(p.last.unwrap().snippet, "Sent an attachment");
        // Long text is cut.
        let long = "x".repeat(500);
        let msgs = thread_msgs(&[("90", long.as_str())]);
        let s = card_preview(&th, Some(&msgs), &mut plain)
            .last
            .unwrap()
            .snippet;
        assert_eq!(s.chars().count(), CARD_SNIPPET_CHARS + 1);
    }
}
