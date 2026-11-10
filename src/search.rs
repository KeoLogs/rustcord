//! Message search, following Discord's own search box (Discord Messenger's
//! search is a stub, so this follows the official client).
//!
//! Syntax typed in the box (keys are case-insensitive, values may be quoted
//! when they contain spaces, everything else is free text):
//!
//! | filter            | meaning                                        |
//! |-------------------|------------------------------------------------|
//! | `from:user`       | author (name, @name, `<@id>` or a raw id)      |
//! | `mentions:user`   | messages mentioning that user                  |
//! | `has:link`        | link, embed, file, video, image, sound, sticker, poll |
//! | `before:2026-11-09` | sent before that (local) day                 |
//! | `after:2026-11-09`  | sent after that day                          |
//! | `during:2026-11-09` | sent on that day (`2026-11` / `2026` = month / year) |
//! | `in:channel`      | channel (servers only; name, #name, `<#id>`, id) |
//! | `pinned:true`     | only pinned (or `false`: only unpinned)        |
//!
//! Protocol (user accounts only; bots get 403 "Bots cannot use this endpoint"):
//! - `GET /guilds/{id}/messages/search` or `GET /channels/{id}/messages/search`
//!   (DMs and group DMs).
//! - Query string: `content`, repeated `author_id`, `mentions`, `has`,
//!   `channel_id` (servers only), `min_id` / `max_id` (snowflakes, exclusive),
//!   `pinned`, `sort_by` (`timestamp` | `relevance`), `sort_order`
//!   (`desc` | `asc`), `offset` (at most 9975), `include_nsfw` (servers only).
//!   25 results per page.
//! - 200: `{ total_results, messages: [[message], …], doing_deep_historical_index }`.
//!   Each inner array is one hit (older API versions put context messages
//!   around it and mark the hit with `hit: true`).
//! - 202: the server's index isn't built yet: `{ code: 110000,
//!   documents_indexed, retry_after }` (seconds). [`SearchPanel`] shows
//!   "indexing" once with a "Try again" button that unlocks after
//!   `retry_after`; it never retries (or polls) by itself.
//! - Typing searches after a ~400 ms pause ([`DEBOUNCE_SECS`]), only when
//!   the query is complete and changed; Enter searches at once.
//!
//! The module is data in, actions out: the app feeds results in with
//! [`SearchPanel::on_results`] / `on_indexing` / `on_error` and handles the
//! [`SearchAction`]s that [`SearchPanel::show`] returns. Network calls are
//! `Api::search` (backend side).

use std::ops::Range;
use std::time::Duration;

use chrono::{NaiveDate, NaiveTime, TimeDelta, TimeZone};
use eframe::egui::{self, Color32, RichText, Sense, vec2};
use serde_json::Value;

use crate::icons;
use crate::markdown;
use crate::theme::{Bold, Palette};
use crate::types::Message;

/// Discord's epoch (2015-01-01T00:00:00Z) in unix milliseconds.
pub const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
/// Results per page (Discord's fixed page size for user accounts).
pub const PAGE_SIZE: u32 = 25;
/// Largest offset Discord accepts; later results can't be paged to.
pub const MAX_OFFSET: u32 = 9975;
/// Characters of a result's text shown before it's cut off.
/// Autocomplete rows for users / channels.
const MAX_SUGGESTIONS: usize = 8;
/// Pause after typing before searching by itself.
pub const DEBOUNCE_SECS: f64 = 0.4;

// ---------------------------------------------------------------------------
// Snowflakes and dates
// ---------------------------------------------------------------------------

/// The smallest snowflake at a unix time in ms (0 before Discord's epoch).
pub fn snowflake_from_unix_ms(unix_ms: i64) -> u64 {
    let epoch = DISCORD_EPOCH_MS as i64;
    if unix_ms <= epoch {
        0
    } else {
        ((unix_ms - epoch) as u64) << 22
    }
}

/// When a snowflake was made, in unix ms.
#[cfg(test)]
pub fn snowflake_unix_ms(id: u64) -> i64 {
    ((id >> 22) + DISCORD_EPOCH_MS) as i64
}

/// Unix ms of the first moment of `day` in `tz` (local midnight; if a DST
/// change skips midnight, the first hour that exists).
pub fn day_start_ms<Tz: TimeZone>(day: NaiveDate, tz: &Tz) -> i64 {
    let midnight = day.and_time(NaiveTime::MIN);
    tz.from_local_datetime(&midnight)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(midnight + TimeDelta::hours(1)))
                .earliest()
        })
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| midnight.and_utc().timestamp_millis())
}

/// The smallest snowflake of a local day (what `before:` sends as `max_id`).
#[cfg(test)]
pub fn snowflake_for_day<Tz: TimeZone>(day: NaiveDate, tz: &Tz) -> u64 {
    snowflake_from_unix_ms(day_start_ms(day, tz))
}

/// `YYYY-MM-DD` (one day), `YYYY-MM` (a month) or `YYYY` (a year) as the
/// days `[start, end)`.
fn parse_date_span(s: &str) -> Option<(NaiveDate, NaiveDate)> {
    let parts: Vec<&str> = s.split('-').collect();
    let num = |p: &str| -> Option<u32> {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse().ok())
            .flatten()
    };
    let year =
        |p: &str| -> Option<i32> { (p.len() == 4).then(|| num(p)).flatten().map(|y| y as i32) };
    match parts.as_slice() {
        [y, m, d] => {
            let day = NaiveDate::from_ymd_opt(year(y)?, num(m)?, num(d)?)?;
            Some((day, day.succ_opt()?))
        }
        [y, m] => {
            let (y, m) = (year(y)?, num(m)?);
            let start = NaiveDate::from_ymd_opt(y, m, 1)?;
            let end = if m == 12 {
                NaiveDate::from_ymd_opt(y + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(y, m + 1, 1)?
            };
            Some((start, end))
        }
        [y] => {
            let y = year(y)?;
            Some((
                NaiveDate::from_ymd_opt(y, 1, 1)?,
                NaiveDate::from_ymd_opt(y + 1, 1, 1)?,
            ))
        }
        _ => None,
    }
}

fn is_snowflake(s: &str) -> bool {
    (15..=21).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Query syntax
// ---------------------------------------------------------------------------

/// `has:` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Has {
    Link,
    Embed,
    File,
    Video,
    Image,
    Sound,
    Sticker,
    Poll,
}

impl Has {
    pub const ALL: [Has; 8] = [
        Has::Link,
        Has::Embed,
        Has::File,
        Has::Video,
        Has::Image,
        Has::Sound,
        Has::Sticker,
        Has::Poll,
    ];

    /// The value in the box and in the API's `has` parameter.
    pub fn as_str(self) -> &'static str {
        match self {
            Has::Link => "link",
            Has::Embed => "embed",
            Has::File => "file",
            Has::Video => "video",
            Has::Image => "image",
            Has::Sound => "sound",
            Has::Sticker => "sticker",
            Has::Poll => "poll",
        }
    }

    pub fn parse(s: &str) -> Option<Has> {
        let s = s.to_ascii_lowercase();
        Has::ALL.into_iter().find(|h| h.as_str() == s)
    }
}

/// The filters the box understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKey {
    From,
    Mentions,
    Has,
    Before,
    During,
    After,
    In,
    Pinned,
}

impl FilterKey {
    /// In the order Discord lists them under "Search options".
    pub const ALL: [FilterKey; 8] = [
        FilterKey::From,
        FilterKey::Mentions,
        FilterKey::Has,
        FilterKey::Before,
        FilterKey::During,
        FilterKey::After,
        FilterKey::In,
        FilterKey::Pinned,
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            FilterKey::From => "from",
            FilterKey::Mentions => "mentions",
            FilterKey::Has => "has",
            FilterKey::Before => "before",
            FilterKey::During => "during",
            FilterKey::After => "after",
            FilterKey::In => "in",
            FilterKey::Pinned => "pinned",
        }
    }

    /// The grey hint after the keyword ("from: user").
    pub fn hint(self) -> &'static str {
        match self {
            FilterKey::From | FilterKey::Mentions => "user",
            FilterKey::Has => "link, embed or file",
            FilterKey::Before | FilterKey::During | FilterKey::After => "specific date",
            FilterKey::In => "channel",
            FilterKey::Pinned => "true or false",
        }
    }

    pub fn from_keyword(s: &str) -> Option<FilterKey> {
        FilterKey::ALL
            .into_iter()
            .find(|k| k.keyword().eq_ignore_ascii_case(s))
    }
}

/// Result order (Discord's "Newest / Oldest / Most relevant").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchSort {
    #[default]
    Newest,
    Oldest,
    Relevant,
}

impl SearchSort {
    pub const ALL: [SearchSort; 3] = [SearchSort::Newest, SearchSort::Oldest, SearchSort::Relevant];

    pub fn label(self) -> &'static str {
        match self {
            SearchSort::Newest => "Newest",
            SearchSort::Oldest => "Oldest",
            SearchSort::Relevant => "Most relevant",
        }
    }

    /// `(sort_by, sort_order)`.
    fn params(self) -> (&'static str, &'static str) {
        match self {
            SearchSort::Newest => ("timestamp", "desc"),
            SearchSort::Oldest => ("timestamp", "asc"),
            SearchSort::Relevant => ("relevance", "desc"),
        }
    }
}

/// Where a search runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchScope {
    /// A whole server.
    Guild(String),
    /// One DM or group DM.
    Dm(String),
}

impl SearchScope {
    pub fn is_dm(&self) -> bool {
        matches!(self, SearchScope::Dm(_))
    }

    /// Path under the API base.
    pub fn path(&self) -> String {
        match self {
            SearchScope::Guild(id) => format!("/guilds/{id}/messages/search"),
            SearchScope::Dm(id) => format!("/channels/{id}/messages/search"),
        }
    }
}

/// A parsed search, ready to send.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchQuery {
    /// Free text (words that aren't filters), as typed.
    pub content: String,
    pub author_ids: Vec<String>,
    pub mentions: Vec<String>,
    pub has: Vec<Has>,
    /// Exclusive lower bound (messages with a larger id).
    pub min_id: Option<u64>,
    /// Exclusive upper bound.
    pub max_id: Option<u64>,
    /// `in:` channels (servers only).
    pub channel_ids: Vec<String>,
    pub pinned: Option<bool>,
    pub sort: SearchSort,
    /// Include age-restricted channels (servers only).
    pub include_nsfw: bool,
}

impl SearchQuery {
    /// Nothing to search for (Discord does nothing on Enter then).
    pub fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
            && self.author_ids.is_empty()
            && self.mentions.is_empty()
            && self.has.is_empty()
            && self.min_id.is_none()
            && self.max_id.is_none()
            && self.channel_ids.is_empty()
            && self.pinned.is_none()
    }

    /// Query-string pairs for one page starting at `offset`.
    pub fn params(&self, offset: u32, scope: &SearchScope) -> Vec<(&'static str, String)> {
        let mut p: Vec<(&'static str, String)> = Vec::new();
        let content = self.content.trim();
        if !content.is_empty() {
            p.push(("content", content.to_string()));
        }
        p.extend(self.author_ids.iter().map(|id| ("author_id", id.clone())));
        p.extend(self.mentions.iter().map(|id| ("mentions", id.clone())));
        p.extend(self.has.iter().map(|h| ("has", h.as_str().to_string())));
        if let Some(id) = self.min_id {
            p.push(("min_id", id.to_string()));
        }
        if let Some(id) = self.max_id {
            p.push(("max_id", id.to_string()));
        }
        if !scope.is_dm() {
            p.extend(self.channel_ids.iter().map(|id| ("channel_id", id.clone())));
        }
        if let Some(pinned) = self.pinned {
            p.push(("pinned", pinned.to_string()));
        }
        let (by, order) = self.sort.params();
        p.push(("sort_by", by.into()));
        p.push(("sort_order", order.into()));
        let offset = offset.min(MAX_OFFSET);
        if offset > 0 {
            p.push(("offset", offset.to_string()));
        }
        if !scope.is_dm() && self.include_nsfw {
            p.push(("include_nsfw", "true".into()));
        }
        p
    }
}

/// Full request URL, e.g. `search_url("https://discord.com/api/v10", …)`.
pub fn search_url(api_base: &str, scope: &SearchScope, query: &SearchQuery, offset: u32) -> String {
    let base = format!("{api_base}{}", scope.path());
    match reqwest::Url::parse(&base) {
        Ok(mut url) => {
            url.query_pairs_mut()
                .extend_pairs(query.params(offset, scope));
            url.to_string()
        }
        Err(_) => base,
    }
}

/// Looks names up for `from:` / `mentions:` / `in:`. Each gets the name
/// without `@` / `#` and should match case-insensitively on username,
/// display name or nickname (channel name for `channel`).
pub struct Resolver<'a> {
    pub user: &'a dyn Fn(&str) -> Option<String>,
    pub channel: &'a dyn Fn(&str) -> Option<String>,
}

/// One filter as typed, for the chips under the box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub key: FilterKey,
    /// The value as typed (quotes removed).
    pub value: String,
    /// Byte range of the whole `key:value` in the input.
    pub range: Range<usize>,
    /// Why it can't be used (unknown user, bad date…). Not sent.
    pub problem: Option<String>,
}

/// What [`parse`] makes of the box.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Parsed {
    pub query: SearchQuery,
    pub filters: Vec<Filter>,
}

impl Parsed {
    pub fn problems(&self) -> impl Iterator<Item = &str> {
        self.filters.iter().filter_map(|f| f.problem.as_deref())
    }
}

/// Split the box into words, keeping `"quoted parts"` (spaces included)
/// inside one word. Returns byte ranges.
fn tokenize(input: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    let mut in_quote = false;
    for (i, c) in input.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
            start.get_or_insert(i);
        } else if c.is_whitespace() && !in_quote {
            if let Some(s) = start.take() {
                out.push(s..i);
            }
        } else {
            start.get_or_insert(i);
        }
    }
    if let Some(s) = start {
        out.push(s..input.len());
    }
    out
}

/// `key:value` with a known key → the key and the unquoted value.
fn split_filter(token: &str) -> Option<(FilterKey, &str)> {
    let (k, v) = token.split_once(':')?;
    Some((FilterKey::from_keyword(k)?, v.trim_matches('"')))
}

/// A user / channel given as `<@id>`, `<#id>`, a raw id or a name.
fn resolve(
    value: &str,
    sigils: &[char],
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let v = value.trim();
    let v = match v.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        Some(inner) => inner,
        None => v,
    };
    let v = v.trim_start_matches(sigils);
    if v.is_empty() {
        None
    } else if is_snowflake(v) {
        Some(v.to_string())
    } else {
        lookup(v)
    }
}

/// Parse the box using the local time zone for dates.
pub fn parse(input: &str, resolver: &Resolver<'_>) -> Parsed {
    parse_with_tz(input, resolver, &chrono::Local)
}

/// [`parse`] with an explicit time zone (dates are that zone's days).
pub fn parse_with_tz<Tz: TimeZone>(input: &str, resolver: &Resolver<'_>, tz: &Tz) -> Parsed {
    let mut query = SearchQuery::default();
    let mut filters = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    // Bounds in unix ms: messages at or after `lo`, before `hi`.
    let mut lo: Option<i64> = None;
    let mut hi: Option<i64> = None;

    fn push_unique<T: PartialEq>(v: &mut Vec<T>, x: T) {
        if !v.contains(&x) {
            v.push(x);
        }
    }

    for range in tokenize(input) {
        let token = &input[range.clone()];
        let Some((key, value)) = split_filter(token) else {
            words.push(token);
            continue;
        };
        if value.trim().is_empty() {
            // Half-typed ("from:") — neither a filter nor text yet.
            continue;
        }
        let mut problem = None;
        match key {
            FilterKey::From | FilterKey::Mentions => {
                match resolve(value, &['@', '!'], resolver.user) {
                    Some(id) if key == FilterKey::From => push_unique(&mut query.author_ids, id),
                    Some(id) => push_unique(&mut query.mentions, id),
                    None => problem = Some(format!("No user called “{value}”")),
                }
            }
            FilterKey::In => match resolve(value, &['#'], resolver.channel) {
                Some(id) => push_unique(&mut query.channel_ids, id),
                None => problem = Some(format!("No channel called “{value}”")),
            },
            FilterKey::Has => match Has::parse(value) {
                Some(h) => push_unique(&mut query.has, h),
                None => {
                    let all: Vec<&str> = Has::ALL.iter().map(|h| h.as_str()).collect();
                    problem = Some(format!("has: can be {}", all.join(", ")));
                }
            },
            FilterKey::Pinned => match value.to_ascii_lowercase().as_str() {
                "true" | "yes" => query.pinned = Some(true),
                "false" | "no" => query.pinned = Some(false),
                _ => problem = Some("pinned: can be true or false".into()),
            },
            FilterKey::Before | FilterKey::During | FilterKey::After => {
                match parse_date_span(value) {
                    Some((start, end)) => {
                        let (s, e) = (day_start_ms(start, tz), day_start_ms(end, tz));
                        let (new_lo, new_hi) = match key {
                            FilterKey::Before => (None, Some(s)),
                            FilterKey::After => (Some(e), None),
                            _ => (Some(s), Some(e)),
                        };
                        if let Some(l) = new_lo {
                            lo = Some(lo.map_or(l, |x| x.max(l)));
                        }
                        if let Some(h) = new_hi {
                            hi = Some(hi.map_or(h, |x| x.min(h)));
                        }
                    }
                    None => problem = Some("Use a date like 2026-11-09".into()),
                }
            }
        }
        filters.push(Filter {
            key,
            value: value.to_string(),
            range,
            problem,
        });
    }

    query.content = words.join(" ");
    // min_id is exclusive: the last id of the millisecond before `lo`.
    query.min_id = lo
        .map(snowflake_from_unix_ms)
        .filter(|&id| id > 0)
        .map(|id| id - 1);
    query.max_id = hi.map(snowflake_from_unix_ms);
    Parsed { query, filters }
}

/// Remove `range` (a filter) from the box, with one neighbouring space.
fn remove_range(input: &str, range: Range<usize>) -> String {
    let before = &input[..range.start];
    let after = &input[range.end..];
    let mut out = String::with_capacity(input.len());
    if let Some(a) = after.strip_prefix(' ') {
        out.push_str(before);
        out.push_str(a);
    } else if let Some(b) = before.strip_suffix(' ') {
        out.push_str(b);
        out.push_str(after);
    } else {
        out.push_str(before);
        out.push_str(after);
    }
    out
}

// ---------------------------------------------------------------------------
// API response
// ---------------------------------------------------------------------------

/// What `Api::search` got back.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchResponse {
    /// One page of hits (raw message objects; convert with
    /// `Message::from_json`).
    Page {
        total: u64,
        messages: Vec<Value>,
        /// Discord is still indexing older messages; some may be missing.
        partial: bool,
    },
    /// HTTP 202: the index isn't ready. Try again after `retry_after` s.
    Indexing {
        retry_after: f32,
        documents_indexed: u64,
    },
}

impl SearchResponse {
    pub fn from_json(status: u16, body: &Value) -> Self {
        if status == 202 {
            return SearchResponse::Indexing {
                retry_after: body
                    .get("retry_after")
                    .and_then(Value::as_f64)
                    .unwrap_or(2.0) as f32,
                documents_indexed: body
                    .get("documents_indexed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            };
        }
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .map(|hits| {
                hits.iter()
                    .filter_map(|hit| match hit {
                        // [context…, hit, context…] or just [hit].
                        Value::Array(group) => group
                            .iter()
                            .find(|m| m.get("hit").and_then(Value::as_bool) == Some(true))
                            .or_else(|| group.first())
                            .cloned(),
                        Value::Object(_) => Some(hit.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        SearchResponse::Page {
            total: body
                .get("total_results")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            messages,
            partial: body
                .get("doing_deep_historical_index")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Result text
// ---------------------------------------------------------------------------

/// Lower-case words of the free text to highlight in results.
pub fn highlight_terms(content: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in content.split_whitespace() {
        let w = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if !w.is_empty() && !terms.contains(&w) {
            terms.push(w);
        }
    }
    // Longest first so "testing" wins over "test" at the same spot.
    terms.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
    terms
}

/// Does `hay[start..]` start with `term` (already lower-case), ignoring
/// case? Returns the end byte.
fn match_at(hay: &str, start: usize, term: &str) -> Option<usize> {
    let mut want = term.chars().peekable();
    let mut end = start;
    for (j, c) in hay[start..].char_indices() {
        if want.peek().is_none() {
            break;
        }
        for lc in c.to_lowercase() {
            if want.next() != Some(lc) {
                return None;
            }
        }
        end = start + j + c.len_utf8();
    }
    (want.peek().is_none() && end > start).then_some(end)
}

/// Byte ranges of `hay` matching any term (case-insensitive), merged.
pub fn match_ranges(hay: &str, terms: &[String]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    if terms.is_empty() {
        return out;
    }
    for (i, _) in hay.char_indices() {
        if out.last().is_some_and(|r| i < r.end) {
            continue;
        }
        if let Some(end) = terms.iter().find_map(|t| match_at(hay, i, t)) {
            match out.last_mut() {
                Some(last) if last.end == i => last.end = end,
                _ => out.push(i..end),
            }
        }
    }
    out
}

/// "1,234".
fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Pages that can be reached for `total` results.
fn page_count(total: u64) -> u32 {
    let reachable = total.min(MAX_OFFSET as u64 + PAGE_SIZE as u64);
    reachable.div_ceil(PAGE_SIZE as u64) as u32
}

// ---------------------------------------------------------------------------
// Autocomplete
// ---------------------------------------------------------------------------

/// A user or channel offered after `from:` / `mentions:` / `in:`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// What goes into the box (a username or channel name).
    pub value: String,
    /// What the row shows (display name).
    pub label: String,
    /// Grey text after the label (e.g. the @username), may be empty.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SuggestItem {
    Key(FilterKey),
    Value { key: FilterKey, sugg: Suggestion },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Suggest {
    title: &'static str,
    /// The word being completed.
    token: Range<usize>,
    items: Vec<SuggestItem>,
    /// Pre-selected row (Enter takes it). None for filter names, so Enter
    /// still searches when a word happens to look like "in" or "has".
    default_selected: Option<usize>,
}

/// The word at the end of the box being typed (empty at a fresh word).
fn current_token(input: &str) -> Range<usize> {
    match tokenize(input).last() {
        Some(r) if r.end == input.len() && !input.ends_with(char::is_whitespace) => r.clone(),
        // An open quote runs to the end, trailing spaces included.
        Some(r) if r.end == input.len() && input[r.clone()].matches('"').count() % 2 == 1 => {
            r.clone()
        }
        _ => input.len()..input.len(),
    }
}

fn plain(s: &str) -> Suggestion {
    Suggestion {
        value: s.into(),
        label: s.into(),
        detail: String::new(),
    }
}

fn suggest(
    input: &str,
    in_dm: bool,
    users: &dyn Fn(&str, usize) -> Vec<Suggestion>,
    channels: &dyn Fn(&str, usize) -> Vec<Suggestion>,
    today: NaiveDate,
) -> Suggest {
    let token = current_token(input);
    let word = &input[token.clone()];
    let keys = |prefix: &str| -> Vec<SuggestItem> {
        FilterKey::ALL
            .into_iter()
            .filter(|k| !(in_dm && *k == FilterKey::In))
            .filter(|k| {
                k.keyword()
                    .get(..prefix.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
            })
            .map(SuggestItem::Key)
            .collect()
    };

    let Some((key, partial)) = split_filter(word) else {
        if word.contains(':') {
            return Suggest::default();
        }
        return Suggest {
            title: "Search options",
            token,
            items: keys(word),
            default_selected: None,
        };
    };
    if in_dm && key == FilterKey::In {
        return Suggest::default();
    }
    let lower = partial.to_lowercase();
    let starts = |s: &str| s.to_lowercase().starts_with(&lower);
    let (title, values): (&'static str, Vec<Suggestion>) = match key {
        FilterKey::From => (
            "From user",
            users(partial.trim_start_matches('@'), MAX_SUGGESTIONS),
        ),
        FilterKey::Mentions => (
            "Mentions user",
            users(partial.trim_start_matches('@'), MAX_SUGGESTIONS),
        ),
        FilterKey::In => (
            "In channel",
            channels(partial.trim_start_matches('#'), MAX_SUGGESTIONS),
        ),
        FilterKey::Has => (
            "Message contains",
            Has::ALL
                .iter()
                .map(|h| h.as_str())
                .filter(|h| starts(h))
                .map(plain)
                .collect(),
        ),
        FilterKey::Pinned => (
            "Pinned",
            ["true", "false"]
                .into_iter()
                .filter(|v| starts(v))
                .map(plain)
                .collect(),
        ),
        FilterKey::Before | FilterKey::During | FilterKey::After => {
            let days = [today, today.pred_opt().unwrap_or(today)];
            let labels = ["Today", "Yesterday"];
            (
                "Date (YYYY-MM-DD)",
                days.iter()
                    .zip(labels)
                    .map(|(d, label)| Suggestion {
                        value: d.format("%Y-%m-%d").to_string(),
                        label: label.into(),
                        detail: d.format("%Y-%m-%d").to_string(),
                    })
                    .filter(|s| starts(&s.value))
                    .collect(),
            )
        }
    };
    // Already typed in full: nothing left to complete.
    if values.len() == 1 && values[0].value.eq_ignore_ascii_case(partial) {
        return Suggest::default();
    }
    let items: Vec<SuggestItem> = values
        .into_iter()
        .map(|sugg| SuggestItem::Value { key, sugg })
        .collect();
    let default_selected = (!items.is_empty()).then_some(0);
    Suggest {
        title,
        token,
        items,
        default_selected,
    }
}

/// `value` quoted if it has spaces.
fn quote(value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

/// Put a suggestion in place of the word being typed.
fn apply_suggestion(input: &mut String, token: Range<usize>, item: &SuggestItem) {
    let text = match item {
        SuggestItem::Key(k) => format!("{}:", k.keyword()),
        SuggestItem::Value { key, sugg } => format!("{}:{} ", key.keyword(), quote(&sugg.value)),
    };
    input.replace_range(token, &text);
}

/// The box without its `in:` filters (and with spaces tidied).
fn strip_in_filters(input: &str) -> String {
    let mut out = input.to_string();
    for r in tokenize(input).into_iter().rev() {
        if split_filter(&input[r.clone()]).is_some_and(|(k, _)| k == FilterKey::In) {
            out = remove_range(&out, r);
        }
    }
    out.trim().to_string()
}

/// Add `key:` at the end of the box (from the idle "Search options" list).
fn append_key(input: &mut String, key: FilterKey) {
    if !input.is_empty() && !input.ends_with(' ') {
        input.push(' ');
    }
    input.push_str(key.keyword());
    input.push(':');
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

/// What the panel wants the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchAction {
    /// Run a search (send `ToBackend::Search`). Answer with the same `seq`.
    Search {
        seq: u64,
        query: SearchQuery,
        offset: u32,
    },
    /// Open the channel scrolled to the message.
    Jump {
        channel_id: String,
        message_id: String,
    },
    /// Esc pressed with nothing to dismiss: close the popup.
    Close,
}

/// Everything the panel needs from the app, per frame.
pub struct SearchCtx<'a> {
    pub pal: &'a Palette,
    /// Placeholder: "Search Pegasi Peripherals" / "Search @leo".
    pub placeholder: &'a str,
    /// DMs have no `in:`.
    pub in_dm: bool,
    pub include_nsfw: bool,
    /// The images module: avatars or plain letter discs.
    pub show_images: bool,
    pub resolver: Resolver<'a>,
    /// Users whose name/username starts with (or contains) the text, best
    /// first, at most `n`.
    pub suggest_users: &'a dyn Fn(&str, usize) -> Vec<Suggestion>,
    pub suggest_channels: &'a dyn Fn(&str, usize) -> Vec<Suggestion>,
    /// Names for channel headers.
    pub channel_name: &'a dyn Fn(&str) -> Option<String>,
    /// Mentions, roles and channels in result text (rendered as markdown
    /// like the chat).
    pub lookup: &'a dyn markdown::Lookup,
    /// Settings > Chat formatting; off = plain text.
    pub formatting: bool,
    /// The Emoji images module.
    pub emoji_images: bool,
    /// Height of the results list; None = fill the space below the box
    /// (side panel). Popups should set it (their height starts tiny).
    pub list_height: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum Status {
    #[default]
    Idle,
    Loading,
    Indexing {
        retry_after: f32,
        /// `ui` time the "Try again" button unlocks (set on the first
        /// frame after the answer).
        deadline: Option<f64>,
        documents: u64,
    },
    Done,
    Error(String),
    /// Filters that can't be used; nothing was sent.
    Invalid(String),
}

/// The search box and its results. Keep one per window; call
/// [`reset`](Self::reset) when the server / DM being searched changes.
#[derive(Default)]
pub struct SearchPanel {
    pub input: String,
    sort: SearchSort,
    status: Status,
    /// Bumped per request; answers with another seq are stale.
    seq: u64,
    query: Option<SearchQuery>,
    terms: Vec<String>,
    offset: u32,
    total: u64,
    partial: bool,
    results: Vec<Message>,
    /// Parsed markdown of each result (same order), parsed once per page.
    blocks: Vec<Vec<markdown::Block>>,
    /// Parse of `input` (re-done when it changes or on Enter).
    parsed: Option<(String, Parsed)>,
    sugg_for: String,
    sugg_selected: Option<usize>,
    sugg_touched: bool,
    sugg_dismissed: bool,
    sugg_rect: Option<egui::Rect>,
    focus_next: bool,
    cursor_to_end: bool,
    scroll_top: bool,
    /// `(name, id)` of the channel [`open`](Self::open) scoped to, so
    /// `in:name` resolves to that exact channel even if names repeat.
    scoped_channel: Option<(String, String)>,
    /// `ui` time of the last edit in the box (debounced search).
    typed_at: Option<f64>,
}

impl SearchPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ctrl+F. `Some((channel_id, channel_name))` scopes the search to that
    /// channel (`in:name ` is put first in the box, replacing other `in:`
    /// filters); `None` searches the whole server / DM (removes `in:`
    /// filters). Other text in the box is kept; the box gets focus.
    pub fn open(&mut self, channel: Option<(&str, &str)>) {
        let rest = strip_in_filters(&self.input);
        self.scoped_channel = channel.map(|(id, name)| (name.to_string(), id.to_string()));
        self.input = match channel {
            Some((_, name)) => format!("in:{} {rest}", quote(name)),
            None => rest,
        };
        self.parsed = None;
        self.focus_next = true;
        self.cursor_to_end = true;
    }

    /// Forget the search and results (scope changed or popup closed for
    /// good). In-flight answers are dropped.
    pub fn reset(&mut self) {
        let seq = self.seq + 1;
        *self = Self {
            seq,
            sort: self.sort,
            scoped_channel: self.scoped_channel.take(),
            ..Self::default()
        };
    }

    /// Demo mode only (screenshots): type a query, or show the indexing /
    /// error state.
    pub fn demo(&mut self, view: &str) {
        match view {
            "search-indexing" => {
                self.input = "release".into();
                self.status = Status::Indexing {
                    retry_after: 8.0,
                    deadline: None,
                    documents: 1234,
                };
            }
            "search-error" => {
                self.input = "release".into();
                self.status = Status::Error("Search failed (HTTP 500). Try again.".into());
            }
            "search-empty" => {
                self.input = "zebra crossing".into();
                self.typed_at = Some(0.0);
            }
            _ => {
                self.input = "the".into();
                self.typed_at = Some(0.0);
            }
        }
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        matches!(self.status, Status::Loading | Status::Indexing { .. })
    }

    /// A page of results arrived.
    pub fn on_results(&mut self, seq: u64, total: u64, messages: Vec<Message>, partial: bool) {
        if seq != self.seq {
            return;
        }
        self.total = total;
        self.blocks = messages
            .iter()
            .map(|m| markdown::parse(&m.content))
            .collect();
        self.results = messages;
        self.partial = partial;
        self.status = Status::Done;
        self.scroll_top = true;
    }

    /// HTTP 202: Discord is indexing. Shown once; the user retries.
    pub fn on_indexing(&mut self, seq: u64, retry_after: f32, documents_indexed: u64) {
        if seq != self.seq {
            return;
        }
        self.status = Status::Indexing {
            retry_after: retry_after.clamp(1.0, 30.0),
            deadline: None,
            documents: documents_indexed,
        };
    }

    pub fn on_error(&mut self, seq: u64, error: String) {
        if seq != self.seq {
            return;
        }
        self.status = Status::Error(error);
    }

    fn parsed(&mut self, resolver: &Resolver<'_>, fresh: bool) -> &Parsed {
        let stale = fresh || self.parsed.as_ref().is_none_or(|(s, _)| *s != self.input);
        if stale {
            let scoped = &self.scoped_channel;
            let channel = |name: &str| match scoped {
                Some((n, id)) if n.eq_ignore_ascii_case(name) => Some(id.clone()),
                _ => (resolver.channel)(name),
            };
            let resolver = Resolver {
                user: resolver.user,
                channel: &channel,
            };
            self.parsed = Some((self.input.clone(), parse(&self.input, &resolver)));
        }
        &self.parsed.as_ref().expect("just parsed").1
    }

    /// Enter: start a new search from the box.
    fn submit(&mut self, cx: &SearchCtx<'_>) -> Option<SearchAction> {
        let parsed = self.parsed(&cx.resolver, true).clone();
        self.begin(parsed, cx.include_nsfw)
    }

    /// Debounced search while typing: only a complete, valid query that
    /// differs from the one shown (no error flashes for half-typed filters).
    fn auto_submit(&mut self, cx: &SearchCtx<'_>) -> Option<SearchAction> {
        let parsed = self.parsed(&cx.resolver, true).clone();
        if parsed.problems().next().is_some() || parsed.query.is_empty() {
            return None;
        }
        let mut q = parsed.query.clone();
        q.sort = self.sort;
        q.include_nsfw = cx.include_nsfw;
        if self.query.as_ref() == Some(&q) {
            return None;
        }
        self.begin(parsed, cx.include_nsfw)
    }

    fn begin(&mut self, parsed: Parsed, include_nsfw: bool) -> Option<SearchAction> {
        let problems: Vec<&str> = parsed.problems().collect();
        if !problems.is_empty() {
            self.status = Status::Invalid(problems.join("\n"));
            return None;
        }
        let mut query = parsed.query;
        if query.is_empty() {
            return None;
        }
        query.sort = self.sort;
        query.include_nsfw = include_nsfw;
        self.terms = highlight_terms(&query.content);
        self.query = Some(query);
        self.offset = 0;
        self.total = 0;
        self.resend()
    }

    /// (Re)send the current query at the current offset.
    fn resend(&mut self) -> Option<SearchAction> {
        let query = self.query.clone()?;
        self.seq += 1;
        self.status = Status::Loading;
        self.results = Vec::new();
        self.blocks = Vec::new();
        Some(SearchAction::Search {
            seq: self.seq,
            query,
            offset: self.offset,
        })
    }

    fn goto_offset(&mut self, offset: u32) -> Option<SearchAction> {
        self.offset = offset.min(MAX_OFFSET);
        self.resend()
    }

    fn set_sort(&mut self, sort: SearchSort) -> Option<SearchAction> {
        if sort == self.sort {
            return None;
        }
        self.sort = sort;
        let q = self.query.as_mut()?;
        q.sort = sort;
        self.goto_offset(0)
    }

    /// Draw the box and results. Returns what the app should do.
    pub fn show(&mut self, ui: &mut egui::Ui, cx: &SearchCtx<'_>) -> Option<SearchAction> {
        let pal = cx.pal;
        let mut action = None;
        let id = egui::Id::unique("rustcord_search_box");

        // --- Autocomplete state -------------------------------------------
        if self.sugg_for != self.input {
            self.sugg_for.clone_from(&self.input);
            self.sugg_touched = false;
            self.sugg_dismissed = false;
        }
        let focused = ui.memory(|m| m.has_focus(id));
        let over_list = self.sugg_rect.is_some_and(|r| ui.rect_contains_pointer(r));
        let sugg = if self.sugg_dismissed || !(focused || over_list) {
            Suggest::default()
        } else {
            let today = chrono::Local::now().date_naive();
            suggest(
                &self.input,
                cx.in_dm,
                cx.suggest_users,
                cx.suggest_channels,
                today,
            )
        };
        let n = sugg.items.len();
        if !self.sugg_touched {
            self.sugg_selected = sugg.default_selected;
        }
        let mut accept: Option<usize> = None;
        if focused {
            let key = |k: egui::Key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
            if n > 0 {
                if key(egui::Key::ArrowDown) {
                    self.sugg_selected = Some(self.sugg_selected.map_or(0, |s| (s + 1) % n));
                    self.sugg_touched = true;
                }
                if key(egui::Key::ArrowUp) {
                    self.sugg_selected =
                        Some(self.sugg_selected.map_or(n - 1, |s| (s + n - 1) % n));
                    self.sugg_touched = true;
                }
                if key(egui::Key::Tab) {
                    accept = Some(self.sugg_selected.unwrap_or(0));
                }
                if self.sugg_selected.is_some() && accept.is_none() && key(egui::Key::Enter) {
                    accept = self.sugg_selected;
                }
            }
            if key(egui::Key::Escape) {
                if n > 0 {
                    self.sugg_dismissed = true;
                } else {
                    action = Some(SearchAction::Close);
                }
            }
        }
        if let Some(i) = accept {
            self.accept(&sugg, i);
        }

        // --- Search box ---------------------------------------------------
        if std::mem::take(&mut self.cursor_to_end)
            && let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id)
        {
            let end = egui::text::CCursor::new(self.input.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), id);
        }
        if std::mem::take(&mut self.focus_next) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let box_fill = if pal.dark { pal.servers } else { pal.input };
        egui::Frame::new()
            .fill(box_fill)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let icon_w = 20.0 + ui.spacing().item_spacing.x;
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .id(id)
                            .hint_text(RichText::new(cx.placeholder).color(pal.muted))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_width(ui.available_width() - icon_w),
                    );
                    if resp.changed() {
                        self.typed_at = Some(ui.input(|i| i.time));
                    }
                    if resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && action.is_none()
                    {
                        self.typed_at = None;
                        action = self.submit(cx);
                        self.sugg_dismissed = true;
                    }
                    if self.input.is_empty() {
                        ui.label(RichText::new(icons::MAGNIFYING_GLASS).color(pal.muted));
                    } else if ui
                        .add(
                            egui::Button::new(RichText::new(icons::X).color(pal.muted))
                                .frame(false),
                        )
                        .on_hover_text("Clear search")
                        .clicked()
                    {
                        self.reset();
                        self.focus_next = true;
                    }
                });
            });

        // --- Filter chips ---------------------------------------------------
        let filters = self.parsed(&cx.resolver, false).filters.clone();
        if !filters.is_empty() {
            let mut remove = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for f in &filters {
                    let text =
                        RichText::new(format!("{}: {}  {}", f.key.keyword(), f.value, icons::X))
                            .size(12.0)
                            .color(if f.problem.is_some() {
                                pal.danger
                            } else {
                                pal.text
                            });
                    let mut chip = egui::Button::new(text).fill(pal.selected).corner_radius(10);
                    if f.problem.is_some() {
                        chip = chip.stroke(egui::Stroke::new(1.0, pal.danger));
                    }
                    let tip = f.problem.as_deref().unwrap_or("Remove filter");
                    if ui.add(chip).on_hover_text(tip).clicked() {
                        remove = Some(f.range.clone());
                    }
                }
            });
            if let Some(r) = remove {
                self.input = remove_range(&self.input, r);
                self.focus_next = true;
                self.cursor_to_end = true;
            }
        }

        // --- Suggestions ----------------------------------------------------
        if n > 0 {
            let frame = egui::Frame::new()
                .fill(if pal.dark { pal.servers } else { pal.chat })
                .corner_radius(6)
                .inner_margin(6)
                .show(ui, |ui| {
                    suggestion_list(ui, sugg.title, &sugg.items, self.sugg_selected, pal)
                });
            self.sugg_rect = Some(frame.response.rect);
            if let Some(i) = frame.inner {
                self.accept(&sugg, i);
            }
        } else {
            self.sugg_rect = None;
        }

        // --- Body -------------------------------------------------------------
        ui.add_space(4.0);
        // Debounced search while typing (one request per pause).
        if let Some(t) = self.typed_at {
            let now = ui.input(|i| i.time);
            if now - t >= DEBOUNCE_SECS {
                self.typed_at = None;
                if action.is_none() {
                    action = self.auto_submit(cx);
                }
            } else {
                ui.ctx()
                    .request_repaint_after(Duration::from_secs_f64(DEBOUNCE_SECS - (now - t)));
            }
        }
        // Indexing: the "Try again" button unlocks after retry_after. No
        // automatic retry.
        let mut indexing_wait = None;
        if let Status::Indexing {
            retry_after,
            deadline,
            ..
        } = &mut self.status
        {
            let now = ui.input(|i| i.time);
            let at = *deadline.get_or_insert(now + f64::from(*retry_after));
            if now < at {
                indexing_wait = Some((at - now).ceil() as u64);
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
        }
        match self.status.clone() {
            Status::Idle => {
                if n == 0 {
                    let items: Vec<SuggestItem> = FilterKey::ALL
                        .into_iter()
                        .filter(|k| !(cx.in_dm && *k == FilterKey::In))
                        .map(SuggestItem::Key)
                        .collect();
                    if let Some(i) = suggestion_list(ui, "Search options", &items, None, pal)
                        && let SuggestItem::Key(k) = items[i]
                    {
                        append_key(&mut self.input, k);
                        self.focus_next = true;
                        self.cursor_to_end = true;
                    }
                }
            }
            Status::Loading => centered_note(ui, pal, true, "Searching…", None),
            Status::Indexing { documents, .. } => {
                let what = if cx.in_dm { "conversation" } else { "server" };
                let detail = (documents > 0)
                    .then(|| format!("{} messages indexed so far", group_digits(documents)));
                centered_note(
                    ui,
                    pal,
                    false,
                    &format!("Discord is still indexing this {what}."),
                    detail.as_deref(),
                );
                ui.vertical_centered(|ui| {
                    let label = match indexing_wait {
                        Some(s) => format!("Try again in {s}s"),
                        None => "Try again".to_string(),
                    };
                    if ui
                        .add_enabled(indexing_wait.is_none(), pal.primary_button(&label))
                        .clicked()
                    {
                        action = self.resend();
                    }
                });
            }
            Status::Error(e) => {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Search failed").color(pal.danger).bold());
                    ui.label(RichText::new(e).small().color(pal.muted));
                    ui.add_space(6.0);
                    if ui.add(pal.primary_button("Try again")).clicked() {
                        action = self.resend();
                    }
                });
            }
            Status::Invalid(e) => {
                ui.add_space(8.0);
                ui.label(RichText::new(e).color(pal.danger));
            }
            Status::Done => {
                if let Some(a) = self.results_ui(ui, cx) {
                    action = Some(a);
                }
            }
        }
        action
    }

    fn accept(&mut self, sugg: &Suggest, i: usize) {
        if let Some(item) = sugg.items.get(i) {
            apply_suggestion(&mut self.input, sugg.token.clone(), item);
            self.cursor_to_end = true;
            self.focus_next = true;
            self.sugg_touched = false;
        }
    }

    fn results_ui(&mut self, ui: &mut egui::Ui, cx: &SearchCtx<'_>) -> Option<SearchAction> {
        let pal = cx.pal;
        let mut action = None;

        ui.horizontal(|ui| {
            let label = if self.total == 1 {
                "1 Result".to_string()
            } else {
                format!("{} Results", group_digits(self.total))
            };
            ui.label(RichText::new(label).color(pal.strong).bold());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut sort = self.sort;
                egui::ComboBox::from_id_salt("search_sort")
                    .selected_text(sort.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for s in SearchSort::ALL {
                            ui.selectable_value(&mut sort, s, s.label());
                        }
                    });
                if let Some(a) = self.set_sort(sort) {
                    action = Some(a);
                }
            });
        });
        if self.partial {
            ui.label(
                RichText::new("Discord is still indexing older messages, so some may be missing.")
                    .small()
                    .color(pal.muted),
            );
        }

        if self.results.is_empty() {
            centered_note(
                ui,
                pal,
                false,
                "No results found",
                Some("Try different words, or remove some filters."),
            );
            return action;
        }

        let pages = page_count(self.total);
        let pager_h = if pages > 1 { 34.0 } else { 0.0 };
        let height = cx
            .list_height
            .unwrap_or_else(|| ui.available_height() - pager_h)
            .max(80.0);
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("search_results")
            .auto_shrink([false, true])
            // An Area's ui has little free height on its own: ask for the
            // whole list height once there's more than fits.
            .min_scrolled_height(height)
            .max_height(height);
        if std::mem::take(&mut self.scroll_top) {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            let mut last_channel: Option<&str> = None;
            for (i, m) in self.results.iter().enumerate() {
                if last_channel != Some(m.channel_id.as_str()) {
                    last_channel = Some(&m.channel_id);
                    ui.add_space(6.0);
                    let name = (cx.channel_name)(&m.channel_id);
                    let header = match name {
                        Some(n) if cx.in_dm => n,
                        Some(n) => format!("# {n}"),
                        None if cx.in_dm => "Direct message".into(),
                        None => "# unknown-channel".into(),
                    };
                    ui.label(RichText::new(header).size(12.0).color(pal.muted).bold());
                }
                if result_card(ui, m, self.blocks.get(i), &self.terms, cx) {
                    action = Some(SearchAction::Jump {
                        channel_id: m.channel_id.clone(),
                        message_id: m.id.clone(),
                    });
                }
            }
        });

        if pages > 1 {
            let page = self.offset / PAGE_SIZE + 1;
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(page > 1, egui::Button::new("◀ Back"))
                    .clicked()
                {
                    action = self.goto_offset(self.offset.saturating_sub(PAGE_SIZE));
                }
                ui.label(RichText::new(format!("Page {page} of {pages}")).color(pal.muted));
                if ui
                    .add_enabled(page < pages, egui::Button::new("Next ▶"))
                    .clicked()
                {
                    action = self.goto_offset(self.offset + PAGE_SIZE);
                }
            });
        }
        action
    }
}

/// "Search options" / value rows. Returns the clicked row.
fn suggestion_list(
    ui: &mut egui::Ui,
    title: &str,
    items: &[SuggestItem],
    selected: Option<usize>,
    pal: &Palette,
) -> Option<usize> {
    let mut clicked = None;
    ui.label(pal.section_label(title));
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, item) in items.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() || selected == Some(i) {
            ui.painter().rect_filled(rect, 4, pal.hover);
        }
        let (main, extra, bold) = match item {
            SuggestItem::Key(k) => (format!("{}:", k.keyword()), k.hint().to_string(), true),
            SuggestItem::Value { sugg, .. } => (sugg.label.clone(), sugg.detail.clone(), false),
        };
        let font = if bold {
            crate::theme::bold_font(14.5)
        } else {
            egui::FontId::proportional(14.5)
        };
        let p = ui.painter();
        let x = rect.left() + 8.0;
        let main_g = p.layout_no_wrap(main, font, pal.strong);
        let w = main_g.size().x;
        p.galley(
            egui::pos2(x, rect.center().y - main_g.size().y / 2.0),
            main_g,
            pal.strong,
        );
        if !extra.is_empty() {
            p.text(
                egui::pos2(x + w + 6.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                extra,
                egui::FontId::proportional(13.0),
                pal.muted,
            );
        }
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

fn centered_note(
    ui: &mut egui::Ui,
    pal: &Palette,
    spinner: bool,
    text: &str,
    detail: Option<&str>,
) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        if spinner {
            ui.spinner();
            ui.add_space(4.0);
        }
        ui.label(RichText::new(text).color(pal.text));
        if let Some(d) = detail {
            ui.label(RichText::new(d).small().color(pal.muted));
        }
    });
    ui.add_space(16.0);
}

/// One result: avatar, name, time and the message rendered as markdown
/// (spoilers stay hidden until clicked; clicking one reveals it rather than
/// jumping). Returns true when the card, its text or its Jump button was
/// clicked.
fn result_card(
    ui: &mut egui::Ui,
    m: &Message,
    blocks: Option<&Vec<markdown::Block>>,
    terms: &[String],
    cx: &SearchCtx<'_>,
) -> bool {
    let pal = cx.pal;
    // Spoiler reveal state is per card, apart from the chat's.
    let key = format!("search:{}", m.id);
    let mut jump = false;
    // The card's own click sense sits under its contents, so spoilers and
    // the Jump button get their clicks first.
    let card = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
        let mut prepared = egui::Frame::new()
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .outer_margin(egui::Margin::symmetric(0, 2))
            .begin(ui);
        {
            let ui = &mut prepared.content_ui;
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let size = 32.0;
                let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
                avatar(ui, rect, m, cx);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        // Room for the Jump button on the right.
                        ui.set_max_width((ui.available_width() - 56.0).max(40.0));
                        ui.add(
                            egui::Label::new(
                                RichText::new(&m.author.name).color(pal.strong).bold(),
                            )
                            .truncate()
                            .selectable(false),
                        );
                        ui.label(
                            RichText::new(crate::timefmt::full(m.time))
                                .small()
                                .color(pal.muted),
                        )
                        .on_hover_text(crate::timefmt::long(m.time));
                    });
                    if !m.content.is_empty() {
                        let plain;
                        let blocks = match blocks {
                            Some(b) if cx.formatting => b.as_slice(),
                            _ => {
                                plain = markdown::plain(&m.content);
                                plain.as_slice()
                            }
                        };
                        let mut rctx = markdown::RenderCtx::new(pal, cx.lookup, &key);
                        rctx.emoji_images = cx.emoji_images;
                        rctx.bold_family = Some(crate::theme::bold_family());
                        rctx.highlight = terms;
                        let out = markdown::render(ui, blocks, &rctx);
                        // Text, mentions and links open the result too.
                        jump |= out.action.is_some() || out.response.is_some_and(|r| r.clicked());
                    }
                    for a in &m.attachments {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} {}  ({})",
                                    icons::PAPERCLIP,
                                    a.filename,
                                    a.human_size()
                                ))
                                .small()
                                .color(pal.muted),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    }
                    if m.content.is_empty() && m.attachments.is_empty() {
                        ui.label(RichText::new("(no text)").italics().color(pal.muted));
                    }
                });
            });
        }
        let outer = prepared.allocate_space(ui);
        let hovered = ui.rect_contains_pointer(outer.rect);
        prepared.frame.fill = if hovered { pal.msg_hover } else { pal.chat };
        prepared.paint(ui);
        if hovered {
            // Sized to its text and never wrapped (the "Jum/p" bug), at the
            // card's top right.
            let btn = egui::Button::new(RichText::new("Jump").size(12.0).color(pal.text))
                .fill(pal.selected)
                .corner_radius(4)
                .wrap_mode(egui::TextWrapMode::Extend)
                .min_size(vec2(44.0, 20.0));
            let row = egui::Rect::from_min_max(
                egui::pos2(outer.rect.left(), outer.rect.top() + 8.0),
                egui::pos2(outer.rect.right() - 10.0, outer.rect.top() + 28.0),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            jump |= child.add(btn).clicked();
        }
    });
    let resp = card
        .response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    jump || resp.clicked()
}

fn avatar(ui: &egui::Ui, rect: egui::Rect, m: &Message, cx: &SearchCtx<'_>) {
    let pal = cx.pal;
    let r = rect.width() / 2.0;
    if cx.show_images && !m.author.avatar_url.is_empty() {
        ui.painter().circle_filled(rect.center(), r, pal.hover);
        egui::Image::from_uri(&m.author.avatar_url)
            .corner_radius(r)
            .show_loading_spinner(false)
            .paint_at(ui, rect);
    } else {
        ui.painter().circle_filled(rect.center(), r, pal.accent);
        let letter = m
            .author
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter,
            crate::theme::bold_font(14.5),
            Color32::WHITE,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    fn users(name: &str) -> Option<String> {
        match name.to_lowercase().as_str() {
            "leo" => Some("111111111111111111".into()),
            "big bob" => Some("222222222222222222".into()),
            _ => None,
        }
    }

    fn channels(name: &str) -> Option<String> {
        (name.eq_ignore_ascii_case("general")).then(|| "333333333333333333".into())
    }

    fn resolver() -> Resolver<'static> {
        Resolver {
            user: &users,
            channel: &channels,
        }
    }

    fn p(input: &str) -> Parsed {
        parse_with_tz(input, &resolver(), &Utc)
    }

    #[test]
    fn tokenizes_with_quotes() {
        let s = r#"hi  from:"big bob" there"#;
        let t: Vec<&str> = tokenize(s).into_iter().map(|r| &s[r]).collect();
        assert_eq!(t, vec!["hi", r#"from:"big bob""#, "there"]);
        let open = r#"from:"big b"#;
        assert_eq!(tokenize(open), vec![0..open.len()]);
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn parses_text_and_filters() {
        let parsed = p(
            r#"hello FROM:leo world has:image has:link has:image in:#general pinned:true from:"big bob""#,
        );
        let q = &parsed.query;
        assert_eq!(q.content, "hello world");
        assert_eq!(
            q.author_ids,
            vec!["111111111111111111", "222222222222222222"]
        );
        assert_eq!(q.has, vec![Has::Image, Has::Link]);
        assert_eq!(q.channel_ids, vec!["333333333333333333"]);
        assert_eq!(q.pinned, Some(true));
        assert_eq!(parsed.problems().count(), 0);
        assert_eq!(parsed.filters.len(), 7);
        assert_eq!(parsed.filters[1].value, "image");
        assert_eq!(parsed.filters.last().unwrap().value, "big bob");
    }

    #[test]
    fn ids_and_mentions_resolve_without_lookup() {
        let q = p("from:<@!123456789012345678> mentions:@leo mentions:987654321098765432 in:<#444444444444444444>").query;
        assert_eq!(q.author_ids, vec!["123456789012345678"]);
        assert_eq!(q.mentions, vec!["111111111111111111", "987654321098765432"]);
        assert_eq!(q.channel_ids, vec!["444444444444444444"]);
        assert!(q.content.is_empty());
    }

    #[test]
    fn problems_and_half_typed_filters() {
        let parsed = p("from:nobody has:gif pinned:maybe before:2026-13-01 in:random");
        let problems: Vec<&str> = parsed.problems().collect();
        assert_eq!(problems.len(), 5);
        assert!(problems[0].contains("nobody"));
        assert!(parsed.query.is_empty());

        // "from:" with nothing after it is ignored; URLs and unknown keys stay text.
        let q = p("from: https://example.com note:x").query;
        assert_eq!(q.content, "https://example.com note:x");
        assert!(q.author_ids.is_empty());
        assert!(p("").query.is_empty());
        assert!(p("from:").query.is_empty());
    }

    #[test]
    fn date_filters_become_snowflake_bounds() {
        let day_ms = |y, m, d| {
            Utc.with_ymd_and_hms(y, m, d, 0, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let q = p("during:2026-11-09").query;
        assert_eq!(
            q.min_id,
            Some(snowflake_from_unix_ms(day_ms(2026, 11, 9)) - 1)
        );
        assert_eq!(q.max_id, Some(snowflake_from_unix_ms(day_ms(2026, 11, 10))));

        let q = p("before:2026-11-09").query;
        assert_eq!(q.min_id, None);
        assert_eq!(q.max_id, Some(snowflake_from_unix_ms(day_ms(2026, 11, 9))));

        // after: means after that whole day.
        let q = p("after:2026-11-09").query;
        assert_eq!(
            q.min_id,
            Some(snowflake_from_unix_ms(day_ms(2026, 11, 10)) - 1)
        );
        assert_eq!(q.max_id, None);

        // Combined: the tightest bounds win.
        let q = p("after:2026-01-01 before:2026-02-01 during:2026-01").query;
        assert_eq!(
            q.min_id,
            Some(snowflake_from_unix_ms(day_ms(2026, 1, 2)) - 1)
        );
        assert_eq!(q.max_id, Some(snowflake_from_unix_ms(day_ms(2026, 2, 1))));

        // Month / year spans.
        let q = p("during:2025-12").query;
        assert_eq!(q.max_id, Some(snowflake_from_unix_ms(day_ms(2026, 1, 1))));
        let q = p("during:2025").query;
        assert_eq!(
            q.min_id,
            Some(snowflake_from_unix_ms(day_ms(2025, 1, 1)) - 1)
        );

        // Before Discord existed: min bound dropped, not underflowed.
        let q = p("after:2010-01-01").query;
        assert_eq!(q.min_id, None);
    }

    #[test]
    fn dates_use_the_local_day() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let parsed = parse_with_tz("during:2026-11-09", &resolver(), &tz);
        let start = Utc
            .with_ymd_and_hms(2026, 11, 8, 22, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(parsed.query.min_id, Some(snowflake_from_unix_ms(start) - 1));
        let day = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        assert_eq!(day_start_ms(day, &tz), start);
        assert_eq!(snowflake_for_day(day, &tz), snowflake_from_unix_ms(start));
    }

    #[test]
    fn snowflakes() {
        // Example from Discord's docs.
        let id = 175_928_847_299_117_063u64;
        assert_eq!(snowflake_unix_ms(id), 1_462_015_105_796);
        let low = snowflake_from_unix_ms(1_462_015_105_796);
        assert!(low <= id && id - low < (1 << 22));
        assert_eq!(snowflake_unix_ms(low), 1_462_015_105_796);
        assert_eq!(snowflake_from_unix_ms(DISCORD_EPOCH_MS as i64), 0);
        assert_eq!(snowflake_from_unix_ms(0), 0);
        assert_eq!(snowflake_from_unix_ms(DISCORD_EPOCH_MS as i64 + 1), 1 << 22);
        assert!(is_snowflake("175928847299117063"));
        assert!(!is_snowflake("1234"));
        assert!(!is_snowflake("17592884729911706x"));
    }

    #[test]
    fn builds_guild_params() {
        let mut q =
            p("hello world from:leo has:file has:poll in:general pinned:false during:2026-11-09")
                .query;
        q.include_nsfw = true;
        let scope = SearchScope::Guild("9".into());
        let params = q.params(50, &scope);
        let get = |k: &str| -> Vec<&str> {
            params
                .iter()
                .filter(|(pk, _)| *pk == k)
                .map(|(_, v)| v.as_str())
                .collect()
        };
        assert_eq!(get("content"), vec!["hello world"]);
        assert_eq!(get("author_id"), vec!["111111111111111111"]);
        assert_eq!(get("has"), vec!["file", "poll"]);
        assert_eq!(get("channel_id"), vec!["333333333333333333"]);
        assert_eq!(get("pinned"), vec!["false"]);
        assert_eq!(get("min_id").len(), 1);
        assert_eq!(get("max_id").len(), 1);
        assert_eq!(get("sort_by"), vec!["timestamp"]);
        assert_eq!(get("sort_order"), vec!["desc"]);
        assert_eq!(get("offset"), vec!["50"]);
        assert_eq!(get("include_nsfw"), vec!["true"]);
        assert!(get("mentions").is_empty());
    }

    #[test]
    fn dm_params_skip_server_only_fields() {
        let mut q = p("cats in:general").query;
        q.include_nsfw = true;
        q.sort = SearchSort::Relevant;
        let params = q.params(0, &SearchScope::Dm("5".into()));
        assert!(
            params
                .iter()
                .all(|(k, _)| *k != "channel_id" && *k != "include_nsfw")
        );
        assert!(params.iter().all(|(k, _)| *k != "offset"));
        assert!(params.contains(&("sort_by", "relevance".into())));
        let oldest = SearchQuery {
            sort: SearchSort::Oldest,
            content: "x".into(),
            ..Default::default()
        };
        let params = oldest.params(99_999, &SearchScope::Dm("5".into()));
        assert!(params.contains(&("sort_order", "asc".into())));
        assert!(params.contains(&("offset", MAX_OFFSET.to_string())));
    }

    #[test]
    fn builds_urls() {
        let q = p("hello world has:link has:image").query;
        let url = search_url(
            "https://discord.com/api/v10",
            &SearchScope::Guild("42".into()),
            &q,
            25,
        );
        assert_eq!(
            url,
            "https://discord.com/api/v10/guilds/42/messages/search?content=hello+world\
             &has=link&has=image&sort_by=timestamp&sort_order=desc&offset=25"
        );
        let url = search_url(
            "https://discord.com/api/v10",
            &SearchScope::Dm("7".into()),
            &p("a&b").query,
            0,
        );
        assert!(
            url.starts_with("https://discord.com/api/v10/channels/7/messages/search?content=a%26b")
        );
    }

    #[test]
    fn parses_responses() {
        let body = serde_json::json!({
            "total_results": 1234,
            "doing_deep_historical_index": true,
            "messages": [
                [{ "id": "1" }],
                [{ "id": "2a" }, { "id": "2", "hit": true }, { "id": "2b" }],
                { "id": "3" },
                []
            ]
        });
        match SearchResponse::from_json(200, &body) {
            SearchResponse::Page {
                total,
                messages,
                partial,
            } => {
                assert_eq!(total, 1234);
                assert!(partial);
                let ids: Vec<&str> = messages.iter().map(|m| m["id"].as_str().unwrap()).collect();
                assert_eq!(ids, vec!["1", "2", "3"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        let indexing = serde_json::json!({
            "message": "Index not yet available. Try again later",
            "code": 110000, "documents_indexed": 500, "retry_after": 3
        });
        assert_eq!(
            SearchResponse::from_json(202, &indexing),
            SearchResponse::Indexing {
                retry_after: 3.0,
                documents_indexed: 500
            }
        );
        assert_eq!(
            SearchResponse::from_json(200, &serde_json::json!({})),
            SearchResponse::Page {
                total: 0,
                messages: vec![],
                partial: false
            }
        );
    }

    #[test]
    fn highlights_case_insensitively() {
        let terms = highlight_terms(r#"Test "hello," test testing"#);
        assert_eq!(terms, vec!["testing", "hello", "test"]);
        let text = "A TEST of testing, Héllo HELLO";
        let r = match_ranges(text, &terms);
        let hits: Vec<&str> = r.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(hits, vec!["TEST", "testing", "HELLO"]);
        // Non-ASCII terms and adjacent matches merge.
        let terms = highlight_terms("héllo");
        assert_eq!(match_ranges("HÉLLOhéllo!", &terms), vec![0..12]);
        assert!(match_ranges("anything", &[]).is_empty());
    }

    fn no_users(_: &str, _: usize) -> Vec<Suggestion> {
        vec![]
    }

    fn some_users(prefix: &str, n: usize) -> Vec<Suggestion> {
        ["leo", "lena", "bob"]
            .into_iter()
            .filter(|u| u.starts_with(prefix))
            .take(n)
            .map(|u| Suggestion {
                value: u.into(),
                label: u.to_uppercase(),
                detail: u.into(),
            })
            .collect()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
    }

    #[test]
    fn suggests_filter_names() {
        let s = suggest("", false, &no_users, &no_users, today());
        assert_eq!(s.items.len(), FilterKey::ALL.len());
        assert_eq!(s.default_selected, None);
        let s = suggest("", true, &no_users, &no_users, today());
        assert!(!s.items.contains(&SuggestItem::Key(FilterKey::In)));

        let s = suggest("cats p", false, &no_users, &no_users, today());
        assert_eq!(s.items, vec![SuggestItem::Key(FilterKey::Pinned)]);
        assert_eq!(s.token, 5..6);
        let s = suggest("cats ", false, &no_users, &no_users, today());
        assert_eq!(s.token, 5..5);
        assert!(
            suggest("hello", false, &no_users, &no_users, today())
                .items
                .is_empty()
        );
    }

    #[test]
    fn suggests_values() {
        let s = suggest("from:le", false, &some_users, &no_users, today());
        assert_eq!(s.title, "From user");
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.default_selected, Some(0));

        let s = suggest("x has:", false, &no_users, &no_users, today());
        assert_eq!(s.items.len(), Has::ALL.len());
        let s = suggest("has:s", false, &no_users, &no_users, today());
        assert_eq!(s.items.len(), 2); // sound, sticker
        // Fully typed: nothing to complete.
        assert!(
            suggest("has:link", false, &no_users, &no_users, today())
                .items
                .is_empty()
        );

        let s = suggest("before:2026", false, &no_users, &no_users, today());
        let values: Vec<String> = s
            .items
            .iter()
            .map(|i| match i {
                SuggestItem::Value { sugg, .. } => sugg.value.clone(),
                SuggestItem::Key(_) => unreachable!(),
            })
            .collect();
        assert_eq!(values, vec!["2026-11-09", "2026-11-08"]);
        assert!(
            suggest("in:", true, &no_users, &some_users, today())
                .items
                .is_empty()
        );
    }

    #[test]
    fn applying_suggestions_edits_the_box() {
        let mut input = "cats fr".to_string();
        let s = suggest(&input, false, &some_users, &no_users, today());
        apply_suggestion(&mut input, s.token.clone(), &s.items[0]);
        assert_eq!(input, "cats from:");

        let s = suggest(&input, false, &some_users, &no_users, today());
        apply_suggestion(&mut input, s.token.clone(), &s.items[0]);
        assert_eq!(input, "cats from:leo ");

        let mut input = "x from:\"big".to_string();
        let item = SuggestItem::Value {
            key: FilterKey::From,
            sugg: plain("big bob"),
        };
        let token = current_token(&input);
        apply_suggestion(&mut input, token, &item);
        assert_eq!(input, "x from:\"big bob\" ");

        let mut input = "cats".to_string();
        append_key(&mut input, FilterKey::Has);
        assert_eq!(input, "cats has:");
    }

    #[test]
    fn removing_a_filter_chip() {
        let s = "cats from:leo has:link";
        let parsed = p(s);
        assert_eq!(
            remove_range(s, parsed.filters[0].range.clone()),
            "cats has:link"
        );
        assert_eq!(
            remove_range(s, parsed.filters[1].range.clone()),
            "cats from:leo"
        );
        assert_eq!(remove_range("from:leo", 0..8), "");
    }

    #[test]
    fn open_scopes_to_a_channel_or_the_server() {
        assert_eq!(
            strip_in_filters("cats in:general from:leo in:x"),
            "cats from:leo"
        );
        let mut panel = SearchPanel::new();
        panel.input = "cats in:old".into();
        panel.open(Some(("555555555555555555", "general")));
        assert_eq!(panel.input, "in:general cats");
        assert!(panel.focus_next);
        // The scoped id wins over the name lookup (names can repeat).
        let parsed = panel.parsed(&resolver(), false).clone();
        assert_eq!(parsed.query.channel_ids, vec!["555555555555555555"]);
        assert_eq!(parsed.query.content, "cats");

        panel.open(Some(("666666666666666666", "dev chat")));
        assert_eq!(panel.input, "in:\"dev chat\" cats");
        let parsed = panel.parsed(&resolver(), false).clone();
        assert_eq!(parsed.query.channel_ids, vec!["666666666666666666"]);

        panel.open(None);
        assert_eq!(panel.input, "cats");
        assert!(
            panel
                .parsed(&resolver(), false)
                .query
                .channel_ids
                .is_empty()
        );
        panel.open(None);
        assert_eq!(panel.input, "cats");
    }

    /// Draw every state headlessly to catch layout panics.
    #[test]
    fn panel_draws_every_state() {
        let pal = crate::theme::ThemeChoice::DiscordDark.palette();
        let name = |id: &str| (id == "333333333333333333").then(|| "general".to_string());
        let cx = SearchCtx {
            pal: &pal,
            placeholder: "Search Test Server",
            in_dm: false,
            include_nsfw: false,
            show_images: false,
            resolver: resolver(),
            suggest_users: &some_users,
            suggest_channels: &no_users,
            lookup: &crate::markdown::NoLookup,
            formatting: true,
            emoji_images: false,
            channel_name: &name,
            list_height: Some(300.0),
        };
        let msg = |id: &str, channel: &str| {
            Message::from_json(&serde_json::json!({
                "id": id, "channel_id": channel, "content": "hello **<@1>** world ||secret||",
                "timestamp": "2026-11-09T12:00:00+00:00",
                "author": { "id": "1", "username": "leo" },
                "attachments": [{ "filename": "a.png", "size": 2048 }]
            }))
        };
        let ctx = egui::Context::default();
        let mut panel = SearchPanel::new();
        let frame = |panel: &mut SearchPanel| {
            let mut out = None;
            let mut full = ctx.run_ui(egui::RawInput::default(), |ui| out = panel.show(ui, &cx));
            full.textures_delta.clear();
            out
        };
        frame(&mut panel); // idle: search options
        panel.open(Some(("333333333333333333", "general")));
        frame(&mut panel);
        frame(&mut panel); // focused: suggestions
        let Some(SearchAction::Search { seq, .. }) = panel.begin(p("hello in:general"), false)
        else {
            panic!("expected a search");
        };
        frame(&mut panel); // loading
        panel.on_results(
            seq,
            60,
            vec![msg("1", "333333333333333333"), msg("2", "9")],
            true,
        );
        frame(&mut panel); // results, pager
        let seq = match panel.goto_offset(25) {
            Some(SearchAction::Search { seq, .. }) => seq,
            _ => panic!("expected a search"),
        };
        panel.on_results(seq, 0, vec![], false);
        frame(&mut panel); // no results
        let seq = match panel.resend() {
            Some(SearchAction::Search { seq, .. }) => seq,
            _ => panic!("expected a search"),
        };
        panel.on_indexing(seq, 2.0, 1234);
        frame(&mut panel);
        panel.on_error(panel.seq, "HTTP 403 - Bots cannot use this endpoint".into());
        frame(&mut panel);
        panel.input = "from:nobody".into();
        let _ = panel.begin(p("from:nobody"), false);
        frame(&mut panel); // invalid + red chip
    }

    #[test]
    fn paging_and_numbers() {
        assert_eq!(page_count(0), 0);
        assert_eq!(page_count(25), 1);
        assert_eq!(page_count(26), 2);
        assert_eq!(page_count(1_000_000), MAX_OFFSET / PAGE_SIZE + 1);
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1234), "1,234");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn panel_requests_and_drops_stale_answers() {
        let mut panel = SearchPanel::new();
        assert_eq!(panel.begin(p(""), false), None);
        assert_eq!(panel.begin(p("from:nobody"), false), None);
        assert!(matches!(panel.status, Status::Invalid(_)));

        let Some(SearchAction::Search { seq, query, offset }) = panel.begin(p("hello"), true)
        else {
            panic!("expected a search");
        };
        assert_eq!(offset, 0);
        assert!(query.include_nsfw);
        assert!(panel.is_loading());

        // Page 2 supersedes page 1.
        let Some(SearchAction::Search {
            seq: seq2, offset, ..
        }) = panel.goto_offset(PAGE_SIZE)
        else {
            panic!("expected a search");
        };
        assert_eq!(offset, PAGE_SIZE);
        panel.on_results(seq, 100, vec![], false);
        assert!(panel.is_loading(), "stale answer ignored");
        panel.on_indexing(seq2, 0.1, 0);
        assert!(matches!(panel.status, Status::Indexing { retry_after, .. } if retry_after == 1.0));
        let Some(SearchAction::Search {
            seq: seq3, offset, ..
        }) = panel.resend()
        else {
            panic!("expected a retry");
        };
        assert_eq!(offset, PAGE_SIZE);
        panel.on_results(seq3, 100, vec![], true);
        assert_eq!(panel.status, Status::Done);
        assert_eq!(panel.total, 100);

        // Changing the sort re-runs from the first page.
        let Some(SearchAction::Search { query, offset, .. }) = panel.set_sort(SearchSort::Oldest)
        else {
            panic!("expected a search");
        };
        assert_eq!((query.sort, offset), (SearchSort::Oldest, 0));

        panel.reset();
        assert_eq!(panel.status, Status::Idle);
        assert!(panel.query.is_none());
        assert_eq!(panel.sort, SearchSort::Oldest);
    }
}
