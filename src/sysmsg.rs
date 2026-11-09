//! Messages that aren't plain text: Discord's system messages (joins, pins,
//! calls, boosts, threads…), stickers and forwarded messages. Pure parsing
//! and wording, with unit tests; `app.rs` draws them.

use serde_json::Value;

use crate::types::Attachment;

/// A sticker from `sticker_items`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sticker {
    pub id: String,
    pub name: String,
    /// 1 PNG, 2 APNG, 3 Lottie, 4 GIF.
    pub format: u8,
}

impl Sticker {
    /// Picture URL from Discord's media proxy (`size` px), or None for Lottie
    /// stickers (vector animations egui can't draw).
    pub fn image_url(&self, size: u32) -> Option<String> {
        match self.format {
            3 => None,
            4 => Some(format!(
                "https://media.discordapp.net/stickers/{}.gif?size={size}",
                self.id
            )),
            // PNG / APNG (APNG shows its first frame).
            _ => Some(format!(
                "https://media.discordapp.net/stickers/{}.png?size={size}",
                self.id
            )),
        }
    }
}

pub fn parse_stickers(v: &Value) -> Vec<Sticker> {
    v.get("sticker_items")
        .or_else(|| v.get("stickers"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    Some(Sticker {
                        id: s.get("id")?.as_str()?.to_string(),
                        name: s
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Sticker")
                            .to_string(),
                        format: s.get("format_type").and_then(Value::as_u64).unwrap_or(1) as u8,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The message inside a forward (`message_snapshots[0].message`).
#[derive(Debug, Clone)]
pub struct Forwarded {
    pub content: String,
    pub attachments: Vec<Attachment>,
    pub stickers: Vec<Sticker>,
    /// Number of embeds in the original (not drawn, just mentioned).
    pub embeds: usize,
}

pub fn parse_forwarded(v: &Value) -> Option<Forwarded> {
    let snap = v.get("message_snapshots")?.as_array()?.first()?;
    let m = snap.get("message")?;
    Some(Forwarded {
        content: m
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        attachments: m
            .get("attachments")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(Attachment::from_json).collect())
            .unwrap_or_default(),
        stickers: parse_stickers(m),
        embeds: m
            .get("embeds")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    })
}

/// Kind of icon / colour for a system message line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysKind {
    /// Green arrow: joins, people added, calls.
    Join,
    /// Red arrow: people removed / left, missed calls.
    Leave,
    Pin,
    Call,
    Boost,
    Thread,
    /// Pencil: name / icon changes.
    Edit,
    /// Anything else (muted info icon).
    Info,
}

/// One part of a system message line: plain text, or a bold name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Name(String),
}

/// What to show for a system message: icon kind and the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemLine {
    pub kind: SysKind,
    pub parts: Vec<Part>,
}

/// Message types drawn as normal messages (default, reply, slash commands,
/// thread starter, context menu command).
pub fn is_normal(kind: u8) -> bool {
    matches!(kind, 0 | 19 | 20 | 21 | 23)
}

/// Discord's random welcome lines (picked from the message id like
/// Discord does, so everyone sees the same one).
const JOIN: [(&str, &str); 13] = [
    ("", " joined the party."),
    ("", " is here."),
    ("Welcome, ", ". We hope you brought pizza."),
    ("A wild ", " appeared."),
    ("", " just landed."),
    ("", " just slid into the server."),
    ("", " just showed up!"),
    ("Welcome ", ". Say hi!"),
    ("", " hopped into the server."),
    ("Everyone welcome ", "!"),
    ("Glad you're here, ", "."),
    ("Good to see you, ", "."),
    ("Yay you made it, ", "!"),
];

/// The line for a system message, or None if `kind` is a normal message.
/// `author` is the (display) name of the author, `target` the first
/// mentioned user's name (people added / removed), `content` the message
/// content (new channel name, thread name, boost count…), `server` the
/// server's name and `id` the message id (picks the welcome line).
pub fn system_line(
    kind: u8,
    author: &str,
    target: Option<&str>,
    content: &str,
    server: &str,
    id: &str,
) -> Option<SystemLine> {
    use Part::{Name, Text};
    let t = |s: &str| Text(s.to_string());
    let n = |s: &str| Name(s.to_string());
    let line = |kind, parts| Some(SystemLine { kind, parts });
    let target = target.unwrap_or("someone");
    match kind {
        k if is_normal(k) => None,
        1 => line(
            SysKind::Join,
            vec![n(author), t(" added "), n(target), t(" to the group.")],
        ),
        2 if target == author || target == "someone" => {
            line(SysKind::Leave, vec![n(author), t(" left the group.")])
        }
        2 => line(
            SysKind::Leave,
            vec![n(author), t(" removed "), n(target), t(" from the group.")],
        ),
        3 => line(SysKind::Call, vec![n(author), t(" started a call.")]),
        4 => line(
            SysKind::Edit,
            vec![n(author), t(" changed the channel name: "), n(content)],
        ),
        5 => line(
            SysKind::Edit,
            vec![n(author), t(" changed the channel icon.")],
        ),
        6 => line(
            SysKind::Pin,
            vec![n(author), t(" pinned a message to this channel.")],
        ),
        7 => {
            let i = id
                .parse::<u64>()
                .map(|v| ((v >> 22) % JOIN.len() as u64) as usize)
                .unwrap_or(0);
            let (before, after) = JOIN[i];
            line(SysKind::Join, vec![t(before), n(author), t(after)])
        }
        8 => {
            let times = content.parse::<u32>().unwrap_or(1);
            let text = if times > 1 {
                format!(" just boosted the server {times} times!")
            } else {
                " just boosted the server!".to_string()
            };
            line(SysKind::Boost, vec![n(author), Text(text)])
        }
        9..=11 => line(
            SysKind::Boost,
            vec![
                n(author),
                t(" just boosted the server! "),
                n(server),
                Text(format!(" has achieved Level {}!", kind - 8)),
            ],
        ),
        12 => line(
            SysKind::Info,
            vec![
                n(author),
                t(" has added "),
                n(content),
                t(" to this channel. Its most important updates will show up here."),
            ],
        ),
        14 | 15 => line(
            SysKind::Info,
            vec![t("This server is no longer eligible for Server Discovery.")],
        ),
        18 => line(
            SysKind::Thread,
            vec![n(author), t(" started a thread: "), n(content)],
        ),
        24 => line(
            SysKind::Info,
            vec![t("AutoMod blocked a message in this channel.")],
        ),
        25 => line(
            SysKind::Boost,
            vec![n(author), t(" joined as a subscriber.")],
        ),
        27 => line(SysKind::Call, vec![n(author), t(" started "), n(content)]),
        28 => line(SysKind::Leave, vec![n(author), t(" ended "), n(content)]),
        29 => line(SysKind::Info, vec![n(author), t(" is now a speaker.")]),
        31 => line(
            SysKind::Edit,
            vec![n(author), t(" changed the Stage topic: "), n(content)],
        ),
        36 | 37 => line(
            SysKind::Info,
            vec![t("Server raid protection settings changed.")],
        ),
        44 => line(SysKind::Boost, vec![n(author), t(" made a purchase.")]),
        46 => line(SysKind::Info, vec![t("A poll has closed.")]),
        _ => line(
            SysKind::Info,
            vec![t("A system message RustCord can't show yet.")],
        ),
    }
}

/// Plain one-line text of a system line (notifications, previews).
#[cfg_attr(not(test), allow(dead_code))]
pub fn plain(line: &SystemLine) -> String {
    line.parts
        .iter()
        .map(|p| match p {
            Part::Text(s) | Part::Name(s) => s.as_str(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stickers_parse_and_urls() {
        let v = json!({ "sticker_items": [
            { "id": "1", "name": "Wave", "format_type": 1 },
            { "id": "2", "name": "Dance", "format_type": 3 },
            { "id": "3", "name": "Spin", "format_type": 4 },
            { "name": "no id" },
        ]});
        let s = parse_stickers(&v);
        assert_eq!(s.len(), 3);
        assert_eq!(
            s[0].image_url(160).as_deref(),
            Some("https://media.discordapp.net/stickers/1.png?size=160")
        );
        assert_eq!(s[1].image_url(160), None);
        assert!(s[2].image_url(160).unwrap().contains("3.gif"));
    }

    #[test]
    fn forwarded_parse() {
        let v = json!({ "content": "", "message_snapshots": [{ "message": {
            "content": "look at this", "attachments": [{ "filename": "a.png", "url": "u" }],
            "embeds": [{}, {}]
        }}]});
        let f = parse_forwarded(&v).unwrap();
        assert_eq!(f.content, "look at this");
        assert_eq!(f.attachments.len(), 1);
        assert_eq!(f.embeds, 2);
        assert!(parse_forwarded(&json!({ "content": "x" })).is_none());
    }

    #[test]
    fn system_lines() {
        assert!(system_line(0, "a", None, "", "S", "1").is_none());
        assert!(system_line(19, "a", None, "", "S", "1").is_none());
        let pin = system_line(6, "Sam", None, "", "S", "1").unwrap();
        assert_eq!(pin.kind, SysKind::Pin);
        assert_eq!(plain(&pin), "Sam pinned a message to this channel.");
        let join = system_line(7, "Leo", None, "", "S", "0").unwrap();
        assert_eq!(plain(&join), "Leo joined the party.");
        let boost = system_line(8, "Leo", None, "3", "S", "1").unwrap();
        assert_eq!(plain(&boost), "Leo just boosted the server 3 times!");
        let tier = system_line(10, "Leo", None, "", "Cafe", "1").unwrap();
        assert_eq!(
            plain(&tier),
            "Leo just boosted the server! Cafe has achieved Level 2!"
        );
        let add = system_line(1, "A", Some("B"), "", "", "1").unwrap();
        assert_eq!(plain(&add), "A added B to the group.");
        let left = system_line(2, "A", None, "", "", "1").unwrap();
        assert_eq!(plain(&left), "A left the group.");
        let thread = system_line(18, "A", None, "ideas", "", "1").unwrap();
        assert_eq!(plain(&thread), "A started a thread: ideas");
        // Unknown types still say something.
        assert!(system_line(250, "A", None, "", "", "1").is_some());
    }
}
