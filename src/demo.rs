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
//!
//! Message richness, account settings (Batches C and D) in the demo:
//! - #welcome has markdown (headings, quotes, lists, code, spoilers,
//!   mentions, timestamps), a CI bot embed with fields, a YouTube-style
//!   link preview, links (one untrusted → link warning), an image, two
//!   messages from a blocked user and two pinned messages (📌).
//! - You have Manage Messages in the Test Server (Contributor role), so
//!   "Pin message" shows there; in Gaming Hangout it doesn't.
//! - Synced settings: a custom status, Gaming Hangout + Big Public Server in
//!   a "Games" folder, DM safety settings. Patches are applied and echoed.
//! - The Test Server has a custom :pepe: emoji for `:` autocomplete.
//! - #showcase has a Unicode torture test (symbols, fancy text, zalgo, RTL,
//!   CJK, kaomoji, flags, ZWJ emoji), stickers, system messages (joins,
//!   pin, thread, boost, call), a forwarded message, a poll and messages
//!   with nothing RustCord can draw (shown as "unsupported", never blank).
//! - The GIF button shows fake results (offline, so the pictures fail).

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
use crate::settings_proto::{self as sp, ExplicitFilter, RailItem, UserSettings};
use crate::types::{Channel, Guild, Message, ToBackend, ToUi, User};

// Snowflake-like ids so "Member since" shows believable dates.
const ME: &str = "460000000000000001";
const SAM: &str = "300000000000000002";
const ALEX: &str = "520000000000000003";
const TEST_GUILD: &str = "100";
/// Someone you've blocked (their messages are hidden behind "Show").
const TROLL: &str = "950000000000000010";

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
        "204" => 70,
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
    // Me: Contributor (Manage Messages) in the Test Server.
    let mut test_guild = Guild::from_json(&json!({
        "id": TEST_GUILD,
        "name": "RustCord Test Server",
        "owner_id": SAM,
        "emojis": [{ "id": "123", "name": "pepe" }],
        "roles": [
            { "id": TEST_GUILD, "name": "@everyone", "color": 0, "position": 0, "permissions": "104324673" },
            { "id": "500", "name": "Admin", "color": 0xf1c40f, "position": 5, "hoist": true, "permissions": "8" },
            { "id": "501", "name": "Moderators", "color": 0x3498db, "position": 4, "hoist": true, "permissions": "8192" },
            { "id": "502", "name": "Bots", "color": 0x9b59b6, "position": 3, "hoist": true },
            // Manage Messages: you can pin here.
            { "id": "503", "name": "Contributor", "color": 0x2ecc71, "position": 2, "permissions": "8192" },
            { "id": "504", "name": "Rustacean", "color": 0xe67e22, "position": 1 },
        ],
        "channels": [
            ch("200", "welcome", 0, None),
            { "id": "300", "name": "TEXT CHANNELS", "type": 4, "position": 1 },
            { "id": "201", "name": "general", "type": 0, "position": 0, "parent_id": "300",
              "last_message_id": last_id("201").to_string(),
              "topic": "Chat about anything. Be nice, no spoilers without tags." },
            ch("202", "memes", 1, Some("300")),
            ch("204", "showcase", 2, Some("300")),
            { "id": "301", "name": "Dev", "type": 4, "position": 2 },
            { "id": "203", "name": "rust", "type": 0, "position": 0, "parent_id": "301",
              "last_message_id": last_id("203").to_string(),
              "topic": "RustCord development · egui 0.37 · PRs welcome" },
            { "id": "302", "name": "Voice Channels", "type": 4, "position": 3 },
            { "id": "230", "name": "General", "type": 2, "position": 0, "parent_id": "302" },
            { "id": "231", "name": "Gaming", "type": 2, "position": 1, "parent_id": "302" },
            { "id": "232", "name": "Town Hall", "type": 13, "position": 2, "parent_id": "302" },
        ]
    }));
    test_guild.my_roles = vec!["503".into()];
    send(ToUi::Ready {
        me: User::from_json(&me),
        guilds: vec![
            test_guild,
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
        blocked: vec![TROLL.into()],
        user_settings: None,
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
    let mut settings = demo_settings();
    // Pinned state per (channel, message), seeded from the sample history.
    let mut pinned: HashMap<(String, String), bool> = HashMap::new();
    let mut notes: HashMap<String, String> = HashMap::new();
    notes.insert(SAM.into(), "Owes me a code review".into());

    let mut next_id = 1000u64;
    // Messages created during the demo, so reloading history keeps them.
    let mut created: HashMap<String, Vec<Value>> = HashMap::new();
    let emit = |v: Value, created: &mut HashMap<String, Vec<Value>>| {
        let cid = v["channel_id"].as_str().unwrap_or_default().to_string();
        created.entry(cid).or_default().push(v.clone());
        send(ToUi::NewMessage(Box::new(Message::from_json(&v))));
    };
    let start = tokio::time::Instant::now();
    // Scripted events: (seconds after start, step).
    let script = [4u64, 6, 15, 25];
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
                    // Two people start typing in #general.
                    0 => {
                        for (id, name) in [(ALEX, "Alex"), ("610000000000000004", "Maya")] {
                            send(ToUi::Typing {
                                channel_id: "201".into(),
                                user_id: id.into(),
                                name: Some(name.into()),
                            });
                        }
                    }
                    1 => {
                        next_id += 1;
                        emit(json!({
                            "id": next_id.to_string(), "channel_id": "201", "guild_id": "100",
                            "author": user(SAM, "Sam", None),
                            "content": "@You can you review the notifications PR when you get a sec?",
                            "mentions": [me],
                            "timestamp": iso_minutes_ago(0),
                        }), &mut created);
                    }
                    2 => {
                        let olly = "390000000000000007";
                        send(ToUi::Presences(vec![(
                            olly.to_string(),
                            Presence::from_json(&presence_json("online", None)),
                        )]));
                        if let Some(u) = ListUpdate::from_json(&member_list_olly_online()) {
                            send(ToUi::MemberList(u));
                        }
                    }
                    3 => send(ToUi::MessageAck(ReadStateEntry {
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
                let messages = history(&channel_id, &created, &pinned);
                send(ToUi::History {
                    messages,
                    channel_id,
                })
            }
            ToBackend::LoadAround {
                channel_id,
                message_id,
            } => {
                // The demo history is short, so "around" is all of it.
                let messages = history(&channel_id, &created, &pinned);
                send(ToUi::HistoryAround {
                    channel_id,
                    message_id,
                    messages,
                })
            }
            ToBackend::LoadPins { channel_id } => {
                let pins = sample_values(&channel_id)
                    .into_iter()
                    .filter(|v| is_pinned(&pinned, &channel_id, v))
                    .rev()
                    .collect();
                send(ToUi::Pins { channel_id, pins });
            }
            ToBackend::SetPinned {
                channel_id,
                message_id,
                pinned: pin,
            } => {
                eprintln!("demo: pin={pin} #{channel_id} {message_id}");
                pinned.insert((channel_id.clone(), message_id.clone()), pin);
                send(ToUi::PinChanged {
                    channel_id: channel_id.clone(),
                    message_id,
                    pinned: pin,
                });
                // Like CHANNEL_PINS_UPDATE.
                send(ToUi::PinsUpdated(channel_id));
            }
            ToBackend::LoadUserSettings => send(ToUi::UserSettings(Box::new(settings.clone()))),
            ToBackend::PatchUserSettings { settings_b64 } => {
                eprintln!("demo: PATCH settings-proto {settings_b64}");
                if settings.apply_patch(&settings_b64).is_ok() {
                    send(ToUi::UserSettings(Box::new(settings.clone())));
                }
            }
            ToBackend::SetPresence { status, custom } => {
                eprintln!("demo: op 3 status={status} custom={custom:?}");
                let activity = custom.map(|c| {
                    json!([{ "type": 4, "name": "Custom Status", "state": c.text,
                             "emoji": { "name": c.emoji_name } }])
                });
                send(ToUi::Presences(vec![(
                    ME.to_string(),
                    Presence::from_json(&json!({
                        "status": if status == "invisible" { "offline" } else { &status },
                        "activities": activity.unwrap_or_else(|| json!([])),
                    })),
                )]));
            }
            ToBackend::LeaveGuild { guild_id } => {
                eprintln!("demo: leave server {guild_id}");
                send(ToUi::GuildRemoved(guild_id));
            }
            ToBackend::SaveFile { url, filename } => {
                eprintln!("demo: save {url} as {filename}");
                send(ToUi::Notice(format!("Demo mode: would save {filename}")));
            }
            ToBackend::CloseDm { channel_id } => eprintln!("demo: close DM {channel_id}"),
            ToBackend::LoadGifs { query } => {
                eprintln!("demo: GIFs {query:?}");
                tokio::time::sleep(Duration::from_millis(300)).await;
                send(ToUi::Gifs {
                    results: crate::gif_picker::demo_results(query.as_deref()),
                    query,
                });
            }
            ToBackend::Send {
                channel_id,
                content,
                files,
                spoilers,
                ..
            } => {
                next_id += 1;
                let attachments: Vec<Value> = files
                    .iter()
                    .zip(spoilers.iter().chain(std::iter::repeat(&false)))
                    .map(|(p, spoiler)| {
                        let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                        json!({
                            "url": "https://example.com/upload",
                            "filename": if *spoiler { format!("SPOILER_{name}") } else { name.to_string() },
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
                        "timestamp": iso_minutes_ago(0),
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
                    "content": content, "edited_timestamp": iso_minutes_ago(0),
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

/// An ISO-8601 timestamp `minutes` before now (Discord's format).
fn iso_minutes_ago(minutes: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339()
}

fn user(id: &str, name: &str, avatar: Option<&str>) -> Value {
    json!({ "id": id, "username": name.to_lowercase(), "global_name": name, "avatar": avatar })
}

/// Synced settings for the demo account: a custom status, a "Games"
/// folder and DM safety settings.
fn demo_settings() -> UserSettings {
    let mut s = UserSettings::default();
    let custom = sp::CustomStatus {
        text: "porting Discord Messenger".into(),
        emoji_name: Some("🦀".into()),
        ..Default::default()
    };
    // Each patch carries a whole top-level section, so build each one from
    // the state after the previous one.
    let steps: [&dyn Fn(&UserSettings) -> String; 5] = [
        &|s| s.patch_status(sp::Status::Online),
        &|s| s.patch_custom_status(Some(&custom)),
        &|s| {
            s.patch_guild_folders(&[
                RailItem::Guild(TEST_GUILD.into()),
                RailItem::Folder {
                    id: 4242,
                    name: Some("Games".into()),
                    color: Some(0xeb459e),
                    guild_ids: vec!["101".into(), "102".into()],
                },
            ])
        },
        &|s| s.patch_explicit_filter(ExplicitFilter::NonFriends),
        &|s| s.patch_compact(false),
    ];
    for step in steps {
        let p = step(&s);
        let _ = s.apply_patch(&p);
    }
    s
}

fn is_pinned(pinned: &HashMap<(String, String), bool>, channel_id: &str, v: &Value) -> bool {
    let id = v["id"].as_str().unwrap_or_default().to_string();
    pinned
        .get(&(channel_id.to_string(), id))
        .copied()
        .unwrap_or_else(|| v["pinned"].as_bool().unwrap_or(false))
}

/// Sample history plus messages sent during the demo, with pin changes.
fn history(
    channel_id: &str,
    created: &HashMap<String, Vec<Value>>,
    pinned: &HashMap<(String, String), bool>,
) -> Vec<Message> {
    sample_values(channel_id)
        .into_iter()
        .chain(created.get(channel_id).into_iter().flatten().cloned())
        .map(|mut v| {
            v["pinned"] = json!(is_pinned(pinned, channel_id, &v));
            Message::from_json(&v)
        })
        .collect()
}

/// Minutes-ago for message `n` of 8+: two days ago, yesterday, then today,
/// so the date lines show.
fn minutes_ago(n: u64) -> i64 {
    match n {
        1 | 2 => 2 * 24 * 60 + 40 - n as i64,
        3 | 4 => 24 * 60 + 30 - n as i64,
        _ => 60 - 3 * n as i64,
    }
}

fn sample_values(channel_id: &str) -> Vec<Value> {
    if channel_id == "200" {
        return welcome_values();
    }
    if channel_id == "204" {
        return showcase_values();
    }
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
            "timestamp": iso_minutes_ago(minutes_ago(n)),
        });
        if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            obj.extend(extra.clone());
        }
        v
    };
    vec![
        msg(
            &sam,
            "hey, is the new client working?",
            json!({ "pinned": true }),
        ),
        msg(
            &alex,
            "Unicode: Përshëndetje! Здравствуй 你好 こんにちは 안녕하세요 مرحبا नमस्ते Γειά 😀🎉👍❤️ ✓ ★ → ♫ <:pepe:123>",
            json!({}),
        ),
        msg(&me, "yeah! you can drag the panels around now", json!({})),
        msg(
            &alex,
            "nice, does it do replies?",
            json!({ "referenced_message": { "id": (last - 5).to_string(), "author": me,
                    "content": "yeah! you can drag the panels around now" } }),
        ),
        msg(
            &me,
            "and edits",
            json!({ "edited_timestamp": iso_minutes_ago(30) }),
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

/// #showcase: Unicode coverage, stickers, system messages, a forward, a
/// poll and content RustCord can't draw (never blank).
fn showcase_values() -> Vec<Value> {
    let sam = user(SAM, "Sam", None);
    let alex = user(ALEX, "Alex", None);
    let maya = user("610000000000000004", "Maya", None);
    let olly = user("390000000000000007", "Olly", None);
    let me = user(ME, "You", None);
    let items: Vec<(Value, i64, &str, Value)> = vec![
        (
            alex.clone(),
            90,
            "Symbols ☆♪✓⚡☯ ★✦✧❤❥ ♠♣♥♦ ☀☁☂☃ ⌘⌥⏎ · arrows ←↑→↓⇄⟶ · box ╔═╗║╚═╝ ▀▄█░▒▓ · Braille ⠓⠑⠇⠇⠕ · math ∑∫√≠≈∞",
            json!({}),
        ),
        (
            alex.clone(),
            89,
            "Fancy text: 𝓯𝓪𝓷𝓬𝔂 𝕥𝕖𝕩𝕥 𝐛𝐨𝐥𝐝 𝘪𝘵𝘢𝘭𝘪𝘤 𝚖𝚘𝚗𝚘 ⓒⓘⓡⓒⓛⓔ ｆｕｌｌｗｉｄｔｈ · Zalgo: Z̷a̶l̵g̴o̸ t̶̡e̷x̸t̴",
            json!({}),
        ),
        (
            alex.clone(),
            88,
            "Scripts: עברית · العربية · 中文 · 日本語 · 한국어 · ไทย · हिन्दी · ქართული · Հայերեն · አማርኛ · ᏣᎳᎩ · Ελληνικά · Русский",
            json!({}),
        ),
        (
            alex.clone(),
            87,
            "Kaomoji ʕ•ᴥ•ʔ (╯°□°)╯︵ ┻━┻ ¯\\_(ツ)_/¯ ༼ つ ◕_◕ ༽つ ꧁꧂ · flags 🇬🇧🇽🇰 · keycaps 1️⃣2️⃣#️⃣ · ZWJ 👨‍👩‍👧 🏳️‍🌈 · skin 👍🏽",
            json!({}),
        ),
        (
            sam.clone(),
            60,
            "",
            json!({ "sticker_items": [{ "id": "749054660769218631", "name": "Wumpus wave", "format_type": 1 }] }),
        ),
        (
            sam.clone(),
            59,
            "",
            json!({ "sticker_items": [{ "id": "816087792291282944", "name": "Hype dance", "format_type": 3 }] }),
        ),
        (maya.clone(), 50, "", json!({ "type": 7 })),
        (olly.clone(), 45, "", json!({ "type": 7 })),
        (sam.clone(), 40, "", json!({ "type": 6 })),
        (alex.clone(), 35, "release-notes", json!({ "type": 18 })),
        (maya.clone(), 30, "2", json!({ "type": 8 })),
        (
            me.clone(),
            25,
            "",
            json!({ "type": 3, "call": { "participants": [ME] } }),
        ),
        (
            alex.clone(),
            20,
            "",
            json!({ "message_reference": { "type": 1 }, "message_snapshots": [{ "message": {
            "content": "forwarded from #general: **ship it** 🚀", "attachments": [], "embeds": [] } }] }),
        ),
        (
            sam.clone(),
            15,
            "",
            json!({ "poll": { "question": { "text": "Tabs or spaces?" } } }),
        ),
        (
            alex,
            10,
            "",
            json!({ "type": 0, "components": [{ "type": 1, "components": [{ "type": 2, "label": "Click" }] }] }),
        ),
        (sam, 5, "", json!({ "type": 0 })),
    ];
    let last = last_id("204");
    let first = last + 1 - items.len() as u64;
    items
        .into_iter()
        .enumerate()
        .map(|(i, (author, ago, content, extra))| {
            let mut v = json!({
                "id": (first + i as u64).to_string(), "channel_id": "204", "author": author,
                "content": content, "timestamp": iso_minutes_ago(ago),
            });
            if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
                obj.extend(extra.clone());
            }
            v
        })
        .collect()
}

/// #welcome: everything Batch C can draw.
fn welcome_values() -> Vec<Value> {
    let sam = user(SAM, "Sam", None);
    let alex = user(ALEX, "Alex", None);
    let maya = user("610000000000000004", "Maya", None);
    let me = user(ME, "You", None);
    let troll = user(TROLL, "Troll", None);
    let mut bot = user("700000000000000006", "RustBot", None);
    bot["bot"] = json!(true);
    let soon = chrono::Utc::now().timestamp() + 2 * 3600;

    let items: Vec<(Value, i64, &str, Value)> = vec![
        (
            sam.clone(),
            2 * 24 * 60 + 39,
            "**Welcome to the RustCord Test Server!** 🦀\nSay hi in <#201>, and ping <@&501> if something breaks.",
            json!({ "pinned": true }),
        ),
        (
            alex.clone(),
            2 * 24 * 60 + 30,
            "# Markdown test\nSome **bold**, *italic*, __underline__, ~~strike~~ and `inline code`.\n> quoting <@300000000000000002> here\n- lists work\n  - nested too\n||spoiler: the cake is a lie||\nRelease party <t:SOON:R> (<t:SOON:f>) :pepe: <:pepe:123>",
            json!({ "mentions": [sam] }),
        ),
        (
            me.clone(),
            24 * 60 + 29,
            "```rust\nfn main() {\n    println!(\"hello from RustCord\");\n}\n```",
            json!({}),
        ),
        (troll.clone(), 24 * 60 + 20, "first!!1 lol", json!({})),
        (troll, 24 * 60 + 19, "anyone there??", json!({})),
        (
            bot,
            50,
            "",
            json!({ "embeds": [{
                "type": "rich",
                "title": "Build #128 passed",
                "url": "https://github.com/KeoLogs/rustcord/actions",
                "description": "All checks are green on `feat/messenger-complete`.\n**3** jobs · ~~2~~ 0 warnings",
                "color": 0x57f287,
                "author": { "name": "GitHub Actions", "url": "https://github.com/KeoLogs/rustcord" },
                "fields": [
                    { "name": "Linux", "value": "✅ 2m 10s", "inline": true },
                    { "name": "Windows", "value": "✅ 4m 02s", "inline": true },
                    { "name": "macOS", "value": "✅ 3m 31s", "inline": true },
                    { "name": "Commit", "value": "[`df82209`](https://github.com/KeoLogs/rustcord/commit/df82209) Polish pass" }
                ],
                "footer": { "text": "RustCord CI" },
                "timestamp": iso_minutes_ago(50),
            }] }),
        ),
        (
            maya,
            30,
            "this never gets old https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            json!({ "pinned": true, "embeds": [{
                "type": "video",
                "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "title": "Rick Astley - Never Gonna Give You Up (Official Video)",
                "description": "The official video for “Never Gonna Give You Up” by Rick Astley.",
                "color": 0xff0000,
                "provider": { "name": "YouTube", "url": "https://www.youtube.com" },
                "author": { "name": "Rick Astley", "url": "https://www.youtube.com/@RickAstleyYT" },
                "thumbnail": { "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg", "width": 480, "height": 360 },
                "video": { "url": "https://www.youtube.com/embed/dQw4w9WgXcQ", "width": 1280, "height": 720 }
            }] }),
        ),
        (
            sam,
            20,
            "egui docs: [docs.rs/egui](https://docs.rs/egui) · Discord's own (trusted, opens straight away): <https://discord.com/developers/docs> · dev chat in <#203>",
            json!({}),
        ),
        (
            alex,
            10,
            "new screenshot",
            json!({ "attachments": [{
                "url": "https://cdn.discordapp.com/attachments/1/2/screenshot.png",
                "proxy_url": "https://media.discordapp.net/attachments/1/2/screenshot.png",
                "filename": "screenshot.png", "content_type": "image/png",
                "width": 1280, "height": 800, "size": 245000
            }] }),
        ),
    ];
    let last = last_id("200");
    let first = last + 1 - items.len() as u64;
    items
        .into_iter()
        .enumerate()
        .map(|(i, (author, ago, content, extra))| {
            let mut v = json!({
                "id": (first + i as u64).to_string(), "channel_id": "200", "author": author,
                "content": content.replace("SOON", &soon.to_string()),
                "timestamp": iso_minutes_ago(ago),
            });
            if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
                obj.extend(extra.clone());
            }
            v
        })
        .collect()
}
