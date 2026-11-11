//! Small notice cards in the bottom-right corner ("Update available",
//! "RustCord closed unexpectedly"): a title, one line of text and buttons.
//! They never act on their own; every button is a click.

use eframe::egui::{self, RichText};

use crate::icons;
use crate::theme::{Bold as _, Palette};

/// Draw one card. `slot` stacks cards (0 = lowest). Returns the index of
/// the clicked button; the first button is the primary one.
pub fn card(
    ctx: &egui::Context,
    pal: &Palette,
    slot: usize,
    icon: &str,
    title: &str,
    text: &str,
    buttons: &[(&str, &str)],
) -> Option<usize> {
    let mut clicked = None;
    egui::Area::new(egui::Id::unique("rc_notice_card").with(slot))
        .order(egui::Order::Foreground)
        .anchor(
            egui::Align2::RIGHT_BOTTOM,
            egui::vec2(-16.0, -(84.0 + 100.0 * slot as f32)),
        )
        .show(ctx, |ui| {
            crate::widgets::menu_frame(pal)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_max_width(340.0);
                    ui.label(
                        RichText::new(icons::label(icon, title))
                            .bold()
                            .color(pal.strong),
                    );
                    if !text.is_empty() {
                        ui.label(RichText::new(text).small().color(pal.muted));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        for (i, (icon, label)) in buttons.iter().enumerate() {
                            let text = icons::label(icon, label);
                            let button = if i == 0 {
                                pal.primary_button(&text)
                            } else {
                                egui::Button::new(text)
                            };
                            if ui.add(button).clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                });
        });
    clicked
}
