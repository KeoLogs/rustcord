//! Local crash log. A panic writes a short report (version, where, the
//! message, a backtrace) to `<data dir>/crash/`, with the token redacted.
//! On the next start RustCord offers "Copy report" / "Open folder".
//!
//! Nothing is ever uploaded: the report only leaves the PC if the user
//! copies it somewhere. At most [`KEEP`] reports are kept.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Reports kept on disk (oldest deleted first).
pub const KEEP: usize = 5;
/// Longest report written.
const MAX_REPORT: usize = 64 * 1024;
/// What replaces a token.
pub const REDACTED: &str = "[token redacted]";
/// Name of the marker holding the newest report already offered.
const SEEN: &str = "last-seen";

/// Secrets to cut out of any report, exactly as they are (the session
/// tokens; see [`register_secret`]). Never logged or printed.
static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The report from last time that hasn't been offered yet.
static PENDING: Mutex<Option<PathBuf>> = Mutex::new(None);
static HAS_PENDING: AtomicBool = AtomicBool::new(false);
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remember a token so a crash report can never contain it, whatever form
/// the panic message has. Called when a session starts.
pub fn register_secret(secret: &str) {
    let secret = secret.trim();
    let bare = secret.strip_prefix("Bot ").unwrap_or(secret).trim();
    if bare.len() < 8 {
        return;
    }
    let mut list = lock(&SECRETS);
    if !list.iter().any(|s| s == bare) {
        // A handful of accounts at most; drop the oldest beyond that.
        if list.len() >= 16 {
            list.remove(0);
        }
        list.push(bare.to_owned());
    }
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Does this word look like a Discord token? `id.timestamp.hmac` (three
/// base64url parts) or the old `mfa.` form.
fn looks_like_token(word: &str) -> bool {
    if let Some(rest) = word.strip_prefix("mfa.") {
        return rest.len() >= 20 && rest.chars().all(is_token_char);
    }
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() == 3
        && parts[0].len() >= 18
        && parts[1].len() >= 4
        && parts[2].len() >= 20
        && parts.iter().all(|p| p.chars().all(is_token_char))
}

/// Cut tokens out of `text`: every registered secret, then anything shaped
/// like a token (so an unregistered one is caught too).
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets {
        if secret.len() >= 8 {
            text = text.replace(secret.as_str(), REDACTED);
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut word_start = None;
    let flush = |out: &mut String, word: &str| {
        if looks_like_token(word) {
            out.push_str(REDACTED);
        } else {
            out.push_str(word);
        }
    };
    for (i, c) in text.char_indices() {
        let in_word = is_token_char(c) || c == '.';
        match (word_start, in_word) {
            (None, true) => word_start = Some(i),
            (Some(start), false) => {
                flush(&mut out, &text[start..i]);
                word_start = None;
                out.push(c);
            }
            (None, false) => out.push(c),
            (Some(_), true) => {}
        }
    }
    if let Some(start) = word_start {
        flush(&mut out, &text[start..]);
    }
    out
}

/// The report text (before redaction).
pub fn build_report(message: &str, location: &str, thread: &str, backtrace: &str) -> String {
    format!(
        "RustCord {} crash report\nOS: {} {}\nThread: {thread}\nWhere: {location}\n\n{message}\n\nBacktrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "txt")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("crash-"))
        })
        .collect();
    // crash-<unix seconds, zero padded>-<n>.txt: name order is time order.
    list.sort();
    list
}

/// Redact and write a report; returns its path. Old reports beyond
/// [`KEEP`] are deleted.
pub fn write_report(
    dir: &Path,
    report: &str,
    secrets: &[String],
    unix: u64,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut text = redact(report, secrets);
    if text.len() > MAX_REPORT {
        let mut cut = MAX_REPORT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[report cut short]\n");
    }
    let mut n = 0;
    let path = loop {
        let p = dir.join(format!("crash-{unix:012}-{n}.txt"));
        if !p.exists() {
            break p;
        }
        n += 1;
    };
    std::fs::write(&path, text)?;
    let all = reports(dir);
    for old in all.iter().take(all.len().saturating_sub(KEEP)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// The newest report, if it hasn't been offered to the user yet.
pub fn unseen(dir: &Path) -> Option<PathBuf> {
    let newest = reports(dir).pop()?;
    let seen = std::fs::read_to_string(dir.join(SEEN)).unwrap_or_default();
    (newest.file_name()?.to_str()? != seen.trim()).then_some(newest)
}

/// Remember that this report was offered (so it isn't offered again).
pub fn mark_seen(report: &Path) {
    if let (Some(dir), Some(name)) = (report.parent(), report.file_name().and_then(|n| n.to_str()))
    {
        let _ = std::fs::write(dir.join(SEEN), name);
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Install the panic hook and look for a report from last time. Call once
/// at start-up with the app's data folder.
pub fn install(data_dir: Option<PathBuf>) {
    let Some(dir) = data_dir.map(|d| d.join("crash")) else {
        return;
    };
    if let Some(report) = unseen(&dir) {
        *lock(&PENDING) = Some(report);
        HAS_PENDING.store(true, Ordering::Relaxed);
    }
    *lock(&DIR) = Some(dir.clone());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        let location = info.location().map_or_else(
            || "unknown".to_owned(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        let thread = std::thread::current();
        let report = build_report(
            &message,
            &location,
            thread.name().unwrap_or("unnamed"),
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        // `try_lock`: a panic while the list is locked must not hang here.
        let secrets = SECRETS.try_lock().map(|s| s.clone()).unwrap_or_default();
        let _ = write_report(&dir, &report, &secrets, now_unix());
        previous(info);
    }));
}

/// Developer options: write a sample report through the same path (no
/// panic), so the "closed unexpectedly" notice can be tried on the next
/// start.
pub fn write_test_report() -> Result<PathBuf, String> {
    let dir = lock(&DIR).clone().ok_or("No data folder on this system.")?;
    let report = build_report(
        "Test report from Settings → Developer options (RustCord did not crash).",
        "settings",
        "main",
        "(none)",
    );
    let secrets = lock(&SECRETS).clone();
    write_report(&dir, &report, &secrets, now_unix()).map_err(|e| e.to_string())
}

/// The report to offer, if any (cheap: one atomic load when there's none).
pub fn pending() -> Option<PathBuf> {
    if !HAS_PENDING.load(Ordering::Relaxed) {
        return None;
    }
    lock(&PENDING).clone()
}

/// The user answered the notice (any button): don't offer it again.
pub fn dismiss() {
    HAS_PENDING.store(false, Ordering::Relaxed);
    if let Some(report) = lock(&PENDING).take() {
        mark_seen(&report);
    }
}

/// The report's text for "Copy report".
pub fn read(report: &Path) -> Option<String> {
    std::fs::read_to_string(report).ok()
}

/// "Open folder": show the crash folder in the file manager (backend,
/// blocking). Only ever opens that local folder.
pub fn open_folder(report: &Path) {
    let Some(dir) = report.parent() else { return };
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up strings in the shape of tokens (not real ones), joined at
    // compile time so no token-shaped text sits in the source.
    const FAKE: &str = concat!(
        "MTIzNDU2Nzg5MDEyMzQ1Njc4",
        ".",
        "GaBcDe",
        ".",
        "abcdefghijklmnopqrstuvwxyz0123456789ab"
    );
    const FAKE_MFA: &str = concat!("mfa", ".", "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG");

    #[test]
    fn redacts_registered_secrets_exactly() {
        let secrets = vec!["hunter2-not-token-shaped".to_owned()];
        let out = redact("auth=hunter2-not-token-shaped; ok", &secrets);
        assert_eq!(out, format!("auth={REDACTED}; ok"));
        // Too-short "secrets" are ignored (they'd shred the report).
        assert_eq!(redact("abc", &["a".to_owned()]), "abc");
    }

    #[test]
    fn redacts_anything_token_shaped_without_knowing_it() {
        for text in [
            format!("Authorization: {FAKE}"),
            format!("Bot {FAKE}"),
            format!("token=\"{FAKE}\","),
            FAKE.to_string(),
            format!("x {FAKE_MFA} y"),
        ] {
            let out = redact(&text, &[]);
            assert!(!out.contains(FAKE), "{out}");
            assert!(!out.contains(FAKE_MFA), "{out}");
            assert!(!out.contains("abcdefghijklmnop"), "{out}");
            assert!(out.contains(REDACTED));
        }
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in [
            "src\\app.rs:1234 index out of bounds: the len is 3 but the index is 7",
            "called `Option::unwrap()` on a `None` value",
            "https://cdn.discordapp.com/avatars/123456789012345678/abc.png?size=64",
            "version 0.10.0 at std::panicking::begin_panic_handler",
            "core::ops::function::FnOnce::call_once{{vtable.shim}}",
            "ünïcödé — text … stays",
        ] {
            assert_eq!(redact(text, &[]), text);
        }
    }

    #[test]
    fn a_written_report_never_contains_the_token() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("crash");
        let secret = "s3cret-session-token-value".to_owned();
        let report = build_report(
            &format!("request failed: Authorization {secret} and also {FAKE}"),
            "src/api.rs:10",
            "tokio-runtime-worker",
            &format!("0: rustcord::api::send\n   token={FAKE}\n   bot=Bot {secret}"),
        );
        let path =
            write_report(&dir, &report, std::slice::from_ref(&secret), 1_700_000_000).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains(&secret));
        assert!(!written.contains(FAKE));
        assert!(written.contains(REDACTED));
        // The useful parts survive.
        assert!(written.contains("src/api.rs:10"));
        assert!(written.contains("rustcord::api::send"));
        assert!(written.contains(env!("CARGO_PKG_VERSION")));
        // Nothing else was written to the folder.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn keeps_only_the_newest_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for t in 0..(KEEP as u64 + 3) {
            write_report(dir, "r", &[], 1000 + t).unwrap();
        }
        let left = reports(dir);
        assert_eq!(left.len(), KEEP);
        assert!(left[0].to_string_lossy().contains("crash-000000001003-0"));
        // Two in the same second don't overwrite each other.
        let a = write_report(dir, "a", &[], 5000).unwrap();
        let b = write_report(dir, "b", &[], 5000).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_report_is_offered_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(unseen(dir), None);
        let first = write_report(dir, "one", &[], 100).unwrap();
        assert_eq!(unseen(dir), Some(first.clone()));
        mark_seen(&first);
        assert_eq!(unseen(dir), None);
        // A new crash is offered again.
        let second = write_report(dir, "two", &[], 200).unwrap();
        assert_eq!(unseen(dir), Some(second));
    }

    #[test]
    fn long_reports_are_cut() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_report(tmp.path(), &"é".repeat(MAX_REPORT), &[], 1).unwrap();
        let len = std::fs::metadata(path).unwrap().len() as usize;
        assert!(len <= MAX_REPORT + 32, "{len}");
    }
}
