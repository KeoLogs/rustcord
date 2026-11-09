# Voice in RustCord — design and plan

Status: **groundwork**. RustCord can't join calls yet. This branch adds the
pure protocol layer (`src/voice/protocol.rs`), the "who's in voice" state
(`src/voice/state.rs`), the voice widgets (`src/voice/panel.rs`,
`src/voice/icons.rs`), and an opt-in build spike (`--features voice`,
`src/voice/spike.rs`) that proves the chosen crates compile together.

Legend used below: **[V]** verified against Discord's docs or the DAVE
whitepaper (and, where marked, two independent client implementations);
**[R]** taken from a reference implementation (discord.js
`@discordjs/voice` and/or songbird) but not stated in Discord's docs;
**[A]** assumed — check against a live Discord call.

## Why this is a big job

Since **1–2 March 2026** Discord requires DAVE (end-to-end encryption,
MLS-based) on voice and video calls; clients without it are refused with
voice close code **4017** "E2EE/DAVE protocol required" [V]. So a working
voice client needs three layers of crypto/protocol on top of audio:

1. transport encryption (AEAD over RTP, key from the voice gateway),
2. DAVE: an MLS group per call (key packages, proposals, commits,
   welcomes, epochs, transitions) and per-frame AES-128-GCM with a key
   ratchet per sender,
3. Opus at 48 kHz stereo in 20 ms frames, plus capture/playback devices,
   jitter buffering and mixing.

[A] Stage channels may be exempt from DAVE; treat everything as DAVE.

## Flow

```
UI click on a voice channel
  └─ ToBackend::VoiceJoin → main gateway op 4 {guild_id, channel_id, self_mute, self_deaf}
main gateway dispatches:
  VOICE_STATE_UPDATE (ours: session_id)   ┐ either order; `JoinCollector`
  VOICE_SERVER_UPDATE (token, endpoint)   ┘ → ConnectInfo
voice websocket  wss://{endpoint}/?v=8
  ← op 8 Hello {heartbeat_interval}          start heartbeats (op 3 {t, seq_ack})
  → op 0 Identify {server_id, user_id, session_id, token, max_dave_protocol_version: 1}
  ← op 2 Ready {ssrc, ip, port, modes}
UDP to ip:port
  → IP discovery request (74 bytes, type 1)
  ← IP discovery response (type 2: our external ip + port)
  → op 1 Select Protocol {protocol:"udp", data:{address, port, mode}}
  ← op 4 Session Description {mode, secret_key[32], dave_protocol_version}
DAVE (when dave_protocol_version > 0)
  ← op 25 external sender (binary)           → op 26 key package (binary)
  ← op 27 proposals (binary)                 → op 28 commit [+ welcome] (binary)
  ← op 29 announce commit / op 30 welcome    → op 23 transition ready
  ← op 22 execute transition                 (switch keys)
  ← op 24 prepare epoch (epoch 1 = new group; re-send key package)
  ← op 21 prepare transition (downgrade to 0 → passthrough)
media
  → op 5 Speaking {speaking: 1, delay: 0, ssrc} before the first packet
  ↔ RTP over UDP: header | AEAD(DAVE(opus)) | tag | 4-byte nonce
```

### Main gateway [V]

- Op 4 Voice State Update joins (`channel_id` set), leaves (`null`), or
  changes self mute/deafen. DM calls use `guild_id: null` (and need a ring
  over REST for 1:1 calls — later). [A] user clients also send
  `self_video: false`; we include it.
- `VOICE_SERVER_UPDATE {token, guild_id | channel_id, endpoint}`;
  `endpoint: null` means the server went away — wait for another.
- Voice states for the channel list come from `READY` guilds (user
  accounts) [A], `GUILD_CREATE.voice_states` (no `guild_id` in the
  entries) [V], `PASSIVE_UPDATE_V1.voice_states` for unfocused guilds
  (user accounts) [A], and `VOICE_STATE_UPDATE` [V].

### Voice gateway v8 [V]

- URL `wss://{endpoint}/?v=8`. Versions < 4 are rejected.
- Server messages that may be re-sent carry `seq`; binary ones carry it as a
  big-endian u16 prefix. Keep only the last value and send it as `seq_ack`
  in op 3 Heartbeat `{t, seq_ack}` and op 7 Resume. `-1` (or omitted) if
  none yet. [R] discord.js tracks seq from both JSON and binary frames.
- `heartbeat_interval` in op 2 Ready is "erroneous" — use op 8 Hello's.
- Op 5 Speaking from the server maps `user_id` ↔ `ssrc` (needed to know
  whose audio a packet is). Op 11 Clients Connect `{user_ids}`, op 13
  Client Disconnect `{user_id}`. Ops 12/14/15/18/20 exist but are
  undocumented; parsed as `Unknown` and ignored.
- Close codes: 4014 / 4021 / 4022 "should not reconnect", 4015 "try
  resuming" [V]; the rest of `close_action` is [A] (4006/4009/4011 →
  fresh connect; 4001–4005/4012/4016/4017/4020 → stop; network drop →
  resume).

### UDP IP discovery [V]

74 bytes, big-endian: `u16 type` (1 request / 2 response), `u16 length = 70`,
`u32 ssrc`, `64 bytes address` (NUL-terminated), `u16 port`. The request
leaves address and port zero [R].

### RTP and transport encryption

- Header [V]: `0x80`, payload type `0x78`, `u16 sequence`, `u32 timestamp`
  (+960 per 20 ms frame), `u32 ssrc`.
- Modes [V]: `aead_aes256_gcm_rtpsize` (preferred) and
  `aead_xchacha20_poly1305_rtpsize` (required); all others deprecated.
- `*_rtpsize` layout [V for the nonce suffix, R for the rest — discord.js
  and songbird agree]:
  `[RTP header + CSRCs + 4-byte extension preamble] = AAD | ciphertext |
  16-byte tag | 4-byte nonce`. The AEAD nonce is the 4 nonce bytes followed
  by zeros (12 bytes for AES-GCM, 24 for XChaCha20). Our counter is a
  big-endian u32 that starts random and wraps. The RTP extension *body* is
  inside the ciphertext: after decrypting, skip `4 × length` bytes to reach
  the Opus frame.
- RTCP (packet types 200–204) arrives on the same socket; skip it.
- Before the first audio packet send op 5 with a non-zero speaking flag, or
  the connection is dropped. When speech stops send five silence frames
  `F8 FF FE`.

### DAVE [V: whitepaper v1.1.4]

- MLS 1.0, ciphersuite 2 (`DHKEMP256_AES128GCM_SHA256_P256`), basic
  credentials whose identity is the big-endian u64 user id; the voice
  gateway is the single external sender.
- Binary frames: server → client `u16 seq | u8 op | payload`; client →
  server `u8 op | payload` (no seq).
  - 25 external sender: TLS `ExternalSender` → `set_external_sender`.
  - 26 key package (ours).
  - 27 proposals: `u8 operation` (0 append / 1 revoke) + `opaque<V>`.
  - 28 commit + optional welcome (ours).
  - 29 announce commit: `u16 transition_id` + MLS commit.
  - 30 welcome: `u16 transition_id` + MLS welcome.
- JSON: 21 prepare transition `{protocol_version, transition_id}`, 22
  execute `{transition_id}`, 23 ready `{transition_id}`, 24 prepare epoch
  `{protocol_version, epoch}`, 31 invalid commit/welcome `{transition_id}`.
  `transition_id: 0` executes immediately (sole member).
- Frame encryption: per-sender key from `MLS-Exporter("Discord Secure
  Frames v0", le64(user_id), 16)`, ratcheted per generation (top byte of a
  32-bit nonce); AES-128-GCM with an 8-byte tag. Frame = ciphertext | tag |
  ULEB128 nonce | ULEB128 unencrypted ranges | 1-byte size | `FA FA`. Opus
  is fully encrypted. Receivers keep old keys ~10 s across transitions.
  Silence frames `F8 FF FE` pass through.
- Verification: 30-digit epoch authenticator ("privacy code") and 45-digit
  pairwise fingerprints. [A] we only show the privacy code at first.

We do **not** implement MLS ourselves: `davey` does all of the above
behind `DaveSession` (`set_external_sender`, `create_key_package`,
`process_proposals`, `process_commit`, `process_welcome`,
`prepare_transition`, `execute_transition`, `encrypt_opus`, `decrypt`,
`set_passthrough_mode`, `voice_privacy_code`). Our job is to route the
opcodes to it — `protocol.rs` already parses/builds every one of them.

## Crate choices

| Need | Pick | Licence | Build | Notes |
|---|---|---|---|---|
| DAVE / MLS | **davey 0.2** | MIT [V: docs.rs] | pure Rust (openmls 0.8 + RustCrypto), no C deps [V] | By Snazzah; 0.2.0 built on docs.rs 2 Nov 2026 [V]. Reported to be what songbird (serenity) uses for DAVE [R — not re-checked]. Supports DAVE protocol v1. The only maintained permissive-licence Rust DAVE impl we found. |
| Opus | **opus2 0.4** (`bundled`) | MIT/Apache-2.0; libopus BSD-3 | C, built by cmake | Same crate songbird uses. Alt: `opus` 0.4 (opusic-sys, also cmake). `mousiki` (pure Rust Opus) is too young. |
| Transport AEAD | **aes-gcm 0.10**, **chacha20poly1305 0.10** | MIT/Apache-2.0 | pure Rust | Already in the tree via openmls_rust_crypto → no extra compile under `voice`. |
| Audio I/O | **cpal 0.18** | Apache-2.0 | ALSA on Linux (`libasound2-dev`), WASAPI, CoreAudio | Standard choice. Linux CI needs `libasound2-dev` once enabled. |
| Resampling | rubato (later, only if a device can't do 48 kHz) | MIT | pure Rust | |
| Full stack | songbird | ISC | — | Rejected as a whole: tied to serenity/twilight bot gateways, bot-oriented (no user-account voice), pulls symphonia, its own tungstenite version. We copy its *approach*, not the crate. |
| Not chosen | `dave` crate | AGPL-3.0-only | — | GPLv3 §13 allows linking it, but the AGPL terms would then apply to that part of every RustCord binary; tiny user base (hundreds of downloads). Avoid. |

GPL-3.0-only is compatible with MIT, Apache-2.0, ISC and BSD-3.

### Build requirements

- Default build: unchanged (no new deps compiled; `Cargo.lock` gains
  entries for the optional crates, which is normal for optional deps).
- `--features voice`: **cmake** and a C compiler on every platform (libopus),
  **libasound2-dev** on Linux (cpal/ALSA). GitHub's Windows/macOS/Linux
  runners have cmake; Linux needs `apt-get install libasound2-dev`.
- Spike result (this container, Linux x86_64, rustc 1.97): see "Spike
  numbers" below.

### RAM / binary cost (estimates unless measured)

- davey/openmls: one MLS group per call; group state grows with members
  (a few KB each). Expect < 1 MB for a 25-person call. [A]
- Opus: encoder ~30 KB, decoder ~26 KB **per speaking user** (decoders are
  created on first packet and dropped after a few seconds of silence).
- Audio buffers: 20 ms stereo i16 = 3.75 KB per frame; a 60–100 ms jitter
  buffer per speaker ≈ 20 KB. cpal streams: a few hundred KB.
- Total while in a call: ~1–3 MB on top of the app. Zero when the Voice
  module is off (nothing allocated, no threads).
- Binary: measured in the spike below.

## Staged plan

**Stage 0 — this branch (done):** protocol types + tests, voice-state
tracking, widgets, design doc, opt-in build spike.

**Stage 1 — wire up "who's in voice" (small, no audio):**
- `gateway.rs`: forward `voice_states` from READY / GUILD_CREATE /
  PASSIVE_UPDATE_V1 and `VOICE_STATE_UPDATE` as
  `ToUi::VoiceStates { guild_id, list: Value }` and
  `ToUi::VoiceStateUpdate(Value)`.
- `app.rs`: keep a `voice::state::VoiceStates`; draw voice channels with
  `panel::voice_channel_row` and members with `panel::channel_members`.
- New **Voice** module in `Modules` (off ⇒ don't store voice states).

**Stage 2 — join + listen (largest step):**
- Make `voice` a real feature and the Voice module's runtime switch. New
  `src/voice/conn.rs` in the backend: voice websocket (tokio-tungstenite,
  already a dep), heartbeat, resume, UDP socket, IP discovery,
  `Transport` (from the spike), `DaveSession` routing, per-SSRC Opus
  decoders, a small jitter buffer, mixing, cpal output.
- `ToBackend::VoiceJoin {guild_id, channel_id}`, `VoiceLeave`,
  `VoiceSetSelf {mute, deaf}`; `ToUi::VoiceServerUpdate`,
  `ToUi::VoiceConn(ConnStatus)`, `ToUi::VoiceSpeaking {user_id, speaking}`.
- The "Voice Connected" bar and mute/deafen buttons go live.
- Only joinable by a click (no auto-join, no auto-rejoin on startup).

**Stage 3 — speak:** cpal input, voice-activity detection (energy gate) or
push-to-talk, Opus encoder, DAVE `encrypt_opus`, transport seal, op 5
speaking, five silence frames on stop. Input/output device pickers and
volume in a Settings → Voice page.

**Stage 4 — polish:** per-user volume, privacy-code display / verification,
noise suppression (optional, e.g. nnnoiseless — RAM cost!), join/leave
sounds, DM calls (ring), reconnect UX.

**Stage 5 — video / Go Live (much later):** receive-only first (VP8/H264
decode + DAVE codec-specific unencrypted ranges), then camera / screen
share. Large: codecs, capture APIs per OS, RTX/NACK, simulcast.

### Honest estimate

| Stage | Work | Risk |
|---|---|---|
| 1 who's in voice | ~0.5 day | low |
| 2 join + listen | 4–7 days | **high**: DAVE transitions are only testable against live Discord; jitter/clock drift; cpal device quirks per OS |
| 3 speak | 2–4 days | medium (VAD tuning, echo without AEC) |
| 4 polish | 3–5 days | low–medium |
| 5 video | weeks | high |

Cloud sessions can't reach Discord, so stages 2–3 need Leo to test locally
on each OS. Debug logging of opcodes (never tokens/keys) will be essential.

## Spike numbers

Measured on this branch (Linux x86_64 container, rustc 1.97, cmake +
libasound2-dev installed):

- `cargo build/clippy/test --features voice`: green; 4 spike tests pass
  (both `*_rtpsize` AEAD round trips incl. tamper rejection, Opus 20 ms
  stereo encode/decode, cpal host/device lookup, a davey `DaveSession`
  producing a key package).
- Dependency graph: 469 crates (default) → 600 with `voice` (+131, mostly
  openmls/RustCrypto and cpal/ALSA). Default build compiles none of them.
- Release binary (thin LTO, not stripped): 28.09 MB default → 29.00 MB with
  `voice` and the smoke check linked in (**+0.9 MB**). The real client
  will add a little more (voice WS/UDP code), not another crate stack.
- `RUSTCORD_VOICE_SPIKE=1 ./rustcord` (release, `--features voice`) runs
  the whole smoke check — Opus, both ciphers, DAVE key package, ALSA device
  lookup — with a peak RSS of **13 MB** for the process (no window, no
  network), i.e. the voice stack itself is small; the per-call estimates
  above still need checking in a real call.
- Windows/macOS: not built here. All chosen crates list those platforms
  (cpal: WASAPI/CoreAudio; opus2 bundled libopus via cmake; davey and the
  AEADs are pure Rust) [R]. First CI run with `--features voice` will tell.

Run it locally: `RUSTCORD_VOICE_SPIKE=1 cargo run --release --features
voice` prints the results and exits (no network, no window).

## Open questions [A]

- Does READY for user accounts on our capabilities include
  `guilds[].voice_states`? (Bots get them in GUILD_CREATE.)
- Exact semantics of the JSON `seq` vs the binary u16 seq (same counter?
  We track both as one `u64`.)
- Whether user accounts must send extra Identify fields (`video`,
  `streams`) on the voice gateway, as Discord's own client does.
- Undocumented voice ops 12/14/15/18/20 (video sink wants, platform…).
- Stage channels and DAVE.
