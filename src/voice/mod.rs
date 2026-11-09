//! Voice channels. Groundwork stage: protocol types (pure, tested), the
//! UI-facing voice state, and the voice widgets. Audio, UDP and DAVE come
//! later — see docs/VOICE.md for the plan. The `voice` cargo feature
//! (off by default) compiles a spike with the chosen DAVE / Opus / AEAD
//! crates to prove they build; nothing in the default build depends on it.

pub mod icons;
pub mod panel;
pub mod protocol;
pub mod state;

#[cfg(feature = "voice")]
pub mod spike;
