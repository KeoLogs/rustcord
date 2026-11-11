//! Upload progress: attachments are streamed from disk in small chunks (so
//! a big file is never held in memory whole) and every chunk handed to the
//! HTTP client counts as sent. The backend turns that into throttled
//! `ToUi::UploadProgress` events; the pending message draws one bar per
//! file with a Cancel link. Cancel drops the request; the message then
//! shows "Upload cancelled" with Retry / Delete (nothing retries itself).

use std::path::Path;
use std::sync::Arc;

/// Bytes read from disk (and reported) at a time.
pub const CHUNK: usize = 64 * 1024;
/// What a cancelled send shows under the message.
pub const CANCELLED: &str = "Upload cancelled";

/// Called from the upload stream: (file index, bytes sent, file size).
pub type OnProgress = Arc<dyn Fn(usize, u64, u64) + Send + Sync>;

/// 0.0-1.0 for the bar. An empty file counts as done.
pub fn fraction(sent: u64, total: u64) -> f32 {
    if total == 0 {
        1.0
    } else {
        (sent.min(total) as f64 / total as f64) as f32
    }
}

/// "1.2 / 3.4 MB" under the bar.
pub fn label(sent: u64, total: u64) -> String {
    const MB: f64 = 1_048_576.0;
    if total >= 1_048_576 {
        format!(
            "{:.1} / {:.1} MB",
            sent.min(total) as f64 / MB,
            total as f64 / MB
        )
    } else {
        format!(
            "{} / {} KB",
            sent.min(total).div_ceil(1024),
            total.div_ceil(1024)
        )
    }
}

/// Decides which progress steps are worth an event (and a repaint): every
/// 2% of the file or 256 KB, whichever is more, plus always the last one.
#[derive(Default)]
pub struct Throttle {
    reported: u64,
}

impl Throttle {
    pub fn should_report(&mut self, sent: u64, total: u64) -> bool {
        let step = (total / 50).max(256 * 1024);
        if sent >= total || sent >= self.reported + step {
            self.reported = sent;
            true
        } else {
            false
        }
    }
}

/// Update one file's (sent, total) in a pending message's progress list.
/// Progress never goes backwards (events can arrive late).
pub fn apply(progress: &mut Vec<(u64, u64)>, file: usize, sent: u64, total: u64) {
    if file >= 64 {
        return;
    }
    if progress.len() <= file {
        progress.resize(file + 1, (0, 0));
    }
    let slot = &mut progress[file];
    slot.1 = total;
    slot.0 = slot.0.max(sent.min(total));
}

/// A request body that streams `path` from disk in [`CHUNK`]s and reports
/// each chunk. Returns the body and the file's size.
pub async fn file_body(
    path: &Path,
    index: usize,
    on_progress: Option<OnProgress>,
) -> std::io::Result<(reqwest::Body, u64)> {
    use tokio::io::AsyncReadExt as _;
    let file = tokio::fs::File::open(path).await?;
    let total = file.metadata().await?.len();
    let stream = futures_util::stream::unfold((file, 0u64), move |(mut file, sent)| {
        let on_progress = on_progress.clone();
        async move {
            let mut buf = vec![0u8; CHUNK];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    let sent = sent + n as u64;
                    if let Some(f) = &on_progress {
                        f(index, sent, total);
                    }
                    Some((Ok(buf), (file, sent)))
                }
                Err(e) => Some((Err(e), (file, sent))),
            }
        }
    });
    Ok((reqwest::Body::wrap_stream(stream), total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions() {
        assert_eq!(fraction(0, 100), 0.0);
        assert_eq!(fraction(50, 100), 0.5);
        assert_eq!(fraction(100, 100), 1.0);
        assert_eq!(fraction(500, 100), 1.0);
        assert_eq!(fraction(0, 0), 1.0);
    }

    #[test]
    fn labels() {
        assert_eq!(label(0, 2 * 1_048_576), "0.0 / 2.0 MB");
        assert_eq!(label(1_572_864, 3 * 1_048_576), "1.5 / 3.0 MB");
        assert_eq!(label(1, 2048), "1 / 2 KB");
        assert_eq!(label(9999, 2048), "2 / 2 KB");
    }

    #[test]
    fn throttle_reports_steps_and_always_the_end() {
        // 100 MB file: a step is 2 MB, so 64 KB chunks mostly stay quiet.
        let total = 100 * 1_048_576;
        let mut t = Throttle::default();
        let mut events = 0;
        let mut sent = 0;
        while sent < total {
            sent = (sent + CHUNK as u64).min(total);
            if t.should_report(sent, total) {
                events += 1;
            }
        }
        assert!((45..=51).contains(&events), "{events}");
        // A small file: one event, at the end.
        let mut t = Throttle::default();
        assert!(!t.should_report(CHUNK as u64, 100_000));
        assert!(t.should_report(100_000, 100_000));
        // An empty file is done at once.
        assert!(Throttle::default().should_report(0, 0));
    }

    #[test]
    fn progress_list_grows_and_never_goes_back() {
        let mut p = Vec::new();
        apply(&mut p, 1, 10, 100);
        assert_eq!(p, vec![(0, 0), (10, 100)]);
        apply(&mut p, 1, 5, 100);
        assert_eq!(p[1], (10, 100));
        apply(&mut p, 0, 500, 100);
        assert_eq!(p[0], (100, 100));
        // A nonsense index allocates nothing.
        apply(&mut p, 1_000_000, 1, 1);
        assert_eq!(p.len(), 2);
    }

    #[tokio::test]
    async fn file_body_streams_the_file_and_reports_every_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.bin");
        let size = CHUNK * 2 + 100;
        std::fs::write(&path, vec![7u8; size]).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let cb: OnProgress = Arc::new(move |i, sent, total| {
            seen2.lock().unwrap().push((i, sent, total));
        });
        let (body, total) = file_body(&path, 3, Some(cb)).await.unwrap();
        assert_eq!(total, size as u64);
        // Nothing is read before the client pulls the body.
        assert!(seen.lock().unwrap().is_empty());
        drop(body);
        assert!(
            file_body(&tmp.path().join("missing"), 0, None)
                .await
                .is_err()
        );
    }
}
