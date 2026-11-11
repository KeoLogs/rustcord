//! Notification sound: RustCord's own short "ping" (`assets/sounds/ping.wav`,
//! synthesised by `scripts/make_ping.py`; not Discord's sound), or a file
//! the user picked (wav / ogg / mp3, at most 1 MB).
//!
//! Nothing audio-related exists until a sound plays: the output device is
//! opened for that one sound on a backend blocking thread
//! (`ToBackend::PlaySound`) and closed when it ends. With the sound off (or
//! the Notifications module off) this module is never called.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The bundled ping (in the binary's read-only data, not on the heap).
static PING: &[u8] = include_bytes!("../assets/sounds/ping.wav");

/// Biggest custom sound file.
pub const MAX_CUSTOM_BYTES: u64 = 1024 * 1024;
/// File types the picker offers and `check_custom` accepts.
pub const EXTENSIONS: &[&str] = &["wav", "ogg", "mp3"];
pub const DEFAULT_VOLUME: u8 = 50;
/// Longest a sound may play (a custom file could be minutes of silence).
const MAX_PLAY: std::time::Duration = std::time::Duration::from_secs(5);

/// One sound at a time: a burst of notifications pings once.
static PLAYING: AtomicBool = AtomicBool::new(false);

/// Is this file usable as the custom sound? `len` is its size in bytes.
/// The error is shown to the user.
pub fn check_custom(path: &Path, len: u64) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !ext.is_some_and(|e| EXTENSIONS.contains(&e.as_str())) {
        return Err("The sound must be a .wav, .ogg or .mp3 file.".into());
    }
    if len == 0 {
        return Err("That sound file is empty.".into());
    }
    if len > MAX_CUSTOM_BYTES {
        return Err("The sound file must be 1 MB or smaller.".into());
    }
    Ok(())
}

/// Slider position (0-100) to amplitude. Squared, so the slider feels even.
pub fn gain(volume: u8) -> f32 {
    let v = f32::from(volume.min(100)) / 100.0;
    v * v
}

/// Read and check the custom sound file.
fn load_custom(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path)
        .map_err(|_| "The custom sound file can't be found.".to_owned())?
        .len();
    check_custom(path, len)?;
    std::fs::read(path).map_err(|_| "The custom sound file can't be read.".to_owned())
}

/// Play the notification sound and wait for it to end (call on a blocking
/// thread). A custom file that can't be used falls back to the ping and
/// returns the reason for a toast.
pub fn play(custom: Option<&Path>, volume: u8) -> Result<(), String> {
    if volume == 0 || PLAYING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let result = play_now(custom, volume);
    PLAYING.store(false, Ordering::Release);
    result
}

fn play_now(custom: Option<&Path>, volume: u8) -> Result<(), String> {
    use std::io::Cursor;
    let mut problem = None;
    let custom = custom.and_then(|p| match load_custom(p) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            problem = Some(e);
            None
        }
    });
    let source = match custom.map(|bytes| rodio::Decoder::new(Cursor::new(bytes))) {
        Some(Ok(decoder)) => decoder_box(decoder),
        Some(Err(_)) => {
            problem = Some("The custom sound file can't be played.".to_owned());
            ping()?
        }
        None => ping()?,
    };
    // No output device (or it's busy): stay silent, that's not an error
    // worth a toast for every notification.
    let Ok(mut sink) = rodio::DeviceSinkBuilder::open_default_sink() else {
        return problem.map_or(Ok(()), Err);
    };
    sink.log_on_drop(false);
    let player = rodio::Player::connect_new(sink.mixer());
    player.set_volume(gain(volume));
    player.append(source);
    let started = std::time::Instant::now();
    while !player.empty() && started.elapsed() < MAX_PLAY {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    player.stop();
    problem.map_or(Ok(()), Err)
}

type Source = Box<dyn rodio::Source + Send>;

fn decoder_box<R>(d: rodio::Decoder<R>) -> Source
where
    R: std::io::Read + std::io::Seek + Send + Sync + 'static,
{
    Box::new(d)
}

fn ping() -> Result<Source, String> {
    rodio::Decoder::new(std::io::Cursor::new(PING))
        .map(decoder_box)
        .map_err(|_| "The notification sound can't be played.".to_owned())
}

/// The "Browse…" dialog for the custom sound (backend).
pub async fn pick_file() -> Option<std::path::PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Notification sound")
        .add_filter("Sound (wav, ogg, mp3)", EXTENSIONS)
        .pick_file()
        .await
        .map(|f| f.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source as _;

    #[test]
    fn custom_file_rules() {
        let ok = |name: &str, len| check_custom(Path::new(name), len);
        assert!(ok("ding.wav", 10).is_ok());
        assert!(ok("C:/sounds/Ding.OGG", MAX_CUSTOM_BYTES).is_ok());
        assert!(ok("a.b.mp3", 4000).is_ok());
        assert!(ok("ding.mp3", MAX_CUSTOM_BYTES + 1).is_err());
        assert!(ok("ding.wav", 0).is_err());
        for bad in ["ding.flac", "ding", "wav", "ding.wav.exe", "ding.m4a"] {
            assert!(ok(bad, 10).is_err(), "{bad}");
        }
    }

    #[test]
    fn volume_curve() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(100), 1.0);
        assert_eq!(gain(250), 1.0);
        assert!((gain(50) - 0.25).abs() < 1e-6);
        assert!(gain(20) < gain(40));
    }

    #[test]
    fn the_bundled_ping_is_a_short_wav_that_decodes() {
        assert_eq!(&PING[..4], b"RIFF");
        assert_eq!(&PING[8..12], b"WAVE");
        assert!(PING.len() < 32 * 1024, "keep the ping tiny");
        let source = ping().expect("the ping decodes");
        let rate = source.sample_rate().get() as usize;
        let channels = source.channels().get() as usize;
        let samples = source.count();
        let secs = samples as f32 / (rate * channels) as f32;
        assert!((0.1..0.5).contains(&secs), "{secs}");
    }

    #[test]
    fn a_missing_custom_file_is_reported_not_fatal() {
        assert!(load_custom(Path::new("/definitely/not/here.wav")).is_err());
    }
}
