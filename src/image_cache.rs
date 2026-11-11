//! Disk image cache: avatars, icons, emoji and attachments are kept in
//! `<data dir>/cache/images` so they aren't downloaded again on the next
//! start. Least recently used files go first once the folder passes the
//! size set in Settings → Performance (200 MB by default, 0 = off).
//!
//! [`Loader`] replaces egui's http bytes loader. It holds exactly what that
//! loader held (the bytes of images egui currently shows, dropped by
//! `ctx.forget_image`), so the cache costs disk, not RAM: there is no
//! in-memory index, only a running byte count. Each image loads on its own
//! short-lived thread (as before); that thread also does the disk read and
//! write, so the UI thread never touches the disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::SystemTime;

use eframe::egui;
use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};

/// Default size limit (Settings → Performance).
pub const DEFAULT_MB: u32 = 200;
/// The slider's upper end.
pub const MAX_MB: u32 = 1000;
/// Files bigger than this are never cached (they would push everything
/// else out).
const MAX_FILE: u64 = 32 * 1024 * 1024;

/// Size limit in bytes; 0 = the cache is off (nothing read or written).
static LIMIT: AtomicU64 = AtomicU64::new(0);
/// Bytes on disk (from one scan at start-up, then kept by `put`/`evict`).
static USED: AtomicU64 = AtomicU64::new(0);
static DIR: OnceLock<PathBuf> = OnceLock::new();
/// One eviction at a time.
static EVICTING: Mutex<()> = Mutex::new(());

/// Apply the setting. Lowering it trims the folder on the next write.
pub fn set_limit_mb(mb: u32) {
    LIMIT.store(u64::from(mb) * 1024 * 1024, Ordering::Relaxed);
}

/// Bytes the cache uses on disk right now (for the "Clear cache" button).
pub fn used_bytes() -> u64 {
    USED.load(Ordering::Relaxed)
}

/// "Clear cache": delete every cached file, off the UI thread.
pub fn clear_async(ctx: &egui::Context) {
    let Some(dir) = DIR.get() else { return };
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _guard = EVICTING.lock();
        clear(dir);
        USED.store(0, Ordering::Relaxed);
        ctx.request_repaint();
    });
}

/// Install the loader (after egui_extras' loaders, so it is asked first)
/// and count what is already on disk.
pub fn install(ctx: &egui::Context, data_dir: Option<PathBuf>) {
    if let Some(dir) = data_dir.map(|d| d.join("cache").join("images")) {
        let _ = DIR.set(dir);
        std::thread::spawn(|| {
            if let Some(dir) = DIR.get() {
                USED.store(dir_size(dir), Ordering::Relaxed);
            }
        });
    }
    ctx.add_bytes_loader(Arc::new(Loader::default()));
}

// ---- the files (pure: every function takes the folder) ----

/// The cache file name for a URL: a hash, so names are safe and say
/// nothing. Discord's signed attachment links change their `ex` / `is` /
/// `hm` parameters every day; those are left out so the picture is still
/// found.
pub fn cache_key(url: &str) -> String {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let mut norm = String::with_capacity(url.len());
    norm.push_str(path);
    for (i, p) in query
        .split('&')
        .filter(|p| {
            let name = p.split('=').next().unwrap_or("");
            !p.is_empty() && !matches!(name, "ex" | "is" | "hm")
        })
        .enumerate()
    {
        norm.push(if i == 0 { '?' } else { '&' });
        norm.push_str(p);
    }
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, norm.as_bytes());
    digest.as_ref()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A cached file is `[mime length: 1 byte][mime][image bytes]`.
fn encode(bytes: &[u8], mime: Option<&str>) -> Vec<u8> {
    let mime = mime.unwrap_or("").as_bytes();
    let mime = &mime[..mime.len().min(255)];
    let mut out = Vec::with_capacity(1 + mime.len() + bytes.len());
    out.push(mime.len() as u8);
    out.extend_from_slice(mime);
    out.extend_from_slice(bytes);
    out
}

fn decode(mut file: Vec<u8>) -> Option<(Vec<u8>, Option<String>)> {
    let n = usize::from(*file.first()?);
    if file.len() < 1 + n {
        return None;
    }
    let mime = std::str::from_utf8(&file[1..1 + n]).ok()?.to_owned();
    file.drain(..1 + n);
    Some((file, (!mime.is_empty()).then_some(mime)))
}

/// Read a cached image and mark it as just used.
pub fn get(dir: &Path, key: &str) -> Option<(Vec<u8>, Option<String>)> {
    let path = dir.join(key);
    let file = std::fs::read(&path).ok()?;
    // LRU is by modification time: reading counts as use.
    if let Ok(f) = std::fs::File::options().write(true).open(&path) {
        let _ = f.set_modified(SystemTime::now());
    }
    decode(file)
}

/// Write an image; returns the bytes added on disk (0 if it wasn't stored).
pub fn put(dir: &Path, key: &str, bytes: &[u8], mime: Option<&str>) -> u64 {
    if bytes.is_empty() || bytes.len() as u64 > MAX_FILE {
        return 0;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return 0;
    }
    let data = encode(bytes, mime);
    // Write then rename, so a crash never leaves half a picture.
    let tmp = dir.join(format!("{key}.tmp"));
    let path = dir.join(key);
    let old = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if std::fs::write(&tmp, &data).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return 0;
    }
    (data.len() as u64).saturating_sub(old)
}

fn entries(dir: &Path) -> Vec<(SystemTime, u64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| {
                (
                    m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    m.len(),
                    e.path(),
                )
            })
        })
        .collect()
}

pub fn dir_size(dir: &Path) -> u64 {
    entries(dir).iter().map(|e| e.1).sum()
}

/// Delete the least recently used files until the folder is at most 90% of
/// `limit` (so it isn't trimmed again on the very next write). Returns the
/// size afterwards.
pub fn evict(dir: &Path, limit: u64) -> u64 {
    let mut files = entries(dir);
    let mut total: u64 = files.iter().map(|e| e.1).sum();
    if total <= limit {
        return total;
    }
    let target = limit / 10 * 9;
    files.sort_by_key(|e| e.0);
    for (_, size, path) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    total
}

/// Delete every file in the cache folder (never anything else).
pub fn clear(dir: &Path) {
    for (_, _, path) in entries(dir) {
        let _ = std::fs::remove_file(path);
    }
}

// ---- the loader ----

#[derive(Clone)]
struct File {
    bytes: Arc<[u8]>,
    mime: Option<String>,
}

type Entry = Poll<Result<File, String>>;

/// Loads `http(s)://` images: from the disk cache when it has them,
/// otherwise from the network (then stores them).
#[derive(Default)]
pub struct Loader {
    cache: Arc<egui::mutex::Mutex<HashMap<String, Entry>>>,
}

impl Loader {
    pub const ID: &'static str = egui::generate_loader_id!(Loader);
}

/// Load one image: disk first, then the network. Runs on its own thread.
fn fetch(uri: &str) -> Result<File, String> {
    let limit = LIMIT.load(Ordering::Relaxed);
    let disk = (limit > 0).then(|| DIR.get()).flatten();
    let key = disk.map(|_| cache_key(uri));
    if let (Some(dir), Some(key)) = (disk, &key)
        && let Some((bytes, mime)) = get(dir, key)
    {
        return Ok(File {
            bytes: bytes.into(),
            mime,
        });
    }
    let response = ehttp::fetch_blocking(&ehttp::Request::get(uri))
        .map_err(|_| "Failed to load image".to_owned())?;
    if !response.ok {
        return Err(format!("Failed to load image ({})", response.status));
    }
    let mime = response.content_type().map(str::to_owned);
    if let (Some(dir), Some(key)) = (disk, &key) {
        let added = put(dir, key, &response.bytes, mime.as_deref());
        let used = USED.fetch_add(added, Ordering::Relaxed) + added;
        if used > limit
            && let Ok(_guard) = EVICTING.try_lock()
        {
            USED.store(evict(dir, limit), Ordering::Relaxed);
        }
    }
    Ok(File {
        bytes: response.bytes.into(),
        mime,
    })
}

impl BytesLoader for Loader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !uri.starts_with("https://") && !uri.starts_with("http://") {
            return Err(LoadError::NotSupported);
        }
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(uri).cloned() {
            return match entry {
                Poll::Ready(Ok(file)) => Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(file.bytes),
                    mime: file.mime,
                }),
                Poll::Ready(Err(err)) => Err(LoadError::Loading(err)),
                Poll::Pending => Ok(BytesPoll::Pending { size: None }),
            };
        }
        cache.insert(uri.to_owned(), Poll::Pending);
        drop(cache);

        let uri = uri.to_owned();
        let ctx = ctx.clone();
        let cache = Arc::clone(&self.cache);
        let spawned = std::thread::Builder::new()
            .name("image".into())
            .spawn(move || {
                let result = fetch(&uri);
                // Forgotten while loading (`forget_image`): drop the result.
                let repaint = match cache.lock().get_mut(&uri) {
                    Some(entry) => {
                        *entry = Poll::Ready(result);
                        true
                    }
                    None => false,
                };
                // Never lock the Context while the cache lock is held.
                if repaint {
                    ctx.request_repaint();
                }
            });
        if spawned.is_err() {
            return Err(LoadError::Loading("Failed to load image".to_owned()));
        }
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        let _ = self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|entry| match entry {
                Poll::Ready(Ok(file)) => {
                    file.bytes.len() + file.mime.as_ref().map_or(0, String::len)
                }
                Poll::Ready(Err(err)) => err.len(),
                Poll::Pending => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn age(dir: &Path, key: &str, secs_ago: u64) {
        let f = std::fs::File::options()
            .write(true)
            .open(dir.join(key))
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
            .unwrap();
    }

    #[test]
    fn key_ignores_the_signature_parameters() {
        let a = cache_key("https://cdn.discordapp.com/attachments/1/2/a.png?ex=1&is=2&hm=3");
        let b = cache_key("https://cdn.discordapp.com/attachments/1/2/a.png?ex=9&is=8&hm=7&");
        let c = cache_key("https://cdn.discordapp.com/attachments/1/2/a.png");
        assert_eq!(a, b);
        assert_eq!(a, c);
        // Size and format still matter.
        let small = cache_key("https://cdn.discordapp.com/avatars/1/x.png?size=64");
        let big = cache_key("https://cdn.discordapp.com/avatars/1/x.png?size=128&ex=1");
        assert_ne!(small, big);
        assert_eq!(
            big,
            cache_key("https://cdn.discordapp.com/avatars/1/x.png?ex=5&size=128")
        );
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn put_then_get_round_trips_bytes_and_mime() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("images");
        assert_eq!(put(&dir, "k1", b"hello", Some("image/png")), 15);
        assert_eq!(
            get(&dir, "k1"),
            Some((b"hello".to_vec(), Some("image/png".into())))
        );
        assert_eq!(put(&dir, "k2", b"x", None), 2);
        assert_eq!(get(&dir, "k2"), Some((b"x".to_vec(), None)));
        assert_eq!(get(&dir, "missing"), None);
        // Nothing empty or oversized is stored.
        assert_eq!(put(&dir, "k3", b"", None), 0);
        assert_eq!(dir_size(&dir), 17);
        // Replacing a file only counts the growth.
        assert_eq!(put(&dir, "k2", b"xyz", None), 2);
    }

    #[test]
    fn eviction_removes_the_least_recently_used_first() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let blob = [7u8; 99]; // 100 bytes on disk each
        for (key, secs) in [("old", 300), ("mid", 200), ("new", 100)] {
            put(dir, key, &blob, None);
            age(dir, key, secs);
        }
        // Under the limit: nothing happens.
        assert_eq!(evict(dir, 300), 300);
        assert!(dir.join("old").exists());
        // Over: trimmed to 90% of the limit, oldest first.
        assert_eq!(evict(dir, 250), 200);
        assert!(!dir.join("old").exists());
        assert!(dir.join("mid").exists() && dir.join("new").exists());
    }

    #[test]
    fn reading_a_file_saves_it_from_eviction() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let blob = [7u8; 99];
        for (key, secs) in [("a", 300), ("b", 200), ("c", 100)] {
            put(dir, key, &blob, None);
            age(dir, key, secs);
        }
        // "a" is the oldest until it is used again.
        assert!(get(dir, "a").is_some());
        assert_eq!(evict(dir, 150), 100);
        assert!(dir.join("a").exists());
        assert!(!dir.join("b").exists() && !dir.join("c").exists());
    }

    #[test]
    fn clear_empties_the_folder_and_zero_limit_evicts_all() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        put(dir, "a", b"abc", None);
        put(dir, "b", b"abc", None);
        assert_eq!(evict(dir, 0), 0);
        put(dir, "c", b"abc", None);
        clear(dir);
        assert_eq!(dir_size(dir), 0);
        // A missing folder is fine.
        assert_eq!(evict(&dir.join("nope"), 10), 0);
    }
}
