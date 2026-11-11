//! Where RustCord keeps its saved data (config, layout, window state,
//! themes). Demo runs (`RUSTCORD_DEMO=1`, with or without the screenshot /
//! layout variables) get their own storage key, so they never read or write
//! the real user's files.

use std::path::PathBuf;

/// eframe app id (storage folder name) of a normal run.
pub const APP_ID: &str = "RustCord";
/// eframe app id of every demo run.
pub const DEMO_APP_ID: &str = "RustCord-Demo";
/// eframe app id of a `RUSTCORD_TEST_ACCOUNT` run (memprobe live mode).
pub const TEST_APP_ID: &str = "RustCord-Test";

/// `RUSTCORD_TEST_ACCOUNT=<user id>`: log in as that saved account (token
/// from the OS credential store, never from the environment), with its own
/// storage, read-only. Ignored in demo runs.
pub fn test_account_from_env() -> Option<String> {
    std::env::var("RUSTCORD_TEST_ACCOUNT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// `RUSTCORD_START_MINIMIZED=1` (memprobe and other test launches): the
/// window starts hidden and unfocused, then minimises itself on the first
/// frame and keeps rendering in the taskbar. The saved window state is
/// never touched by such runs.
pub fn start_minimized_from_env() -> bool {
    std::env::var("RUSTCORD_START_MINIMIZED").is_ok_and(|v| v.trim() == "1")
}

/// True when `RUSTCORD_DEMO` asks for the offline demo.
pub fn demo_from_env() -> bool {
    is_demo_value(std::env::var("RUSTCORD_DEMO").ok().as_deref())
}

/// The value of `RUSTCORD_DEMO` that turns demo mode on (only `1`).
pub fn is_demo_value(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

/// The eframe app id (and so the storage folder) for this run.
pub fn app_id(demo: bool) -> &'static str {
    if demo { DEMO_APP_ID } else { APP_ID }
}

/// eframe's data folder for this run.
pub fn data_dir(demo: bool) -> Option<PathBuf> {
    eframe::storage_dir(app_id(demo))
}

/// `<data dir>/themes`.
pub fn themes_dir(demo: bool) -> Option<PathBuf> {
    data_dir(demo).map(|d| d.join("themes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_value() {
        assert!(is_demo_value(Some("1")));
        assert!(is_demo_value(Some(" 1 ")));
        assert!(!is_demo_value(Some("0")));
        assert!(!is_demo_value(Some("")));
        assert!(!is_demo_value(None));
    }

    #[test]
    fn demo_never_shares_the_real_key() {
        assert_ne!(app_id(true), app_id(false));
        assert_eq!(app_id(false), "RustCord");
        if let (Some(real), Some(demo)) = (data_dir(false), data_dir(true)) {
            assert_ne!(real, demo);
            assert!(!demo.starts_with(&real));
        }
        if let (Some(real), Some(demo)) = (themes_dir(false), themes_dir(true)) {
            assert_ne!(real, demo);
        }
    }
}
