//! Saved accounts: tokens live in the OS credential store, never in Config.
//!
//! - Windows: Credential Manager, macOS: Keychain, Linux: Secret Service
//!   (GNOME Keyring / KWallet), through the `keyring` crate.
//! - One entry per account: service [`SERVICE`], user `account:<user id>`.
//!   A token whose user id isn't known yet (the one-time migration of the
//!   old plaintext `saved_token`) sits under [`PENDING_KEY`] until READY
//!   tells us who it belongs to, then it is re-keyed.
//! - `Config` only holds non-secret metadata: [`SavedAccount`] (id, names,
//!   avatar hash, bot flag), `last_account`, and the per-account
//!   [`AccountData`] section. At most [`MAX_ACCOUNTS`] are saved.
//!
//! Tokens are a [`Token`]: no `Display`, no `Serialize`, and a `Debug` that
//! prints `Token(<redacted>)`. Never format a token into logs, toasts,
//! errors or crash output. Store errors are mapped to short messages that
//! can't carry the secret (keyring's `BadEncoding` holds the raw bytes).
//!
//! The account switcher (left-click switches, right-click "Open in new
//! window" / "Remove account…") builds on [`Vault`]:
//! `config.accounts` lists the menu rows, [`Vault::token`] gets the token to
//! start a session, [`Vault::forget`] removes one account.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize};

use crate::config::{ClientIdentity, Config};
use crate::types::User;

/// Up to 5 saved accounts, like Discord.
pub const MAX_ACCOUNTS: usize = 5;
/// Credential store service name.
pub const SERVICE: &str = "RustCord";
/// Store key for a token whose user id isn't known yet.
pub const PENDING_KEY: &str = "pending";

/// Store key of an account's token.
pub fn account_key(user_id: &str) -> String {
    format!("account:{user_id}")
}

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A Discord token ("Bot " prefix for bots). Deliberately has no `Display`
/// and no `Serialize`; `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The secret itself, for the backend's Authorization header / IDENTIFY
    /// and the credential store. Never format it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Read-only, for the migration of the old `Config::saved_token`; a token is
/// never serialized.
impl<'de> Deserialize<'de> for Token {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Token)
    }
}

// ---------------------------------------------------------------------------
// Metadata (non-secret, saved in Config)
// ---------------------------------------------------------------------------

/// One saved account as the switcher shows it. No secrets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedAccount {
    pub user_id: String,
    /// The unique @username.
    pub username: String,
    /// Display name (global_name, else username) for the menu row.
    pub display_name: String,
    /// Avatar hash (`None` = Discord's default avatar).
    pub avatar_hash: Option<String>,
    pub is_bot: bool,
}

impl SavedAccount {
    pub fn from_user(me: &User) -> Self {
        Self {
            user_id: me.id.clone(),
            username: me.username.clone(),
            display_name: me.name.clone(),
            avatar_hash: avatar_hash_from_url(&me.avatar_url),
            is_bot: me.bot,
        }
    }

    /// CDN URL of the avatar (64 px), for the switcher rows.
    pub fn avatar_url(&self) -> String {
        crate::types::avatar_url(&self.user_id, self.avatar_hash.as_deref())
    }
}

/// `…/avatars/<id>/<hash>.<ext>?size=64` → `<hash>`; default avatars → None.
fn avatar_hash_from_url(url: &str) -> Option<String> {
    let rest = url.split("/avatars/").nth(1)?;
    let file = rest.split('/').nth(1)?;
    let hash = file.split(['.', '?']).next()?;
    (!hash.is_empty()).then(|| hash.to_string())
}

/// Per-account local data, keyed by user id in `Config::per_account`.
/// Discord-side things belong here (last channel, drafts, "Show as");
/// RustCord settings, themes, mods, layout and modules stay shared in
/// `Config`. Saved on a switch / exit, restored after READY.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountData {
    /// Channel to reopen when switching back to this account.
    pub last_channel: Option<String>,
    /// Unsent message box text per channel id.
    pub drafts: BTreeMap<String, String>,
    /// "Show as" for this account (`None` = the shared default).
    pub client_identity: Option<ClientIdentity>,
}

// ---------------------------------------------------------------------------
// Secret stores
// ---------------------------------------------------------------------------

/// Credential store failure. The message never contains the secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No credential store on this system (e.g. no Secret Service running).
    Unavailable(String),
    /// The store refused or failed the request.
    Failed(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Unavailable(why) => {
                write!(f, "the system's credential store isn't available ({why})")
            }
            StoreError::Failed(why) => write!(f, "the system's credential store failed ({why})"),
        }
    }
}

/// Where tokens are kept. The OS store in the app, [`MemoryStore`] in tests
/// and demo mode.
pub trait SecretStore {
    fn get(&self, key: &str) -> Result<Option<Token>, StoreError>;
    fn set(&self, key: &str, token: &Token) -> Result<(), StoreError>;
    /// Deleting a missing entry is not an error.
    fn delete(&self, key: &str) -> Result<(), StoreError>;
}

/// The OS credential store (via `keyring`).
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(SERVICE, key).map_err(map_keyring_error)
    }
}

/// Map a keyring error to a message that can't hold the secret.
fn map_keyring_error(e: keyring::Error) -> StoreError {
    use keyring::Error as E;
    match e {
        E::NoStorageAccess(_) => StoreError::Unavailable("access denied or locked".into()),
        E::NoDefaultStore => StoreError::Unavailable("no credential store".into()),
        E::PlatformFailure(_) => StoreError::Failed("platform error".into()),
        E::BadEncoding(_) => StoreError::Failed("stored entry isn't text".into()),
        E::TooLong(..) => StoreError::Failed("entry too long".into()),
        E::Invalid(..) => StoreError::Failed("invalid entry".into()),
        E::Ambiguous(_) => StoreError::Failed("more than one matching entry".into()),
        E::NoEntry => StoreError::Failed("no entry".into()),
        _ => StoreError::Failed("unknown error".into()),
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<Token>, StoreError> {
        match Self::entry(key)?.get_password() {
            Ok(s) => Ok(Some(Token(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e)),
        }
    }

    fn set(&self, key: &str, token: &Token) -> Result<(), StoreError> {
        Self::entry(key)?
            .set_password(token.expose())
            .map_err(map_keyring_error)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_error(e)),
        }
    }
}

/// In-memory store: tests and demo mode (which never touches the real one).
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Token>>,
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.entries.lock().map(|e| e.len()).unwrap_or(0);
        write!(f, "MemoryStore({n} entries, <redacted>)")
    }
}

impl MemoryStore {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Token>> {
        // A poisoned lock still holds valid data.
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Token>, StoreError> {
        Ok(self.map().get(key).cloned())
    }

    fn set(&self, key: &str, token: &Token) -> Result<(), StoreError> {
        self.map().insert(key.to_string(), token.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.map().remove(key);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Vault: the accounts API
// ---------------------------------------------------------------------------

/// Saving an account failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountError {
    /// Already [`MAX_ACCOUNTS`] saved; remove one first.
    Full,
    Store(StoreError),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccountError::Full => write!(
                f,
                "You can save up to {MAX_ACCOUNTS} accounts. Remove one to save this login."
            ),
            AccountError::Store(e) => write!(f, "Couldn't save the login: {e}"),
        }
    }
}

impl From<StoreError> for AccountError {
    fn from(e: StoreError) -> Self {
        AccountError::Store(e)
    }
}

/// The accounts API: tokens in a [`SecretStore`], metadata in `Config`.
pub struct Vault {
    store: Box<dyn SecretStore>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vault(<redacted>)")
    }
}

impl Vault {
    /// The OS credential store.
    pub fn os() -> Self {
        Self::with_store(Box::new(KeyringStore))
    }

    /// In memory only (demo mode, tests).
    pub fn memory() -> Self {
        Self::with_store(Box::new(MemoryStore::default()))
    }

    pub fn with_store(store: Box<dyn SecretStore>) -> Self {
        Self { store }
    }

    /// A saved account's token (`None` if it has no entry).
    pub fn token(&self, user_id: &str) -> Result<Option<Token>, StoreError> {
        self.store.get(&account_key(user_id))
    }

    /// The token to log in with at start-up: the last used account's, else a
    /// pending (just migrated) one. Returns the account's user id when known.
    pub fn startup_token(&self, config: &Config) -> Option<(Token, Option<String>)> {
        if let Some(id) = &config.last_account
            && config.accounts.iter().any(|a| &a.user_id == id)
            && let Ok(Some(t)) = self.token(id)
        {
            return Some((t, Some(id.clone())));
        }
        match self.store.get(PENDING_KEY) {
            Ok(Some(t)) => Some((t, None)),
            _ => None,
        }
    }

    /// Remember an account after READY: store its token under its user id,
    /// add or refresh its metadata, make it the last account, and drop any
    /// pending entry. Refuses a new account past [`MAX_ACCOUNTS`].
    pub fn save(
        &self,
        config: &mut Config,
        account: SavedAccount,
        token: &Token,
    ) -> Result<(), AccountError> {
        let existing = config
            .accounts
            .iter()
            .position(|a| a.user_id == account.user_id);
        if existing.is_none() && config.accounts.len() >= MAX_ACCOUNTS {
            return Err(AccountError::Full);
        }
        self.store.set(&account_key(&account.user_id), token)?;
        // Re-keyed: the pending copy isn't needed any more.
        let _ = self.store.delete(PENDING_KEY);
        config.last_account = Some(account.user_id.clone());
        match existing {
            Some(i) => config.accounts[i] = account,
            None => config.accounts.push(account),
        }
        Ok(())
    }

    /// Remove one account: its store entry, metadata and per-account data.
    pub fn forget(&self, config: &mut Config, user_id: &str) -> Result<(), StoreError> {
        let result = self.store.delete(&account_key(user_id));
        config.accounts.retain(|a| a.user_id != user_id);
        config.per_account.remove(user_id);
        if config.last_account.as_deref() == Some(user_id) {
            config.last_account = None;
        }
        result
    }

    /// Remove every saved account ("Remember me" switched off).
    pub fn forget_all(&self, config: &mut Config) -> Result<(), StoreError> {
        let ids: Vec<String> = config.accounts.iter().map(|a| a.user_id.clone()).collect();
        let mut result = self.clear_pending();
        for id in ids {
            if let Err(e) = self.forget(config, &id) {
                result = Err(e);
            }
        }
        config.last_account = None;
        result
    }

    /// Drop a pending token (it turned out invalid, or the user logged out
    /// before READY).
    pub fn clear_pending(&self) -> Result<(), StoreError> {
        self.store.delete(PENDING_KEY)
    }

    /// One-time migration of the old plaintext `saved_token`: move it into
    /// the store under [`PENDING_KEY`] (re-keyed at READY by [`Vault::save`])
    /// and drop it from `Config`. Returns whether there was one. On a store
    /// error the token is handed back so this session can still log in; it
    /// is never written back to the config file either way.
    pub fn migrate(&self, config: &mut Config) -> Result<bool, (StoreError, Token)> {
        let Some(token) = config.legacy_saved_token.take() else {
            return Ok(false);
        };
        if token.is_empty() || !config.remember_login {
            return Ok(true);
        }
        match self.store.set(PENDING_KEY, &token) {
            Ok(()) => Ok(true),
            Err(e) => Err((e, token)),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "FAKE.TOKEN.123";

    fn user(id: &str) -> User {
        User {
            id: id.into(),
            name: format!("Name {id}"),
            username: format!("user{id}"),
            avatar_url: crate::types::avatar_url(id, Some("abc123")),
            bot: false,
        }
    }

    fn acct(id: &str) -> SavedAccount {
        SavedAccount::from_user(&user(id))
    }

    #[test]
    fn save_load_remove() {
        let v = Vault::memory();
        let mut c = Config::default();
        v.save(&mut c, acct("1"), &Token::new(FAKE)).unwrap();
        assert_eq!(v.token("1").unwrap(), Some(Token::new(FAKE)));
        assert_eq!(c.accounts.len(), 1);
        assert_eq!(c.last_account.as_deref(), Some("1"));
        assert_eq!(c.accounts[0].avatar_hash.as_deref(), Some("abc123"));
        assert_eq!(c.accounts[0].username, "user1");

        // Saving again refreshes, doesn't duplicate.
        let mut a = acct("1");
        a.display_name = "New".into();
        v.save(&mut c, a, &Token::new("FAKE.TOKEN.456")).unwrap();
        assert_eq!(c.accounts.len(), 1);
        assert_eq!(c.accounts[0].display_name, "New");
        assert_eq!(v.token("1").unwrap(), Some(Token::new("FAKE.TOKEN.456")));

        let (t, id) = v.startup_token(&c).unwrap();
        assert_eq!(t, Token::new("FAKE.TOKEN.456"));
        assert_eq!(id.as_deref(), Some("1"));

        c.per_account.insert("1".into(), AccountData::default());
        v.forget(&mut c, "1").unwrap();
        assert_eq!(v.token("1").unwrap(), None);
        assert!(c.accounts.is_empty());
        assert!(c.per_account.is_empty());
        assert_eq!(c.last_account, None);
        assert!(v.startup_token(&c).is_none());
        // Forgetting twice is fine.
        v.forget(&mut c, "1").unwrap();
    }

    #[test]
    fn forget_only_removes_that_account() {
        let v = Vault::memory();
        let mut c = Config::default();
        v.save(&mut c, acct("1"), &Token::new("A")).unwrap();
        v.save(&mut c, acct("2"), &Token::new("B")).unwrap();
        v.forget(&mut c, "2").unwrap();
        assert_eq!(v.token("1").unwrap(), Some(Token::new("A")));
        assert_eq!(c.accounts.len(), 1);
        // The last account went away: nothing to auto-log in with.
        assert_eq!(c.last_account, None);
        v.forget_all(&mut c).unwrap();
        assert_eq!(v.token("1").unwrap(), None);
        assert!(c.accounts.is_empty());
    }

    #[test]
    fn five_account_cap() {
        let v = Vault::memory();
        let mut c = Config::default();
        for i in 0..MAX_ACCOUNTS {
            v.save(&mut c, acct(&i.to_string()), &Token::new(FAKE))
                .unwrap();
        }
        assert_eq!(
            v.save(&mut c, acct("99"), &Token::new(FAKE)),
            Err(AccountError::Full)
        );
        assert_eq!(v.token("99").unwrap(), None);
        assert_eq!(c.accounts.len(), MAX_ACCOUNTS);
        // An already saved account can still be refreshed.
        v.save(&mut c, acct("0"), &Token::new("X")).unwrap();
        // And after removing one there's room again.
        v.forget(&mut c, "1").unwrap();
        v.save(&mut c, acct("99"), &Token::new(FAKE)).unwrap();
        assert_eq!(c.accounts.len(), MAX_ACCOUNTS);
    }

    #[test]
    fn migration_runs_once_and_rekeys_at_ready() {
        let json = format!(r#"{{ "remember_login": true, "saved_token": "{FAKE}" }}"#);
        let mut c: Config = serde_json::from_str(&json).unwrap();
        assert!(c.legacy_saved_token.is_some());
        let v = Vault::memory();

        assert_eq!(v.migrate(&mut c), Ok(true));
        assert!(c.legacy_saved_token.is_none());
        assert_eq!(v.migrate(&mut c), Ok(false), "second run does nothing");
        // Written back without the token.
        let saved = serde_json::to_string(&c).unwrap();
        assert!(!saved.contains(FAKE));
        assert!(!saved.contains("saved_token"));
        let reloaded: Config = serde_json::from_str(&saved).unwrap();
        assert!(reloaded.legacy_saved_token.is_none());

        // Start-up logs in with the pending token; READY re-keys it.
        let (t, id) = v.startup_token(&c).unwrap();
        assert_eq!(t, Token::new(FAKE));
        assert_eq!(id, None);
        v.save(&mut c, acct("42"), &t).unwrap();
        assert_eq!(v.store.get(PENDING_KEY).unwrap(), None);
        assert_eq!(v.token("42").unwrap(), Some(Token::new(FAKE)));
        assert_eq!(
            v.startup_token(&c).unwrap().1.as_deref(),
            Some("42"),
            "next start uses the re-keyed entry"
        );
    }

    #[test]
    fn migration_with_remember_off_drops_the_token() {
        let json = format!(r#"{{ "remember_login": false, "saved_token": "{FAKE}" }}"#);
        let mut c: Config = serde_json::from_str(&json).unwrap();
        let v = Vault::memory();
        assert_eq!(v.migrate(&mut c), Ok(true));
        assert!(c.legacy_saved_token.is_none());
        assert!(v.startup_token(&c).is_none());
    }

    #[test]
    fn debug_is_redacted() {
        let t = Token::new(FAKE);
        assert_eq!(format!("{t:?}"), "Token(<redacted>)");
        assert!(!format!("{:?}", Some(&t)).contains(FAKE));

        let store = MemoryStore::default();
        store.set("k", &t).unwrap();
        assert!(!format!("{store:?}").contains(FAKE));
        let v = Vault::with_store(Box::new(store));
        assert!(!format!("{v:?}").contains(FAKE));

        let json = format!(r#"{{ "saved_token": "{FAKE}" }}"#);
        let c: Config = serde_json::from_str(&json).unwrap();
        assert!(!format!("{c:?}").contains(FAKE));
        assert!(!format!("{c:#?}").contains(FAKE));
    }

    /// Token leak: after "Remember me" nothing that is persisted (Config,
    /// as eframe writes it) or printed (Debug, errors) holds the token.
    #[test]
    fn remember_me_leaves_no_token_in_persisted_data() {
        let v = Vault::memory();
        let mut c = Config {
            remember_login: true,
            ..Config::default()
        };
        v.save(&mut c, acct("7"), &Token::new(FAKE)).unwrap();
        c.per_account.insert("7".into(), AccountData::default());
        for text in [
            serde_json::to_string(&c).unwrap(),
            serde_json::to_string_pretty(&c).unwrap(),
            format!("{c:?}"),
            format!("{:?}", c.accounts),
            AccountError::Full.to_string(),
            AccountError::Store(StoreError::Failed("x".into())).to_string(),
        ] {
            assert!(!text.contains(FAKE), "token leaked into: {text}");
        }
    }

    #[test]
    fn keyring_errors_never_carry_the_secret() {
        let e = map_keyring_error(keyring::Error::BadEncoding(FAKE.as_bytes().to_vec()));
        assert!(!e.to_string().contains(FAKE));
        assert!(!format!("{e:?}").contains(FAKE));
        let e = map_keyring_error(keyring::Error::Invalid(FAKE.into(), FAKE.into()));
        assert!(!e.to_string().contains(FAKE));
    }

    #[test]
    fn avatar_hash_parsing() {
        assert_eq!(
            avatar_hash_from_url(&crate::types::avatar_url("5", Some("a_ff"))),
            Some("a_ff".into())
        );
        assert_eq!(
            avatar_hash_from_url(&crate::types::avatar_url("5", None)),
            None
        );
        assert_eq!(
            acct("5").avatar_url(),
            crate::types::avatar_url("5", Some("abc123"))
        );
    }

    /// Token leak, logs: RustCord has no `log`/`tracing` subscriber, so text
    /// output is `println!` / `eprintln!` / `dbg!` / `panic!` / toasts.
    /// Scan the sources: no such sink may format anything named like a
    /// token, and no struct with a token field may derive `Debug`.
    #[test]
    fn no_token_reaches_log_sinks() {
        const SINKS: &[&str] = &[
            "println!",
            "eprintln!",
            "print!",
            "eprint!",
            "dbg!",
            "panic!",
            "log::",
            "tracing::",
            "toast",
            "Status(",
        ];
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut problems = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            // The derive written right above the current struct.
            let mut pending_derive = String::new();
            let mut struct_derive = String::new();
            for (n, line) in text.lines().enumerate() {
                let t = line.trim();
                if t.starts_with("//") {
                    continue;
                }
                if t.starts_with("#[derive(") {
                    pending_derive = t.to_string();
                } else if t.starts_with("pub struct ") || t.starts_with("struct ") {
                    struct_derive = std::mem::take(&mut pending_derive);
                } else if !t.starts_with("#[") {
                    pending_derive.clear();
                }
                let lower = t.to_ascii_lowercase();
                let mentions_token = ["{token", "token:?", "token)", ", token", "token.expose"]
                    .iter()
                    .any(|p| lower.contains(p));
                if mentions_token && SINKS.iter().any(|s| t.contains(s)) {
                    problems.push(format!("{name}:{}: {t}", n + 1));
                }
                // A token field in a struct that derives Debug.
                let field = t.trim_start_matches("pub ");
                if (field.starts_with("token: String")
                    || field.starts_with("raw_token: String")
                    || field.starts_with("token: Option<String>"))
                    && struct_derive.contains("Debug")
                {
                    problems.push(format!("{name}:{}: Debug on a token field", n + 1));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "token may leak:\n{}",
            problems.join("\n")
        );
    }

    // Mods / plugin host: there is no PluginHost on this branch yet. When it
    // lands, add a test here that its API surface exposes neither a `Token`
    // nor `Config::accounts` / `Vault` (the hard rule: mods get no token or
    // account list).
}
