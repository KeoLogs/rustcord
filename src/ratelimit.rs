//! Shared HTTP 429 handling for every REST call in `api.rs`.
//!
//! Discord rate limits per route bucket (learned from `X-RateLimit-Bucket`
//! plus the route's major parameter) and globally. This module keeps that
//! state; `Api::send` asks it before each request and feeds it the response
//! headers afterwards.
//!
//! Policy:
//! - GET requests may wait for a known reset (up to [`MAX_GET_WAIT`]) and,
//!   after a 429, wait for `Retry-After` and retry **once**.
//! - Everything else (user write actions) is never retried. A 429, or a
//!   bucket we already know is empty, becomes a typed [`RateLimited`] error
//!   that the UI shows as "Slow down, try again in Ns".

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Longest a GET will sleep for a rate limit before giving up.
pub const MAX_GET_WAIT: Duration = Duration::from_secs(10);
/// A write may wait this long for a bucket that is about to reset (that's
/// not a retry: nothing was sent yet). Longer waits fail fast instead.
pub const MAX_WRITE_WAIT: Duration = Duration::from_secs(1);
/// Used when a 429 carries no usable Retry-After (e.g. Cloudflare).
const FALLBACK_RETRY: Duration = Duration::from_secs(5);

/// A request was (or would be) rate limited. Never retried automatically
/// for writes; the UI waits `retry_after` before re-enabling the control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    pub retry_after: Duration,
    pub global: bool,
}

impl RateLimited {
    /// Whole seconds to show the user (at least 1).
    pub fn secs(&self) -> u64 {
        self.retry_after.as_secs_f64().ceil().max(1.0) as u64
    }
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Slow down, try again in {}s", self.secs())
    }
}

impl std::error::Error for RateLimited {}

/// The `RateLimited` inside an error chain, if any.
pub fn rate_limited(err: &anyhow::Error) -> Option<RateLimited> {
    err.chain()
        .find_map(|e| e.downcast_ref::<RateLimited>())
        .copied()
}

/// Text for the user: the quiet "Slow down…" for rate limits, worded HTTP
/// failures, else the error (see [`crate::http_error::user_message`]).
pub fn user_message(err: &anyhow::Error) -> String {
    crate::http_error::user_message(err)
}

/// Rate-limit headers of one response.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Headers {
    pub bucket: Option<String>,
    pub remaining: Option<u32>,
    pub reset_after: Option<f64>,
    pub retry_after: Option<f64>,
    pub global: bool,
}

impl Headers {
    pub fn from_map(h: &reqwest::header::HeaderMap) -> Self {
        let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
        Self {
            bucket: get("x-ratelimit-bucket").map(str::to_string),
            remaining: get("x-ratelimit-remaining").and_then(|v| v.parse().ok()),
            reset_after: get("x-ratelimit-reset-after").and_then(|v| v.parse().ok()),
            retry_after: get("retry-after").and_then(|v| v.parse().ok()),
            global: get("x-ratelimit-global").is_some_and(|v| v.eq_ignore_ascii_case("true"))
                || get("x-ratelimit-scope") == Some("global"),
        }
    }
}

/// Route key: method + path with the major parameter (channel, guild,
/// webhook) kept and every other snowflake replaced, plus that major id.
pub fn route_key(method: &str, path: &str) -> (String, String) {
    let path = path.trim_start_matches("/api/v10");
    let mut out = String::from(method);
    let mut major = String::new();
    let mut prev = "";
    for (i, seg) in path.split('/').filter(|s| !s.is_empty()).enumerate() {
        out.push('/');
        let is_id = seg.len() >= 15 && seg.bytes().all(|b| b.is_ascii_digit());
        // Only a top-level resource id is a major parameter.
        if is_id && i == 1 && matches!(prev, "channels" | "guilds" | "webhooks") {
            major = seg.to_string();
            out.push_str(seg);
        } else if is_id {
            out.push_str(":id");
        } else if prev == "reactions" {
            // Emoji in reaction routes share one bucket.
            out.push_str(":emoji");
        } else {
            out.push_str(seg);
        }
        prev = seg;
    }
    (out, major)
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    remaining: u32,
    reset_at: Instant,
}

#[derive(Default)]
struct Inner {
    /// route key -> bucket key (hash + major param), once Discord told us.
    routes: HashMap<String, String>,
    buckets: HashMap<String, Bucket>,
    global_until: Option<Instant>,
}

impl Inner {
    fn bucket_key(&self, route: &str) -> String {
        self.routes
            .get(route)
            .cloned()
            .unwrap_or_else(|| route.to_string())
    }
}

/// Per-route buckets plus the global lock. Cheap; shared by all `Api`
/// clones. The mutex is never held across an await.
#[derive(Default)]
pub struct Limiter {
    inner: Mutex<Inner>,
}

impl Limiter {
    /// How long `route` must wait before sending now, if at all.
    pub fn wait_for(&self, route: &str, now: Instant) -> Option<Duration> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let global = inner.global_until.filter(|t| *t > now).map(|t| t - now);
        let bucket = inner
            .buckets
            .get(&inner.bucket_key(route))
            .filter(|b| b.remaining == 0 && b.reset_at > now)
            .map(|b| b.reset_at - now);
        match (global, bucket) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// Record the headers of a (non-429) response.
    pub fn update(&self, route: &str, major: &str, h: &Headers, now: Instant) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = match &h.bucket {
            Some(hash) => {
                let key = format!("{hash}:{major}");
                inner.routes.insert(route.to_string(), key.clone());
                key
            }
            None => inner.bucket_key(route),
        };
        if let (Some(remaining), Some(reset)) = (h.remaining, h.reset_after) {
            inner.buckets.insert(
                key,
                Bucket {
                    remaining,
                    reset_at: now + secs(reset),
                },
            );
        }
        // Drop buckets that have long reset so the map stays small.
        if inner.buckets.len() > 256 {
            inner.buckets.retain(|_, b| b.reset_at > now);
        }
    }

    /// Record a 429. `body_retry_after`/`body_global` come from the JSON
    /// body when present (more precise than the header). Returns the error.
    pub fn on_429(
        &self,
        route: &str,
        major: &str,
        h: &Headers,
        body_retry_after: Option<f64>,
        body_global: bool,
        now: Instant,
    ) -> RateLimited {
        let retry_after = body_retry_after
            .or(h.retry_after)
            .or(h.reset_after)
            .map(secs)
            .unwrap_or(FALLBACK_RETRY);
        let global = h.global || body_global;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if global {
            inner.global_until = Some(now + retry_after);
        } else {
            let key = match &h.bucket {
                Some(hash) => {
                    let key = format!("{hash}:{major}");
                    inner.routes.insert(route.to_string(), key.clone());
                    key
                }
                None => inner.bucket_key(route),
            };
            inner.buckets.insert(
                key,
                Bucket {
                    remaining: 0,
                    reset_at: now + retry_after,
                },
            );
        }
        RateLimited {
            retry_after,
            global,
        }
    }
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s.clamp(0.0, 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "111111111111111111";
    const M: &str = "222222222222222222";

    #[test]
    fn route_keys_keep_major_param_only() {
        let (r, major) = route_key("GET", &format!("/api/v10/channels/{C}/messages/{M}"));
        assert_eq!(r, format!("GET/channels/{C}/messages/:id"));
        assert_eq!(major, C);
        let (r, _) = route_key(
            "PUT",
            &format!("/api/v10/channels/{C}/messages/{M}/reactions/%F0%9F%91%8D/@me"),
        );
        assert_eq!(
            r,
            format!("PUT/channels/{C}/messages/:id/reactions/:emoji/@me")
        );
        let (r, major) = route_key("DELETE", &format!("/api/v10/users/@me/guilds/{C}"));
        assert_eq!(r, "DELETE/users/@me/guilds/:id");
        assert_eq!(major, "");
    }

    #[test]
    fn bucket_exhausted_then_resets() {
        let l = Limiter::default();
        let now = Instant::now();
        let h = Headers {
            bucket: Some("abc".into()),
            remaining: Some(0),
            reset_after: Some(2.0),
            ..Default::default()
        };
        l.update("GET/x", "1", &h, now);
        assert_eq!(l.wait_for("GET/x", now), Some(Duration::from_secs(2)));
        assert_eq!(l.wait_for("GET/x", now + Duration::from_secs(3)), None);
        // Another route on the same hash but a different major is separate.
        l.update(
            "GET/y",
            "2",
            &Headers {
                remaining: None,
                ..h.clone()
            },
            now,
        );
        assert_eq!(l.wait_for("GET/y", now), None);
        // Remaining > 0: no wait.
        let ok = Headers {
            remaining: Some(4),
            ..h
        };
        l.update("GET/x", "1", &ok, now);
        assert_eq!(l.wait_for("GET/x", now), None);
    }

    #[test]
    fn global_429_blocks_every_route() {
        let l = Limiter::default();
        let now = Instant::now();
        let rl = l.on_429(
            "POST/a",
            "",
            &Headers {
                global: true,
                ..Default::default()
            },
            Some(1.5),
            false,
            now,
        );
        assert!(rl.global);
        assert_eq!(rl.secs(), 2);
        assert_eq!(rl.to_string(), "Slow down, try again in 2s");
        assert!(l.wait_for("GET/other", now).is_some());
        assert!(
            l.wait_for("GET/other", now + Duration::from_secs(2))
                .is_none()
        );
    }

    #[test]
    fn route_429_uses_header_then_fallback() {
        let l = Limiter::default();
        let now = Instant::now();
        let h = Headers {
            retry_after: Some(3.0),
            ..Default::default()
        };
        let rl = l.on_429("PUT/r", "", &h, None, false, now);
        assert_eq!(rl.retry_after, Duration::from_secs(3));
        assert!(!rl.global);
        assert!(l.wait_for("PUT/r", now).is_some());
        assert!(l.wait_for("PUT/other", now).is_none());
        let rl = l.on_429("PUT/s", "", &Headers::default(), None, false, now);
        assert_eq!(rl.retry_after, FALLBACK_RETRY);
    }

    #[test]
    fn typed_error_survives_context() {
        use anyhow::Context;
        let e: anyhow::Result<()> = Err(anyhow::Error::new(RateLimited {
            retry_after: Duration::from_millis(400),
            global: false,
        }));
        let e = e.context("react failed").unwrap_err();
        assert_eq!(rate_limited(&e).map(|r| r.secs()), Some(1));
        assert_eq!(user_message(&e), "Slow down, try again in 1s");
        let other = anyhow::anyhow!("boom");
        assert_eq!(user_message(&other), "boom");
    }
}
