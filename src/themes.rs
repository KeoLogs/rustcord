//! Custom themes: the theme folder (`ThemeStore`), importing `.rctheme`
//! files / folders / `.zip`s, and the active theme (`ActiveTheme`) with the
//! painting helpers panes use for gradients, glass and the chat background.
//!
//! Themes live in `<data dir>/themes/<id>/theme.rctheme` (+ an optional
//! background image). None are bundled into the binary; the default look
//! (Discord Dark) is always available, and example themes (Discord Light,
//! test themes) live in `examples/themes/` for Import. With the "Custom
//! themes" module off, nothing is read from disk.
//!
//! Memory: only the active theme's background image is decoded, downscaled
//! to at most `MAX_IMAGE_SIDE` and uploaded once; dropping the `ActiveTheme`
//! (switching themes) frees the texture.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
    TextureHandle, TextureOptions, pos2, vec2,
};

use crate::config::Config;
use crate::layout::Pane;
use crate::theme::{Palette, ThemeChoice};
use crate::theme_file::{
    Gradient, ImageFit, MAX_IMAGE_FILE, MAX_THEME_FILE, THEME_FILE, ThemeDef, ThemeStyle,
    parse_theme, safe_asset_path,
};

/// Background images are downscaled to fit in this many pixels per side.
pub const MAX_IMAGE_SIDE: u32 = 1600;
/// Largest `.zip` accepted for import.
pub const MAX_ZIP: u64 = 8 * 1024 * 1024;
/// Most entries a `.zip` may have.
const MAX_ZIP_ENTRIES: usize = 64;
/// Most themes listed.
const MAX_THEMES: usize = 200;

/// `<RustCord data dir>/themes` (eframe's storage folder; demo runs use
/// the demo folder).
pub fn default_dir(demo: bool) -> Option<PathBuf> {
    crate::storage::themes_dir(demo)
}

/// One folder in the themes directory.
#[derive(Clone, Debug)]
pub struct ThemeEntry {
    /// Folder name.
    pub id: String,
    /// The parsed theme, or why it can't be used.
    pub theme: Result<ThemeDef, String>,
}

/// Result of the last import / delete, shown on the Themes page.
#[derive(Clone, Debug)]
pub enum Notice {
    Ok(String),
    Err(String),
}

/// The themes folder and what's in it. Cheap: parsed theme files only (no
/// images), and nothing at all when the module is off.
pub struct ThemeStore {
    dir: Option<PathBuf>,
    enabled: bool,
    pub entries: Vec<ThemeEntry>,
    /// What's applied now (mirror of `Config`; see `sync_with`).
    pub active_custom: Option<String>,
    pub notice: Option<Notice>,
}

impl ThemeStore {
    /// `dir` is usually `default_dir(demo)`. Scans it when `enabled`.
    pub fn new(dir: Option<PathBuf>, enabled: bool) -> Self {
        let mut s = Self {
            dir,
            enabled,
            entries: Vec::new(),
            active_custom: None,
            notice: None,
        };
        s.refresh();
        s
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Module switched on / off. Off forgets the list.
    pub fn set_enabled(&mut self, on: bool) {
        if on != self.enabled {
            self.enabled = on;
            self.refresh();
        }
    }

    /// Mirror the applied theme from `Config` (for the "Active" badge).
    pub fn sync_with(&mut self, config: &Config) {
        self.active_custom = if config.modules.custom_themes {
            config.custom_theme.clone()
        } else {
            None
        };
        self.set_enabled(config.modules.custom_themes);
    }

    /// Re-read the folder (after an import or when the page opens).
    pub fn refresh(&mut self) {
        self.entries.clear();
        if !self.enabled {
            return;
        }
        let Some(dir) = &self.dir else { return };
        let Ok(read) = fs::read_dir(dir) else { return };
        for e in read.flatten().take(MAX_THEMES) {
            let Some(id) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            // Skip staging folders, files and symlinks.
            if id.starts_with('.') || !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let theme = read_theme_file(&e.path().join(THEME_FILE)).map_err(|e| format!("{e:#}"));
            self.entries.push(ThemeEntry { id, theme });
        }
        self.entries.sort_by_key(|e| {
            e.theme
                .as_ref()
                .map(|t| t.meta.name.to_lowercase())
                .unwrap_or_else(|_| e.id.clone())
        });
    }

    pub fn get(&self, id: &str) -> Option<&ThemeDef> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.theme.as_ref().ok())
    }

    /// Import a theme and refresh. Sets `notice`. (Small file copies; the
    /// file picker itself should run in the backend.)
    pub fn import(&mut self, src: &Path) -> Result<String> {
        let res = if !self.enabled {
            Err(anyhow!("turn on the Custom themes module first"))
        } else if let Some(dir) = &self.dir {
            import(dir, src)
        } else {
            Err(anyhow!("no data folder on this system"))
        };
        self.refresh();
        self.notice = Some(match &res {
            Ok(id) => Notice::Ok(format!(
                "Imported \"{}\"",
                self.get(id).map(|t| t.meta.name.as_str()).unwrap_or(id)
            )),
            Err(e) => Notice::Err(format!("Couldn't import: {e:#}")),
        });
        res
    }

    /// Delete an installed theme (the page asks for confirmation first).
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let dir = self
            .dir
            .as_deref()
            .ok_or_else(|| anyhow!("no themes folder"))?;
        let res = delete(dir, id);
        self.refresh();
        self.notice = Some(match &res {
            Ok(()) => Notice::Ok("Theme deleted".into()),
            Err(e) => Notice::Err(format!("Couldn't delete: {e:#}")),
        });
        res
    }

    /// Read a theme and its image (file I/O + decoding; can run on any thread).
    pub fn load(&self, id: &str) -> Result<LoadedTheme> {
        if !self.enabled {
            bail!("the Custom themes module is off");
        }
        let dir = self
            .dir
            .as_deref()
            .ok_or_else(|| anyhow!("no themes folder"))?;
        load_theme(dir, id)
    }
}

// ---- Files ------------------------------------------------------------------

/// Read a regular file (not a symlink) of at most `cap` bytes.
fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{}", path.display()))?;
    if !meta.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    if meta.len() > cap {
        bail!(
            "{} is too big ({} KB, max {} KB)",
            file_name(path),
            meta.len() / 1024,
            cap / 1024
        );
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    fs::File::open(path)?.take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        bail!("{} is too big", file_name(path));
    }
    Ok(buf)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parse_bytes(bytes: &[u8]) -> Result<ThemeDef> {
    let text = std::str::from_utf8(bytes).map_err(|_| anyhow!("theme file is not UTF-8 text"))?;
    parse_theme(text)
}

fn read_theme_file(path: &Path) -> Result<ThemeDef> {
    parse_bytes(&read_capped(path, MAX_THEME_FILE)?)
}

/// A folder-name id from the theme's name: `"Frutiger Aero!"` → `frutiger-aero`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "theme".into() } else { out }
}

/// Ids are folder names we made: `[a-z0-9-]`, no dots or separators.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What an import found: the theme file and its image (if any).
struct Bundle {
    theme_bytes: Vec<u8>,
    def: ThemeDef,
    image: Option<(PathBuf, Vec<u8>)>,
}

/// Import `src` (a `.rctheme` file, a folder holding `theme.rctheme`, or a
/// `.zip` of either) into `themes_dir`. Returns the new theme's id. Only the
/// theme file and the image it names are copied.
pub fn import(themes_dir: &Path, src: &Path) -> Result<String> {
    let meta = fs::symlink_metadata(src).with_context(|| format!("{}", src.display()))?;
    if meta.file_type().is_symlink() {
        bail!("symlinks aren't accepted");
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bundle = if meta.is_dir() {
        bundle_from_folder(src)?
    } else if ext == "zip" {
        bundle_from_zip(&read_capped(src, MAX_ZIP)?)?
    } else if ext == "rctheme" {
        let theme_bytes = read_capped(src, MAX_THEME_FILE)?;
        let def = parse_bytes(&theme_bytes)?;
        // A lone file may sit next to its image.
        let folder = src.parent().unwrap_or(Path::new("."));
        let image = read_bundle_image(&def, folder)?;
        Bundle {
            theme_bytes,
            def,
            image,
        }
    } else {
        bail!("choose a .rctheme file, a .zip or a theme folder");
    };
    install(themes_dir, bundle)
}

fn read_bundle_image(def: &ThemeDef, folder: &Path) -> Result<Option<(PathBuf, Vec<u8>)>> {
    let Some(spec) = &def.style.image else {
        return Ok(None);
    };
    let path = folder.join(&spec.path);
    let bytes = read_capped(&path, MAX_IMAGE_FILE)
        .with_context(|| format!("background image {}", spec.path.display()))?;
    check_image(&bytes)?;
    Ok(Some((spec.path.clone(), bytes)))
}

fn bundle_from_folder(dir: &Path) -> Result<Bundle> {
    let theme_bytes = read_capped(&dir.join(THEME_FILE), MAX_THEME_FILE)
        .with_context(|| format!("no readable {THEME_FILE} in this folder"))?;
    let def = parse_bytes(&theme_bytes)?;
    // No symlinked folders on the way to the image either.
    if let Some(spec) = &def.style.image {
        let mut p = dir.to_path_buf();
        for c in spec.path.parent().into_iter().flat_map(Path::components) {
            p.push(c);
            // (Missing folders are reported when the image is read.)
            if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                bail!("symlinks aren't accepted");
            }
        }
    }
    let image = read_bundle_image(&def, dir)?;
    Ok(Bundle {
        theme_bytes,
        def,
        image,
    })
}

/// Unpack only what we need from a zip: `theme.rctheme` at the root or in a
/// single top folder, plus the image it names. Any unsafe entry (absolute,
/// `..`, symlink) rejects the whole archive.
fn bundle_from_zip(bytes: &[u8]) -> Result<Bundle> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("not a valid zip: {e}"))?;
    if zip.len() > MAX_ZIP_ENTRIES {
        bail!("zip has too many files (max {MAX_ZIP_ENTRIES})");
    }
    let mut theme_at: Option<(usize, PathBuf)> = None;
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| anyhow!("zip: {e}"))?;
        let raw = f
            .name()
            .map_err(|e| anyhow!("zip: bad file name: {e}"))?
            .into_owned();
        let Some(name) = f.enclosed_name() else {
            bail!("zip entry \"{raw}\" points outside the theme folder");
        };
        if raw.starts_with('/') || raw.contains('\\') || raw.contains(':') {
            bail!("zip entry \"{raw}\" isn't a safe relative path");
        }
        if f.is_symlink() {
            bail!("zip entry \"{raw}\" is a symlink");
        }
        if name
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("zip entry \"{raw}\" isn't a safe relative path");
        }
        if f.is_file() && name.file_name().is_some_and(|n| n == THEME_FILE) {
            let depth = name.components().count();
            if depth > 2 {
                continue;
            }
            if theme_at
                .as_ref()
                .is_some_and(|(_, p)| p.components().count() <= depth)
            {
                continue;
            }
            theme_at = Some((i, name.clone()));
        }
        names.push(name);
    }
    let Some((idx, theme_path)) = theme_at else {
        bail!("no {THEME_FILE} in the zip");
    };
    let theme_bytes = read_zip_entry(&mut zip, idx, MAX_THEME_FILE)?;
    let def = parse_bytes(&theme_bytes)?;
    let image = match &def.style.image {
        None => None,
        Some(spec) => {
            let want = theme_path
                .parent()
                .unwrap_or(Path::new(""))
                .join(&spec.path);
            let i = names.iter().position(|n| *n == want).ok_or_else(|| {
                anyhow!("background image {} isn't in the zip", spec.path.display())
            })?;
            let bytes = read_zip_entry(&mut zip, i, MAX_IMAGE_FILE)?;
            check_image(&bytes)?;
            Some((spec.path.clone(), bytes))
        }
    };
    Ok(Bundle {
        theme_bytes,
        def,
        image,
    })
}

fn read_zip_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, i: usize, cap: u64) -> Result<Vec<u8>> {
    let f = zip.by_index(i).map_err(|e| anyhow!("zip: {e}"))?;
    let name = f.name().map(|n| n.into_owned()).unwrap_or_default();
    if !f.is_file() || f.size() > cap {
        bail!("{name} is too big (max {} KB)", cap / 1024);
    }
    let mut buf = Vec::new();
    // `take` guards against a lying size header (zip bombs).
    f.take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| anyhow!("zip: {name}: {e}"))?;
    if buf.len() as u64 > cap {
        bail!("{name} is too big (max {} KB)", cap / 1024);
    }
    Ok(buf)
}

/// Copy a bundle into `<themes_dir>/<id>` via a staging folder.
fn install(themes_dir: &Path, b: Bundle) -> Result<String> {
    fs::create_dir_all(themes_dir).with_context(|| format!("{}", themes_dir.display()))?;
    let base = slug(&b.def.meta.name);
    let id = (1..1000)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|id| fs::symlink_metadata(themes_dir.join(id)).is_err())
        .ok_or_else(|| anyhow!("too many themes called {base}"))?;
    let staging = themes_dir.join(format!(".import-{id}"));
    let _ = fs::remove_dir_all(&staging);
    let write = || -> Result<()> {
        fs::create_dir_all(&staging)?;
        fs::write(staging.join(THEME_FILE), &b.theme_bytes)?;
        if let Some((rel, bytes)) = &b.image {
            // Re-check: only ever write inside the staging folder.
            let rel = safe_asset_path(&rel.to_string_lossy())?;
            let dest = staging.join(&rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(dest, bytes)?;
        }
        fs::rename(&staging, themes_dir.join(&id))?;
        Ok(())
    };
    write().inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    Ok(id)
}

/// Remove `<themes_dir>/<id>`.
pub fn delete(themes_dir: &Path, id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("not a theme id: {id}");
    }
    let path = themes_dir.join(id);
    let meta = fs::symlink_metadata(&path).with_context(|| format!("theme {id} not found"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        // Never follow a link out of the themes folder; just drop the link.
        fs::remove_file(&path)?;
    } else {
        fs::remove_dir_all(&path)?;
    }
    Ok(())
}

/// A theme read from disk, ready to become the `ActiveTheme`.
pub struct LoadedTheme {
    pub id: String,
    pub def: ThemeDef,
    pub image: Option<ColorImage>,
}

impl std::fmt::Debug for LoadedTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTheme")
            .field("id", &self.id)
            .field("name", &self.def.meta.name)
            .field("image", &self.image.as_ref().map(|i| i.size))
            .finish()
    }
}

/// Read `<themes_dir>/<id>` and decode its image.
pub fn load_theme(themes_dir: &Path, id: &str) -> Result<LoadedTheme> {
    if !valid_id(id) {
        bail!("not a theme id: {id}");
    }
    let dir = themes_dir.join(id);
    let def = read_theme_file(&dir.join(THEME_FILE))?;
    let image = match &def.style.image {
        Some(spec) => {
            let bytes = read_capped(&dir.join(&spec.path), MAX_IMAGE_FILE)?;
            Some(decode_image(&bytes)?)
        }
        None => None,
    };
    Ok(LoadedTheme {
        id: id.to_string(),
        def,
        image,
    })
}

fn image_reader(bytes: &[u8]) -> Result<image::ImageReader<Cursor<&[u8]>>> {
    let mut r = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow!("image: {e}"))?;
    if !matches!(
        r.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    ) {
        bail!("the background image must be a PNG or JPEG");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    r.limits(limits);
    Ok(r)
}

/// Cheap check at import: a PNG/JPEG whose header we can read.
fn check_image(bytes: &[u8]) -> Result<()> {
    let (w, h) = image_reader(bytes)?
        .into_dimensions()
        .map_err(|e| anyhow!("background image: {e}"))?;
    if w > 8192 || h > 8192 {
        bail!("background image is too large ({w}×{h}, max 8192×8192)");
    }
    Ok(())
}

/// Decode and downscale a PNG / JPEG to at most `MAX_IMAGE_SIDE`.
pub fn decode_image(bytes: &[u8]) -> Result<ColorImage> {
    let img = image_reader(bytes)?
        .decode()
        .map_err(|e| anyhow!("background image: {e}"))?;
    let img = if img.width().max(img.height()) > MAX_IMAGE_SIDE {
        img.resize(
            MAX_IMAGE_SIDE,
            MAX_IMAGE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

/// Open the themes folder in the system file manager (creates it first).
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(dir).spawn().map(|_| ())
}

/// The file / folder picker for "Import theme…" (call from the backend).
pub async fn pick_import_source(folder: bool) -> Option<PathBuf> {
    let d = rfd::AsyncFileDialog::new().set_title("Import theme");
    let picked = if folder {
        d.pick_folder().await
    } else {
        d.add_filter("RustCord theme", &["rctheme", "zip"])
            .pick_file()
            .await
    };
    picked.map(|f| f.path().to_path_buf())
}

// ---- The active theme -------------------------------------------------------

/// The theme the app draws with: colours, style and (maybe) a background
/// image texture. Replace it to switch themes; the old texture is freed.
pub struct ActiveTheme {
    pub palette: Palette,
    pub style: ThemeStyle,
    bg: Option<TextureHandle>,
}

impl ActiveTheme {
    /// The default look (Discord Dark), used when no custom theme applies.
    pub fn default_theme() -> Self {
        Self {
            palette: ThemeChoice::DiscordDark.palette(),
            style: ThemeStyle::default(),
            bg: None,
        }
    }

    /// Upload the image (once) and take the theme.
    pub fn from_loaded(ctx: &egui::Context, t: LoadedTheme) -> Self {
        let bg = t.image.map(|img| {
            let opts = match t.def.style.image.as_ref().map(|i| i.fit) {
                Some(ImageFit::Tile) => TextureOptions::LINEAR_REPEAT,
                _ => TextureOptions::LINEAR,
            };
            ctx.load_texture(format!("theme-bg-{}", t.id), img, opts)
        });
        Self {
            palette: t.def.palette,
            style: t.def.style,
            bg,
        }
    }

    /// The theme `config` asks for: the custom one if the module is on and it
    /// loads, else the default look. The error says why a custom theme didn't load.
    pub fn from_config(
        ctx: &egui::Context,
        config: &Config,
        store: &ThemeStore,
    ) -> (Self, Option<String>) {
        if config.modules.custom_themes
            && let Some(id) = &config.custom_theme
        {
            match store.load(id) {
                Ok(t) => return (Self::from_loaded(ctx, t), None),
                Err(e) => return (Self::default_theme(), Some(format!("{e:#}"))),
            }
        }
        (Self::default_theme(), None)
    }

    /// Set egui's visuals and text sizes. Call when the theme changes (and
    /// after anything else that calls `Palette::apply`).
    pub fn apply(&self, ctx: &egui::Context) {
        self.palette.apply(ctx);
        apply_style(ctx, &self.style);
    }

    /// Fill for the pane's `Frame`: transparent when
    /// `paint_pane_background` draws the background instead.
    pub fn pane_fill(&self, pane: Pane) -> Color32 {
        if self.paints(pane) {
            Color32::TRANSPARENT
        } else {
            self.palette.pane_fill(pane)
        }
    }

    fn paints(&self, pane: Pane) -> bool {
        self.style.pane(pane).is_custom() || (pane == Pane::Messages && self.bg.is_some())
    }

    /// Draw a pane's background (call before its contents, with the pane's
    /// full rect). Does nothing for panes a plain `Frame` fill covers.
    pub fn paint_pane_background(&self, painter: &Painter, rect: Rect, pane: Pane) {
        if !self.paints(pane) {
            return;
        }
        let window = painter.ctx().content_rect();
        paint_pane(painter, rect, window, pane, &self.palette, &self.style);
        if pane == Pane::Messages
            && let (Some(tex), Some(spec)) = (&self.bg, &self.style.image)
        {
            let ps = self.style.pane(pane);
            let r = rect.shrink(ps.inset as f32);
            let painter = painter.with_clip_rect(r.intersect(painter.clip_rect()));
            let size = tex.size_vec2();
            let uv = match spec.fit {
                ImageFit::Tile => {
                    Rect::from_min_max(Pos2::ZERO, pos2(r.width() / size.x, r.height() / size.y))
                }
                ImageFit::Cover => cover_uv(r.size(), size),
            };
            painter.image(tex.id(), r, uv, spec.tint.gamma_multiply(spec.opacity));
        }
    }

    /// The whole-window backdrop (behind inset / glass panes and in the gaps
    /// between tiles). Optional: panes paint their own part of it anyway.
    pub fn paint_backdrop(&self, painter: &Painter, rect: Rect) {
        if let Some(g) = &self.style.backdrop {
            gradient_rect(painter, rect, 0.0, 0.0, |y| {
                g.at((y - rect.top()) / rect.height().max(1.0))
            });
        }
    }
}

/// UV rect showing the centre of an image scaled to cover `area`.
fn cover_uv(area: egui::Vec2, img: egui::Vec2) -> Rect {
    let (aw, ah) = (area.x.max(1.0), area.y.max(1.0));
    let scale = (aw / img.x).max(ah / img.y);
    let (w, h) = (aw / (img.x * scale), ah / (img.y * scale));
    Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h))
}

/// Radii, shadows, link colour and font scale on top of `Palette::apply`.
pub fn apply_style(ctx: &egui::Context, s: &ThemeStyle) {
    let r = CornerRadius::same(s.widget_radius);
    let (wr, mr) = (s.window_radius, s.menu_radius);
    let (ws, ps, link, border) = (s.window_shadow, s.popup_shadow, s.link, s.widget_border);
    let scale = s.font_scale;
    ctx.global_style_mut(|st| {
        let v = &mut st.visuals;
        v.window_corner_radius = CornerRadius::same(wr);
        v.menu_corner_radius = CornerRadius::same(mr);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        if let Some(sh) = ws {
            v.window_shadow = sh;
        }
        if let Some(sh) = ps {
            v.popup_shadow = sh;
        }
        if let Some(l) = link {
            v.hyperlink_color = l;
        }
        if let Some(b) = border {
            let hot = v.selection.bg_fill;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, b);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, hot);
            v.widgets.active.bg_stroke = Stroke::new(1.0, hot);
            v.widgets.open.bg_stroke = Stroke::new(1.0, b);
        }
        if (scale - 1.0).abs() > f32::EPSILON {
            for font in st.text_styles.values_mut() {
                // Half-point steps keep the font atlas small.
                font.size = (font.size * scale * 2.0).round() / 2.0;
            }
        }
    });
}

// ---- Painting ---------------------------------------------------------------

/// Paint a pane's background: the backdrop slice (if the pane is inset or
/// glass), its shadow, then the pane's gradient / glass card. `window` is the
/// full window rect the backdrop gradient spans.
pub fn paint_pane(
    painter: &Painter,
    rect: Rect,
    window: Rect,
    pane: Pane,
    pal: &Palette,
    style: &ThemeStyle,
) {
    let ps = style.pane(pane);
    let base = ps
        .gradient
        .unwrap_or_else(|| Gradient::flat(pal.pane_fill(pane)));
    let see_through = ps.glass || ps.inset > 0;
    if see_through {
        match &style.backdrop {
            Some(g) => gradient_rect(painter, rect, 0.0, 0.0, |y| {
                g.at((y - window.top()) / window.height().max(1.0))
            }),
            // No backdrop: under glass shows the darkest panel colour.
            None => {
                painter.rect_filled(rect, 0, pal.servers);
            }
        }
    }
    let card = rect.shrink(ps.inset as f32);
    let r = ps.corner_radius as f32;
    if ps.inset > 0
        && let Some(sh) = style.panel_shadow
    {
        painter.add(sh.as_shape(card, CornerRadius::same(ps.corner_radius)));
    }
    let alpha = if ps.glass { style.glass_opacity } else { 1.0 };
    gradient_rect(painter, card, r, r, |y| {
        base.at((y - card.top()) / card.height().max(1.0))
            .gamma_multiply(alpha)
    });
    if ps.glass {
        glass_sheen(painter, card, r);
    }
}

/// Glossy top: a fading white band, a bright highlight line and a faint rim.
fn glass_sheen(painter: &Painter, card: Rect, r: f32) {
    let band = Rect::from_min_size(
        card.min,
        vec2(card.width(), (card.height() * 0.35).min(90.0)),
    );
    gradient_rect(painter, band, r, 0.0, |y| {
        let t = (y - band.top()) / band.height().max(1.0);
        Color32::from_white_alpha((70.0 * (1.0 - t)) as u8)
    });
    let x = (card.left() + r * 0.7)..=(card.right() - r * 0.7);
    painter.hline(
        x,
        card.top() + 1.0,
        Stroke::new(1.0, Color32::from_white_alpha(190)),
    );
    painter.rect_stroke(
        card,
        CornerRadius::same(r as u8),
        Stroke::new(1.0, Color32::from_white_alpha(60)),
        StrokeKind::Inside,
    );
}

/// Fill `rect` (rounded: `r_top` / `r_bottom` corner radii) with a colour
/// that depends on y. Built as horizontal strips with a one-pixel feathered
/// edge on the sides, so rounded corners stay smooth.
pub fn gradient_rect(
    painter: &Painter,
    rect: Rect,
    r_top: f32,
    r_bottom: f32,
    color_at: impl Fn(f32) -> Color32,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let half = rect.width().min(rect.height()) / 2.0;
    let (rt, rb) = (r_top.min(half), r_bottom.min(half));
    let feather = 1.0 / painter.ctx().pixels_per_point();

    // Rows: corners sampled finely, the straight part every 24 points (so
    // middle gradient stops come out right).
    let mut ys = Vec::with_capacity(48);
    let arc_steps = 8;
    for i in 0..=arc_steps {
        ys.push(rect.top() + rt * i as f32 / arc_steps as f32);
        ys.push(rect.bottom() - rb * i as f32 / arc_steps as f32);
    }
    let mut y = rect.top() + rt;
    while y < rect.bottom() - rb {
        ys.push(y);
        y += 24.0;
    }
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);

    let inset = |y: f32| -> f32 {
        let d = if y < rect.top() + rt {
            rt - (y - rect.top())
        } else if y > rect.bottom() - rb {
            rb - (rect.bottom() - y)
        } else {
            return 0.0;
        };
        let r = if y < rect.center().y { rt } else { rb };
        r - (r * r - d * d).max(0.0).sqrt()
    };

    let mut mesh = Mesh::default();
    for (row, &y) in ys.iter().enumerate() {
        let dx = inset(y);
        let c = color_at(y);
        let (l, rr) = (rect.left() + dx, rect.right() - dx);
        let ff = if rt > 0.0 || rb > 0.0 { feather } else { 0.0 };
        mesh.colored_vertex(pos2(l - ff, y), Color32::TRANSPARENT);
        mesh.colored_vertex(pos2(l, y), c);
        mesh.colored_vertex(pos2(rr, y), c);
        mesh.colored_vertex(pos2(rr + ff, y), Color32::TRANSPARENT);
        if row > 0 {
            let b = (row as u32 - 1) * 4;
            let t = row as u32 * 4;
            for k in 0..3 {
                mesh.add_triangle(b + k, b + k + 1, t + k);
                mesh.add_triangle(b + k + 1, t + k + 1, t + k);
            }
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// A tiny fake RustCord window in the theme's colours (Themes page cards).
pub fn paint_preview(painter: &Painter, rect: Rect, pal: &Palette, style: &ThemeStyle) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    // Scale the style down so the preview reads like a miniature.
    let mut s = style.clone();
    for p in &mut s.panes {
        p.corner_radius = p.corner_radius.min(6) / 2 + p.corner_radius.min(1);
        p.inset = p.inset.min(4) / 2;
    }
    s.panel_shadow = None;
    painter.rect_filled(rect, 0, pal.servers);
    if let Some(g) = &s.backdrop {
        gradient_rect(&painter, rect, 0.0, 0.0, |y| {
            g.at((y - rect.top()) / rect.height())
        });
    }
    let w = rect.width();
    let rail = Rect::from_min_size(rect.min, vec2(w * 0.11, rect.height()));
    let side = Rect::from_min_max(
        pos2(rail.right(), rect.top()),
        pos2(rect.left() + w * 0.36, rect.bottom()),
    );
    let comp_h = rect.height() * 0.17;
    let chat = Rect::from_min_max(
        pos2(side.right(), rect.top()),
        pos2(rect.right(), rect.bottom() - comp_h),
    );
    let comp = Rect::from_min_max(pos2(side.right(), chat.bottom()), rect.max);
    for (r, pane) in [
        (rail, Pane::Servers),
        (side, Pane::Channels),
        (chat, Pane::Messages),
        (comp, Pane::Composer),
    ] {
        if s.pane(pane).is_custom() {
            paint_pane(&painter, r, rect, pane, pal, &s);
        } else {
            painter.rect_filled(r, 0, pal.pane_fill(pane));
        }
    }
    // Rail: server icons.
    let d = rail.width() * 0.62;
    for i in 0..4 {
        let c = pos2(rail.center().x, rail.top() + 10.0 + i as f32 * (d + 5.0));
        let col = if i == 0 { pal.accent } else { pal.rail_icon };
        painter.circle_filled(c, d / 2.0, col);
    }
    // Sidebar: channel rows, one selected.
    for i in 0..5 {
        let y = side.top() + 12.0 + i as f32 * 12.0;
        let row = Rect::from_min_size(
            pos2(side.left() + 5.0, y - 4.5),
            vec2(side.width() - 10.0, 9.0),
        );
        if i == 1 {
            painter.rect_filled(row, 2, pal.selected);
        }
        let len = [0.6, 0.75, 0.5, 0.65, 0.45][i];
        let line = Rect::from_min_size(
            pos2(row.left() + 4.0, y - 1.5),
            vec2((row.width() - 8.0) * len, 3.0),
        );
        painter.rect_filled(line, 1, if i == 1 { pal.strong } else { pal.muted });
    }
    // Chat: avatar + name + text lines.
    for i in 0..3 {
        let y = chat.top() + 12.0 + i as f32 * 24.0;
        let av = pos2(chat.left() + 11.0, y + 4.0);
        let colors = [pal.online, pal.accent, pal.idle];
        painter.circle_filled(av, 6.0, colors[i]);
        let x = chat.left() + 22.0;
        painter.rect_filled(
            Rect::from_min_size(pos2(x, y), vec2(26.0, 3.5)),
            1,
            if i == 1 { pal.accent } else { pal.strong },
        );
        let len = [0.8, 0.55, 0.7][i];
        painter.rect_filled(
            Rect::from_min_size(pos2(x, y + 7.0), vec2((chat.right() - x - 8.0) * len, 3.0)),
            1,
            pal.text,
        );
    }
    // Composer: the input box and an accent "send" button.
    let input = comp.shrink2(vec2(6.0, comp.height() * 0.22));
    painter.rect_filled(input, style.widget_radius / 2 + 1, pal.input);
    let btn = Rect::from_min_size(
        pos2(input.right() - 22.0, input.top() + 2.0),
        vec2(19.0, input.height() - 4.0),
    );
    let g = style.accent_gradient.unwrap_or(Gradient::flat(pal.accent));
    gradient_rect(&painter, btn, 2.0, 2.0, |y| {
        g.at((y - btn.top()) / btn.height())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const THEME: &str = "format = 1\n[meta]\nname = \"Sky Blue!\"\nbase = \"light\"\n";

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn with_image() -> String {
        format!("{THEME}[images]\nchat_background = \"img/bg.png\"\nopacity = 0.2\n")
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Frutiger Aero!"), "frutiger-aero");
        assert_eq!(slug("  2000s   Messenger "), "2000s-messenger");
        assert_eq!(slug("✨✨"), "theme");
        assert!(valid_id("frutiger-aero-2"));
        for bad in ["", "..", "a/b", "A", ".x", "a\\b"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn import_file_list_load_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let themes = tmp.path().join("themes");
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("img")).unwrap();
        fs::write(src.join("sky.rctheme"), with_image()).unwrap();
        fs::write(src.join("img/bg.png"), png_bytes()).unwrap();

        let mut store = ThemeStore::new(Some(themes.clone()), true);
        assert!(store.entries.is_empty());
        let id = store.import(&src.join("sky.rctheme")).unwrap();
        assert_eq!(id, "sky-blue");
        assert!(themes.join("sky-blue/img/bg.png").is_file());
        assert!(matches!(store.notice, Some(Notice::Ok(_))));
        // Same name again gets a new id.
        assert_eq!(
            store.import(&src.join("sky.rctheme")).unwrap(),
            "sky-blue-2"
        );
        assert_eq!(store.entries.len(), 2);
        assert_eq!(store.get("sky-blue").unwrap().meta.name, "Sky Blue!");

        let loaded = store.load("sky-blue").unwrap();
        assert_eq!(loaded.image.as_ref().unwrap().size, [4, 3]);
        assert!(store.load("../etc").is_err());

        store.delete("sky-blue-2").unwrap();
        assert_eq!(store.entries.len(), 1);
        assert!(!themes.join("sky-blue-2").exists());
        assert!(store.delete("..").is_err());
        assert!(store.delete("missing").is_err());

        // Module off: nothing listed, nothing loaded, no imports.
        let mut off = ThemeStore::new(Some(themes.clone()), false);
        assert!(off.entries.is_empty());
        assert!(off.load("sky-blue").is_err());
        assert!(off.import(&src.join("sky.rctheme")).is_err());
        off.set_enabled(true);
        assert_eq!(off.entries.len(), 1);
    }

    #[test]
    fn import_folder_and_broken_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let themes = tmp.path().join("themes");
        let src = tmp.path().join("my-theme");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(THEME_FILE), THEME).unwrap();
        fs::write(src.join("notes.txt"), "not copied").unwrap();
        let id = import(&themes, &src).unwrap();
        assert!(themes.join(&id).join(THEME_FILE).is_file());
        assert!(!themes.join(&id).join("notes.txt").exists());

        // Missing image → clear error, nothing installed.
        fs::write(src.join(THEME_FILE), with_image()).unwrap();
        let e = import(&themes, &src).unwrap_err();
        assert!(format!("{e:#}").contains("background image"), "{e:#}");
        // Image that isn't one.
        fs::create_dir_all(src.join("img")).unwrap();
        fs::write(src.join("img/bg.png"), b"nope").unwrap();
        assert!(import(&themes, &src).is_err());
        // Bad theme text.
        fs::write(src.join(THEME_FILE), "format = 1\n").unwrap();
        assert!(import(&themes, &src).is_err());
        // Wrong file type.
        fs::write(tmp.path().join("x.txt"), THEME).unwrap();
        assert!(import(&themes, &tmp.path().join("x.txt")).is_err());

        // A broken theme already in the folder is listed with its error.
        fs::create_dir_all(themes.join("broken")).unwrap();
        fs::write(themes.join("broken").join(THEME_FILE), "format = 9").unwrap();
        let store = ThemeStore::new(Some(themes.clone()), true);
        assert_eq!(store.entries.len(), 2);
        let broken = store.entries.iter().find(|e| e.id == "broken").unwrap();
        assert!(broken.theme.as_ref().unwrap_err().contains("newer"));
        // No staging folders left behind.
        assert!(
            fs::read_dir(&themes)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with('.'))
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let themes = tmp.path().join("themes");
        let real = tmp.path().join("real.rctheme");
        fs::write(&real, THEME).unwrap();
        let link = tmp.path().join("link.rctheme");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(format!("{:#}", import(&themes, &link).unwrap_err()).contains("symlink"));

        let dir = tmp.path().join("t");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&real, dir.join(THEME_FILE)).unwrap();
        assert!(import(&themes, &dir).is_err());

        // Image behind a symlinked folder.
        let dir2 = tmp.path().join("t2");
        fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
        fs::write(tmp.path().join("elsewhere/bg.png"), png_bytes()).unwrap();
        fs::create_dir_all(&dir2).unwrap();
        fs::write(dir2.join(THEME_FILE), with_image()).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), dir2.join("img")).unwrap();
        assert!(format!("{:#}", import(&themes, &dir2).unwrap_err()).contains("symlink"));

        // Deleting a symlinked theme only removes the link.
        fs::create_dir_all(&themes).unwrap();
        std::os::unix::fs::symlink(&dir2, themes.join("linked")).unwrap();
        delete(&themes, "linked").unwrap();
        assert!(dir2.join(THEME_FILE).exists());
    }

    #[test]
    fn zip_import() {
        let tmp = tempfile::tempdir().unwrap();
        let themes = tmp.path().join("themes");
        let png = png_bytes();
        // Theme inside a top folder, with its image.
        let good = zip_of(&[
            ("sky/theme.rctheme", with_image().as_bytes()),
            ("sky/img/bg.png", &png),
            ("sky/README.md", b"hi"),
        ]);
        let z = tmp.path().join("sky.zip");
        fs::write(&z, &good).unwrap();
        let id = import(&themes, &z).unwrap();
        assert!(themes.join(&id).join("img/bg.png").is_file());
        assert!(!themes.join(&id).join("README.md").exists());
        assert!(load_theme(&themes, &id).unwrap().image.is_some());

        let check = |entries: &[(&str, &[u8])], needle: &str| {
            fs::write(&z, zip_of(entries)).unwrap();
            let e = format!("{:#}", import(&themes, &z).unwrap_err());
            assert!(e.contains(needle), "{needle}: {e}");
        };
        check(
            &[("theme.rctheme", THEME.as_bytes()), ("../evil.png", &png)],
            "outside",
        );
        check(
            &[("theme.rctheme", THEME.as_bytes()), ("/abs.png", &png)],
            "\"/abs.png\"",
        );
        check(&[("other.txt", b"x")], "no theme.rctheme");
        check(
            &[("theme.rctheme", with_image().as_bytes())],
            "isn't in the zip",
        );
        let big = vec![b'#'; 70 * 1024];
        check(&[("theme.rctheme", &big)], "too big");
        let many: Vec<(String, Vec<u8>)> = (0..70).map(|i| (format!("f{i}"), vec![])).collect();
        let many_ref: Vec<(&str, &[u8])> = many
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        check(&many_ref, "too many");
        fs::write(&z, b"not a zip").unwrap();
        assert!(format!("{:#}", import(&themes, &z).unwrap_err()).contains("zip"));

        // A symlink entry rejects the archive.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("theme.rctheme", opts).unwrap();
        w.write_all(THEME.as_bytes()).unwrap();
        w.add_symlink("img", "/etc", opts).unwrap();
        fs::write(&z, w.finish().unwrap().into_inner()).unwrap();
        assert!(format!("{:#}", import(&themes, &z).unwrap_err()).contains("symlink"));
    }

    #[test]
    fn image_decode_caps() {
        let img = image::RgbaImage::from_pixel(3200, 10, image::Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        let c = decode_image(out.get_ref()).unwrap();
        assert_eq!(c.size[0], MAX_IMAGE_SIDE as usize);
        assert!(decode_image(b"GIF89a....").is_err());
        assert!(decode_image(&[]).is_err());
    }

    #[test]
    fn cover_uv_centres() {
        let uv = cover_uv(vec2(200.0, 100.0), vec2(100.0, 100.0));
        assert!((uv.width() - 1.0).abs() < 1e-4 && (uv.height() - 0.5).abs() < 1e-4);
        assert!((uv.center().y - 0.5).abs() < 1e-4);
    }
}
