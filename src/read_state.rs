//! Read state: which channels have unread messages and how many times you
//! were mentioned in each.
//!
//! Protocol notes (from Discord Messenger, `DiscordInstance.cpp`):
//! - User accounts get `read_state` in READY: `{ entries: [{ id,
//!   last_message_id, mention_count, read_state_type? }], version, partial }`
//!   (older payloads send the array directly).
//! - `MESSAGE_ACK` (another session read a channel) uses `channel_id` /
//!   `message_id` instead of `id` / `last_message_id`.
//! - `PASSIVE_UPDATE_V1` updates `last_message_id` for channels in big
//!   servers we don't get MESSAGE_CREATE for.
//! - Bots have no read state: every channel starts read, and anything that
//!   arrives after launch is unread until you open it.
//!
//! Message ids are snowflakes, so "newer" is just a bigger number.

use std::collections::HashMap;

use serde_json::Value;

/// One entry of READY `read_state` or a MESSAGE_ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadStateEntry {
    pub channel_id: String,
    /// Last message the user has read (0 = unknown).
    pub last_acked: u64,
    pub mention_count: u32,
}

fn snowflake(v: &Value, key: &str) -> u64 {
    match v.get(key) {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn count(v: &Value, key: &str) -> u32 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0) as u32
}

impl ReadStateEntry {
    /// An entry from READY `read_state`. Returns None for non-channel read
    /// states (`read_state_type` != 0, e.g. guild events).
    pub fn from_ready(v: &Value) -> Option<Self> {
        if v.get("read_state_type")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            != 0
        {
            return None;
        }
        let channel_id = v.get("id").and_then(Value::as_str)?.to_string();
        Some(Self {
            channel_id,
            last_acked: snowflake(v, "last_message_id"),
            mention_count: count(v, "mention_count"),
        })
    }

    /// A MESSAGE_ACK dispatch.
    pub fn from_ack(v: &Value) -> Option<Self> {
        if v.get("ack_type").and_then(Value::as_u64).unwrap_or(0) != 0 {
            return None;
        }
        let channel_id = v.get("channel_id").and_then(Value::as_str)?.to_string();
        Some(Self {
            channel_id,
            last_acked: snowflake(v, "message_id"),
            mention_count: count(v, "mention_count"),
        })
    }

    /// READY `read_state`: either `{ entries: [...] }` or a bare array.
    pub fn parse_ready(v: &Value) -> Vec<Self> {
        let list = v.get("entries").and_then(Value::as_array).or(v.as_array());
        list.map(|arr| arr.iter().filter_map(Self::from_ready).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ChannelRead {
    /// Newest message we've read.
    acked: u64,
    /// Newest message in the channel.
    last: u64,
    mentions: u32,
}

#[derive(Debug, Default)]
pub struct ReadState {
    channels: HashMap<String, ChannelRead>,
}

impl ReadState {
    pub fn clear(&mut self) {
        self.channels.clear();
    }

    /// Start tracking a channel. A channel we know nothing about counts as
    /// read up to its current last message.
    pub fn track(&mut self, channel_id: &str, last_message: u64) {
        let c = self
            .channels
            .entry(channel_id.to_string())
            .or_insert(ChannelRead {
                acked: last_message,
                last: last_message,
                mentions: 0,
            });
        c.last = c.last.max(last_message);
    }

    /// Apply READY read state (call after `track`ing the channels).
    pub fn load(&mut self, entries: &[ReadStateEntry]) {
        for e in entries {
            let Some(c) = self.channels.get_mut(&e.channel_id) else {
                // Read states for servers you've left stick around; skip them.
                continue;
            };
            // 0 = never acked; Discord Messenger treats these as read too.
            if e.last_acked != 0 {
                c.acked = e.last_acked;
            }
            c.mentions = e.mention_count;
        }
    }

    /// Another session (or we) read the channel up to `message`.
    pub fn ack(&mut self, channel_id: &str, message: u64, mentions: u32) {
        let c = self.channels.entry(channel_id.to_string()).or_default();
        c.acked = message;
        c.last = c.last.max(message);
        c.mentions = mentions;
    }

    /// Mark everything in the channel as read. Returns the message id to send
    /// in the ack request, or None if it was already read.
    pub fn mark_read(&mut self, channel_id: &str) -> Option<u64> {
        let c = self.channels.get_mut(channel_id)?;
        if c.acked >= c.last && c.mentions == 0 {
            return None;
        }
        c.acked = c.last;
        c.mentions = 0;
        (c.last != 0).then_some(c.last)
    }

    /// A new message arrived. `viewing` = the channel is open and the
    /// window focused, so it's read straight away.
    pub fn on_message(
        &mut self,
        channel_id: &str,
        message: u64,
        mentions_me: bool,
        mine: bool,
        viewing: bool,
    ) {
        let c = self
            .channels
            .entry(channel_id.to_string())
            // First message in a channel we weren't tracking (new DM etc.).
            .or_insert(ChannelRead {
                acked: message.saturating_sub(1),
                last: 0,
                mentions: 0,
            });
        if message <= c.last {
            return; // duplicate / replayed
        }
        let was_read = c.acked >= c.last;
        c.last = message;
        if mine || (viewing && was_read) {
            // Sending a message marks the channel read on Discord's side.
            c.acked = message;
            if mine {
                c.mentions = 0;
            }
        } else if mentions_me {
            c.mentions += 1;
        }
    }

    /// Newer last message from PASSIVE_UPDATE_V1 / CHANNEL_UPDATE.
    pub fn set_last(&mut self, channel_id: &str, message: u64) {
        if let Some(c) = self.channels.get_mut(channel_id) {
            c.last = c.last.max(message);
        } else {
            self.track(channel_id, message);
        }
    }

    pub fn is_unread(&self, channel_id: &str) -> bool {
        self.channels
            .get(channel_id)
            .is_some_and(|c| c.last > c.acked)
    }

    pub fn mentions(&self, channel_id: &str) -> u32 {
        self.channels.get(channel_id).map_or(0, |c| c.mentions)
    }

    pub fn last_acked(&self, channel_id: &str) -> u64 {
        self.channels.get(channel_id).map_or(0, |c| c.acked)
    }

    #[cfg(test)]
    pub fn last_message(&self, channel_id: &str) -> u64 {
        self.channels.get(channel_id).map_or(0, |c| c.last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> ReadState {
        let mut rs = ReadState::default();
        rs.track("a", 100);
        rs.track("b", 200);
        rs.track("c", 300);
        rs.load(&ReadStateEntry::parse_ready(&json!({
            "entries": [
                { "id": "a", "last_message_id": "100", "mention_count": 0 },
                { "id": "b", "last_message_id": "150", "mention_count": 2 },
                { "id": "gone", "last_message_id": "1", "mention_count": 9 },
                { "id": "c", "last_message_id": "1", "read_state_type": 1 },
            ],
            "version": 3
        })));
        rs
    }

    #[test]
    fn ready_read_state() {
        let rs = state();
        assert!(!rs.is_unread("a"));
        assert!(rs.is_unread("b"));
        assert_eq!(rs.mentions("b"), 2);
        // Non-channel entry ignored, so c stays read.
        assert!(!rs.is_unread("c"));
        // Unknown channels are never unread.
        assert!(!rs.is_unread("gone"));
    }

    #[test]
    fn bare_array_read_state() {
        let e = ReadStateEntry::parse_ready(&json!([{ "id": "x", "last_message_id": "5" }]));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].last_acked, 5);
    }

    #[test]
    fn new_messages() {
        let mut rs = state();
        rs.on_message("a", 101, false, false, false);
        assert!(rs.is_unread("a"));
        assert_eq!(rs.mentions("a"), 0);
        rs.on_message("a", 102, true, false, false);
        assert_eq!(rs.mentions("a"), 1);
        // Replayed (resume) message doesn't count twice.
        rs.on_message("a", 102, true, false, false);
        assert_eq!(rs.mentions("a"), 1);
        // My own message marks it read.
        rs.on_message("a", 103, false, true, false);
        assert!(!rs.is_unread("a"));
        assert_eq!(rs.mentions("a"), 0);
    }

    #[test]
    fn viewing_keeps_read() {
        let mut rs = state();
        rs.on_message("a", 101, true, false, true);
        assert!(!rs.is_unread("a"));
        assert_eq!(rs.mentions("a"), 0);
        // But viewing a channel that was already unread (marked unread) keeps it unread.
        rs.on_message("b", 201, false, false, true);
        assert!(rs.is_unread("b"));
    }

    #[test]
    fn untracked_channel_becomes_unread() {
        let mut rs = ReadState::default();
        rs.on_message("dm", 500, true, false, false);
        assert!(rs.is_unread("dm"));
        assert_eq!(rs.mentions("dm"), 1);
    }

    #[test]
    fn mark_read_and_ack() {
        let mut rs = state();
        assert_eq!(rs.mark_read("b"), Some(200));
        assert!(!rs.is_unread("b"));
        assert_eq!(rs.mentions("b"), 0);
        assert_eq!(rs.mark_read("b"), None);
        // Mark unread from another session.
        let ack = ReadStateEntry::from_ack(
            &json!({ "channel_id": "b", "message_id": "180", "mention_count": 1, "version": 4 }),
        )
        .unwrap();
        rs.ack(&ack.channel_id, ack.last_acked, ack.mention_count);
        assert!(rs.is_unread("b"));
        assert_eq!(rs.mentions("b"), 1);
    }

    #[test]
    fn passive_update() {
        let mut rs = state();
        rs.set_last("a", 120);
        assert!(rs.is_unread("a"));
        rs.set_last("a", 110); // older value doesn't go backwards
        assert_eq!(rs.last_message("a"), 120);
    }
}
