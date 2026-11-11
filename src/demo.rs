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
//! - #general has a thread card (type 18) for "release plans" with three
//!   messages; it shows the count until the thread is opened once, then
//!   Sam's last message. #showcase's thread start names no thread we know
//!   ("This thread isn't loaded", not clickable).
//! - The GIF button shows fake results (offline, so the pictures fail).
//!
//! History and sending (Brief A) in the demo:
//! - #history has 300 messages: scrolling up loads 50 more at a time
//!   (`before=`), and it opens at the first unread message (120 unread, so
//!   it loads around your last read one, with the red NEW line).
//! - Sam's link in #welcome jumps to an old #history message: with #history
//!   already open that leaves a gap ("Load more"); "Jump to present" brings
//!   you back.
//! - Sending shows the message faded for ~0.7 s. A message with the word
//!   "fail" in it fails the first time (red, Retry / Delete).
//! - The status bar's Reconnect pretends to reconnect.
//!
//! `RUSTCORD_DEMO_BIG=1` adds a synthetic big account for memory tests
//! (`demo/big.rs`): ~100 more servers, ~3000 channels, ~200 DMs and ~1000
//! friends in one realistic-size READY, fed through the gateway's inflater
//! and dispatch like a live connection; their channels have 50-message
//! histories with many avatars and custom emoji.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

mod big;

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
const MAYA: &str = "610000000000000004";
const STRANGER: &str = "970000000000000011";

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
    (ME, "You", &["503", "505"], "online", None),
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
        "206" => HISTORY_LAST,
        "250" => 263,
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
    let test_guild_json = json!({
        "id": TEST_GUILD,
        "name": "RustCord Test Server",
        "owner_id": SAM,
        "features": ["COMMUNITY"],
        "emojis": [{ "id": "123", "name": "pepe" }],
        "roles": [
            { "id": TEST_GUILD, "name": "@everyone", "color": 0, "position": 0, "permissions": "104324673" },
            { "id": "500", "name": "Admin", "color": 0xf1c40f, "position": 6, "hoist": true, "permissions": "8" },
            // You: every server tool (Server Settings) without Administrator,
            // so channel permissions still apply to you.
            { "id": "505", "name": "Manager", "color": 0, "position": 5, "permissions": crate::admin::demo::MANAGER_PERMISSIONS.to_string() },
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
            ch("206", "history", 4, Some("300")),
            // Forum (Threads module).
            { "id": "205", "name": "help", "type": 15, "position": 3, "parent_id": "300",
              "topic": "Ask anything about RustCord.",
              "available_tags": [{ "id": "900", "name": "Question" }, { "id": "901", "name": "Bug" }] },
            { "id": "301", "name": "Dev", "type": 4, "position": 2 },
            { "id": "203", "name": "rust", "type": 0, "position": 0, "parent_id": "301",
              "last_message_id": last_id("203").to_string(),
              "topic": "RustCord development · egui 0.37 · PRs welcome" },
            { "id": "302", "name": "Voice Channels", "type": 4, "position": 3 },
            { "id": "230", "name": "General", "type": 2, "position": 0, "parent_id": "302" },
            { "id": "231", "name": "Gaming", "type": 2, "position": 1, "parent_id": "302" },
            { "id": "232", "name": "Town Hall", "type": 13, "position": 2, "parent_id": "302" },
        ]
    });
    let guild_values = vec![
        test_guild_json,
        json!({
            "id": "101",
            "name": "Gaming Hangout",
            "channels": [ch("210", "lobby", 0, None), ch("211", "clips", 1, None)]
        }),
        json!({
            "id": "102",
            "name": "Big Public Server",
            "channels": [ch("220", "announcements", 0, None)]
        }),
    ];
    let dm_values = vec![
        json!({
            "id": "400", "type": 1, "last_message_id": "9",
            "recipients": [user(SAM, "Sam", None)]
        }),
        json!({
            "id": "401", "type": 1, "last_message_id": "95",
            "recipients": [user(ALEX, "Alex", None)]
        }),
        // A message request (Friends module): never auto-acked.
        json!({
            "id": "402", "type": 1, "last_message_id": "3",
            "is_message_request": true,
            "is_message_request_timestamp": iso_minutes_ago(90),
            "recipients": [user(STRANGER, "Stranger", None)]
        }),
    ];
    let read_values = json!([
        { "id": "200", "last_message_id": "10" },
        { "id": "201", "last_message_id": "26" },
        { "id": "202", "last_message_id": "33" },
        { "id": "203", "last_message_id": "57", "mention_count": 2 },
        // 120 unread: the first unread isn't in the newest page.
        { "id": "206", "last_message_id": (HISTORY_LAST - 120).to_string() },
        { "id": "210", "last_message_id": "78", "mention_count": 1 },
        { "id": "211", "last_message_id": "85" },
        { "id": "220", "last_message_id": "80" },
        { "id": "400", "last_message_id": "9" },
        { "id": "401", "last_message_id": "94", "mention_count": 1 },
    ]);
    let guild_setting_values = json!([
        { "guild_id": "100", "message_notifications": 1,
          "channel_overrides": [{ "channel_id": "202", "muted": true, "mute_config": null }] },
        { "guild_id": "102", "muted": true,
          "mute_config": { "end_time": null, "selected_time_window": -1 } },
    ]);
    // --- friends --- (READY relationships + message requests)
    let ready_rels = json!({ "relationships": [
        { "id": SAM, "type": 1, "user": user(SAM, "Sam", None) },
        { "id": ALEX, "type": 1, "user": user(ALEX, "Alex", None) },
        { "id": MAYA, "type": 3, "user": user(MAYA, "Maya", None) },
        { "id": TROLL, "type": 2, "user": user(TROLL, "Troll", None) },
    ], "private_channels": [
        { "id": "402", "type": 1, "is_message_request": true,
          "is_message_request_timestamp": iso_minutes_ago(90) },
    ] });
    let big_account = big::enabled();
    if big_account {
        // RUSTCORD_DEMO_BIG: the demo's servers plus ~100 generated ones in
        // one READY, through the gateway's inflater and dispatch.
        let as_vec = |v: &Value| v.as_array().cloned().unwrap_or_default();
        let parts = big::DemoParts {
            guilds: guild_values,
            merged_members: vec![
                json!([{ "user_id": ME, "roles": ["503", "505"] }]),
                json!([]),
                json!([]),
            ],
            dms: dm_values,
            read_state: as_vec(&read_values),
            guild_settings: as_vec(&guild_setting_values),
            relationships: as_vec(&ready_rels["relationships"]),
            users: Vec::new(),
        };
        let started = std::time::Instant::now();
        let frames = [
            big::ready_frame(parts),
            big::compress(&big::supplemental_payload(&[TEST_GUILD, "101", "102"])),
        ];
        let built = started.elapsed();
        let mut my_id = String::new();
        let mut inflater = crate::gateway::Inflater::new();
        for frame in frames {
            let events = inflater
                .push(&frame)
                .ok()
                .flatten()
                .and_then(|json| crate::gateway::dispatch_bytes(&json, &mut my_id).ok())
                .unwrap_or_default();
            for ev in events {
                send(ev);
            }
        }
        eprintln!(
            "demo: big account frames built in {} ms, inflated + parsed in {} ms",
            built.as_millis(),
            (started.elapsed() - built).as_millis()
        );
    } else {
        let mut guilds: Vec<Guild> = guild_values.iter().map(Guild::from_json).collect();
        guilds[0].my_roles = vec!["503".into(), "505".into()];
        send(ToUi::Ready {
            me: User::from_json(&me),
            premium_type: 0,
            guilds,
            dms: dm_values.iter().map(Channel::from_json).collect(),
            read_state: ReadStateEntry::parse_ready(&read_values),
            guild_settings: GuildSettings::parse_ready(&guild_setting_values),
            blocked: vec![TROLL.into()],
            user_settings: None,
        });
    }

    // --- threads --- (READY guild threads: one under #general)
    if let Some(ev) = crate::threads::ThreadEvent::from_guild(&json!({
        "id": TEST_GUILD,
        "threads": [
            { "id": "250", "type": 11, "name": "release plans", "parent_id": "201",
              "owner_id": SAM, "message_count": 3, "member_count": 3,
              "last_message_id": last_id("250").to_string(),
              "member": { "id": "250", "flags": 1 },
              "thread_metadata": { "archived": false, "auto_archive_duration": 4320 } },
            { "id": "251", "type": 11, "name": "theme ideas", "parent_id": "201",
              "owner_id": ALEX, "message_count": 3, "member_count": 2,
              "last_message_id": last_id("201").to_string(),
              "thread_metadata": { "archived": false, "auto_archive_duration": 1440 } },
            { "id": "252", "type": 11, "name": "old bug hunt", "parent_id": "201",
              "owner_id": SAM, "message_count": 12, "member_count": 4,
              "last_message_id": last_id("201").to_string(),
              "thread_metadata": { "archived": true, "auto_archive_duration": 1440 } }
        ]
    })) {
        send(ToUi::Thread(ev));
    }

    // The big READY already carried these (with ~1000 more friends).
    if !big_account {
        send(ToUi::Relationships(crate::friends::parse_ready(
            &ready_rels,
        )));
        send(ToUi::MessageRequests(crate::friends::parse_ready_requests(
            &ready_rels,
        )));
    }

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
    // Nonces of "fail" messages that already failed once (Retry works).
    let mut failed_once = std::collections::HashSet::new();
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
                let messages = page_before(history(&channel_id, &created, &pinned), None);
                send(ToUi::History {
                    messages,
                    channel_id,
                })
            }
            ToBackend::LoadPage { channel_id, before } => {
                eprintln!("demo: page of #{channel_id} before {before:?}");
                tokio::time::sleep(Duration::from_millis(250)).await;
                let all = history(&channel_id, &created, &pinned);
                let at = before.as_deref().and_then(|b| b.parse().ok());
                send(ToUi::Page {
                    messages: page_before(all, at),
                    channel_id,
                    before,
                })
            }
            ToBackend::LoadAround {
                channel_id,
                message_id,
            } => {
                let all = history(&channel_id, &created, &pinned);
                let messages = page_around(all, message_id.parse().unwrap_or(0));
                send(ToUi::HistoryAround {
                    channel_id,
                    message_id,
                    messages,
                })
            }
            ToBackend::Reconnect => {
                eprintln!("demo: reconnect");
                send(ToUi::Status("Reconnecting…".into()));
                tokio::time::sleep(Duration::from_millis(800)).await;
                send(ToUi::Resumed);
            }
            ToBackend::CopyImage { url } => {
                eprintln!("demo: copy image {url}");
                send(ToUi::Notice("Demo mode: would copy the image".into()));
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
            ToBackend::Admin(req) => {
                // Bodies and replies are never printed (webhook URLs).
                eprintln!("demo: {req:?}");
                send(ToUi::Admin(Box::new(crate::admin::Response {
                    result: crate::admin::demo::respond(&req),
                    guild_id: req.guild_id.clone(),
                    kind: req.kind,
                    key: req.key.clone(),
                    was_write: req.is_write(),
                })));
            }
            ToBackend::AdminPick(slot) => {
                // No file dialog in the demo: a tiny stand-in picture.
                send(ToUi::AdminPicked(Box::new(crate::admin::Picked {
                    slot,
                    name: "demo-image.png".into(),
                    mime: "image/png".into(),
                    bytes: std::sync::Arc::new(vec![0x89, b'P', b'N', b'G']),
                })));
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
                nonce,
                content,
                files,
                spoilers,
                ..
            } => {
                // Long enough to see the faded message.
                tokio::time::sleep(Duration::from_millis(700)).await;
                if content.to_lowercase().contains("fail") && failed_once.insert(nonce.clone()) {
                    send(ToUi::SendFailed {
                        channel_id,
                        nonce,
                        error: "Demo mode: this one fails the first time".into(),
                    });
                    continue;
                }
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
                        "nonce": nonce,
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
            // A real PNG in the temp folder (never the user's config).
            ToBackend::SavePastedImage(img) => {
                match crate::paste::save_png(&img, &crate::paste::paste_dir()) {
                    Ok(path) => send(ToUi::FilesPicked(vec![path])),
                    Err(e) => send(ToUi::Notice(format!("Couldn't paste the image: {e}"))),
                }
            }
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
            // QR login is the login screen's own task (remote_auth.rs).
            ToBackend::StartQrLogin | ToBackend::CancelQrLogin => {}
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
            // --- themes --- (the picker works offline too)
            ToBackend::PickTheme { folder } => {
                send(ToUi::ThemePicked(
                    crate::themes::pick_import_source(folder).await,
                ));
            }
            // --- presets --- (local files; work offline too)
            ToBackend::PickPreset => {
                if let Some(e) = crate::presets::pick_file().await {
                    send(ToUi::PresetFile(e));
                }
            }
            ToBackend::ExportPreset { file_name, text } => {
                if let Some(e) = crate::presets::export_file(&file_name, text).await {
                    send(ToUi::PresetFile(e));
                }
            }
            // --- onboarding --- (preview works; joining is off in the demo)
            ToBackend::ResolveInvite { code } => send(ToUi::InviteResolved {
                result: crate::invites::classify_fetch(
                    200,
                    &json!({
                        "code": code, "type": 0,
                        "guild": { "id": "103", "name": "Rust Lounge", "features": [] },
                        "approximate_member_count": 1234, "approximate_presence_count": 321
                    }),
                ),
                code,
            }),
            ToBackend::JoinInvite { code } => send(ToUi::InviteJoined {
                code,
                outcome: crate::invites::JoinOutcome::Failed("Demo mode: not joining".into()),
            }),
            ToBackend::LoadScreening { guild_id } => send(ToUi::ScreeningLoaded {
                guild_id,
                result: Ok(crate::screening::VerificationForm::from_json(&json!({
                    "version": "2026-01-01T00:00:00+00:00",
                    "description": "Welcome! Please read the rules before chatting.",
                    "form_fields": [{
                        "field_type": "TERMS", "label": "Read and agree to the server rules",
                        "values": ["Be kind to each other.", "No spam or self-promotion.",
                                   "Keep spoilers in spoiler tags."],
                        "required": true
                    }]
                }))),
            }),
            ToBackend::LoadOnboarding { guild_id, page } => send(ToUi::OnboardingLoaded {
                result: Ok(crate::onboarding::Onboarding::from_json(&json!({
                    "guild_id": guild_id, "enabled": true, "mode": 0,
                    "default_channel_ids": ["200", "201"],
                    "prompts": [
                        { "id": "p1", "type": 0, "title": "What brings you here?",
                          "single_select": false, "required": true, "in_onboarding": true,
                          "options": [
                            { "id": "o1", "title": "Using RustCord", "description": "Help and tips",
                              "role_ids": ["504"], "channel_ids": ["205"] },
                            { "id": "o2", "title": "Hacking on it", "description": "Dev talk and PRs",
                              "role_ids": ["503"], "channel_ids": ["203"] },
                            { "id": "o3", "title": "Just hanging out", "channel_ids": ["202"] }
                          ] },
                        { "id": "p2", "type": 1, "title": "Pick a region",
                          "single_select": true, "required": false, "in_onboarding": true,
                          "options": [
                            { "id": "o4", "title": "Europe" }, { "id": "o5", "title": "Americas" },
                            { "id": "o6", "title": "Asia / Pacific" }
                          ] }
                    ]
                }))),
                guild_id,
                page,
            }),
            ToBackend::SubmitScreening { guild_id, .. }
            | ToBackend::SubmitOnboarding { guild_id, .. } => {
                eprintln!("demo: submit for {guild_id}");
            }
            // --- threads --- (the demo has no threads; nothing to load)
            ToBackend::LoadActiveThreads { guild_id } => {
                send(ToUi::ThreadsUnavailable { guild_id });
            }
            ToBackend::Threads(crate::threads::ThreadAction::LoadMore { forum_id, query }) => {
                let page = crate::threads::ThreadPage::from_json(
                    &json!({ "threads": [{
                        "id": "260", "type": 11, "name": "How do I change the theme?",
                        "parent_id": forum_id, "guild_id": TEST_GUILD, "owner_id": ALEX,
                        "message_count": 2, "applied_tags": ["900"],
                        "thread_metadata": { "archived": false }
                    }], "has_more": false, "total_results": 1 }),
                    Some(TEST_GUILD),
                );
                send(ToUi::ForumPage {
                    forum_id,
                    query,
                    page,
                });
            }
            ToBackend::Threads(crate::threads::ThreadAction::JoinThread { thread_id }) => {
                send(ToUi::ThreadJoined {
                    thread_id,
                    joined: true,
                });
            }
            ToBackend::Threads(crate::threads::ThreadAction::CreateThread {
                channel_id,
                name,
                private,
                auto_archive_minutes,
                ..
            }) => {
                // Only after the dialog’s Create was clicked.
                next_id += 1;
                let thread = crate::threads::Thread::from_json(
                    &json!({
                        "id": next_id.to_string(), "type": if private { 12 } else { 11 },
                        "name": name, "parent_id": channel_id, "owner_id": ME,
                        "member": { "id": next_id.to_string(), "flags": 1 },
                        "thread_metadata": {
                            "archived": false, "auto_archive_duration": auto_archive_minutes
                        }
                    }),
                    Some(TEST_GUILD),
                );
                send(ToUi::ThreadCreated(thread));
            }
            ToBackend::Threads(crate::threads::ThreadAction::LeaveThread { thread_id }) => {
                send(ToUi::ThreadJoined {
                    thread_id,
                    joined: false,
                });
            }
            ToBackend::Threads(action) => eprintln!("demo: threads {action:?}"),
            // --- friends ---
            ToBackend::Friends(action) => {
                use crate::friends::{FriendsAction, Outcome};
                eprintln!("demo: friends {action:?}");
                let result = match &action {
                    FriendsAction::Message(id) => Ok(Outcome::DmOpened(json!({
                        "id": "403", "type": 1, "last_message_id": "1",
                        "recipients": [user(id, "Friend", None)]
                    }))),
                    FriendsAction::SendRequest(name) => Ok(Outcome::RequestSent(name.clone())),
                    _ => Ok(Outcome::Done),
                };
                send(ToUi::FriendResult { action, result });
            }
            ToBackend::MessageRequest(action) => {
                eprintln!("demo: message request {action:?}");
                send(ToUi::RequestResult {
                    action,
                    result: Ok(()),
                });
            }
            // --- search --- (substring match over the sample channels)
            ToBackend::Search { seq, query, .. } => {
                let needle = query.content.to_lowercase();
                let messages: Vec<Message> = ["200", "201", "202", "203", "204", "210"]
                    .iter()
                    .flat_map(|c| history(c, &created, &pinned))
                    .filter(|m| !needle.is_empty() && m.content.to_lowercase().contains(&needle))
                    .take(crate::search::PAGE_SIZE as usize)
                    .collect();
                send(ToUi::SearchResults {
                    seq,
                    total: messages.len() as u64,
                    messages,
                    partial: false,
                });
            }
            // --- reactions --- (echo like the gateway would)
            ToBackend::React {
                channel_id,
                message_id,
                emoji,
                add,
                burst,
            } => {
                let d = json!({
                    "channel_id": channel_id, "message_id": message_id, "user_id": ME,
                    "emoji": { "id": emoji.id, "name": emoji.name, "animated": emoji.animated },
                    "burst": burst,
                });
                let t = if add {
                    "MESSAGE_REACTION_ADD"
                } else {
                    "MESSAGE_REACTION_REMOVE"
                };
                if let Some(ev) = crate::reactions::ReactionEvent::from_dispatch(t, &d) {
                    send(ToUi::Reaction(ev));
                }
            }
            ToBackend::LoadReactors {
                message_id,
                emoji,
                burst,
                ..
            } => send(ToUi::Reactors {
                message_id,
                emoji,
                burst,
                names: vec!["Sam".into(), "Alex".into(), "You".into()],
            }),
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

/// Newest message in #history (ids 600..=899; sent messages start at 1001).
const HISTORY_LAST: u64 = 899;
/// Messages in #history.
const HISTORY_LEN: u64 = 300;

/// #history: 300 numbered messages, a few minutes apart.
fn history_values() -> Vec<Value> {
    let people = [
        user(SAM, "Sam", None),
        user(ALEX, "Alex", None),
        user(MAYA, "Maya", None),
    ];
    let first = HISTORY_LAST + 1 - HISTORY_LEN;
    (first..=HISTORY_LAST)
        .map(|id| {
            let n = id - first + 1;
            let ago = ((HISTORY_LAST - id) * 7) as i64;
            json!({
                "id": id.to_string(), "channel_id": "206",
                "author": people[(n % 3) as usize],
                "content": format!("History message #{n} (id {id})"),
                "timestamp": iso_minutes_ago(ago),
            })
        })
        .collect()
}

/// Up to 50 messages before `before` (None: the newest), oldest first.
fn page_before(all: Vec<Message>, before: Option<u64>) -> Vec<Message> {
    let mut older: Vec<Message> = all
        .into_iter()
        .filter(|m| before.is_none_or(|b| crate::history::id_of(m) < b))
        .collect();
    let skip = older.len().saturating_sub(crate::history::PAGE_SIZE);
    older.drain(..skip);
    older
}

/// 50 messages around `id`, like `?around=`.
fn page_around(all: Vec<Message>, id: u64) -> Vec<Message> {
    let half = crate::history::PAGE_SIZE / 2;
    let at = all
        .iter()
        .position(|m| crate::history::id_of(m) >= id)
        .unwrap_or(all.len());
    let start = at.saturating_sub(half);
    all.into_iter()
        .skip(start)
        .take(crate::history::PAGE_SIZE)
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
    if let Some(values) = big::history_values(channel_id) {
        return values;
    }
    if channel_id == "200" {
        return welcome_values();
    }
    if channel_id == "204" {
        return showcase_values();
    }
    if channel_id == "250" {
        return thread_values();
    }
    if channel_id == "206" {
        return history_values();
    }
    let sam = user(SAM, "Sam", None);
    let alex = user(ALEX, "Alex", None);
    let me = user(ME, "You", None);
    // Ids count up to the channel's last message so read state lines up.
    let last = last_id(channel_id);
    // #general has one more: the thread card (inserted below).
    let first = if channel_id == "201" {
        last - 8
    } else {
        last - 7
    };
    let mut n = 0u64;
    let mut msg = |author: &Value, content: &str, extra: Value| {
        let id = first + n;
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
    let mut out = vec![
        msg(
            &sam,
            "hey, is the new client working? the test lounge: https://discord.gg/rustcord",
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
            json!({ "referenced_message": { "id": (first + 2).to_string(), "author": me,
                    "content": "yeah! you can drag the panels around now" } }),
        ),
        msg(
            &me,
            "and edits",
            json!({ "edited_timestamp": iso_minutes_ago(30) }),
        ),
        msg(
            &sam,
            "the layout looks sick 🔥",
            json!({ "reactions": [
                { "emoji": { "id": null, "name": "🔥" }, "count": 3,
                  "count_details": { "burst": 0, "normal": 3 }, "me": true },
                { "emoji": { "id": null, "name": "👍" }, "count": 1,
                  "count_details": { "burst": 0, "normal": 1 }, "me": false }
            ] }),
        ),
        msg(
            &alex,
            "@You did you see the unread badges?",
            json!({ "mentions": [me] }),
        ),
        if channel_id == "401" {
            msg(
                &alex,
                "look at this",
                json!({ "attachments": [{ "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg", "proxy_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg", "filename": "big.jpg", "content_type": "image/jpeg", "width": 1280, "height": 720, "size": 120000 }] }),
            )
        } else {
            msg(
                &sam,
                "here's the build log",
                json!({ "attachments": [{ "url": "https://example.com/build.log", "filename": "build.log", "size": 20480, "content_type": "text/plain" }] }),
            )
        },
    ];
    if channel_id == "201" {
        // A thread start (type 18) for "release plans" (thread 250), after
        // the sixth message; the two after it move up one id.
        for v in &mut out[6..] {
            let id = v["id"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            v["id"] = json!((id + 1).to_string());
        }
        out.insert(
            6,
            json!({
                "id": (first + 6).to_string(), "channel_id": channel_id, "type": 18,
                "author": sam, "content": "release plans",
                "timestamp": iso_minutes_ago(minutes_ago(6) - 1),
                "message_reference": { "channel_id": "250", "guild_id": TEST_GUILD },
            }),
        );
    }
    out
}

/// Thread "release plans" (250) under #general: what its card previews
/// once it has been opened.
fn thread_values() -> Vec<Value> {
    let last = last_id("250");
    [
        (
            user(SAM, "Sam", None),
            30,
            "ok so: themes first, then the thread cards",
        ),
        (
            user(ALEX, "Alex", None),
            20,
            "sounds good, I'll take the **theme files**",
        ),
        (user(SAM, "Sam", None), 10, "ship it friday? 🚀"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (author, ago, content))| {
        json!({
            "id": (last - 2 + i as u64).to_string(), "channel_id": "250",
            "author": author, "content": content,
            "timestamp": iso_minutes_ago(ago),
        })
    })
    .collect()
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
            "egui docs: [docs.rs/egui](https://docs.rs/egui) · Discord's own (trusted, opens straight away): <https://discord.com/developers/docs> · dev chat in <#203> · an old one in #history: https://discord.com/channels/100/206/640",
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
