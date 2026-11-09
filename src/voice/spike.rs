//! Build spike for `--features voice` (OFF by default): proves the chosen
//! crates compile and fit together with our pure protocol code.
//!
//! - transport encryption for both `*_rtpsize` modes (aes-gcm,
//!   chacha20poly1305 — the same RustCrypto versions openmls already uses),
//! - Opus encode/decode of 20 ms 48 kHz stereo frames (opus2 + bundled
//!   libopus, built with cmake),
//! - cpal audio device enumeration (ALSA on Linux, WASAPI on Windows,
//!   CoreAudio on macOS),
//! - a DAVE session (davey, pure Rust on OpenMLS) creating a key package and
//!   frame-encrypting / passthrough of Opus.
//!
//! Not wired into the app. Run with `cargo test --features voice spike`.

use aes_gcm::aead::AeadInPlace;
use aes_gcm::{Aes256Gcm, KeyInit};
use chacha20poly1305::XChaCha20Poly1305;

use super::protocol::{self, Mode, NonceCounter, ProtoError};

/// A transport cipher for the voice UDP socket (key from op 4).
pub enum Transport {
    Aes(Box<Aes256Gcm>),
    XChaCha(Box<XChaCha20Poly1305>),
}

impl Transport {
    pub fn new(mode: Mode, key: &[u8]) -> Option<Self> {
        if key.len() != Mode::KEY_LEN {
            return None;
        }
        Some(match mode {
            Mode::Aes256GcmRtpSize => {
                Transport::Aes(Box::new(Aes256Gcm::new_from_slice(key).ok()?))
            }
            Mode::XChaCha20Poly1305RtpSize => {
                Transport::XChaCha(Box::new(XChaCha20Poly1305::new_from_slice(key).ok()?))
            }
        })
    }

    fn mode(&self) -> Mode {
        match self {
            Transport::Aes(_) => Mode::Aes256GcmRtpSize,
            Transport::XChaCha(_) => Mode::XChaCha20Poly1305RtpSize,
        }
    }

    /// Seal one outgoing packet: RTP header is the AAD, the (DAVE-encrypted)
    /// Opus frame is encrypted, then tag and 4-byte nonce are appended.
    pub fn seal(&self, header: &[u8], payload: &[u8], nonce: &mut NonceCounter) -> Vec<u8> {
        let suffix = nonce.next_suffix();
        let full = protocol::expand_nonce(suffix);
        let n = &full[..self.mode().nonce_len()];
        let mut body = payload.to_vec();
        let tag = match self {
            Transport::Aes(c) => c
                .encrypt_in_place_detached(n.into(), header, &mut body)
                .map(|t| t.to_vec()),
            Transport::XChaCha(c) => c
                .encrypt_in_place_detached(n.into(), header, &mut body)
                .map(|t| t.to_vec()),
        }
        .expect("AEAD encryption of a voice packet cannot fail");
        protocol::seal_packet(header, &body, &tag, suffix)
    }

    /// Open a received packet; returns the media payload (extension body
    /// already skipped) and the RTP header.
    pub fn open(&self, packet: &[u8]) -> Result<(protocol::RtpHeader, Vec<u8>), ProtoError> {
        let header = protocol::parse_rtp(packet)?;
        let l = protocol::sealed_layout(packet, &header)?;
        let mut suffix = [0u8; 4];
        suffix.copy_from_slice(&packet[l.nonce_suffix.clone()]);
        let full = protocol::expand_nonce(suffix);
        let n = &full[..self.mode().nonce_len()];
        let aad = &packet[l.aad.clone()];
        let tag = &packet[l.tag.clone()];
        let mut body = packet[l.ciphertext.clone()].to_vec();
        let ok = match self {
            Transport::Aes(c) => c
                .decrypt_in_place_detached(n.into(), aad, &mut body, tag.into())
                .is_ok(),
            Transport::XChaCha(c) => c
                .decrypt_in_place_detached(n.into(), aad, &mut body, tag.into())
                .is_ok(),
        };
        if !ok {
            return Err(ProtoError::Invalid("decryption failed"));
        }
        let skip = protocol::rtp_extension_body_len(packet, &header).min(body.len());
        Ok((header, body.split_off(skip)))
    }
}

/// Runtime smoke check, run by `RUSTCORD_VOICE_SPIKE=1 cargo run --features
/// voice`: exercises Opus, both transport ciphers, a DAVE session and the
/// real audio devices, prints the results and returns. It never touches the
/// network. Useful on a machine with a sound card (CI/containers have none).
pub fn smoke() {
    use cpal::traits::{DeviceTrait, HostTrait};

    let pcm = vec![0i16; protocol::FRAME_SAMPLES as usize * 2];
    let mut packet = [0u8; 1275];
    let encoded = opus2::Encoder::new(48_000, opus2::Channels::Stereo, opus2::Application::Voip)
        .and_then(|mut e| e.encode(&pcm, &mut packet));
    match encoded {
        Ok(n) => println!("opus: encoded a 20 ms stereo frame into {n} bytes"),
        Err(e) => println!("opus: FAILED {e}"),
    }

    for mode in [Mode::Aes256GcmRtpSize, Mode::XChaCha20Poly1305RtpSize] {
        let t = Transport::new(mode, &[1u8; 32]).expect("32-byte key");
        let header = protocol::RtpCounter::new(0, 0).next_header(1);
        let sealed = t.seal(&header, &protocol::OPUS_SILENCE, &mut NonceCounter(0));
        let ok = t
            .open(&sealed)
            .is_ok_and(|(_, p)| p == protocol::OPUS_SILENCE);
        println!("transport {}: round trip {}", mode.as_str(), ok);
    }

    match std::num::NonZeroU16::new(davey::DAVE_PROTOCOL_VERSION)
        .and_then(|v| davey::DaveSession::new(v, 1, 2, None).ok())
    {
        Some(mut s) => match s.create_key_package() {
            Ok(kp) => println!("dave: key package of {} bytes", kp.len()),
            Err(e) => println!("dave: key package FAILED {e:?}"),
        },
        None => println!("dave: session FAILED"),
    }

    let host = cpal::default_host();
    println!("audio host: {:?}", host.id());
    let name = |d: Option<cpal::Device>| {
        d.and_then(|d| d.description().ok().map(|x| x.name().to_string()))
            .unwrap_or_else(|| "none".into())
    };
    println!("default input: {}", name(host.default_input_device()));
    println!("default output: {}", name(host.default_output_device()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    #[test]
    fn spike_transport_roundtrip_both_modes() {
        for mode in [Mode::Aes256GcmRtpSize, Mode::XChaCha20Poly1305RtpSize] {
            let key = [7u8; 32];
            let t = Transport::new(mode, &key).unwrap();
            let mut nonce = NonceCounter(41);
            let mut rtp = protocol::RtpCounter::new(1, 2);
            let header = rtp.next_header(1234);
            let pkt = t.seal(&header, &protocol::OPUS_SILENCE, &mut nonce);
            assert_eq!(pkt.len(), 12 + 3 + 16 + 4);
            assert_eq!(&pkt[pkt.len() - 4..], &41u32.to_be_bytes());
            let (h, payload) = t.open(&pkt).unwrap();
            assert_eq!(h.ssrc, 1234);
            assert_eq!(payload, protocol::OPUS_SILENCE);

            // Tampered header (AAD) fails.
            let mut bad = pkt.clone();
            bad[3] ^= 1;
            assert!(t.open(&bad).is_err());
        }
        assert!(Transport::new(Mode::Aes256GcmRtpSize, &[0; 16]).is_none());
    }

    #[test]
    fn spike_opus_20ms_stereo() {
        let mut enc =
            opus2::Encoder::new(48_000, opus2::Channels::Stereo, opus2::Application::Voip)
                .expect("opus encoder");
        let mut dec = opus2::Decoder::new(48_000, opus2::Channels::Stereo).expect("opus decoder");
        let n = protocol::FRAME_SAMPLES as usize;
        let pcm: Vec<i16> = (0..n * 2)
            .map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16)
            .collect();
        let mut packet = [0u8; 1275];
        let len = enc.encode(&pcm, &mut packet).expect("encode");
        assert!(len > 0);
        let mut out = vec![0i16; n * 2];
        let got = dec.decode(&packet[..len], &mut out, false).expect("decode");
        assert_eq!(got, n);
    }

    #[test]
    fn spike_cpal_host() {
        use cpal::traits::HostTrait;
        // CI / containers have no sound card: only check the API links.
        let host = cpal::default_host();
        let _ = host.default_input_device();
        let _ = host.default_output_device();
    }

    #[test]
    fn spike_dave_session() {
        let v = NonZeroU16::new(davey::DAVE_PROTOCOL_VERSION).unwrap();
        let mut s = davey::DaveSession::new(v, 104694319306248192, 41771983423143937, None)
            .expect("dave session");
        let kp = s.create_key_package().expect("key package");
        let frame = protocol::dave_key_package(&kp);
        assert_eq!(frame[0], protocol::op::DAVE_MLS_KEY_PACKAGE);
        assert!(kp.len() > 100);
        // Not in a group yet: can't encrypt.
        assert!(!s.is_ready());
        // Silence passes through untouched per the whitepaper.
        if let Ok(out) = s.encrypt_opus(&protocol::OPUS_SILENCE) {
            assert_eq!(&*out, &protocol::OPUS_SILENCE);
        }
    }
}
