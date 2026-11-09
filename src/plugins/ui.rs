//! Drawing for plugins: widget specs built by Lua, and the
//! Settings → Plugins page (list, toggles, install approval, log viewer).

use eframe::egui::{self, Color32, RichText, vec2};

use super::manager::PendingInstall;
use super::manifest::{Manifest, Permission};
use super::runtime::{Runtime, UiTarget, Widget};
use super::{PluginGrant, PluginHost, PluginsAction, open_folder};
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
    module_on: bool,
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
                let mut on = module_on;
                if toggle(ui, &mut on, pal).clicked() {
                    action = Some(PluginsAction::SetModule(on));
                }
            });
        });
    });
    if !module_on {
        return action;
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
        if icon_button(ui, Icon::Folder, "Open plugins folder", pal).clicked() {
            open_folder(&host.mgr.plugins_dir);
        }
        if icon_button(ui, Icon::Reload, "Reload all", pal)
            .on_hover_text("Re-read the plugins folder and restart every plugin")
            .clicked()
        {
            let ids: Vec<String> = host.mgr.entries.iter().map(|e| e.id.clone()).collect();
            for id in ids {
                host.mgr.reload(&id);
            }
            host.mgr.scan();
            host.sync();
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
        plugin_card(host, ui, pal, row);
        ui.add_space(6.0);
    }

    if let Some(a) = dialogs(host, ui.ctx(), pal) {
        action = Some(a);
    }
    action
}

fn plugin_card(host: &mut PluginHost, ui: &mut egui::Ui, pal: &Palette, row: &Row) {
    let grant = host.grant(&row.id);
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
                        host.revoke(&row.id);
                        host.sync();
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
            if icon_button(ui, Icon::Reload, "Reload", pal).clicked() {
                host.mgr.reload(&row.id);
                host.sync();
            }
            if icon_button(ui, Icon::Log, "Log", pal).clicked() {
                host.ui.log_view = Some(row.id.clone());
            }
            if icon_button(ui, Icon::Folder, "Folder", pal).clicked() {
                open_folder(&host.mgr.plugins_dir.join(&row.id));
            }
            if icon_button(ui, Icon::Trash, "Uninstall", pal).clicked() {
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
fn dialogs(host: &mut PluginHost, ctx: &egui::Context, pal: &Palette) -> Option<PluginsAction> {
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
                    let msg = host.finish_install(p, choice == Approve::Yes);
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
                        host.set_grant(PluginGrant {
                            id: id.clone(),
                            permissions: m.permissions.clone(),
                        });
                        host.mgr.reset(&id);
                        host.sync();
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
            host.revoke(&id);
            let msg = match host.mgr.uninstall(&id) {
                Ok(()) => format!("Uninstalled {name}"),
                Err(e) => e,
            };
            host.sync();
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

/// Vector icons for the plugins page (painted, never emoji).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Folder,
    Reload,
    Log,
    Trash,
}

/// Paint `icon` into `rect` (square) with `color`.
pub fn paint_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    use egui::{Shape, Stroke, pos2};
    let s = rect.width();
    let p = |x: f32, y: f32| pos2(rect.left() + x * s, rect.top() + y * s);
    let stroke = Stroke::new((s / 10.0).max(1.2), color);
    match icon {
        Icon::Folder => {
            let pts = vec![
                p(0.08, 0.22),
                p(0.40, 0.22),
                p(0.50, 0.34),
                p(0.92, 0.34),
                p(0.92, 0.82),
                p(0.08, 0.82),
            ];
            painter.add(Shape::closed_line(pts, stroke));
        }
        Icon::Reload => {
            let c = rect.center();
            let r = s * 0.34;
            let n = 20;
            // Three-quarter circle from the top, clockwise, gap top-right.
            let pts: Vec<_> = (0..=n)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_4 * 0.5
                        - (i as f32 / n as f32) * std::f32::consts::PI * 1.6;
                    pos2(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            let end = pts[0];
            painter.add(Shape::line(pts, stroke));
            // Arrow head at the start of the arc.
            painter.add(Shape::line(
                vec![
                    pos2(end.x - s * 0.20, end.y - s * 0.02),
                    end,
                    pos2(end.x + s * 0.02, end.y - s * 0.22),
                ],
                stroke,
            ));
        }
        Icon::Log => {
            let pts = vec![
                p(0.20, 0.08),
                p(0.64, 0.08),
                p(0.82, 0.26),
                p(0.82, 0.92),
                p(0.20, 0.92),
            ];
            painter.add(Shape::closed_line(pts, stroke));
            for y in [0.42, 0.58, 0.74] {
                painter.line_segment([p(0.34, y), p(0.68, y)], stroke);
            }
        }
        Icon::Trash => {
            painter.line_segment([p(0.12, 0.24), p(0.88, 0.24)], stroke);
            painter.add(Shape::line(
                vec![p(0.38, 0.24), p(0.40, 0.10), p(0.60, 0.10), p(0.62, 0.24)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![p(0.22, 0.24), p(0.28, 0.90), p(0.72, 0.90), p(0.78, 0.24)],
                stroke,
            ));
            for x in [0.42, 0.58] {
                painter.line_segment([p(x, 0.40), p(x, 0.76)], stroke);
            }
        }
    }
}

/// A small flat button: vector icon + label. Trash is drawn in danger red.
fn icon_button(ui: &mut egui::Ui, icon: Icon, text: &str, pal: &Palette) -> egui::Response {
    let color = if icon == Icon::Trash {
        pal.danger
    } else {
        pal.text
    };
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), color);
    let icon_size = 14.0;
    let pad = vec2(8.0, 4.0);
    let size = vec2(
        pad.x * 2.0 + icon_size + 6.0 + galley.size().x,
        (galley.size().y.max(icon_size) + pad.y * 2.0).max(24.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if resp.is_pointer_button_down_on() {
            pal.selected
        } else if resp.hovered() {
            pal.hover
        } else {
            pal.input
        };
        painter.rect_filled(rect, 4, fill);
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + pad.x + icon_size / 2.0, rect.center().y),
            vec2(icon_size, icon_size),
        );
        paint_icon(painter, icon_rect, icon, color);
        let text_pos = egui::pos2(
            icon_rect.right() + 6.0,
            rect.center().y - galley.size().y / 2.0,
        );
        painter.galley(text_pos, galley, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
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
