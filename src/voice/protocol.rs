//! Discord voice protocol, pure parts: voice gateway (v8) payloads,
//! heartbeat bookkeeping, close-code handling, UDP IP discovery, RTP
//! headers and the framing of the `*_rtpsize` AEAD transport modes.
//!
//! No I/O and no crypto here: the backend owns the sockets and (behind the
//! `voice` feature) the ciphers, Opus codec and DAVE session. Everything in
//! this file is unit-tested against byte fixtures.
//!
//! Sources (see docs/VOICE.md for the full write-up and what is verified):
//! - Discord docs, "Voice Connections" and "Opcodes and Status Codes"
//!   (opcodes 0–13, 21–31, close codes, IP discovery, RTP layout, v8
//!   `seq` / `seq_ack`).
//! - DAVE protocol whitepaper (daveprotocol.com) for the JSON and binary
//!   layouts of opcodes 21–31.
//! - discord.js `@discordjs/voice` and songbird for the details the docs
//!   leave out (nonce placement, AAD, binary framing). Where those two
//!   agree we treat it as verified; anything else is marked ASSUMED.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Voice gateway version we speak (`?v=8`: server buffering, `seq_ack`).
pub const VOICE_GATEWAY_VERSION: u8 = 8;

/// Opus over RTP as Discord uses it: 48 kHz stereo, 20 ms frames.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const FRAME_MS: u32 = 20;
/// Samples per channel in one 20 ms frame; also the RTP timestamp step.
pub const FRAME_SAMPLES: u32 = SAMPLE_RATE / 1000 * FRAME_MS;
/// RTP payload type Discord uses for Opus.
pub const RTP_PAYLOAD_TYPE_OPUS: u8 = 0x78;
/// RTP version 2, no padding, no extension, no CSRCs.
pub const RTP_VERSION_FLAGS: u8 = 0x80;
/// The Opus silence frame. Send five after speech stops (and the SFU may
/// inject them; DAVE passes them through untouched).
pub const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// Voice gateway opcodes.
pub mod op {
    pub const IDENTIFY: u8 = 0;
    pub const SELECT_PROTOCOL: u8 = 1;
    pub const READY: u8 = 2;
    pub const HEARTBEAT: u8 = 3;
    pub const SESSION_DESCRIPTION: u8 = 4;
    pub const SPEAKING: u8 = 5;
    pub const HEARTBEAT_ACK: u8 = 6;
    pub const RESUME: u8 = 7;
    pub const HELLO: u8 = 8;
    pub const RESUMED: u8 = 9;
    pub const CLIENTS_CONNECT: u8 = 11;
    pub const CLIENT_DISCONNECT: u8 = 13;
    pub const DAVE_PREPARE_TRANSITION: u8 = 21;
    pub const DAVE_EXECUTE_TRANSITION: u8 = 22;
    pub const DAVE_TRANSITION_READY: u8 = 23;
    pub const DAVE_PREPARE_EPOCH: u8 = 24;
    /// Binary.
    pub const DAVE_MLS_EXTERNAL_SENDER: u8 = 25;
    /// Binary, client → server.
    pub const DAVE_MLS_KEY_PACKAGE: u8 = 26;
    /// Binary.
    pub const DAVE_MLS_PROPOSALS: u8 = 27;
    /// Binary, client → server.
    pub const DAVE_MLS_COMMIT_WELCOME: u8 = 28;
    /// Binary.
    pub const DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION: u8 = 29;
    /// Binary.
    pub const DAVE_MLS_WELCOME: u8 = 30;
    pub const DAVE_MLS_INVALID_COMMIT_WELCOME: u8 = 31;
}

/// Speaking flags (op 5 bitmask).
pub mod speaking {
    pub const MICROPHONE: u32 = 1 << 0;
    pub const SOUNDSHARE: u32 = 1 << 1;
    pub const PRIORITY: u32 = 1 << 2;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// Not JSON, or the JSON isn't an object with a numeric `op`.
    BadJson,
    /// A binary frame or packet shorter than its fixed header.
    TooShort,
    /// A field had the wrong value (e.g. IP discovery type).
    Invalid(&'static str),
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::BadJson => write!(f, "malformed voice gateway JSON"),
            ProtoError::TooShort => write!(f, "voice packet too short"),
            ProtoError::Invalid(what) => write!(f, "invalid voice packet: {what}"),
        }
    }
}

impl std::error::Error for ProtoError {}

// ---------------------------------------------------------------------------
// Main gateway: joining / leaving
// ---------------------------------------------------------------------------

/// Main gateway op 4 (Voice State Update). `channel_id: None` leaves.
/// `guild_id: None` is a DM / group DM call.
///
/// User clients also send `self_video` (and Discord's own client a `flags`
/// field); bots send only the first four. ASSUMED: `self_video: false` is
/// harmless for bots (the field is documented as optional on the user
/// gateway and ignored otherwise).
pub fn voice_state_update(
    guild_id: Option<&str>,
    channel_id: Option<&str>,
    self_mute: bool,
    self_deaf: bool,
) -> Value {
    json!({
        "op": 4,
        "d": {
            "guild_id": guild_id,
            "channel_id": channel_id,
            "self_mute": self_mute,
            "self_deaf": self_deaf,
            "self_video": false,
        }
    })
}

/// `VOICE_SERVER_UPDATE` dispatch: where to connect and with which token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceServer {
    pub token: String,
    pub guild_id: Option<String>,
    /// Set instead of `guild_id` for DM calls.
    pub channel_id: Option<String>,
    /// `None` = the voice server went away; wait for another update.
    pub endpoint: Option<String>,
}

impl VoiceServer {
    pub fn from_json(d: &Value) -> Option<Self> {
        Some(Self {
            token: d["token"].as_str()?.to_string(),
            guild_id: d["guild_id"].as_str().map(str::to_string),
            channel_id: d["channel_id"].as_str().map(str::to_string),
            endpoint: d["endpoint"]
                .as_str()
                .filter(|e| !e.is_empty())
                .map(str::to_string),
        })
    }
}

/// `wss://` URL for a voice endpoint from `VOICE_SERVER_UPDATE` (which comes
/// as `host:port`, sometimes with a scheme in older payloads).
pub fn gateway_url(endpoint: &str) -> String {
    let host = endpoint
        .trim()
        .trim_start_matches("wss://")
        .trim_start_matches("ws://")
        .trim_end_matches('/');
    format!("wss://{host}/?v={VOICE_GATEWAY_VERSION}")
}

/// What the voice websocket needs, collected from the two dispatches that
/// follow op 4: our own `VOICE_STATE_UPDATE` (session id) and
/// `VOICE_SERVER_UPDATE` (token + endpoint). They can arrive in either
/// order, and a server update can be repeated when Discord moves the call.
#[derive(Debug, Clone, Default)]
pub struct JoinCollector {
    /// Server (guild) id, or the DM channel id for calls.
    pub server_id: String,
    pub user_id: String,
    session_id: Option<String>,
    server: Option<VoiceServer>,
}

/// Everything needed for voice op 0 Identify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectInfo {
    pub url: String,
    pub server_id: String,
    pub user_id: String,
    pub session_id: String,
    pub token: String,
}

impl JoinCollector {
    pub fn new(server_id: &str, user_id: &str) -> Self {
        Self {
            server_id: server_id.to_string(),
            user_id: user_id.to_string(),
            ..Default::default()
        }
    }

    /// Feed a `VOICE_STATE_UPDATE`; only ours (same user) counts.
    pub fn on_voice_state(&mut self, d: &Value) -> Option<ConnectInfo> {
        if d["user_id"].as_str() != Some(self.user_id.as_str()) {
            return None;
        }
        self.session_id = d["session_id"].as_str().map(str::to_string);
        self.ready()
    }

    /// Feed a `VOICE_SERVER_UPDATE` for our server.
    pub fn on_voice_server(&mut self, server: VoiceServer) -> Option<ConnectInfo> {
        let id = server.guild_id.as_deref().or(server.channel_id.as_deref());
        if id != Some(self.server_id.as_str()) {
            return None;
        }
        self.server = Some(server);
        self.ready()
    }

    fn ready(&self) -> Option<ConnectInfo> {
        let server = self.server.as_ref()?;
        Some(ConnectInfo {
            url: gateway_url(server.endpoint.as_deref()?),
            server_id: self.server_id.clone(),
            user_id: self.user_id.clone(),
            session_id: self.session_id.clone()?,
            token: server.token.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Voice gateway: outgoing
// ---------------------------------------------------------------------------

/// Op 0 Identify. `max_dave_protocol_version: 0` = no DAVE (only works on
/// calls that don't require it, which since March 2026 is none).
pub fn identify(info: &ConnectInfo, max_dave_protocol_version: u16) -> Value {
    json!({
        "op": op::IDENTIFY,
        "d": {
            "server_id": info.server_id,
            "user_id": info.user_id,
            "session_id": info.session_id,
            "token": info.token,
            "max_dave_protocol_version": max_dave_protocol_version,
        }
    })
}

/// Op 7 Resume (v8 carries `seq_ack`).
pub fn resume(info: &ConnectInfo, seq_ack: Option<u64>) -> Value {
    json!({
        "op": op::RESUME,
        "d": {
            "server_id": info.server_id,
            "session_id": info.session_id,
            "token": info.token,
            "seq_ack": seq_ack_value(seq_ack),
        }
    })
}

/// Op 1 Select Protocol with our discovered external address.
pub fn select_protocol(address: &str, port: u16, mode: Mode) -> Value {
    json!({
        "op": op::SELECT_PROTOCOL,
        "d": {
            "protocol": "udp",
            "data": { "address": address, "port": port, "mode": mode.as_str() }
        }
    })
}

/// Op 5 Speaking. Must be sent (non-zero) before the first audio packet or
/// Discord drops the connection with an invalid SSRC error.
pub fn speaking(flags: u32, ssrc: u32) -> Value {
    json!({
        "op": op::SPEAKING,
        "d": { "speaking": flags, "delay": 0, "ssrc": ssrc }
    })
}

/// Op 3 Heartbeat (v8): `{t: nonce, seq_ack}`.
pub fn heartbeat(nonce: u64, seq_ack: Option<u64>) -> Value {
    json!({
        "op": op::HEARTBEAT,
        "d": { "t": nonce, "seq_ack": seq_ack_value(seq_ack) }
    })
}

/// "Omit it or send -1 if none has been received."
fn seq_ack_value(seq_ack: Option<u64>) -> Value {
    seq_ack.map_or(json!(-1), |s| json!(s))
}

/// Op 23 DAVE Transition Ready.
pub fn dave_transition_ready(transition_id: u16) -> Value {
    json!({ "op": op::DAVE_TRANSITION_READY, "d": { "transition_id": transition_id } })
}

/// Op 31 DAVE MLS Invalid Commit Welcome (asks to be re-added).
pub fn dave_invalid_commit_welcome(transition_id: u16) -> Value {
    json!({ "op": op::DAVE_MLS_INVALID_COMMIT_WELCOME, "d": { "transition_id": transition_id } })
}

/// Op 26 (binary): our MLS KeyPackage. Client frames have no sequence
/// number, just the opcode byte.
pub fn dave_key_package(key_package: &[u8]) -> Vec<u8> {
    binary_frame(op::DAVE_MLS_KEY_PACKAGE, &[key_package])
}

/// Op 28 (binary): MLS Commit, followed by the Welcome when the commit adds
/// members.
pub fn dave_commit_welcome(commit: &[u8], welcome: Option<&[u8]>) -> Vec<u8> {
    binary_frame(
        op::DAVE_MLS_COMMIT_WELCOME,
        &[commit, welcome.unwrap_or_default()],
    )
}

fn binary_frame(opcode: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len = 1 + parts.iter().map(|p| p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(len);
    out.push(opcode);
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

// ---------------------------------------------------------------------------
// Voice gateway: incoming
// ---------------------------------------------------------------------------

/// MLS proposals operation (op 27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalsOp {
    Append,
    Revoke,
}

/// A message from the voice gateway, JSON or binary.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    Hello {
        heartbeat_interval_ms: f64,
    },
    /// `heartbeat_interval` in Ready is documented as erroneous; ignored.
    Ready {
        ssrc: u32,
        ip: String,
        port: u16,
        modes: Vec<String>,
    },
    SessionDescription {
        mode: String,
        secret_key: Vec<u8>,
        dave_protocol_version: u16,
    },
    /// Someone's speaking state; maps their SSRC to their user id.
    Speaking {
        user_id: Option<String>,
        ssrc: u32,
        speaking: u32,
    },
    HeartbeatAck {
        nonce: Option<u64>,
    },
    Resumed,
    ClientsConnect {
        user_ids: Vec<String>,
    },
    ClientDisconnect {
        user_id: String,
    },
    DavePrepareTransition {
        transition_id: u16,
        protocol_version: u16,
    },
    DaveExecuteTransition {
        transition_id: u16,
    },
    DavePrepareEpoch {
        epoch: u64,
        protocol_version: u16,
    },
    /// Op 25: TLS-serialised `ExternalSender`, passed as-is to the MLS lib.
    DaveExternalSender(Vec<u8>),
    /// Op 27: the rest is a TLS `opaque<V>` of proposals (append) or
    /// proposal refs (revoke), passed as-is to the MLS lib.
    DaveProposals {
        op: ProposalsOp,
        data: Vec<u8>,
    },
    /// Op 29: MLS Commit to apply at `transition_id`.
    DaveAnnounceCommit {
        transition_id: u16,
        commit: Vec<u8>,
    },
    /// Op 30: MLS Welcome to join at `transition_id`.
    DaveWelcome {
        transition_id: u16,
        welcome: Vec<u8>,
    },
    /// Anything else (ops 12/14/15/18/20 and future ones are sent by
    /// Discord but undocumented). Ignored, but its `seq` still counts.
    Unknown {
        op: u8,
    },
}

/// A parsed message and its v8 sequence number (if the server sent one).
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub seq: Option<u64>,
    pub event: VoiceEvent,
}

fn u16_of(v: &Value) -> u16 {
    v.as_u64().unwrap_or(0).min(u16::MAX as u64) as u16
}

/// Snowflakes arrive as strings, but be lenient about numbers.
fn id_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Parse a text (JSON) frame.
pub fn parse_text(text: &str) -> Result<Incoming, ProtoError> {
    let v: Value = serde_json::from_str(text).map_err(|_| ProtoError::BadJson)?;
    let opcode = v["op"].as_u64().ok_or(ProtoError::BadJson)?;
    let seq = v["seq"].as_u64();
    let d = &v["d"];
    let event = match opcode.min(255) as u8 {
        op::HELLO => VoiceEvent::Hello {
            heartbeat_interval_ms: d["heartbeat_interval"].as_f64().unwrap_or(0.0),
        },
        op::READY => VoiceEvent::Ready {
            ssrc: d["ssrc"].as_u64().unwrap_or(0) as u32,
            ip: d["ip"].as_str().unwrap_or_default().to_string(),
            port: u16_of(&d["port"]),
            modes: d["modes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        },
        op::SESSION_DESCRIPTION => VoiceEvent::SessionDescription {
            mode: d["mode"].as_str().unwrap_or_default().to_string(),
            secret_key: d["secret_key"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_u64)
                        .map(|b| b as u8)
                        .collect()
                })
                .unwrap_or_default(),
            dave_protocol_version: u16_of(&d["dave_protocol_version"]),
        },
        op::SPEAKING => VoiceEvent::Speaking {
            user_id: id_of(&d["user_id"]),
            ssrc: d["ssrc"].as_u64().unwrap_or(0) as u32,
            speaking: d["speaking"].as_u64().unwrap_or(0) as u32,
        },
        // v8: `{t: nonce}`; older versions: the bare nonce.
        op::HEARTBEAT_ACK => VoiceEvent::HeartbeatAck {
            nonce: d["t"].as_u64().or_else(|| d.as_u64()),
        },
        op::RESUMED => VoiceEvent::Resumed,
        op::CLIENTS_CONNECT => VoiceEvent::ClientsConnect {
            user_ids: d["user_ids"]
                .as_array()
                .map(|a| a.iter().filter_map(id_of).collect())
                .unwrap_or_default(),
        },
        op::CLIENT_DISCONNECT => VoiceEvent::ClientDisconnect {
            user_id: id_of(&d["user_id"]).unwrap_or_default(),
        },
        op::DAVE_PREPARE_TRANSITION => VoiceEvent::DavePrepareTransition {
            transition_id: u16_of(&d["transition_id"]),
            protocol_version: u16_of(&d["protocol_version"]),
        },
        op::DAVE_EXECUTE_TRANSITION => VoiceEvent::DaveExecuteTransition {
            transition_id: u16_of(&d["transition_id"]),
        },
        op::DAVE_PREPARE_EPOCH => VoiceEvent::DavePrepareEpoch {
            epoch: d["epoch"].as_u64().unwrap_or(0),
            protocol_version: u16_of(&d["protocol_version"]),
        },
        other => VoiceEvent::Unknown { op: other },
    };
    Ok(Incoming { seq, event })
}

/// Parse a binary frame from the server: `u16 seq (BE) | u8 opcode | payload`.
pub fn parse_binary(frame: &[u8]) -> Result<Incoming, ProtoError> {
    if frame.len() < 3 {
        return Err(ProtoError::TooShort);
    }
    let seq = u16::from_be_bytes([frame[0], frame[1]]) as u64;
    let opcode = frame[2];
    let p = &frame[3..];
    let with_transition = |p: &[u8]| -> Result<(u16, Vec<u8>), ProtoError> {
        if p.len() < 2 {
            return Err(ProtoError::TooShort);
        }
        Ok((u16::from_be_bytes([p[0], p[1]]), p[2..].to_vec()))
    };
    let event = match opcode {
        op::DAVE_MLS_EXTERNAL_SENDER => VoiceEvent::DaveExternalSender(p.to_vec()),
        op::DAVE_MLS_PROPOSALS => {
            let (&kind, rest) = p.split_first().ok_or(ProtoError::TooShort)?;
            let op = match kind {
                0 => ProposalsOp::Append,
                1 => ProposalsOp::Revoke,
                _ => return Err(ProtoError::Invalid("proposals operation")),
            };
            VoiceEvent::DaveProposals {
                op,
                data: rest.to_vec(),
            }
        }
        op::DAVE_MLS_ANNOUNCE_COMMIT_TRANSITION => {
            let (transition_id, commit) = with_transition(p)?;
            VoiceEvent::DaveAnnounceCommit {
                transition_id,
                commit,
            }
        }
        op::DAVE_MLS_WELCOME => {
            let (transition_id, welcome) = with_transition(p)?;
            VoiceEvent::DaveWelcome {
                transition_id,
                welcome,
            }
        }
        other => VoiceEvent::Unknown { op: other },
    };
    Ok(Incoming {
        seq: Some(seq),
        event,
    })
}

// ---------------------------------------------------------------------------
// Heartbeat (v8)
// ---------------------------------------------------------------------------

/// Heartbeat bookkeeping for one voice websocket connection, plus the last
/// `seq` seen (which survives into a Resume). Time is passed in so this is
/// testable without sleeping.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    interval: Duration,
    next_at: Option<Instant>,
    /// Nonce of the heartbeat in flight (not yet acknowledged).
    pending: Option<(u64, Instant)>,
    /// Last `seq` received, JSON or binary.
    seq_ack: Option<u64>,
    /// Round trip of the last acknowledged heartbeat ("ping" in the UI).
    pub rtt: Option<Duration>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

impl Heartbeat {
    pub fn new() -> Self {
        Self {
            interval: Duration::from_millis(13_750),
            next_at: None,
            pending: None,
            seq_ack: None,
            rtt: None,
        }
    }

    /// Op 8 Hello: start beating. The first beat goes out after one
    /// interval (Discord doesn't ask for jitter on the voice gateway).
    pub fn on_hello(&mut self, interval_ms: f64, now: Instant) {
        let ms = if interval_ms.is_finite() && interval_ms > 0.0 {
            interval_ms
        } else {
            13_750.0
        };
        self.interval = Duration::from_secs_f64(ms / 1000.0);
        self.next_at = Some(now + self.interval);
        self.pending = None;
    }

    /// Record a server `seq` (call for every message that carries one).
    pub fn observe_seq(&mut self, seq: Option<u64>) {
        if let Some(s) = seq {
            self.seq_ack = Some(s);
        }
    }

    pub fn seq_ack(&self) -> Option<u64> {
        self.seq_ack
    }

    /// New connection without resuming: forget the sequence.
    pub fn reset_seq(&mut self) {
        self.seq_ack = None;
    }

    /// When the next heartbeat is due (for `tokio::time::sleep_until`).
    pub fn next_at(&self) -> Option<Instant> {
        self.next_at
    }

    /// Is a heartbeat due?
    pub fn due(&self, now: Instant) -> bool {
        self.next_at.is_some_and(|t| now >= t)
    }

    /// The previous heartbeat was never acknowledged and the next one is
    /// due: the connection is a zombie, close and resume.
    pub fn is_zombie(&self, now: Instant) -> bool {
        self.due(now) && self.pending.is_some()
    }

    /// Build the heartbeat to send now. `nonce` should be unique per beat
    /// (a millisecond timestamp works; keep it below 2^53 for Discord's JS).
    pub fn beat(&mut self, nonce: u64, now: Instant) -> Value {
        self.pending = Some((nonce, now));
        self.next_at = Some(now + self.interval);
        heartbeat(nonce, self.seq_ack)
    }

    /// Op 6. Returns the round-trip time if the nonce matches.
    pub fn on_ack(&mut self, nonce: Option<u64>, now: Instant) -> Option<Duration> {
        let (sent_nonce, sent_at) = self.pending?;
        if nonce.is_some_and(|n| n != sent_nonce) {
            return None;
        }
        self.pending = None;
        let rtt = now.saturating_duration_since(sent_at);
        self.rtt = Some(rtt);
        Some(rtt)
    }
}

// ---------------------------------------------------------------------------
// Close codes
// ---------------------------------------------------------------------------

/// What to do after the voice websocket closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Reconnect and send op 7 Resume.
    Resume,
    /// Reconnect from scratch (op 0 Identify); may need a fresh op 4.
    Reconnect,
    /// Give up; the call is over (show the reason).
    Stop,
}

/// Close code → action. Discord documents "should not reconnect" only for
/// 4014 / 4021 / 4022 and "try resuming" for 4015; the rest is ASSUMED from
/// what the codes mean (our own bugs and auth failures: stop).
pub fn close_action(code: u16) -> CloseAction {
    match code {
        // Kicked / moved / main session gone, rate limited, call ended.
        4014 | 4021 | 4022 => CloseAction::Stop,
        // Voice server crashed: "Try resuming."
        4015 => CloseAction::Resume,
        // Session invalid / timed out / server not found: start over.
        4006 | 4009 | 4011 => CloseAction::Reconnect,
        // Protocol errors (our bug), bad token, DAVE required: stop.
        4001..=4005 | 4012 | 4016 | 4017 | 4020 => CloseAction::Stop,
        // Network drops (1006), normal closes, unknown codes.
        _ => CloseAction::Resume,
    }
}

/// Human-readable reason for the UI.
pub fn close_reason(code: u16) -> &'static str {
    match code {
        4001 => "Unknown opcode",
        4002 => "Failed to decode payload",
        4003 => "Not authenticated",
        4004 => "Authentication failed",
        4005 => "Already authenticated",
        4006 => "Session no longer valid",
        4009 => "Session timed out",
        4011 => "Voice server not found",
        4012 => "Unknown protocol",
        4014 => "Disconnected",
        4015 => "Voice server crashed",
        4016 => "Unknown encryption mode",
        4017 => "This call requires end-to-end encryption (DAVE)",
        4020 => "Bad request",
        4021 => "Rate limited",
        4022 => "Call ended",
        _ => "Connection lost",
    }
}

// ---------------------------------------------------------------------------
// UDP IP discovery
// ---------------------------------------------------------------------------

/// IP discovery packet: 2 type + 2 length + 4 SSRC + 64 address + 2 port.
pub const IP_DISCOVERY_LEN: usize = 74;

/// Request (type 1, length 70, our SSRC, rest zero).
pub fn ip_discovery_request(ssrc: u32) -> [u8; IP_DISCOVERY_LEN] {
    let mut p = [0u8; IP_DISCOVERY_LEN];
    p[0..2].copy_from_slice(&1u16.to_be_bytes());
    p[2..4].copy_from_slice(&70u16.to_be_bytes());
    p[4..8].copy_from_slice(&ssrc.to_be_bytes());
    p
}

/// Response (type 2): our external address and port as the server sees us.
pub fn parse_ip_discovery(p: &[u8]) -> Result<(String, u16), ProtoError> {
    if p.len() < IP_DISCOVERY_LEN {
        return Err(ProtoError::TooShort);
    }
    if u16::from_be_bytes([p[0], p[1]]) != 2 {
        return Err(ProtoError::Invalid("not an IP discovery response"));
    }
    let addr = &p[8..72];
    let end = addr.iter().position(|&b| b == 0).unwrap_or(addr.len());
    let ip = std::str::from_utf8(&addr[..end])
        .map_err(|_| ProtoError::Invalid("address is not UTF-8"))?;
    if ip.is_empty() {
        return Err(ProtoError::Invalid("empty address"));
    }
    let port = u16::from_be_bytes([p[72], p[73]]);
    Ok((ip.to_string(), port))
}

// ---------------------------------------------------------------------------
// RTP
// ---------------------------------------------------------------------------

/// The fixed RTP header we send: 12 bytes, no CSRCs, no extension.
pub fn rtp_header(sequence: u16, timestamp: u32, ssrc: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0] = RTP_VERSION_FLAGS;
    h[1] = RTP_PAYLOAD_TYPE_OPUS;
    h[2..4].copy_from_slice(&sequence.to_be_bytes());
    h[4..8].copy_from_slice(&timestamp.to_be_bytes());
    h[8..12].copy_from_slice(&ssrc.to_be_bytes());
    h
}

/// A received RTP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    pub payload_type: u8,
    pub marker: bool,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub csrc_count: u8,
    pub has_extension: bool,
    /// Bytes in the clear for the `*_rtpsize` modes, which are also the
    /// AEAD associated data: 12 + 4·CSRC, plus the 4-byte extension
    /// preamble (profile + length) when the X bit is set. The extension
    /// *body* is encrypted.
    pub clear_len: usize,
}

/// Is this datagram RTCP rather than RTP? (RTCP packet types 200–204 sit
/// where RTP has marker + payload type.)
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 2 && (200..=204).contains(&packet[1])
}

pub fn parse_rtp(packet: &[u8]) -> Result<RtpHeader, ProtoError> {
    if packet.len() < 12 {
        return Err(ProtoError::TooShort);
    }
    if packet[0] >> 6 != 2 {
        return Err(ProtoError::Invalid("RTP version"));
    }
    let csrc_count = packet[0] & 0x0F;
    let has_extension = packet[0] & 0x10 != 0;
    let mut clear_len = 12 + 4 * csrc_count as usize;
    if has_extension {
        clear_len += 4;
    }
    if packet.len() < clear_len {
        return Err(ProtoError::TooShort);
    }
    Ok(RtpHeader {
        payload_type: packet[1] & 0x7F,
        marker: packet[1] & 0x80 != 0,
        sequence: u16::from_be_bytes([packet[2], packet[3]]),
        timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        csrc_count,
        has_extension,
        clear_len,
    })
}

/// Length in bytes of the (encrypted) extension body, from the preamble's
/// length field (in 32-bit words). After decrypting, skip this many bytes
/// to reach the Opus frame. 0 without an extension.
pub fn rtp_extension_body_len(packet: &[u8], header: &RtpHeader) -> usize {
    if !header.has_extension {
        return 0;
    }
    let at = header.clear_len - 2;
    u16::from_be_bytes([packet[at], packet[at + 1]]) as usize * 4
}

/// Sequence/timestamp counters for the packets we send.
#[derive(Debug, Clone, Copy)]
pub struct RtpCounter {
    pub sequence: u16,
    pub timestamp: u32,
}

impl RtpCounter {
    /// Start from random values (RFC 3550 recommends it).
    pub fn new(sequence: u16, timestamp: u32) -> Self {
        Self {
            sequence,
            timestamp,
        }
    }

    /// Header for the next 20 ms frame, then advance.
    pub fn next_header(&mut self, ssrc: u32) -> [u8; 12] {
        let h = rtp_header(self.sequence, self.timestamp, ssrc);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(FRAME_SAMPLES);
        h
    }
}

// ---------------------------------------------------------------------------
// Transport encryption framing (`*_rtpsize` AEAD modes)
// ---------------------------------------------------------------------------

/// Transport encryption modes we support. Both are AEAD with a 32-byte key
/// (Session Description `secret_key`) and a 16-byte tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Preferred when offered.
    Aes256GcmRtpSize,
    /// Always offered ("required").
    XChaCha20Poly1305RtpSize,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Aes256GcmRtpSize => "aead_aes256_gcm_rtpsize",
            Mode::XChaCha20Poly1305RtpSize => "aead_xchacha20_poly1305_rtpsize",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "aead_aes256_gcm_rtpsize" => Some(Mode::Aes256GcmRtpSize),
            "aead_xchacha20_poly1305_rtpsize" => Some(Mode::XChaCha20Poly1305RtpSize),
            _ => None,
        }
    }

    /// Full AEAD nonce length (12 for AES-GCM, 24 for XChaCha20).
    pub fn nonce_len(self) -> usize {
        match self {
            Mode::Aes256GcmRtpSize => 12,
            Mode::XChaCha20Poly1305RtpSize => 24,
        }
    }

    pub const KEY_LEN: usize = 32;
    pub const TAG_LEN: usize = 16;
    /// Bytes of nonce carried at the end of each packet.
    pub const NONCE_SUFFIX_LEN: usize = 4;
}

/// Pick a mode from Ready's `modes`: AES-GCM if offered (Discord's
/// preference, and hardware-accelerated), else XChaCha20.
pub fn choose_mode(offered: &[String]) -> Option<Mode> {
    let has = |m: Mode| offered.iter().any(|o| o == m.as_str());
    [Mode::Aes256GcmRtpSize, Mode::XChaCha20Poly1305RtpSize]
        .into_iter()
        .find(|&m| has(m))
}

/// Where the parts of an encrypted `*_rtpsize` packet are:
/// `[clear header = AAD][ciphertext][16-byte tag][4-byte nonce]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedLayout {
    pub aad: std::ops::Range<usize>,
    pub ciphertext: std::ops::Range<usize>,
    pub tag: std::ops::Range<usize>,
    pub nonce_suffix: std::ops::Range<usize>,
}

/// Split a received packet for decryption.
pub fn sealed_layout(packet: &[u8], header: &RtpHeader) -> Result<SealedLayout, ProtoError> {
    let n = packet.len();
    let suffix = Mode::TAG_LEN + Mode::NONCE_SUFFIX_LEN;
    if n < header.clear_len + suffix {
        return Err(ProtoError::TooShort);
    }
    let ct_end = n - suffix;
    Ok(SealedLayout {
        aad: 0..header.clear_len,
        ciphertext: header.clear_len..ct_end,
        tag: ct_end..ct_end + Mode::TAG_LEN,
        nonce_suffix: n - Mode::NONCE_SUFFIX_LEN..n,
    })
}

/// Expand the 4-byte packet nonce into the AEAD nonce: the 4 bytes first,
/// the rest zero (12 bytes for AES-GCM, 24 for XChaCha20).
/// Callers take `&n[..mode.nonce_len()]`.
pub fn expand_nonce(suffix: [u8; 4]) -> [u8; 24] {
    let mut n = [0u8; 24];
    n[..4].copy_from_slice(&suffix);
    n
}

/// Our outgoing nonce: a big-endian u32 that increments per packet and
/// wraps. Starting value is random per session.
#[derive(Debug, Clone, Copy)]
pub struct NonceCounter(pub u32);

impl NonceCounter {
    /// The 4 bytes to append to the next packet (and to expand into the
    /// AEAD nonce), then advance.
    pub fn next_suffix(&mut self) -> [u8; 4] {
        let s = self.0.to_be_bytes();
        self.0 = self.0.wrapping_add(1);
        s
    }
}

/// Assemble an outgoing packet from its sealed parts.
pub fn seal_packet(header: &[u8], ciphertext: &[u8], tag: &[u8], suffix: [u8; 4]) -> Vec<u8> {
    let mut p = Vec::with_capacity(header.len() + ciphertext.len() + tag.len() + 4);
    p.extend_from_slice(header);
    p.extend_from_slice(ciphertext);
    p.extend_from_slice(tag);
    p.extend_from_slice(&suffix);
    p
}

// ---------------------------------------------------------------------------
// DAVE frame check
// ---------------------------------------------------------------------------

/// DAVE frames end with this marker.
pub const DAVE_MAGIC: [u8; 2] = [0xFA, 0xFA];

/// Cheap "protocol frame check" from the DAVE whitepaper: does this
/// (transport-decrypted) media frame look DAVE-encrypted? Checks the magic
/// marker, that the supplemental size fits, and that the ULEB128 nonce
/// parses. The MLS library does the real validation; this is for stats and
/// for deciding passthrough on silence frames.
pub fn looks_like_dave_frame(frame: &[u8]) -> bool {
    let n = frame.len();
    if n < 3 || frame[n - 2..] != DAVE_MAGIC {
        return false;
    }
    let supplemental = frame[n - 3] as usize;
    // 8-byte tag + ≥1-byte nonce + size byte + 2-byte marker.
    if !(12..=n).contains(&supplemental) {
        return false;
    }
    let start = n - supplemental + 8;
    let end = n - 3;
    read_uleb128(&frame[start..end]).is_some_and(|(_, used)| used >= 1)
}

/// Decode a ULEB128 number; returns (value, bytes used).
pub fn read_uleb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(10) {
        value |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ConnectInfo {
        ConnectInfo {
            url: gateway_url("c-lhr05-1234.discord.media:443"),
            server_id: "41771983423143937".into(),
            user_id: "104694319306248192".into(),
            session_id: "my_session_id".into(),
            token: "my_token".into(),
        }
    }

    #[test]
    fn url_from_endpoint() {
        assert_eq!(
            gateway_url("c-lhr05-1234.discord.media:443"),
            "wss://c-lhr05-1234.discord.media:443/?v=8"
        );
        assert_eq!(
            gateway_url("wss://x.discord.media/"),
            "wss://x.discord.media/?v=8"
        );
    }

    #[test]
    fn op4_join_and_leave() {
        let j = voice_state_update(Some("1"), Some("2"), false, true);
        assert_eq!(j["op"], 4);
        assert_eq!(j["d"]["channel_id"], "2");
        assert_eq!(j["d"]["self_deaf"], true);
        let l = voice_state_update(Some("1"), None, false, false);
        assert!(l["d"]["channel_id"].is_null());
        let dm = voice_state_update(None, Some("9"), false, false);
        assert!(dm["d"]["guild_id"].is_null());
    }

    #[test]
    fn join_collector_either_order() {
        let state = json!({"user_id": "104694319306248192", "session_id": "my_session_id",
                           "channel_id": "5", "guild_id": "41771983423143937"});
        let other = json!({"user_id": "1", "session_id": "nope"});
        let server = VoiceServer::from_json(&json!({
            "token": "my_token", "guild_id": "41771983423143937",
            "endpoint": "c-lhr05-1234.discord.media:443"
        }))
        .unwrap();

        let mut a = JoinCollector::new("41771983423143937", "104694319306248192");
        assert!(a.on_voice_state(&other).is_none());
        assert!(a.on_voice_state(&state).is_none());
        assert_eq!(a.on_voice_server(server.clone()), Some(info()));

        let mut b = JoinCollector::new("41771983423143937", "104694319306248192");
        assert!(b.on_voice_server(server).is_none());
        assert_eq!(b.on_voice_state(&state), Some(info()));
    }

    #[test]
    fn join_collector_waits_for_endpoint() {
        let mut c = JoinCollector::new("g", "me");
        c.on_voice_state(&json!({"user_id": "me", "session_id": "s"}));
        let no_ep =
            VoiceServer::from_json(&json!({"token": "t", "guild_id": "g", "endpoint": null}))
                .unwrap();
        assert!(c.on_voice_server(no_ep).is_none());
        // A server update for another guild is ignored.
        let other =
            VoiceServer::from_json(&json!({"token": "t", "guild_id": "x", "endpoint": "e:443"}))
                .unwrap();
        assert!(c.on_voice_server(other).is_none());
        // DM calls identify by channel id.
        let mut dm = JoinCollector::new("dm1", "me");
        dm.on_voice_state(&json!({"user_id": "me", "session_id": "s"}));
        let srv = VoiceServer::from_json(
            &json!({"token": "t", "channel_id": "dm1", "endpoint": "e:443"}),
        )
        .unwrap();
        assert_eq!(dm.on_voice_server(srv).unwrap().server_id, "dm1");
    }

    #[test]
    fn identify_and_resume_payloads() {
        let id = identify(&info(), 1);
        assert_eq!(
            id,
            json!({"op": 0, "d": {
                "server_id": "41771983423143937", "user_id": "104694319306248192",
                "session_id": "my_session_id", "token": "my_token",
                "max_dave_protocol_version": 1
            }})
        );
        let r = resume(&info(), Some(10));
        assert_eq!(r["op"], 7);
        assert_eq!(r["d"]["seq_ack"], 10);
        assert_eq!(resume(&info(), None)["d"]["seq_ack"], -1);
    }

    #[test]
    fn select_protocol_and_speaking() {
        let s = select_protocol("127.0.0.1", 1337, Mode::Aes256GcmRtpSize);
        assert_eq!(
            s,
            json!({"op": 1, "d": {"protocol": "udp", "data": {
                "address": "127.0.0.1", "port": 1337, "mode": "aead_aes256_gcm_rtpsize"}}})
        );
        let sp = speaking(speaking::MICROPHONE, 1);
        assert_eq!(
            sp,
            json!({"op": 5, "d": {"speaking": 1, "delay": 0, "ssrc": 1}})
        );
    }

    #[test]
    fn parse_hello_ready_session() {
        let h = parse_text(r#"{"op":8,"d":{"v":8,"heartbeat_interval":41250}}"#).unwrap();
        assert_eq!(
            h.event,
            VoiceEvent::Hello {
                heartbeat_interval_ms: 41250.0
            }
        );
        let r = parse_text(
            r#"{"op":2,"d":{"ssrc":1,"ip":"127.0.0.1","port":1234,
               "modes":["aead_aes256_gcm_rtpsize","aead_xchacha20_poly1305_rtpsize","xsalsa20_poly1305"],
               "heartbeat_interval":1}}"#,
        )
        .unwrap();
        let VoiceEvent::Ready {
            ssrc,
            ip,
            port,
            modes,
        } = r.event
        else {
            panic!("not ready")
        };
        assert_eq!((ssrc, ip.as_str(), port), (1, "127.0.0.1", 1234));
        assert_eq!(choose_mode(&modes), Some(Mode::Aes256GcmRtpSize));

        let sd = parse_text(
            r#"{"op":4,"seq":2,"d":{"mode":"aead_xchacha20_poly1305_rtpsize",
               "secret_key":[251,100,11,1,2,3],"dave_protocol_version":1}}"#,
        )
        .unwrap();
        assert_eq!(sd.seq, Some(2));
        assert_eq!(
            sd.event,
            VoiceEvent::SessionDescription {
                mode: "aead_xchacha20_poly1305_rtpsize".into(),
                secret_key: vec![251, 100, 11, 1, 2, 3],
                dave_protocol_version: 1
            }
        );
    }

    #[test]
    fn parse_speaking_clients_and_acks() {
        let s =
            parse_text(r#"{"op":5,"seq":7,"d":{"user_id":"42","ssrc":99,"speaking":1}}"#).unwrap();
        assert_eq!(s.seq, Some(7));
        assert_eq!(
            s.event,
            VoiceEvent::Speaking {
                user_id: Some("42".into()),
                ssrc: 99,
                speaking: 1
            }
        );
        let c = parse_text(r#"{"op":11,"seq":3,"d":{"user_ids":["1","2"]}}"#).unwrap();
        assert_eq!(
            c.event,
            VoiceEvent::ClientsConnect {
                user_ids: vec!["1".into(), "2".into()]
            }
        );
        let d = parse_text(r#"{"op":13,"d":{"user_id":"2"}}"#).unwrap();
        assert_eq!(
            d.event,
            VoiceEvent::ClientDisconnect {
                user_id: "2".into()
            }
        );
        let ack = parse_text(r#"{"op":6,"d":{"t":1501184119561}}"#).unwrap();
        assert_eq!(
            ack.event,
            VoiceEvent::HeartbeatAck {
                nonce: Some(1501184119561)
            }
        );
        let old_ack = parse_text(r#"{"op":6,"d":1501184119561}"#).unwrap();
        assert_eq!(
            old_ack.event,
            VoiceEvent::HeartbeatAck {
                nonce: Some(1501184119561)
            }
        );
        assert_eq!(
            parse_text(r#"{"op":9,"d":null}"#).unwrap().event,
            VoiceEvent::Resumed
        );
        assert_eq!(
            parse_text(r#"{"op":18,"d":{}}"#).unwrap().event,
            VoiceEvent::Unknown { op: 18 }
        );
        assert_eq!(parse_text("nope"), Err(ProtoError::BadJson));
        assert_eq!(parse_text(r#"{"d":1}"#), Err(ProtoError::BadJson));
    }

    #[test]
    fn parse_dave_json_ops() {
        // Fixtures from the DAVE whitepaper.
        let p = parse_text(r#"{"op":21,"d":{"protocol_version":0,"transition_id":10}}"#).unwrap();
        assert_eq!(
            p.event,
            VoiceEvent::DavePrepareTransition {
                transition_id: 10,
                protocol_version: 0
            }
        );
        let e = parse_text(r#"{"op":22,"d":{"transition_id":10}}"#).unwrap();
        assert_eq!(
            e.event,
            VoiceEvent::DaveExecuteTransition { transition_id: 10 }
        );
        let ep = parse_text(r#"{"op":24,"d":{"protocol_version":1,"epoch":1}}"#).unwrap();
        assert_eq!(
            ep.event,
            VoiceEvent::DavePrepareEpoch {
                epoch: 1,
                protocol_version: 1
            }
        );
        assert_eq!(
            dave_transition_ready(10),
            json!({"op":23,"d":{"transition_id":10}})
        );
        assert_eq!(
            dave_invalid_commit_welcome(32),
            json!({"op":31,"d":{"transition_id":32}})
        );
    }

    #[test]
    fn parse_dave_binary_ops() {
        let ext = parse_binary(&[0x00, 0x05, 25, 0xAA, 0xBB]).unwrap();
        assert_eq!(ext.seq, Some(5));
        assert_eq!(ext.event, VoiceEvent::DaveExternalSender(vec![0xAA, 0xBB]));

        let prop = parse_binary(&[0x01, 0x00, 27, 1, 0x02, 0xCC, 0xDD]).unwrap();
        assert_eq!(prop.seq, Some(256));
        assert_eq!(
            prop.event,
            VoiceEvent::DaveProposals {
                op: ProposalsOp::Revoke,
                data: vec![0x02, 0xCC, 0xDD]
            }
        );
        assert_eq!(
            parse_binary(&[0, 1, 27, 9]),
            Err(ProtoError::Invalid("proposals operation"))
        );

        let commit = parse_binary(&[0xFF, 0xFF, 29, 0x00, 0x0A, 1, 2, 3]).unwrap();
        assert_eq!(commit.seq, Some(65535));
        assert_eq!(
            commit.event,
            VoiceEvent::DaveAnnounceCommit {
                transition_id: 10,
                commit: vec![1, 2, 3]
            }
        );
        let welcome = parse_binary(&[0, 2, 30, 0x01, 0x02, 9]).unwrap();
        assert_eq!(
            welcome.event,
            VoiceEvent::DaveWelcome {
                transition_id: 0x0102,
                welcome: vec![9]
            }
        );
        assert_eq!(parse_binary(&[0, 2, 30, 1]), Err(ProtoError::TooShort));
        assert_eq!(parse_binary(&[0, 2]), Err(ProtoError::TooShort));
        assert_eq!(
            parse_binary(&[0, 3, 99]).unwrap().event,
            VoiceEvent::Unknown { op: 99 }
        );
    }

    #[test]
    fn dave_binary_builders() {
        assert_eq!(dave_key_package(&[1, 2]), vec![26, 1, 2]);
        assert_eq!(dave_commit_welcome(&[1], None), vec![28, 1]);
        assert_eq!(dave_commit_welcome(&[1], Some(&[2, 3])), vec![28, 1, 2, 3]);
    }

    #[test]
    fn heartbeat_cycle() {
        let t0 = Instant::now();
        let mut hb = Heartbeat::new();
        assert!(!hb.due(t0));
        hb.on_hello(1000.0, t0);
        assert!(!hb.due(t0));
        let t1 = t0 + Duration::from_millis(1000);
        assert!(hb.due(t1));
        assert!(!hb.is_zombie(t1));

        // No seq yet → -1.
        assert_eq!(hb.beat(111, t1), json!({"op":3,"d":{"t":111,"seq_ack":-1}}));
        hb.observe_seq(Some(4));
        hb.observe_seq(None);
        assert_eq!(hb.seq_ack(), Some(4));

        // Wrong nonce is ignored; right one gives the RTT.
        assert_eq!(hb.on_ack(Some(5), t1 + Duration::from_millis(40)), None);
        assert_eq!(
            hb.on_ack(Some(111), t1 + Duration::from_millis(42)),
            Some(Duration::from_millis(42))
        );
        assert_eq!(hb.rtt, Some(Duration::from_millis(42)));

        let t2 = t1 + Duration::from_millis(1000);
        assert_eq!(hb.beat(222, t2), json!({"op":3,"d":{"t":222,"seq_ack":4}}));
        // Never acked and the next one is due: zombie.
        let t3 = t2 + Duration::from_millis(1000);
        assert!(hb.is_zombie(t3));
        hb.reset_seq();
        assert_eq!(hb.seq_ack(), None);
    }

    #[test]
    fn heartbeat_bad_interval_falls_back() {
        let t0 = Instant::now();
        let mut hb = Heartbeat::new();
        hb.on_hello(f64::NAN, t0);
        assert_eq!(hb.next_at(), Some(t0 + Duration::from_millis(13_750)));
    }

    #[test]
    fn close_codes() {
        assert_eq!(close_action(4014), CloseAction::Stop);
        assert_eq!(close_action(4021), CloseAction::Stop);
        assert_eq!(close_action(4022), CloseAction::Stop);
        assert_eq!(close_action(4015), CloseAction::Resume);
        assert_eq!(close_action(4006), CloseAction::Reconnect);
        assert_eq!(close_action(4017), CloseAction::Stop);
        assert_eq!(close_action(1006), CloseAction::Resume);
        assert!(close_reason(4017).contains("DAVE"));
    }

    #[test]
    fn ip_discovery_roundtrip() {
        let req = ip_discovery_request(0x0102_0304);
        assert_eq!(&req[..8], &[0, 1, 0, 70, 1, 2, 3, 4]);
        assert!(req[8..].iter().all(|&b| b == 0));

        let mut resp = [0u8; IP_DISCOVERY_LEN];
        resp[0..2].copy_from_slice(&2u16.to_be_bytes());
        resp[2..4].copy_from_slice(&70u16.to_be_bytes());
        resp[4..8].copy_from_slice(&0x0102_0304u32.to_be_bytes());
        resp[8..8 + 11].copy_from_slice(b"203.0.113.7");
        resp[72..74].copy_from_slice(&50004u16.to_be_bytes());
        assert_eq!(
            parse_ip_discovery(&resp),
            Ok(("203.0.113.7".to_string(), 50004))
        );
        assert_eq!(
            parse_ip_discovery(&req),
            Err(ProtoError::Invalid("not an IP discovery response"))
        );
        assert_eq!(parse_ip_discovery(&resp[..20]), Err(ProtoError::TooShort));
    }

    #[test]
    fn rtp_header_roundtrip() {
        let h = rtp_header(0xABCD, 0x1122_3344, 0xDEAD_BEEF);
        assert_eq!(
            h,
            [
                0x80, 0x78, 0xAB, 0xCD, 0x11, 0x22, 0x33, 0x44, 0xDE, 0xAD, 0xBE, 0xEF
            ]
        );
        let p = parse_rtp(&h).unwrap();
        assert_eq!(p.payload_type, 0x78);
        assert_eq!(p.sequence, 0xABCD);
        assert_eq!(p.timestamp, 0x1122_3344);
        assert_eq!(p.ssrc, 0xDEAD_BEEF);
        assert_eq!(p.clear_len, 12);
        assert!(!is_rtcp(&h));
        assert!(is_rtcp(&[0x81, 201, 0, 7]));
        assert_eq!(parse_rtp(&h[..11]), Err(ProtoError::TooShort));
        assert_eq!(
            parse_rtp(&[0x40; 12]),
            Err(ProtoError::Invalid("RTP version"))
        );
    }

    #[test]
    fn rtp_with_csrc_and_extension() {
        // V=2, X=1, CC=1; one CSRC, then extension preamble BEDE / len 2 words.
        let mut p = vec![0x91, 0x78, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
        p.extend_from_slice(&[9, 9, 9, 9]); // CSRC
        p.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x02]); // preamble
        p.extend_from_slice(&[0u8; 8]); // (encrypted) extension body
        p.extend_from_slice(&[0xAA; 5]); // (encrypted) opus
        p.extend_from_slice(&[0xEE; 16]); // tag
        p.extend_from_slice(&[0, 0, 0, 7]); // nonce
        let h = parse_rtp(&p).unwrap();
        assert_eq!(h.csrc_count, 1);
        assert!(h.has_extension);
        assert_eq!(h.clear_len, 20);
        assert_eq!(rtp_extension_body_len(&p, &h), 8);
        let l = sealed_layout(&p, &h).unwrap();
        assert_eq!(l.aad, 0..20);
        assert_eq!(l.ciphertext, 20..33);
        assert_eq!(l.tag, 33..49);
        assert_eq!(l.nonce_suffix, 49..53);
        assert_eq!(&p[l.nonce_suffix.clone()], &[0, 0, 0, 7]);
    }

    #[test]
    fn sealed_layout_too_short() {
        let h = rtp_header(1, 2, 3);
        let p = seal_packet(&h, &[], &[0; 15], [0; 4]);
        let parsed = parse_rtp(&p).unwrap();
        assert_eq!(sealed_layout(&p, &parsed), Err(ProtoError::TooShort));
    }

    #[test]
    fn nonce_counter_and_expand() {
        let mut c = NonceCounter(u32::MAX);
        assert_eq!(c.next_suffix(), [0xFF; 4]);
        assert_eq!(c.next_suffix(), [0, 0, 0, 0]);
        assert_eq!(c.next_suffix(), [0, 0, 0, 1]);
        let n = expand_nonce([1, 2, 3, 4]);
        assert_eq!(&n[..12], &[1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(Mode::Aes256GcmRtpSize.nonce_len(), 12);
        assert_eq!(Mode::XChaCha20Poly1305RtpSize.nonce_len(), 24);
        assert!(n[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn rtp_counter_steps_20ms() {
        let mut c = RtpCounter::new(u16::MAX, u32::MAX - 100);
        let h1 = c.next_header(7);
        let h2 = c.next_header(7);
        let (a, b) = (parse_rtp(&h1).unwrap(), parse_rtp(&h2).unwrap());
        assert_eq!(b.sequence, a.sequence.wrapping_add(1));
        assert_eq!(b.timestamp, a.timestamp.wrapping_add(960));
        assert_eq!(FRAME_SAMPLES, 960);
    }

    #[test]
    fn mode_choice() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            choose_mode(&s(&["aead_xchacha20_poly1305_rtpsize"])),
            Some(Mode::XChaCha20Poly1305RtpSize)
        );
        assert_eq!(choose_mode(&s(&["xsalsa20_poly1305"])), None);
        assert_eq!(
            Mode::parse("aead_aes256_gcm_rtpsize"),
            Some(Mode::Aes256GcmRtpSize)
        );
    }

    #[test]
    fn dave_frame_check() {
        assert_eq!(read_uleb128(&[0xE5, 0x8E, 0x26]), Some((624_485, 3)));
        assert_eq!(read_uleb128(&[0x80]), None);
        // opus(4) | tag(8) | nonce uleb(1) | size(1) | FA FA
        let mut f = vec![1, 2, 3, 4];
        f.extend_from_slice(&[0; 8]);
        f.push(0x05);
        f.push(12);
        f.extend_from_slice(&DAVE_MAGIC);
        assert!(looks_like_dave_frame(&f));
        assert!(!looks_like_dave_frame(&OPUS_SILENCE));
        let n = f.len();
        f[n - 3] = 200; // size larger than frame
        assert!(!looks_like_dave_frame(&f));
    }
}
