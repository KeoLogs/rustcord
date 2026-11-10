//! Paste media into the message box: Ctrl+V with an image or copied files
//! adds them to the upload list (the cards above the box). Nothing is sent
//! until the user presses Enter or the send button, like any other upload.
//!
//! - egui turns Ctrl+V into `Event::Paste(text)` or, when the clipboard has
//!   no text, `Event::PasteImage`. Copied files (Explorer, Finder, file
//!   managers) are read with `arboard` (`clipboard_files`). On Windows a
//!   file-only clipboard gives egui no event at all, so `ctrl_v_pressed`
//!   watches the V key while Ctrl is held and the message box has focus.
//! - Clipboard reads are local OS calls on the UI thread (no network). A
//!   pasted image is encoded to PNG by the backend on a blocking thread
//!   (`ToBackend::SavePastedImage` → `save_png` → `ToUi::FilesPicked`).
//! - Discord's per-file limit depends on Nitro and server boosts
//!   (`upload_limit`); bigger files are refused with a clear message.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui::ColorImage;

const MB: u64 = 1024 * 1024;

/// Discord's per-file upload limit for me in this place: the best of my
/// Nitro (`premium_type` 1 Classic, 2 Nitro, 3 Basic) and the server's
/// boost level (`premium_tier`).
pub fn upload_limit(premium_type: u8, guild_tier: u8) -> u64 {
    let nitro = match premium_type {
        2 => 500 * MB,
        1 | 3 => 50 * MB,
        _ => 10 * MB,
    };
    let boost = match guild_tier {
        3 => 100 * MB,
        2 => 50 * MB,
        _ => 10 * MB,
    };
    nitro.max(boost)
}

/// The limit as Discord writes it ("10 MB").
pub fn limit_label(limit: u64) -> String {
    format!("{} MB", limit / MB)
}

/// Folder for pasted images (in the OS temp folder, pruned after a day).
pub fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("rustcord-paste")
}

/// Encode a pasted image as `image.png` (Discord's name for pasted images)
/// in a fresh sub-folder of `dir`, so two pastes are two uploads. Old
/// pastes (over a day) are removed first.
pub fn save_png(image: &ColorImage, dir: &Path) -> anyhow::Result<PathBuf> {
    prune(dir, Duration::from_secs(24 * 3600));
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, bytes)
        .ok_or_else(|| anyhow::anyhow!("The pasted image is empty"))?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let folder = dir.join(stamp.to_string());
    std::fs::create_dir_all(&folder)?;
    let path = folder.join("image.png");
    buf.save_with_format(&path, image::ImageFormat::Png)?;
    Ok(path)
}

/// Remove paste folders older than `max_age`.
fn prune(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Files copied in a file manager, if the clipboard holds any.
pub fn clipboard_files() -> Vec<PathBuf> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.get().file_list())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
}

/// Is the V key down right now? Only asked while Ctrl is held and the
/// message box has focus (Windows gives egui no event for a file-only
/// clipboard). Elsewhere copied files arrive with a text paste.
#[cfg(windows)]
pub fn v_key_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_V};
    // SAFETY: GetAsyncKeyState only reads the keyboard state.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_V)) };
    (state as u16) & 0x8000 != 0
}

#[cfg(not(windows))]
pub fn v_key_down() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    #[test]
    fn limits_follow_nitro_and_boosts() {
        assert_eq!(upload_limit(0, 0), 10 * MB);
        assert_eq!(upload_limit(3, 0), 50 * MB);
        assert_eq!(upload_limit(2, 0), 500 * MB);
        assert_eq!(upload_limit(0, 2), 50 * MB);
        assert_eq!(upload_limit(0, 3), 100 * MB);
        // The best of the two wins.
        assert_eq!(upload_limit(2, 3), 500 * MB);
        assert_eq!(limit_label(upload_limit(0, 0)), "10 MB");
    }

    #[test]
    fn saves_png_in_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let img = ColorImage::new([3, 2], vec![Color32::RED; 6]);
        let a = save_png(&img, dir.path()).unwrap();
        let b = save_png(&img, dir.path()).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.file_name().unwrap(), "image.png");
        let back = image::open(&a).unwrap();
        assert_eq!((back.width(), back.height()), (3, 2));
    }
}
