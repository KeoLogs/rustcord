//! The login screen: paste a token (user or bot), optionally remember it
//! (in the OS credential store, see `accounts.rs`).

use eframe::egui::{self, RichText, vec2};

use crate::accounts::Token;
use crate::theme::Palette;

#[derive(Default)]
pub struct LoginForm {
    pub token: String,
    pub is_bot: bool,
    pub remember: bool,
    pub show_token: bool,
    pub error: Option<String>,
    /// Shown in the "Add an account" dialog: no welcome title, no demo link.
    pub add_account: bool,
    /// "Scan QR code" panel while it's open (`qr_login.rs`).
    pub qr: Option<crate::qr_login::QrPanel>,
}

pub enum LoginAction {
    /// Log in with this token ("Bot " prefix for bots).
    Login {
        token: Token,
        remember: bool,
    },
    Demo,
}

impl LoginForm {
    pub fn new(remember: bool, error: Option<String>) -> Self {
        Self {
            remember,
            error,
            ..Default::default()
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<LoginAction> {
        let mut action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.servers))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() - 420.0).max(20.0) / 2.0);
                    egui::Frame::new()
                        .fill(pal.chat)
                        .corner_radius(8)
                        .inner_margin(32)
                        .show(ui, |ui| {
                            ui.set_width(420.0);
                            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                action = self.card(ui, pal);
                            });
                        });
                });
            });
        action
    }

    /// The form itself (also used inside the "Add an account" dialog).
    pub fn card(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<LoginAction> {
        let mut action = None;
        if !self.add_account {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Welcome back!").size(24.0).strong());
                ui.label(
                    RichText::new("Log in to RustCord with your Discord token").color(pal.muted),
                );
            });
            ui.add_space(20.0);
        }

        // Token paste or QR code (user accounts; see qr_login.rs).
        ui.horizontal(|ui| {
            if ui.selectable_label(self.qr.is_none(), "Token").clicked() {
                self.qr = None;
            }
            let qr_tab = format!("{} Scan QR code", crate::icons::QR_CODE);
            if ui.selectable_label(self.qr.is_some(), qr_tab).clicked() && self.qr.is_none() {
                self.qr = Some(crate::qr_login::QrPanel::open(ui.ctx()));
            }
        });
        ui.add_space(10.0);
        if let Some(qr) = &mut self.qr {
            if let Some(token) = qr.ui(ui, pal) {
                action = Some(LoginAction::Login {
                    token,
                    remember: self.remember,
                });
            }
            ui.add_space(10.0);
            ui.checkbox(&mut self.remember, "Remember me on this PC");
            return action;
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("ACCOUNT TYPE")
                    .small()
                    .strong()
                    .color(pal.muted),
            );
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.is_bot, false, "User account");
            ui.selectable_value(&mut self.is_bot, true, "Bot");
        });
        ui.add_space(10.0);

        ui.label(RichText::new("TOKEN").small().strong().color(pal.muted));
        let mut submitted = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.token)
                    .password(!self.show_token)
                    .desired_width(ui.available_width() - 40.0)
                    .margin(vec2(8.0, 8.0))
                    .hint_text("Paste your token"),
            );
            submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let eye = if self.show_token {
                crate::icons::EYE_SLASH
            } else {
                crate::icons::EYE
            };
            if ui.button(eye).on_hover_text("Show / hide token").clicked() {
                self.show_token = !self.show_token;
            }
        });
        ui.add_space(6.0);
        ui.checkbox(&mut self.remember, "Remember me on this PC")
            .on_hover_text(
                "Keeps the login in the system's credential store (Windows Credential \
                 Manager, macOS Keychain or Secret Service), not in a file.",
            );

        if let Some(err) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(err).color(pal.danger));
        }

        ui.add_space(14.0);
        let login = ui.add_sized([ui.available_width(), 38.0], pal.primary_button("Log In"));
        if (login.clicked() || submitted) && !self.token.trim().is_empty() {
            let raw = self.token.trim().trim_start_matches("Bot ").to_string();
            let token = if self.is_bot {
                format!("Bot {raw}")
            } else {
                raw
            };
            action = Some(LoginAction::Login {
                token: Token::new(token),
                remember: self.remember,
            });
        }

        ui.add_space(10.0);
        egui::CollapsingHeader::new(RichText::new("How do I get my token?").color(pal.muted)).show(
            ui,
            |ui| {
                ui.label(
                    "User account: open discord.com/app in your browser, press F12, \
                     go to Network, click any channel, open a request to /api and copy \
                     the 'authorization' request header.",
                );
                ui.label(
                    "Bot: Discord Developer Portal → your app → Bot → Reset Token. \
                     Turn on the Message Content intent.",
                );
                ui.label(
                    RichText::new(
                        "Never share your token. Anyone with it can use your account. \
                         Third-party clients are against Discord's Terms of Service; use an alt.",
                    )
                    .color(pal.danger),
                );
            },
        );

        if self.add_account {
            return action;
        }
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            if ui
                .link(RichText::new("Try the offline demo").color(pal.muted))
                .clicked()
            {
                action = Some(LoginAction::Demo);
            }
        });
        action
    }
}
