//! QR code login through Discord's remote auth gateway (v2).
//!
//! The user scans the code with the Discord app on their phone and approves
//! the login there. RustCord only does the handshake below; it never sends
//! anything that logs in without that approval.
//!
//! Protocol (checked against docs.discord.food, "Remote Authentication"):
//!
//! 1. Connect to [`GATEWAY_URL`] with `Origin: https://discord.com` (other
//!    origins are rejected). The server sends `hello` with
//!    `heartbeat_interval` and `timeout_ms` (session lifespan).
//! 2. Send `init` with `encoded_public_key`: base64 of the DER
//!    SubjectPublicKeyInfo of a fresh RSA-2048 key.
//! 3. The server sends `nonce_proof` with `encrypted_nonce`; decrypt it with
//!    RSA-OAEP (SHA-256, MGF1-SHA-256, no label) and send
//!    `{"op":"nonce_proof","nonce": base64url(nonce)}` (no padding).
//! 4. `pending_remote_init` carries the `fingerprint`: base64url SHA-256 of
//!    our public key (checked here). The QR code is
//!    `https://discord.com/ra/<fingerprint>`.
//! 5. `pending_ticket` (scanned): `encrypted_user_payload` decrypts to
//!    `id:discriminator:avatar:username` for the "Check your phone" step.
//! 6. `pending_login` (approved): exchange `ticket` at
//!    `POST /users/@me/remote-auth/login`, then decrypt `encrypted_token`.
//!    `cancel`: the phone cancelled. Close 4003: the code timed out.
//!
//! `heartbeat` goes out every `heartbeat_interval` ms; a missing
//! `heartbeat_ack` ends the attempt (the panel offers a retry).
//!
//! Runs on its own thread with a small runtime: the login screen has no
//! session backend. It lives exactly as long as the QR panel: dropping the
//! panel's sender ends the thread and closes the socket.
//!
//! The token is a [`Token`] from the moment it is decrypted and is never
//! formatted; the ticket and ciphertexts are never logged either.

use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{
    KeySize, OAEP_SHA256_MGF1SHA256, OaepPrivateDecryptingKey, PrivateDecryptingKey,
};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::accounts::Token;
use crate::types::{ToBackend, ToUi};

pub const GATEWAY_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
/// The remote auth gateway only accepts Discord's own web origins.
pub const ORIGIN: &str = "https://discord.com";
/// Ticket exchange (the task's v9 path; the protocol page gives no version).
pub const LOGIN_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";
/// What the QR code encodes, followed by the fingerprint.
pub const QR_BASE: &str = "https://discord.com/ra/";

/// Longest a 429 on the ticket exchange may make us wait before one retry.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);
/// Fallback session lifespan if `hello` has none (Discord uses ~2 minutes).
const DEFAULT_TIMEOUT_MS: u64 = 150_000;

/// What the QR panel hears from the remote auth task (`ToUi::QrLogin`).
/// `Debug` is safe: `Token`'s is redacted.
#[derive(Debug)]
pub enum QrEvent {
    /// Show this URL as a QR code.
    Code(String),
    /// Scanned on a phone; waiting for the user to approve there.
    Scanned { username: String, avatar: String },
    /// Approved: log in with this token (same path as a pasted one).
    Token(Token),
    /// The phone cancelled.
    Cancelled,
    /// Anything else (timeout, network, captcha): user-safe text.
    Failed(String),
}

// ---------------------------------------------------------------------------
// Keys and crypto
// ---------------------------------------------------------------------------

/// The per-attempt RSA key. No `Debug`: it holds the private key.
pub struct Keys {
    key: OaepPrivateDecryptingKey,
    /// DER SubjectPublicKeyInfo of the public key.
    spki: Vec<u8>,
}

impl Keys {
    /// A fresh RSA-2048 key (e = 65537).
    pub fn generate() -> Result<Self> {
        let key = PrivateDecryptingKey::generate(KeySize::Rsa2048)
            .map_err(|_| anyhow!("couldn't create a login key"))?;
        Self::from_key(key)
    }

    fn from_key(key: PrivateDecryptingKey) -> Result<Self> {
        let spki = key
            .public_key()
            .as_der()
            .map_err(|_| anyhow!("couldn't encode the login key"))?
            .as_ref()
            .to_vec();
        let key = OaepPrivateDecryptingKey::new(key)
            .map_err(|_| anyhow!("couldn't use the login key"))?;
        Ok(Self { key, spki })
    }

    /// `encoded_public_key` for `init`: standard base64 of the SPKI DER.
    pub fn encoded_public_key(&self) -> String {
        STANDARD.encode(&self.spki)
    }

    /// base64url (no padding) SHA-256 of the SPKI DER; must equal the
    /// server's `pending_remote_init` fingerprint.
    pub fn fingerprint(&self) -> String {
        let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &self.spki);
        URL_SAFE_NO_PAD.encode(d.as_ref())
    }

    /// RSA-OAEP (SHA-256) decrypt of a standard-base64 ciphertext.
    pub fn decrypt_b64(&self, b64: &str) -> Result<Vec<u8>> {
        let ciphertext = STANDARD
            .decode(b64.trim())
            .map_err(|_| anyhow!("bad base64 from Discord"))?;
        let mut out = vec![0u8; self.key.min_output_size()];
        let n = self
            .key
            .decrypt(&OAEP_SHA256_MGF1SHA256, &ciphertext, &mut out, None)
            .map_err(|_| anyhow!("couldn't decrypt Discord's reply"))?
            .len();
        out.truncate(n);
        Ok(out)
    }

    /// The `nonce` to send back: base64url (no padding) of the decrypted
    /// `encrypted_nonce`.
    pub fn nonce_proof(&self, encrypted_nonce: &str) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(self.decrypt_b64(encrypted_nonce)?))
    }

    /// Decrypt the exchange's `encrypted_token` straight into a [`Token`].
    pub fn decrypt_token(&self, encrypted_token: &str) -> Result<Token> {
        let bytes = self.decrypt_b64(encrypted_token)?;
        let text =
            String::from_utf8(bytes).map_err(|_| anyhow!("Discord sent an unreadable login"))?;
        let token = Token::new(text.trim());
        if token.is_empty() {
            bail!("Discord sent an empty login");
        }
        Ok(token)
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// A message from the remote auth gateway. No `Debug`: `PendingLogin`
/// holds the ticket.
pub enum Incoming {
    Hello {
        heartbeat_interval: u64,
        timeout_ms: u64,
    },
    NonceProof {
        encrypted_nonce: String,
    },
    PendingRemoteInit {
        fingerprint: String,
    },
    PendingTicket {
        encrypted_user_payload: String,
    },
    PendingLogin {
        ticket: String,
    },
    Cancel,
    HeartbeatAck,
    /// An op we don't use (or a payload missing its field).
    Other,
}

/// Parse a text frame leniently (unknown ops become `Other`).
pub fn parse(text: &str) -> Option<Incoming> {
    let v: Value = serde_json::from_str(text).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    Some(match v.get("op").and_then(Value::as_str)? {
        "hello" => match n("heartbeat_interval") {
            Some(heartbeat_interval) => Incoming::Hello {
                heartbeat_interval,
                timeout_ms: n("timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS),
            },
            None => Incoming::Other,
        },
        "nonce_proof" => s("encrypted_nonce").map_or(Incoming::Other, |encrypted_nonce| {
            Incoming::NonceProof { encrypted_nonce }
        }),
        "pending_remote_init" => s("fingerprint").map_or(Incoming::Other, |fingerprint| {
            Incoming::PendingRemoteInit { fingerprint }
        }),
        "pending_ticket" => {
            s("encrypted_user_payload").map_or(Incoming::Other, |encrypted_user_payload| {
                Incoming::PendingTicket {
                    encrypted_user_payload,
                }
            })
        }
        "pending_login" => {
            s("ticket").map_or(Incoming::Other, |ticket| Incoming::PendingLogin { ticket })
        }
        "cancel" => Incoming::Cancel,
        "heartbeat_ack" => Incoming::HeartbeatAck,
        _ => Incoming::Other,
    })
}

pub fn init_msg(keys: &Keys) -> Value {
    json!({ "op": "init", "encoded_public_key": keys.encoded_public_key() })
}

pub fn nonce_proof_msg(nonce: &str) -> Value {
    json!({ "op": "nonce_proof", "nonce": nonce })
}

pub fn heartbeat_msg() -> Value {
    json!({ "op": "heartbeat" })
}

pub fn qr_url(fingerprint: &str) -> String {
    format!("{QR_BASE}{fingerprint}")
}

/// The scanned account, from `encrypted_user_payload`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPreview {
    pub id: String,
    pub discriminator: String,
    /// Avatar hash; `None` when Discord sends `0` (default avatar).
    pub avatar: Option<String>,
    pub username: String,
}

impl UserPreview {
    /// `id:discriminator:avatar:username` (the username keeps any colons).
    pub fn parse(plain: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(plain).ok()?;
        let mut parts = text.splitn(4, ':');
        let id = parts.next()?.trim().to_string();
        let discriminator = parts.next()?.to_string();
        let avatar = parts.next()?.to_string();
        let username = parts.next()?.to_string();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let avatar = (!avatar.is_empty() && avatar != "0").then_some(avatar);
        Some(Self {
            id,
            discriminator,
            avatar,
            username,
        })
    }

    /// "name" for new usernames, "name#1234" for legacy ones.
    pub fn display_name(&self) -> String {
        if self.discriminator.is_empty() || self.discriminator.chars().all(|c| c == '0') {
            self.username.clone()
        } else {
            format!("{}#{}", self.username, self.discriminator)
        }
    }

    pub fn avatar_url(&self) -> String {
        crate::types::avatar_url(&self.id, self.avatar.as_deref())
    }
}

/// User-facing text for a close code from the remote auth gateway.
pub fn close_reason(code: u16) -> String {
    match code {
        4003 => "The QR code expired. Try again for a new one.".into(),
        4000 | 4001 => "Discord rejected the QR login request. Try again, or paste a token.".into(),
        4002 => "The QR login handshake failed. Try again.".into(),
        _ => "Discord closed the QR login. Try again.".into(),
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Start the remote auth task. It waits for `ToBackend::StartQrLogin`,
/// reports through `ToUi::QrLogin`, and exits when `rx` closes.
pub fn spawn(
    tx: Sender<ToUi>,
    rx: UnboundedReceiver<ToBackend>,
    repaint: impl Fn() + Send + 'static,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            let _ = tx.send(ToUi::QrLogin(QrEvent::Failed(
                "Couldn't start the QR login.".into(),
            )));
            repaint();
            return;
        };
        rt.block_on(run(tx, rx, repaint));
    });
}

async fn run(tx: Sender<ToUi>, mut rx: UnboundedReceiver<ToBackend>, repaint: impl Fn()) {
    let send = |ev: QrEvent| {
        let _ = tx.send(ToUi::QrLogin(ev));
        repaint();
    };
    let mut start = false;
    loop {
        if !start {
            match rx.recv().await {
                Some(ToBackend::StartQrLogin) => {}
                Some(_) => continue,
                None => return,
            }
        }
        start = false;
        tokio::select! {
            r = attempt(&send) => match r {
                Ok(Some(ev)) => send(ev),
                Ok(None) => {}
                Err(e) => send(QrEvent::Failed(e.to_string())),
            },
            // Dropping `attempt` drops (closes) the socket.
            req = interrupted(&mut rx) => match req {
                Interrupt::Restart => start = true,
                Interrupt::Cancel => {}
                Interrupt::Stop => return,
            },
        }
    }
}

enum Interrupt {
    /// "Try again": a new code.
    Restart,
    /// `CancelQrLogin`: stop this attempt, wait for a new start.
    Cancel,
    /// The panel is gone.
    Stop,
}

async fn interrupted(rx: &mut UnboundedReceiver<ToBackend>) -> Interrupt {
    loop {
        match rx.recv().await {
            Some(ToBackend::StartQrLogin) => return Interrupt::Restart,
            Some(ToBackend::CancelQrLogin) => return Interrupt::Cancel,
            Some(_) => {}
            None => return Interrupt::Stop,
        }
    }
}

/// Next heartbeat tick, or never before `hello`.
async fn tick(heartbeat: &mut Option<tokio::time::Interval>) {
    match heartbeat {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// One QR login attempt, from connect to token (or a final event).
/// Errors carry user-safe text only.
async fn attempt(send: &impl Fn(QrEvent)) -> Result<Option<QrEvent>> {
    // Key generation takes a moment; keep it off the socket's clock.
    let keys = tokio::task::spawn_blocking(Keys::generate)
        .await
        .map_err(|_| anyhow!("Couldn't create a login key."))?
        .map_err(|_| anyhow!("Couldn't create a login key."))?;

    let mut req = GATEWAY_URL
        .into_client_request()
        .map_err(|_| anyhow!("Couldn't start the QR login."))?;
    req.headers_mut()
        .insert("Origin", HeaderValue::from_static(ORIGIN));
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|_| anyhow!("Couldn't reach Discord for QR login. Check your connection."))?;
    let (mut write, mut read) = ws.split();
    let text = |v: Value| WsMessage::Text(v.to_string().into());
    let lost = || anyhow!("Lost the connection to Discord. Try again.");

    let mut heartbeat: Option<tokio::time::Interval> = None;
    let mut acked = true;
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_millis(
        DEFAULT_TIMEOUT_MS,
    )));
    let mut ticket: Option<String> = None;

    while ticket.is_none() {
        let msg = tokio::select! {
            _ = tick(&mut heartbeat) => {
                if !acked {
                    return Err(lost());
                }
                acked = false;
                write.send(text(heartbeat_msg())).await.map_err(|_| lost())?;
                continue;
            }
            _ = &mut deadline => {
                let _ = write.send(WsMessage::Close(Some(close_normal()))).await;
                return Ok(Some(QrEvent::Failed(close_reason(4003))));
            }
            m = read.next() => m,
        };
        let frame = match msg {
            Some(Ok(WsMessage::Text(t))) => t,
            Some(Ok(WsMessage::Close(close))) => {
                let code = close.map_or(1000, |c| u16::from(c.code));
                return Ok(Some(QrEvent::Failed(close_reason(code))));
            }
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return Err(lost()),
        };
        let Some(incoming) = parse(frame.as_str()) else {
            continue;
        };
        match incoming {
            Incoming::Hello {
                heartbeat_interval,
                timeout_ms,
            } => {
                let every = Duration::from_millis(heartbeat_interval.max(1_000));
                heartbeat = Some(tokio::time::interval_at(
                    tokio::time::Instant::now() + every,
                    every,
                ));
                // A little past the server's own timeout (it closes 4003).
                deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_millis(timeout_ms + 5_000));
                write
                    .send(text(init_msg(&keys)))
                    .await
                    .map_err(|_| lost())?;
            }
            Incoming::NonceProof { encrypted_nonce } => {
                let nonce = keys
                    .nonce_proof(&encrypted_nonce)
                    .map_err(|_| anyhow!("The QR login handshake failed. Try again."))?;
                write
                    .send(text(nonce_proof_msg(&nonce)))
                    .await
                    .map_err(|_| lost())?;
            }
            Incoming::PendingRemoteInit { fingerprint } => {
                if fingerprint != keys.fingerprint() {
                    let _ = write.send(WsMessage::Close(Some(close_normal()))).await;
                    bail!("Discord's QR code didn't match this login. Try again.");
                }
                send(QrEvent::Code(qr_url(&fingerprint)));
            }
            Incoming::PendingTicket {
                encrypted_user_payload,
            } => {
                // The preview is cosmetic; a bad one just shows no name.
                let user = keys
                    .decrypt_b64(&encrypted_user_payload)
                    .ok()
                    .and_then(|p| UserPreview::parse(&p));
                send(QrEvent::Scanned {
                    username: user
                        .as_ref()
                        .map(UserPreview::display_name)
                        .unwrap_or_default(),
                    avatar: user
                        .as_ref()
                        .map(UserPreview::avatar_url)
                        .unwrap_or_default(),
                });
            }
            Incoming::PendingLogin { ticket: t } => ticket = Some(t),
            Incoming::Cancel => {
                let _ = write.send(WsMessage::Close(Some(close_normal()))).await;
                return Ok(Some(QrEvent::Cancelled));
            }
            Incoming::HeartbeatAck => acked = true,
            Incoming::Other => {}
        }
    }
    // Approved on the phone: the socket's job is done.
    let _ = write.send(WsMessage::Close(Some(close_normal()))).await;
    drop((write, read));

    let ticket = ticket.unwrap_or_default();
    let encrypted = exchange_ticket(&ticket).await?;
    let token = keys
        .decrypt_token(&encrypted)
        .map_err(|_| anyhow!("Couldn't read the login Discord sent. Try again."))?;
    Ok(Some(QrEvent::Token(token)))
}

fn close_normal() -> CloseFrame {
    CloseFrame {
        code: CloseCode::Normal,
        reason: "".into(),
    }
}

/// What the ticket exchange answered, from status + JSON body.
#[derive(Debug, PartialEq, Eq)]
pub enum Exchange {
    /// `encrypted_token` (ciphertext only; useless without our key).
    Encrypted(String),
    /// 429: wait this long, then retry once.
    RetryAfter(Duration),
    /// Failure, user-safe text.
    Failed(String),
}

/// Decide from an exchange response (pure, tested).
pub fn read_exchange(status: u16, body: &Value) -> Exchange {
    if (200..300).contains(&status) {
        return match body.get("encrypted_token").and_then(Value::as_str) {
            Some(t) if !t.is_empty() => Exchange::Encrypted(t.to_string()),
            _ => Exchange::Failed("Discord didn't send a login. Try again.".into()),
        };
    }
    if body.get("captcha_key").is_some() || body.get("captcha_sitekey").is_some() {
        return Exchange::Failed(
            "Discord asked for a captcha, which RustCord can't show. Log in with a token instead."
                .into(),
        );
    }
    match status {
        429 => {
            let secs = body
                .get("retry_after")
                .and_then(Value::as_f64)
                .unwrap_or(5.0)
                .max(0.0);
            Exchange::RetryAfter(Duration::from_secs_f64(secs.min(3600.0)))
        }
        400 | 404 => Exchange::Failed("The QR login expired. Try again for a new code.".into()),
        401 | 403 => Exchange::Failed("Discord refused the QR login. Try a token instead.".into()),
        _ => Exchange::Failed(format!(
            "Discord couldn't finish the QR login (HTTP {status})."
        )),
    }
}

/// `POST /users/@me/remote-auth/login {"ticket"}` → `encrypted_token`.
async fn exchange_ticket(ticket: &str) -> Result<String> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("RustCord/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .context("Couldn't start the QR login.")?;
    let mut retried = false;
    loop {
        let resp = http
            .post(LOGIN_URL)
            .header("Origin", ORIGIN)
            .json(&json!({ "ticket": ticket }))
            .send()
            .await
            .map_err(|_| anyhow!("Couldn't reach Discord to finish the QR login."))?;
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        match read_exchange(status, &body) {
            Exchange::Encrypted(t) => return Ok(t),
            Exchange::RetryAfter(wait) if !retried && wait <= MAX_RETRY_WAIT => {
                retried = true;
                tokio::time::sleep(wait).await;
            }
            Exchange::RetryAfter(wait) => {
                bail!(
                    "Discord is rate limiting logins. Try again in {} s.",
                    wait.as_secs().max(1)
                )
            }
            Exchange::Failed(text) => bail!(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::rsa::{OaepPublicEncryptingKey, PublicEncryptingKey};

    /// Encrypt like Discord does: RSA-OAEP SHA-256 with our public key.
    fn encrypt_for(keys: &Keys, plain: &[u8]) -> String {
        let public = PublicEncryptingKey::from_der(&keys.spki).unwrap();
        let public = OaepPublicEncryptingKey::new(public).unwrap();
        let mut out = vec![0u8; public.ciphertext_size()];
        let ct = public
            .encrypt(&OAEP_SHA256_MGF1SHA256, plain, &mut out, None)
            .unwrap();
        STANDARD.encode(ct)
    }

    #[test]
    fn keys_nonce_proof_fingerprint_and_token() {
        let keys = Keys::generate().unwrap();

        // init: base64 SPKI that parses back as an RSA-2048 public key.
        let der = STANDARD.decode(keys.encoded_public_key()).unwrap();
        let public = PublicEncryptingKey::from_der(&der).unwrap();
        assert_eq!(public.key_size_bits(), 2048);
        let init = init_msg(&keys);
        assert_eq!(init["op"], "init");
        assert_eq!(init["encoded_public_key"], keys.encoded_public_key());

        // nonce_proof: base64url (no padding) of the decrypted nonce.
        let nonce: Vec<u8> = (0u8..32).map(|b| b.wrapping_mul(37) ^ 0xA5).collect();
        let proof = keys.nonce_proof(&encrypt_for(&keys, &nonce)).unwrap();
        assert_eq!(proof, URL_SAFE_NO_PAD.encode(&nonce));
        assert!(!proof.contains('=') && !proof.contains('+') && !proof.contains('/'));
        let msg = nonce_proof_msg(&proof);
        assert_eq!(msg["op"], "nonce_proof");
        assert_eq!(msg["nonce"], proof.as_str());

        // Fingerprint: base64url SHA-256 of the SPKI (32 bytes → 43 chars).
        let fp = keys.fingerprint();
        assert_eq!(fp.len(), 43);
        let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &der);
        assert_eq!(fp, URL_SAFE_NO_PAD.encode(d.as_ref()));
        assert_eq!(qr_url(&fp), format!("https://discord.com/ra/{fp}"));

        // User preview and token decrypt.
        let user = keys
            .decrypt_b64(&encrypt_for(&keys, b"80351110224678912:0:a_abc:nelly"))
            .unwrap();
        let user = UserPreview::parse(&user).unwrap();
        assert_eq!(user.username, "nelly");
        assert_eq!(user.avatar.as_deref(), Some("a_abc"));
        let token = keys
            .decrypt_token(&encrypt_for(&keys, b"mfa.FAKE-token-123"))
            .unwrap();
        assert_eq!(token.expose(), "mfa.FAKE-token-123");
        assert_eq!(format!("{token:?}"), "Token(<redacted>)");
        assert_eq!(
            format!("{:?}", QrEvent::Token(token)),
            "Token(Token(<redacted>))"
        );

        // Garbage is an error, not a panic.
        assert!(keys.decrypt_b64("not base64!").is_err());
        assert!(keys.decrypt_b64(&STANDARD.encode([1u8; 256])).is_err());
        // Another key can't read our ciphertext.
        let other = Keys::generate().unwrap();
        assert!(other.decrypt_b64(&encrypt_for(&keys, b"x")).is_err());
        assert_ne!(other.fingerprint(), keys.fingerprint());
    }

    #[test]
    fn parses_gateway_messages() {
        assert!(matches!(
            parse(r#"{"op":"hello","heartbeat_interval":41250,"timeout_ms":152308}"#),
            Some(Incoming::Hello {
                heartbeat_interval: 41250,
                timeout_ms: 152308
            })
        ));
        assert!(matches!(
            parse(r#"{"op":"hello","heartbeat_interval":41250}"#),
            Some(Incoming::Hello {
                timeout_ms: DEFAULT_TIMEOUT_MS,
                ..
            })
        ));
        assert!(matches!(
            parse(r#"{"op":"nonce_proof","encrypted_nonce":"abc="}"#),
            Some(Incoming::NonceProof { encrypted_nonce }) if encrypted_nonce == "abc="
        ));
        assert!(matches!(
            parse(r#"{"op":"pending_remote_init","fingerprint":"fp_-x"}"#),
            Some(Incoming::PendingRemoteInit { fingerprint }) if fingerprint == "fp_-x"
        ));
        assert!(matches!(
            parse(r#"{"op":"pending_ticket","encrypted_user_payload":"u"}"#),
            Some(Incoming::PendingTicket { encrypted_user_payload }) if encrypted_user_payload == "u"
        ));
        assert!(matches!(
            parse(r#"{"op":"pending_login","ticket":"t1"}"#),
            Some(Incoming::PendingLogin { ticket }) if ticket == "t1"
        ));
        assert!(matches!(
            parse(r#"{"op":"cancel"}"#),
            Some(Incoming::Cancel)
        ));
        assert!(matches!(
            parse(r#"{"op":"heartbeat_ack"}"#),
            Some(Incoming::HeartbeatAck)
        ));
        // Lenient: unknown ops and missing fields are `Other`; junk is None.
        assert!(matches!(
            parse(r#"{"op":"pending_finish"}"#),
            Some(Incoming::Other)
        ));
        assert!(matches!(
            parse(r#"{"op":"pending_login"}"#),
            Some(Incoming::Other)
        ));
        assert!(matches!(parse(r#"{"op":"hello"}"#), Some(Incoming::Other)));
        assert!(parse("{}").is_none());
        assert!(parse("not json").is_none());
        assert_eq!(heartbeat_msg(), json!({ "op": "heartbeat" }));
    }

    #[test]
    fn user_preview() {
        let u = UserPreview::parse(b"123:0:0:sam").unwrap();
        assert_eq!(u.avatar, None);
        assert_eq!(u.display_name(), "sam");
        assert_eq!(u.avatar_url(), crate::types::avatar_url("123", None));
        let u = UserPreview::parse(b"123:0042:ff:old:name").unwrap();
        assert_eq!(u.username, "old:name");
        assert_eq!(u.display_name(), "old:name#0042");
        assert_eq!(u.avatar_url(), crate::types::avatar_url("123", Some("ff")));
        assert!(UserPreview::parse(b"123:0:0").is_none());
        assert!(UserPreview::parse(b"abc:0:0:x").is_none());
        assert!(UserPreview::parse(&[0xff, 0xfe]).is_none());
    }

    #[test]
    fn exchange_responses() {
        assert_eq!(
            read_exchange(200, &json!({ "encrypted_token": "ct" })),
            Exchange::Encrypted("ct".into())
        );
        assert!(matches!(
            read_exchange(200, &json!({})),
            Exchange::Failed(_)
        ));
        assert_eq!(
            read_exchange(429, &json!({ "retry_after": 1.5 })),
            Exchange::RetryAfter(Duration::from_millis(1500))
        );
        let Exchange::Failed(captcha) = read_exchange(
            400,
            &json!({ "captcha_key": ["captcha-required"], "captcha_sitekey": "x" }),
        ) else {
            panic!("captcha must fail");
        };
        assert!(captcha.contains("captcha"));
        assert!(matches!(
            read_exchange(400, &Value::Null),
            Exchange::Failed(_)
        ));
        assert!(matches!(
            read_exchange(500, &Value::Null),
            Exchange::Failed(_)
        ));
    }

    #[test]
    fn close_codes() {
        assert!(close_reason(4003).contains("expired"));
        for code in [1000, 4000, 4001, 4002, 4999] {
            assert!(!close_reason(code).is_empty());
        }
    }
}
