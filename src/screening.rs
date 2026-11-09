//! Membership screening ("rules screening"): servers with
//! `MEMBER_VERIFICATION_GATE_ENABLED` make new members agree to the rules
//! before they can talk. Until then the member object has `pending: true`.
//!
//! Data in, actions out: the dialog never does networking. The caller loads
//! the form through the backend ([`load`]), shows a [`ScreeningDialog`], and
//! on [`ScreeningAction::Submit`] sends the body with [`submit`]. Nothing is
//! submitted unless the user ticked "I have read and agree" and pressed
//! Submit.
//!
//! Protocol notes (Discord's web client; not documented for user accounts):
//! - `GET guilds/{id}/member-verification?with_guild=false` →
//!   `{ version, description, form_fields: [{ field_type, label, values,
//!   required, description?, placeholder?, choices? }] }`. `version` is an
//!   ISO timestamp string and must be echoed back unchanged.
//! - `PUT guilds/{id}/requests/@me` with `{ version, form_fields }`, each
//!   field being the original object plus `response` (`true` for TERMS, text
//!   for TEXT_INPUT / PARAGRAPH, the choice index for MULTIPLE_CHOICE).
//!   Answer: a join request with `application_status` (`APPROVED` for
//!   rules-only forms, `SUBMITTED` when moderators must review a join
//!   application, `REJECTED`). Some servers may answer 204 / empty; treated
//!   as approved. `pending` flips to false through GUILD_MEMBER_UPDATE.
//! - TEXT_INPUT / PARAGRAPH / MULTIPLE_CHOICE belong to "apply to join"
//!   applications; their shapes are less certain (limits 150 / 1000 chars
//!   assumed). Unknown field types block submitting if required and point
//!   the user to the official app.

use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use serde_json::{Value, json};

use crate::api::Api;
use crate::theme::{Bold, Palette};

/// Assumed answer limits for application questions.
pub const TEXT_INPUT_MAX: usize = 150;
pub const PARAGRAPH_MAX: usize = 1000;

/// The server feature that turns screening on.
pub const FEATURE: &str = "MEMBER_VERIFICATION_GATE_ENABLED";

/// True if my member object says I still have to complete screening.
pub fn member_is_pending(member: &Value) -> bool {
    member
        .get("pending")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// True if the server uses membership screening.
pub fn guild_has_screening(features: &[String]) -> bool {
    features.iter().any(|f| f == FEATURE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// The rules list + "I have read and agree".
    Terms,
    TextInput,
    Paragraph,
    MultipleChoice,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    pub kind: FieldKind,
    pub label: String,
    pub description: Option<String>,
    /// TERMS: the rules. MULTIPLE_CHOICE: the choices.
    pub values: Vec<String>,
    pub required: bool,
    pub placeholder: Option<String>,
    /// The original object, echoed back with `response` added.
    raw: Value,
}

impl FormField {
    fn from_json(v: &Value) -> Self {
        let kind = match v.get("field_type").and_then(Value::as_str).unwrap_or("") {
            "TERMS" => FieldKind::Terms,
            "TEXT_INPUT" => FieldKind::TextInput,
            "PARAGRAPH" => FieldKind::Paragraph,
            "MULTIPLE_CHOICE" => FieldKind::MultipleChoice,
            other => FieldKind::Unknown(other.to_string()),
        };
        let strings = |key: &str| -> Vec<String> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut values = strings("values");
        if values.is_empty() {
            values = strings("choices");
        }
        let text = |key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self {
            kind,
            label: text("label").unwrap_or_default(),
            description: text("description"),
            values,
            required: v.get("required").and_then(Value::as_bool).unwrap_or(true),
            placeholder: text("placeholder"),
            raw: v.clone(),
        }
    }
}

/// The screening form of a server.
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationForm {
    /// Echoed back as-is (an ISO timestamp string).
    pub version: Value,
    pub description: Option<String>,
    pub fields: Vec<FormField>,
}

impl VerificationForm {
    pub fn from_json(v: &Value) -> Self {
        Self {
            version: v.get("version").cloned().unwrap_or(Value::Null),
            description: v
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            fields: v
                .get("form_fields")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(FormField::from_json).collect())
                .unwrap_or_default(),
        }
    }
}

/// The user's answer to one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Agree(bool),
    Text(String),
    Choice(Option<usize>),
    Unsupported,
}

/// A form being filled in.
#[derive(Debug, Clone)]
pub struct ScreeningForm {
    pub form: VerificationForm,
    pub answers: Vec<Answer>,
}

impl ScreeningForm {
    pub fn new(form: VerificationForm) -> Self {
        let answers = form
            .fields
            .iter()
            .map(|f| match f.kind {
                FieldKind::Terms => Answer::Agree(false),
                FieldKind::TextInput | FieldKind::Paragraph => Answer::Text(String::new()),
                FieldKind::MultipleChoice => Answer::Choice(None),
                FieldKind::Unknown(_) => Answer::Unsupported,
            })
            .collect();
        Self { form, answers }
    }

    /// Why the form can't be sent yet, or Ok.
    pub fn validate(&self) -> Result<(), String> {
        for (f, a) in self.form.fields.iter().zip(&self.answers) {
            match a {
                Answer::Agree(ok) => {
                    if f.required && !ok {
                        return Err("Please agree to the rules first.".into());
                    }
                }
                Answer::Text(t) => {
                    let max = if f.kind == FieldKind::Paragraph {
                        PARAGRAPH_MAX
                    } else {
                        TEXT_INPUT_MAX
                    };
                    let n = t.trim().chars().count();
                    if f.required && n == 0 {
                        return Err(format!("\"{}\" needs an answer.", f.label));
                    }
                    if n > max {
                        return Err(format!(
                            "\"{}\" is too long ({n}/{max} characters).",
                            f.label
                        ));
                    }
                }
                Answer::Choice(c) => {
                    if f.required && c.is_none() {
                        return Err(format!("Pick an answer for \"{}\".", f.label));
                    }
                }
                Answer::Unsupported => {
                    if f.required {
                        return Err("This server asks something RustCord can't show yet. \
                                    Complete it in the official app."
                            .into());
                    }
                }
            }
        }
        Ok(())
    }

    /// Body for `PUT guilds/{id}/requests/@me`.
    pub fn payload(&self) -> Value {
        let fields: Vec<Value> = self
            .form
            .fields
            .iter()
            .zip(&self.answers)
            .map(|(f, a)| {
                let mut field = if f.raw.is_object() {
                    f.raw.clone()
                } else {
                    json!({})
                };
                let response = match a {
                    Answer::Agree(ok) => json!(ok),
                    Answer::Text(t) => json!(t.trim()),
                    Answer::Choice(Some(i)) => json!(i),
                    Answer::Choice(None) | Answer::Unsupported => Value::Null,
                };
                field["response"] = response;
                field
            })
            .collect();
        json!({ "version": self.form.version, "form_fields": fields })
    }
}

/// How a submitted form ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreeningOutcome {
    /// Done; you can talk now.
    Approved,
    /// A moderator has to review the application.
    Submitted,
    Rejected(Option<String>),
}

/// Read the `PUT requests/@me` answer.
pub fn classify_submit(body: &Value) -> ScreeningOutcome {
    match body.get("application_status").and_then(Value::as_str) {
        Some("SUBMITTED") => ScreeningOutcome::Submitted,
        Some("REJECTED") => ScreeningOutcome::Rejected(
            body.get("rejection_reason")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        ),
        _ => ScreeningOutcome::Approved,
    }
}

/// Backend helper: load a server's screening form.
pub async fn load(api: &Api, guild_id: &str) -> Result<VerificationForm, String> {
    api.member_verification(guild_id)
        .await
        .map(|v| VerificationForm::from_json(&v))
        .map_err(|e| e.to_string())
}

/// Backend helper: submit the form. Only after the user pressed Submit.
pub async fn submit(api: &Api, guild_id: &str, body: &Value) -> Result<ScreeningOutcome, String> {
    api.submit_member_verification(guild_id, body)
        .await
        .map(|v| classify_submit(&v))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// What the dialog asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum ScreeningAction {
    /// Send `PUT guilds/{guild_id}/requests/@me` with `body`, then call
    /// `set_result`.
    Submit {
        guild_id: String,
        body: Value,
    },
    /// Load the form again after an error.
    Retry {
        guild_id: String,
    },
    Close,
}

#[derive(Debug, Clone)]
enum Stage {
    Loading,
    Failed(String),
    Ready(ScreeningForm),
    Done(ScreeningOutcome),
}

/// The rules dialog for one server.
#[derive(Debug, Clone)]
pub struct ScreeningDialog {
    pub guild_id: String,
    pub guild_name: String,
    pub icon_url: Option<String>,
    stage: Stage,
    submitting: bool,
    error: Option<String>,
}

impl ScreeningDialog {
    /// Loading state; send the load request alongside it.
    pub fn new(guild_id: &str, guild_name: &str, icon_url: Option<String>) -> Self {
        Self {
            guild_id: guild_id.to_string(),
            guild_name: guild_name.to_string(),
            icon_url,
            stage: Stage::Loading,
            submitting: false,
            error: None,
        }
    }

    pub fn set_loaded(&mut self, form: VerificationForm) {
        self.stage = Stage::Ready(ScreeningForm::new(form));
        self.error = None;
    }

    pub fn set_failed(&mut self, error: impl Into<String>) {
        self.stage = Stage::Failed(error.into());
    }

    /// The submit answer arrived.
    pub fn set_result(&mut self, result: Result<ScreeningOutcome, String>) {
        self.submitting = false;
        match result {
            Ok(outcome) => self.stage = Stage::Done(outcome),
            Err(e) => self.error = Some(e),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> Option<ScreeningAction> {
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_screening")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                crate::invites::server_icon(ui, r, self.icon_url.as_deref(), &self.guild_name, pal);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&self.guild_name)
                            .size(16.0)
                            .bold()
                            .color(pal.strong),
                    );
                    ui.label(
                        RichText::new("Complete these steps to start talking")
                            .size(13.0)
                            .color(pal.muted),
                    );
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            match &mut self.stage {
                Stage::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading the rules…").color(pal.muted));
                    });
                }
                Stage::Failed(e) => {
                    ui.label(RichText::new(e.as_str()).color(pal.danger));
                    if ui.button("Retry").clicked() {
                        action = Some(ScreeningAction::Retry {
                            guild_id: self.guild_id.clone(),
                        });
                    }
                }
                Stage::Done(outcome) => {
                    let (text, col) = match outcome {
                        ScreeningOutcome::Approved => (
                            "All done! You can talk in this server now.".to_string(),
                            pal.online,
                        ),
                        ScreeningOutcome::Submitted => (
                            "Application sent. A moderator will review it.".to_string(),
                            pal.idle,
                        ),
                        ScreeningOutcome::Rejected(reason) => (
                            match reason {
                                Some(r) => format!("Your application was rejected: {r}"),
                                None => "Your application was rejected.".to_string(),
                            },
                            pal.danger,
                        ),
                    };
                    ui.label(RichText::new(text).color(col));
                }
                Stage::Ready(form) => form_ui(ui, pal, form),
            }
            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(pal.danger).size(13.0));
            }
            ui.add_space(10.0);
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match &self.stage {
                    Stage::Ready(form) => {
                        let valid = form.validate();
                        let label = if self.submitting {
                            "Submitting…"
                        } else {
                            "Submit"
                        };
                        let b = pal.primary_button(label).min_size(vec2(96.0, 32.0));
                        let resp = ui.add_enabled(valid.is_ok() && !self.submitting, b);
                        let resp = match &valid {
                            Err(why) => resp.on_disabled_hover_text(why),
                            Ok(()) => resp,
                        };
                        if resp.clicked() {
                            self.submitting = true;
                            self.error = None;
                            action = Some(ScreeningAction::Submit {
                                guild_id: self.guild_id.clone(),
                                body: form.payload(),
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(ScreeningAction::Close);
                        }
                    }
                    _ => {
                        if ui.button("Close").clicked() {
                            action = Some(ScreeningAction::Close);
                        }
                    }
                },
            );
        });
        if action.is_none() && modal.should_close() && !self.submitting {
            action = Some(ScreeningAction::Close);
        }
        action
    }
}

fn form_ui(ui: &mut egui::Ui, pal: &Palette, form: &mut ScreeningForm) {
    egui::ScrollArea::vertical()
        .max_height(380.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if let Some(d) = &form.form.description {
                ui.label(RichText::new(d).color(pal.text));
                ui.add_space(8.0);
            }
            for (i, (f, a)) in form
                .form
                .fields
                .iter()
                .zip(form.answers.iter_mut())
                .enumerate()
            {
                ui.push_id(i, |ui| field_ui(ui, pal, f, a));
                ui.add_space(10.0);
            }
        });
}

fn field_ui(ui: &mut egui::Ui, pal: &Palette, f: &FormField, a: &mut Answer) {
    let title = match (&f.kind, f.label.is_empty()) {
        (FieldKind::Terms, true) => "Server rules",
        (_, true) => "Question",
        _ => f.label.as_str(),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.label(pal.section_label(title));
        if f.required && !matches!(f.kind, FieldKind::Terms) {
            ui.label(RichText::new("*").color(pal.danger));
        }
    });
    if let Some(d) = &f.description {
        ui.label(RichText::new(d).size(13.0).color(pal.muted));
    }
    ui.add_space(4.0);
    match a {
        Answer::Agree(ok) => {
            egui::Frame::new()
                .fill(pal.input)
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (n, rule) in f.values.iter().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("{}.", n + 1))
                                    .bold()
                                    .color(pal.strong),
                            );
                            ui.add(egui::Label::new(RichText::new(rule).color(pal.text)).wrap());
                        });
                        ui.add_space(2.0);
                    }
                });
            ui.add_space(6.0);
            agree_row(ui, pal, ok);
        }
        Answer::Text(t) => {
            let max = if f.kind == FieldKind::Paragraph {
                PARAGRAPH_MAX
            } else {
                TEXT_INPUT_MAX
            };
            let mut edit = if f.kind == FieldKind::Paragraph {
                egui::TextEdit::multiline(t).desired_rows(4)
            } else {
                egui::TextEdit::singleline(t)
            };
            edit = edit.desired_width(f32::INFINITY).char_limit(max);
            if let Some(p) = &f.placeholder {
                edit = edit.hint_text(p.as_str());
            }
            ui.add(edit);
            ui.label(
                RichText::new(format!("{}/{max}", t.chars().count()))
                    .size(11.5)
                    .color(pal.muted),
            );
        }
        Answer::Choice(c) => {
            for (i, choice) in f.values.iter().enumerate() {
                if ui.radio(*c == Some(i), choice.as_str()).clicked() {
                    *c = Some(i);
                }
            }
        }
        Answer::Unsupported => {
            ui.label(
                RichText::new(
                    "RustCord can't show this question yet. Answer it in the official app.",
                )
                .color(pal.idle),
            );
        }
    }
}

/// "I have read and agree to the rules" with a Discord-like checkbox.
fn agree_row(ui: &mut egui::Ui, pal: &Palette, ok: &mut bool) {
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if resp.clicked() {
        *ok = !*ok;
    }
    let p = ui.painter();
    let bx = egui::Rect::from_min_size(
        egui::pos2(row.left(), row.center().y - 10.0),
        vec2(20.0, 20.0),
    );
    if *ok {
        p.rect_filled(bx, 4, pal.accent);
        let c = bx.center();
        let s = Stroke::new(2.0, Color32::WHITE);
        p.line_segment([c + vec2(-5.0, 0.0), c + vec2(-1.5, 3.5)], s);
        p.line_segment([c + vec2(-1.5, 3.5), c + vec2(5.0, -4.0)], s);
    } else {
        p.rect_stroke(
            bx,
            4,
            Stroke::new(1.5, if resp.hovered() { pal.text } else { pal.muted }),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        egui::pos2(bx.right() + 10.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        "I have read and agree to the rules",
        egui::FontId::proportional(14.5),
        pal.text,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
}

/// The bar above the composer in a server where I'm `pending`: "Complete
/// these steps to start talking" + Complete. Returns true when Complete was
/// clicked (open a [`ScreeningDialog`] and load the form).
pub fn pending_banner(ui: &mut egui::Ui, pal: &Palette) -> bool {
    let mut clicked = false;
    egui::Frame::new()
        .fill(pal.input)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                shield_icon(ui.painter(), r, pal.accent);
                ui.label(
                    RichText::new("Complete these steps to start talking")
                        .color(pal.strong)
                        .bold(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = ui
                        .add(pal.primary_button("Complete").min_size(vec2(90.0, 30.0)))
                        .clicked();
                });
            });
        });
    clicked
}

/// A simple shield outline with a check.
fn shield_icon(p: &egui::Painter, r: egui::Rect, color: Color32) {
    let c = r.center();
    let pts = vec![
        c + vec2(0.0, -8.0),
        c + vec2(7.0, -5.0),
        c + vec2(6.0, 3.0),
        c + vec2(0.0, 8.5),
        c + vec2(-6.0, 3.0),
        c + vec2(-7.0, -5.0),
    ];
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.8, color)));
    let s = Stroke::new(1.8, color);
    p.line_segment([c + vec2(-3.0, 0.0), c + vec2(-0.5, 2.5)], s);
    p.line_segment([c + vec2(-0.5, 2.5), c + vec2(3.5, -2.5)], s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_form() -> Value {
        json!({
            "version": "2021-08-08T19:40:22.735000+00:00",
            "description": "Be nice",
            "form_fields": [{
                "field_type": "TERMS",
                "label": "Read and agree to the server rules",
                "values": ["No spam", "No NSFW"],
                "required": true,
                "description": null,
                "automations": null
            }]
        })
    }

    #[test]
    fn parses_form() {
        let f = VerificationForm::from_json(&rules_form());
        assert_eq!(f.version, json!("2021-08-08T19:40:22.735000+00:00"));
        assert_eq!(f.description.as_deref(), Some("Be nice"));
        assert_eq!(f.fields.len(), 1);
        assert_eq!(f.fields[0].kind, FieldKind::Terms);
        assert_eq!(f.fields[0].values, ["No spam", "No NSFW"]);
        assert!(f.fields[0].required);
        assert!(VerificationForm::from_json(&Value::Null).fields.is_empty());
    }

    #[test]
    fn terms_payload() {
        let mut s = ScreeningForm::new(VerificationForm::from_json(&rules_form()));
        assert!(s.validate().is_err(), "must agree first");
        s.answers[0] = Answer::Agree(true);
        assert!(s.validate().is_ok());
        let body = s.payload();
        assert_eq!(body["version"], "2021-08-08T19:40:22.735000+00:00");
        let field = &body["form_fields"][0];
        assert_eq!(field["response"], json!(true));
        // The original field is echoed back unchanged.
        assert_eq!(field["field_type"], "TERMS");
        assert_eq!(field["values"], json!(["No spam", "No NSFW"]));
        assert_eq!(field["automations"], Value::Null);
        assert_eq!(body["form_fields"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn application_payload_and_validation() {
        let form = VerificationForm::from_json(&json!({
            "version": "v1",
            "form_fields": [
                { "field_type": "TEXT_INPUT", "label": "Name", "required": true },
                { "field_type": "PARAGRAPH", "label": "Why", "required": false },
                { "field_type": "MULTIPLE_CHOICE", "label": "Age", "choices": ["<18", "18+"], "required": true },
                { "field_type": "FUTURE_THING", "label": "?", "required": false }
            ]
        }));
        let mut s = ScreeningForm::new(form);
        assert!(s.validate().unwrap_err().contains("Name"));
        s.answers[0] = Answer::Text("  Leo ".into());
        assert!(s.validate().unwrap_err().contains("Age"));
        s.answers[2] = Answer::Choice(Some(1));
        assert!(s.validate().is_ok());
        s.answers[1] = Answer::Text("x".repeat(PARAGRAPH_MAX + 1));
        assert!(s.validate().unwrap_err().contains("too long"));
        s.answers[1] = Answer::Text(String::new());

        let body = s.payload();
        let f = body["form_fields"].as_array().unwrap();
        assert_eq!(f[0]["response"], "Leo");
        assert_eq!(f[1]["response"], "");
        assert_eq!(f[2]["response"], 1);
        assert_eq!(f[3]["response"], Value::Null);

        // A required unknown field blocks submitting.
        let s = ScreeningForm::new(VerificationForm::from_json(&json!({
            "form_fields": [{ "field_type": "FUTURE_THING", "required": true }]
        })));
        assert!(s.validate().is_err());
    }

    #[test]
    fn outcomes_and_pending() {
        assert_eq!(classify_submit(&Value::Null), ScreeningOutcome::Approved);
        assert_eq!(
            classify_submit(&json!({ "application_status": "APPROVED" })),
            ScreeningOutcome::Approved
        );
        assert_eq!(
            classify_submit(&json!({ "application_status": "SUBMITTED" })),
            ScreeningOutcome::Submitted
        );
        assert_eq!(
            classify_submit(&json!({ "application_status": "REJECTED", "rejection_reason": "no" })),
            ScreeningOutcome::Rejected(Some("no".into()))
        );
        assert!(member_is_pending(&json!({ "pending": true })));
        assert!(!member_is_pending(&json!({ "pending": false })));
        assert!(!member_is_pending(&json!({})));
        assert!(guild_has_screening(&[FEATURE.to_string()]));
        assert!(!guild_has_screening(&["COMMUNITY".to_string()]));
    }
}
