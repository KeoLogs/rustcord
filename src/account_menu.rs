//! The account switcher: the accounts part of the user panel menu (saved
//! accounts with a check on the current one, "Add an account", "Log Out"),
//! the right-click menu on an account ("Open in new window", "Remove
//! account…") and the confirm dialogs.
//!
//! Nothing here touches tokens: rows come from `Config::accounts`
//! (metadata only). The app turns an [`AccountAction`] into a switch, a new
//! window, etc. Every action is a user click; nothing switches by itself.

use eframe::egui::{self, Color32, RichText, vec2};

use crate::accounts::SavedAccount;
use crate::config::Config;
use crate::icons;
use crate::theme::{self, Palette};
use crate::widgets::{self, MENU_ROW, Tone};

/// What the user picked in the account menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountAction {
    /// Left-click on another saved account: switch this window to it.
    Switch(String),
    /// Right-click → "Open in new window".
    OpenWindow(String),
    /// Right-click → "Remove account…" (already confirmed).
    Remove(String),
    /// "Add an account": the token login in a modal.
    Add,
    /// "Log Out" (already confirmed): removes only the current account.
    LogOut,
    /// Account windows: close this window (disconnects that account).
    CloseWindow,
}

/// A question to confirm before acting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    Remove { user_id: String, name: String },
    LogOut { name: String },
}

/// The accounts part of the user panel menu. `current` is this window's
/// account (user id); `in_window` is true in an extra account window,
/// which only offers to close itself (switching happens in the main window).
/// Returns an action; asks for a confirm through `confirm` instead for
/// Remove / Log Out.
pub fn section(
    ui: &mut egui::Ui,
    config: &Config,
    current: Option<&str>,
    in_window: bool,
    confirm: &mut Option<Confirm>,
    pal: &Palette,
) -> Option<AccountAction> {
    if in_window {
        return widgets::menu_item(ui, Some(icons::X), "Close Window", Tone::Normal, pal)
            .on_hover_text("Disconnects this account and closes its window.")
            .clicked()
            .then_some(AccountAction::CloseWindow);
    }
    let mut action = None;
    if !config.accounts.is_empty() {
        header(ui, "SWITCH ACCOUNTS", pal);
    }
    for acct in &config.accounts {
        let is_current = current == Some(acct.user_id.as_str());
        let resp = account_row(ui, acct, is_current, config, pal);
        let resp = if is_current {
            resp.on_hover_text("Signed in here · right-click for more")
        } else {
            resp.on_hover_text("Click to switch · right-click for more")
        };
        if resp.clicked() && !is_current {
            action = Some(AccountAction::Switch(acct.user_id.clone()));
            ui.close();
        }
        egui::Popup::context_menu(&resp)
            .frame(widgets::menu_frame(pal))
            .show(|ui| {
                let open = widgets::menu_item(
                    ui,
                    Some(icons::ARROW_SQUARE_OUT),
                    "Open in new window",
                    if is_current {
                        Tone::Disabled
                    } else {
                        Tone::Normal
                    },
                    pal,
                );
                if open.clicked() {
                    action = Some(AccountAction::OpenWindow(acct.user_id.clone()));
                }
                widgets::menu_separator(ui, pal);
                if widgets::menu_item(
                    ui,
                    Some(icons::USER_MINUS),
                    "Remove account…",
                    Tone::Danger,
                    pal,
                )
                .clicked()
                {
                    *confirm = Some(Confirm::Remove {
                        user_id: acct.user_id.clone(),
                        name: acct.display_name.clone(),
                    });
                }
            });
    }
    if widgets::menu_item(
        ui,
        Some(icons::USER_PLUS),
        "Add an account",
        Tone::Normal,
        pal,
    )
    .clicked()
    {
        action = Some(AccountAction::Add);
    }
    widgets::menu_separator(ui, pal);
    if widgets::menu_item(ui, Some(icons::SIGN_OUT), "Log Out", Tone::Danger, pal).clicked() {
        let name = current
            .and_then(|id| config.accounts.iter().find(|a| a.user_id == id))
            .map(|a| a.display_name.clone())
            .unwrap_or_default();
        *confirm = Some(Confirm::LogOut { name });
    }
    action
}

/// Small muted caps header inside the menu.
fn header(ui: &mut egui::Ui, text: &str, pal: &Palette) {
    let w = ui.available_width().clamp(180.0, 240.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        theme::bold_font(12.0),
        pal.muted,
    );
}

/// One saved account: avatar, display name and @username, a check mark on
/// the current one. Hover like `widgets::menu_item`.
fn account_row(
    ui: &mut egui::Ui,
    acct: &SavedAccount,
    current: bool,
    config: &Config,
    pal: &Palette,
) -> egui::Response {
    let w = ui.available_width().clamp(180.0, 240.0);
    let h = MENU_ROW + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), egui::Sense::click());
    let hovered = resp.hovered() && !current;
    if hovered {
        ui.painter()
            .rect_filled(rect, 4, widgets::pressed_fill(&resp, pal.accent));
    } else if current {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let (fg, sub) = if hovered {
        (Color32::WHITE, Color32::WHITE)
    } else {
        (pal.text, pal.muted)
    };
    let av = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 8.0 + 14.0, rect.center().y),
        vec2(28.0, 28.0),
    );
    let name = if acct.display_name.is_empty() {
        &acct.username
    } else {
        &acct.display_name
    };
    crate::app::paint_avatar(ui, av, &acct.user_id, name, &acct.avatar_url(), config, pal);
    let x = av.right() + 10.0;
    let max_w = rect.right() - x - if current { 30.0 } else { 8.0 };
    let n = crate::app::truncated(ui, name, theme::bold_font(14.0), fg, max_w);
    let handle = if acct.is_bot {
        format!("{} · bot", acct.username)
    } else {
        acct.username.clone()
    };
    let u = crate::app::truncated(ui, &handle, egui::FontId::proportional(12.0), sub, max_w);
    let total = n.size().y + u.size().y - 2.0;
    let top = rect.center().y - total / 2.0;
    let nh = n.size().y;
    ui.painter().galley(egui::pos2(x, top), n, fg);
    ui.painter().galley(egui::pos2(x, top + nh - 2.0), u, sub);
    if current {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            icons::CHECK,
            icons::font(18.0),
            pal.accent,
        );
    }
    if current {
        resp
    } else {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// The confirm dialog for `confirm`. Returns the action once confirmed;
/// clears `confirm` when answered either way.
pub fn confirm_dialog(
    ctx: &egui::Context,
    confirm: &mut Option<Confirm>,
    pal: &Palette,
) -> Option<AccountAction> {
    let c = confirm.clone()?;
    let (title, body, button, action) = match &c {
        Confirm::Remove { user_id, name } => (
            format!("Remove {name}?"),
            "This removes the account from RustCord on this PC (its saved login is \
             deleted from the system's credential store). You can add it again later \
             with its token."
                .to_string(),
            "Remove",
            AccountAction::Remove(user_id.clone()),
        ),
        Confirm::LogOut { name } => (
            "Log Out".to_string(),
            if name.is_empty() {
                "Are you sure you want to log out?".to_string()
            } else {
                format!(
                    "Are you sure you want to log out of {name}? Its saved login is removed \
                     from this PC; your other accounts stay saved."
                )
            },
            "Log Out",
            AccountAction::LogOut,
        ),
    };
    let mut close = false;
    let mut out = None;
    let modal = egui::Modal::new(egui::Id::unique("rc_confirm_account")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(title);
        ui.label(body);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new(button).color(Color32::WHITE)).fill(pal.danger),
                )
                .clicked()
            {
                out = Some(action.clone());
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        *confirm = None;
    }
    out
}
