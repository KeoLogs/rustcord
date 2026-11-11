//! Developer options: a custom API / CDN base URL (for a proxy or a test
//! server you run yourself). Empty = Discord's own.
//!
//! The API base is read when a session starts (`backend::Options`); every
//! REST request is rewritten in `Api::send`. The CDN base is applied where
//! images are fetched (`image_cache::Loader`, `Api::download`). The gateway
//! is not affected, and there is no way to turn TLS checks off.

use std::borrow::Cow;
use std::sync::RwLock;

pub const DEFAULT_API: &str = "https://discord.com/api/v10";
pub const DEFAULT_CDN: &str = "https://cdn.discordapp.com";
/// Longest base URL accepted.
const MAX_LEN: usize = 200;

/// The CDN base in use (`None` = Discord's).
static CDN: RwLock<Option<String>> = RwLock::new(None);

/// Check a base URL typed in Settings. `Ok(None)` = empty (use Discord's).
/// It must be `https://…`; plain `http://` is only allowed for this PC
/// (localhost). The error is shown under the field.
pub fn normalize(input: &str) -> Result<Option<String>, String> {
    let url = input.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(None);
    }
    if url.len() > MAX_LEN {
        return Err("That address is too long.".into());
    }
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '@' | '\\'))
    {
        return Err("Use a plain address like https://example.com/api/v10.".into());
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("The address must start with https://")?;
    let authority = rest.split('/').next().unwrap_or("");
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                authority
            }
        })
    };
    if host.is_empty() {
        return Err("The address has no host name.".into());
    }
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    match scheme {
        "https" => {}
        "http" if local => {}
        "http" => return Err("Plain http:// is only allowed for localhost.".into()),
        _ => return Err("The address must start with https://".into()),
    }
    Ok(Some(url.to_owned()))
}

/// The saved setting as a usable base: `None` when empty or not valid (a
/// bad value never breaks logging in; it just isn't used).
pub fn usable(saved: &str) -> Option<String> {
    normalize(saved).ok().flatten()
}

/// Swap `default` at the start of `url` for `custom`. Other URLs (and
/// look-alike hosts such as `discord.com.evil.example`) are untouched.
pub fn rewrite<'a>(url: &'a str, default: &str, custom: Option<&str>) -> Cow<'a, str> {
    let Some(custom) = custom else {
        return Cow::Borrowed(url);
    };
    match url.strip_prefix(default) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '?']) => {
            Cow::Owned(format!("{custom}{rest}"))
        }
        _ => Cow::Borrowed(url),
    }
}

/// Apply the CDN setting (cheap when nothing changed; called each frame).
pub fn set_cdn(saved: &str) {
    let new = usable(saved);
    let same = CDN.read().map(|c| *c == new).unwrap_or(false);
    if !same && let Ok(mut c) = CDN.write() {
        *c = new;
    }
}

/// An image / download URL with the custom CDN base applied.
pub fn cdn_url(url: &str) -> Cow<'_, str> {
    match CDN.read() {
        Ok(c) => match c.as_deref() {
            Some(custom) => Cow::Owned(rewrite(url, DEFAULT_CDN, Some(custom)).into_owned()),
            None => Cow::Borrowed(url),
        },
        Err(_) => Cow::Borrowed(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_means_default() {
        assert_eq!(normalize(""), Ok(None));
        assert_eq!(normalize("   "), Ok(None));
        assert_eq!(normalize("/"), Ok(None));
        assert_eq!(usable(""), None);
    }

    #[test]
    fn accepts_https_and_local_http() {
        for (input, want) in [
            (
                "https://example.com/api/v10/",
                "https://example.com/api/v10",
            ),
            (" https://proxy.example:8443 ", "https://proxy.example:8443"),
            (
                "http://localhost:8080/api/v10",
                "http://localhost:8080/api/v10",
            ),
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000"),
            ("http://[::1]:3000/api", "http://[::1]:3000/api"),
        ] {
            assert_eq!(normalize(input), Ok(Some(want.to_owned())), "{input}");
        }
    }

    #[test]
    fn rejects_unsafe_or_broken_addresses() {
        for bad in [
            "http://example.com/api",
            "http://localhost.evil.example/api",
            "ftp://example.com",
            "example.com/api",
            "https://",
            "https:///api",
            "https://user:pass@example.com",
            "https://example.com/api?x=1",
            "https://example.com/#frag",
            "https://exa mple.com",
            "javascript:alert(1)",
        ] {
            assert!(normalize(bad).is_err(), "{bad}");
            assert_eq!(usable(bad), None);
        }
        assert!(normalize(&format!("https://{}.example", "a".repeat(300))).is_err());
    }

    #[test]
    fn rewrites_only_the_default_base() {
        let custom = Some("http://localhost:8080/api/v10");
        assert_eq!(
            rewrite(
                "https://discord.com/api/v10/channels/1/messages?limit=50",
                DEFAULT_API,
                custom
            ),
            "http://localhost:8080/api/v10/channels/1/messages?limit=50"
        );
        assert_eq!(
            rewrite(DEFAULT_API, DEFAULT_API, custom),
            "http://localhost:8080/api/v10"
        );
        // No custom base: untouched, and not even copied.
        assert!(matches!(
            rewrite("https://discord.com/api/v10/x", DEFAULT_API, None),
            Cow::Borrowed(_)
        ));
        // Other hosts and look-alikes are never rewritten.
        for other in [
            "https://discord.com/api/v9/x",
            "https://discord.com/api/v100/x",
            "https://discord.com.evil.example/api/v10/x",
            "https://api.github.com/repos/KeoLogs/rustcord/releases/latest",
        ] {
            assert_eq!(rewrite(other, DEFAULT_API, custom), other);
        }
        let cdn = Some("https://cdn.example");
        assert_eq!(
            rewrite(
                "https://cdn.discordapp.com/avatars/1/a.png?size=64",
                DEFAULT_CDN,
                cdn
            ),
            "https://cdn.example/avatars/1/a.png?size=64"
        );
        assert_eq!(
            rewrite(
                "https://cdn.discordapp.com.evil.example/a.png",
                DEFAULT_CDN,
                cdn
            ),
            "https://cdn.discordapp.com.evil.example/a.png"
        );
        assert_eq!(
            rewrite(
                "https://media.discordapp.net/attachments/1/2/a.png",
                DEFAULT_CDN,
                cdn
            ),
            "https://media.discordapp.net/attachments/1/2/a.png"
        );
    }
}
