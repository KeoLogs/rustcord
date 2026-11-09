//! Message reactions: data, gateway event handling, optimistic updates and
//! the Discord-style reaction bar under a message.
//!
//! Discord Messenger doesn't do reactions, so this follows Discord's API:
//!
//! - A message's `reactions` array holds
//!   `{ emoji: { id, name, animated }, count, count_details: { burst, normal },
//!   me, me_burst, burst_colors: ["#rrggbb", …] }`. "Burst" reactions are
//!   Super Reactions; `count` is normal + burst.
//! - Gateway events (fields used, all others ignored):
//!   - `MESSAGE_REACTION_ADD`: `channel_id`, `message_id`, `user_id`,
//!     `emoji`, `burst` (or `type` 1), `burst_colors`.
//!   - `MESSAGE_REACTION_REMOVE`: `channel_id`, `message_id`, `user_id`,
//!     `emoji`, `burst` (or `type` 1).
//!   - `MESSAGE_REACTION_REMOVE_ALL`: `channel_id`, `message_id`.
//!   - `MESSAGE_REACTION_REMOVE_EMOJI`: `channel_id`, `message_id`, `emoji`.
//!
//!   Bots only get these with the GUILD_MESSAGE_REACTIONS (1 << 10) and
//!   DIRECT_MESSAGE_REACTIONS (1 << 13) intents.
//! - REST (see `api.rs`): `PUT/DELETE channels/{c}/messages/{m}/reactions/
//!   {emoji}/@me`, and `GET …/reactions/{emoji}?limit=&type=` for who
//!   reacted.
//!
//! Reacting only ever happens from a click: [`reaction_bar`] returns a
//! [`ReactionAction`] and the caller sends the request.
//!
//! Optimistic updates: the caller applies the change locally through
//! [`PendingReactions::begin`], sends the request, then feeds every gateway
//! reaction event through [`PendingReactions::on_event`] (which skips the
//! echo of our own pending change) and failures through
//! [`PendingReactions::on_failure`] (which undoes it).

use eframe::egui::{self, Color32, Sense, vec2};
use serde_json::Value;

use crate::emoji_picker::{self, CustomEmojiGroup, EmojiRef, Picked, PickerCtx, RecentEmoji};
use crate::theme::{self, Palette};

/// Pending optimistic changes older than this (seconds) are dropped by
/// [`PendingReactions::prune`]. The request has long finished by then.
pub const PENDING_MAX_AGE: f64 = 30.0;
/// Reactors asked for per tooltip.
pub const REACTORS_LIMIT: u8 = 10;

const PILL_H: f32 = 24.0;
const PILL_EMOJI: f32 = 16.0;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// One emoji's reactions on a message.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    pub emoji: EmojiRef,
    /// Normal (non-super) reactions.
    pub normal: u32,
    /// Super Reactions.
    pub burst: u32,
    /// I reacted normally.
    pub me: bool,
    /// I super-reacted.
    pub me_burst: bool,
    /// Super Reaction colours.
    pub burst_colors: Vec<Color32>,
}

impl Reaction {
    fn new(emoji: EmojiRef) -> Self {
        Self {
            emoji,
            normal: 0,
            burst: 0,
            me: false,
            me_burst: false,
            burst_colors: Vec::new(),
        }
    }

    /// Total count (Discord's `count`).
    pub fn count(&self) -> u32 {
        self.normal + self.burst
    }

    /// Parse one entry of a message's `reactions` array.
    pub fn from_json(v: &Value) -> Option<Self> {
        let emoji = EmojiRef::from_json(v.get("emoji")?)?;
        let num = |v: Option<&Value>| v.and_then(Value::as_u64).map(|n| n as u32);
        let count = num(v.get("count")).unwrap_or(0);
        let details = v.get("count_details");
        let burst = num(details.and_then(|d| d.get("burst"))).unwrap_or(0);
        // Older payloads have no count_details: everything is normal.
        let normal =
            num(details.and_then(|d| d.get("normal"))).unwrap_or(count.saturating_sub(burst));
        let r = Self {
            emoji,
            normal,
            burst,
            me: v.get("me").and_then(Value::as_bool).unwrap_or(false),
            me_burst: v.get("me_burst").and_then(Value::as_bool).unwrap_or(false),
            burst_colors: parse_colors(v.get("burst_colors")),
        };
        (r.count() > 0).then_some(r)
    }
}

/// A message's `reactions` (missing / null gives an empty list).
pub fn parse(v: &Value) -> Vec<Reaction> {
    v.as_array()
        .map(|arr| arr.iter().filter_map(Reaction::from_json).collect())
        .unwrap_or_default()
}

/// `["#rrggbb", …]` → colours (bad entries skipped).
fn parse_colors(v: Option<&Value>) -> Vec<Color32> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| {
                    let hex = s.trim_start_matches('#');
                    let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
                    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Gateway events
// ---------------------------------------------------------------------------

/// A parsed reaction gateway event.
#[derive(Debug, Clone, PartialEq)]
pub enum ReactionEvent {
    Add {
        channel_id: String,
        message_id: String,
        user_id: String,
        emoji: EmojiRef,
        burst: bool,
        burst_colors: Vec<Color32>,
    },
    Remove {
        channel_id: String,
        message_id: String,
        user_id: String,
        emoji: EmojiRef,
        burst: bool,
    },
    RemoveAll {
        channel_id: String,
        message_id: String,
    },
    RemoveEmoji {
        channel_id: String,
        message_id: String,
        emoji: EmojiRef,
    },
}

impl ReactionEvent {
    /// Parse a dispatch (`t`, `d`). `None` for other events or payloads
    /// missing the ids.
    pub fn from_dispatch(t: &str, d: &Value) -> Option<Self> {
        let s = |k: &str| {
            d.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let channel_id = s("channel_id")?;
        let message_id = s("message_id")?;
        let burst = d.get("burst").and_then(Value::as_bool).unwrap_or(false)
            || d.get("type").and_then(Value::as_u64) == Some(1);
        let emoji = || d.get("emoji").and_then(EmojiRef::from_json);
        Some(match t {
            "MESSAGE_REACTION_ADD" => Self::Add {
                channel_id,
                message_id,
                user_id: s("user_id")?,
                emoji: emoji()?,
                burst,
                burst_colors: parse_colors(d.get("burst_colors")),
            },
            "MESSAGE_REACTION_REMOVE" => Self::Remove {
                channel_id,
                message_id,
                user_id: s("user_id")?,
                emoji: emoji()?,
                burst,
            },
            "MESSAGE_REACTION_REMOVE_ALL" => Self::RemoveAll {
                channel_id,
                message_id,
            },
            "MESSAGE_REACTION_REMOVE_EMOJI" => Self::RemoveEmoji {
                channel_id,
                message_id,
                emoji: emoji()?,
            },
            _ => return None,
        })
    }

    pub fn channel_id(&self) -> &str {
        match self {
            Self::Add { channel_id, .. }
            | Self::Remove { channel_id, .. }
            | Self::RemoveAll { channel_id, .. }
            | Self::RemoveEmoji { channel_id, .. } => channel_id,
        }
    }

    pub fn message_id(&self) -> &str {
        match self {
            Self::Add { message_id, .. }
            | Self::Remove { message_id, .. }
            | Self::RemoveAll { message_id, .. }
            | Self::RemoveEmoji { message_id, .. } => message_id,
        }
    }
}

/// Apply a gateway event to a message's reactions. `my_user_id` decides
/// `me` / `me_burst`. Our own add/remove is idempotent (adding when `me` is
/// already set changes nothing), so a duplicate never double-counts.
pub fn apply_event(reactions: &mut Vec<Reaction>, ev: &ReactionEvent, my_user_id: &str) {
    match ev {
        ReactionEvent::Add {
            user_id,
            emoji,
            burst,
            burst_colors,
            ..
        } => {
            let mine = user_id == my_user_id;
            let i = match reactions.iter().position(|r| r.emoji.same(emoji)) {
                Some(i) => i,
                None => {
                    reactions.push(Reaction::new(emoji.clone()));
                    reactions.len() - 1
                }
            };
            let r = &mut reactions[i];
            if *burst && r.burst_colors.is_empty() {
                r.burst_colors = burst_colors.clone();
            }
            bump(r, *burst, mine, true);
        }
        ReactionEvent::Remove {
            user_id,
            emoji,
            burst,
            ..
        } => {
            if let Some(r) = reactions.iter_mut().find(|r| r.emoji.same(emoji)) {
                bump(r, *burst, user_id == my_user_id, false);
            }
            reactions.retain(|r| r.count() > 0);
        }
        ReactionEvent::RemoveAll { .. } => reactions.clear(),
        ReactionEvent::RemoveEmoji { emoji, .. } => reactions.retain(|r| !r.emoji.same(emoji)),
    }
}

/// Count one add/remove. For my own reaction only if `me` actually changes.
/// Returns whether anything changed.
fn bump(r: &mut Reaction, burst: bool, mine: bool, add: bool) -> bool {
    let (flag, n) = if burst {
        (&mut r.me_burst, &mut r.burst)
    } else {
        (&mut r.me, &mut r.normal)
    };
    if mine {
        if *flag == add {
            return false;
        }
        *flag = add;
    }
    *n = if add { *n + 1 } else { n.saturating_sub(1) };
    true
}

/// Apply my own add/remove locally. Returns false if it changes nothing
/// (already reacted / not reacted), in which case there's nothing to send.
pub fn apply_local(
    reactions: &mut Vec<Reaction>,
    emoji: &EmojiRef,
    add: bool,
    burst: bool,
) -> bool {
    let changed = match reactions.iter_mut().find(|r| r.emoji.same(emoji)) {
        Some(r) => bump(r, burst, true, add),
        None if add => {
            let mut r = Reaction::new(emoji.clone());
            bump(&mut r, burst, true, true);
            reactions.push(r);
            true
        }
        None => false,
    };
    reactions.retain(|r| r.count() > 0);
    changed
}

// ---------------------------------------------------------------------------
// Optimistic updates
// ---------------------------------------------------------------------------

/// A reaction request we've applied locally but not seen confirmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub channel_id: String,
    pub message_id: String,
    pub emoji: EmojiRef,
    pub add: bool,
    pub burst: bool,
    /// When it was made (`ui.input(|i| i.time)` or any clock in seconds).
    pub since: f64,
}

impl Pending {
    fn matches(&self, message_id: &str, emoji: &EmojiRef, add: bool, burst: bool) -> bool {
        self.message_id == message_id
            && self.add == add
            && self.burst == burst
            && self.emoji.same(emoji)
    }
}

/// Our in-flight reaction changes (usually zero or one).
#[derive(Debug, Clone, Default)]
pub struct PendingReactions {
    pub items: Vec<Pending>,
}

impl PendingReactions {
    /// Apply my click locally and remember it. Returns true if a request
    /// should be sent (false = nothing changed, e.g. already reacted).
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        reactions: &mut Vec<Reaction>,
        channel_id: &str,
        message_id: &str,
        emoji: &EmojiRef,
        add: bool,
        burst: bool,
        now: f64,
    ) -> bool {
        if !apply_local(reactions, emoji, add, burst) {
            return false;
        }
        self.items.push(Pending {
            channel_id: channel_id.to_string(),
            message_id: message_id.to_string(),
            emoji: emoji.clone(),
            add,
            burst,
            since: now,
        });
        true
    }

    /// Apply a gateway event, skipping the echo of a pending change of ours
    /// (it's already applied). Call for every reaction event of a message
    /// you hold, whether or not anything is pending.
    pub fn on_event(
        &mut self,
        reactions: &mut Vec<Reaction>,
        ev: &ReactionEvent,
        my_user_id: &str,
    ) {
        match ev {
            ReactionEvent::Add {
                message_id,
                user_id,
                emoji,
                burst,
                ..
            }
            | ReactionEvent::Remove {
                message_id,
                user_id,
                emoji,
                burst,
                ..
            } if user_id == my_user_id => {
                let add = matches!(ev, ReactionEvent::Add { .. });
                if let Some(i) = self
                    .items
                    .iter()
                    .position(|p| p.matches(message_id, emoji, add, *burst))
                {
                    self.items.remove(i);
                    return;
                }
            }
            ReactionEvent::RemoveAll { message_id, .. } => {
                self.items.retain(|p| &p.message_id != message_id);
            }
            ReactionEvent::RemoveEmoji {
                message_id, emoji, ..
            } => {
                self.items
                    .retain(|p| &p.message_id != message_id || !p.emoji.same(emoji));
            }
            _ => {}
        }
        apply_event(reactions, ev, my_user_id);
    }

    /// The request failed: undo the local change (if it's still pending).
    pub fn on_failure(
        &mut self,
        reactions: &mut Vec<Reaction>,
        message_id: &str,
        emoji: &EmojiRef,
        add: bool,
        burst: bool,
    ) {
        if let Some(i) = self
            .items
            .iter()
            .position(|p| p.matches(message_id, emoji, add, burst))
        {
            self.items.remove(i);
            apply_local(reactions, emoji, !add, burst);
        }
    }

    /// Forget changes older than `max_age` seconds (their echo was lost or
    /// the message was evicted). Their local effect stays.
    pub fn prune(&mut self, now: f64, max_age: f64) {
        self.items.retain(|p| now - p.since <= max_age);
    }

    /// Anything pending for this message? (e.g. to skip replacing its
    /// reactions with a stale REST copy).
    pub fn has_message(&self, message_id: &str) -> bool {
        self.items.iter().any(|p| p.message_id == message_id)
    }
}

// ---------------------------------------------------------------------------
// Tooltip text
// ---------------------------------------------------------------------------

/// Discord's tooltip: "Ann, Bo and 3 others reacted with :fire:".
/// `names` are the reactors loaded so far, `total` the reaction count.
pub fn reactors_text(emoji: &EmojiRef, names: &[String], total: u32) -> String {
    let code = emoji.shortcode();
    let shown = names.len().min(3);
    let others = (total as usize).saturating_sub(shown);
    let names = &names[..shown];
    let who = match (names, others) {
        ([], _) => return format!("{total} reacted with {code}"),
        ([a], 0) => a.clone(),
        ([a, b], 0) => format!("{a} and {b}"),
        ([a, b, c], 0) => format!("{a}, {b} and {c}"),
        (list, n) => format!(
            "{} and {n} other{}",
            list.join(", "),
            if n == 1 { "" } else { "s" }
        ),
    };
    format!("{who} reacted with {code}")
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// What the user did in the reaction bar. The caller turns Add/Remove into
/// [`PendingReactions::begin`] + a `ToBackend` request.
#[derive(Debug, Clone, PartialEq)]
pub enum ReactionAction {
    /// Add my (normal) reaction. `from_picker`: record it in the recent list.
    Add { emoji: EmojiRef, from_picker: bool },
    /// Remove my reaction (`burst`: my Super Reaction).
    Remove { emoji: EmojiRef, burst: bool },
    /// A pill's tooltip is showing but the caller had no text for it: load
    /// who reacted (sent every frame while shown, so de-duplicate).
    WantReactors { emoji: EmojiRef, burst: bool },
}

/// What the reaction bar needs from the caller.
pub struct ReactionCtx<'a> {
    /// Unique per message, e.g. `egui::Id::unique(("reactions", &msg.id))`.
    pub id: egui::Id,
    pub pal: &'a Palette,
    /// Emoji images module on (false: emoji as text, no image loading).
    pub images: bool,
    /// May add reactions here (permission / not archived). Removing my own
    /// is always allowed.
    pub can_react: bool,
    /// Show the "+" button (Discord only shows it on message hover).
    pub show_add: bool,
    /// Custom emoji offered in the picker.
    pub custom: &'a [CustomEmojiGroup],
    pub recent: &'a RecentEmoji,
    /// Tooltip text for (emoji, burst), e.g. from [`reactors_text`].
    /// `None` = not loaded yet.
    pub tooltip: &'a dyn Fn(&EmojiRef, bool) -> Option<String>,
}

impl ReactionCtx<'_> {
    fn picker(&self) -> PickerCtx<'_> {
        PickerCtx {
            pal: self.pal,
            images: self.images,
            custom: self.custom,
            recent: self.recent,
            allow_text: false,
        }
    }
}

/// Discord-style reaction pills under a message, plus the "+" button with
/// the emoji picker. Draws nothing (and takes no space) without reactions
/// unless `show_add`.
pub fn reaction_bar(
    ui: &mut egui::Ui,
    reactions: &[Reaction],
    ctx: &ReactionCtx,
) -> Option<ReactionAction> {
    if reactions.is_empty() && !(ctx.show_add && ctx.can_react) {
        return None;
    }
    let mut action = None;
    let mut want = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, r) in reactions.iter().enumerate() {
            // Super Reactions get their own pill, like Discord.
            if r.burst > 0 {
                let resp = pill(ui, r, true, ctx, ctx.id.with((i, 1)), &mut want);
                if resp.clicked()
                    && let Some(a) = click(r, true, ctx)
                {
                    action = Some(a);
                }
            }
            if r.normal > 0 {
                let resp = pill(ui, r, false, ctx, ctx.id.with((i, 0)), &mut want);
                if resp.clicked()
                    && let Some(a) = click(r, false, ctx)
                {
                    action = Some(a);
                }
            }
        }
        if ctx.show_add && ctx.can_react {
            let resp = add_button(ui, ctx);
            if let Some(emoji) = emoji_picker::popup(&resp, &ctx.picker()).and_then(Picked::emoji) {
                action = Some(ReactionAction::Add {
                    emoji,
                    from_picker: true,
                });
            }
        }
    });
    action.or(want)
}

/// Clicking a pill: remove mine, or add a normal reaction. (Adding Super
/// Reactions isn't supported, so a burst pill adds a normal one.)
fn click(r: &Reaction, burst: bool, ctx: &ReactionCtx) -> Option<ReactionAction> {
    let mine = if burst { r.me_burst } else { r.me };
    if mine {
        Some(ReactionAction::Remove {
            emoji: r.emoji.clone(),
            burst,
        })
    } else if ctx.can_react && !(burst && r.me) {
        Some(ReactionAction::Add {
            emoji: r.emoji.clone(),
            from_picker: false,
        })
    } else {
        None
    }
}

/// One pill: emoji + count. Accent border and tint when it's mine.
fn pill(
    ui: &mut egui::Ui,
    r: &Reaction,
    burst: bool,
    ctx: &ReactionCtx,
    id: egui::Id,
    want: &mut Option<ReactionAction>,
) -> egui::Response {
    let pal = ctx.pal;
    let (count, mine) = if burst {
        (r.burst, r.me_burst)
    } else {
        (r.normal, r.me)
    };
    let font = theme::bold_font(13.0);
    let count_text = count.to_string();
    let galley = ui
        .painter()
        .layout_no_wrap(count_text, font, Color32::PLACEHOLDER);
    let emoji_w = if ctx.images || !r.emoji.is_custom() {
        PILL_EMOJI
    } else {
        // Text mode custom emoji: ":name:" is wider.
        ui.painter()
            .layout_no_wrap(
                r.emoji.text(),
                egui::FontId::proportional(13.0),
                Color32::PLACEHOLDER,
            )
            .size()
            .x
    };
    let width = 8.0 + emoji_w + 6.0 + galley.size().x + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, PILL_H), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    if ui.is_rect_visible(rect) {
        let accent = if burst {
            r.burst_colors.first().copied().unwrap_or(pal.accent)
        } else {
            pal.accent
        };
        let (fill, stroke) = if mine {
            (accent.gamma_multiply(0.18), accent)
        } else if resp.hovered() {
            (pal.sidebar, pal.muted.gamma_multiply(0.6))
        } else if burst {
            (pal.sidebar, accent.gamma_multiply(0.45))
        } else {
            (pal.sidebar, Color32::TRANSPARENT)
        };
        ui.painter().rect(
            rect.shrink(0.5),
            8,
            fill,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let emoji_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 8.0, rect.center().y - PILL_EMOJI / 2.0),
            vec2(emoji_w, PILL_EMOJI),
        );
        if ctx.images {
            egui::Image::from_uri(r.emoji.image_url())
                .show_loading_spinner(false)
                .paint_at(ui, emoji_rect);
        } else if r.emoji.is_custom() {
            ui.painter().text(
                emoji_rect.left_center(),
                egui::Align2::LEFT_CENTER,
                r.emoji.text(),
                egui::FontId::proportional(13.0),
                pal.text,
            );
        } else {
            emoji_picker::draw_emoji_text(ui, &r.emoji, emoji_rect, pal.text);
        }
        let color = if mine { pal.strong } else { pal.muted };
        ui.painter().galley(
            egui::pos2(
                emoji_rect.right() + 6.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            color,
        );
    }
    let clickable = mine || ctx.can_react;
    let resp = if clickable {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    resp.on_hover_ui(|ui| {
        ui.set_max_width(240.0);
        ui.horizontal(|ui| {
            let (img, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            if ctx.images {
                egui::Image::from_uri(r.emoji.image_url()).paint_at(ui, img);
            } else {
                emoji_picker::draw_emoji_text(ui, &r.emoji, img, pal.text);
            }
            let text = match (ctx.tooltip)(&r.emoji, burst) {
                Some(t) => t,
                None => {
                    *want = Some(ReactionAction::WantReactors {
                        emoji: r.emoji.clone(),
                        burst,
                    });
                    format!("{} · loading…", r.emoji.shortcode())
                }
            };
            ui.add(egui::Label::new(egui::RichText::new(text).color(pal.text)).wrap());
        });
    })
}

/// The "+" add-reaction pill. Returns its response (the picker anchors to it).
fn add_button(ui: &mut egui::Ui, ctx: &ReactionCtx) -> egui::Response {
    let pal = ctx.pal;
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, PILL_H), Sense::hover());
    let resp = ui.interact(rect, ctx.id.with("add"), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let hot = resp.hovered() || open;
    ui.painter().rect(
        rect.shrink(0.5),
        8,
        pal.sidebar,
        egui::Stroke::new(
            1.0,
            if hot {
                pal.muted.gamma_multiply(0.6)
            } else {
                Color32::TRANSPARENT
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "☺+",
        egui::FontId::proportional(15.0),
        if hot { pal.text } else { pal.muted },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Add Reaction")
}

/// The picker as an "Add Reaction" popup on any button (e.g. the message
/// hover bar), for messages that have no reactions yet.
pub fn picker_popup(button: &egui::Response, ctx: &ReactionCtx) -> Option<ReactionAction> {
    if !ctx.can_react {
        return None;
    }
    emoji_picker::popup(button, &ctx.picker())
        .and_then(Picked::emoji)
        .map(|emoji| ReactionAction::Add {
            emoji,
            from_picker: true,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "100";

    fn thumbs() -> EmojiRef {
        EmojiRef::unicode("👍")
    }

    fn sample() -> Vec<Reaction> {
        parse(&json!([
            {
                "emoji": { "id": null, "name": "👍" },
                "count": 3,
                "count_details": { "burst": 1, "normal": 2 },
                "me": true, "me_burst": false,
                "burst_colors": ["#ff0000", "#00FF00", "bad"]
            },
            {
                "emoji": { "id": "42", "name": "pepe", "animated": true },
                "count": 1, "me": false
            }
        ]))
    }

    fn add(user: &str, emoji: EmojiRef, burst: bool) -> ReactionEvent {
        ReactionEvent::Add {
            channel_id: "c".into(),
            message_id: "m".into(),
            user_id: user.into(),
            emoji,
            burst,
            burst_colors: Vec::new(),
        }
    }

    fn remove(user: &str, emoji: EmojiRef, burst: bool) -> ReactionEvent {
        ReactionEvent::Remove {
            channel_id: "c".into(),
            message_id: "m".into(),
            user_id: user.into(),
            emoji,
            burst,
        }
    }

    #[test]
    fn parses_reactions() {
        let r = sample();
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].normal, r[0].burst, r[0].count()), (2, 1, 3));
        assert!(r[0].me && !r[0].me_burst);
        assert_eq!(
            r[0].burst_colors,
            vec![Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 255, 0)]
        );
        // No count_details: all normal.
        assert_eq!((r[1].normal, r[1].burst), (1, 0));
        assert!(r[1].emoji.animated);
        assert!(parse(&json!(null)).is_empty());
        // Junk entries skipped.
        assert!(parse(&json!([{ "count": 2 }, { "emoji": {}, "count": 1 }])).is_empty());
    }

    #[test]
    fn parses_events() {
        let d = json!({
            "user_id": "7", "channel_id": "c", "message_id": "m", "guild_id": "g",
            "emoji": { "id": null, "name": "🔥" }, "burst": false, "type": 0
        });
        assert_eq!(
            ReactionEvent::from_dispatch("MESSAGE_REACTION_ADD", &d),
            Some(add("7", EmojiRef::unicode("🔥"), false))
        );
        let d = json!({
            "user_id": "7", "channel_id": "c", "message_id": "m",
            "emoji": { "id": "5", "name": "x" }, "type": 1
        });
        let ev = ReactionEvent::from_dispatch("MESSAGE_REACTION_REMOVE", &d).unwrap();
        assert_eq!(ev, remove("7", EmojiRef::custom("5", "x", false), true));
        assert_eq!((ev.channel_id(), ev.message_id()), ("c", "m"));
        let d = json!({ "channel_id": "c", "message_id": "m" });
        assert!(matches!(
            ReactionEvent::from_dispatch("MESSAGE_REACTION_REMOVE_ALL", &d),
            Some(ReactionEvent::RemoveAll { .. })
        ));
        // REMOVE_EMOJI needs the emoji; ADD needs the user.
        assert!(ReactionEvent::from_dispatch("MESSAGE_REACTION_REMOVE_EMOJI", &d).is_none());
        assert!(ReactionEvent::from_dispatch("MESSAGE_REACTION_ADD", &d).is_none());
        assert!(ReactionEvent::from_dispatch("MESSAGE_CREATE", &d).is_none());
    }

    #[test]
    fn applies_events() {
        let mut r = sample();
        // Someone else adds 👍.
        apply_event(&mut r, &add("7", thumbs(), false), ME);
        assert_eq!(r[0].normal, 3);
        // New emoji goes to the end.
        apply_event(&mut r, &add("7", EmojiRef::unicode("🔥"), false), ME);
        assert_eq!(r[2].emoji, EmojiRef::unicode("🔥"));
        assert!(!r[2].me);
        // My own add when I already reacted: no double count.
        apply_event(&mut r, &add(ME, thumbs(), false), ME);
        assert_eq!(r[0].normal, 3);
        // My Super Reaction.
        apply_event(&mut r, &add(ME, thumbs(), true), ME);
        assert!(r[0].me_burst);
        assert_eq!(r[0].burst, 2);
        // Removing the last 🔥 drops the pill.
        apply_event(&mut r, &remove("7", EmojiRef::unicode("🔥"), false), ME);
        assert_eq!(r.len(), 2);
        // Custom emoji matched by id even if renamed.
        apply_event(
            &mut r,
            &remove("8", EmojiRef::custom("42", "renamed", true), false),
            ME,
        );
        assert_eq!(r.len(), 1);
        // Removing something absent is harmless.
        apply_event(&mut r, &remove("8", EmojiRef::unicode("🎉"), false), ME);
        assert_eq!(r.len(), 1);
        apply_event(
            &mut r,
            &ReactionEvent::RemoveEmoji {
                channel_id: "c".into(),
                message_id: "m".into(),
                emoji: thumbs(),
            },
            ME,
        );
        assert!(r.is_empty());
        let mut r = sample();
        apply_event(
            &mut r,
            &ReactionEvent::RemoveAll {
                channel_id: "c".into(),
                message_id: "m".into(),
            },
            ME,
        );
        assert!(r.is_empty());
    }

    #[test]
    fn optimistic_add_then_echo() {
        let mut r = sample();
        let mut p = PendingReactions::default();
        let fire = EmojiRef::unicode("🔥");
        assert!(p.begin(&mut r, "c", "m", &fire, true, false, 0.0));
        assert_eq!(r[2].normal, 1);
        assert!(r[2].me);
        assert!(p.has_message("m"));
        // The gateway echo is skipped (already applied).
        p.on_event(&mut r, &add(ME, fire.clone(), false), ME);
        assert_eq!(r[2].normal, 1);
        assert!(p.items.is_empty());
        // Others still count.
        p.on_event(&mut r, &add("7", fire, false), ME);
        assert_eq!(r[2].normal, 2);
    }

    #[test]
    fn optimistic_rapid_toggle() {
        let mut r = sample();
        let mut p = PendingReactions::default();
        // Remove my 👍 then add it back before either echo arrives.
        assert!(p.begin(&mut r, "c", "m", &thumbs(), false, false, 0.0));
        assert_eq!(r[0].normal, 1);
        assert!(p.begin(&mut r, "c", "m", &thumbs(), true, false, 0.1));
        assert_eq!(r[0].normal, 2);
        p.on_event(&mut r, &remove(ME, thumbs(), false), ME);
        p.on_event(&mut r, &add(ME, thumbs(), false), ME);
        assert_eq!(r[0].normal, 2);
        assert!(r[0].me);
        assert!(p.items.is_empty());
    }

    #[test]
    fn optimistic_noop_and_failure() {
        let mut r = sample();
        let mut p = PendingReactions::default();
        // Already reacted: nothing to send.
        assert!(!p.begin(&mut r, "c", "m", &thumbs(), true, false, 0.0));
        assert!(p.items.is_empty());
        // Removing a reaction that isn't there.
        assert!(!p.begin(
            &mut r,
            "c",
            "m",
            &EmojiRef::unicode("🎉"),
            false,
            false,
            0.0
        ));
        // A failed add is undone.
        let party = EmojiRef::unicode("🎉");
        assert!(p.begin(&mut r, "c", "m", &party, true, false, 0.0));
        assert_eq!(r.len(), 3);
        p.on_failure(&mut r, "m", &party, true, false);
        assert_eq!(r.len(), 2);
        assert!(p.items.is_empty());
        // A failed remove is undone.
        assert!(p.begin(&mut r, "c", "m", &thumbs(), false, false, 0.0));
        assert!(!r[0].me);
        p.on_failure(&mut r, "m", &thumbs(), false, false);
        assert!(r[0].me);
        assert_eq!(r[0].normal, 2);
        // Failure with nothing pending changes nothing.
        p.on_failure(&mut r, "m", &thumbs(), false, false);
        assert_eq!(r[0].normal, 2);
    }

    #[test]
    fn pending_cleared_by_remove_all_and_prune() {
        let mut r = sample();
        let mut p = PendingReactions::default();
        p.begin(&mut r, "c", "m", &EmojiRef::unicode("🔥"), true, false, 0.0);
        p.on_event(
            &mut r,
            &ReactionEvent::RemoveAll {
                channel_id: "c".into(),
                message_id: "m".into(),
            },
            ME,
        );
        assert!(r.is_empty() && p.items.is_empty());
        let mut r = sample();
        p.begin(&mut r, "c", "m", &EmojiRef::unicode("🔥"), true, false, 0.0);
        p.prune(10.0, PENDING_MAX_AGE);
        assert_eq!(p.items.len(), 1);
        p.prune(31.0, PENDING_MAX_AGE);
        assert!(p.items.is_empty());
    }

    #[test]
    fn tooltip_text() {
        let e = EmojiRef::custom("1", "fire", false);
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            reactors_text(&e, &n(&["Ann"]), 1),
            "Ann reacted with :fire:"
        );
        assert_eq!(
            reactors_text(&e, &n(&["Ann", "Bo"]), 2),
            "Ann and Bo reacted with :fire:"
        );
        assert_eq!(
            reactors_text(&e, &n(&["Ann", "Bo", "Cy"]), 3),
            "Ann, Bo and Cy reacted with :fire:"
        );
        assert_eq!(
            reactors_text(&e, &n(&["Ann", "Bo", "Cy", "Di"]), 4),
            "Ann, Bo, Cy and 1 other reacted with :fire:"
        );
        assert_eq!(
            reactors_text(&e, &n(&["Ann"]), 9),
            "Ann and 8 others reacted with :fire:"
        );
        assert_eq!(reactors_text(&e, &[], 5), "5 reacted with :fire:");
    }
}
