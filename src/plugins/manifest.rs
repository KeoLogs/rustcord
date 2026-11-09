//! `plugin.toml`: what a plugin is and which permissions it asks for.
//!
//! ```toml
//! id = "hello"
//! name = "Hello"
//! version = "1.0.0"
//! author = "Leo"
//! description = "Adds /shrug and /time."
//! min_rustcord = "0.1.0"
//! permissions = ["commands"]
//! entry = "main.luau"
//! ```

use serde::{Deserialize, Serialize};

/// Something a plugin may do. Declared in `plugin.toml`, approved by the
/// user, enforced by the Lua API (a call without the permission errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Receive `message` / `message_update` events for channels you can see.
    ReadMessages,
    /// Add panels, buttons and a settings section.
    Ui,
    /// Register local `/commands` that produce text for the message box.
    Commands,
    /// Change how messages look on this PC (never what's sent).
    MessageDisplay,
    /// A small key-value store on this PC.
    Storage,
    /// Show local pop-up notices.
    Notifications,
}

impl Permission {
    pub const ALL: [Permission; 6] = [
        Permission::ReadMessages,
        Permission::Ui,
        Permission::Commands,
        Permission::MessageDisplay,
        Permission::Storage,
        Permission::Notifications,
    ];

    /// The name used in `plugin.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Permission::ReadMessages => "read_messages",
            Permission::Ui => "ui",
            Permission::Commands => "commands",
            Permission::MessageDisplay => "message_display",
            Permission::Storage => "storage",
            Permission::Notifications => "notifications",
        }
    }

    pub fn from_key(key: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Permission::ReadMessages => "Read messages",
            Permission::Ui => "Add panels and buttons",
            Permission::Commands => "Slash commands",
            Permission::MessageDisplay => "Change how messages look",
            Permission::Storage => "Save data",
            Permission::Notifications => "Show notices",
        }
    }

    /// What approving it means, for the approval dialog.
    pub fn description(self) -> &'static str {
        match self {
            Permission::ReadMessages => {
                "Sees new and edited messages in the channels you can see. It can't send \
                 them anywhere: plugins have no network or file access."
            }
            Permission::Ui => "Adds its own panels, buttons and a settings section.",
            Permission::Commands => {
                "Adds /commands that fill your message box with text. You still press \
                 Enter to send; plugins can never send anything themselves."
            }
            Permission::MessageDisplay => {
                "Changes how messages are shown on this PC only (e.g. hide words). \
                 Nothing changes on Discord."
            }
            Permission::Storage => "Keeps up to 256 KB of its own settings / notes on this PC.",
            Permission::Notifications => "Shows small pop-up notices inside RustCord.",
        }
    }
}

/// Parsed and validated `plugin.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub min_rustcord: Option<String>,
    /// Sorted, no duplicates.
    pub permissions: Vec<Permission>,
    /// Path of the entry script inside the plugin folder.
    pub entry: String,
}

/// The raw TOML (lenient: unknown keys are ignored for forward compatibility).
#[derive(Deserialize)]
struct RawManifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    min_rustcord: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default = "default_entry")]
    entry: String,
}

fn default_entry() -> String {
    "main.luau".into()
}

/// Largest accepted `plugin.toml`.
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024;

impl Manifest {
    /// Parse and validate. Errors are short, user-facing sentences.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err("plugin.toml is too big".into());
        }
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| format!("plugin.toml: {}", e.message()))?;
        if !valid_id(&raw.id) {
            return Err(format!(
                "Invalid id \"{}\": use 1-64 lowercase letters, digits, - or _",
                raw.id
            ));
        }
        let name = raw.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("name must be 1-64 characters".into());
        }
        let version = raw.version.trim().to_string();
        if version.is_empty() || version.len() > 32 {
            return Err("version must be 1-32 characters".into());
        }
        if raw.author.chars().count() > 64 {
            return Err("author is too long (max 64)".into());
        }
        if raw.description.chars().count() > 500 {
            return Err("description is too long (max 500)".into());
        }
        if let Some(min) = &raw.min_rustcord {
            let Some(min_v) = parse_version(min) else {
                return Err(format!("min_rustcord \"{min}\" isn't a version like 0.1.0"));
            };
            let ours = parse_version(env!("CARGO_PKG_VERSION")).unwrap_or_default();
            if min_v > ours {
                return Err(format!(
                    "Needs RustCord {min} or newer (this is {})",
                    env!("CARGO_PKG_VERSION")
                ));
            }
        }
        let mut permissions = Vec::new();
        for p in &raw.permissions {
            let Some(perm) = Permission::from_key(p) else {
                return Err(format!("Unknown permission \"{p}\""));
            };
            permissions.push(perm);
        }
        permissions.sort();
        permissions.dedup();
        if !valid_entry(&raw.entry) {
            return Err(format!(
                "entry \"{}\" must be a .luau or .lua file inside the plugin folder",
                raw.entry
            ));
        }
        Ok(Manifest {
            id: raw.id,
            name,
            version,
            author: raw.author.trim().to_string(),
            description: raw.description.trim().to_string(),
            min_rustcord: raw.min_rustcord,
            permissions,
            entry: raw.entry,
        })
    }

    pub fn has(&self, p: Permission) -> bool {
        self.permissions.contains(&p)
    }
}

/// Plugin ids double as folder names: keep them boring.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// A relative path of plain segments ending in .luau / .lua.
fn valid_entry(entry: &str) -> bool {
    (entry.ends_with(".luau") || entry.ends_with(".lua"))
        && entry.len() <= 128
        && entry.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        })
}

/// "1.2.3" → (1, 2, 3). Missing parts are 0; pre-release suffixes ignored.
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let core = v.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || -> Option<u32> {
        match parts.next() {
            None => Some(0),
            Some(p) => p.parse().ok(),
        }
    };
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
        id = "hello"
        name = "Hello"
        version = "1.0.0"
        author = "Leo"
        description = "Says hi"
        min_rustcord = "0.1.0"
        permissions = ["commands", "storage", "commands"]
        entry = "main.luau"
        future_key = "ignored"
    "#;

    #[test]
    fn parses_good_manifest() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.id, "hello");
        assert_eq!(m.name, "Hello");
        assert_eq!(
            m.permissions,
            vec![Permission::Commands, Permission::Storage]
        );
        assert!(m.has(Permission::Commands));
        assert!(!m.has(Permission::ReadMessages));
        assert_eq!(m.entry, "main.luau");
    }

    #[test]
    fn defaults() {
        let m = Manifest::parse("id = \"x\"\nname = \"X\"\nversion = \"1\"").unwrap();
        assert_eq!(m.entry, "main.luau");
        assert!(m.permissions.is_empty());
    }

    #[test]
    fn rejects_bad_manifests() {
        let bad = |s: &str| Manifest::parse(s).unwrap_err();
        let base = |extra: &str, id: &str| {
            format!("id = \"{id}\"\nname = \"X\"\nversion = \"1.0\"\n{extra}")
        };
        assert!(bad("not toml = = =").contains("plugin.toml"));
        assert!(bad("name = \"X\"\nversion = \"1\"").contains("plugin.toml"));
        assert!(bad(&base("", "Bad Id")).contains("Invalid id"));
        assert!(bad(&base("", "../evil")).contains("Invalid id"));
        assert!(bad(&base("", "")).contains("Invalid id"));
        assert!(bad(&base("permissions = [\"network\"]", "x")).contains("Unknown permission"));
        assert!(bad(&base("entry = \"../x.luau\"", "x")).contains("entry"));
        assert!(bad(&base("entry = \"/etc/x.luau\"", "x")).contains("entry"));
        assert!(bad(&base("entry = \"main.txt\"", "x")).contains("entry"));
        assert!(bad(&base("entry = \"a\\\\b.luau\"", "x")).contains("entry"));
        assert!(bad(&base("min_rustcord = \"999.0.0\"", "x")).contains("Needs RustCord"));
        assert!(bad(&base("min_rustcord = \"soon\"", "x")).contains("min_rustcord"));
        assert!(bad("id = \"x\"\nname = \"\"\nversion = \"1\"").contains("name"));
        assert!(Manifest::parse(&base("entry = \"src/main.lua\"", "x")).is_ok());
    }

    #[test]
    fn ids_and_versions() {
        assert!(valid_id("word-filter"));
        assert!(valid_id("a_1"));
        assert!(!valid_id("-a"));
        assert!(!valid_id("A"));
        assert!(!valid_id(&"a".repeat(65)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.1"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.0.0-beta"), Some((1, 0, 0)));
        assert_eq!(parse_version("1.x"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }
}
