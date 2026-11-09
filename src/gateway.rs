//! Discord gateway (WebSocket) connection: heartbeating, identify / resume,
//! and translating dispatch events into `ToUi` messages.
//!
//! Resuming: READY gives a `session_id` and `resume_gateway_url`. If the
//! connection drops for a reason that allows it, the next connection goes to
//! `resume_gateway_url` and sends op 6 RESUME with the last sequence number;
//! Discord replays the events we missed and sends RESUMED. If it can't
//! resume it sends op 9 INVALID_SESSION (`d` = whether resuming may still
//! work) and we identify from scratch.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message as WsMessage, protocol::WebSocketConfig},
};

use crate::notify::GuildSettings;
use crate::read_state::ReadStateEntry;
use crate::types::{Channel, Guild, Message, MessagePatch, ToUi, User};

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const GATEWAY_QUERY: &str = "?v=10&encoding=json";

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

/// Close codes that mean "don't bother reconnecting".
const FATAL_CLOSE_CODES: &[u16] = &[4004, 4010, 4011, 4012, 4013, 4014];
/// Close codes after which the session can't be resumed (4007 invalid seq,
/// 4009 session timed out): reconnect with a fresh IDENTIFY.
const NO_RESUME_CLOSE_CODES: &[u16] = &[4007, 4009];

pub struct GatewayConfig {
    pub token: String,
    pub is_bot: bool,
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

    // 1. Hello -> heartbeat interval
    let hello = next_json(&mut read)
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

    loop {
        tokio::select! {
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
                    WsMessage::Text(t) => serde_json::from_str(t.as_str())?,
                    WsMessage::Close(Some(close)) => {
                        let code: u16 = close.code.into();
                        if FATAL_CLOSE_CODES.contains(&code) {
                            session.reset();
                            let why = match code {
                                4004 => "Invalid token. Check it and log in again.".to_string(),
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
                            }
                            "RESUMED" => send_ui(ToUi::Resumed),
                            _ => {}
                        }
                        if let Some(ev) = handle_dispatch(event, d, &session.my_id) {
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
        d["intents"] = json!(BOT_INTENTS);
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

async fn next_json<S>(read: &mut S) -> Result<Option<Value>>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    while let Some(frame) = read.next().await {
        if let WsMessage::Text(t) = frame? {
            return Ok(Some(serde_json::from_str(t.as_str())?));
        }
    }
    Ok(None)
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
            "wss://gateway-us-east1-b.discord.gg/?v=10&encoding=json"
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
