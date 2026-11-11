//! Link safety: trusted domains and the "Leaving RustCord" warning.
//!
//! Ported from Discord Messenger (`LocalSettings::CheckTrustedDomain`,
//! `MessageList::ConfirmOpenLink`). Links to Discord's own domains open
//! straight away; anything else asks first, with a "trust this domain"
//! checkbox. Unlike Discord Messenger, trusting a domain also trusts its
//! subdomains (`discord.com` covers `canary.discord.com`).
//!
//! Host parsing is deliberately strict and fails closed: anything odd
//! (userinfo tricks, percent-encoded hosts, non-web schemes) is treated as
//! untrusted, so the worst case is an extra prompt.
//!
//! Nothing here opens a link. [`LinkWarning::show`] returns the user's
//! decision and the caller opens it with `ctx.open_url`.

use eframe::egui::{self, Color32, RichText};

use crate::theme::Palette;

/// Domains trusted out of the box (and all their subdomains).
pub const DEFAULT_TRUSTED: &[&str] = &[
    "discord.com",
    "discord.gg",
    "discordapp.com",
    "cdn.discordapp.com",
    "media.discordapp.net",
];

/// Longest URL shown in the warning dialog.
const MAX_SHOWN_URL: usize = 300;

/// The scheme of a URL, lowercased ("https"), if it has a valid one.
pub fn scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.trim().split_once(':')?;
    let mut chars = scheme.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (first_ok && rest_ok).then(|| scheme.to_ascii_lowercase())
}

/// True for `http://` and `https://` links.
pub fn is_web_url(url: &str) -> bool {
    matches!(scheme(url).as_deref(), Some("http" | "https")) && host(url).is_some()
}

/// The host of an http(s) URL, lowercased, without port, userinfo or
/// trailing dot. `None` if the URL isn't http(s) or the host looks wrong.
///
/// Follows what browsers do for the tricky cases: `\` ends the host like
/// `/`, and everything before the last `@` is userinfo
/// (`https://discord.com@evil.example` goes to `evil.example`).
pub fn host(url: &str) -> Option<String> {
    let url = url.trim();
    let scheme = scheme(url)?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = &url[scheme.len() + 1..];
    // Browsers accept any number of slashes / backslashes after the scheme.
    let rest = rest.trim_start_matches(['/', '\\']);
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    let host_port = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        // IPv6 literal: [::1]:8080
        let end = v6.find(']')?;
        &host_port[..end + 2]
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let valid = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'))
        && !host.starts_with('.')
        && !host.contains("..");
    valid.then_some(host)
}

/// Turn a user-entered domain ("*.Example.com.", "https://x.org/") into the
/// form stored in the trusted list ("example.com"). `None` if it's junk.
pub fn normalize_domain(domain: &str) -> Option<String> {
    let d = domain.trim();
    let d = if d.contains("://") {
        host(d)?
    } else {
        d.trim_start_matches("*.")
            .trim_start_matches('.')
            .trim_end_matches('/')
            .trim_end_matches('.')
            .to_ascii_lowercase()
    };
    let valid = !d.is_empty()
        && d.contains('.')
        && !d.contains("..")
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then_some(d)
}

/// True if `host` is `domain` or one of its subdomains.
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// Can this link open without asking? True only for http(s) links whose host
/// is (a subdomain of) a default trusted domain or one in `extra_trusted`.
pub fn is_trusted(url: &str, extra_trusted: &[String]) -> bool {
    let Some(host) = host(url) else {
        return false;
    };
    DEFAULT_TRUSTED.iter().any(|d| host_matches(&host, d))
        || extra_trusted
            .iter()
            .filter_map(|d| normalize_domain(d))
            .any(|d| host_matches(&host, &d))
}

/// The domain offered by the "Trust … from now on" checkbox: the host
/// without a leading `www.`.
pub fn trust_domain_for(url: &str) -> Option<String> {
    let host = host(url)?;
    // Trusting an IP address or a bare name isn't something to offer.
    if host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok() || !host.contains('.') {
        return None;
    }
    let d = host.strip_prefix("www.").unwrap_or(&host);
    normalize_domain(d)
}

/// What to do with a clicked link.
#[derive(Debug)]
pub enum LinkCheck {
    /// Trusted (or warnings are off): open it now.
    Open(String),
    /// Ask first: keep this in the UI state and call [`LinkWarning::show`]
    /// each frame until it returns a decision.
    Ask(LinkWarning),
}

/// Decide whether a link needs the warning. `warn` is
/// `Config::warn_before_links`. Non-web links (e.g. `steam://`) always ask.
pub fn check(url: &str, warn: bool, extra_trusted: &[String]) -> LinkCheck {
    let web = is_web_url(url);
    if web && (!warn || is_trusted(url, extra_trusted)) {
        LinkCheck::Open(url.trim().to_string())
    } else {
        LinkCheck::Ask(LinkWarning::new(url))
    }
}

/// The user's answer to the link warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkDecision {
    /// Open `url`. If `trust` is set, add that domain to the trusted list.
    Visit {
        url: String,
        trust: Option<String>,
    },
    GoBack,
}

/// State of the "Leaving RustCord" dialog.
#[derive(Debug, Clone)]
pub struct LinkWarning {
    url: String,
    /// Host shown in the title; None for non-web links.
    host: Option<String>,
    /// Domain the checkbox would trust (None: no checkbox).
    domain: Option<String>,
    trust: bool,
}

impl LinkWarning {
    pub fn new(url: &str) -> Self {
        let url = url.trim().to_string();
        Self {
            host: host(&url),
            domain: trust_domain_for(&url),
            url,
            trust: false,
        }
    }

    /// Draw the modal. Returns `Some` once the user decides (Escape or
    /// clicking outside = go back). Opens nothing itself.
    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> Option<LinkDecision> {
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::unique("rustcord_link_warning")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading("Leaving RustCord");
            ui.add_space(4.0);
            match &self.host {
                Some(h) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label("This link goes to ");
                        ui.label(RichText::new(h).strong());
                        ui.label(". Are you sure you want to go there?");
                    });
                }
                None => {
                    ui.label(
                        RichText::new(
                            "This link isn't a web page. It may open another program \
                             on your computer.",
                        )
                        .color(pal.idle),
                    );
                }
            }
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(pal.input)
                .corner_radius(4)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(shorten(&self.url, MAX_SHOWN_URL))
                                .monospace()
                                .color(pal.muted),
                        )
                        .wrap(),
                    );
                });
            if let Some(domain) = &self.domain {
                ui.add_space(6.0);
                ui.checkbox(&mut self.trust, format!("Trust {domain} from now on"));
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let visit = egui::Button::new(RichText::new("Visit site").color(Color32::WHITE))
                    .fill(pal.accent);
                if ui.add(visit).clicked() {
                    decision = Some(LinkDecision::Visit {
                        url: self.url.clone(),
                        trust: self.domain.clone().filter(|_| self.trust),
                    });
                }
                if ui.button("Go back").clicked() {
                    decision = Some(LinkDecision::GoBack);
                }
            });
        });
        if decision.is_none() && modal.should_close() {
            decision = Some(LinkDecision::GoBack);
        }
        decision
    }
}

/// At most `max` characters, with "…" in the middle if cut.
/// A `discord.com/channels/...` link: which channel (and message) to open
/// inside RustCord instead of the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLink {
    /// Server id, or `@me` for DMs.
    pub guild_id: String,
    pub channel_id: String,
    pub message_id: Option<String>,
}

/// Parse `https://(ptb.|canary.)discord(app).com/channels/<guild or @me>/
/// <channel>[/<message>]`. Strict: only Discord's own hosts, numeric ids,
/// nothing else in the path (a query or fragment is ignored).
pub fn message_link(url: &str) -> Option<MessageLink> {
    let host = host(url)?;
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let base = host
        .strip_prefix("ptb.")
        .or_else(|| host.strip_prefix("canary."))
        .unwrap_or(host);
    if base != "discord.com" && base != "discordapp.com" {
        return None;
    }
    let url = url.trim();
    let after_scheme = url.split_once("://")?.1;
    let path = after_scheme.split_once('/')?.1;
    let path = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut parts = path.split('/');
    if parts.next()? != "channels" {
        return None;
    }
    let numeric = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
    let guild_id = parts.next().filter(|g| *g == "@me" || numeric(g))?;
    let channel_id = parts.next().filter(|c| numeric(c))?;
    let message_id = match parts.next() {
        Some(m) if numeric(m) => Some(m.to_string()),
        Some(_) => return None,
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(MessageLink {
        guild_id: guild_id.to_string(),
        channel_id: channel_id.to_string(),
        message_id,
    })
}

fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().skip(n - half).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(
            host("https://discord.com/channels/1/2").unwrap(),
            "discord.com"
        );
        assert_eq!(host("HTTPS://Discord.COM:443/x").unwrap(), "discord.com");
        assert_eq!(host("http://example.org?q=1").unwrap(), "example.org");
        assert_eq!(host("https://example.org#frag").unwrap(), "example.org");
        assert_eq!(host("https://example.org.").unwrap(), "example.org");
        assert_eq!(host("  https://example.org  ").unwrap(), "example.org");
        assert_eq!(host("https://[::1]:8080/").unwrap(), "[::1]");
        assert_eq!(host("https://user:pw@example.org/").unwrap(), "example.org");
    }

    #[test]
    fn junk_hosts() {
        for junk in [
            "",
            "discord.com",
            "://discord.com",
            "https://",
            "https:///",
            "https://exa mple.org",
            "https://..",
            "https://.example.org",
            "https://exa..mple.org",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "steam://run/440",
            "https://[::1",
            "1https://x.org",
            "https://discord%2ecom/",
            "https://déjà.example/",
        ] {
            assert_eq!(host(junk), None, "{junk:?}");
            assert!(!is_trusted(junk, &[]), "{junk:?}");
        }
    }

    #[test]
    fn default_trust() {
        for ok in [
            "https://discord.com/channels/@me",
            "https://discord.gg/abc",
            "https://canary.discord.com/",
            "https://ptb.discordapp.com/",
            "https://cdn.discordapp.com/attachments/1/2/a.png",
            "https://media.discordapp.net/attachments/1/2/a.png?width=10",
            "http://discord.com",
        ] {
            assert!(is_trusted(ok, &[]), "{ok}");
        }
        for bad in [
            "https://example.org",
            "https://discord.com.evil.example/",
            "https://evildiscord.com/",
            "https://notdiscord.gg/",
            "https://discord.co/",
            "ftp://discord.com/",
        ] {
            assert!(!is_trusted(bad, &[]), "{bad}");
        }
    }

    #[test]
    fn userinfo_and_backslash_tricks() {
        // Goes to evil.example, not discord.com.
        assert!(!is_trusted("https://discord.com@evil.example/", &[]));
        assert!(!is_trusted("https://discord.com:443@evil.example/", &[]));
        // Browsers treat `\` like `/`: host is evil.example.
        assert!(!is_trusted("https://evil.example\\@discord.com/", &[]));
        assert_eq!(host("https:\\\\evil.example").unwrap(), "evil.example");
        // The real host is trusted even with userinfo.
        assert!(is_trusted("https://someone@discord.com/", &[]));
    }

    #[test]
    fn extra_trust() {
        let extra = vec!["YouTube.com".to_string(), "*.github.io".into(), " ".into()];
        assert!(is_trusted("https://www.youtube.com/watch?v=x", &extra));
        assert!(is_trusted("https://youtube.com/", &extra));
        assert!(is_trusted("https://leo.github.io/", &extra));
        assert!(!is_trusted("https://notyoutube.com/", &extra));
        assert!(!is_trusted("https://youtube.com.evil.example/", &extra));
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_domain("*.Example.com.").unwrap(), "example.com");
        assert_eq!(
            normalize_domain("https://www.x.org/a").unwrap(),
            "www.x.org"
        );
        assert_eq!(normalize_domain(".x.org/").unwrap(), "x.org");
        assert_eq!(normalize_domain(""), None);
        assert_eq!(normalize_domain("com"), None);
        assert_eq!(normalize_domain("a b.com"), None);
    }

    #[test]
    fn trust_domain_offer() {
        assert_eq!(
            trust_domain_for("https://www.youtube.com/watch").unwrap(),
            "youtube.com"
        );
        assert_eq!(trust_domain_for("https://docs.rs/egui").unwrap(), "docs.rs");
        assert_eq!(trust_domain_for("http://127.0.0.1:8000/"), None);
        assert_eq!(trust_domain_for("http://localhost/"), None);
        assert_eq!(trust_domain_for("steam://run/1"), None);
    }

    #[test]
    fn check_decides() {
        assert!(matches!(
            check("https://discord.gg/x", true, &[]),
            LinkCheck::Open(_)
        ));
        assert!(matches!(
            check("https://example.org", true, &[]),
            LinkCheck::Ask(_)
        ));
        // Warnings off: web links open, other schemes still ask.
        assert!(matches!(
            check("https://example.org", false, &[]),
            LinkCheck::Open(_)
        ));
        assert!(matches!(
            check("steam://run/440", false, &[]),
            LinkCheck::Ask(_)
        ));
    }

    #[test]
    fn shorten_long_urls() {
        assert_eq!(shorten("abc", 10), "abc");
        let s = shorten(&"x".repeat(50), 10);
        assert_eq!(s.chars().count(), 11);
        assert!(s.contains('…'));
    }

    #[test]
    fn discord_message_links_are_parsed() {
        let l = message_link("https://discord.com/channels/100/206/640").unwrap();
        assert_eq!(l.guild_id, "100");
        assert_eq!(l.channel_id, "206");
        assert_eq!(l.message_id.as_deref(), Some("640"));
        // DMs, channel-only links, other Discord hosts, query / slash.
        let dm = message_link("https://discord.com/channels/@me/401/94").unwrap();
        assert_eq!(dm.guild_id, "@me");
        let ch = message_link("https://canary.discord.com/channels/100/206/").unwrap();
        assert_eq!(ch.message_id, None);
        assert!(message_link("https://ptb.discordapp.com/channels/1/2/3?x=1").is_some());
        assert!(message_link("http://www.discord.com/channels/1/2/3#top").is_some());
    }

    #[test]
    fn other_links_are_not_message_links() {
        for url in [
            "https://discord.com/channels/100",
            "https://discord.com/channels/100/abc/1",
            "https://discord.com/channels/100/206/640/extra",
            "https://discord.com/invite/abc",
            "https://evil.example/channels/100/206/640",
            "https://discord.com.evil.example/channels/1/2/3",
            "https://discord.com@evil.example/channels/1/2/3",
            "discord.com/channels/1/2/3",
            "https://discord.gg/channels/1/2/3",
        ] {
            assert_eq!(message_link(url), None, "{url}");
        }
    }
}
