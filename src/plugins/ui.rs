//! Drawing for plugins: widget specs built by Lua, and the
//! Settings → Plugins page (list, toggles, install approval, log viewer).

use eframe::egui::{self, Color32, RichText, vec2};

use super::manager::PendingInstall;
use super::manifest::{Manifest, Permission};
use super::runtime::{Runtime, UiTarget, Widget};
use super::{PluginGrant, PluginHost, PluginsAction, open_folder};
use crate::config::Config;
use crate::theme::{Bold, Palette};

/// Dialog / status state of the Settings → Plugins page.
#[derive(Default)]
pub struct PageState {
    /// A staged install waiting for approval.
    pub pending: Option<PendingInstall>,
    /// Plugin id the user is enabling (permission approval).
    pub approve_enable: Option<String>,
    pub confirm_uninstall: Option<String>,
    pub log_view: Option<String>,
    /// (message, is_error)
    pub status: Option<(String, bool)>,
}

/// Draw widget specs. Returns the clicked button's callback index and
/// errors (e.g. storage full).
pub fn render_widgets(
    ui: &mut egui::Ui,
    rt: &Runtime,
    widgets: &[Widget],
    salt: impl std::hash::Hash + std::fmt::Debug,
    pal: &Palette,
) -> (Option<usize>, Vec<String>) {
    let base = egui::Id::unique("plugin_ui").with(salt);
    let mut clicked = None;
    let mut errors = Vec::new();
    for (n, w) in widgets.iter().enumerate() {
        match w {
            Widget::Heading(t) => {
                ui.label(RichText::new(t).size(16.0).bold().color(pal.strong));
            }
            Widget::Label(t) => {
                ui.label(RichText::new(t).color(pal.text));
            }
            Widget::Small(t) => {
                ui.label(RichText::new(t).small().color(pal.muted));
            }
            Widget::Separator => {
                ui.separator();
            }
            Widget::Button { text, cb } => {
                if ui.button(text).clicked() {
                    clicked = Some(*cb);
                }
            }
            Widget::TextBox {
                key,
                hint,
                multiline,
            } => {
                let mut value = rt.storage_str(key);
                let edit = if *multiline {
                    egui::TextEdit::multiline(&mut value).desired_rows(6)
                } else {
                    egui::TextEdit::singleline(&mut value)
                };
                let resp = ui.add(
                    edit.id(base.with(("tb", n, key)))
                        .hint_text(hint.as_str())
                        .char_limit(10_000)
                        .desired_width(f32::INFINITY),
                );
                if resp.changed()
                    && let Err(e) = rt.storage_set(key, serde_json::Value::String(value))
                {
                    errors.push(e);
                }
            }
            Widget::Checkbox { key, text } => {
                let mut on = rt.storage_bool(key);
                if ui.checkbox(&mut on, text.as_str()).changed()
                    && let Err(e) = rt.storage_set(key, serde_json::Value::Bool(on))
                {
                    errors.push(e);
                }
            }
            Widget::Image { url, size } => {
                ui.add(
                    egui::Image::from_uri(url.as_str()).fit_to_exact_size(vec2(size[0], size[1])),
                );
            }
        }
    }
    (clicked, errors)
}

/// One installed plugin's row, snapshotted before drawing.
struct Row {
    index: usize,
    id: String,
    manifest: Result<Manifest, String>,
    running: bool,
    has_settings: bool,
    load_error: Option<String>,
    auto_disabled: Option<String>,
    memory: usize,
}

pub fn settings_page(
    host: &mut PluginHost,
    ui: &mut egui::Ui,
    config: &mut Config,
    pal: &Palette,
) -> Option<PluginsAction> {
    let mut action = None;
    hint(
        ui,
        pal,
        "Plugins are Luau scripts that add commands, panels and local tweaks. Each runs \
         in its own sandbox with only the permissions you approve: no network, files or \
         account access, and they can never send messages for you.",
    );
    ui.add_space(8.0);
    card(ui, pal, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width() - 60.0);
                ui.label(RichText::new("Plugins module").bold().color(pal.strong));
                ui.label(
                    RichText::new("Off = no plugin is loaded and nothing runs.")
                        .small()
                        .color(pal.muted),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle(ui, &mut config.modules.plugins, pal)
            });
        });
    });
    if !config.modules.plugins {
        return None;
    }
    if !host.active {
        // The module was just switched on; `update` scans next frame.
        ui.ctx().request_repaint();
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        let busy = host.picker.is_some();
        if ui
            .add_enabled(!busy, pal.primary_button("Install plugin (.zip)…"))
            .clicked()
        {
            host.pick_install_source(ui.ctx(), true);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Install from folder…"))
            .clicked()
        {
            host.pick_install_source(ui.ctx(), false);
        }
        if ui.button("📁 Open plugins folder").clicked() {
            open_folder(&host.mgr.plugins_dir);
        }
        if ui
            .button("↻ Reload all")
            .on_hover_text("Re-read the plugins folder and restart every plugin")
            .clicked()
        {
            let ids: Vec<String> = host.mgr.entries.iter().map(|e| e.id.clone()).collect();
            for id in ids {
                host.mgr.reload(&id);
            }
            host.mgr.scan();
            host.sync(&config.enabled_plugins);
        }
    });
    if host.picker.is_some() {
        hint(ui, pal, "Waiting for the file picker…");
    }
    if let Some((text, error)) = &host.ui.status {
        ui.label(RichText::new(text).color(if *error { pal.danger } else { pal.online }));
    }
    ui.add_space(8.0);

    let rows: Vec<Row> = host
        .mgr
        .entries
        .iter()
        .enumerate()
        .map(|(index, e)| Row {
            index,
            id: e.id.clone(),
            manifest: e.manifest.clone(),
            running: e.is_running(),
            has_settings: e.runtime.as_ref().is_some_and(Runtime::has_settings),
            load_error: e.load_error.clone(),
            auto_disabled: e.auto_disabled.clone(),
            memory: e.runtime.as_ref().map_or(0, Runtime::used_memory),
        })
        .collect();
    if rows.is_empty() {
        hint(
            ui,
            pal,
            "No plugins installed. Install one from a .zip or a folder; RustCord's \
             examples/plugins folder has a few to try.",
        );
    }
    for row in &rows {
        plugin_card(host, ui, config, pal, row);
        ui.add_space(6.0);
    }

    if let Some(a) = dialogs(host, ui.ctx(), config, pal) {
        action = Some(a);
    }
    action
}

fn plugin_card(
    host: &mut PluginHost,
    ui: &mut egui::Ui,
    config: &mut Config,
    pal: &Palette,
    row: &Row,
) {
    let grant = config
        .enabled_plugins
        .iter()
        .find(|g| g.id == row.id)
        .cloned();
    card(ui, pal, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width() - 60.0);
                match &row.manifest {
                    Ok(m) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&m.name).bold().color(pal.strong));
                            let by = if m.author.is_empty() {
                                format!("v{}", m.version)
                            } else {
                                format!("v{} · by {}", m.version, m.author)
                            };
                            ui.label(RichText::new(by).small().color(pal.muted));
                        });
                        if !m.description.is_empty() {
                            ui.label(RichText::new(&m.description).small().color(pal.text));
                        }
                    }
                    Err(e) => {
                        ui.label(RichText::new(&row.id).bold().color(pal.strong));
                        ui.label(
                            RichText::new(format!("Can't load: {e}"))
                                .small()
                                .color(pal.danger),
                        );
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let Ok(m) = &row.manifest else {
                    return;
                };
                let mut on = grant.is_some();
                if toggle(ui, &mut on, pal).clicked() {
                    if on {
                        host.ui.approve_enable = Some(m.id.clone());
                    } else {
                        config.enabled_plugins.retain(|g| g.id != row.id);
                        host.sync(&config.enabled_plugins);
                    }
                }
            });
        });

        if let Ok(m) = &row.manifest {
            ui.horizontal_wrapped(|ui| {
                if m.permissions.is_empty() {
                    ui.label(RichText::new("No permissions").small().color(pal.muted));
                }
                for p in &m.permissions {
                    chip(ui, pal, p.label()).on_hover_text(p.description());
                }
            });
            let status: Option<(String, Color32)> = if let Some(reason) = &row.auto_disabled {
                Some((reason.clone(), pal.danger))
            } else if let Some(err) = &row.load_error {
                Some((format!("Failed to start: {err}"), pal.danger))
            } else if let Some(g) = &grant {
                if !g.covers(m) {
                    Some((
                        "This version asks for new permissions. Switch it off and on to review them."
                            .into(),
                        pal.idle,
                    ))
                } else if row.running {
                    let mb = row.memory as f64 / 1_048_576.0;
                    Some((format!("Running · {mb:.1} MB"), pal.online))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((text, color)) = status {
                ui.label(RichText::new(text).small().color(color));
            }
        }

        ui.horizontal_wrapped(|ui| {
            if ui.small_button("↻ Reload").clicked() {
                host.mgr.reload(&row.id);
                host.sync(&config.enabled_plugins);
            }
            if ui.small_button("📄 Log").clicked() {
                host.ui.log_view = Some(row.id.clone());
            }
            if ui.small_button("📁 Folder").clicked() {
                open_folder(&host.mgr.plugins_dir.join(&row.id));
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("Uninstall").small().color(pal.danger)).small(),
                )
                .clicked()
            {
                host.ui.confirm_uninstall = Some(row.id.clone());
            }
        });

        if row.running && row.has_settings {
            egui::CollapsingHeader::new("Settings")
                .id_salt(("plugin_settings", &row.id))
                .show(ui, |ui| {
                    host.draw_ui(ui, row.index, UiTarget::Settings, pal);
                });
        }
    });
}

/// The modal dialogs: install approval, enable approval, uninstall, log.
fn dialogs(
    host: &mut PluginHost,
    ctx: &egui::Context,
    config: &mut Config,
    pal: &Palette,
) -> Option<PluginsAction> {
    let mut action = None;

    if let Some(p) = &host.ui.pending {
        let title = match &p.replaces {
            Some(v) => format!(
                "Update {} (v{v} → v{})?",
                p.manifest.name, p.manifest.version
            ),
            None => format!("Install {}?", p.manifest.name),
        };
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_plugin_install")).show(ctx, |ui| {
            choice = approval_body(ui, pal, &title, &p.manifest, true);
        });
        if modal.should_close() && choice.is_none() {
            choice = Some(Approve::Cancel);
        }
        if let Some(choice) = choice {
            let p = host.ui.pending.take().expect("pending install");
            match choice {
                Approve::Cancel => host.mgr.cancel_install(p),
                Approve::Yes | Approve::KeepOff => {
                    let msg = host.finish_install(p, config, choice == Approve::Yes);
                    let ok = msg.starts_with("Installed");
                    host.ui.status = Some((msg.clone(), !ok));
                    action = Some(PluginsAction::Notice(msg));
                }
            }
        }
    }

    if let Some(id) = host.ui.approve_enable.clone() {
        let manifest = host
            .mgr
            .index(&id)
            .and_then(|i| host.mgr.entries[i].manifest.clone().ok());
        match manifest {
            None => host.ui.approve_enable = None,
            Some(m) => {
                let mut choice = None;
                let modal =
                    egui::Modal::new(egui::Id::unique("rc_plugin_enable")).show(ctx, |ui| {
                        choice = approval_body(ui, pal, &format!("Enable {}?", m.name), &m, false);
                    });
                if modal.should_close() && choice.is_none() {
                    choice = Some(Approve::Cancel);
                }
                if let Some(choice) = choice {
                    host.ui.approve_enable = None;
                    if choice == Approve::Yes {
                        config.enabled_plugins.retain(|g| g.id != id);
                        config.enabled_plugins.push(PluginGrant {
                            id: id.clone(),
                            permissions: m.permissions.clone(),
                        });
                        host.mgr.reset(&id);
                        host.sync(&config.enabled_plugins);
                    }
                }
            }
        }
    }

    if let Some(id) = host.ui.confirm_uninstall.clone() {
        let name = host
            .mgr
            .index(&id)
            .map_or(id.clone(), |i| host.mgr.entries[i].name().to_string());
        let mut close = false;
        let mut confirm = false;
        let modal = egui::Modal::new(egui::Id::unique("rc_plugin_uninstall")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(format!("Uninstall {name}?"));
            ui.label("This deletes the plugin's folder and its saved data from this PC.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("Uninstall").color(Color32::WHITE))
                            .fill(pal.danger),
                    )
                    .clicked()
                {
                    confirm = true;
                }
            });
        });
        if confirm {
            config.enabled_plugins.retain(|g| g.id != id);
            let msg = match host.mgr.uninstall(&id) {
                Ok(()) => format!("Uninstalled {name}"),
                Err(e) => e,
            };
            host.sync(&config.enabled_plugins);
            host.ui.status = Some((msg.clone(), false));
            action = Some(PluginsAction::Notice(msg));
            close = true;
        }
        if close || modal.should_close() {
            host.ui.confirm_uninstall = None;
        }
    }

    if let Some(id) = host.ui.log_view.clone() {
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("rc_plugin_log")).show(ctx, |ui| {
            ui.set_width(560.0);
            let Some(i) = host.mgr.index(&id) else {
                close = true;
                return;
            };
            let e = &host.mgr.entries[i];
            ui.heading(format!("{} — log", e.name()));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let log = e.log.borrow();
                    if log.lines.is_empty() {
                        ui.label(RichText::new("Nothing logged yet.").color(pal.muted));
                    }
                    for l in &log.lines {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&l.time).small().monospace().color(pal.muted));
                            ui.label(RichText::new(&l.text).monospace().color(if l.error {
                                pal.danger
                            } else {
                                pal.text
                            }));
                        });
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Clear").clicked() {
                    e.log.borrow_mut().lines.clear();
                }
                if ui.add(pal.primary_button("Close")).clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            host.ui.log_view = None;
        }
    }
    action
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Approve {
    Yes,
    KeepOff,
    Cancel,
}

/// Plugin details and the permissions it asks for, with the buttons.
fn approval_body(
    ui: &mut egui::Ui,
    pal: &Palette,
    title: &str,
    m: &Manifest,
    installing: bool,
) -> Option<Approve> {
    let mut choice = None;
    ui.set_max_width(460.0);
    ui.heading(title);
    let by = if m.author.is_empty() {
        format!("{} v{}", m.id, m.version)
    } else {
        format!("{} v{} by {}", m.id, m.version, m.author)
    };
    ui.label(RichText::new(by).small().color(pal.muted));
    if !m.description.is_empty() {
        ui.label(&m.description);
    }
    ui.add_space(8.0);
    ui.label(pal.section_label("This plugin can"));
    if m.permissions.is_empty() {
        ui.label(RichText::new("Nothing beyond logging (no permissions).").color(pal.muted));
    }
    for p in Permission::ALL.iter().filter(|p| m.has(**p)) {
        ui.label(RichText::new(format!("• {}", p.label())).color(pal.strong));
        ui.label(RichText::new(p.description()).small().color(pal.muted));
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Only install plugins you trust. Plugins run in a sandbox without network, \
             file or account access and can't send messages, but they can see what their \
             permissions allow.",
        )
        .small()
        .color(pal.muted),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            choice = Some(Approve::Cancel);
        }
        if installing && ui.button("Install, keep off").clicked() {
            choice = Some(Approve::KeepOff);
        }
        let label = if installing {
            "Install & enable"
        } else {
            "Enable"
        };
        if ui.add(pal.primary_button(label)).clicked() {
            choice = Some(Approve::Yes);
        }
    });
    choice
}

fn card<R>(ui: &mut egui::Ui, pal: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn chip(ui: &mut egui::Ui, pal: &Palette, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(pal.input)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(pal.text));
        })
        .response
}

fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

/// Discord-style on/off switch (same look as the settings page's).
fn toggle(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off = Color32::from_gray(if pal.dark { 0x50 } else { 0xb0 });
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    let bg = Color32::from_rgb(
        l(off.r(), pal.online.r()),
        l(off.g(), pal.online.g()),
        l(off.b(), pal.online.b()),
    );
    ui.painter().rect_filled(rect, 11, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp
}
