//! The "Scan QR code" panel on the login screen.
//!
//! Opening it starts the remote auth task (`remote_auth.rs`, its own small
//! backend thread) and draws the code it reports as a texture. Dropping the
//! panel (closing it, or logging in) drops the channel, which closes the
//! socket, and frees the QR texture and the scanned avatar.

use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions, vec2};
use tokio::sync::mpsc::UnboundedSender;

use crate::accounts::Token;
use crate::remote_auth::QrEvent;
use crate::theme::Palette;
use crate::types::{ToBackend, ToUi};

/// Drawn size of the code (it is scaled up with nearest filtering).
const QR_SIZE: f32 = 176.0;
/// White border around the code, in modules (the spec's quiet zone).
const QUIET_ZONE: usize = 4;

enum Step {
    Connecting,
    Code,
    Scanned { username: String, avatar: String },
    LoggingIn,
    Cancelled,
    Failed(String),
}

pub struct QrPanel {
    ctx: egui::Context,
    tx: UnboundedSender<ToBackend>,
    rx: Receiver<ToUi>,
    step: Step,
    texture: Option<TextureHandle>,
}

impl QrPanel {
    /// Start the remote auth task and ask for a code.
    pub fn open(ctx: &egui::Context) -> Self {
        let (ui_tx, rx) = mpsc::channel();
        let (tx, be_rx) = tokio::sync::mpsc::unbounded_channel();
        let repaint = ctx.clone();
        crate::remote_auth::spawn(ui_tx, be_rx, move || repaint.request_repaint());
        let _ = tx.send(ToBackend::StartQrLogin);
        Self {
            ctx: ctx.clone(),
            tx,
            rx,
            step: Step::Connecting,
            texture: None,
        }
    }

    fn restart(&mut self) {
        self.free_images();
        self.step = Step::Connecting;
        let _ = self.tx.send(ToBackend::StartQrLogin);
    }

    fn free_images(&mut self) {
        self.texture = None;
        if let Step::Scanned { avatar, .. } = &self.step
            && !avatar.is_empty()
        {
            self.ctx.forget_image(avatar);
        }
    }

    /// Returns the token once the user approved on their phone.
    pub fn ui(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<Token> {
        let mut token = None;
        while let Ok(ev) = self.rx.try_recv() {
            let ToUi::QrLogin(ev) = ev else { continue };
            match ev {
                QrEvent::Code(url) => {
                    self.free_images();
                    self.texture = qr_texture(&self.ctx, &url);
                    self.step = match self.texture {
                        Some(_) => Step::Code,
                        None => Step::Failed("Couldn't draw the QR code.".into()),
                    };
                }
                QrEvent::Scanned { username, avatar } => {
                    self.texture = None;
                    self.step = Step::Scanned { username, avatar };
                }
                QrEvent::Token(t) => {
                    self.free_images();
                    self.step = Step::LoggingIn;
                    token = Some(t);
                }
                QrEvent::Cancelled => {
                    self.free_images();
                    self.step = Step::Cancelled;
                }
                QrEvent::Failed(text) => {
                    self.free_images();
                    self.step = Step::Failed(text);
                }
            }
        }

        let mut retry = false;
        ui.vertical_centered(|ui| match &self.step {
            Step::Connecting | Step::LoggingIn => {
                placeholder(ui, pal, |ui| {
                    ui.spinner();
                });
                let text = if matches!(self.step, Step::Connecting) {
                    "Getting a code from Discord…"
                } else {
                    "Logging in…"
                };
                ui.label(RichText::new(text).color(pal.muted));
            }
            Step::Code => {
                if let Some(tex) = &self.texture {
                    ui.add(egui::Image::new((tex.id(), vec2(QR_SIZE, QR_SIZE))).corner_radius(6));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Log in with QR code").strong().size(16.0));
                ui.label(
                    RichText::new(
                        "Scan this with the Discord mobile app (Settings → Scan QR \
                             Code), then approve the login on your phone.",
                    )
                    .color(pal.muted),
                );
            }
            Step::Scanned { username, avatar } => {
                placeholder(ui, pal, |ui| {
                    if avatar.is_empty() {
                        ui.label(
                            RichText::new(crate::icons::DEVICE_MOBILE)
                                .size(48.0)
                                .color(pal.muted),
                        );
                    } else {
                        ui.add(
                            egui::Image::new(avatar.as_str())
                                .fit_to_exact_size(vec2(64.0, 64.0))
                                .corner_radius(32),
                        );
                    }
                });
                ui.label(RichText::new("Check your phone!").strong().size(16.0));
                let who = if username.is_empty() {
                    "Approve the login in the Discord app.".to_string()
                } else {
                    format!("Logging in as {username}. Approve it in the Discord app.")
                };
                ui.label(RichText::new(who).color(pal.muted));
                retry = ui
                    .link(RichText::new("Not you? Start over").color(pal.muted))
                    .clicked();
            }
            Step::Cancelled | Step::Failed(_) => {
                placeholder(ui, pal, |ui| {
                    ui.label(
                        RichText::new(crate::icons::QR_CODE)
                            .size(48.0)
                            .color(pal.muted),
                    );
                });
                let text = match &self.step {
                    Step::Failed(text) => text.as_str(),
                    _ => "The login was cancelled on your phone.",
                };
                let color = if matches!(self.step, Step::Failed(_)) {
                    pal.danger
                } else {
                    pal.muted
                };
                ui.label(RichText::new(text).color(color));
                ui.add_space(6.0);
                retry = ui
                    .button(format!("{} Try again", crate::icons::ARROW_CLOCKWISE))
                    .clicked();
            }
        });
        if retry {
            self.restart();
        }
        token
    }
}

impl Drop for QrPanel {
    fn drop(&mut self) {
        // Closing the channel also ends the task; say so explicitly first.
        let _ = self.tx.send(ToBackend::CancelQrLogin);
        self.free_images();
    }
}

/// A QR-sized box with `content` centred in it, so the card keeps its size.
fn placeholder(ui: &mut egui::Ui, pal: &Palette, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(pal.input)
        .corner_radius(6)
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                vec2(QR_SIZE, QR_SIZE),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                content,
            );
        });
}

/// The QR code as a texture: one pixel per module plus the quiet zone,
/// drawn scaled up with nearest filtering (a few KB of RAM).
fn qr_texture(ctx: &egui::Context, url: &str) -> Option<TextureHandle> {
    let image = qr_image(url)?;
    Some(ctx.load_texture("rustcord-qr-login", image, TextureOptions::NEAREST))
}

/// Dark modules black on white, whatever the theme (scanners need it).
fn qr_image(data: &str) -> Option<ColorImage> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let w = code.width();
    let side = w + 2 * QUIET_ZONE;
    let mut pixels = vec![Color32::WHITE; side * side];
    for (i, c) in code.to_colors().into_iter().enumerate() {
        if c == qrcode::Color::Dark {
            let (x, y) = (i % w + QUIET_ZONE, i / w + QUIET_ZONE);
            pixels[y * side + x] = Color32::BLACK;
        }
    }
    Some(ColorImage::new([side, side], pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_image_has_quiet_zone_and_finder() {
        let url = crate::remote_auth::qr_url("Zq0bRBBn3iRzjBlD6rdCDZBspcqJKQ0J9N4VWUKXMxQ");
        let img = qr_image(&url).unwrap();
        let side = img.size[0];
        assert_eq!(img.size[0], img.size[1]);
        // Version ≥ 3 for this length: 29 modules + 8 quiet.
        assert!(side >= 29 + 2 * QUIET_ZONE);
        // Quiet zone is white; the top-left finder pattern starts dark.
        assert!(
            img.pixels[..side * QUIET_ZONE]
                .iter()
                .all(|&p| p == Color32::WHITE)
        );
        assert_eq!(img.pixels[QUIET_ZONE * side + QUIET_ZONE], Color32::BLACK);
    }
}
