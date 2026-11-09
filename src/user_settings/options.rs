//! Settings → Windows / Advanced: the Discord Messenger options RustCord
//! lacked (DM `windows/OptionsDialog.cpp` PG_WINDOW / PG_CONNECTION,
//! `core/config/LocalSettings`), as a Config-agnostic [`AppOptions`] the
//! integrator stores (e.g. `Config::app_options`, `#[serde(default)]`).
//!
//! What each option needs from the integrator (this module only stores the
//! setting):
//! - `save_window_size`: eframe `NativeOptions::persist_window` (on = eframe
//!   restores size / position from its storage).
//! - `start_maximized` / `start_minimized`: `ViewportBuilder::with_maximized`
//!   / `with_minimized` in `main.rs`. Like DM, "start minimised" only applies
//!   when RustCord was opened on startup; integrator passes a `--minimized`
//!   flag in the autostart entry and checks it.
//! - `minimize_to_tray`: TODO, needs a tray crate (`tray-icon` + an event
//!   loop hook); the switch is stored but does nothing yet (labelled so).
//! - `open_on_startup`: per OS, written when the setting changes (TODO):
//!   - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value
//!     `RustCord` = `"<exe>" --minimized?` (no admin rights needed).
//!   - macOS: `~/Library/LaunchAgents/io.github.keologs.rustcord.plist` with
//!     `RunAtLoad` true (or `SMAppService` from a bundled app).
//!   - Linux / BSD: XDG autostart `~/.config/autostart/rustcord.desktop`
//!     ([`linux_autostart_entry`]).
//! - `api_base` / `cdn_base`: `api::API_BASE` and `types` CDN constants
//!   must read these (None = official). Changing them logs out
//!   ([`super::Action::LogOutForNewBackend`]) so the Discord token is never
//!   sent to another host, as Discord Messenger does.
//! - `verify_tls`: reqwest `ClientBuilder::danger_accept_invalid_certs` and
//!   a custom rustls verifier for tokio-tungstenite. Takes effect at the next
//!   login. Turning it off asks first (DM's IDS_CERT_SETTING_CONFIRM).
//! - `check_updates`: on startup send `Request::CheckUpdates` (unauthenticated
//!   GitHub API, no token) and show a notice if [`UpdateInfo::newer`].
//! - `language`: placeholder (English only; see `language.rs`).

use std::cmp::Ordering;

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::widgets::{self, Confirm};
use super::{Action, Ctx, Request};

pub const OFFICIAL_API: &str = "https://discord.com/api/v10";
pub const OFFICIAL_CDN: &str = "https://cdn.discordapp.com";
/// GitHub releases API for update checks.
pub const RELEASES_URL: &str = "https://api.github.com/repos/KeoLogs/rustcord/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/KeoLogs/rustcord/releases";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppOptions {
    // Windows
    pub save_window_size: bool,
    pub start_maximized: bool,
    pub open_on_startup: bool,
    pub start_minimized: bool,
    /// TODO: needs a tray crate; stored only.
    pub minimize_to_tray: bool,
    // Advanced
    /// Custom REST base (None = official). Without a trailing slash.
    pub api_base: Option<String>,
    /// Custom CDN base (None = official).
    pub cdn_base: Option<String>,
    pub verify_tls: bool,
    pub check_updates: bool,
    /// BCP-47 language tag (only "en-US" for now).
    pub language: String,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            save_window_size: true,
            start_maximized: false,
            open_on_startup: false,
            start_minimized: false,
            minimize_to_tray: false,
            api_base: None,
            cdn_base: None,
            verify_tls: true,
            check_updates: false,
            language: "en-US".into(),
        }
    }
}

impl AppOptions {
    pub fn api_base(&self) -> &str {
        self.api_base.as_deref().unwrap_or(OFFICIAL_API)
    }

    pub fn cdn_base(&self) -> &str {
        self.cdn_base.as_deref().unwrap_or(OFFICIAL_CDN)
    }
}

/// Check and normalise a custom base URL: https (http only for localhost),
/// a host, no query / fragment / spaces, no trailing slash.
pub fn validate_base_url(input: &str) -> Result<String, String> {
    let url = input.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err("Enter a URL.".into());
    }
    if url.chars().any(char::is_whitespace) || url.contains(['?', '#', '@']) {
        return Err("That URL has characters that aren't allowed (spaces, ?, # or @).".into());
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("The URL must start with https://")?;
    let host = rest.split('/').next().unwrap_or("");
    let host_name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if host_name.is_empty() {
        return Err("The URL needs a host name.".into());
    }
    let local = matches!(host_name, "localhost" | "127.0.0.1" | "[::1]");
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {}
        "http" if local => {}
        "http" => return Err("Use https:// (plain http is only allowed for localhost).".into()),
        _ => return Err("The URL must start with https://".into()),
    }
    Ok(url.to_string())
}

/// XDG autostart entry (`~/.config/autostart/rustcord.desktop`).
pub fn linux_autostart_entry(exe: &str, minimized: bool) -> String {
    let quoted = format!("\"{}\"", exe.replace('\\', "\\\\").replace('"', "\\\""));
    let args = if minimized { " --minimized" } else { "" };
    format!(
        "[Desktop Entry]\nType=Application\nName=RustCord\nExec={quoted}{args}\n\
         X-GNOME-Autostart-enabled=true\nTerminal=false\n"
    )
}

// ---------------------------------------------------------------------------
// Versions / updates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub core: [u64; 3],
    /// Pre-release identifiers ("beta", "1"); empty = release.
    pub pre: Vec<String>,
}

/// "v1.2.3", "1.2", "1.2.3-beta.1+build" → Version. None if not a version.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut out = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[i] = p.parse().ok()?;
    }
    let pre = match pre {
        Some(p) if p.is_empty() || p.split('.').any(str::is_empty) => return None,
        Some(p) => p.split('.').map(str::to_string).collect(),
        None => Vec::new(),
    };
    Some(Version { core: out, pre })
}

/// Semver precedence (build metadata ignored).
pub fn compare_versions(a: &Version, b: &Version) -> Ordering {
    a.core
        .cmp(&b.core)
        .then_with(|| match (a.pre.is_empty(), b.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (x, y) in a.pre.iter().zip(&b.pre) {
                    let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(m), Ok(n)) => m.cmp(&n),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => x.cmp(y),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                a.pre.len().cmp(&b.pre.len())
            }
        })
}

/// Is `latest` newer than `current`? False if either doesn't parse.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => compare_versions(&l, &c) == Ordering::Greater,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    /// Tag of the latest release ("v0.2.0").
    pub latest: String,
    pub newer: bool,
    /// Release page.
    pub url: String,
}

/// GitHub `releases/latest` body → UpdateInfo, compared with `current`.
pub fn parse_release(v: &Value, current: &str) -> Result<UpdateInfo, String> {
    let tag = v
        .get("tag_name")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or("The latest release has no version tag.")?;
    if parse_version(tag).is_none() {
        return Err(format!(
            "The latest release's tag \"{tag}\" isn't a version."
        ));
    }
    let url = v
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://github.com/"))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    Ok(UpdateInfo {
        latest: tag.to_string(),
        newer: is_newer(tag, current),
        url,
    })
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Urls { api: String, cdn: String },
    Official,
    DisableTls,
}

#[derive(Default)]
pub struct OptionsPage {
    api_text: Option<String>,
    cdn_text: Option<String>,
    url_error: Option<String>,
    pending: Option<Pending>,
    checking: bool,
    update: Option<Result<UpdateInfo, String>>,
}

impl OptionsPage {
    pub fn apply_update(&mut self, r: Result<UpdateInfo, String>) {
        self.checking = false;
        self.update = Some(r);
    }

    /// Latest update check result (for a startup notice).
    pub fn update(&self) -> Option<&Result<UpdateInfo, String>> {
        self.update.as_ref()
    }

    /// Windows + Advanced. `opts` is the stored [`AppOptions`]; save the
    /// config when it changes.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx, opts: &mut AppOptions) -> Option<Action> {
        self.windows(ui, c, opts);
        ui.add_space(14.0);
        self.advanced(ui, c, opts)
    }

    /// Settings → Windows.
    pub fn windows(&mut self, ui: &mut egui::Ui, c: &Ctx, opts: &mut AppOptions) {
        let pal = c.pal;
        widgets::label(ui, pal, "WINDOW");
        widgets::switch_row(
            ui,
            &mut opts.save_window_size,
            "Remember window size and position",
            "Reopen RustCord where you left it.",
            true,
            pal,
        );
        widgets::switch_row(
            ui,
            &mut opts.start_maximized,
            "Start maximised",
            "Open RustCord filling the screen.",
            true,
            pal,
        );
        widgets::label(ui, pal, "SYSTEM STARTUP BEHAVIOUR");
        widgets::switch_row(
            ui,
            &mut opts.open_on_startup,
            "Open RustCord",
            "Start RustCord when you log in to your computer.",
            true,
            pal,
        );
        widgets::switch_row(
            ui,
            &mut opts.start_minimized,
            "Start minimised",
            "When opened on startup, RustCord starts in the background.",
            opts.open_on_startup,
            pal,
        );
        widgets::switch_row(
            ui,
            &mut opts.minimize_to_tray,
            "Minimise to tray",
            "Hitting X keeps RustCord running in the system tray. Not working yet: \
             RustCord doesn't have a tray icon so far; the setting is saved for when it does.",
            true,
            pal,
        );
    }

    /// Settings → Advanced.
    pub fn advanced(
        &mut self,
        ui: &mut egui::Ui,
        c: &Ctx,
        opts: &mut AppOptions,
    ) -> Option<Action> {
        let pal = c.pal;
        let mut action = None;
        widgets::label(ui, pal, "UPDATES");
        widgets::switch_row(
            ui,
            &mut opts.check_updates,
            "Check for updates on startup",
            "Asks GitHub (without your Discord token) whether a newer RustCord release \
             is out.",
            true,
            pal,
        );
        ui.horizontal(|ui| {
            let b = ui.add_enabled(!self.checking, egui::Button::new("Check now"));
            if b.clicked() {
                self.checking = true;
                self.update = None;
                action = Some(Action::Request(Request::CheckUpdates));
            }
            if self.checking {
                ui.spinner();
            }
            let current = env!("CARGO_PKG_VERSION");
            match &self.update {
                Some(Ok(u)) if u.newer => {
                    ui.label(
                        RichText::new(format!("RustCord {} is available.", u.latest))
                            .color(pal.online),
                    );
                    if ui.link("Open release page").clicked() {
                        action = Some(Action::OpenLink(u.url.clone()));
                    }
                }
                Some(Ok(_)) => {
                    widgets::hint(ui, pal, &format!("You're up to date (RustCord {current})."));
                }
                Some(Err(e)) => widgets::error(ui, pal, e),
                None => {}
            }
        });
        ui.add_space(14.0);

        widgets::label(ui, pal, "CONNECTION");
        let mut tls = opts.verify_tls;
        if widgets::switch_row(
            ui,
            &mut tls,
            "Verify TLS certificates",
            "Keep this on. Turning it off lets anyone between you and Discord read your \
             traffic and token. Takes effect the next time you log in.",
            true,
            pal,
        ) {
            if tls {
                opts.verify_tls = true;
            } else {
                self.pending = Some(Pending::DisableTls);
            }
        }

        widgets::label(ui, pal, "DISCORD API URL");
        let api = self
            .api_text
            .get_or_insert_with(|| opts.api_base().to_string());
        ui.add(egui::TextEdit::singleline(api).desired_width(f32::INFINITY));
        widgets::label(ui, pal, "DISCORD CDN URL");
        let cdn = self
            .cdn_text
            .get_or_insert_with(|| opts.cdn_base().to_string());
        ui.add(egui::TextEdit::singleline(cdn).desired_width(f32::INFINITY));
        widgets::hint(
            ui,
            pal,
            "For self-hosted Discord-compatible servers or proxies. Changing these logs you \
             out, and your Discord token is not sent to the new address.",
        );
        ui.horizontal(|ui| {
            if ui.add(pal.primary_button("Apply")).clicked() {
                let api = validate_base_url(self.api_text.as_deref().unwrap_or(""));
                let cdn = validate_base_url(self.cdn_text.as_deref().unwrap_or(""));
                match (api, cdn) {
                    (Ok(api), Ok(cdn)) => {
                        if api == opts.api_base() && cdn == opts.cdn_base() {
                            self.url_error = Some("Nothing changed.".into());
                        } else {
                            self.url_error = None;
                            self.pending = Some(Pending::Urls { api, cdn });
                        }
                    }
                    (Err(e), _) => self.url_error = Some(format!("API URL: {e}")),
                    (_, Err(e)) => self.url_error = Some(format!("CDN URL: {e}")),
                }
            }
            let custom = opts.api_base.is_some() || opts.cdn_base.is_some();
            if ui
                .add_enabled(custom, egui::Button::new("Revert to official"))
                .clicked()
            {
                self.url_error = None;
                self.pending = Some(Pending::Official);
            }
        });
        if let Some(e) = &self.url_error {
            widgets::error(ui, pal, e);
        }

        let (title, body, yes) = match &self.pending {
            None => return action,
            Some(Pending::DisableTls) => (
                "Turn off certificate checks?",
                "You are about to turn off TLS certificate verification. This exposes you to \
                 man-in-the-middle attacks: anyone on your network path could read your \
                 messages and steal your token. Prefer installing the needed certificate \
                 in your system instead.\n\nYou can turn it back on at any time.",
                "Turn off",
            ),
            Some(Pending::Urls { .. }) => (
                "Use a different backend?",
                "You'll be logged out. Are you sure you want to route RustCord's traffic \
                 through a service that may not be Discord? Only do this if you trust it.",
                "Log out and apply",
            ),
            Some(Pending::Official) => (
                "Back to Discord?",
                "You'll be logged out and RustCord will use Discord's official servers again.",
                "Log out and revert",
            ),
        };
        match widgets::confirm_modal(ui.ctx(), "options", pal, title, body, yes, true) {
            Some(Confirm::Yes) => match self.pending.take() {
                Some(Pending::DisableTls) => opts.verify_tls = false,
                Some(Pending::Urls { api, cdn }) => {
                    opts.api_base = (api != OFFICIAL_API).then_some(api);
                    opts.cdn_base = (cdn != OFFICIAL_CDN).then_some(cdn);
                    self.api_text = None;
                    self.cdn_text = None;
                    action = Some(Action::LogOutForNewBackend);
                }
                Some(Pending::Official) => {
                    opts.api_base = None;
                    opts.cdn_base = None;
                    self.api_text = None;
                    self.cdn_text = None;
                    action = Some(Action::LogOutForNewBackend);
                }
                None => {}
            },
            Some(Confirm::No) => self.pending = None,
            None => {}
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions() {
        let v = parse_version("v1.2.3").unwrap();
        assert_eq!(v.core, [1, 2, 3]);
        assert_eq!(parse_version("1.2").unwrap().core, [1, 2, 0]);
        assert_eq!(
            parse_version("V0.1.0-beta.2+abc").unwrap().pre,
            ["beta", "2"]
        );
        for bad in [
            "", "v", "1..2", "1.2.3.4", "x1.0", "1.0-", "1.0-a..b", "1.a",
        ] {
            assert!(parse_version(bad).is_none(), "{bad}");
        }
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.10.0", "0.9.0"), "numeric, not string, compare");
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        // Pre-releases (semver precedence).
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(is_newer("1.0.0-beta.11", "1.0.0-beta.2"));
        assert!(is_newer("1.0.0-beta", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(is_newer("1.0.0-alpha.beta", "1.0.0-alpha.1"));
        assert!(!is_newer("1.0.0+build.5", "1.0.0"));
    }

    #[test]
    fn releases() {
        let r = parse_release(
            &json!({ "tag_name": "v9.0.0", "html_url": "https://github.com/KeoLogs/rustcord/releases/tag/v9.0.0" }),
            "0.1.0",
        )
        .unwrap();
        assert!(r.newer);
        assert!(r.url.ends_with("/v9.0.0"));
        let r = parse_release(
            &json!({ "tag_name": "v0.0.1", "html_url": "https://evil.example" }),
            "0.1.0",
        )
        .unwrap();
        assert!(!r.newer);
        assert_eq!(r.url, RELEASES_PAGE);
        assert!(parse_release(&json!({ "message": "Not Found" }), "0.1.0").is_err());
        assert!(parse_release(&json!({ "tag_name": "nightly" }), "0.1.0").is_err());
    }

    #[test]
    fn urls() {
        assert_eq!(
            validate_base_url(" https://example.com/api/v10/ ").unwrap(),
            "https://example.com/api/v10"
        );
        assert_eq!(
            validate_base_url("http://localhost:8080/api").unwrap(),
            "http://localhost:8080/api"
        );
        assert!(validate_base_url("http://example.com").is_err());
        assert!(validate_base_url("ftp://example.com").is_err());
        assert!(validate_base_url("example.com").is_err());
        assert!(validate_base_url("https://").is_err());
        assert!(validate_base_url("https://a.com/?x=1").is_err());
        assert!(validate_base_url("https://user@a.com").is_err());
        assert!(validate_base_url("").is_err());
        assert!(validate_base_url(OFFICIAL_API).is_ok());
    }

    #[test]
    fn options_defaults_and_serde() {
        let o: AppOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, AppOptions::default());
        assert!(o.verify_tls && o.save_window_size && !o.check_updates);
        assert_eq!(o.api_base(), OFFICIAL_API);
        let o: AppOptions =
            serde_json::from_str(r#"{"api_base":"https://x.y/api","unknown":1}"#).unwrap();
        assert_eq!(o.api_base(), "https://x.y/api");
        assert_eq!(o.cdn_base(), OFFICIAL_CDN);
    }

    #[test]
    fn autostart_entry() {
        let e = linux_autostart_entry("/opt/rust cord/rustcord", true);
        assert!(e.contains("Exec=\"/opt/rust cord/rustcord\" --minimized\n"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(!linux_autostart_entry("/x", false).contains("--minimized"));
    }
}
