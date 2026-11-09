//! Discord gateway (WebSocket) connection: heartbeating, identify, and
//! translating dispatch events into `ToUi` messages.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

use crate::types::{Channel, Guild, Message, MessagePatch, ToUi, User};

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

// Gateway opcodes
const OP_DISPATCH: u64 = 0;
const OP_HEARTBEAT: u64 = 1;
const OP_IDENTIFY: u64 = 2;
const OP_RECONNECT: u64 = 7;
const OP_INVALID_SESSION: u64 = 9;
const OP_HELLO: u64 = 10;
const OP_HEARTBEAT_ACK: u64 = 11;

// Intents (only used for bot accounts): GUILDS | GUILD_MESSAGES |
// GUILD_MESSAGE_TYPING | DIRECT_MESSAGES | DIRECT_MESSAGE_TYPING | MESSAGE_CONTENT
const BOT_INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 11) | (1 << 12) | (1 << 14) | (1 << 15);

/// Close codes that mean "don't bother reconnecting".
const FATAL_CLOSE_CODES: &[u16] = &[4004, 4010, 4011, 4012, 4013, 4014];

pub struct GatewayConfig {
    pub token: String,
    pub is_bot: bool,
}

/// Why a gateway session ended.
pub enum SessionEnd {
    /// Worth reconnecting.
    Retry,
    /// Bad token / disallowed intents etc. Stop.
    Fatal(String),
}

/// Run one gateway session until it disconnects.
pub async fn run_session(
    cfg: &GatewayConfig,
    tx: &Sender<ToUi>,
    repaint: &(dyn Fn() + Sync),
) -> Result<SessionEnd> {
    let send_ui = |ev: ToUi| {
        let _ = tx.send(ev);
        repaint();
    };

    send_ui(ToUi::Status("Connecting…".into()));
    let (ws, _) = connect_async(GATEWAY_URL)
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

    // 2. Identify
    let mut d = json!({
        "token": cfg.token,
        "properties": { "os": std::env::consts::OS, "browser": "RustCord", "device": "RustCord" },
    });
    if cfg.is_bot {
        d["intents"] = json!(BOT_INTENTS);
    }
    write
        .send(WsMessage::Text(
            json!({ "op": OP_IDENTIFY, "d": d }).to_string().into(),
        ))
        .await?;

    // 3. Main loop
    let mut seq: Option<u64> = None;
    let mut heartbeat = tokio::time::interval(Duration::from_millis(interval_ms));
    heartbeat.tick().await; // first tick fires immediately; skip it
    let mut awaiting_ack = false;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if awaiting_ack {
                    // Zombie connection: no ACK since last heartbeat.
                    return Ok(SessionEnd::Retry);
                }
                write.send(WsMessage::Text(json!({ "op": OP_HEARTBEAT, "d": seq }).to_string().into())).await?;
                awaiting_ack = true;
            }

            frame = read.next() => {
                let Some(frame) = frame else { return Ok(SessionEnd::Retry) };
                let payload: Value = match frame? {
                    WsMessage::Text(t) => serde_json::from_str(t.as_str())?,
                    WsMessage::Close(Some(close)) => {
                        let code: u16 = close.code.into();
                        if FATAL_CLOSE_CODES.contains(&code) {
                            return Ok(SessionEnd::Fatal(format!("Gateway closed ({code}): {}", close.reason)));
                        }
                        return Ok(SessionEnd::Retry);
                    }
                    WsMessage::Close(None) => return Ok(SessionEnd::Retry),
                    _ => continue,
                };

                if let Some(s) = payload["s"].as_u64() {
                    seq = Some(s);
                }

                match payload["op"].as_u64() {
                    Some(OP_DISPATCH) => {
                        let event = payload["t"].as_str().unwrap_or_default();
                        if let Some(ev) = handle_dispatch(event, &payload["d"]) {
                            send_ui(ev);
                        }
                    }
                    Some(OP_HEARTBEAT) => {
                        write.send(WsMessage::Text(json!({ "op": OP_HEARTBEAT, "d": seq }).to_string().into())).await?;
                    }
                    Some(OP_HEARTBEAT_ACK) => awaiting_ack = false,
                    Some(OP_RECONNECT) | Some(OP_INVALID_SESSION) => return Ok(SessionEnd::Retry),
                    _ => {}
                }
            }
        }
    }
}

fn handle_dispatch(event: &str, d: &Value) -> Option<ToUi> {
    match event {
        "READY" => {
            // User accounts get full guilds here; bots get unavailable stubs
            // and receive the real data via GUILD_CREATE afterwards.
            let guilds = d["guilds"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|g| !g["unavailable"].as_bool().unwrap_or(false))
                        .map(Guild::from_json)
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
            })
        }
        "GUILD_CREATE" => Some(ToUi::GuildAdded(Guild::from_json(d))),
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
