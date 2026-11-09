//! Offline demo mode: `RUSTCORD_DEMO=1 cargo run`.
//!
//! Feeds the UI fake servers, channels and messages without connecting to
//! Discord. Useful for working on the UI/layout without a token, and in cloud
//! sessions that can't reach Discord.

use std::sync::mpsc::Sender;

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::types::{Channel, Guild, Message, ToBackend, ToUi, User};

pub async fn run(tx: Sender<ToUi>, mut rx: UnboundedReceiver<ToBackend>, repaint: impl Fn()) {
    let send = |ev| {
        let _ = tx.send(ev);
        repaint();
    };

    let me = user("1", "You", None);
    send(ToUi::Ready {
        me: User::from_json(&me),
        guilds: vec![Guild::from_json(&json!({
            "id": "100",
            "name": "RustCord Test Server",
            "channels": [
                { "id": "200", "name": "welcome", "type": 0, "position": 0 },
                { "id": "300", "name": "TEXT CHANNELS", "type": 4, "position": 1 },
                { "id": "201", "name": "general", "type": 0, "position": 0, "parent_id": "300" },
                { "id": "202", "name": "memes", "type": 0, "position": 1, "parent_id": "300" },
                { "id": "301", "name": "Dev", "type": 4, "position": 2 },
                { "id": "203", "name": "rust", "type": 0, "position": 0, "parent_id": "301" },
            ]
        }))],
        dms: vec![Channel::from_json(&json!({
            "id": "400", "type": 1, "last_message_id": "9",
            "recipients": [user("2", "Sam", None)]
        }))],
    });
    send(ToUi::Status("Demo mode (offline)".into()));

    let mut next_id = 1000u64;
    while let Some(req) = rx.recv().await {
        match req {
            ToBackend::LoadHistory { channel_id } => send(ToUi::History {
                messages: sample_messages(&channel_id),
                channel_id,
            }),
            ToBackend::Send {
                channel_id,
                content,
                ..
            } => {
                next_id += 1;
                send(ToUi::NewMessage(Message::from_json(&json!({
                    "id": next_id.to_string(),
                    "channel_id": channel_id,
                    "author": me,
                    "content": content,
                    "timestamp": "2026-11-09T12:00:00.000000+00:00",
                }))));
            }
            ToBackend::Edit {
                channel_id,
                message_id,
                content,
            } => send(ToUi::MessageUpdated(crate::types::MessagePatch::from_json(
                &json!({
                    "id": message_id, "channel_id": channel_id,
                    "content": content, "edited_timestamp": "2026-11-09T12:00:00Z",
                }),
            ))),
            ToBackend::Delete {
                channel_id,
                message_id,
            } => send(ToUi::MessageDeleted {
                channel_id,
                message_id,
            }),
            ToBackend::Typing { .. } => {}
        }
    }
}

fn user(id: &str, name: &str, avatar: Option<&str>) -> Value {
    json!({ "id": id, "username": name.to_lowercase(), "global_name": name, "avatar": avatar })
}

fn sample_messages(channel_id: &str) -> Vec<Message> {
    let sam = user("2", "Sam", None);
    let alex = user("3", "Alex", None);
    let me = user("1", "You", None);
    let msg = |id: &str, author: &Value, content: &str, extra: Value| {
        let mut v = json!({
            "id": id, "channel_id": channel_id, "author": author, "content": content,
            "timestamp": format!("2026-11-09T09:{}:00.000000+00:00", 10 + id.len()),
        });
        if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        Message::from_json(&v)
    };
    vec![
        msg("1", &sam, "hey, is the new client working?", json!({})),
        msg("2", &sam, "the layout looks sick", json!({})),
        msg(
            "3",
            &me,
            "yeah! you can drag the panels around now",
            json!({}),
        ),
        msg(
            "4",
            &alex,
            "nice, does it do replies?",
            json!({ "referenced_message": { "author": me, "content": "yeah! you can drag the panels around now" } }),
        ),
        msg(
            "5",
            &me,
            "and edits",
            json!({ "edited_timestamp": "2026-11-09T09:20:00Z" }),
        ),
        msg(
            "6",
            &sam,
            "here's the build log",
            json!({ "attachments": [{ "url": "https://example.com/build.log", "filename": "build.log", "size": 20480, "content_type": "text/plain" }] }),
        ),
    ]
}
