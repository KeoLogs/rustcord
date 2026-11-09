//! Offline demo mode: `RUSTCORD_DEMO=1 cargo run`.
//!
//! Feeds the UI fake servers, channels and messages without connecting to
//! Discord. Useful for working on the UI/layout without a token, and in cloud
//! sessions that can't reach Discord.
//!
//! Read state in the demo:
//! - Test Server: #general unread, #memes muted (greyed), #rust unread with
//!   2 mentions.
//! - Gaming Hangout: #lobby has 1 mention (red badge on the server icon).
//! - Big Public Server: whole server muted.
//! - DMs: Alex has 1 unread message.
//! - ~6 s after start Sam mentions you in #general (desktop notification +
//!   inbox entry); ~25 s after start "another device" reads #lobby
//!   (MESSAGE_ACK).
//!
//! Members & presence in the demo:
//! - Test Server has roles (Admin, Moderators, Bots hoisted; Contributor,
//!   Rustacean coloured) and a grouped member list answered to op 14, with
//!   online / idle / dnd / offline people, custom statuses and a bot.
//!   ~15 s after start Olly comes online (DELETE / INSERT / UPDATE ops).
//! - Other servers have no list, so they show "recently active".
//! - Clicking a name or avatar opens a fake profile; notes are kept in memory.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::members::{ListUpdate, Member};
use crate::notify::GuildSettings;
use crate::presence::Presence;
use crate::profile::Profile;
use crate::read_state::ReadStateEntry;
use crate::types::{Channel, Guild, Message, ToBackend, ToUi, User};

// Snowflake-like ids so "Member since" shows believable dates.
const ME: &str = "460000000000000001";
const SAM: &str = "300000000000000002";
const ALEX: &str = "520000000000000003";
const TEST_GUILD: &str = "100";

/// Demo people in the Test Server.
/// (id, name, role ids, status, activity as (type, text)).
type Person = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
    Option<(u8, &'static str)>,
);

const PEOPLE: &[Person] = &[
    (
        SAM,
        "Sam",
        &["500"],
        "online",
        Some((4, "🦀|shipping RustCord")),
    ),
    (
        "610000000000000004",
        "Maya",
        &["501", "503"],
        "dnd",
        Some((2, "Spotify")),
    ),
    (
        "700000000000000006",
        "RustBot",
        &["502"],
        "online",
        Some((0, "with crabs")),
    ),
    (
        ALEX,
        "Alex",
        &["503", "504"],
        "online",
        Some((0, "Minecraft")),
    ),
    (
        "640000000000000005",
        "Ian",
        &["504"],
        "idle",
        Some((4, "☕|brb, coffee")),
    ),
    ("880000000000000008", "Jess", &[], "online", None),
    (ME, "You", &["503"], "online", None),
    ("390000000000000007", "Olly", &["503"], "offline", None),
    ("900000000000000009", "Priya", &[], "offline", None),
];

fn person(id: &str) -> Option<&'static Person> {
    PEOPLE.iter().find(|p| p.0 == id)
}

fn presence_json(status: &str, activity: Option<(u8, &str)>) -> Value {
    let activities = match activity {
        Some((4, text)) => {
            // "emoji|text"
            let (emoji, state) = text.split_once('|').unwrap_or(("", text));
            json!([{ "type": 4, "name": "Custom Status", "state": state, "emoji": { "name": emoji } }])
        }
        Some((t, text)) => json!([{ "type": t, "name": text }]),
        None => json!([]),
    };
    json!({ "status": status, "activities": activities })
}

fn member_json(id: &str, status_override: Option<&str>) -> Value {
    let (id, name, roles, status, activity) = *person(id).expect("demo person");
    let mut u = user(id, name, None);
    if name == "RustBot" {
        u["bot"] = json!(true);
    }
    let status = status_override.unwrap_or(status);
    json!({
        "user": u, "roles": roles, "joined_at": "2024-03-05T18:30:00.000000+00:00",
        "presence": presence_json(status, activity),
    })
}

/// The Test Server's member list as Discord would send it after op 14.
fn member_list_sync() -> Value {
    let group = |id: &str, n: usize| json!({ "group": { "id": id, "count": n } });
    let mut items = Vec::new();
    let mut groups = Vec::new();
    for (gid, ids) in [
        ("500", vec![SAM]),
        ("501", vec!["610000000000000004"]),
        ("502", vec!["700000000000000006"]),
        (
            "online",
            vec![ALEX, "640000000000000005", "880000000000000008", ME],
        ),
        ("offline", vec!["390000000000000007", "900000000000000009"]),
    ] {
        items.push(group(gid, ids.len()));
        groups.push(json!({ "id": gid, "count": ids.len() }));
        items.extend(
            ids.iter()
                .map(|id| json!({ "member": member_json(id, None) })),
        );
    }
    json!({ "guild_id": TEST_GUILD, "id": "everyone", "member_count": 9, "online_count": 7,
            "groups": groups,
            "ops": [{ "op": "SYNC", "range": [0, 99], "items": items }] })
}

/// Olly comes online: moves from "Offline" into "Online" (alphabetical).
fn member_list_olly_online() -> Value {
    let olly = "390000000000000007";
    json!({ "guild_id": TEST_GUILD, "id": "everyone", "member_count": 9, "online_count": 8,
        "groups": [
            { "id": "500", "count": 1 }, { "id": "501", "count": 1 }, { "id": "502", "count": 1 },
            { "id": "online", "count": 5 }, { "id": "offline", "count": 1 },
        ],
        "ops": [
            { "op": "DELETE", "index": 12 },
            { "op": "INSERT", "index": 10, "item": { "member": member_json(olly, Some("online")) } },
            { "op": "UPDATE", "index": 6, "item": { "group": { "id": "online", "count": 5 } } },
            { "op": "UPDATE", "index": 12, "item": { "group": { "id": "offline", "count": 1 } } },
        ] })
}

fn fake_profile(user_id: &str, guild_id: Option<String>) -> Profile {
    let (bio, pronouns, theme) = match user_id {
        SAM => (
            "Maintainer of the RustCord test server.\nProbably rewriting something in Rust right now 🦀",
            "they/them",
            json!([0x5865f2, 0xeb459e]),
        ),
        "700000000000000006" => ("Beep boop. I post build results.", "", json!(null)),
        _ => ("", "", json!(null)),
    };
    let mut v = json!({
        "user": { "id": user_id, "bio": bio },
        "user_profile": { "bio": bio, "pronouns": pronouns, "theme_colors": theme },
    });
    if let (Some(g), Some(_)) = (&guild_id, person(user_id))
        && g == TEST_GUILD
    {
        v["guild_member"] = member_json(user_id, None);
    }
    Profile::from_json(&v, guild_id)
}

/// Newest message id in each demo channel (sample history ends here).
fn last_id(channel_id: &str) -> u64 {
    match channel_id {
        "200" => 10,
        "201" => 30,
        "202" => 40,
        "203" => 60,
        "210" => 80,
        "211" => 85,
        "220" => 90,
        "400" => 9,
        "401" => 95,
        _ => 20,
    }
}

pub async fn run(tx: Sender<ToUi>, mut rx: UnboundedReceiver<ToBackend>, repaint: impl Fn()) {
    let send = |ev| {
        let _ = tx.send(ev);
        repaint();
    };

    let me = user(ME, "You", None);
    let ch = |id: &str, name: &str, pos: i64, parent: Option<&str>| {
        json!({ "id": id, "name": name, "type": 0, "position": pos, "parent_id": parent,
                "last_message_id": last_id(id).to_string() })
    };
    send(ToUi::Status("Demo mode (offline)".into()));
    send(ToUi::Ready {
        me: User::from_json(&me),
        guilds: vec![
            Guild::from_json(&json!({
                "id": TEST_GUILD,
                "name": "RustCord Test Server",
                "roles": [
                    { "id": TEST_GUILD, "name": "@everyone", "color": 0, "position": 0 },
                    { "id": "500", "name": "Admin", "color": 0xf1c40f, "position": 5, "hoist": true },
                    { "id": "501", "name": "Moderators", "color": 0x3498db, "position": 4, "hoist": true },
                    { "id": "502", "name": "Bots", "color": 0x9b59b6, "position": 3, "hoist": true },
                    { "id": "503", "name": "Contributor", "color": 0x2ecc71, "position": 2 },
                    { "id": "504", "name": "Rustacean", "color": 0xe67e22, "position": 1 },
                ],
                "channels": [
                    ch("200", "welcome", 0, None),
                    { "id": "300", "name": "TEXT CHANNELS", "type": 4, "position": 1 },
                    ch("201", "general", 0, Some("300")),
                    ch("202", "memes", 1, Some("300")),
                    { "id": "301", "name": "Dev", "type": 4, "position": 2 },
                    ch("203", "rust", 0, Some("301")),
                ]
            })),
            Guild::from_json(&json!({
                "id": "101",
                "name": "Gaming Hangout",
                "channels": [ch("210", "lobby", 0, None), ch("211", "clips", 1, None)]
            })),
            Guild::from_json(&json!({
                "id": "102",
                "name": "Big Public Server",
                "channels": [ch("220", "announcements", 0, None)]
            })),
        ],
        dms: vec![
            Channel::from_json(&json!({
                "id": "400", "type": 1, "last_message_id": "9",
                "recipients": [user(SAM, "Sam", None)]
            })),
            Channel::from_json(&json!({
                "id": "401", "type": 1, "last_message_id": "95",
                "recipients": [user(ALEX, "Alex", None)]
            })),
        ],
        read_state: ReadStateEntry::parse_ready(&json!({ "entries": [
            { "id": "200", "last_message_id": "10" },
            { "id": "201", "last_message_id": "26" },
            { "id": "202", "last_message_id": "33" },
            { "id": "203", "last_message_id": "57", "mention_count": 2 },
            { "id": "210", "last_message_id": "78", "mention_count": 1 },
            { "id": "211", "last_message_id": "85" },
            { "id": "220", "last_message_id": "80" },
            { "id": "400", "last_message_id": "9" },
            { "id": "401", "last_message_id": "94", "mention_count": 1 },
        ], "version": 1 })),
        guild_settings: GuildSettings::parse_ready(&json!({ "entries": [
            { "guild_id": "100", "message_notifications": 1,
              "channel_overrides": [{ "channel_id": "202", "muted": true, "mute_config": null }] },
            { "guild_id": "102", "muted": true,
              "mute_config": { "end_time": null, "selected_time_window": -1 } },
        ], "version": 1 })),
    });

    // READY_SUPPLEMENTAL: presences for DM friends and the Test Server.
    send(ToUi::Presences(
        PEOPLE
            .iter()
            .map(|(id, _, _, status, act)| {
                (
                    id.to_string(),
                    Presence::from_json(&presence_json(status, *act)),
                )
            })
            .collect(),
    ));
    let mut notes: HashMap<String, String> = HashMap::new();
    notes.insert(SAM.into(), "Owes me a code review".into());

    let mut next_id = 1000u64;
    // Messages created during the demo, so reloading history keeps them.
    let mut created: HashMap<String, Vec<Value>> = HashMap::new();
    let emit = |v: Value, created: &mut HashMap<String, Vec<Value>>| {
        let cid = v["channel_id"].as_str().unwrap_or_default().to_string();
        created.entry(cid).or_default().push(v.clone());
        send(ToUi::NewMessage(Message::from_json(&v)));
    };
    let start = tokio::time::Instant::now();
    // Scripted events: (seconds after start, step).
    let script = [6u64, 15, 25];
    let mut step = 0;

    loop {
        let wait = script
            .get(step)
            .map(|s| start + Duration::from_secs(*s))
            .unwrap_or(start + Duration::from_secs(60 * 60 * 24 * 365));
        let req = tokio::select! {
            req = rx.recv() => match req {
                Some(r) => r,
                None => return,
            },
            _ = tokio::time::sleep_until(wait) => {
                match step {
                    0 => {
                        next_id += 1;
                        emit(json!({
                            "id": next_id.to_string(), "channel_id": "201", "guild_id": "100",
                            "author": user(SAM, "Sam", None),
                            "content": "@You can you review the notifications PR when you get a sec?",
                            "mentions": [me],
                            "timestamp": "2026-11-09T12:05:00.000000+00:00",
                        }), &mut created);
                    }
                    1 => {
                        let olly = "390000000000000007";
                        send(ToUi::Presences(vec![(
                            olly.to_string(),
                            Presence::from_json(&presence_json("online", None)),
                        )]));
                        if let Some(u) = ListUpdate::from_json(&member_list_olly_online()) {
                            send(ToUi::MemberList(u));
                        }
                    }
                    2 => send(ToUi::MessageAck(ReadStateEntry {
                        channel_id: "210".into(),
                        last_acked: last_id("210"),
                        mention_count: 0,
                    })),
                    _ => {}
                }
                step += 1;
                continue;
            }
        };
        match req {
            ToBackend::LoadHistory { channel_id } => {
                let mut messages = sample_messages(&channel_id);
                messages.extend(
                    created
                        .get(&channel_id)
                        .into_iter()
                        .flatten()
                        .map(Message::from_json),
                );
                send(ToUi::History {
                    messages,
                    channel_id,
                })
            }
            ToBackend::Send {
                channel_id,
                content,
                files,
                ..
            } => {
                next_id += 1;
                let attachments: Vec<Value> = files
                    .iter()
                    .map(|p| {
                        json!({
                            "url": "https://example.com/upload",
                            "filename": p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                            "size": std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                        })
                    })
                    .collect();
                emit(
                    json!({
                        "id": next_id.to_string(),
                        "channel_id": channel_id,
                        "author": me,
                        "content": content,
                        "attachments": attachments,
                        "timestamp": "2026-11-09T12:00:00.000000+00:00",
                    }),
                    &mut created,
                );
            }
            // No real file dialog in demo mode: pretend two files were picked.
            ToBackend::PickFiles => send(ToUi::FilesPicked(vec![
                "holiday-photo.png".into(),
                "notes.txt".into(),
            ])),
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
            // Nothing to tell Discord offline; log so the flow can be checked.
            ToBackend::Ack {
                channel_id,
                message_id,
            } => eprintln!("demo: ack #{channel_id} up to {message_id}"),
            ToBackend::MarkUnread {
                channel_id,
                message_id,
                mention_count,
            } => eprintln!(
                "demo: mark unread #{channel_id} after {message_id} ({mention_count} mentions)"
            ),
            ToBackend::AckBulk(acks) => eprintln!("demo: ack-bulk {acks:?}"),
            ToBackend::Notify { title, body } => {
                eprintln!("demo: notification \"{title}\": {body}");
                tokio::task::spawn_blocking(move || crate::notify::show_desktop(&title, &body));
            }
            ToBackend::SubscribeMembers {
                guild_id, ranges, ..
            } => {
                eprintln!("demo: op 14 {guild_id} {ranges:?}");
                if guild_id == TEST_GUILD
                    && let Some(u) = ListUpdate::from_json(&member_list_sync())
                {
                    send(ToUi::MemberList(u));
                }
            }
            ToBackend::RequestMembers { guild_id, user_ids } => {
                eprintln!("demo: op 8 {guild_id} {} ids", user_ids.len());
                if guild_id == TEST_GUILD {
                    let members = user_ids
                        .iter()
                        .filter(|id| person(id).is_some())
                        .map(|id| Member::from_json(&member_json(id, None)))
                        .collect();
                    send(ToUi::Members {
                        guild_id,
                        members,
                        full_list: false,
                    });
                }
            }
            ToBackend::RequestMemberList { .. } => {}
            ToBackend::LoadProfile { user_id, guild_id } => {
                send(ToUi::Profile(Box::new(fake_profile(&user_id, guild_id))))
            }
            ToBackend::LoadNote { user_id } => {
                let note = notes.get(&user_id).cloned().unwrap_or_default();
                send(ToUi::Note { user_id, note });
            }
            ToBackend::SetNote { user_id, note } => {
                eprintln!("demo: note for {user_id}: {note:?}");
                notes.insert(user_id.clone(), note.clone());
                send(ToUi::Note { user_id, note });
            }
        }
    }
}

fn user(id: &str, name: &str, avatar: Option<&str>) -> Value {
    json!({ "id": id, "username": name.to_lowercase(), "global_name": name, "avatar": avatar })
}

fn sample_messages(channel_id: &str) -> Vec<Message> {
    let sam = user(SAM, "Sam", None);
    let alex = user(ALEX, "Alex", None);
    let me = user(ME, "You", None);
    // Ids count up to the channel's last message so read state lines up.
    let last = last_id(channel_id);
    let mut n = 0u64;
    let mut msg = |author: &Value, content: &str, extra: Value| {
        let id = last - 7 + n;
        n += 1;
        let mut v = json!({
            "id": id.to_string(), "channel_id": channel_id, "author": author, "content": content,
            "timestamp": format!("2026-11-09T09:{:02}:00.000000+00:00", 10 + n),
        });
        if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        Message::from_json(&v)
    };
    vec![
        msg(&sam, "hey, is the new client working?", json!({})),
        msg(
            &alex,
            "Unicode: Përshëndetje! Здравствуй 你好 こんにちは 안녕하세요 مرحبا नमस्ते Γειά 😀🎉👍❤️ ✓ ★ → ♫ <:pepe:123>",
            json!({}),
        ),
        msg(&me, "yeah! you can drag the panels around now", json!({})),
        msg(
            &alex,
            "nice, does it do replies?",
            json!({ "referenced_message": { "author": me, "content": "yeah! you can drag the panels around now" } }),
        ),
        msg(
            &me,
            "and edits",
            json!({ "edited_timestamp": "2026-11-09T09:20:00Z" }),
        ),
        msg(&sam, "the layout looks sick 🔥", json!({})),
        msg(
            &alex,
            "@You did you see the unread badges?",
            json!({ "mentions": [me] }),
        ),
        msg(
            &sam,
            "here's the build log",
            json!({ "attachments": [{ "url": "https://example.com/build.log", "filename": "build.log", "size": 20480, "content_type": "text/plain" }] }),
        ),
    ]
}
