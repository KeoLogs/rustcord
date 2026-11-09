//! Discord gateway (WebSocket) connection: heartbeating, identify / resume,
//! and translating dispatch events into `ToUi` messages.
//!
//! Resuming: READY gives a `session_id` and `resume_gateway_url`. If the
//! connection drops for a reason that allows it, the next connection goes to
//! `resume_gateway_url` and sends op 6 RESUME with the last sequence number;
//! Discord replays the events we missed and sends RESUMED. If it can't
//! resume it sends op 9 INVALID_SESSION (`d` = whether resuming may still
//! work) and we identify from scratch.
//!
//! Transport compression: we connect with `compress=zlib-stream`, so Discord
//! sends binary frames that are pieces of one long zlib stream. A message is
//! complete when the bytes end with `00 00 FF FF` (a zlib sync flush); one
//! `Inflater` per connection keeps the stream's dictionary between messages
//! and is thrown away on reconnect / resume. This shrinks a big account's
//! READY (~18 MB of JSON) several times on the wire.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use flate2::{Decompress, FlushDecompress};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message as WsMessage, protocol::WebSocketConfig},
};

use crate::members::{self, ListUpdate, Member, Role};
use crate::notify::GuildSettings;
use crate::presence::{self, Presence};
use crate::read_state::ReadStateEntry;
use crate::types::{Channel, Guild, Message, MessagePatch, ToUi, User};

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";
const GATEWAY_QUERY: &str = "?v=10&encoding=json&compress=zlib-stream";

/// Every complete zlib-stream message ends with this (Z_SYNC_FLUSH).
const ZLIB_SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
/// Largest inflated message we accept (same as the WebSocket limit).
const MAX_INFLATED: usize = 256 << 20;

// Gateway opcodes
const OP_DISPATCH: u64 = 0;
const OP_HEARTBEAT: u64 = 1;
const OP_IDENTIFY: u64 = 2;
const OP_RESUME: u64 = 6;
const OP_RECONNECT: u64 = 7;
const OP_INVALID_SESSION: u64 = 9;
const OP_HELLO: u64 = 10;
const OP_HEARTBEAT_ACK: u64 = 11;

// Intents (only used for bot accounts): GUILDS | GUILD_MESSAGES |
// GUILD_MESSAGE_TYPING | DIRECT_MESSAGES | DIRECT_MESSAGE_TYPING | MESSAGE_CONTENT
const BOT_INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 11) | (1 << 12) | (1 << 14) | (1 << 15);
/// Privileged, opt-in (Settings → Appearance): GUILD_MEMBERS | GUILD_PRESENCES.
const BOT_MEMBER_INTENTS: u64 = (1 << 1) | (1 << 8);

/// Close codes that mean "don't bother reconnecting".
const FATAL_CLOSE_CODES: &[u16] = &[4004, 4010, 4011, 4012, 4013, 4014];
/// Close codes after which the session can't be resumed (4007 invalid seq,
/// 4009 session timed out): reconnect with a fresh IDENTIFY.
const NO_RESUME_CLOSE_CODES: &[u16] = &[4007, 4009];

pub struct GatewayConfig {
    pub token: String,
    pub is_bot: bool,
    /// Bots: also ask for the Server Members and Presence intents.
    pub member_intents: bool,
}

/// What we need to resume a session. Lives across reconnects.
#[derive(Default)]
pub struct Session {
    pub session_id: Option<String>,
    pub resume_url: Option<String>,
    pub seq: Option<u64>,
    /// My user id (from READY), to find my roles in GUILD_CREATE.
    pub my_id: String,
}

impl Session {
    fn can_resume(&self) -> bool {
        self.session_id.is_some() && self.seq.is_some()
    }

    fn reset(&mut self) {
        self.session_id = None;
        self.resume_url = None;
        self.seq = None;
    }

    fn url(&self) -> String {
        match (&self.resume_url, self.can_resume()) {
            (Some(url), true) => format!("{}/{GATEWAY_QUERY}", url.trim_end_matches('/')),
            _ => GATEWAY_URL.to_string(),
        }
    }
}

/// Why a gateway session ended.
pub enum SessionEnd {
    /// Worth reconnecting (resuming if the session allows it).
    Retry,
    /// Bad token / disallowed intents etc. Stop.
    Fatal(String),
}

/// Run one gateway connection until it disconnects.
pub async fn run_session(
    cfg: &GatewayConfig,
    session: &mut Session,
    tx: &Sender<ToUi>,
    commands: &mut UnboundedReceiver<Value>,
    repaint: &(dyn Fn() + Sync),
) -> Result<SessionEnd> {
    let send_ui = |ev: ToUi| {
        let _ = tx.send(ev);
        repaint();
    };

    let resuming = session.can_resume();
    send_ui(ToUi::Status(
        if resuming {
            "Reconnecting…"
        } else {
            "Connecting…"
        }
        .into(),
    ));
    let (ws, _) = connect_async_with_config(session.url(), Some(ws_config()), false)
        .await
        .context("gateway connect failed")?;
    let (mut write, mut read) = ws.split();
    // A fresh zlib stream for every connection (including resumes).
    let mut inflater = Inflater::new();

    // 1. Hello -> heartbeat interval
    let hello = next_json(&mut read, &mut inflater)
        .await?
        .ok_or_else(|| anyhow!("closed before Hello"))?;
    if hello["op"].as_u64() != Some(OP_HELLO) {
        bail!("expected Hello, got {hello}");
    }
    let interval_ms = hello["d"]["heartbeat_interval"].as_u64().unwrap_or(41_250);

    // 2. Resume or identify
    let first = if resuming {
        json!({ "op": OP_RESUME, "d": {
            "token": cfg.token,
            "session_id": session.session_id,
            "seq": session.seq,
        }})
    } else {
        identify(cfg)
    };
    write
        .send(WsMessage::Text(first.to_string().into()))
        .await?;

    // 3. Main loop
    let mut heartbeat = tokio::time::interval(Duration::from_millis(interval_ms));
    heartbeat.tick().await; // first tick fires immediately; skip it
    let mut awaiting_ack = false;
    // Ops from the UI (op 14 / op 8) wait until READY or RESUMED.
    let mut ready = false;

    loop {
        tokio::select! {
            Some(cmd) = commands.recv(), if ready => {
                write.send(WsMessage::Text(cmd.to_string().into())).await?;
            }

            _ = heartbeat.tick() => {
                if awaiting_ack {
                    // Zombie connection: no ACK since last heartbeat. Resume.
                    return Ok(SessionEnd::Retry);
                }
                write.send(WsMessage::Text(json!({ "op": OP_HEARTBEAT, "d": session.seq }).to_string().into())).await?;
                awaiting_ack = true;
            }

            frame = read.next() => {
                let Some(frame) = frame else { return Ok(SessionEnd::Retry) };
                let payload: Value = match frame? {
                    WsMessage::Binary(b) => match inflater.push(&b)? {
                        Some(json) => serde_json::from_slice(&json)?,
                        // Part of a message; the rest comes in the next frame.
                        None => continue,
                    },
                    WsMessage::Text(t) => serde_json::from_str(t.as_str())?,
                    WsMessage::Close(Some(close)) => {
                        let code: u16 = close.code.into();
                        if FATAL_CLOSE_CODES.contains(&code) {
                            session.reset();
                            let why = match code {
                                4004 => "Invalid token. Check it and log in again.".to_string(),
                                4014 if cfg.member_intents => "Discord refused the bot's intents. Turn on the Message Content, Server Members and Presence intents in the Developer Portal (or switch off \"Bots: load the full member list\" in Settings → Appearance).".to_string(),
                                4014 => "Discord refused the bot's intents. Turn on the Message Content intent in the Developer Portal.".to_string(),
                                _ => format!("Discord closed the connection ({code}): {}", close.reason),
                            };
                            return Ok(SessionEnd::Fatal(why));
                        }
                        if NO_RESUME_CLOSE_CODES.contains(&code) {
                            session.reset();
                        }
                        return Ok(SessionEnd::Retry);
                    }
                    WsMessage::Close(None) => return Ok(SessionEnd::Retry),
                    _ => continue,
                };

                if let Some(s) = payload["s"].as_u64() {
                    session.seq = Some(s);
                }

                match payload["op"].as_u64() {
                    Some(OP_DISPATCH) => {
                        let event = payload["t"].as_str().unwrap_or_default();
                        let d = &payload["d"];
                        match event {
                            "READY" => {
                                session.session_id = d["session_id"].as_str().map(str::to_string);
                                session.resume_url =
                                    d["resume_gateway_url"].as_str().map(str::to_string);
                                session.my_id = d["user"]["id"].as_str().unwrap_or_default().to_string();
                                ready = true;
                            }
                            "RESUMED" => {
                                ready = true;
                                send_ui(ToUi::Resumed);
                            }
                            _ => {}
                        }
                        if let Some(ev) = handle_dispatch(event, d, &session.my_id) {
                            send_ui(ev);
                        }
                        for ev in member_events(event, d) {
                            send_ui(ev);
                        }
                    }
                    Some(OP_HEARTBEAT) => {
                        write.send(WsMessage::Text(json!({ "op": OP_HEARTBEAT, "d": session.seq }).to_string().into())).await?;
                    }
                    Some(OP_HEARTBEAT_ACK) => awaiting_ack = false,
                    // Discord asks us to reconnect: resume on the next connection.
                    Some(OP_RECONNECT) => return Ok(SessionEnd::Retry),
                    Some(OP_INVALID_SESSION) => {
                        // d = true: resuming may still work; false: identify again.
                        if !payload["d"].as_bool().unwrap_or(false) {
                            session.reset();
                        }
                        // Discord asks for a random 1-5 s wait before retrying.
                        tokio::time::sleep(Duration::from_millis(1000 + jitter_ms(4000))).await;
                        return Ok(SessionEnd::Retry);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn identify(cfg: &GatewayConfig) -> Value {
    let mut d = json!({
        "token": cfg.token,
        "properties": { "os": std::env::consts::OS, "browser": "RustCord", "device": "RustCord" },
    });
    if cfg.is_bot {
        let extra = if cfg.member_intents {
            BOT_MEMBER_INTENTS
        } else {
            0
        };
        d["intents"] = json!(BOT_INTENTS | extra);
    }
    json!({ "op": OP_IDENTIFY, "d": d })
}

/// Cheap pseudo-random 0..max milliseconds (no rand dependency needed).
fn jitter_ms(max: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    nanos % max.max(1)
}

/// My role ids from a list of member objects (`user.id` or `user_id`).
fn my_roles(members: &Value, my_id: &str) -> Vec<String> {
    members
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|m| m["user_id"].as_str().or(m["user"]["id"].as_str()) == Some(my_id))
        })
        .and_then(|m| m["roles"].as_array())
        .map(|roles| {
            roles
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn handle_dispatch(event: &str, d: &Value, my_id: &str) -> Option<ToUi> {
    match event {
        "READY" => {
            // User accounts get full guilds here; bots get unavailable stubs
            // and receive the real data via GUILD_CREATE afterwards.
            // `merged_members[i]` (user accounts) is my member object in guilds[i].
            let guilds = d["guilds"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .enumerate()
                        .filter(|(_, g)| !g["unavailable"].as_bool().unwrap_or(false))
                        .map(|(i, g)| {
                            let mut guild = Guild::from_json(g);
                            guild.my_roles = my_roles(&d["merged_members"][i], my_id);
                            guild
                        })
                        .collect()
                })
                .unwrap_or_default();
            // DMs and group DMs (user accounts only; bots get them lazily).
            let dms = d["private_channels"]
                .as_array()
                .map(|arr| arr.iter().map(Channel::from_json).collect())
                .unwrap_or_default();
            Some(ToUi::Ready {
                me: User::from_json(&d["user"]),
                guilds,
                dms,
                read_state: ReadStateEntry::parse_ready(&d["read_state"]),
                guild_settings: GuildSettings::parse_ready(&d["user_guild_settings"]),
            })
        }
        "GUILD_CREATE" => {
            let mut guild = Guild::from_json(d);
            guild.my_roles = my_roles(&d["members"], my_id);
            Some(ToUi::GuildAdded(guild))
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => Some(ToUi::ChannelUpserted {
            guild_id: d["guild_id"].as_str().map(str::to_string),
            channel: Channel::from_json(d),
        }),
        "MESSAGE_CREATE" => Some(ToUi::NewMessage(Message::from_json(d))),
        "MESSAGE_UPDATE" => Some(ToUi::MessageUpdated(MessagePatch::from_json(d))),
        "MESSAGE_DELETE" => Some(ToUi::MessageDeleted {
            channel_id: d["channel_id"].as_str().unwrap_or_default().to_string(),
            message_id: d["id"].as_str().unwrap_or_default().to_string(),
        }),
        "MESSAGE_ACK" => ReadStateEntry::from_ack(d).map(ToUi::MessageAck),
        "PASSIVE_UPDATE_V1" => {
            let channels = d["channels"]
                .as_array()?
                .iter()
                .filter_map(|c| {
                    let id = c["id"].as_str()?;
                    let last = c["last_message_id"].as_str()?.parse().ok()?;
                    Some((id.to_string(), last))
                })
                .collect();
            Some(ToUi::LastMessages(channels))
        }
        "USER_GUILD_SETTINGS_UPDATE" => {
            Some(ToUi::GuildSettingsUpdated(GuildSettings::from_json(d)))
        }
        "TYPING_START" => {
            let member = &d["member"];
            let name = member["nick"]
                .as_str()
                .or_else(|| member["user"]["global_name"].as_str())
                .or_else(|| member["user"]["username"].as_str())
                .map(str::to_string);
            Some(ToUi::Typing {
                channel_id: d["channel_id"].as_str().unwrap_or_default().to_string(),
                user_id: d["user_id"].as_str().unwrap_or_default().to_string(),
                name,
            })
        }
        _ => None,
    }
}

/// Members, roles, presence and notes (see `members.rs`, `presence.rs`).
fn member_events(event: &str, d: &Value) -> Vec<ToUi> {
    let guild_id = || d["guild_id"].as_str().unwrap_or_default().to_string();
    let parse_members = |v: &Value| -> Vec<Member> {
        v.as_array()
            .map(|a| a.iter().map(Member::from_json).collect())
            .unwrap_or_default()
    };
    let parse_presences = |v: &Value| -> Vec<(String, Presence)> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let id = p["user"]["id"].as_str().or(p["user_id"].as_str())?;
                Some((id.to_string(), Presence::from_json(p)))
            })
            .collect()
    };
    let mut out = Vec::new();
    match event {
        "READY_SUPPLEMENTAL" => {
            out.push(ToUi::Presences(presence::parse_supplemental(d)));
            // merged_members[i] belongs to guilds[i].
            let guilds = d["guilds"].as_array().into_iter().flatten();
            for (i, g) in guilds.enumerate() {
                let members = parse_members(&d["merged_members"][i]);
                if let (Some(id), false) = (g["id"].as_str(), members.is_empty()) {
                    out.push(ToUi::Members {
                        guild_id: id.to_string(),
                        members,
                        full_list: false,
                    });
                }
            }
        }
        "PRESENCE_UPDATE" => {
            if let Some(id) = d["user"]["id"].as_str() {
                out.push(ToUi::Presences(vec![(
                    id.to_string(),
                    Presence::from_json(d),
                )]));
            }
        }
        "GUILD_MEMBER_LIST_UPDATE" => {
            if let Some(u) = ListUpdate::from_json(d) {
                out.push(ToUi::MemberList(u));
            }
        }
        "GUILD_MEMBERS_CHUNK" | "GUILD_CREATE" => {
            let guild_id = if event == "GUILD_CREATE" {
                d["id"].as_str().unwrap_or_default().to_string()
            } else {
                guild_id()
            };
            let mut members = parse_members(&d["members"]);
            // Presences in a chunk are separate from the members.
            let presences = parse_presences(&d["presences"]);
            for m in &mut members {
                if let Some((_, p)) = presences.iter().find(|(id, _)| *id == m.user.id) {
                    m.presence = Some(p.clone());
                }
            }
            if !presences.is_empty() {
                out.push(ToUi::Presences(presences));
            }
            let full_list = d["nonce"].as_str() == Some(members::NONCE_LIST);
            if !members.is_empty() || full_list {
                out.push(ToUi::Members {
                    guild_id,
                    members,
                    full_list,
                });
            }
        }
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" => out.push(ToUi::Members {
            guild_id: guild_id(),
            members: vec![Member::from_json(d)],
            full_list: false,
        }),
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => out.push(ToUi::RoleUpserted {
            guild_id: guild_id(),
            role: Role::from_json(&d["role"]),
        }),
        "GUILD_ROLE_DELETE" => out.push(ToUi::RoleDeleted {
            guild_id: guild_id(),
            role_id: d["role_id"].as_str().unwrap_or_default().to_string(),
        }),
        "USER_NOTE_UPDATE" => {
            if let Some(id) = d["id"].as_str() {
                out.push(ToUi::Note {
                    user_id: id.to_string(),
                    note: d["note"].as_str().unwrap_or_default().to_string(),
                });
            }
        }
        _ => {}
    }
    out
}

async fn next_json<S>(read: &mut S, inflater: &mut Inflater) -> Result<Option<Value>>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    while let Some(frame) = read.next().await {
        match frame? {
            WsMessage::Text(t) => return Ok(Some(serde_json::from_str(t.as_str())?)),
            WsMessage::Binary(b) => {
                if let Some(json) = inflater.push(&b)? {
                    return Ok(Some(serde_json::from_slice(&json)?));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

/// Inflates Discord's `zlib-stream` transport compression. One per
/// connection: the zlib context carries over from message to message.
pub struct Inflater {
    zlib: Decompress,
    /// Compressed bytes of the message being received.
    pending: Vec<u8>,
}

impl Inflater {
    pub fn new() -> Self {
        Self {
            zlib: Decompress::new(true),
            pending: Vec::new(),
        }
    }

    /// Feed one binary frame. Returns the inflated JSON once a whole message
    /// (ending in `00 00 FF FF`) has arrived, `None` while waiting for more.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        self.pending.extend_from_slice(frame);
        if !self.pending.ends_with(&ZLIB_SUFFIX) {
            if self.pending.len() > MAX_INFLATED {
                bail!("compressed gateway message too large");
            }
            return Ok(None);
        }
        let mut out: Vec<u8> = Vec::with_capacity((self.pending.len() * 4).max(1024));
        let mut input = &self.pending[..];
        loop {
            if out.len() == out.capacity() {
                if out.len() >= MAX_INFLATED {
                    bail!("inflated gateway message too large");
                }
                out.reserve(out.len());
            }
            let (in_before, out_before) = (self.zlib.total_in(), out.len());
            self.zlib
                .decompress_vec(input, &mut out, FlushDecompress::Sync)
                .context("gateway zlib-stream")?;
            let used = (self.zlib.total_in() - in_before) as usize;
            input = &input[used..];
            // Done when all input is used and zlib didn't fill the buffer
            // (a full buffer may mean more output is waiting).
            let stuck = used == 0 && out.len() == out_before;
            if (input.is_empty() && out.len() < out.capacity()) || stuck {
                break;
            }
        }
        // Don't keep a READY-sized buffer around for the whole session.
        self.pending.clear();
        if self.pending.capacity() > 1 << 20 {
            self.pending.shrink_to(64 << 10);
        }
        out.shrink_to_fit();
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_url() {
        let mut s = Session::default();
        assert_eq!(s.url(), GATEWAY_URL);
        s.resume_url = Some("wss://gateway-us-east1-b.discord.gg".into());
        // No session id / seq yet: can't resume.
        assert_eq!(s.url(), GATEWAY_URL);
        s.session_id = Some("abc".into());
        s.seq = Some(42);
        assert_eq!(
            s.url(),
            "wss://gateway-us-east1-b.discord.gg/?v=10&encoding=json&compress=zlib-stream"
        );
        s.reset();
        assert!(!s.can_resume());
    }

    #[test]
    fn roles_from_members() {
        let members = json!([
            { "user_id": "9", "roles": ["x"] },
            { "user": { "id": "1" }, "roles": ["a", "b"] },
        ]);
        assert_eq!(my_roles(&members, "1"), vec!["a", "b"]);
        assert!(my_roles(&Value::Null, "1").is_empty());
    }

    #[test]
    fn member_chunk_with_presences() {
        let d = json!({ "guild_id": "g", "nonce": "rc-list",
            "members": [{ "user": { "id": "1", "username": "a" }, "roles": ["r"] }],
            "presences": [{ "user": { "id": "1" }, "status": "idle" }],
        });
        let evs = member_events("GUILD_MEMBERS_CHUNK", &d);
        assert!(matches!(&evs[0], ToUi::Presences(p) if p[0].0 == "1"));
        match &evs[1] {
            ToUi::Members {
                guild_id,
                members,
                full_list,
            } => {
                assert_eq!(guild_id, "g");
                assert!(*full_list);
                assert_eq!(members[0].roles, ["r"]);
                assert!(members[0].presence.is_some());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(member_events("TYPING_START", &d).is_empty());
    }

    #[test]
    fn identify_intents() {
        let mut cfg = GatewayConfig {
            token: "t".into(),
            is_bot: true,
            member_intents: false,
        };
        assert_eq!(identify(&cfg)["d"]["intents"], json!(BOT_INTENTS));
        cfg.member_intents = true;
        assert_eq!(identify(&cfg)["d"]["intents"], json!(BOT_INTENTS | 2 | 256));
        cfg.is_bot = false;
        assert!(identify(&cfg)["d"].get("intents").is_none());
    }

    /// Compress messages the way Discord does: one zlib stream, each message
    /// ending with a sync flush.
    fn zlib_stream(messages: &[&str]) -> Vec<Vec<u8>> {
        use flate2::{Compress, Compression, FlushCompress};
        let mut z = Compress::new(Compression::default(), true);
        messages
            .iter()
            .map(|m| {
                let mut out = Vec::with_capacity(m.len() + 64);
                let mut input = m.as_bytes();
                loop {
                    if out.len() == out.capacity() {
                        out.reserve(1024);
                    }
                    let before = z.total_in();
                    z.compress_vec(input, &mut out, FlushCompress::Sync)
                        .unwrap();
                    input = &input[(z.total_in() - before) as usize..];
                    if input.is_empty() && out.len() < out.capacity() {
                        break;
                    }
                }
                assert!(out.ends_with(&ZLIB_SUFFIX));
                out
            })
            .collect()
    }

    #[test]
    fn zlib_stream_messages() {
        // A big READY-like message (inflates past the first buffer guess),
        // then a small one that relies on the shared dictionary.
        let big = json!({ "op": 0, "t": "READY", "d": {
            "guilds": (0..2000).map(|i| json!({ "id": i.to_string(), "name": "server" })).collect::<Vec<_>>(),
        }})
        .to_string();
        let small = r#"{"op":11}"#;
        let frames = zlib_stream(&[&big, small, &big]);
        assert!(frames[0].len() * 5 < big.len(), "should compress well");

        let mut inf = Inflater::new();
        // Split the first message across three frames like a fragmented send.
        let f = &frames[0];
        let (a, rest) = f.split_at(f.len() / 3);
        let (b, c) = rest.split_at(rest.len() / 2);
        assert!(inf.push(a).unwrap().is_none());
        assert!(inf.push(b).unwrap().is_none());
        let out = inf.push(c).unwrap().expect("complete message");
        assert_eq!(out, big.as_bytes());
        assert_eq!(inf.push(&frames[1]).unwrap().unwrap(), small.as_bytes());
        assert_eq!(inf.push(&frames[2]).unwrap().unwrap(), big.as_bytes());
        let v: Value = serde_json::from_slice(small.as_bytes()).unwrap();
        assert_eq!(v["op"], 11);
    }

    #[test]
    fn zlib_stream_reset_on_reconnect() {
        // A new connection is a new stream: a fresh Inflater must handle its
        // first message, and garbage is an error rather than a hang.
        let frames = zlib_stream(&[r#"{"op":10}"#]);
        let mut inf = Inflater::new();
        assert_eq!(inf.push(&frames[0]).unwrap().unwrap(), br#"{"op":10}"#);
        let mut fresh = Inflater::new();
        assert!(fresh.push(&frames[0]).unwrap().is_some());
        let mut bad = Inflater::new();
        assert!(bad.push(&[1, 2, 3, 0, 0, 0xff, 0xff]).is_err());
    }

    #[test]
    fn passive_update() {
        let d = json!({ "guild_id": "g", "channels": [
            { "id": "c1", "last_message_id": "55" },
            { "id": "c2", "last_message_id": null },
        ]});
        match handle_dispatch("PASSIVE_UPDATE_V1", &d, "1") {
            Some(ToUi::LastMessages(v)) => assert_eq!(v, vec![("c1".to_string(), 55)]),
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// WebSocket limits. The default 16 MB frame / 64 MB message caps are too
/// small for accounts in many servers: their READY payload alone can be 18 MB+.
fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_frame_size(Some(256 << 20))
        .max_message_size(Some(256 << 20))
}
