//! Server onboarding: the questions a server asks new members ("What are
//! you interested in?") that hand out roles and channels, plus the
//! "Channels & Roles" page to change the answers later.
//!
//! Data in, actions out: the widgets never do networking. The caller loads
//! the questions with [`load`], shows an [`OnboardingFlow`] (step-through
//! modal) or a [`ChannelsRoles`] page, and on [`OnboardingAction::Submit`]
//! sends the body with [`submit`]. Answers are only ever saved because the
//! user pressed Finish / Save.
//!
//! Protocol notes (Discord's web client; partly undocumented, so parsed
//! leniently and worth checking against the real client):
//! - `GET guilds/{id}/onboarding` → `{ guild_id, prompts, default_channel_ids,
//!   enabled, mode }`. A prompt: `{ id, type (0 multiple choice, 1 dropdown),
//!   title, single_select, required, in_onboarding, options }`; an option:
//!   `{ id, title, description, emoji: { id, name, animated } (or flattened
//!   emoji_id / emoji_name / emoji_animated), role_ids, channel_ids }`.
//! - `PUT guilds/{id}/onboarding-responses` with
//!   `{ onboarding_responses: [option ids], onboarding_prompts_seen:
//!   { prompt_id: unix_ms }, onboarding_responses_seen: { option_id: unix_ms } }`.
//!   We believe the list *replaces* all earlier answers (so the Channels &
//!   Roles page sends the full set), and that the seen maps take unix
//!   milliseconds. The answer echoes `onboarding_responses`.
//! - Earlier answers: the onboarding GET is not known to include them for
//!   users. We take them from a previous PUT answer, from an
//!   `onboarding_responses` field if one shows up, or guess them from the
//!   roles I hold ([`guess_selected`]). Options that only add channels can't
//!   be guessed (they live in channel opt-in settings), so they start off.
//! - `required` only applies to prompts shown during onboarding
//!   (`in_onboarding`). Member flags: COMPLETED_ONBOARDING = 1 << 1,
//!   STARTED_ONBOARDING = 1 << 3; the server feature is `GUILD_ONBOARDING`.
//!   With no onboarding questions, Finish still sends an empty answer list
//!   to mark onboarding complete (believed to be what the client does).

use std::collections::{BTreeMap, HashSet};

use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use serde_json::{Value, json};

use crate::api::Api;
use crate::theme::{self, Bold, Palette};

pub const FEATURE: &str = "GUILD_ONBOARDING";
pub const MEMBER_FLAG_COMPLETED_ONBOARDING: u64 = 1 << 1;
#[cfg(test)]
pub const MEMBER_FLAG_STARTED_ONBOARDING: u64 = 1 << 3;

/// True if I should see the onboarding flow for this server.
pub fn needs_onboarding(features: &[String], member_flags: u64) -> bool {
    features.iter().any(|f| f == FEATURE) && member_flags & MEMBER_FLAG_COMPLETED_ONBOARDING == 0
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn ids_at(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An option's emoji: custom (id) or Unicode (name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionEmoji {
    pub id: Option<String>,
    pub name: Option<String>,
    pub animated: bool,
}

impl OptionEmoji {
    fn from_option(v: &Value) -> Option<Self> {
        let e = &v["emoji"];
        let (id, name, animated) = if e.is_object() {
            (
                str_at(e, "id"),
                str_at(e, "name"),
                e["animated"].as_bool().unwrap_or(false),
            )
        } else {
            (
                str_at(v, "emoji_id"),
                str_at(v, "emoji_name"),
                v["emoji_animated"].as_bool().unwrap_or(false),
            )
        };
        (id.is_some() || name.is_some()).then_some(Self { id, name, animated })
    }

    /// Image URL: Discord CDN for custom emoji, Twemoji for Unicode.
    pub fn url(&self) -> Option<String> {
        if let Some(id) = &self.id {
            let ext = if self.animated { "gif" } else { "png" };
            return Some(format!(
                "https://cdn.discordapp.com/emojis/{id}.{ext}?size=48"
            ));
        }
        let name = self.name.as_deref()?;
        crate::emoji::unicode_segments(name)
            .into_iter()
            .find_map(|s| match s {
                crate::emoji::Segment::Emoji { url, .. } => Some(url),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOption {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub emoji: Option<OptionEmoji>,
    pub role_ids: Vec<String>,
    pub channel_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    MultipleChoice,
    Dropdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub id: String,
    pub kind: PromptKind,
    pub title: String,
    pub single_select: bool,
    pub required: bool,
    pub in_onboarding: bool,
    pub options: Vec<PromptOption>,
}

impl Prompt {
    fn from_json(v: &Value) -> Option<Self> {
        let options: Vec<PromptOption> = v["options"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some(PromptOption {
                            id: str_at(o, "id")?,
                            title: str_at(o, "title").unwrap_or_default(),
                            description: str_at(o, "description"),
                            emoji: OptionEmoji::from_option(o),
                            role_ids: ids_at(o, "role_ids"),
                            channel_ids: ids_at(o, "channel_ids"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id: str_at(v, "id")?,
            kind: if v["type"].as_u64() == Some(1) {
                PromptKind::Dropdown
            } else {
                PromptKind::MultipleChoice
            },
            title: str_at(v, "title").unwrap_or_default(),
            single_select: v["single_select"].as_bool().unwrap_or(false),
            required: v["required"].as_bool().unwrap_or(false),
            in_onboarding: v["in_onboarding"].as_bool().unwrap_or(true),
            options,
        })
    }

    /// True unless it's required and nothing is picked.
    pub fn satisfied(&self, selected: &HashSet<String>) -> bool {
        !self.required || self.options.iter().any(|o| selected.contains(&o.id))
    }
}

/// A server's onboarding questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Onboarding {
    pub guild_id: String,
    pub prompts: Vec<Prompt>,
    pub default_channel_ids: Vec<String>,
    pub enabled: bool,
    /// 0 default, 1 advanced.
    pub mode: u64,
    /// Earlier answers, if the payload had them.
    pub responses: Vec<String>,
}

impl Onboarding {
    pub fn from_json(v: &Value) -> Self {
        Self {
            guild_id: str_at(v, "guild_id").unwrap_or_default(),
            prompts: v["prompts"]
                .as_array()
                .map(|a| a.iter().filter_map(Prompt::from_json).collect())
                .unwrap_or_default(),
            default_channel_ids: ids_at(v, "default_channel_ids"),
            enabled: v["enabled"].as_bool().unwrap_or(true),
            mode: v["mode"].as_u64().unwrap_or(0),
            responses: existing_responses(v),
        }
    }

    /// Prompts shown during onboarding, in order.
    pub fn onboarding_prompts(&self) -> Vec<&Prompt> {
        self.prompts.iter().filter(|p| p.in_onboarding).collect()
    }
}

/// `onboarding_responses` from a PUT answer (or any payload carrying it).
pub fn existing_responses(v: &Value) -> Vec<String> {
    ids_at(v, "onboarding_responses")
}

/// Options I probably picked before: every option whose roles I all hold.
pub fn guess_selected(onb: &Onboarding, my_roles: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for p in &onb.prompts {
        for o in &p.options {
            if !o.role_ids.is_empty() && o.role_ids.iter().all(|r| my_roles.contains(r)) {
                out.insert(o.id.clone());
                if p.single_select {
                    break;
                }
            }
        }
    }
    out
}

/// Pick / unpick an option. Single-select prompts keep at most one.
pub fn toggle(prompt: &Prompt, selected: &mut HashSet<String>, option_id: &str) {
    if selected.remove(option_id) {
        return;
    }
    if prompt.single_select {
        for o in &prompt.options {
            selected.remove(&o.id);
        }
    }
    selected.insert(option_id.to_string());
}

/// Ids of prompts among `prompts` that are required but unanswered.
pub fn missing_required(prompts: &[&Prompt], selected: &HashSet<String>) -> Vec<String> {
    prompts
        .iter()
        .filter(|p| !p.satisfied(selected))
        .map(|p| p.id.clone())
        .collect()
}

/// Body for `PUT guilds/{id}/onboarding-responses`: every selected option
/// (in prompt order), and `now_ms` for each seen prompt and its options.
pub fn responses_payload(
    prompts: &[Prompt],
    selected: &HashSet<String>,
    seen_prompts: &[String],
    now_ms: u64,
) -> Value {
    let responses: Vec<&str> = prompts
        .iter()
        .flat_map(|p| p.options.iter())
        .filter(|o| selected.contains(&o.id))
        .map(|o| o.id.as_str())
        .collect();
    let mut prompts_seen = BTreeMap::new();
    let mut responses_seen = BTreeMap::new();
    for p in prompts.iter().filter(|p| seen_prompts.contains(&p.id)) {
        prompts_seen.insert(p.id.clone(), now_ms);
        for o in &p.options {
            responses_seen.insert(o.id.clone(), now_ms);
        }
    }
    json!({
        "onboarding_responses": responses,
        "onboarding_prompts_seen": prompts_seen,
        "onboarding_responses_seen": responses_seen,
    })
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Backend helper: load a server's onboarding questions.
pub async fn load(api: &Api, guild_id: &str) -> Result<Onboarding, String> {
    api.guild_onboarding(guild_id)
        .await
        .map(|v| {
            let mut o = Onboarding::from_json(&v);
            if o.guild_id.is_empty() {
                o.guild_id = guild_id.to_string();
            }
            o
        })
        .map_err(|e| e.to_string())
}

/// Backend helper: save answers. Returns the saved option ids. Only after
/// the user pressed Finish / Save.
pub async fn submit(api: &Api, guild_id: &str, body: &Value) -> Result<Vec<String>, String> {
    api.put_onboarding_responses(guild_id, body)
        .await
        .map(|v| {
            let saved = existing_responses(&v);
            if saved.is_empty() && v.get("onboarding_responses").is_none() {
                existing_responses(body)
            } else {
                saved
            }
        })
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// What the onboarding widgets ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum OnboardingAction {
    /// Send `PUT guilds/{guild_id}/onboarding-responses` with `body`, then
    /// call `set_saved`.
    Submit {
        guild_id: String,
        body: Value,
    },
    Close,
}

/// The step-through onboarding modal (one question per page).
#[derive(Debug, Clone)]
pub struct OnboardingFlow {
    pub guild_name: String,
    onb: Onboarding,
    selected: HashSet<String>,
    /// Indices into `onb.prompts` of the onboarding questions.
    steps: Vec<usize>,
    step: usize,
    seen: Vec<String>,
    submitting: bool,
    error: Option<String>,
    done: bool,
}

impl OnboardingFlow {
    /// `prior`: earlier answers (see the module docs).
    pub fn new(onb: Onboarding, guild_name: &str, prior: HashSet<String>) -> Self {
        let steps: Vec<usize> = onb
            .prompts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.in_onboarding && !p.options.is_empty())
            .map(|(i, _)| i)
            .collect();
        let mut f = Self {
            guild_name: guild_name.to_string(),
            onb,
            selected: prior,
            steps,
            step: 0,
            seen: Vec::new(),
            submitting: false,
            error: None,
            done: false,
        };
        f.mark_seen();
        f
    }

    pub fn guild_id(&self) -> &str {
        &self.onb.guild_id
    }

    #[cfg(test)]
    pub fn step(&self) -> usize {
        self.step
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 >= self.steps.len()
    }

    pub fn current(&self) -> Option<&Prompt> {
        self.steps.get(self.step).map(|&i| &self.onb.prompts[i])
    }

    #[cfg(test)]
    pub fn selected(&self) -> &HashSet<String> {
        &self.selected
    }

    fn mark_seen(&mut self) {
        if let Some(id) = self.current().map(|p| p.id.clone())
            && !self.seen.contains(&id)
        {
            self.seen.push(id);
        }
    }

    pub fn toggle(&mut self, option_id: &str) {
        if let Some(&i) = self.steps.get(self.step) {
            toggle(&self.onb.prompts[i], &mut self.selected, option_id);
            self.error = None;
        }
    }

    /// Ok if the current question may be left (required ones need a pick).
    pub fn can_continue(&self) -> Result<(), String> {
        match self.current() {
            Some(p) if !p.satisfied(&self.selected) => {
                Err("This question is required. Pick at least one answer.".into())
            }
            _ => Ok(()),
        }
    }

    /// Next question. False if blocked (required) or already last.
    pub fn next(&mut self) -> bool {
        if let Err(e) = self.can_continue() {
            self.error = Some(e);
            return false;
        }
        if self.is_last() {
            return false;
        }
        self.step += 1;
        self.mark_seen();
        true
    }

    pub fn back(&mut self) {
        self.step = self.step.saturating_sub(1);
        self.error = None;
    }

    /// Skip an optional question: clear its answers and move on. False for
    /// required questions.
    pub fn skip(&mut self) -> bool {
        let Some(p) = self.current() else {
            return false;
        };
        if p.required {
            return false;
        }
        let ids: Vec<String> = p.options.iter().map(|o| o.id.clone()).collect();
        for id in ids {
            self.selected.remove(&id);
        }
        if !self.is_last() {
            self.step += 1;
            self.mark_seen();
        }
        true
    }

    /// The PUT body, or why it can't be sent yet (a required question is
    /// unanswered).
    pub fn payload(&self, now_ms: u64) -> Result<Value, String> {
        let onboarding: Vec<&Prompt> = self.steps.iter().map(|&i| &self.onb.prompts[i]).collect();
        if let Some(missing) = missing_required(&onboarding, &self.selected).first() {
            let title = self
                .onb
                .prompts
                .iter()
                .find(|p| &p.id == missing)
                .map(|p| p.title.as_str())
                .unwrap_or("A question");
            return Err(format!("\"{title}\" is required."));
        }
        Ok(responses_payload(
            &self.onb.prompts,
            &self.selected,
            &self.seen,
            now_ms,
        ))
    }

    /// The save answer arrived.
    pub fn set_saved(&mut self, result: Result<Vec<String>, String>) {
        self.submitting = false;
        match result {
            Ok(_) => self.done = true,
            Err(e) => self.error = Some(e),
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn show(&mut self, ctx: &egui::Context, pal: &Palette) -> Option<OnboardingAction> {
        let mut action = None;
        let modal = egui::Modal::new(egui::Id::unique("rc_onboarding")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(pal.section_label(&self.guild_name));
            if self.done {
                ui.add_space(6.0);
                ui.heading("You're all set!");
                ui.label(
                    RichText::new("Your roles and channels have been updated.").color(pal.muted),
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(pal.primary_button("Done")).clicked() {
                        action = Some(OnboardingAction::Close);
                    }
                });
                return;
            }
            let total = self.step_count();
            if total > 0 {
                progress_bar(ui, pal, self.step + 1, total);
            }
            ui.add_space(6.0);
            match self.current().cloned() {
                Some(p) => {
                    ui.heading(&p.title);
                    ui.horizontal(|ui| {
                        let hint = if p.single_select {
                            "Pick one"
                        } else {
                            "Pick as many as you like"
                        };
                        ui.label(RichText::new(hint).size(13.0).color(pal.muted));
                        if p.required {
                            ui.label(RichText::new("Required").size(13.0).color(pal.danger));
                        }
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if let Some(id) = prompt_options(ui, pal, &p, &self.selected) {
                                self.toggle(&id);
                            }
                        });
                }
                None => {
                    ui.heading("Welcome!");
                    ui.label(
                        RichText::new(
                            "This server has no questions for you. Press Finish to start.",
                        )
                        .color(pal.muted),
                    );
                }
            }
            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).size(13.0).color(pal.danger));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if self.step > 0 && ui.button("Back").clicked() {
                    self.back();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = self.can_continue();
                    if self.is_last() {
                        let label = if self.submitting {
                            "Saving…"
                        } else {
                            "Finish"
                        };
                        let b = pal.primary_button(label).min_size(vec2(96.0, 32.0));
                        if ui.add_enabled(ok.is_ok() && !self.submitting, b).clicked() {
                            match self.payload(now_ms()) {
                                Ok(body) => {
                                    self.submitting = true;
                                    self.error = None;
                                    action = Some(OnboardingAction::Submit {
                                        guild_id: self.onb.guild_id.clone(),
                                        body,
                                    });
                                }
                                Err(e) => self.error = Some(e),
                            }
                        }
                    } else {
                        let b = pal.primary_button("Next").min_size(vec2(96.0, 32.0));
                        if ui.add_enabled(ok.is_ok(), b).clicked() {
                            self.next();
                        }
                    }
                    let optional = self.current().is_some_and(|p| !p.required);
                    if optional
                        && !self.is_last()
                        && !self.submitting
                        && ui.button("Skip").clicked()
                    {
                        self.skip();
                    }
                });
            });
        });
        if action.is_none() && modal.should_close() && !self.submitting {
            action = Some(OnboardingAction::Close);
        }
        action
    }
}

fn progress_bar(ui: &mut egui::Ui, pal: &Palette, n: usize, total: usize) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width() - 60.0, 6.0), Sense::hover());
        ui.painter().rect_filled(r, 3, pal.input);
        let mut done = r;
        done.set_width(r.width() * n as f32 / total as f32);
        ui.painter().rect_filled(done, 3, pal.accent);
        ui.label(
            RichText::new(format!("{n} of {total}"))
                .size(11.5)
                .color(pal.muted),
        );
    });
}

/// The options of one prompt. Returns the id of a clicked option.
fn prompt_options(
    ui: &mut egui::Ui,
    pal: &Palette,
    p: &Prompt,
    selected: &HashSet<String>,
) -> Option<String> {
    let mut clicked = None;
    ui.push_id(&p.id, |ui| match p.kind {
        PromptKind::MultipleChoice => {
            for o in &p.options {
                if option_card(ui, pal, o, selected.contains(&o.id), p.single_select).clicked() {
                    clicked = Some(o.id.clone());
                }
                ui.add_space(6.0);
            }
        }
        PromptKind::Dropdown => {
            let summary: Vec<&str> = p
                .options
                .iter()
                .filter(|o| selected.contains(&o.id))
                .map(|o| o.title.as_str())
                .collect();
            let text = if summary.is_empty() {
                "Select…".to_string()
            } else {
                summary.join(", ")
            };
            egui::ComboBox::from_id_salt("dropdown")
                .selected_text(text)
                .width(ui.available_width().min(420.0))
                .show_ui(ui, |ui| {
                    for o in &p.options {
                        let on = selected.contains(&o.id);
                        if ui.selectable_label(on, &o.title).clicked() {
                            clicked = Some(o.id.clone());
                        }
                    }
                });
        }
    });
    clicked
}

/// A selectable answer card: emoji, title, description, and a check box /
/// radio on the right.
fn option_card(
    ui: &mut egui::Ui,
    pal: &Palette,
    o: &PromptOption,
    on: bool,
    single: bool,
) -> egui::Response {
    let has_desc = o.description.is_some();
    let h = if has_desc { 58.0 } else { 44.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let hovered = resp.hovered();
    let fill = if on {
        pal.selected
    } else if hovered {
        pal.hover
    } else {
        pal.input
    };
    let p = ui.painter();
    p.rect_filled(rect, 8, fill);
    if on {
        p.rect_stroke(
            rect,
            8,
            Stroke::new(1.5, pal.accent),
            egui::StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + 12.0;
    if let Some(url) = o.emoji.as_ref().and_then(OptionEmoji::url) {
        let er =
            egui::Rect::from_center_size(egui::pos2(x + 12.0, rect.center().y), vec2(24.0, 24.0));
        egui::Image::from_uri(url)
            .show_loading_spinner(false)
            .paint_at(ui, er);
        x += 34.0;
    }
    let right = rect.right() - 40.0;
    let p = ui.painter();
    let title_y = if has_desc {
        rect.top() + 18.0
    } else {
        rect.center().y
    };
    let title = p.layout(
        o.title.clone(),
        theme::bold_font(15.0),
        pal.strong,
        right - x,
    );
    p.galley(
        egui::pos2(x, title_y - title.size().y / 2.0),
        title,
        pal.strong,
    );
    if let Some(d) = &o.description {
        let mut job = egui::text::LayoutJob::simple_singleline(
            d.clone(),
            egui::FontId::proportional(13.0),
            pal.muted,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(right - x);
        let g = ui.painter().layout_job(job);
        ui.painter()
            .galley(egui::pos2(x, rect.top() + 32.0), g, pal.muted);
    }
    // Check box / radio.
    let c = egui::pos2(rect.right() - 22.0, rect.center().y);
    let p = ui.painter();
    if single {
        p.circle_stroke(
            c,
            9.0,
            Stroke::new(1.5, if on { pal.accent } else { pal.muted }),
        );
        if on {
            p.circle_filled(c, 5.0, pal.accent);
        }
    } else {
        let b = egui::Rect::from_center_size(c, vec2(18.0, 18.0));
        if on {
            p.rect_filled(b, 4, pal.accent);
            let s = Stroke::new(2.0, Color32::WHITE);
            p.line_segment([c + vec2(-4.5, 0.0), c + vec2(-1.0, 3.5)], s);
            p.line_segment([c + vec2(-1.0, 3.5), c + vec2(4.5, -3.5)], s);
        } else {
            p.rect_stroke(b, 4, Stroke::new(1.5, pal.muted), egui::StrokeKind::Inside);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "Channels & Roles": every question on one page, to change answers after
/// onboarding. Host it in the messages area (like Discord) or a modal.
#[derive(Debug, Clone)]
pub struct ChannelsRoles {
    pub guild_name: String,
    onb: Onboarding,
    selected: HashSet<String>,
    saved: HashSet<String>,
    saving: bool,
    error: Option<String>,
}

impl ChannelsRoles {
    /// `current`: the answers as they stand (see the module docs).
    pub fn new(onb: Onboarding, guild_name: &str, current: HashSet<String>) -> Self {
        Self {
            guild_name: guild_name.to_string(),
            onb,
            selected: current.clone(),
            saved: current,
            saving: false,
            error: None,
        }
    }

    pub fn guild_id(&self) -> &str {
        &self.onb.guild_id
    }

    pub fn dirty(&self) -> bool {
        self.selected != self.saved
    }

    pub fn toggle(&mut self, prompt_id: &str, option_id: &str) {
        if let Some(p) = self.onb.prompts.iter().find(|p| p.id == prompt_id) {
            toggle(p, &mut self.selected, option_id);
            self.error = None;
        }
    }

    /// The PUT body (every prompt counts as seen), or why not.
    pub fn payload(&self, now_ms: u64) -> Result<Value, String> {
        let onboarding = self.onb.onboarding_prompts();
        if let Some(missing) = missing_required(&onboarding, &self.selected).first() {
            let title = onboarding
                .iter()
                .find(|p| &p.id == missing)
                .map(|p| p.title.as_str())
                .unwrap_or("A question");
            return Err(format!("\"{title}\" is required."));
        }
        let seen: Vec<String> = self.onb.prompts.iter().map(|p| p.id.clone()).collect();
        Ok(responses_payload(
            &self.onb.prompts,
            &self.selected,
            &seen,
            now_ms,
        ))
    }

    /// The save answer arrived (`Ok(saved ids)`).
    pub fn set_saved(&mut self, result: Result<Vec<String>, String>) {
        self.saving = false;
        match result {
            Ok(ids) => {
                self.saved = ids.into_iter().collect();
                self.selected = self.saved.clone();
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<OnboardingAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.heading("Channels & Roles");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    action = Some(OnboardingAction::Close);
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "Answer questions to get access to more channels and roles in {}.",
                self.guild_name
            ))
            .color(pal.muted),
        );
        ui.add_space(8.0);
        let footer_h = if self.dirty() || self.error.is_some() {
            52.0
        } else {
            0.0
        };
        let mut clicked: Option<(String, String)> = None;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - footer_h).max(120.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.onb.prompts.is_empty() {
                    ui.label(RichText::new("This server has no questions.").color(pal.muted));
                }
                for p in &self.onb.prompts {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&p.title).size(16.0).bold().color(pal.strong));
                        if p.required && p.in_onboarding {
                            ui.label(RichText::new("Required").size(11.5).color(pal.danger));
                        }
                    });
                    ui.add_space(4.0);
                    if let Some(id) = prompt_options(ui, pal, p, &self.selected) {
                        clicked = Some((p.id.clone(), id));
                    }
                    ui.add_space(14.0);
                }
            });
        if let Some((p, o)) = clicked {
            self.toggle(&p, &o);
        }
        if footer_h > 0.0 {
            egui::Frame::new()
                .fill(pal.servers)
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        match &self.error {
                            Some(e) => ui.label(RichText::new(e).color(pal.danger)),
                            None => ui.label(
                                RichText::new("Careful — you have unsaved changes!")
                                    .color(pal.text),
                            ),
                        };
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if self.saving {
                                "Saving…"
                            } else {
                                "Save Changes"
                            };
                            let save = egui::Button::new(
                                RichText::new(label).color(Color32::WHITE).bold(),
                            )
                            .fill(pal.online);
                            if ui.add_enabled(self.dirty() && !self.saving, save).clicked() {
                                match self.payload(now_ms()) {
                                    Ok(body) => {
                                        self.saving = true;
                                        self.error = None;
                                        action = Some(OnboardingAction::Submit {
                                            guild_id: self.onb.guild_id.clone(),
                                            body,
                                        });
                                    }
                                    Err(e) => self.error = Some(e),
                                }
                            }
                            if ui
                                .add_enabled(!self.saving, egui::Button::new("Reset"))
                                .clicked()
                            {
                                self.selected = self.saved.clone();
                                self.error = None;
                            }
                        });
                    });
                });
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "guild_id": "42",
            "enabled": true,
            "mode": 0,
            "default_channel_ids": ["100", "101"],
            "prompts": [
                {
                    "id": "p1", "type": 0, "title": "What brings you here?",
                    "single_select": false, "required": true, "in_onboarding": true,
                    "options": [
                        { "id": "o1", "title": "Mousepads", "description": "Product news",
                          "emoji": { "id": null, "name": "🎮", "animated": false },
                          "role_ids": ["r1"], "channel_ids": ["200"] },
                        { "id": "o2", "title": "Gaming", "description": null,
                          "emoji_id": "555", "emoji_name": "gg", "emoji_animated": true,
                          "role_ids": ["r2", "r3"], "channel_ids": [] }
                    ]
                },
                {
                    "id": "p2", "type": 1, "title": "Region",
                    "single_select": true, "required": false, "in_onboarding": true,
                    "options": [
                        { "id": "o3", "title": "EU", "role_ids": ["r4"] },
                        { "id": "o4", "title": "NA", "role_ids": ["r5"] }
                    ]
                },
                {
                    "id": "p3", "type": 0, "title": "Pings",
                    "single_select": false, "required": true, "in_onboarding": false,
                    "options": [ { "id": "o5", "title": "Drops", "channel_ids": ["300"] } ]
                },
                { "id": "bad" , "options": "junk" },
                { "title": "no id" }
            ]
        })
    }

    #[test]
    fn parses_onboarding() {
        let o = Onboarding::from_json(&sample());
        assert_eq!(o.guild_id, "42");
        assert_eq!(o.prompts.len(), 4, "prompt without id dropped");
        assert_eq!(o.default_channel_ids, ["100", "101"]);
        let p1 = &o.prompts[0];
        assert!(p1.required && p1.in_onboarding && !p1.single_select);
        assert_eq!(p1.kind, PromptKind::MultipleChoice);
        assert_eq!(p1.options[0].description.as_deref(), Some("Product news"));
        assert_eq!(p1.options[1].description, None);
        let e1 = p1.options[0].emoji.as_ref().unwrap();
        assert!(e1.id.is_none());
        assert!(e1.url().unwrap().contains("twemoji"));
        let e2 = p1.options[1].emoji.as_ref().unwrap();
        assert_eq!(
            e2.url().unwrap(),
            "https://cdn.discordapp.com/emojis/555.gif?size=48"
        );
        assert_eq!(o.prompts[1].kind, PromptKind::Dropdown);
        assert!(o.prompts[3].options.is_empty());
        assert_eq!(o.onboarding_prompts().len(), 3);
    }

    #[test]
    fn toggling() {
        let o = Onboarding::from_json(&sample());
        let mut sel = HashSet::new();
        toggle(&o.prompts[0], &mut sel, "o1");
        toggle(&o.prompts[0], &mut sel, "o2");
        assert_eq!(sel.len(), 2, "multi-select keeps both");
        toggle(&o.prompts[0], &mut sel, "o1");
        assert!(!sel.contains("o1"), "second click unpicks");
        toggle(&o.prompts[1], &mut sel, "o3");
        toggle(&o.prompts[1], &mut sel, "o4");
        assert!(sel.contains("o4") && !sel.contains("o3"), "single-select");
        assert!(sel.contains("o2"), "other prompts untouched");
    }

    #[test]
    fn required_validation() {
        let o = Onboarding::from_json(&sample());
        let onb = o.onboarding_prompts();
        let mut sel = HashSet::new();
        assert_eq!(missing_required(&onb, &sel), ["p1"]);
        sel.insert("o2".to_string());
        assert!(missing_required(&onb, &sel).is_empty());
        // p3 is required but not in onboarding: only counts on the full list.
        let all: Vec<&Prompt> = o.prompts.iter().collect();
        assert_eq!(missing_required(&all, &sel), ["p3"]);
    }

    #[test]
    fn payload_shape() {
        let o = Onboarding::from_json(&sample());
        let sel: HashSet<String> = ["o4", "o1"].iter().map(|s| s.to_string()).collect();
        let body = responses_payload(&o.prompts, &sel, &["p2".to_string()], 1_700_000_000_000);
        assert_eq!(body["onboarding_responses"], json!(["o1", "o4"]));
        assert_eq!(
            body["onboarding_prompts_seen"],
            json!({ "p2": 1_700_000_000_000u64 })
        );
        assert_eq!(
            body["onboarding_responses_seen"],
            json!({ "o3": 1_700_000_000_000u64, "o4": 1_700_000_000_000u64 })
        );
        let empty = responses_payload(&[], &HashSet::new(), &[], 1);
        assert_eq!(empty["onboarding_responses"], json!([]));
        assert_eq!(empty["onboarding_prompts_seen"], json!({}));
    }

    #[test]
    fn flow_steps() {
        let o = Onboarding::from_json(&sample());
        let mut f = OnboardingFlow::new(o, "Pegasi", HashSet::new());
        // p1, p2 (p3 isn't in onboarding; "bad" has no options).
        assert_eq!(f.step_count(), 2);
        assert_eq!(f.current().unwrap().id, "p1");
        assert!(!f.skip(), "required can't be skipped");
        assert!(!f.next(), "required needs a pick");
        assert!(f.payload(1).is_err());
        f.toggle("o1");
        assert!(f.next());
        assert_eq!(f.current().unwrap().id, "p2");
        assert!(f.is_last());
        f.toggle("o3");
        assert!(f.skip(), "optional can be skipped");
        assert!(!f.selected().contains("o3"), "skip clears the answer");
        f.back();
        assert_eq!(f.step(), 0);
        f.back();
        assert_eq!(f.step(), 0);

        let body = f.payload(5).unwrap();
        assert_eq!(body["onboarding_responses"], json!(["o1"]));
        assert_eq!(body["onboarding_prompts_seen"], json!({ "p1": 5, "p2": 5 }));
        f.set_saved(Ok(vec!["o1".into()]));
        assert!(f.is_done());
    }

    #[test]
    fn flow_without_questions() {
        let o = Onboarding::from_json(&json!({ "guild_id": "1", "prompts": [] }));
        let f = OnboardingFlow::new(o, "x", HashSet::new());
        assert_eq!(f.step_count(), 0);
        assert!(f.is_last());
        assert!(f.can_continue().is_ok());
        let body = f.payload(1).unwrap();
        assert_eq!(body["onboarding_responses"], json!([]));
    }

    #[test]
    fn channels_roles_page() {
        let o = Onboarding::from_json(&sample());
        let roles = vec!["r2".to_string(), "r3".into(), "r4".into(), "r5".into()];
        let guessed = guess_selected(&o, &roles);
        // o2 (r2+r3); p2 is single-select, so only the first held option.
        assert!(guessed.contains("o2"));
        assert!(guessed.contains("o3") && !guessed.contains("o4"));
        assert!(
            !guessed.contains("o5"),
            "channel-only options can't be guessed"
        );

        let mut page = ChannelsRoles::new(o, "Pegasi", guessed);
        assert!(!page.dirty());
        page.toggle("p3", "o5");
        assert!(page.dirty());
        page.toggle("p1", "o2");
        assert!(page.payload(1).is_err(), "p1 is required in onboarding");
        page.toggle("p1", "o1");
        let body = page.payload(9).unwrap();
        assert_eq!(body["onboarding_responses"], json!(["o1", "o3", "o5"]));
        assert_eq!(
            body["onboarding_prompts_seen"].as_object().unwrap().len(),
            4,
            "every prompt counts as seen"
        );
        page.set_saved(Ok(vec!["o1".into(), "o3".into(), "o5".into()]));
        assert!(!page.dirty());
        page.set_saved(Err("nope".into()));
        assert!(!page.dirty());
    }

    #[test]
    fn needs_flow() {
        let f = vec![FEATURE.to_string()];
        assert!(needs_onboarding(&f, 0));
        assert!(needs_onboarding(&f, MEMBER_FLAG_STARTED_ONBOARDING));
        assert!(!needs_onboarding(&f, MEMBER_FLAG_COMPLETED_ONBOARDING));
        assert!(!needs_onboarding(&[], 0));
        assert_eq!(
            existing_responses(&json!({ "onboarding_responses": ["1", 2] })),
            ["1", "2"]
        );
    }
}
