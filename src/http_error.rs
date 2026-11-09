//! Typed REST failures and the text the user sees for them.
//!
//! `Api::check` turns every non-2xx answer into an [`HttpError`] that keeps
//! the status, Discord's JSON error code and message. Code that needs to
//! react to a status (threads' 403/404, no-access channels) asks
//! [`http_status`] / [`is_unavailable`] instead of matching error text, and
//! the `Display` text is already user-facing: raw strings such as
//! "HTTP 403 Forbidden - Missing Access" never reach the UI.

use std::fmt;

use serde_json::Value;

/// Discord JSON error codes we word specially.
pub const CODE_MISSING_ACCESS: u64 = 50001;
pub const CODE_CANNOT_DM: u64 = 50007;
pub const CODE_MISSING_PERMISSIONS: u64 = 50013;

/// Longest Discord message shown as-is (anything longer is a dump).
const MAX_SERVER_MESSAGE: usize = 160;

/// A Discord REST call answered with a non-success status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    /// What was being done, e.g. "Loading messages".
    pub what: String,
    /// HTTP status code.
    pub status: u16,
    /// Discord's JSON error `code`, if any.
    pub code: Option<u64>,
    /// Discord's human-readable `message`, if any.
    pub message: Option<String>,
    /// The answer asks for a captcha (`captcha_key` in the body).
    pub captcha: bool,
}

impl HttpError {
    /// Build from a status and the raw response body (JSON or not).
    pub fn from_body(what: &str, status: u16, body: &str) -> Self {
        let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        Self {
            what: what.to_string(),
            status,
            code: json.get("code").and_then(Value::as_u64),
            message: json
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            captcha: json.get("captcha_key").is_some() || body.contains("captcha-required"),
        }
    }

    /// 403, or Discord's "Missing Access" / "Missing Permissions" codes.
    pub fn is_no_access(&self) -> bool {
        self.status == 403
            || matches!(
                self.code,
                Some(CODE_MISSING_ACCESS | CODE_MISSING_PERMISSIONS)
            )
    }

    /// The short, user-facing reason (no status line, no JSON).
    pub fn reason(&self) -> String {
        if self.captcha {
            return "Discord wants a captcha for this. Use the official app this time.".into();
        }
        match (self.status, self.code) {
            (_, Some(CODE_CANNOT_DM)) => "this person isn't accepting your messages".into(),
            (_, Some(CODE_MISSING_PERMISSIONS)) => "you're missing the permission for that".into(),
            (403, _) | (_, Some(CODE_MISSING_ACCESS)) => "no access".into(),
            (401, _) => "Discord didn't accept the login, log in again".into(),
            (404, _) => "not found".into(),
            (413, _) => "the file is too large".into(),
            (429, _) => "slow down, try again in a moment".into(),
            (500..=599, _) => "Discord is having trouble, try again later".into(),
            (400 | 409 | 422, _) => self
                .message
                .as_deref()
                .filter(|m| m.chars().count() <= MAX_SERVER_MESSAGE && !m.starts_with('{'))
                .map_or_else(|| "Discord refused the request".into(), str::to_string),
            (s, _) => format!("Discord refused the request ({s})"),
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.what.is_empty() {
            write!(f, "{}", capitalise(&self.reason()))
        } else {
            write!(f, "{} failed: {}", self.what, self.reason())
        }
    }
}

impl std::error::Error for HttpError {}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The `HttpError` inside an error chain, if any.
pub fn http_error(err: &anyhow::Error) -> Option<&HttpError> {
    err.chain().find_map(|e| e.downcast_ref::<HttpError>())
}

/// The HTTP status of a failed REST call, if the error came from one.
pub fn http_status(err: &anyhow::Error) -> Option<u16> {
    http_error(err).map(|e| e.status)
}

/// A 403 / 404: the endpoint or resource isn't available to this account.
pub fn is_unavailable(err: &anyhow::Error) -> bool {
    matches!(http_status(err), Some(403 | 404))
}

/// The request was refused for lack of access (403 / Missing Access).
pub fn is_no_access(err: &anyhow::Error) -> bool {
    http_error(err).is_some_and(HttpError::is_no_access)
}

/// Text for the user: the quiet "Slow down…" for rate limits, the worded
/// reason for HTTP failures, "can't reach Discord" for network failures.
pub fn user_message(err: &anyhow::Error) -> String {
    if let Some(rl) = crate::ratelimit::rate_limited(err) {
        return rl.to_string();
    }
    if let Some(h) = http_error(err) {
        return h.to_string();
    }
    if err
        .chain()
        .any(|e| e.downcast_ref::<reqwest::Error>().is_some())
    {
        // The outermost context ("Loading messages request failed") names
        // the action; the reqwest text (URLs, OS errors) doesn't help.
        let outer = err.to_string();
        let action = outer
            .strip_suffix(" request failed")
            .filter(|_| err.downcast_ref::<reqwest::Error>().is_none());
        return match action {
            Some(a) => format!("{a}: can't reach Discord, check your connection"),
            None => "Can't reach Discord, check your connection".into(),
        };
    }
    format!("{err:#}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratelimit::RateLimited;
    use std::time::Duration;

    fn err(status: u16, body: &str) -> anyhow::Error {
        HttpError::from_body("Loading messages", status, body).into()
    }

    #[test]
    fn parses_status_code_and_message() {
        let e = HttpError::from_body(
            "Loading messages",
            403,
            r#"{"message": "Missing Access", "code": 50001}"#,
        );
        assert_eq!(e.status, 403);
        assert_eq!(e.code, Some(50001));
        assert_eq!(e.message.as_deref(), Some("Missing Access"));
        assert!(e.is_no_access());
    }

    #[test]
    fn never_shows_raw_http_text() {
        let e = err(403, r#"{"message": "Missing Access", "code": 50001}"#);
        let text = user_message(&e);
        assert_eq!(text, "Loading messages failed: no access");
        assert!(!text.contains("HTTP"));
        assert!(!text.contains("403"));
        assert!(!text.contains("Forbidden"));

        let e = err(404, r#"{"message": "Unknown Channel", "code": 10003}"#);
        assert_eq!(user_message(&e), "Loading messages failed: not found");
        let e = err(502, "<html>bad gateway</html>");
        assert!(user_message(&e).contains("Discord is having trouble"));
        assert!(!user_message(&e).contains("html"));
    }

    #[test]
    fn typed_status_survives_context() {
        let e = err(404, "").context("Opening thread");
        assert_eq!(http_status(&e), Some(404));
        assert!(is_unavailable(&e));
        let e = err(403, "{}");
        assert!(is_unavailable(&e) && is_no_access(&e));
        let e = err(400, r#"{"message": "Invalid Form Body", "code": 50035}"#);
        assert!(!is_unavailable(&e));
        assert_eq!(
            user_message(&e),
            "Loading messages failed: Invalid Form Body"
        );
        // Text that merely mentions 403 is not a typed status.
        assert_eq!(http_status(&anyhow::anyhow!("HTTP 403")), None);
    }

    #[test]
    fn worded_reasons() {
        let r = |s, b| HttpError::from_body("", s, b).to_string();
        assert_eq!(
            r(403, r#"{"code": 50013}"#),
            "You're missing the permission for that"
        );
        assert_eq!(
            r(
                400,
                r#"{"code": 50007, "message": "Cannot send messages to this user"}"#
            ),
            "This person isn't accepting your messages"
        );
        assert!(r(400, r#"{"captcha_key": ["captcha-required"]}"#).contains("captcha"));
        assert_eq!(
            r(401, "{}"),
            "Discord didn't accept the login, log in again"
        );
        assert_eq!(r(413, ""), "The file is too large");
        assert_eq!(r(418, ""), "Discord refused the request (418)");
        // A long or JSON-looking message isn't shown verbatim.
        let long = format!(r#"{{"message": "{}"}}"#, "x".repeat(300));
        assert_eq!(r(400, &long), "Discord refused the request");
    }

    #[test]
    fn rate_limits_keep_their_wording() {
        let e: anyhow::Error = RateLimited {
            retry_after: Duration::from_secs(3),
            global: false,
        }
        .into();
        assert_eq!(user_message(&e), "Slow down, try again in 3s");
    }

    #[test]
    fn other_errors_pass_through() {
        let e = anyhow::anyhow!("bad file").context("Uploading");
        assert_eq!(user_message(&e), "Uploading: bad file");
    }
}
