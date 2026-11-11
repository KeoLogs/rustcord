//! Where RustCord keeps its saved data (config, layout, window state,
//! themes). Demo runs (`RUSTCORD_DEMO=1`, with or without the screenshot /
//! layout variables) get their own storage key, so they never read or write
//! the real user's files.

use std::path::PathBuf;

/// eframe app id (storage folder name) of a normal run.
pub const APP_ID: &str = "RustCord";
/// eframe app id of every demo run.
pub const DEMO_APP_ID: &str = "RustCord-Demo";

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

/// Env vars a demo started from the login screen must not inherit: a
/// token, and the screenshot switches (those are for scripted runs only).
const DEMO_STRIPPED_ENV: &[&str] = &[
    "DISCORD_TOKEN",
    "RUSTCORD_DEMO_OPEN",
    "RUSTCORD_DEMO_SHOT",
    "RUSTCORD_DEMO_SCALE",
];

/// The command that runs `exe` as an offline demo: `RUSTCORD_DEMO=1`, no
/// token. A demo process uses [`DEMO_APP_ID`] storage and an in-memory
/// credential store, so it can't read or write the real config, saved
/// accounts, layout, window state or tokens.
pub fn demo_command(exe: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.env("RUSTCORD_DEMO", "1");
    for key in DEMO_STRIPPED_ENV {
        cmd.env_remove(key);
    }
    cmd
}

/// "Try the offline demo" on the login screen: start this program again as
/// a separate demo process (the caller then closes this window). The demo
/// never runs inside a normal run, which keeps the real config, accounts
/// and credential store in use.
pub fn launch_demo() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    demo_command(&exe).spawn().map(|_| ())
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
    fn demo_command_is_a_demo_without_a_token() {
        let cmd = demo_command(std::path::Path::new("rustcord"));
        let envs: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("RUSTCORD_DEMO".into(), Some("1".into()))));
        assert!(is_demo_value(Some("1")));
        // The token and screenshot switches are removed, never passed on.
        for key in DEMO_STRIPPED_ENV {
            assert!(envs.contains(&((*key).into(), None)), "{key} not removed");
        }
        assert_eq!(cmd.get_args().count(), 0);
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
