//! Installed plugins on disk and their VMs: scan, load / unload, install,
//! uninstall, reload, error counting and auto-disable.
//!
//! Layout:
//! - `<plugins_dir>/<id>/plugin.toml` + scripts (folder name == id)
//! - `<data_dir>/<id>/storage.json` (kept apart so reinstalling / updating
//!   a plugin keeps its data; uninstalling deletes both)

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::PluginGrant;
use super::install;
use super::manifest::{Manifest, valid_id};
use super::runtime::{LogRing, Runtime};
use super::storage::Storage;

/// This many errors within [`ERROR_WINDOW`] switch a plugin off.
pub const MAX_ERRORS: usize = 5;
pub const ERROR_WINDOW: Duration = Duration::from_secs(60);

pub struct PluginEntry {
    /// Folder name (equals the manifest id when the manifest is valid).
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
    pub runtime: Option<Runtime>,
    /// Why the last load failed.
    pub load_error: Option<String>,
    /// Set when it was switched off for erroring too often.
    pub auto_disabled: Option<String>,
    pub log: Rc<RefCell<LogRing>>,
    errors: VecDeque<Instant>,
    /// A load was tried (don't retry every frame after a failure).
    load_attempted: bool,
}

impl PluginEntry {
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }

    pub fn is_running(&self) -> bool {
        self.runtime.is_some()
    }
}

/// A plugin copied into a staging folder, waiting for the user's approval.
pub struct PendingInstall {
    pub manifest: Manifest,
    /// Version of the installed plugin it replaces.
    pub replaces: Option<String>,
    staging: PathBuf,
    root: PathBuf,
}

pub struct Manager {
    pub plugins_dir: PathBuf,
    pub data_dir: PathBuf,
    pub entries: Vec<PluginEntry>,
    /// Image URLs of unloaded plugins, for the host to free.
    pub forget_images: Vec<String>,
    /// Something loaded / unloaded (commands, panels, display may differ).
    pub changed: bool,
}

impl Manager {
    pub fn new(plugins_dir: PathBuf, data_dir: PathBuf) -> Self {
        Manager {
            plugins_dir,
            data_dir,
            entries: Vec::new(),
            forget_images: Vec::new(),
            changed: false,
        }
    }

    /// (Re)read the plugins folder. Keeps loaded VMs and logs of plugins
    /// that are still there.
    pub fn scan(&mut self) {
        let _ = fs::create_dir_all(&self.plugins_dir);
        let mut found = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.plugins_dir) {
            for e in rd.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(".staging-") {
                    // Leftover from an interrupted install.
                    let _ = fs::remove_dir_all(e.path());
                    continue;
                }
                if name.starts_with('.') || !e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                found.push((name, e.path()));
            }
        }
        found.sort();
        let mut old: Vec<PluginEntry> = std::mem::take(&mut self.entries);
        for (id, dir) in found {
            let manifest = read_entry_manifest(&id, &dir);
            if let Some(pos) = old.iter().position(|e| e.id == id) {
                let mut e = old.swap_remove(pos);
                if e.manifest != manifest {
                    self.unload_entry(&mut e);
                    e.manifest = manifest;
                    e.load_attempted = false;
                }
                self.entries.push(e);
            } else {
                self.entries.push(PluginEntry {
                    id,
                    dir,
                    manifest,
                    runtime: None,
                    load_error: None,
                    auto_disabled: None,
                    log: Rc::default(),
                    errors: VecDeque::new(),
                    load_attempted: false,
                });
            }
        }
        for mut gone in old {
            self.unload_entry(&mut gone);
        }
    }

    /// Load / unload so that exactly the approved plugins run.
    /// Returns the indexes of plugins that were just loaded.
    pub fn reconcile(&mut self, grants: &[PluginGrant]) -> Vec<usize> {
        let mut loaded = Vec::new();
        for i in 0..self.entries.len() {
            let want = {
                let e = &self.entries[i];
                e.auto_disabled.is_none()
                    && e.manifest
                        .as_ref()
                        .is_ok_and(|m| grants.iter().any(|g| g.covers(m)))
            };
            let running = self.entries[i].runtime.is_some();
            if want && !running && !self.entries[i].load_attempted {
                if self.load(i) {
                    loaded.push(i);
                }
            } else if !want && running {
                self.unload_index(i);
            }
        }
        loaded
    }

    fn load(&mut self, i: usize) -> bool {
        let data = self.data_dir.join(&self.entries[i].id).join("storage.json");
        let e = &mut self.entries[i];
        e.load_attempted = true;
        let Ok(m) = &e.manifest else {
            return false;
        };
        match Runtime::load(m, &e.dir, Storage::open(data), e.log.clone()) {
            Ok(rt) => {
                e.runtime = Some(rt);
                e.load_error = None;
                e.log
                    .borrow_mut()
                    .push(format!("Loaded {} {}", m.name, m.version), false);
                self.changed = true;
                true
            }
            Err(err) => {
                e.log
                    .borrow_mut()
                    .push(format!("Failed to load: {err}"), true);
                e.load_error = Some(err);
                false
            }
        }
    }

    fn unload_entry(&mut self, e: &mut PluginEntry) {
        if unload(e, &mut self.forget_images) {
            self.changed = true;
        }
    }

    fn unload_index(&mut self, i: usize) {
        if unload(&mut self.entries[i], &mut self.forget_images) {
            self.changed = true;
        }
    }

    pub fn unload_all(&mut self) {
        let mut entries = std::mem::take(&mut self.entries);
        for e in &mut entries {
            self.unload_entry(e);
        }
    }

    /// Unload and re-read one plugin (it loads again on the next
    /// `reconcile` if still approved).
    pub fn reload(&mut self, id: &str) {
        if let Some(i) = self.index(id) {
            self.unload_index(i);
            let e = &mut self.entries[i];
            e.manifest = read_entry_manifest(&e.id, &e.dir);
            e.load_attempted = false;
            e.load_error = None;
            e.log.borrow_mut().push("Reloading", false);
        }
    }

    /// Clear a plugin's error history so it can load again (after the user
    /// re-enables it).
    pub fn reset(&mut self, id: &str) {
        if let Some(i) = self.index(id) {
            let e = &mut self.entries[i];
            e.auto_disabled = None;
            e.errors.clear();
            e.load_attempted = false;
            e.load_error = None;
        }
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// Note an error from a call into plugin `i`. Too many in a short time
    /// unload it and mark it auto-disabled; returns true then.
    pub fn record_error(&mut self, i: usize, context: &str, err: &str) -> bool {
        let now = Instant::now();
        let e = &mut self.entries[i];
        e.log
            .borrow_mut()
            .push(format!("Error in {context}: {err}"), true);
        e.errors.push_back(now);
        while e
            .errors
            .front()
            .is_some_and(|t| now.duration_since(*t) > ERROR_WINDOW)
        {
            e.errors.pop_front();
        }
        if e.errors.len() < MAX_ERRORS {
            return false;
        }
        let reason = format!(
            "Turned off after {MAX_ERRORS} errors in a minute. Last: {}",
            err.lines().next().unwrap_or(err)
        );
        e.log.borrow_mut().push(reason.clone(), true);
        e.auto_disabled = Some(reason);
        self.unload_index(i);
        true
    }

    /// Copy / extract `src` into a staging folder and validate it.
    pub fn stage_install(&self, src: &Path) -> Result<PendingInstall, String> {
        let _ = fs::create_dir_all(&self.plugins_dir);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let staging = self.plugins_dir.join(format!(".staging-{nanos}"));
        match install::stage(src, &staging) {
            Ok((root, manifest)) => {
                let replaces = self
                    .entries
                    .iter()
                    .find(|e| e.id == manifest.id)
                    .map(|e| match &e.manifest {
                        Ok(m) => m.version.clone(),
                        Err(_) => "?".into(),
                    });
                Ok(PendingInstall {
                    manifest,
                    replaces,
                    staging,
                    root,
                })
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                Err(e)
            }
        }
    }

    pub fn cancel_install(&self, p: PendingInstall) {
        let _ = fs::remove_dir_all(&p.staging);
    }

    /// Move a staged plugin into place (replacing an older copy).
    pub fn finish_install(&mut self, p: PendingInstall) -> Result<String, String> {
        let id = p.manifest.id.clone();
        if !valid_id(&id) {
            self.cancel_install(p);
            return Err("Invalid plugin id".into());
        }
        let target = self.plugins_dir.join(&id);
        if let Some(i) = self.index(&id) {
            self.unload_index(i);
        }
        if target.exists()
            && let Err(e) = fs::remove_dir_all(&target)
        {
            self.cancel_install(p);
            return Err(format!("Couldn't replace the old version: {e}"));
        }
        let moved = fs::rename(&p.root, &target);
        let _ = fs::remove_dir_all(&p.staging);
        moved.map_err(|e| format!("Couldn't install: {e}"))?;
        self.scan();
        self.reset(&id);
        self.changed = true;
        Ok(id)
    }

    /// Delete a plugin's folder and data.
    pub fn uninstall(&mut self, id: &str) -> Result<(), String> {
        if !valid_id(id) {
            return Err("Invalid plugin id".into());
        }
        if let Some(i) = self.index(id) {
            let mut e = self.entries.remove(i);
            self.unload_entry(&mut e);
        }
        let dir = self.plugins_dir.join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete the plugin: {e}"))?;
        }
        let _ = fs::remove_dir_all(self.data_dir.join(id));
        self.changed = true;
        Ok(())
    }

    pub fn running(&self) -> impl Iterator<Item = (usize, &Runtime)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.runtime.as_ref().map(|r| (i, r)))
    }
}

fn read_entry_manifest(id: &str, dir: &Path) -> Result<Manifest, String> {
    let m = install::read_manifest(dir)?;
    if m.id != id {
        return Err(format!(
            "Folder name \"{id}\" doesn't match the plugin id \"{}\"",
            m.id
        ));
    }
    Ok(m)
}

/// Drop a plugin's VM (saving its storage). True if it was running.
fn unload(e: &mut PluginEntry, forget_images: &mut Vec<String>) -> bool {
    let Some(rt) = e.runtime.take() else {
        return false;
    };
    forget_images.extend(rt.image_urls());
    if let Err(err) = rt.flush_storage() {
        e.log.borrow_mut().push(err, true);
    }
    true
}
