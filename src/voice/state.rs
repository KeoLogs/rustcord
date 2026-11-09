//! Who is in which voice channel, and who is speaking. Pure state fed from
//! gateway payloads, so the channel list can show people under voice
//! channels before RustCord can join calls itself.
//!
//! Sources of voice states:
//! - `READY` (user accounts): `guilds[].voice_states` — ASSUMED present for
//!   user accounts on our gateway capabilities; harmless if missing.
//! - `GUILD_CREATE` (bots, and lazily-loaded guilds): `voice_states`, which
//!   lack `guild_id` (the guild is implied).
//! - `PASSIVE_UPDATE_V1` (user accounts, guilds not focused): a full
//!   `voice_states` list for that guild — ASSUMED to replace, not patch.
//! - `VOICE_STATE_UPDATE` dispatch: one user; `channel_id: null` = left.
//!
//! Speaking flags come from voice gateway op 5 and only exist for the call
//! we're in, so they are kept separately and cleared on leave.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// One user's voice state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VoiceState {
    pub user_id: String,
    /// `None` for DM / group DM calls.
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub session_id: String,
    /// Muted / deafened by themselves.
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Muted / deafened by a moderator.
    pub mute: bool,
    pub deaf: bool,
    /// Go Live (screen share) and camera.
    pub self_stream: bool,
    pub self_video: bool,
    /// Stage audience / suppressed.
    pub suppress: bool,
    /// From `member` when the payload has one (dispatches usually do).
    pub display_name: Option<String>,
    pub avatar_hash: Option<String>,
}

fn flag(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}

impl VoiceState {
    /// Parse leniently. `guild_id` fills in for GUILD_CREATE / READY
    /// entries that omit it. `None` when the user isn't in a channel.
    pub fn from_json(v: &Value, guild_id: Option<&str>) -> Option<Self> {
        let user_id = v["user_id"]
            .as_str()
            .or_else(|| v["member"]["user"]["id"].as_str())?
            .to_string();
        let channel_id = v["channel_id"].as_str()?.to_string();
        let user = &v["member"]["user"];
        let display_name = v["member"]["nick"]
            .as_str()
            .or_else(|| user["global_name"].as_str())
            .or_else(|| user["username"].as_str())
            .map(str::to_string);
        Some(Self {
            user_id,
            guild_id: v["guild_id"].as_str().or(guild_id).map(str::to_string),
            channel_id,
            session_id: v["session_id"].as_str().unwrap_or_default().to_string(),
            self_mute: flag(v, "self_mute"),
            self_deaf: flag(v, "self_deaf"),
            mute: flag(v, "mute"),
            deaf: flag(v, "deaf"),
            self_stream: flag(v, "self_stream"),
            self_video: flag(v, "self_video"),
            suppress: flag(v, "suppress"),
            display_name,
            avatar_hash: user["avatar"].as_str().map(str::to_string),
        })
    }

    /// Muted either way (shows the slashed mic).
    pub fn is_muted(&self) -> bool {
        self.self_mute || self.mute
    }

    /// Deafened either way (shows the slashed headphones).
    pub fn is_deafened(&self) -> bool {
        self.self_deaf || self.deaf
    }

    /// Server mute/deafen (shown in red in Discord).
    pub fn is_server_muted(&self) -> bool {
        self.mute || self.deaf
    }
}

/// What a `VOICE_STATE_UPDATE` did, so the UI can react (e.g. play a
/// join sound later, or drop our call when we got moved out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Joined {
        user_id: String,
        channel_id: String,
    },
    Left {
        user_id: String,
        channel_id: String,
    },
    Moved {
        user_id: String,
        from: String,
        to: String,
    },
    /// Same channel, flags changed (mute, deafen, stream…).
    Updated {
        user_id: String,
    },
    Ignored,
}

/// Map key: (guild id or "" for calls, user id). A user is in at most one
/// voice channel per guild (and one call overall for user accounts).
type Key = (String, String);

#[derive(Debug, Default)]
pub struct VoiceStates {
    states: HashMap<Key, VoiceState>,
    /// User ids speaking in the call we're in (voice gateway op 5).
    speaking: HashSet<String>,
    /// SSRC → user id for the call we're in (from op 5), to attribute audio.
    ssrcs: HashMap<u32, String>,
    me: Option<String>,
}

fn key(guild_id: Option<&str>, user_id: &str) -> Key {
    (
        guild_id.unwrap_or_default().to_string(),
        user_id.to_string(),
    )
}

impl VoiceStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Our own user id (from READY), for `my_state`.
    pub fn set_me(&mut self, user_id: &str) {
        self.me = Some(user_id.to_string());
    }

    /// Replace everything known for one guild with a full `voice_states`
    /// list (GUILD_CREATE / READY / PASSIVE_UPDATE_V1).
    pub fn load_guild(&mut self, guild_id: &str, list: &Value) {
        self.remove_guild(guild_id);
        for v in list.as_array().into_iter().flatten() {
            if let Some(s) = VoiceState::from_json(v, Some(guild_id)) {
                self.states.insert(key(Some(guild_id), &s.user_id), s);
            }
        }
    }

    /// Guild deleted / left / unavailable.
    pub fn remove_guild(&mut self, guild_id: &str) {
        self.states.retain(|(g, _), _| g != guild_id);
    }

    /// Apply a `VOICE_STATE_UPDATE` dispatch.
    pub fn apply_update(&mut self, d: &Value) -> Change {
        let Some(user_id) = d["user_id"]
            .as_str()
            .or_else(|| d["member"]["user"]["id"].as_str())
        else {
            return Change::Ignored;
        };
        let guild_id = d["guild_id"].as_str();
        let k = key(guild_id, user_id);
        let old_channel = self.states.get(&k).map(|s| s.channel_id.clone());
        match VoiceState::from_json(d, guild_id) {
            None => {
                // Left (channel_id null).
                self.states.remove(&k);
                self.speaking.remove(user_id);
                match old_channel {
                    Some(channel_id) => Change::Left {
                        user_id: user_id.to_string(),
                        channel_id,
                    },
                    None => Change::Ignored,
                }
            }
            Some(mut s) => {
                // Keep a name we learned earlier if this payload lacks one.
                if let Some(prev) = self.states.get(&k) {
                    if s.display_name.is_none() {
                        s.display_name = prev.display_name.clone();
                    }
                    if s.avatar_hash.is_none() {
                        s.avatar_hash = prev.avatar_hash.clone();
                    }
                }
                let to = s.channel_id.clone();
                self.states.insert(k, s);
                let user_id = user_id.to_string();
                match old_channel {
                    None => Change::Joined {
                        user_id,
                        channel_id: to,
                    },
                    Some(from) if from != to => {
                        self.speaking.remove(&user_id);
                        Change::Moved { user_id, from, to }
                    }
                    Some(_) => Change::Updated { user_id },
                }
            }
        }
    }

    /// People in a voice channel, sorted by name (case-insensitive) then id,
    /// like Discord's list under the channel.
    pub fn in_channel(&self, channel_id: &str) -> Vec<&VoiceState> {
        let mut v: Vec<&VoiceState> = self
            .states
            .values()
            .filter(|s| s.channel_id == channel_id)
            .collect();
        v.sort_by_cached_key(|s| {
            (
                s.display_name.as_deref().unwrap_or("").to_lowercase(),
                s.user_id.clone(),
            )
        });
        v
    }

    pub fn count(&self, channel_id: &str) -> usize {
        self.states
            .values()
            .filter(|s| s.channel_id == channel_id)
            .count()
    }

    /// Our own voice state (the channel we're connected to, as Discord sees
    /// it — also true when we joined from another device).
    pub fn my_state(&self) -> Option<&VoiceState> {
        let me = self.me.as_deref()?;
        self.states.values().find(|s| s.user_id == me)
    }

    pub fn user_state(&self, guild_id: Option<&str>, user_id: &str) -> Option<&VoiceState> {
        self.states.get(&key(guild_id, user_id))
    }

    /// Voice gateway op 5: map the SSRC and set the speaking flag.
    pub fn on_speaking(&mut self, user_id: Option<&str>, ssrc: u32, flags: u32) {
        let user = match user_id {
            Some(u) => {
                self.ssrcs.insert(ssrc, u.to_string());
                u.to_string()
            }
            None => match self.ssrcs.get(&ssrc) {
                Some(u) => u.clone(),
                None => return,
            },
        };
        self.set_speaking(&user, flags != 0);
    }

    /// Speaking from our own voice activity, or from received audio levels.
    pub fn set_speaking(&mut self, user_id: &str, speaking: bool) {
        if speaking {
            self.speaking.insert(user_id.to_string());
        } else {
            self.speaking.remove(user_id);
        }
    }

    pub fn is_speaking(&self, user_id: &str) -> bool {
        self.speaking.contains(user_id)
    }

    /// Who sent audio with this SSRC.
    pub fn user_for_ssrc(&self, ssrc: u32) -> Option<&str> {
        self.ssrcs.get(&ssrc).map(String::as_str)
    }

    /// Voice op 13: someone's client left our call.
    pub fn on_client_disconnect(&mut self, user_id: &str) {
        self.speaking.remove(user_id);
        self.ssrcs.retain(|_, u| u != user_id);
    }

    /// We left the call: forget call-only data.
    pub fn clear_call(&mut self) {
        self.speaking.clear();
        self.ssrcs.clear();
    }

    /// Number of stored states (for Settings → Performance).
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

/// Where our own call is, as far as the UI knows. The backend reports
/// progress; the bar above the profile box shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    /// Op 4 sent, waiting for VOICE_STATE/SERVER_UPDATE.
    Requesting,
    /// Voice websocket / UDP / DAVE handshake in progress.
    Connecting,
    /// Media flowing. `ping_ms` from the voice heartbeat round trip.
    Connected { ping_ms: Option<u32> },
    /// Lost; the backend is resuming or reconnecting.
    Reconnecting,
    /// Gave up (close code reason, "DAVE required", …).
    Failed(String),
}

/// The call we're in (UI side). Created by the user's click on a voice
/// channel, dropped when they press disconnect or the call ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub channel_name: String,
    /// Server name, or empty for DM calls.
    pub guild_name: String,
    pub status: ConnStatus,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// DAVE end-to-end encryption active (protocol version > 0).
    pub e2ee: bool,
}

impl Call {
    pub fn new(
        guild_id: Option<&str>,
        channel_id: &str,
        channel_name: &str,
        guild_name: &str,
    ) -> Self {
        Self {
            guild_id: guild_id.map(str::to_string),
            channel_id: channel_id.to_string(),
            channel_name: channel_name.to_string(),
            guild_name: guild_name.to_string(),
            status: ConnStatus::Requesting,
            self_mute: false,
            self_deaf: false,
            e2ee: false,
        }
    }

    /// Discord's rule: deafening also mutes; undeafening restores the
    /// previous mute only if mute wasn't toggled meanwhile. We keep it
    /// simple and match what users see: deafen ⇒ muted, undeafen ⇒ unmuted
    /// unless they had muted before deafening.
    pub fn toggle_deafen(&mut self, muted_before_deafen: &mut bool) {
        if self.self_deaf {
            self.self_deaf = false;
            self.self_mute = *muted_before_deafen;
        } else {
            *muted_before_deafen = self.self_mute;
            self.self_deaf = true;
            self.self_mute = true;
        }
    }

    /// Unmuting while deafened also undeafens (like Discord).
    pub fn toggle_mute(&mut self) {
        if self.self_mute {
            self.self_mute = false;
            self.self_deaf = false;
        } else {
            self.self_mute = true;
        }
    }

    /// Status line in the bar.
    pub fn status_label(&self) -> String {
        match &self.status {
            ConnStatus::Requesting | ConnStatus::Connecting => "RTC Connecting".into(),
            ConnStatus::Connected { .. } => "Voice Connected".into(),
            ConnStatus::Reconnecting => "Reconnecting".into(),
            ConnStatus::Failed(why) => format!("Disconnected: {why}"),
        }
    }

    /// "General / My Server" under the status.
    pub fn location_label(&self) -> String {
        if self.guild_name.is_empty() {
            self.channel_name.clone()
        } else {
            format!("{} / {}", self.channel_name, self.guild_name)
        }
    }

    /// Signal quality buckets for the bars icon (Discord: green < 100 ms,
    /// yellow < 250 ms, red above). `None` until a ping is known.
    pub fn quality(&self) -> Option<Quality> {
        match self.status {
            ConnStatus::Connected { ping_ms: Some(p) } => Some(match p {
                0..=99 => Quality::Good,
                100..=249 => Quality::Okay,
                _ => Quality::Bad,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Good,
    Okay,
    Bad,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guild_create_states() -> Value {
        // GUILD_CREATE voice states omit guild_id.
        json!([
            {"user_id": "1", "channel_id": "vc1", "session_id": "a", "self_mute": true,
             "self_deaf": false, "mute": false, "deaf": false, "suppress": false,
             "self_video": false},
            {"user_id": "2", "channel_id": "vc1", "session_id": "b", "self_deaf": true,
             "member": {"nick": "bea", "user": {"id": "2", "username": "b", "avatar": "h2"}}},
            {"user_id": "3", "channel_id": "vc2", "session_id": "c", "self_stream": true,
             "member": {"user": {"id": "3", "username": "cam", "global_name": "Cam"}}},
            {"user_id": "4", "channel_id": null}
        ])
    }

    #[test]
    fn load_guild_and_query() {
        let mut vs = VoiceStates::new();
        vs.load_guild("g", &guild_create_states());
        assert_eq!(vs.len(), 3);
        assert_eq!(vs.count("vc1"), 2);
        let vc1 = vs.in_channel("vc1");
        // Unnamed sorts first (""), then "bea".
        assert_eq!(vc1[0].user_id, "1");
        assert_eq!(vc1[1].display_name.as_deref(), Some("bea"));
        assert_eq!(vc1[1].avatar_hash.as_deref(), Some("h2"));
        assert!(vc1[0].is_muted() && !vc1[0].is_deafened());
        assert!(vc1[1].is_deafened());
        let cam = vs.user_state(Some("g"), "3").unwrap();
        assert_eq!(cam.display_name.as_deref(), Some("Cam"));
        assert!(cam.self_stream);
        assert_eq!(cam.guild_id.as_deref(), Some("g"));

        // Reloading replaces.
        vs.load_guild("g", &json!([{"user_id": "9", "channel_id": "vc1"}]));
        assert_eq!(vs.len(), 1);
        vs.remove_guild("g");
        assert!(vs.is_empty());
    }

    #[test]
    fn dispatch_join_move_update_leave() {
        let mut vs = VoiceStates::new();
        let join = json!({"guild_id": "g", "user_id": "5", "channel_id": "vc1",
                          "session_id": "s", "member": {"user": {"id": "5", "username": "eve"}}});
        assert_eq!(
            vs.apply_update(&join),
            Change::Joined {
                user_id: "5".into(),
                channel_id: "vc1".into()
            }
        );
        vs.set_speaking("5", true);
        let mute = json!({"guild_id": "g", "user_id": "5", "channel_id": "vc1",
                          "session_id": "s", "self_mute": true});
        assert_eq!(
            vs.apply_update(&mute),
            Change::Updated {
                user_id: "5".into()
            }
        );
        // Name kept from the earlier payload.
        assert_eq!(vs.in_channel("vc1")[0].display_name.as_deref(), Some("eve"));
        assert!(vs.in_channel("vc1")[0].self_mute);
        let mv = json!({"guild_id": "g", "user_id": "5", "channel_id": "vc2", "session_id": "s"});
        assert_eq!(
            vs.apply_update(&mv),
            Change::Moved {
                user_id: "5".into(),
                from: "vc1".into(),
                to: "vc2".into()
            }
        );
        assert!(!vs.is_speaking("5"));
        let leave = json!({"guild_id": "g", "user_id": "5", "channel_id": null});
        assert_eq!(
            vs.apply_update(&leave),
            Change::Left {
                user_id: "5".into(),
                channel_id: "vc2".into()
            }
        );
        assert_eq!(vs.apply_update(&leave), Change::Ignored);
        assert_eq!(vs.apply_update(&json!({})), Change::Ignored);
    }

    #[test]
    fn same_user_in_two_guilds_and_dm_calls() {
        let mut vs = VoiceStates::new();
        vs.apply_update(&json!({"guild_id": "g1", "user_id": "bot", "channel_id": "a"}));
        vs.apply_update(&json!({"guild_id": "g2", "user_id": "bot", "channel_id": "b"}));
        vs.apply_update(&json!({"user_id": "me", "channel_id": "dm"}));
        assert_eq!(vs.len(), 3);
        assert_eq!(vs.user_state(None, "me").unwrap().channel_id, "dm");
        vs.set_me("me");
        assert_eq!(vs.my_state().unwrap().channel_id, "dm");
    }

    #[test]
    fn speaking_via_ssrc() {
        let mut vs = VoiceStates::new();
        vs.on_speaking(Some("7"), 1234, 1);
        assert!(vs.is_speaking("7"));
        assert_eq!(vs.user_for_ssrc(1234), Some("7"));
        // Later op 5 without user id, by SSRC.
        vs.on_speaking(None, 1234, 0);
        assert!(!vs.is_speaking("7"));
        vs.on_speaking(None, 999, 1); // unknown SSRC: ignored
        assert!(vs.speaking.is_empty());
        vs.on_speaking(Some("7"), 1234, 1);
        vs.on_client_disconnect("7");
        assert!(!vs.is_speaking("7"));
        assert_eq!(vs.user_for_ssrc(1234), None);
        vs.on_speaking(Some("8"), 1, 1);
        vs.clear_call();
        assert!(!vs.is_speaking("8"));
    }

    #[test]
    fn call_mute_deafen_rules() {
        let mut c = Call::new(Some("g"), "vc", "General", "My Server");
        let mut before = false;
        c.toggle_deafen(&mut before);
        assert!(c.self_deaf && c.self_mute);
        c.toggle_deafen(&mut before);
        assert!(!c.self_deaf && !c.self_mute);

        c.toggle_mute();
        c.toggle_deafen(&mut before);
        c.toggle_deafen(&mut before);
        assert!(c.self_mute && !c.self_deaf, "mute kept across deafen");

        c.toggle_deafen(&mut before);
        c.toggle_mute();
        assert!(!c.self_mute && !c.self_deaf, "unmute also undeafens");
    }

    #[test]
    fn call_labels_and_quality() {
        let mut c = Call::new(Some("g"), "vc", "General", "My Server");
        assert_eq!(c.status_label(), "RTC Connecting");
        assert_eq!(c.location_label(), "General / My Server");
        assert_eq!(c.quality(), None);
        c.status = ConnStatus::Connected { ping_ms: Some(42) };
        assert_eq!(c.status_label(), "Voice Connected");
        assert_eq!(c.quality(), Some(Quality::Good));
        c.status = ConnStatus::Connected { ping_ms: Some(180) };
        assert_eq!(c.quality(), Some(Quality::Okay));
        c.status = ConnStatus::Connected { ping_ms: Some(400) };
        assert_eq!(c.quality(), Some(Quality::Bad));
        c.status = ConnStatus::Failed("Call ended".into());
        assert_eq!(c.status_label(), "Disconnected: Call ended");
        let dm = Call::new(None, "d", "Sam", "");
        assert_eq!(dm.location_label(), "Sam");
    }
}
