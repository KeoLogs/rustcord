//! Chat history that feels like Discord's (Brief A, PR3): older pages load
//! with `before=` when you scroll near the top, gaps between loaded batches
//! (after a jump with `around=`) show a "Load more" row, the scroll
//! position holds while a page is put in above you, channels open at the
//! first unread message, and optimistic sends are matched to their echo by
//! `nonce`.
//!
//! Pure logic on the channel's message list (oldest first, sorted by
//! snowflake) and a small `Timeline` per channel; `app.rs` draws it and
//! sends the requests. Nothing here does I/O.

use std::collections::HashSet;

use crate::types::Message;

/// Messages per page (Discord's maximum `limit`).
pub const PAGE_SIZE: usize = 50;
/// Start loading older messages when the view is this close to the top
/// (px).
pub const LOAD_MARGIN: f32 = 400.0;
/// Show "Jump to present" once the view is this many screens above the
/// newest message.
pub const PRESENT_SCREENS: f32 = 2.0;
/// Discord's epoch (2015-01-01) in unix milliseconds, for nonces.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Paging state of one loaded channel.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Timeline {
    /// Loaded messages followed by missing ones (ids, sorted). A gap after
    /// the newest message means the present isn't loaded.
    pub gaps: Vec<u64>,
    /// The oldest loaded message is the channel's first: nothing to load
    /// above it.
    pub at_start: bool,
    /// A page request is in flight (one at a time per channel).
    pub loading: bool,
    /// The last page request failed: no more automatic loads until the
    /// user clicks "Try again".
    pub failed: bool,
}

/// The snowflake of a message (0 if it isn't one).
pub fn id_of(m: &Message) -> u64 {
    m.id.parse().unwrap_or(0)
}

fn bounds(list: &[Message]) -> Option<(u64, u64)> {
    Some((id_of(list.first()?), id_of(list.last()?)))
}

/// Id of the loaded message right after `id` (the gap's far side).
pub fn next_after(list: &[Message], id: u64) -> Option<u64> {
    list.iter().map(id_of).find(|&m| m > id)
}

/// Merge `page` into `list`, keeping it sorted and without duplicates.
/// Returns how many messages were added.
pub fn insert_sorted(list: &mut Vec<Message>, page: Vec<Message>) -> usize {
    let have: HashSet<String> = list.iter().map(|m| m.id.clone()).collect();
    let before = list.len();
    list.extend(page.into_iter().filter(|m| !have.contains(&m.id)));
    let added = list.len() - before;
    if added > 0 {
        // Stable: equal ids (non-snowflakes) keep their order.
        list.sort_by_key(id_of);
    }
    added
}

/// The first history load of a channel (newest page). Merges with what is
/// already there (a jump that arrived first) instead of replacing it.
pub fn apply_latest(list: &mut Vec<Message>, tl: &mut Timeline, mut page: Vec<Message>) {
    page.sort_by_key(id_of);
    let full = page.len() >= PAGE_SIZE;
    let page_min = page.first().map(id_of);
    if list.iter().all(|m| m.local.is_some()) {
        // Nothing real loaded yet: this page is the whole picture.
        tl.at_start = !full;
    } else if let (Some((_, last)), Some(min)) = (bounds(list), page_min) {
        // A trailing gap is closed once the newest page reaches it.
        if min <= last || !full {
            tl.gaps.retain(|&g| g < last);
        } else if !tl.gaps.contains(&last) {
            tl.gaps.push(last);
            tl.gaps.sort_unstable();
        }
    }
    tl.loading = false;
    tl.failed = false;
    insert_sorted(list, page);
}

/// A page from `LoadPage { before }`: older messages at the top, or the
/// messages filling a gap (the gap's far side is `before`).
pub fn apply_page(
    list: &mut Vec<Message>,
    tl: &mut Timeline,
    before: Option<u64>,
    mut page: Vec<Message>,
) {
    let Some(before) = before else {
        apply_latest(list, tl, page);
        return;
    };
    page.sort_by_key(id_of);
    tl.loading = false;
    tl.failed = false;
    let full = page.len() >= PAGE_SIZE;
    let page_min = page.first().map(id_of);
    let gap = tl
        .gaps
        .iter()
        .copied()
        .find(|&g| next_after(list, g) == Some(before));
    match gap {
        // Filling a gap: closed once the page reaches the near side.
        Some(g) => {
            if !full || page_min.is_none_or(|min| min <= g) {
                tl.gaps.retain(|&x| x != g);
            }
        }
        // Paging up from the top.
        None => {
            if !full && bounds(list).is_some_and(|(first, _)| first == before) {
                tl.at_start = true;
            }
        }
    }
    insert_sorted(list, page);
}

/// Messages around a message (a jump, or the first unread). `channel_last`
/// is the channel's newest message id (0 if unknown): below it the present
/// isn't loaded.
pub fn apply_around(
    list: &mut Vec<Message>,
    tl: &mut Timeline,
    mut page: Vec<Message>,
    channel_last: u64,
) {
    page.sort_by_key(id_of);
    let Some((pmin, pmax)) = bounds(&page) else {
        return;
    };
    let add_gap = |g: u64, gaps: &mut Vec<u64>| {
        if !gaps.contains(&g) {
            gaps.push(g);
            gaps.sort_unstable();
        }
    };
    let real: Vec<&Message> = list.iter().filter(|m| m.local.is_none()).collect();
    match (
        real.first().map(|m| id_of(m)),
        real.last().map(|m| id_of(m)),
    ) {
        (Some(first), Some(last)) => {
            if pmax < first {
                // Older than everything loaded: a gap below the page, and
                // the page's top is the new top.
                add_gap(pmax, &mut tl.gaps);
                tl.at_start = false;
            } else if pmin > last {
                // Newer than everything loaded.
                add_gap(last, &mut tl.gaps);
                if channel_last > pmax {
                    add_gap(pmax, &mut tl.gaps);
                }
            } else {
                // Overlaps: gaps the page spans are closed.
                let spanned: Vec<u64> = tl
                    .gaps
                    .iter()
                    .copied()
                    .filter(|&g| g >= pmin && next_after(list, g).is_some_and(|n| n <= pmax))
                    .collect();
                tl.gaps.retain(|g| !spanned.contains(g));
                // A trailing gap the page runs past moves to the page's end.
                if tl.gaps.contains(&last) && pmax > last {
                    tl.gaps.retain(|&g| g != last);
                    if channel_last > pmax {
                        add_gap(pmax, &mut tl.gaps);
                    }
                }
            }
        }
        _ => {
            tl.at_start = false;
            if channel_last > pmax {
                add_gap(pmax, &mut tl.gaps);
            }
        }
    }
    tl.loading = false;
    insert_sorted(list, page);
}

/// Keep at most `cap` messages. `keep_newest`: drop from the top (live
/// messages came in); otherwise drop the newest (paged up), which leaves a
/// gap before the present. Returns the dropped messages (to free images).
pub fn trim(
    list: &mut Vec<Message>,
    tl: &mut Timeline,
    cap: usize,
    keep_newest: bool,
) -> Vec<Message> {
    if list.len() <= cap {
        return Vec::new();
    }
    let extra = list.len() - cap;
    let removed: Vec<Message> = if keep_newest {
        tl.at_start = false;
        list.drain(..extra).collect()
    } else {
        // Never drop my unsent messages; they stay at the bottom.
        let mut removed = Vec::new();
        let mut i = list.len();
        while removed.len() < extra && i > 0 {
            i -= 1;
            if list[i].local.is_none() {
                removed.push(list.remove(i));
            }
        }
        removed
    };
    if let Some((first, _)) = bounds(list) {
        let last_real = list.iter().rev().find(|m| m.local.is_none()).map(id_of);
        tl.gaps
            .retain(|&g| g >= first && last_real.is_none_or(|l| g <= l));
        if !keep_newest
            && let Some(l) = last_real
            && !tl.gaps.contains(&l)
        {
            tl.gaps.push(l);
            tl.gaps.sort_unstable();
        }
    } else {
        tl.gaps.clear();
    }
    removed
}

/// The newest loaded message is followed by a gap (the present isn't
/// loaded): "Jump to present" reloads the channel.
pub fn detached(list: &[Message], tl: &Timeline) -> bool {
    list.iter()
        .rev()
        .find(|m| m.local.is_none())
        .is_some_and(|m| tl.gaps.contains(&id_of(m)))
}

/// Should the list ask for older messages now? `offset` is how far the
/// view is scrolled from the top; `scrollable` that the content is taller
/// than the view (a channel that fits needs no scrolling to trigger it,
/// but the first frame of a channel must not).
pub fn should_load_older(tl: &Timeline, offset: f32, has_messages: bool) -> bool {
    has_messages && !tl.at_start && !tl.loading && !tl.failed && offset < LOAD_MARGIN
}

/// New scroll offset that keeps a row still after content was put in above
/// it: the row moved from `old_y` to `new_y` (content coordinates).
pub fn hold_offset(offset: f32, old_y: f32, new_y: f32) -> f32 {
    (offset + (new_y - old_y)).max(0.0)
}

/// Show the "Jump to present" bar: the present isn't loaded, or the view
/// is well above the newest message.
pub fn show_jump_to_present(distance_from_bottom: f32, view_height: f32, detached: bool) -> bool {
    detached || distance_from_bottom > view_height.max(100.0) * PRESENT_SCREENS
}

/// Where the first unread message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// Index into the list: open there, under the "NEW" line.
    At(usize),
    /// Older than anything loaded: load around the last read message.
    NotLoaded,
    /// Nothing unread (or no read state): open at the bottom.
    None,
}

/// The first message after `last_acked` that someone else wrote.
/// `last_acked == 0` (never acked / unknown) counts as read.
pub fn first_unread(list: &[Message], tl: &Timeline, last_acked: u64, my_id: &str) -> Unread {
    if last_acked == 0 {
        return Unread::None;
    }
    let first_real = list.iter().find(|m| m.local.is_none()).map(id_of);
    let Some(first) = first_real else {
        return Unread::None;
    };
    // Is the last read message (or what follows it) loaded? If the list
    // starts after it with more history above, the first unread is older
    // than what we have.
    if first > last_acked + 1 && !tl.at_start && !list.iter().any(|m| id_of(m) == last_acked) {
        return Unread::NotLoaded;
    }
    match list
        .iter()
        .position(|m| m.local.is_none() && id_of(m) > last_acked && m.author.id != my_id)
    {
        Some(i) => Unread::At(i),
        None => Unread::None,
    }
}

/// A nonce for a message sent now: a snowflake for this millisecond plus a
/// counter, like Discord's own clients (so it sorts like a message id).
pub fn make_nonce(unix_ms: u64, seq: u64) -> String {
    let ms = unix_ms.saturating_sub(DISCORD_EPOCH_MS);
    ((ms << 22) | (seq & 0x3f_ffff)).to_string()
}

/// A confirmed message of mine arrived (MESSAGE_CREATE or the POST
/// answer): replace my optimistic copy with the same nonce. Returns true if
/// it took the copy's place (or was already there); false means it's an
/// ordinary new message for the caller to add.
pub fn reconcile(list: &mut Vec<Message>, msg: &Message) -> bool {
    let local = msg
        .nonce
        .as_deref()
        .and_then(|n| list.iter().position(|m| m.local.is_some() && m.id == n));
    let present = list.iter().any(|m| m.id == msg.id);
    match local {
        Some(i) => {
            list.remove(i);
            if !present {
                insert_sorted(list, vec![msg.clone()]);
            }
            true
        }
        None => present,
    }
}

/// Mark my optimistic message failed (red, Retry / Delete). False if it's
/// gone (already confirmed or deleted).
pub fn mark_failed(list: &mut [Message], nonce: &str, error: &str) -> bool {
    match list
        .iter_mut()
        .find(|m| m.id == nonce)
        .and_then(|m| m.local.as_mut())
    {
        Some(l) => {
            l.error = Some(error.to_string());
            true
        }
        None => false,
    }
}

/// Characters allowed in one message: 4,000 with Nitro (and the old Nitro
/// Classic), else 2,000. `premium_type` is READY's `user.premium_type`.
pub fn char_limit(premium_type: u8) -> usize {
    match premium_type {
        1 | 2 => 4000,
        _ => 2000,
    }
}

/// The counter next to the message box: shown in the last 200 characters
/// before the limit and past it. (text, over the limit).
pub fn char_counter(len: usize, limit: usize) -> Option<(String, bool)> {
    if len + 200 < limit {
        return None;
    }
    Some((format!("{len}/{limit}"), len > limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(id: u64) -> Message {
        msg(id, "9")
    }

    fn msg(id: u64, author: &str) -> Message {
        Message::from_json(&json!({
            "id": id.to_string(), "channel_id": "1", "content": "hi",
            "author": { "id": author, "username": "u" },
        }))
    }

    fn range(a: u64, b: u64) -> Vec<Message> {
        (a..=b).map(m).collect()
    }

    fn ids(list: &[Message]) -> Vec<u64> {
        list.iter().map(id_of).collect()
    }

    fn local(nonce: &str) -> Message {
        let mut l = m(0);
        l.id = nonce.to_string();
        l.local = Some(Box::default());
        l
    }

    #[test]
    fn latest_page_sets_start_when_short() {
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_latest(&mut list, &mut tl, range(1, 10));
        assert!(tl.at_start);
        assert_eq!(ids(&list), (1..=10).collect::<Vec<_>>());
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_latest(&mut list, &mut tl, range(101, 150));
        assert!(!tl.at_start);
    }

    #[test]
    fn older_pages_prepend_and_stop_at_start() {
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_latest(&mut list, &mut tl, range(101, 150));
        tl.loading = true;
        apply_page(&mut list, &mut tl, Some(101), range(51, 100));
        assert!(!tl.loading);
        assert!(!tl.at_start);
        assert_eq!(list.len(), 100);
        assert_eq!(id_of(&list[0]), 51);
        // A short page: that's the beginning of the channel.
        apply_page(&mut list, &mut tl, Some(51), range(40, 50));
        assert!(tl.at_start);
        assert_eq!(id_of(&list[0]), 40);
        assert!(tl.gaps.is_empty());
        // Duplicates are ignored.
        assert_eq!(insert_sorted(&mut list, range(40, 45)), 0);
    }

    #[test]
    fn scroll_position_holds_when_a_page_goes_in_above() {
        // The old top row was 8 px from the content top, the view 120 px
        // down. 50 rows of ~44 px went in above it: the row is now at
        // 2208 px, so the view moves by the same 2200 px.
        assert_eq!(hold_offset(120.0, 8.0, 2208.0), 2320.0);
        // Nothing went in: unchanged.
        assert_eq!(hold_offset(120.0, 8.0, 8.0), 120.0);
        // Never above the top.
        assert_eq!(hold_offset(0.0, 500.0, 100.0), 0.0);
    }

    #[test]
    fn loading_older_is_gated() {
        let mut tl = Timeline::default();
        assert!(should_load_older(&tl, 10.0, true));
        assert!(!should_load_older(&tl, LOAD_MARGIN + 1.0, true));
        assert!(!should_load_older(&tl, 10.0, false));
        tl.loading = true;
        assert!(!should_load_older(&tl, 10.0, true));
        tl.loading = false;
        tl.failed = true;
        assert!(!should_load_older(&tl, 10.0, true));
        tl.failed = false;
        tl.at_start = true;
        assert!(!should_load_older(&tl, 10.0, true));
    }

    #[test]
    fn jump_far_back_leaves_a_gap_that_load_more_fills() {
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_latest(&mut list, &mut tl, range(251, 300));
        // Jump to message 100: 75..=124 comes in, far older than 251.
        apply_around(&mut list, &mut tl, range(75, 124), 300);
        assert_eq!(tl.gaps, vec![124]);
        assert!(!detached(&list, &tl));
        assert_eq!(next_after(&list, 124), Some(251));
        // "Load more": 50 before 251 = 201..=250, still not reaching 124.
        apply_page(&mut list, &mut tl, Some(251), range(201, 250));
        assert_eq!(tl.gaps, vec![124]);
        assert_eq!(next_after(&list, 124), Some(201));
        // 151..=200 doesn't reach 124 either: still a gap.
        apply_page(&mut list, &mut tl, Some(201), range(151, 200));
        assert_eq!(tl.gaps, vec![124]);
        assert_eq!(id_of(&list[0]), 75);
        // The next one reaches it: gap closed.
        apply_page(&mut list, &mut tl, Some(151), range(101, 150));
        assert!(tl.gaps.is_empty());
        assert_eq!(ids(&list), (75..=300).collect::<Vec<_>>());
    }

    #[test]
    fn jump_into_an_unloaded_channel_is_detached() {
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_around(&mut list, &mut tl, range(75, 124), 300);
        assert!(detached(&list, &tl));
        // A live message after it: the gap is now between the batches.
        list.push(m(301));
        assert!(!detached(&list, &tl));
        assert_eq!(next_after(&list, 124), Some(301));
        // Around the present reaches everything: nothing detached.
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_around(&mut list, &mut tl, range(251, 300), 300);
        assert!(!detached(&list, &tl));
    }

    #[test]
    fn latest_page_after_a_jump_closes_or_keeps_the_trailing_gap() {
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_around(&mut list, &mut tl, range(75, 124), 300);
        // The first load arrives after the jump and doesn't reach it.
        apply_latest(&mut list, &mut tl, range(251, 300));
        assert_eq!(tl.gaps, vec![124]);
        assert!(!detached(&list, &tl));
        // One that overlaps closes it.
        let mut list = Vec::new();
        let mut tl = Timeline::default();
        apply_around(&mut list, &mut tl, range(75, 124), 150);
        apply_latest(&mut list, &mut tl, range(101, 150));
        assert!(tl.gaps.is_empty());
    }

    #[test]
    fn trimming_keeps_the_gaps_honest() {
        let mut list = range(1, 10);
        let mut tl = Timeline {
            at_start: true,
            ..Default::default()
        };
        // Live messages: the oldest go.
        let gone = trim(&mut list, &mut tl, 8, true);
        assert_eq!(ids(&gone), vec![1, 2]);
        assert!(!tl.at_start);
        // Paged up past the cap: the newest go and the present is a gap.
        let mut list = range(1, 10);
        list.push(local("99999"));
        let mut tl = Timeline::default();
        let gone = trim(&mut list, &mut tl, 8, false);
        assert_eq!(ids(&gone), vec![10, 9, 8]);
        assert_eq!(tl.gaps, vec![7]);
        // My unsent message stays.
        assert!(list.last().unwrap().local.is_some());
        assert!(detached(&list, &tl));
    }

    #[test]
    fn jump_to_present_bar() {
        assert!(show_jump_to_present(0.0, 600.0, true));
        assert!(!show_jump_to_present(900.0, 600.0, false));
        assert!(show_jump_to_present(1300.0, 600.0, false));
    }

    #[test]
    fn first_unread_position() {
        let mut list = range(11, 20);
        let tl = Timeline::default();
        // Read up to 14: 15 is the first unread.
        assert_eq!(first_unread(&list, &tl, 14, "me"), Unread::At(4));
        // My own messages after the ack don't count.
        list[4] = msg(15, "me");
        assert_eq!(first_unread(&list, &tl, 14, "me"), Unread::At(5));
        // Everything read.
        assert_eq!(first_unread(&list, &tl, 20, "me"), Unread::None);
        // No read state: open at the bottom.
        assert_eq!(first_unread(&list, &tl, 0, "me"), Unread::None);
        // Read up to 5, but only 11.. is loaded and there's more above.
        assert_eq!(first_unread(&list, &tl, 5, "me"), Unread::NotLoaded);
        // …unless 11 is the channel's first message.
        let start = Timeline {
            at_start: true,
            ..Default::default()
        };
        assert_eq!(first_unread(&list, &start, 5, "me"), Unread::At(0));
        // Read up to 10, list starts at 11: 11 is the first unread.
        assert_eq!(first_unread(&list, &tl, 10, "me"), Unread::At(0));
    }

    #[test]
    fn nonces_are_snowflakes_that_count_up() {
        let ms = DISCORD_EPOCH_MS + 1000;
        assert_eq!(make_nonce(ms, 0), (1000u64 << 22).to_string());
        assert_eq!(make_nonce(ms, 1), ((1000u64 << 22) | 1).to_string());
        let a: u64 = make_nonce(ms, 5).parse().unwrap();
        let b: u64 = make_nonce(ms + 1, 0).parse().unwrap();
        assert!(b > a);
    }

    #[test]
    fn echo_with_the_same_nonce_replaces_the_faded_copy() {
        let mut list = range(1, 3);
        list.push(local("5000"));
        let echo = Message::from_json(&json!({
            "id": "4", "channel_id": "1", "content": "hi", "nonce": "5000",
            "author": { "id": "me", "username": "me" },
        }));
        assert_eq!(echo.nonce.as_deref(), Some("5000"));
        assert!(reconcile(&mut list, &echo));
        assert_eq!(ids(&list), vec![1, 2, 3, 4]);
        assert!(list.iter().all(|m| m.local.is_none()));
        // The POST answer comes in after the gateway echo: nothing doubles.
        assert!(reconcile(&mut list, &echo));
        assert_eq!(list.len(), 4);
        // Someone else's message (or no nonce) is a normal new message.
        assert!(!reconcile(&mut list, &m(6)));
        let mut other = m(7);
        other.nonce = Some("12345".into());
        assert!(!reconcile(&mut list, &other));
        // Numeric nonces parse too.
        let n = Message::from_json(&json!({ "id": "8", "nonce": 42 }));
        assert_eq!(n.nonce.as_deref(), Some("42"));
    }

    #[test]
    fn failed_send_is_marked() {
        let mut list = vec![local("5000")];
        assert!(mark_failed(&mut list, "5000", "No connection"));
        assert_eq!(
            list[0].local.as_ref().unwrap().error.as_deref(),
            Some("No connection")
        );
        assert!(!mark_failed(&mut list, "6000", "x"));
    }

    #[test]
    fn character_counter() {
        assert_eq!(char_limit(0), 2000);
        assert_eq!(char_limit(3), 2000);
        assert_eq!(char_limit(2), 4000);
        assert_eq!(char_counter(100, 2000), None);
        assert_eq!(char_counter(1800, 2000), Some(("1800/2000".into(), false)));
        assert_eq!(char_counter(2001, 2000), Some(("2001/2000".into(), true)));
        assert_eq!(char_counter(3000, 4000), None);
    }
}
