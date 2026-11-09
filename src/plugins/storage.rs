//! Per-plugin key-value store (`rustcord.storage`), capped and saved as JSON
//! in the plugin's data folder (`<data>/<id>/storage.json`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

/// Total size cap (keys + JSON-encoded values).
pub const STORAGE_CAP: usize = 256 * 1024;
/// Longest accepted key.
pub const MAX_KEY_LEN: usize = 128;

pub struct Storage {
    /// None = in-memory only (tests, plugins without a data folder).
    path: Option<PathBuf>,
    map: BTreeMap<String, Value>,
    size: usize,
    dirty: bool,
}

fn entry_size(key: &str, value: &Value) -> usize {
    key.len() + serde_json::to_string(value).map_or(0, |s| s.len())
}

impl Storage {
    pub fn in_memory() -> Self {
        Storage {
            path: None,
            map: BTreeMap::new(),
            size: 0,
            dirty: false,
        }
    }

    /// Load `path` if it exists. A broken or oversized file starts empty.
    pub fn open(path: PathBuf) -> Self {
        let mut s = Storage::in_memory();
        if let Ok(text) = std::fs::read_to_string(&path)
            && text.len() <= STORAGE_CAP * 2
            && let Ok(map) = serde_json::from_str::<BTreeMap<String, Value>>(&text)
        {
            let size = map.iter().map(|(k, v)| entry_size(k, v)).sum();
            if size <= STORAGE_CAP {
                s.map = map;
                s.size = size;
            }
        }
        s.path = Some(path);
        s
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    pub fn get_str(&self, key: &str) -> &str {
        self.map.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.map.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    /// Set (or with `Value::Null`, remove) a key. Errors if the key is bad or
    /// the store would go over [`STORAGE_CAP`].
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Err(format!("storage key must be 1-{MAX_KEY_LEN} bytes"));
        }
        let old = self.map.get(key).map_or(0, |v| entry_size(key, v));
        if value.is_null() {
            if self.map.remove(key).is_some() {
                self.size -= old;
                self.dirty = true;
            }
            return Ok(());
        }
        let new = entry_size(key, &value);
        let total = self.size - old + new;
        if total > STORAGE_CAP {
            return Err(format!(
                "storage is full ({} KB max per plugin)",
                STORAGE_CAP / 1024
            ));
        }
        if self.map.get(key) != Some(&value) {
            self.map.insert(key.to_string(), value);
            self.size = total;
            self.dirty = true;
        }
        Ok(())
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Write to disk if changed (temp file + rename).
    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            let text = serde_json::to_string(&self.map).unwrap_or_else(|_| "{}".into());
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, path)
        };
        write().map_err(|e| format!("couldn't save storage: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cap_and_remove() {
        let mut s = Storage::in_memory();
        s.set("a", json!("hello")).unwrap();
        assert_eq!(s.get_str("a"), "hello");
        assert!(s.set("", json!(1)).is_err());
        assert!(s.set(&"k".repeat(200), json!(1)).is_err());
        // Over the cap: refused, old value kept.
        let big = "x".repeat(STORAGE_CAP);
        assert!(s.set("b", json!(big)).unwrap_err().contains("full"));
        assert!(s.get("b").is_none());
        // Just under the cap is fine; replacing a value frees its old size.
        let fits = "x".repeat(STORAGE_CAP - 100);
        s.set("b", json!(fits)).unwrap();
        s.set("b", json!("small")).unwrap();
        s.set("c", json!("x".repeat(1000))).unwrap();
        s.set("c", Value::Null).unwrap();
        assert!(s.get("c").is_none());
        assert_eq!(
            s.size(),
            entry_size("a", &json!("hello")) + entry_size("b", &json!("small"))
        );
    }

    #[test]
    fn persists() {
        let dir = std::env::temp_dir().join(format!("rc-storage-{}", std::process::id()));
        let path = dir.join("p").join("storage.json");
        let mut s = Storage::open(path.clone());
        s.set("notes", json!({"c1": "hi"})).unwrap();
        assert!(s.is_dirty());
        s.flush().unwrap();
        let s2 = Storage::open(path);
        assert_eq!(s2.get("notes"), Some(&json!({"c1": "hi"})));
        let _ = std::fs::remove_dir_all(dir);
    }
}
