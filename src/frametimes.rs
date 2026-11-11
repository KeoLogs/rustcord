//! Frame-time log for `scripts/memprobe.ps1`.
//!
//! Only exists when `RUSTCORD_FRAMETIMES=<path>` is set: the app then keeps
//! an `Option<FrameRecorder>` that is `Some`, pushes eframe's CPU time per
//! frame into a small buffer and appends it to the file every few seconds.
//! Without the variable the app holds `None` and does no work at all.
//!
//! File format, one frame per line: `<unix ms>,<frame ms>` (frame ms with
//! two decimals). The probe script reads it to get frame-time p95.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Frames kept between flushes (older ones are dropped if a flush fails).
const CAPACITY: usize = 2048;
const FLUSH_EVERY: Duration = Duration::from_secs(2);

pub struct FrameRecorder {
    path: PathBuf,
    /// (unix ms, frame ms) since the last flush.
    buf: Vec<(u64, f32)>,
    last_flush: Instant,
}

impl FrameRecorder {
    /// `Some` only when `RUSTCORD_FRAMETIMES` names a file. The file is
    /// truncated so each run starts clean.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("RUSTCORD_FRAMETIMES").filter(|p| !p.is_empty())?;
        Self::new(PathBuf::from(path))
    }

    pub fn new(path: PathBuf) -> Option<Self> {
        std::fs::File::create(&path).ok()?;
        Some(Self {
            path,
            buf: Vec::with_capacity(CAPACITY),
            last_flush: Instant::now(),
        })
    }

    /// Record one frame's CPU time (eframe's `frame.info().cpu_usage`).
    pub fn push(&mut self, seconds: Option<f32>) {
        let Some(s) = seconds else { return };
        if self.buf.len() == CAPACITY {
            self.buf.remove(0);
        }
        self.buf.push((unix_ms(), s * 1000.0));
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        self.last_flush = Instant::now();
        if self.buf.is_empty() {
            return;
        }
        let text = format_lines(&self.buf);
        let ok = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .is_ok();
        if ok {
            self.buf.clear();
        }
    }
}

impl Drop for FrameRecorder {
    fn drop(&mut self) {
        self.flush();
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn format_lines(frames: &[(u64, f32)]) -> String {
    let mut out = String::with_capacity(frames.len() * 20);
    for (at, ms) in frames {
        out.push_str(&format!("{at},{ms:.2}\n"));
    }
    out
}

/// 95th percentile (nearest rank) of frame times in ms; the probe script
/// does the same in PowerShell (kept here as the tested reference).
#[cfg(test)]
fn p95(frames_ms: &[f32]) -> Option<f32> {
    if frames_ms.is_empty() {
        return None;
    }
    let mut v = frames_ms.to_vec();
    v.sort_by(f32::total_cmp);
    let rank = ((v.len() as f32) * 0.95).ceil() as usize;
    Some(v[rank.clamp(1, v.len()) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_format() {
        assert_eq!(
            format_lines(&[(1000, 3.256), (1016, 12.0)]),
            "1000,3.26\n1016,12.00\n"
        );
    }

    #[test]
    fn percentile() {
        assert_eq!(p95(&[]), None);
        assert_eq!(p95(&[5.0]), Some(5.0));
        let v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        assert_eq!(p95(&v), Some(95.0));
    }

    #[test]
    fn writes_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frames.csv");
        let mut r = FrameRecorder::new(path.clone()).unwrap();
        r.push(Some(0.004));
        r.push(None);
        r.push(Some(0.010));
        r.flush();
        r.push(Some(0.002));
        drop(r);
        let text = std::fs::read_to_string(&path).unwrap();
        let ms: Vec<&str> = text.lines().map(|l| l.split(',').nth(1).unwrap()).collect();
        assert_eq!(ms, ["4.00", "10.00", "2.00"]);
    }
}
