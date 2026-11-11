//! Update check: at most once a day, ask GitHub's public releases API
//! whether a newer RustCord exists and show a small notice. Nothing is ever
//! downloaded or installed; "Open release page" opens the browser.
//!
//! The request is made by the backend with its own client: no Discord
//! token, no cookies, only GitHub. Settings → About turns it off.

use serde_json::Value;

/// The only URL the check ever requests.
pub const API_URL: &str = "https://api.github.com/repos/KeoLogs/rustcord/releases/latest";
/// Where "Open release page" goes (fixed: never a URL from the response).
pub const RELEASES_PAGE: &str = "https://github.com/KeoLogs/rustcord/releases/latest";
/// Seconds between checks.
pub const INTERVAL: u64 = 24 * 60 * 60;

/// `1.2.3`, `v1.2.3`, `1.2` or `v1.2.3-beta+5` as numbers (missing parts
/// are 0). `None` for anything that isn't a version.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Is `latest` a newer version than `current`? False when either can't be
/// read, so a strange tag never shows a notice.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Should a check run now? Only when it's on and the last one was at least
/// a day ago (or the clock went backwards).
pub fn due(enabled: bool, last_check: u64, now: u64) -> bool {
    enabled && (now < last_check || now - last_check >= INTERVAL)
}

/// The version tag of a `releases/latest` response. Drafts, pre-releases
/// and tags that aren't plain versions are ignored.
pub fn release_tag(v: &Value) -> Option<String> {
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = v["tag_name"].as_str()?.trim();
    (tag.len() <= 32 && parse_version(tag).is_some()).then(|| tag.to_owned())
}

/// The notice to show: the newer version, unless the user dismissed it or
/// turned the check off.
pub fn notice<'a>(
    enabled: bool,
    latest: Option<&'a str>,
    dismissed: Option<&str>,
    current: &str,
) -> Option<&'a str> {
    let latest = latest?;
    (enabled && is_newer(latest, current) && dismissed != Some(latest)).then_some(latest)
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Ask GitHub for the latest release tag (backend only). `None` on any
/// failure: an update check never shows an error.
pub async fn fetch_latest() -> Option<String> {
    let http = reqwest::Client::builder()
        .user_agent("RustCord")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let resp = http
        .get(API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    release_tag(&resp.json::<Value>().await.ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version(" V2.0 "), Some((2, 0, 0)));
        assert_eq!(parse_version("3"), Some((3, 0, 0)));
        assert_eq!(parse_version("1.2.3-beta.1+build5"), Some((1, 2, 3)));
        for bad in ["", "v", "latest", "1.x.0", "1.2.3.4", "-1.0.0", "1..2"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn compares_numbers_not_text() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v0.10.1", "0.10.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.11", "0.10.5"));
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(!is_newer("v0.9.0", "0.10.0"));
        assert!(!is_newer("0.2.0", "0.10.0"));
        // Unreadable tags never count as newer.
        assert!(!is_newer("nightly", "0.10.0"));
        assert!(!is_newer("9.9.9", "dev"));
    }

    #[test]
    fn at_most_once_a_day_and_only_when_on() {
        assert!(due(true, 0, 1_000_000));
        assert!(!due(true, 1_000_000, 1_000_000 + INTERVAL - 1));
        assert!(due(true, 1_000_000, 1_000_000 + INTERVAL));
        // Clock set back: check again rather than never.
        assert!(due(true, 2_000_000, 1_000_000));
        assert!(!due(false, 0, 1_000_000));
    }

    #[test]
    fn reads_the_release_tag() {
        let ok = json!({"tag_name": "v0.11.0", "draft": false, "prerelease": false,
                        "html_url": "https://evil.example/"});
        assert_eq!(release_tag(&ok).as_deref(), Some("v0.11.0"));
        assert_eq!(
            release_tag(&json!({"tag_name": "0.11.0"})).as_deref(),
            Some("0.11.0")
        );
        for bad in [
            json!({"tag_name": "v0.11.0", "prerelease": true}),
            json!({"tag_name": "v0.11.0", "draft": true}),
            json!({"tag_name": "nightly"}),
            json!({"tag_name": 5}),
            json!({"message": "Not Found"}),
            json!(null),
        ] {
            assert_eq!(release_tag(&bad), None, "{bad}");
        }
    }

    #[test]
    fn notice_only_for_a_newer_undismissed_version() {
        let n = |latest, dismissed| notice(true, latest, dismissed, "0.10.0");
        assert_eq!(n(Some("v0.11.0"), None), Some("v0.11.0"));
        assert_eq!(n(Some("v0.11.0"), Some("v0.10.5")), Some("v0.11.0"));
        assert_eq!(n(Some("v0.11.0"), Some("v0.11.0")), None);
        assert_eq!(n(Some("v0.10.0"), None), None);
        assert_eq!(n(None, None), None);
        assert_eq!(notice(false, Some("v0.11.0"), None, "0.10.0"), None);
    }

    #[test]
    fn only_ever_talks_to_github() {
        assert!(API_URL.starts_with("https://api.github.com/repos/KeoLogs/rustcord/"));
        assert!(RELEASES_PAGE.starts_with("https://github.com/KeoLogs/rustcord/"));
    }
}
