//! User presets (`.rcpreset`): a saved setup made of the panel layout (tile
//! tree), the chosen theme and which modules are on. There are no built-in
//! presets; users save, import, share and apply their own (Settings →
//! Presets, `presets_page.rs`).
//!
//! # File format
//!
//! One JSON document (UTF-8, at most [`MAX_PRESET_FILE`]):
//!
//! ```json
//! {
//!   "format": "rustcord-preset",
//!   "version": 1,
//!   "manifest": { "name": "My setup", "author": "", "description": "" },
//!   "layout": { "format": "rustcord-layout", "version": 1, "tree": { … } },
//!   "theme": { "id": "discord-light", "name": "Discord Light", "file": "…" },
//!   "modules": { "images": true, … }
//! }
//! ```
//!
//! - `layout` is exactly what Settings → Layout → Copy layout makes; it is
//!   read with `layout_presets::import` (size cap, full tree sanitize).
//! - `theme` is `null` for the default look. `file` is the theme's
//!   `.rctheme` text (checked by `theme_file::parse_theme`) so a preset can
//!   be shared with its theme; themes with a background image are only
//!   referenced by id (`file` is `null`), since a preset carries no images.
//! - `modules` is the `config::Modules` table; unknown names are ignored.
//!
//! Unknown top-level or manifest fields are refused, so a preset can never
//! smuggle account data in, and [`Preset`] has no field that could hold
//! any: no tokens, accounts, user / server / channel ids.
//!
//! # Codes
//!
//! "Copy as code" gives one line: `rcpreset1:` + base64 of the compact JSON
//! (presets) or `rctheme1:` + base64 of the `.rctheme` text (themes).
//! [`parse_code`] reads those, raw preset JSON or raw `.rctheme` text.
//!
//! # Storage
//!
//! `<data dir>/presets/<id>.rcpreset`, `id` = `[a-z0-9-]` made from the name
//! ([`crate::themes::slug`]). Imports never copy the source file: it is
//! parsed and the canonical export is written. Nothing is read until the
//! Presets page opens, and nothing at all while the module is off.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use egui_tiles::Tree;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Config, MODULES, Modules};
use crate::layout::Pane;
use crate::theme::Palette;
use crate::theme_file::{MAX_THEME_FILE, parse_theme};

/// Value of `format` in a preset file.
pub const FORMAT: &str = "rustcord-preset";
/// Current preset version; newer ones are refused.
pub const VERSION: u32 = 1;
/// File extension (no dot).
pub const EXTENSION: &str = "rcpreset";
/// Largest preset file / pasted preset accepted (layout ≤ 64 KB + theme
/// ≤ 64 KB + room for the rest).
pub const MAX_PRESET_FILE: u64 = 192 * 1024;
/// Prefix of a preset code.
pub const PRESET_CODE: &str = "rcpreset1:";
/// Prefix of a theme code.
pub const THEME_CODE: &str = "rctheme1:";
/// Most presets listed.
const MAX_PRESETS: usize = 100;
/// Longest preset / theme name.
pub const MAX_NAME: usize = 64;
/// Longest author / description.
const MAX_TEXT: usize = 300;

/// The theme a preset uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetTheme {
    /// Folder id of the theme where it was saved.
    pub id: String,
    /// Its display name.
    pub name: String,
    /// The `.rctheme` text, when it could be included (no background image).
    #[serde(default)]
    pub file: Option<String>,
}

/// A user preset. Only layout, theme and modules: nothing account-related.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub author: String,
    pub description: String,
    /// Sanitized tile tree.
    pub tree: Tree<Pane>,
    /// `None` = the default look.
    pub theme: Option<PresetTheme>,
    pub modules: Modules,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    format: String,
    version: u32,
    manifest: Manifest,
    layout: Value,
    #[serde(default)]
    theme: Option<PresetTheme>,
    modules: Modules,
}

/// Trim, drop control characters, cap at `max` characters.
fn clean(s: &str, max: usize) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Preset {
    /// The current setup as a preset. `theme_file` is the applied theme's
    /// `.rctheme` text (if it can be shared); `theme_name` its display name.
    /// Only `config.modules` and `config.custom_theme` are read.
    pub fn capture(
        name: &str,
        config: &Config,
        tree: &Tree<Pane>,
        theme_name: Option<&str>,
        theme_file: Option<String>,
    ) -> Self {
        let theme = config
            .custom_theme
            .as_ref()
            .filter(|_| config.modules.custom_themes)
            .filter(|id| crate::themes::valid_id(id))
            .map(|id| PresetTheme {
                id: id.clone(),
                name: clean(theme_name.unwrap_or(id), MAX_NAME),
                file: theme_file.filter(|f| theme_text_ok(f).is_ok()),
            });
        let name = clean(name, MAX_NAME);
        Self {
            name: if name.is_empty() {
                "My preset".into()
            } else {
                name
            },
            author: String::new(),
            description: String::new(),
            // Same checks as an import, so the saved tree is always clean.
            tree: crate::layout_presets::sanitize(tree).unwrap_or_else(|_| tree.clone()),
            theme,
            modules: config.modules.clone(),
        }
    }

    /// The preset as pretty JSON (a `.rcpreset` file).
    pub fn export(&self) -> String {
        serde_json::to_string_pretty(&self.doc()).unwrap_or_default()
    }

    /// One-line code for chat: `rcpreset1:<base64>`.
    pub fn code(&self) -> String {
        let json = serde_json::to_string(&self.doc()).unwrap_or_default();
        format!("{PRESET_CODE}{}", STANDARD.encode(json))
    }

    fn doc(&self) -> Doc {
        let layout =
            serde_json::from_str(&crate::layout_presets::export(&self.tree)).unwrap_or(Value::Null);
        Doc {
            format: FORMAT.into(),
            version: VERSION,
            manifest: Manifest {
                name: self.name.clone(),
                author: self.author.clone(),
                description: self.description.clone(),
            },
            layout,
            theme: self.theme.clone(),
            modules: self.modules.clone(),
        }
    }

    /// Read a preset (JSON text). Everything is checked; the result is safe
    /// to apply.
    pub fn import(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.is_empty() {
            bail!("Nothing to import: paste a preset first.");
        }
        if text.len() as u64 > MAX_PRESET_FILE {
            bail!(
                "That's too big to be a RustCord preset (max {} KB).",
                MAX_PRESET_FILE / 1024
            );
        }
        let raw: Value =
            serde_json::from_str(text).context("This isn't a RustCord preset (not JSON)")?;
        let format = raw["format"].as_str().unwrap_or_default();
        if format != FORMAT {
            bail!("This isn't a RustCord preset (format \"{format}\").");
        }
        match raw["version"].as_u64() {
            Some(v) if v > u64::from(VERSION) => {
                bail!("This preset was made by a newer RustCord (version {v}).")
            }
            Some(v) if v >= 1 => {}
            _ => bail!("This isn't a RustCord preset (unknown version)."),
        }
        let doc: Doc = serde_json::from_value(raw)
            .map_err(|e| anyhow!("This preset has a field RustCord doesn't know ({e})."))?;
        let name = clean(&doc.manifest.name, MAX_NAME);
        if name.is_empty() {
            bail!("This preset has no name.");
        }
        let layout = serde_json::to_string(&doc.layout).unwrap_or_default();
        let tree = crate::layout_presets::import(&layout)
            .map_err(|e| anyhow!("The preset's layout is broken: {e:#}"))?;
        let theme = match doc.theme {
            None => None,
            Some(t) => {
                if !crate::themes::valid_id(&t.id) {
                    bail!(
                        "The preset's theme id \"{}\" isn't valid.",
                        clean(&t.id, 40)
                    );
                }
                if let Some(f) = &t.file {
                    theme_text_ok(f).map_err(|e| anyhow!("The preset's theme is broken: {e:#}"))?;
                }
                let name = clean(&t.name, MAX_NAME);
                Some(PresetTheme {
                    name: if name.is_empty() { t.id.clone() } else { name },
                    id: t.id,
                    file: t.file,
                })
            }
        };
        Ok(Self {
            name,
            author: clean(&doc.manifest.author, MAX_TEXT),
            description: clean(&doc.manifest.description, MAX_TEXT),
            tree,
            theme,
            modules: doc.modules,
        })
    }

    /// Names of the modules applying this preset would switch, compared with
    /// `current`. The Presets module itself is never switched by a preset.
    pub fn module_changes(&self, current: &Modules) -> Vec<&'static str> {
        MODULES
            .iter()
            .filter(|m| {
                let (mut a, mut b) = (self.modules.clone(), current.clone());
                (m.get)(&mut a) != (m.get)(&mut b)
            })
            .filter(|m| m.name != PRESETS_MODULE)
            .map(|m| m.name)
            .collect()
    }

    /// The modules to apply: the preset's, but the Presets module keeps its
    /// current value (so applying never hides this page).
    pub fn modules_for(&self, current: &Modules) -> Modules {
        let mut m = self.modules.clone();
        m.presets = current.presets;
        m
    }

    /// Palette of the embedded theme (card previews), if any.
    pub fn palette(&self) -> Option<Palette> {
        let f = self.theme.as_ref()?.file.as_ref()?;
        parse_theme(f).ok().map(|d| d.palette)
    }
}

/// Name of the Presets module in [`MODULES`] (never switched by a preset).
pub const PRESETS_MODULE: &str = "Presets";

/// Theme text a preset may carry: a valid `.rctheme` (≤ 64 KB) that needs
/// no background image. Returns its name.
pub fn theme_text_ok(text: &str) -> Result<String> {
    if text.len() as u64 > MAX_THEME_FILE {
        bail!("theme file is too big (max {} KB)", MAX_THEME_FILE / 1024);
    }
    let def = parse_theme(text)?;
    if def.style.image.is_some() {
        bail!("this theme needs its background image; import its folder or .zip instead");
    }
    Ok(def.meta.name)
}

/// What a pasted code turned out to be.
#[derive(Clone, Debug, PartialEq)]
pub enum Pasted {
    Preset(Box<Preset>),
    /// `.rctheme` text (checked) and the theme's name.
    Theme {
        text: String,
        name: String,
    },
}

/// Read pasted preset or theme code: `rcpreset1:…`, `rctheme1:…`, preset
/// JSON or `.rctheme` text. The error says why it was refused.
pub fn parse_code(text: &str) -> Result<Pasted> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Paste a preset or theme code first.");
    }
    // base64 is 4/3 the size of what it holds.
    if text.len() as u64 > MAX_PRESET_FILE * 4 / 3 + 16 {
        bail!("That's too long to be preset or theme code.");
    }
    if let Some(b64) = text.strip_prefix(PRESET_CODE) {
        let json = decode(b64)?;
        return Preset::import(&json).map(|p| Pasted::Preset(Box::new(p)));
    }
    if let Some(b64) = text.strip_prefix(THEME_CODE) {
        let toml = decode(b64)?;
        let name = theme_text_ok(&toml).map_err(|e| anyhow!("This theme code is broken: {e:#}"))?;
        return Ok(Pasted::Theme { text: toml, name });
    }
    if text.starts_with('{') {
        return Preset::import(text).map(|p| Pasted::Preset(Box::new(p)));
    }
    if !text.contains('=') {
        bail!(
            "This isn't preset or theme code. It should start with {PRESET_CODE} or \
             {THEME_CODE}, be preset JSON, or be a .rctheme file."
        );
    }
    match theme_text_ok(text) {
        Ok(name) => Ok(Pasted::Theme {
            text: text.to_string(),
            name,
        }),
        Err(e) => bail!("This isn't a valid theme: {e:#}"),
    }
}

/// A theme as one-line code: `rctheme1:<base64>`.
pub fn theme_code(text: &str) -> String {
    format!("{THEME_CODE}{}", STANDARD.encode(text))
}

fn decode(b64: &str) -> Result<String> {
    let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(clean)
        .map_err(|_| anyhow!("This code is damaged (couldn't decode it)."))?;
    String::from_utf8(bytes).map_err(|_| anyhow!("This code is damaged (not text)."))
}

// ---- Files ------------------------------------------------------------------

/// A file picked for import, or how an export went.
#[derive(Clone, Debug, PartialEq)]
pub enum FileEvent {
    Picked(PathBuf),
    Exported(PathBuf),
    Failed(String),
}

/// Import: the native file picker (backend thread). `None` = cancelled.
pub async fn pick_file() -> Option<FileEvent> {
    rfd::AsyncFileDialog::new()
        .set_title("Import preset")
        .add_filter("RustCord preset", &[EXTENSION, "json"])
        .pick_file()
        .await
        .map(|f| FileEvent::Picked(f.path().to_path_buf()))
}

/// Export: save dialog, then write the preset (backend thread).
pub async fn export_file(file_name: &str, text: String) -> Option<FileEvent> {
    let target = rfd::AsyncFileDialog::new()
        .set_title("Export preset")
        .add_filter("RustCord preset", &[EXTENSION])
        .set_file_name(file_name)
        .save_file()
        .await?;
    let path = target.path().to_path_buf();
    Some(match tokio::fs::write(&path, text).await {
        Ok(()) => FileEvent::Exported(path),
        Err(e) => FileEvent::Failed(format!("Couldn't export: {e}")),
    })
}

impl PresetStore {
    /// Show the result of a backend file event (import reads the file).
    pub fn file_event(&mut self, e: FileEvent) {
        match e {
            FileEvent::Picked(path) => {
                self.open();
                let _ = self.import(&path);
            }
            FileEvent::Exported(path) => {
                self.notice = Some(Notice::Ok(format!("Exported to {}", path.display())));
            }
            FileEvent::Failed(m) => self.notice = Some(Notice::Err(m)),
        }
    }
}

/// `<data dir>/presets` (demo runs use the demo folder).
pub fn default_dir(demo: bool) -> Option<PathBuf> {
    crate::storage::data_dir(demo).map(|d| d.join("presets"))
}

/// Read a regular file (not a symlink) of at most `cap` bytes as UTF-8.
fn read_capped(path: &Path, cap: u64) -> Result<String> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!("symlinks aren't accepted");
    }
    if !meta.is_file() {
        bail!("choose a .{EXTENSION} file");
    }
    if meta.len() > cap {
        bail!("the file is too big (max {} KB)", cap / 1024);
    }
    let mut buf = Vec::new();
    fs::File::open(path)?.take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        bail!("the file is too big (max {} KB)", cap / 1024);
    }
    String::from_utf8(buf).map_err(|_| anyhow!("the file is not UTF-8 text"))
}

/// Read a `.rcpreset` (or `.json`) file anywhere.
pub fn read_file(path: &Path) -> Result<Preset> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != EXTENSION && ext != "json" {
        bail!("choose a .{EXTENSION} file");
    }
    Preset::import(&read_capped(path, MAX_PRESET_FILE)?)
}

/// Write `preset` into `dir` under a new id made from its name.
pub fn install(dir: &Path, preset: &Preset) -> Result<String> {
    fs::create_dir_all(dir).with_context(|| format!("{}", dir.display()))?;
    let base = crate::themes::slug(&preset.name);
    let id = (1..1000)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|id| fs::symlink_metadata(file_path(dir, id)).is_err())
        .ok_or_else(|| anyhow!("too many presets called {base}"))?;
    let tmp = dir.join(format!(".{id}.tmp"));
    fs::write(&tmp, preset.export())?;
    fs::rename(&tmp, file_path(dir, &id)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(id)
}

fn file_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{EXTENSION}"))
}

/// Remove `<dir>/<id>.rcpreset`.
pub fn delete(dir: &Path, id: &str) -> Result<()> {
    if !crate::themes::valid_id(id) {
        bail!("not a preset id: {id}");
    }
    fs::remove_file(file_path(dir, id)).with_context(|| format!("preset {id} not found"))
}

/// One file in the presets folder.
#[derive(Clone, Debug)]
pub struct PresetEntry {
    pub id: String,
    pub preset: Result<Preset, String>,
}

/// Result of the last action, shown on the Presets page.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Ok(String),
    Err(String),
}

/// The presets folder. Empty until [`PresetStore::open`] (the page opened)
/// and while the module is off.
pub struct PresetStore {
    dir: Option<PathBuf>,
    loaded: bool,
    pub entries: Vec<PresetEntry>,
    pub notice: Option<Notice>,
}

impl PresetStore {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            loaded: false,
            entries: Vec::new(),
            notice: None,
        }
    }

    /// The page opened: read the folder (once; `refresh` re-reads).
    pub fn open(&mut self) {
        if !self.loaded {
            self.refresh();
        }
    }

    /// Module off / page closed: forget everything.
    pub fn close(&mut self) {
        self.loaded = false;
        self.entries = Vec::new();
        self.notice = None;
    }

    pub fn refresh(&mut self) {
        self.loaded = true;
        self.entries.clear();
        let Some(dir) = &self.dir else { return };
        let Ok(read) = fs::read_dir(dir) else { return };
        for e in read.flatten() {
            if self.entries.len() >= MAX_PRESETS {
                break;
            }
            let path = e.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| path.extension().is_some_and(|x| x == EXTENSION))
                .filter(|id| crate::themes::valid_id(id))
                .map(str::to_string)
            else {
                continue;
            };
            let preset = read_capped(&path, MAX_PRESET_FILE)
                .and_then(|t| Preset::import(&t))
                .map_err(|e| format!("{e:#}"));
            self.entries.push(PresetEntry { id, preset });
        }
        self.entries.sort_by_key(|e| {
            e.preset
                .as_ref()
                .map(|p| p.name.to_lowercase())
                .unwrap_or_else(|_| e.id.clone())
        });
    }

    fn dir_or_err(&self) -> Result<&Path> {
        self.dir
            .as_deref()
            .ok_or_else(|| anyhow!("no data folder on this system"))
    }

    /// Save a preset (new file) and refresh. Sets `notice`.
    pub fn save(&mut self, preset: &Preset) -> Result<String> {
        let res = self.dir_or_err().and_then(|d| install(d, preset));
        self.refresh();
        self.notice = Some(match &res {
            Ok(_) => Notice::Ok(format!("Saved preset \"{}\"", preset.name)),
            Err(e) => Notice::Err(format!("Couldn't save the preset: {e:#}")),
        });
        res
    }

    /// Import a picked file. Sets `notice`.
    pub fn import(&mut self, path: &Path) -> Result<String> {
        match read_file(path) {
            Ok(p) => self.save(&p),
            Err(e) => {
                self.notice = Some(Notice::Err(format!("Couldn't import: {e:#}")));
                Err(e)
            }
        }
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        let res = self.dir_or_err().and_then(|d| delete(d, id));
        self.refresh();
        self.notice = Some(match &res {
            Ok(()) => Notice::Ok("Preset deleted".into()),
            Err(e) => Notice::Err(format!("Couldn't delete: {e:#}")),
        });
        res
    }

    pub fn get(&self, id: &str) -> Option<&Preset> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.preset.as_ref().ok())
    }
}

/// Demo mode: two example presets so the Presets page has cards offline.
/// Only writes into the demo's own folder, and only when it's empty.
pub fn seed_demo(dir: &Path) {
    if fs::read_dir(dir).is_ok_and(|mut r| r.next().is_some()) {
        return;
    }
    let light = include_str!("../examples/themes/discord-light/theme.rctheme");
    let mut wide = Preset::capture(
        "Light, wide chat",
        &Config {
            custom_theme: Some("discord-light".into()),
            ..Config::default()
        },
        &crate::layout_presets::build(crate::layout_presets::Preset::WideChat),
        Some("Discord Light"),
        Some(light.to_string()),
    );
    wide.description = "Discord Light with a wide chat; no GIFs or embeds.".into();
    wide.modules.gif_picker = false;
    wide.modules.embeds = false;
    let mut minimal = Preset::capture(
        "Minimal RAM",
        &Config::default(),
        &crate::layout::default_tree(),
        None,
        None,
    );
    minimal.description = "Default look, text only: no images, emoji images or embeds.".into();
    minimal.modules.images = false;
    minimal.modules.emoji_images = false;
    minimal.modules.embeds = false;
    minimal.modules.typing_indicators = false;
    for p in [wide, minimal] {
        let _ = install(dir, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountData, SavedAccount};

    const LIGHT: &str = include_str!("../examples/themes/discord-light/theme.rctheme");

    fn sample() -> Preset {
        let config = Config {
            custom_theme: Some("discord-light".into()),
            ..Config::default()
        };
        let mut p = Preset::capture(
            "My setup",
            &config,
            &crate::layout_presets::build(crate::layout_presets::Preset::WideChat),
            Some("Discord Light"),
            Some(LIGHT.to_string()),
        );
        p.modules.gif_picker = false;
        p.description = "test".into();
        p
    }

    #[test]
    fn round_trip_file_and_code() {
        let p = sample();
        let back = Preset::import(&p.export()).expect("export imports");
        assert_eq!(back, p);
        assert_eq!(back.export(), p.export());
        match parse_code(&p.code()).expect("code imports") {
            Pasted::Preset(q) => assert_eq!(*q, p),
            other => panic!("not a preset: {other:?}"),
        }
        // Raw JSON pastes too.
        assert!(matches!(parse_code(&p.export()), Ok(Pasted::Preset(_))));
    }

    #[test]
    fn round_trip_through_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = PresetStore::new(Some(dir.path().to_path_buf()));
        store.open();
        assert!(store.entries.is_empty());
        let p = sample();
        let id = store.save(&p).unwrap();
        assert_eq!(id, "my-setup");
        assert_eq!(store.get(&id), Some(&p));
        // Same name again gets a new id.
        assert_eq!(store.save(&p).unwrap(), "my-setup-2");
        // A file export imports back.
        let file = dir.path().join("shared.rcpreset");
        fs::write(&file, p.export()).unwrap();
        let id3 = store.import(&file).unwrap();
        assert_eq!(store.get(&id3), Some(&p));
        store.delete("my-setup").unwrap();
        assert!(store.get("my-setup").is_none());
        // The picked file itself sits in the folder too (id "shared").
        assert!(store.get("shared").is_some());
        assert!(store.entries.iter().all(|e| e.preset.is_ok()));
    }

    #[test]
    fn default_look_and_theme_code() {
        let p = Preset::capture(
            "x",
            &Config::default(),
            &crate::layout::default_tree(),
            None,
            None,
        );
        assert_eq!(p.theme, None);
        assert_eq!(Preset::import(&p.export()).unwrap(), p);
        match parse_code(&theme_code(LIGHT)).unwrap() {
            Pasted::Theme { name, text } => {
                assert_eq!(name, "Discord Light");
                assert_eq!(text, LIGHT);
            }
            other => panic!("{other:?}"),
        }
        // Raw .rctheme text is theme code too.
        assert!(matches!(parse_code(LIGHT), Ok(Pasted::Theme { .. })));
    }

    #[test]
    fn module_changes_skip_the_presets_module() {
        let mut p = sample();
        p.modules.presets = false;
        let cur = Modules::default();
        assert_eq!(p.module_changes(&cur), vec!["GIF picker"]);
        assert!(p.modules_for(&cur).presets);
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        let good: Value = serde_json::from_str(&sample().export()).unwrap();
        let with = |f: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            f(&mut v);
            Preset::import(&v.to_string()).unwrap_err().to_string()
        };
        assert!(Preset::import("").is_err());
        assert!(
            Preset::import("not json")
                .unwrap_err()
                .to_string()
                .contains("not JSON")
        );
        assert!(with(&|v| v["format"] = "rustcord-layout".into()).contains("format"));
        assert!(with(&|v| v["version"] = 99.into()).contains("newer"));
        assert!(with(&|v| v["manifest"]["name"] = " ".into()).contains("no name"));
        assert!(with(&|v| v["layout"]["tree"] = Value::Null).contains("layout"));
        assert!(with(&|v| v["theme"]["file"] = "garbage = [".into()).contains("theme"));
        // Unsafe theme ids (paths) are refused.
        for bad in ["../../etc", "a/b", "C:\\x", ".hidden", ""] {
            assert!(
                with(&|v| v["theme"]["id"] = bad.into()).contains("isn't valid"),
                "{bad}"
            );
        }
        // Extra fields (e.g. smuggled account data) are refused.
        assert!(with(&|v| v["token"] = "abc".into()).contains("field"));
        assert!(with(&|v| v["manifest"]["user_id"] = "1".into()).contains("field"));
        // Damaged codes.
        assert!(
            parse_code("rcpreset1:!!!")
                .unwrap_err()
                .to_string()
                .contains("damaged")
        );
        assert!(
            parse_code("hello there")
                .unwrap_err()
                .to_string()
                .contains("isn't preset or theme code")
        );
        assert!(
            parse_code("format = 99")
                .unwrap_err()
                .to_string()
                .contains("valid theme")
        );
    }

    #[test]
    fn oversized_input_is_refused() {
        let mut v: Value = serde_json::from_str(&sample().export()).unwrap();
        v["manifest"]["description"] = "x".repeat(MAX_PRESET_FILE as usize).into();
        assert!(
            Preset::import(&v.to_string())
                .unwrap_err()
                .to_string()
                .contains("too big")
        );
        assert!(parse_code(&"a".repeat(MAX_PRESET_FILE as usize * 2)).is_err());
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.rcpreset");
        fs::write(&big, vec![b' '; MAX_PRESET_FILE as usize + 1]).unwrap();
        assert!(read_file(&big).unwrap_err().to_string().contains("too big"));
        // Long names are cut, not refused.
        let p = Preset::capture(
            &"n".repeat(500),
            &Config::default(),
            &crate::layout::default_tree(),
            None,
            None,
        );
        assert_eq!(p.name.chars().count(), MAX_NAME);
    }

    #[test]
    fn unsafe_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.rcpreset");
        fs::write(&ok, sample().export()).unwrap();
        assert!(read_file(&dir.path().join("x.exe")).is_err());
        assert!(read_file(dir.path()).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.rcpreset");
            std::os::unix::fs::symlink(&ok, &link).unwrap();
            assert!(
                read_file(&link)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
        }
        // Deleting never leaves the folder.
        assert!(delete(dir.path(), "../ok").is_err());
        assert!(delete(dir.path(), "ok/../../x").is_err());
        // A theme that needs an image can't ride along in a preset.
        let with_image = format!("{LIGHT}\n[images]\nchat_background = \"bg.png\"\n");
        assert!(parse_theme(&with_image).unwrap().style.image.is_some());
        assert!(
            theme_text_ok(&with_image)
                .unwrap_err()
                .to_string()
                .contains("background image")
        );
        assert!(parse_code(&theme_code(&with_image)).is_err());
    }

    #[test]
    fn no_account_data_in_a_preset() {
        let mut config = Config {
            custom_theme: Some("discord-light".into()),
            last_account: Some("111122223333444455".into()),
            trusted_domains: vec!["secret-domain.example".into()],
            ..Config::default()
        };
        config.accounts.push(SavedAccount {
            user_id: "111122223333444455".into(),
            username: "secret_username".into(),
            display_name: "Secret Name".into(),
            avatar_hash: Some("a_secretavatarhash".into()),
            is_bot: false,
        });
        let mut data = AccountData {
            last_channel: Some("999988887777666655".into()),
            ..AccountData::default()
        };
        data.drafts
            .insert("555544443333222211".into(), "secret draft".into());
        config.per_account.insert("111122223333444455".into(), data);
        config.legacy_saved_token = Some(crate::accounts::Token::new("mfa.FAKE_TOKEN_VALUE"));
        let p = Preset::capture(
            "x",
            &config,
            &crate::layout::default_tree(),
            Some("Discord Light"),
            Some(LIGHT.into()),
        );
        let text = p.export();
        let code = p.code();
        let decoded = decode(code.strip_prefix(PRESET_CODE).unwrap()).unwrap();
        for out in [&text, &decoded] {
            for leak in [
                "111122223333444455",
                "999988887777666655",
                "555544443333222211",
                "secret_username",
                "Secret Name",
                "secret-domain",
                "secretavatarhash",
                "secret draft",
                "FAKE_TOKEN",
                "\"token\"",
                "\"accounts\"",
                "\"per_account\"",
                "\"last_account\"",
            ] {
                assert!(!out.contains(leak), "preset leaks {leak}");
            }
        }
        let mut keys: Vec<String> = serde_json::from_str::<Value>(&text)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "format", "layout", "manifest", "modules", "theme", "version"
            ]
        );
    }

    #[test]
    fn demo_seed_makes_valid_presets_once() {
        let dir = tempfile::tempdir().unwrap();
        seed_demo(dir.path());
        let mut store = PresetStore::new(Some(dir.path().to_path_buf()));
        store.open();
        assert_eq!(store.entries.len(), 2);
        assert!(store.entries.iter().all(|e| e.preset.is_ok()));
        seed_demo(dir.path());
        store.refresh();
        assert_eq!(store.entries.len(), 2);
    }
}
