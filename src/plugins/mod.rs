//! Plugins: user-installed Luau scripts, each in its own sandboxed VM.
//!
//! The app talks to plugins only through [`PluginHost`]. Plugins can read
//! what they're given, change how messages look locally, add UI drawn by
//! Rust, and propose text for the message box. They can never send, react,
//! join, change settings or touch the network, files or the token.
//! See `docs/PLUGINS.md` for the Lua API and the integration notes below.
//!
//! # Integration (app.rs)
//!
//! Add `plugins: bool` (default **false**) to `config::Modules` and a
//! `MODULES` entry ("Plugins", "Run Luau plugins you install (Settings →
//! Plugins). Each runs sandboxed with only the permissions you approve. Off =
//! no plugin is loaded."). Own one `PluginHost` (`PluginHost::new()`, cheap:
//! nothing is read until the module is on) next to `config` in
//! `RustCordApp`, then:
//!
//! | Where | Call |
//! |---|---|
//! | `App::logic`, every frame (Main screen) | `host.update(ctx, self.config.modules.plugins)` (also on the login screen is fine; off = drops every VM) |
//! | `ToUi::Ready` | `host.on_ready(&me)` |
//! | `ToUi::NewMessage` | `host.on_message(&msg)` |
//! | `ToUi::MessageUpdated` | `host.on_message_update(&patch)` |
//! | opening a channel | `host.on_channel_open(id, guild_id, name)` |
//! | `messages_pane` first pass (filling `md_cache`) | `host.transform_display(&msg).unwrap_or(content)`; clear `md_cache` when `host.display_generation()` changes |
//! | composer submit (`State::submit`) | if `PluginHost::parse_command(draft)` names one of `host.commands()`, call `host.run_command(..)` and put the `Draft` back in the composer instead of sending |
//! | autocomplete (optional) | `host.commands()` for `/` suggestions |
//! | a `Pane::Plugins` tile | `host.draw_panels(ui, &pal)` (or `draw_panel` per `host.panels()`) |
//! | each frame | `for n in host.drain_notifications()` → show as a toast |
//! | Settings → Plugins section | `host.settings_ui(ui, draft.modules.plugins, &pal)`; `PluginsAction::Notice` → toast, `PluginsAction::SetModule(on)` → set `modules.plugins` and save |
//! | `App::save` / on exit | `host.save()` |

// TODO(integration): remove once app.rs calls the host.
#![allow(dead_code)]

pub mod install;
pub mod manager;
pub mod manifest;
pub mod runtime;
pub mod storage;
mod ui;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use mlua::{Lua, Table, Value};
use serde::{Deserialize, Serialize};

use crate::theme::Palette;
use crate::types::{Message, MessagePatch, User};

use manager::{Manager, PendingInstall};
pub use manifest::{Manifest, Permission};
use runtime::{Event, UiTarget};

/// Total time plugins may spend per frame drawing panels and transforming
/// messages; past it, they're skipped until the next frame.
const FRAME_BUDGET: Duration = Duration::from_millis(100);
/// Cached display transforms (message id → result).
const DISPLAY_CACHE_CAP: usize = 4000;

/// A plugin the user enabled, with the permissions they approved. Saved in
/// `<data>/plugin-data/_enabled.json` (the host owns this list, so
/// `Config` only needs the `Modules::plugins` switch). A plugin only runs if its manifest asks for no
/// more than `permissions` (an update asking for more needs re-approval).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginGrant {
    pub id: String,
    pub permissions: Vec<Permission>,
}

impl PluginGrant {
    pub fn covers(&self, m: &Manifest) -> bool {
        self.id == m.id && m.permissions.iter().all(|p| self.permissions.contains(p))
    }
}

/// A `/command` registered by a plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    /// Display name of the plugin.
    pub plugin: String,
}

/// What running a command produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Put this in the message box (the user still presses Enter to send).
    Draft(String),
    /// The command ran and returned nothing: clear the message box.
    NoDraft,
    /// It failed; show the error and leave the message box as it was.
    Error(String),
}

/// Identifies a plugin panel. Stable while the plugin is loaded.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PanelId {
    pub plugin: String,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub struct PanelInfo {
    pub id: PanelId,
    pub title: String,
    pub plugin_name: String,
}

/// A notice a plugin asked to show (`rustcord.notify`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginNotification {
    pub plugin: String,
    pub text: String,
}

/// Returned by [`PluginHost::settings_ui`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginsAction {
    /// Show this as a toast (install / uninstall results).
    Notice(String),
    /// The page's module switch was flipped: set `Modules::plugins` to this
    /// and save the config. (The same switch is in Settings → Modules.)
    SetModule(bool),
}

pub struct PluginHost {
    mgr: Manager,
    active: bool,
    me: Option<User>,
    display_gen: u64,
    display_cache: HashMap<String, (u64, Option<String>)>,
    notices: Vec<PluginNotification>,
    frame_spent: Duration,
    last_flush: Instant,
    picker: Option<mpsc::Receiver<Option<PathBuf>>>,
    selected_panel: Option<PanelId>,
    ui: ui::PageState,
    /// Enabled plugins + approved permissions (None = not read yet).
    grants: Option<Vec<PluginGrant>>,
    grants_path: PathBuf,
}

/// RustCord's data folder (`eframe::storage_dir`), e.g.
/// `%APPDATA%\RustCord\data` on Windows, `~/.local/share/rustcord` on Linux.
fn data_root() -> PathBuf {
    eframe::storage_dir("RustCord").unwrap_or_else(|| PathBuf::from("rustcord-data"))
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    /// Host using the standard folders (`<data>/plugins`,
    /// `<data>/plugin-data`). Reads nothing until the module is on.
    pub fn new() -> Self {
        let root = data_root();
        Self::with_dirs(root.join("plugins"), root.join("plugin-data"))
    }

    pub fn with_dirs(plugins_dir: PathBuf, data_dir: PathBuf) -> Self {
        PluginHost {
            grants: None,
            grants_path: data_dir.join("_enabled.json"),
            mgr: Manager::new(plugins_dir, data_dir),
            active: false,
            me: None,
            display_gen: 0,
            display_cache: HashMap::new(),
            notices: Vec::new(),
            frame_spent: Duration::ZERO,
            last_flush: Instant::now(),
            picker: None,
            selected_panel: None,
            ui: ui::PageState::default(),
        }
    }

    pub fn plugins_dir(&self) -> &std::path::Path {
        &self.mgr.plugins_dir
    }

    /// True while the Plugins module is on (VMs may exist).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Once per frame (from `App::logic`), with `config.modules.plugins`:
    /// follow the module switch and the enabled list, finish installs,
    /// apply auto-disables, save storage.
    pub fn update(&mut self, ctx: &egui::Context, module_on: bool) {
        self.frame_spent = Duration::ZERO;
        if !module_on {
            if self.active {
                self.save();
                self.shutdown_vms();
                self.active = false;
            }
            self.forget_images(ctx);
            return;
        }
        if !self.active {
            self.active = true;
            self.mgr.scan();
        }
        self.poll_picker();
        // Auto-disabled plugins lose their grant (the user re-approves).
        let off: Vec<String> = self
            .mgr
            .entries
            .iter()
            .filter(|e| e.auto_disabled.is_some())
            .map(|e| e.id.clone())
            .collect();
        for id in off {
            if self.grant(&id).is_some() {
                self.revoke(&id);
            }
        }
        self.sync();
        self.forget_images(ctx);
        if self.last_flush.elapsed() > Duration::from_secs(5) {
            self.save();
        }
    }

    fn grants_mut(&mut self) -> &mut Vec<PluginGrant> {
        let path = &self.grants_path;
        self.grants.get_or_insert_with(|| {
            std::fs::read(path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        })
    }

    /// The approved permissions of plugin `id`, if it's enabled.
    pub fn grant(&mut self, id: &str) -> Option<PluginGrant> {
        self.grants_mut().iter().find(|g| g.id == id).cloned()
    }

    /// Enable `id` with these permissions (saved at once).
    fn set_grant(&mut self, grant: PluginGrant) {
        let grants = self.grants_mut();
        grants.retain(|g| g.id != grant.id);
        grants.push(grant);
        self.save_grants();
    }

    /// Disable `id` (saved at once).
    fn revoke(&mut self, id: &str) {
        self.grants_mut().retain(|g| g.id != id);
        self.save_grants();
    }

    fn save_grants(&mut self) {
        let Some(grants) = &self.grants else {
            return;
        };
        if let Some(dir) = self.grants_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(grants) {
            let tmp = self.grants_path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.grants_path);
            }
        }
    }

    /// Load / unload to match `grants`, deliver `ready` to new VMs, and
    /// invalidate cached display text if anything changed.
    fn sync(&mut self) {
        let grants = self.grants_mut().clone();
        let loaded = self.mgr.reconcile(&grants);
        if let Some(me) = self.me.clone() {
            for i in loaded {
                self.fire_one(i, Event::Ready, |lua| user_table(lua, &me));
            }
        }
        let mut storage_changed = false;
        for (_, rt) in self.mgr.running() {
            storage_changed |= rt.take_storage_changed() && rt.has_display();
        }
        if storage_changed || std::mem::take(&mut self.mgr.changed) {
            self.invalidate_display();
        }
        self.collect_notices();
    }

    fn shutdown_vms(&mut self) {
        self.mgr.unload_all();
        self.display_cache = HashMap::new();
        self.display_gen += 1;
        self.notices.clear();
        self.selected_panel = None;
        self.ui = ui::PageState::default();
    }

    fn forget_images(&mut self, ctx: &egui::Context) {
        for url in self.mgr.forget_images.drain(..) {
            ctx.forget_image(&url);
        }
    }

    fn invalidate_display(&mut self) {
        self.display_cache.clear();
        self.display_gen += 1;
    }

    /// Changes whenever transformed message text may differ (plugin loaded,
    /// unloaded or its storage changed). Re-parse cached messages then.
    pub fn display_generation(&self) -> u64 {
        self.display_gen
    }

    /// Save every plugin's storage (call from `App::save`).
    pub fn save(&mut self) {
        self.last_flush = Instant::now();
        let mut errors = Vec::new();
        for (i, rt) in self.mgr.running() {
            if rt.storage_dirty()
                && let Err(e) = rt.flush_storage()
            {
                errors.push((i, e));
            }
        }
        for (i, e) in errors {
            self.mgr.entries[i].log.borrow_mut().push(e, true);
        }
    }

    fn collect_notices(&mut self) {
        for (i, rt) in self.mgr.running() {
            for text in rt.take_notices() {
                self.notices.push(PluginNotification {
                    plugin: self.mgr.entries[i].name().to_string(),
                    text,
                });
            }
        }
    }

    /// Notices plugins asked to show since the last call.
    pub fn drain_notifications(&mut self) -> Vec<PluginNotification> {
        self.collect_notices();
        std::mem::take(&mut self.notices)
    }

    fn fire_one(&mut self, i: usize, ev: Event, arg: impl FnOnce(&Lua) -> mlua::Result<Value>) {
        let Some(rt) = &self.mgr.entries[i].runtime else {
            return;
        };
        let errors = rt.fire(ev, arg);
        for e in errors {
            if self.mgr.record_error(i, &format!("{ev:?} handler"), &e) {
                break;
            }
        }
    }

    fn fire_all(&mut self, ev: Event, arg: impl Fn(&Lua) -> mlua::Result<Value>) {
        if !self.active {
            return;
        }
        let targets: Vec<usize> = self
            .mgr
            .running()
            .filter(|(_, rt)| rt.has_handlers(ev))
            .map(|(i, _)| i)
            .collect();
        for i in targets {
            self.fire_one(i, ev, &arg);
        }
    }

    /// READY: your own account (id, name, username).
    pub fn on_ready(&mut self, me: &User) {
        self.me = Some(me.clone());
        self.fire_all(Event::Ready, |lua| user_table(lua, me));
    }

    /// A new message arrived (plugins with `read_messages`).
    pub fn on_message(&mut self, msg: &Message) {
        self.fire_all(Event::Message, |lua| {
            message_table(lua, msg, &msg.content).map(Value::Table)
        });
    }

    /// A message was edited (plugins with `read_messages`).
    pub fn on_message_update(&mut self, patch: &MessagePatch) {
        self.fire_all(Event::MessageUpdate, |lua| {
            let t = lua.create_table()?;
            t.raw_set("id", patch.id.as_str())?;
            t.raw_set("channel_id", patch.channel_id.as_str())?;
            t.raw_set("content", patch.content.as_deref())?;
            t.raw_set("edited", patch.edited)?;
            Ok(Value::Table(t))
        });
    }

    /// The user opened a channel.
    pub fn on_channel_open(&mut self, channel_id: &str, guild_id: Option<&str>, name: &str) {
        self.fire_all(Event::ChannelOpen, |lua| {
            let t = lua.create_table()?;
            t.raw_set("id", channel_id)?;
            t.raw_set("guild_id", guild_id)?;
            t.raw_set("name", name)?;
            Ok(Value::Table(t))
        });
    }

    /// Text to show instead of `msg.content`, if a `message_display`
    /// plugin changed it. Cached per message; render the result through the
    /// normal markdown path (links still go through `open_link`).
    pub fn transform_display(&mut self, msg: &Message) -> Option<String> {
        if !self.active {
            return None;
        }
        let targets: Vec<usize> = self
            .mgr
            .running()
            .filter(|(_, rt)| rt.has_display())
            .map(|(i, _)| i)
            .collect();
        if targets.is_empty() {
            return None;
        }
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            msg.content.hash(&mut h);
            msg.edited.hash(&mut h);
            h.finish()
        };
        if let Some((h, out)) = self.display_cache.get(&msg.id)
            && *h == hash
        {
            return out.clone();
        }
        if self.frame_spent > FRAME_BUDGET {
            return None; // Try again next frame; don't cache.
        }
        let start = Instant::now();
        let mut content = msg.content.clone();
        let mut changed = false;
        for i in targets {
            let Some(rt) = &self.mgr.entries[i].runtime else {
                continue;
            };
            let result = message_table(rt.lua(), msg, &content)
                .map_err(|e| runtime::clean_error(&e))
                .and_then(|t| rt.display(t));
            match result {
                Ok(Some(text)) => {
                    changed |= text != content;
                    content = text;
                }
                Ok(None) => {}
                Err(e) => {
                    self.mgr.record_error(i, "display", &e);
                }
            }
        }
        self.frame_spent += start.elapsed();
        let out = changed.then_some(content);
        if self.display_cache.len() >= DISPLAY_CACHE_CAP {
            self.display_cache.clear();
        }
        self.display_cache
            .insert(msg.id.clone(), (hash, out.clone()));
        out
    }

    /// Every plugin `/command` (first plugin wins on duplicate names).
    pub fn commands(&self) -> Vec<CommandInfo> {
        let mut out: Vec<CommandInfo> = Vec::new();
        for (i, rt) in self.mgr.running() {
            for (name, description) in rt.commands() {
                if !out.iter().any(|c| c.name == name) {
                    out.push(CommandInfo {
                        name,
                        description,
                        plugin: self.mgr.entries[i].name().to_string(),
                    });
                }
            }
        }
        out
    }

    /// `"/shrug hi"` → `("shrug", "hi")`. None if it isn't a `/command`.
    pub fn parse_command(draft: &str) -> Option<(&str, &str)> {
        let rest = draft.trim_start().strip_prefix('/')?;
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let ok = !name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
        ok.then_some((name, args.trim()))
    }

    /// Run a plugin command. None: no plugin has `name` (send the text as
    /// usual). The result is only ever text for the message box.
    pub fn run_command(
        &mut self,
        name: &str,
        args: &str,
        channel_id: &str,
        guild_id: Option<&str>,
    ) -> Option<CommandOutcome> {
        if !self.active {
            return None;
        }
        let running: Vec<usize> = self.mgr.running().map(|(i, _)| i).collect();
        for i in running {
            let Some(rt) = &self.mgr.entries[i].runtime else {
                continue;
            };
            let Some(result) = rt.run_command(name, args, |lua| {
                let t = lua.create_table()?;
                t.raw_set("channel_id", channel_id)?;
                t.raw_set("guild_id", guild_id)?;
                Ok(Value::Table(t))
            }) else {
                continue;
            };
            self.collect_notices();
            return Some(match result {
                Ok(Some(text)) => CommandOutcome::Draft(text),
                Ok(None) => CommandOutcome::NoDraft,
                Err(e) => {
                    self.mgr.record_error(i, &format!("/{name}"), &e);
                    CommandOutcome::Error(e)
                }
            });
        }
        None
    }

    /// Panels added by plugins (`rustcord.ui.panel`).
    pub fn panels(&self) -> Vec<PanelInfo> {
        let mut out = Vec::new();
        for (i, rt) in self.mgr.running() {
            let e = &self.mgr.entries[i];
            for (index, title) in rt.panel_titles().into_iter().enumerate() {
                out.push(PanelInfo {
                    id: PanelId {
                        plugin: e.id.clone(),
                        index,
                    },
                    title,
                    plugin_name: e.name().to_string(),
                });
            }
        }
        out
    }

    /// Draw one plugin panel.
    pub fn draw_panel(&mut self, ui: &mut egui::Ui, id: &PanelId, pal: &Palette) {
        let Some(i) = self.mgr.index(&id.plugin) else {
            return;
        };
        self.draw_ui(ui, i, UiTarget::Panel(id.index), pal);
    }

    /// All plugin panels in one place: a row of tabs and the selected one.
    pub fn draw_panels(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let panels = self.panels();
        if panels.is_empty() {
            ui.label(
                egui::RichText::new(if self.active {
                    "No plugin panels. Plugins with the \"ui\" permission can add panels here."
                } else {
                    "Turn on the Plugins module (Settings → Modules) to use plugins."
                })
                .color(pal.muted),
            );
            return;
        }
        if !self
            .selected_panel
            .as_ref()
            .is_some_and(|s| panels.iter().any(|p| &p.id == s))
        {
            self.selected_panel = Some(panels[0].id.clone());
        }
        if panels.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for p in &panels {
                    let on = self.selected_panel.as_ref() == Some(&p.id);
                    if ui
                        .selectable_label(on, &p.title)
                        .on_hover_text(&p.plugin_name)
                        .clicked()
                    {
                        self.selected_panel = Some(p.id.clone());
                    }
                }
            });
            ui.separator();
        }
        if let Some(id) = self.selected_panel.clone() {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| self.draw_panel(ui, &id, pal));
        }
    }

    /// Build and draw a panel / settings UI of plugin `i`, then run the
    /// clicked button's callback.
    fn draw_ui(&mut self, ui: &mut egui::Ui, i: usize, target: UiTarget, pal: &Palette) {
        let Some(rt) = &self.mgr.entries[i].runtime else {
            return;
        };
        if self.frame_spent > FRAME_BUDGET {
            ui.label(egui::RichText::new("…").color(pal.muted));
            ui.ctx().request_repaint();
            return;
        }
        let start = Instant::now();
        let built = rt.build_ui(target);
        let salt = (&self.mgr.entries[i].id, format!("{target:?}"));
        let mut errors = Vec::new();
        match built {
            Ok((widgets, callbacks)) => {
                let (clicked, errs) = ui::render_widgets(ui, rt, &widgets, salt, pal);
                errors.extend(errs);
                if let Some(cb) = clicked.and_then(|c| callbacks.get(c))
                    && let Err(e) = rt.call(cb)
                {
                    errors.push(e);
                }
            }
            Err(e) => {
                ui.label(egui::RichText::new(format!("Plugin error: {e}")).color(pal.danger));
                errors.push(e);
            }
        }
        self.frame_spent += start.elapsed();
        for e in errors {
            if self.mgr.record_error(i, "ui", &e) {
                break;
            }
        }
    }

    /// Settings → Plugins page: the module switch, installed plugins
    /// (toggle, permissions, version / author, folder, reload, log,
    /// uninstall) and "Install plugin…" with the approval dialog.
    pub fn settings_ui(
        &mut self,
        ui: &mut egui::Ui,
        module_on: bool,
        pal: &Palette,
    ) -> Option<PluginsAction> {
        ui::settings_page(self, ui, module_on, pal)
    }

    /// Open the system file picker on a thread (a `.zip` or a folder).
    fn pick_install_source(&mut self, ctx: &egui::Context, zip: bool) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("plugin-picker".into())
            .spawn(move || {
                let dialog = rfd::AsyncFileDialog::new().set_title("Install plugin");
                let picked = if zip {
                    block_on(dialog.add_filter("Plugin (.zip)", &["zip"]).pick_file())
                } else {
                    block_on(dialog.pick_folder())
                };
                let _ = tx.send(picked.map(|f| f.path().to_path_buf()));
                ctx.request_repaint();
            });
        if spawned.is_ok() {
            self.picker = Some(rx);
        }
    }

    fn poll_picker(&mut self) {
        let Some(rx) = &self.picker else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(path)) => {
                self.picker = None;
                self.begin_install(&path);
            }
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => self.picker = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Stage a plugin from a folder or zip and ask the user to approve it.
    pub fn begin_install(&mut self, path: &std::path::Path) {
        match self.mgr.stage_install(path) {
            Ok(p) => {
                if let Some(old) = self.ui.pending.take() {
                    self.mgr.cancel_install(old);
                }
                self.ui.pending = Some(p);
            }
            Err(e) => self.ui.status = Some((format!("Couldn't install: {e}"), true)),
        }
    }

    fn finish_install(&mut self, p: PendingInstall, enable: bool) -> String {
        let name = p.manifest.name.clone();
        let grant = PluginGrant {
            id: p.manifest.id.clone(),
            permissions: p.manifest.permissions.clone(),
        };
        match self.mgr.finish_install(p) {
            Ok(id) => {
                if enable {
                    self.set_grant(grant);
                } else {
                    self.revoke(&id);
                }
                self.sync();
                format!("Installed {name}")
            }
            Err(e) => e,
        }
    }
}

/// `{ id, name, username, bot }`.
fn user_table(lua: &Lua, u: &User) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    t.raw_set("id", u.id.as_str())?;
    t.raw_set("name", u.name.as_str())?;
    t.raw_set("username", u.username.as_str())?;
    t.raw_set("bot", u.bot)?;
    Ok(Value::Table(t))
}

/// The message table plugins see: `{ id, channel_id, guild_id, author =
/// { id, name, username, bot }, content, timestamp, edited }`.
fn message_table(lua: &Lua, msg: &Message, content: &str) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set("id", msg.id.as_str())?;
    t.raw_set("channel_id", msg.channel_id.as_str())?;
    t.raw_set("guild_id", msg.guild_id.as_deref())?;
    t.raw_set("author", user_table(lua, &msg.author)?)?;
    t.raw_set("content", content)?;
    t.raw_set("timestamp", msg.time)?;
    t.raw_set("edited", msg.edited)?;
    Ok(t)
}

/// Minimal executor for the file dialog future (on its own thread).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// Open a folder in the system file manager.
fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}
