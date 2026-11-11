//! `RUSTCORD_DEMO=1 RUSTCORD_DEMO_BIG=1`: a synthetic big account for
//! memory measurements without a token (`scripts/memprobe.ps1`).
//!
//! It builds a READY of realistic size (~100 servers, ~3000 channels,
//! ~200 DMs, roles, emoji, stickers, read state, relationships) in the same
//! JSON shape a user account's real READY has, zlib-compresses it like the
//! gateway's `compress=zlib-stream` and hands the bytes to `demo.rs`, which
//! runs them through the gateway's own `Inflater` and dispatch path. So a
//! change to how READY is parsed shows up in the demo's numbers.
//!
//! Everything is generated from indices (nothing random, no big file in the
//! repo). Avatars, icons and emoji point at the real CDN; offline they fail,
//! which is fine for measuring.

use std::io::Write;
use std::sync::OnceLock;

use serde_json::{Value, json};

pub const GUILDS: usize = 100;
/// Categories + text + voice channels per generated server.
pub const CHANNELS_PER_GUILD: usize = 30;
const CATEGORIES_PER_GUILD: usize = 5;
pub const DMS: usize = 200;
const ROLES_PER_GUILD: usize = 150;
const EMOJIS_PER_GUILD: usize = 150;
const STICKERS_PER_GUILD: usize = 8;
const THREADS_PER_GUILD: usize = 6;
/// Distinct people in the account's world (DMs, friends, members).
const USERS: u64 = 3000;
const RELATIONSHIPS: u64 = 1000;
/// Presences / members per server in READY_SUPPLEMENTAL.
const PRESENCES_PER_GUILD: u64 = 40;
/// Messages in a generated channel's history.
const HISTORY: u64 = 50;

const GUILD_BASE: u64 = 810_000_000_000_000_000;
const EMOJI_BASE: u64 = 820_000_000_000_000_000;
const USER_BASE: u64 = 830_000_000_000_000_000;
const DM_BASE: u64 = 840_000_000_000_000_000;
const MSG_BASE: u64 = 1_300_000_000_000_000_000;

const WORDS: &[&str] = &[
    "general", "memes", "music", "gaming", "dev", "art", "news", "help", "random", "clips", "rust",
    "linux", "trading", "events", "lounge", "voice", "media", "bots", "rules", "chill", "anime",
    "food", "photos", "builds", "support", "showcase", "ideas", "offtopic", "maps", "mods",
];

/// Is `RUSTCORD_DEMO_BIG` set (read once)?
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTCORD_DEMO_BIG").is_some_and(|v| v != "0"))
}

fn word(i: u64) -> &'static str {
    WORDS[(i as usize) % WORDS.len()]
}

/// A 32-hex-char hash (avatar / icon / banner) derived from `n`.
fn hash(n: u64) -> String {
    let a = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = a.rotate_left(29) ^ 0xD1B5_4A32_D192_ED03;
    format!("{a:016x}{b:016x}")
}

pub fn guild_id(g: usize) -> u64 {
    GUILD_BASE + g as u64 * 10_000
}

/// Channel `c` of generated server `g`.
pub fn channel_id(g: usize, c: usize) -> u64 {
    guild_id(g) + 1 + c as u64
}

fn role_id(g: usize, r: usize) -> u64 {
    guild_id(g) + 5_000 + r as u64
}

fn user_id(u: u64) -> u64 {
    USER_BASE + (u % USERS) * 7
}

/// Is this one of the generated channels (servers or DMs)?
pub fn is_big_channel(id: &str) -> bool {
    id.parse::<u64>().is_ok_and(|n| {
        (GUILD_BASE..GUILD_BASE + GUILDS as u64 * 10_000).contains(&n)
            || (DM_BASE..DM_BASE + DMS as u64).contains(&n)
    })
}

/// A full user object as READY sends it (avatar, decoration, clan tag...).
pub fn user(u: u64) -> Value {
    let id = user_id(u);
    let name = format!("{}_{}", word(u), u);
    json!({
        "id": id.to_string(),
        "username": name,
        "global_name": format!("{} {}", word(u + 3), u),
        "avatar": if u.is_multiple_of(9) { Value::Null } else if u.is_multiple_of(5) {
            json!(format!("a_{}", hash(id)))
        } else { json!(hash(id)) },
        "discriminator": "0",
        "public_flags": (u % 4) * 64,
        "flags": 0,
        "bot": u.is_multiple_of(97),
        "banner": Value::Null,
        "accent_color": Value::Null,
        "avatar_decoration_data": if u.is_multiple_of(6) {
            json!({ "asset": format!("a_{}", hash(id + 1)), "sku_id": (id + 11).to_string(), "expires_at": Value::Null })
        } else { Value::Null },
        "primary_guild": if u.is_multiple_of(4) {
            json!({ "identity_guild_id": guild_id((u % GUILDS as u64) as usize).to_string(),
                    "identity_enabled": true, "tag": "RUST", "badge": hash(id + 2) })
        } else { Value::Null },
        "collectibles": Value::Null,
        "display_name_styles": Value::Null,
    })
}

fn me() -> Value {
    let mut me = json!({
        "id": super::ME,
        "username": "you",
        "global_name": "You",
        "avatar": Value::Null,
        "discriminator": "0",
        "email": "you@example.invalid",
        "verified": true,
        "mfa_enabled": true,
        "premium_type": 2,
        "flags": 0,
        "phone": Value::Null,
        "locale": "en-GB",
        "nsfw_allowed": true,
    });
    me["bio"] = json!("Synthetic big account for RustCord memory tests.");
    me
}

fn overwrites(g: usize, c: usize) -> Value {
    let gid = guild_id(g);
    Value::Array(
        (0..6)
            .map(|k| {
                json!({
                    "id": if k == 0 { gid.to_string() } else { role_id(g, (c + k) % ROLES_PER_GUILD).to_string() },
                    "type": 0,
                    "allow": ((c * 1024 + k) % 4096).to_string(),
                    "deny": if k == 0 { "2048".to_string() } else { "0".to_string() },
                })
            })
            .collect(),
    )
}

fn channel(g: usize, c: usize) -> Value {
    let id = channel_id(g, c);
    if c < CATEGORIES_PER_GUILD {
        return json!({
            "id": id.to_string(), "type": 4, "name": format!("{} zone", word(c as u64 + g as u64)).to_uppercase(),
            "position": c, "flags": 0, "permission_overwrites": overwrites(g, c),
        });
    }
    let parent = channel_id(g, c % CATEGORIES_PER_GUILD);
    let voice = c.is_multiple_of(6);
    let mut v = json!({
        "id": id.to_string(),
        "type": if voice { 2 } else { 0 },
        "name": format!("{}-{}", word(c as u64 * 7 + g as u64), c),
        "position": c,
        "parent_id": parent.to_string(),
        "flags": 0,
        "permission_overwrites": overwrites(g, c),
        "rate_limit_per_user": if c.is_multiple_of(5) { 10 } else { 0 },
        "nsfw": false,
        "icon_emoji": { "id": Value::Null, "name": "💬" },
        "theme_color": Value::Null,
        "version": 1_700_000_000_000u64 + id % 1_000_000,
    });
    if voice {
        v["bitrate"] = json!(64000);
        v["user_limit"] = json!(0);
        v["rtc_region"] = Value::Null;
    } else {
        v["last_message_id"] = json!(last_message_id(id).to_string());
        v["last_pin_timestamp"] = json!("2026-10-01T12:00:00.000000+00:00");
        v["topic"] = json!(format!(
            "Talk about {} and {} here. Read the pinned messages first, keep it friendly, \
             no spam or self-promotion, and use threads for long discussions. #{c}",
            word(c as u64),
            word(c as u64 + 5)
        ));
        v["default_thread_rate_limit_per_user"] = json!(0);
    }
    v
}

/// Newest message id of a generated channel (its history ends here).
pub fn last_message_id(channel: u64) -> u64 {
    MSG_BASE + (channel % 100_000) * 1_000 + HISTORY
}

fn role(g: usize, r: usize) -> Value {
    let gid = guild_id(g);
    let id = if r == 0 { gid } else { role_id(g, r) };
    let color = if r.is_multiple_of(3) {
        0
    } else {
        (r as u64 * 0x13_57_9B) & 0xFF_FF_FF
    };
    json!({
        "id": id.to_string(),
        "name": if r == 0 { "@everyone".to_string() } else { format!("{} {}", word(r as u64), r) },
        "description": Value::Null,
        "color": color,
        "colors": { "primary_color": color, "secondary_color": Value::Null, "tertiary_color": Value::Null },
        "hoist": r % 10 == 1,
        "icon": if r % 15 == 2 { json!(hash(id)) } else { Value::Null },
        "unicode_emoji": Value::Null,
        "position": ROLES_PER_GUILD - r,
        "permissions": if r == 0 { "2248473465835073".to_string() } else { ((r as u64) << 10).to_string() },
        "managed": r % 20 == 19,
        "mentionable": r.is_multiple_of(4),
        "flags": 0,
        "tags": if r % 20 == 19 { json!({ "bot_id": user_id(r as u64).to_string() }) } else { json!({}) },
    })
}

fn emoji(g: usize, e: usize) -> Value {
    json!({
        "id": (EMOJI_BASE + g as u64 * 1_000 + e as u64).to_string(),
        "name": format!("{}{}", word(e as u64 + 11), e),
        "roles": [],
        "require_colons": true,
        "managed": false,
        "animated": e.is_multiple_of(7),
        "available": true,
    })
}

fn sticker(g: usize, s: usize) -> Value {
    json!({
        "id": (EMOJI_BASE + 500 + g as u64 * 1_000 + s as u64).to_string(),
        "name": format!("{} sticker {s}", word(s as u64)),
        "tags": word(s as u64 + 2),
        "type": 2,
        "format_type": 1,
        "description": "A synthetic sticker for memory tests.",
        "asset": "",
        "available": true,
        "guild_id": guild_id(g).to_string(),
    })
}

fn thread(g: usize, t: usize) -> Value {
    let parent = channel_id(g, CATEGORIES_PER_GUILD + 1 + t);
    let id = guild_id(g) + 8_000 + t as u64;
    json!({
        "id": id.to_string(), "type": 11, "name": format!("{} thread {t}", word(t as u64)),
        "parent_id": parent.to_string(), "guild_id": guild_id(g).to_string(),
        "owner_id": user_id(t as u64).to_string(), "message_count": 12 + t,
        "member_count": 3 + t, "last_message_id": last_message_id(parent).to_string(),
        "rate_limit_per_user": 0, "flags": 0, "total_message_sent": 12 + t,
        "thread_metadata": { "archived": false, "auto_archive_duration": 4320,
            "archive_timestamp": "2026-10-20T10:00:00.000000+00:00", "locked": false,
            "create_timestamp": "2026-10-19T10:00:00.000000+00:00" },
    })
}

fn guild(g: usize) -> Value {
    let id = guild_id(g);
    let features: Vec<&str> = [
        "COMMUNITY",
        "NEWS",
        "ANIMATED_ICON",
        "BANNER",
        "INVITE_SPLASH",
        "ROLE_ICONS",
        "MEMBER_VERIFICATION_GATE_ENABLED",
        "PREVIEW_ENABLED",
        "WELCOME_SCREEN_ENABLED",
        "THREADS_ENABLED",
        "AUTO_MODERATION",
        "SOUNDBOARD",
        "GUILD_ONBOARDING",
        "VANITY_URL",
    ]
    .into_iter()
    .take(4 + g % 10)
    .collect();
    json!({
        "id": id.to_string(),
        "data_mode": "full",
        "version": 1_700_000_000_000u64 + g as u64,
        "joined_at": "2024-03-10T18:00:00.000000+00:00",
        "lazy": true,
        "large": g.is_multiple_of(3),
        "member_count": 500 + g * 997,
        "premium_subscription_count": g % 30,
        "properties": {
            "id": id.to_string(),
            "name": format!("{} {} #{g}", word(g as u64).to_uppercase(), word(g as u64 + 9)),
            "icon": if g.is_multiple_of(8) { Value::Null } else { json!(hash(id)) },
            "banner": if g.is_multiple_of(3) { json!(hash(id + 1)) } else { Value::Null },
            "splash": Value::Null,
            "discovery_splash": Value::Null,
            "description": format!("A synthetic community about {} and {}.", word(g as u64), word(g as u64 + 4)),
            "owner_id": user_id(g as u64).to_string(),
            "features": features,
            "premium_tier": g % 4,
            "verification_level": 1,
            "explicit_content_filter": 2,
            "default_message_notifications": g % 2,
            "mfa_level": 0,
            "nsfw_level": 0,
            "preferred_locale": "en-US",
            "system_channel_id": channel_id(g, CATEGORIES_PER_GUILD).to_string(),
            "system_channel_flags": 0,
            "rules_channel_id": channel_id(g, CATEGORIES_PER_GUILD + 1).to_string(),
            "public_updates_channel_id": channel_id(g, CATEGORIES_PER_GUILD + 2).to_string(),
            "safety_alerts_channel_id": Value::Null,
            "afk_channel_id": Value::Null,
            "afk_timeout": 300,
            "max_members": 500000,
            "max_video_channel_users": 25,
            "max_stage_video_channel_users": 50,
            "vanity_url_code": Value::Null,
            "premium_progress_bar_enabled": false,
            "hub_type": Value::Null,
            "home_header": Value::Null,
            "latest_onboarding_question_id": Value::Null,
            "incidents_data": Value::Null,
        },
        "channels": (0..CHANNELS_PER_GUILD).map(|c| channel(g, c)).collect::<Vec<_>>(),
        "roles": (0..ROLES_PER_GUILD).map(|r| role(g, r)).collect::<Vec<_>>(),
        "emojis": (0..EMOJIS_PER_GUILD).map(|e| emoji(g, e)).collect::<Vec<_>>(),
        "stickers": (0..STICKERS_PER_GUILD).map(|s| sticker(g, s)).collect::<Vec<_>>(),
        "threads": (0..THREADS_PER_GUILD).map(|t| thread(g, t)).collect::<Vec<_>>(),
        "guild_scheduled_events": [],
        "stage_instances": [],
        "activity_instances": [],
        "soundboard_sounds": [],
    })
}

fn dm(d: usize) -> Value {
    let id = DM_BASE + d as u64;
    let group = d % 10 == 9;
    let recipients: Vec<Value> = if group {
        (0..4).map(|k| user(d as u64 * 5 + k)).collect()
    } else {
        vec![user(d as u64)]
    };
    let mut v = json!({
        "id": id.to_string(),
        "type": if group { 3 } else { 1 },
        "last_message_id": last_message_id(id).to_string(),
        "flags": 0,
        "recipients": recipients,
        "is_spam": false,
        "safety_warnings": [],
    });
    if group {
        v["name"] = json!(format!("{} squad", word(d as u64)));
        v["icon"] = Value::Null;
        v["owner_id"] = json!(user_id(d as u64 * 5).to_string());
    }
    v
}

/// My member object for server `g` (READY `merged_members[g]`).
fn my_member(g: usize) -> Value {
    json!([{
        "user_id": super::ME,
        "roles": [role_id(g, 1 + g % 5).to_string(), role_id(g, 7).to_string()],
        "nick": Value::Null, "avatar": Value::Null, "banner": Value::Null, "bio": "",
        "joined_at": "2024-03-10T18:00:00.000000+00:00", "premium_since": Value::Null,
        "deaf": false, "mute": false, "pending": false, "flags": 0,
        "communication_disabled_until": Value::Null,
    }])
}

/// The READY payload (`{"op":0,"t":"READY","s":1,"d":{...}}`) as one
/// zlib-stream frame. `extra` holds the normal demo's own servers / DMs /
/// read state etc. (as JSON), put first so the demo's channels and scripted
/// events still work.
///
/// The servers (most of the bytes) are written one at a time, so building
/// the frame never holds a whole READY as a `Value`: what's measured is
/// the client's parse, not this generator.
pub fn ready_frame(extra: DemoParts) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let w = &mut enc;
    w.write_all(br#"{"op":0,"t":"READY","s":1,"d":{"guilds":["#)
        .expect("zlib to memory");
    let generated = (0..GUILDS).map(guild);
    for (i, g) in extra.guilds.into_iter().chain(generated).enumerate() {
        if i > 0 {
            w.write_all(b",").expect("zlib to memory");
        }
        serde_json::to_writer(&mut *w, &g).expect("JSON to memory");
    }
    w.write_all(b"],").expect("zlib to memory");
    let rest = serde_json::to_vec(&ready_rest(
        extra.merged_members,
        extra.dms,
        extra.read_state,
        extra.guild_settings,
        extra.relationships,
        extra.users,
    ))
    .expect("JSON to memory");
    // `rest` is `{...}`: its fields go after "guilds", then close `d` and
    // the payload.
    w.write_all(&rest[1..]).expect("zlib to memory");
    w.write_all(b"}").expect("zlib to memory");
    sync_flush(enc)
}

/// READY `d` without `guilds`.
fn ready_rest(
    mut merged_members: Vec<Value>,
    mut dms: Vec<Value>,
    mut read_state: Vec<Value>,
    mut guild_settings: Vec<Value>,
    mut relationships: Vec<Value>,
    mut users: Vec<Value>,
) -> Value {
    merged_members.extend((0..GUILDS).map(my_member));
    dms.extend((0..DMS).map(dm));

    for g in 0..GUILDS {
        for c in CATEGORIES_PER_GUILD..CHANNELS_PER_GUILD {
            let id = channel_id(g, c);
            // Most read, some unread, a few with mentions.
            let behind = if (g + c) % 4 == 0 { 3 } else { 0 };
            read_state.push(json!({
                "id": id.to_string(),
                "last_message_id": (last_message_id(id) - behind).to_string(),
                "mention_count": if (g + c) % 23 == 0 { 2 } else { 0 },
                "last_pin_timestamp": "2026-10-01T12:00:00+00:00",
                "flags": 0, "last_viewed": 3700 + c,
            }));
        }
    }
    for d in 0..DMS {
        let id = DM_BASE + d as u64;
        read_state.push(json!({
            "id": id.to_string(),
            "last_message_id": (last_message_id(id) - u64::from(d % 7 == 0)).to_string(),
            "mention_count": u64::from(d % 7 == 0),
            "last_pin_timestamp": "1970-01-01T00:00:00+00:00", "flags": 0,
        }));
    }

    for g in 0..GUILDS {
        guild_settings.push(json!({
            "guild_id": guild_id(g).to_string(),
            "message_notifications": 1,
            "muted": g % 11 == 0,
            "mute_config": if g % 11 == 0 { json!({ "end_time": Value::Null, "selected_time_window": -1 }) } else { Value::Null },
            "suppress_everyone": g % 2 == 0, "suppress_roles": false, "mobile_push": true,
            "hide_muted_channels": false, "flags": 0, "notify_highlights": 0,
            "mute_scheduled_events": false, "version": 120 + g,
            "channel_overrides": (0..4).map(|k| json!({
                "channel_id": channel_id(g, CATEGORIES_PER_GUILD + k * 3).to_string(),
                "muted": k % 2 == 0, "message_notifications": 3, "collapsed": false,
                "mute_config": Value::Null, "flags": 0,
            })).collect::<Vec<_>>(),
        }));
    }

    for r in 0..RELATIONSHIPS {
        let u = 1000 + r;
        relationships.push(json!({
            "id": user_id(u).to_string(), "user_id": user_id(u).to_string(),
            "type": if r % 25 == 0 { 3 } else { 1 },
            "nickname": Value::Null, "since": "2025-01-02T03:04:05.000000+00:00",
            "is_spam_request": false,
        }));
        users.push(user(u));
    }

    json!({
            "v": 10,
            "user": me(),
            "session_id": "demo-big-session",
            "session_type": "normal",
            "resume_gateway_url": "wss://gateway.invalid",
            "auth_session_id_hash": hash(42),
            "country_code": "GB",
            "geo_ordered_rtc_regions": ["rotterdam", "london", "madrid", "milan", "frankfurt"],
            "merged_members": merged_members,
            "private_channels": dms,
            "read_state": { "entries": read_state, "partial": false, "version": 30_000 },
            "user_guild_settings": { "entries": guild_settings, "partial": false, "version": 900 },
            "relationships": relationships,
            "users": users,
            "connected_accounts": [],
            "consents": { "personalization": { "consented": false } },
            "experiments": experiments(1200),
            "guild_experiments": guild_experiments(1500),
            "sessions": [{ "session_id": "demo-big-session", "status": "online", "activities": [],
                           "client_info": { "version": 0, "os": "windows", "client": "web" } }],
            "tutorial": Value::Null,
            "notification_settings": { "flags": 0 },
            "friend_suggestion_count": 0,
            "analytics_token": hash(7),
    })
}

/// Numeric experiment rows, like READY's `experiments` /
/// `guild_experiments` (a real account gets a lot of them).
fn experiments(n: u64) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                json!([
                    (i * 2_654_435_761) % 4_294_967_295,
                    0,
                    i % 4,
                    -1,
                    0,
                    (i * 77) % 10_000,
                    0,
                    0
                ])
            })
            .collect(),
    )
}

/// READY `guild_experiments`: hashed rollouts with bucket ranges and
/// filters (~1 KB each on a real account).
fn guild_experiments(n: u64) -> Value {
    Value::Array(
        (0..n)
            .map(|i| {
                let ranges: Vec<Value> = (0..8)
                    .map(|b| json!([b, [{ "s": b * 1250, "e": b * 1250 + 1250 }]]))
                    .collect();
                let filters: Vec<Value> = (0..4)
                    .map(|f| {
                        json!([
                            1_604_612_045u64 + f,
                            [[1_183_251_248u64, [hash(i * 4 + f)]]]
                        ])
                    })
                    .collect();
                json!([
                    (i * 2_654_435_761) % 4_294_967_295,
                    Value::Null,
                    i % 9,
                    [[ranges, filters]],
                    [{ "b": 1, "k": [guild_id((i % GUILDS as u64) as usize).to_string()] }],
                    [[[[1, [{ "s": 0, "e": 10_000 }]]], []]],
                    Value::Null, 0, 0
                ])
            })
            .collect(),
    )
}

/// READY_SUPPLEMENTAL: presences and member objects for people online in
/// each server, plus friends' presences. `extra_guilds` are the demo's own
/// server ids (first in READY, so first here too).
pub fn supplemental_payload(extra_guilds: &[&str]) -> Value {
    let presence = |u: u64| {
        let status = ["online", "idle", "dnd", "online"][(u % 4) as usize];
        json!({
            "user_id": user_id(u).to_string(),
            "status": status,
            "client_status": { "desktop": status },
            "activities": if u.is_multiple_of(3) { json!([{
                "type": 0, "name": format!("{} Simulator", word(u)), "id": hash(u),
                "created_at": 1_700_000_000_000u64, "timestamps": { "start": 1_700_000_000_000u64 },
                "application_id": (u * 13).to_string(),
            }]) } else { json!([]) },
        })
    };
    let member = |g: usize, u: u64| {
        json!({
            "user_id": user_id(u).to_string(),
            "roles": [role_id(g, (u as usize) % ROLES_PER_GUILD).to_string()],
            "nick": if u.is_multiple_of(5) { json!(format!("{} nick", word(u))) } else { Value::Null },
            "avatar": Value::Null, "joined_at": "2024-05-01T00:00:00.000000+00:00",
            "premium_since": Value::Null, "deaf": false, "mute": false, "pending": false, "flags": 0,
        })
    };
    let mut guilds = Vec::new();
    let mut members = Vec::new();
    for id in extra_guilds {
        guilds.push(json!({ "id": id, "voice_states": [], "embedded_activities": [] }));
        members.push(json!([]));
    }
    let mut presences = vec![Value::Array(Vec::new()); extra_guilds.len()];
    for g in 0..GUILDS {
        let people: Vec<u64> = (0..PRESENCES_PER_GUILD)
            .map(|k| g as u64 * 31 + k)
            .collect();
        guilds.push(
            json!({ "id": guild_id(g).to_string(), "voice_states": [], "embedded_activities": [] }),
        );
        members.push(Value::Array(people.iter().map(|u| member(g, *u)).collect()));
        presences.push(Value::Array(people.iter().map(|u| presence(*u)).collect()));
    }
    json!({
        "op": 0, "t": "READY_SUPPLEMENTAL", "s": 2,
        "d": {
            "guilds": guilds,
            "merged_members": members,
            "merged_presences": {
                "guilds": presences,
                "friends": (1000..1000 + RELATIONSHIPS).map(presence).collect::<Vec<_>>(),
            },
            "lazy_private_channels": [],
            "disclose": [],
        }
    })
}

/// The demo's own servers, DMs etc. as READY JSON pieces.
#[derive(Default)]
pub struct DemoParts {
    pub guilds: Vec<Value>,
    pub merged_members: Vec<Value>,
    pub dms: Vec<Value>,
    pub read_state: Vec<Value>,
    pub guild_settings: Vec<Value>,
    pub relationships: Vec<Value>,
    pub users: Vec<Value>,
}

/// `payload` as one zlib-stream message (ends in `00 00 FF FF`), like a
/// gateway binary frame.
pub fn compress(payload: &Value) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut enc, payload).expect("JSON to memory");
    sync_flush(enc)
}

fn sync_flush(mut enc: flate2::write::ZlibEncoder<Vec<u8>>) -> Vec<u8> {
    // `flush` is a sync flush: the stream so far ends in 00 00 FF FF.
    enc.flush().expect("zlib sync flush");
    let mut out = std::mem::take(enc.get_mut());
    out.shrink_to_fit();
    out
}

/// History for a generated channel: many authors (avatars), custom emoji,
/// reactions, replies and some image attachments.
pub fn history_values(channel_id: &str) -> Option<Vec<Value>> {
    if !is_big_channel(channel_id) {
        return None;
    }
    let ch: u64 = channel_id.parse().ok()?;
    let g = ((ch.saturating_sub(GUILD_BASE)) / 10_000) as usize % GUILDS;
    let last = last_message_id(ch);
    let first = last - HISTORY + 1;
    let out = (0..HISTORY)
        .map(|n| {
            let id = first + n;
            let author = user(ch % 997 + n * 13);
            let e1 = emoji(g, (n as usize * 3) % EMOJIS_PER_GUILD);
            let e2 = emoji(g, (n as usize * 5 + 1) % EMOJIS_PER_GUILD);
            let a = |e: &Value| if e["animated"].as_bool() == Some(true) { "a" } else { "" };
            let mut v = json!({
                "id": id.to_string(), "channel_id": channel_id, "type": 0,
                "author": author,
                "content": format!(
                    "message {n} about {} <{}:{}:{}> and <{}:{}:{}>",
                    word(n), a(&e1), e1["name"].as_str().unwrap_or(""), e1["id"].as_str().unwrap_or(""),
                    a(&e2), e2["name"].as_str().unwrap_or(""), e2["id"].as_str().unwrap_or("")
                ),
                "timestamp": super::iso_minutes_ago((HISTORY - n) as i64 * 3),
                "edited_timestamp": Value::Null, "tts": false, "mention_everyone": false,
                "mentions": [], "mention_roles": [], "attachments": [], "embeds": [],
                "pinned": false, "flags": 0, "components": [],
            });
            if n % 4 == 0 {
                v["reactions"] = json!([
                    { "emoji": { "id": e1["id"], "name": e1["name"], "animated": e1["animated"] },
                      "count": 1 + n % 5, "count_details": { "burst": 0, "normal": 1 + n % 5 },
                      "me": false, "me_burst": false, "burst_colors": [] },
                    { "emoji": { "id": Value::Null, "name": "👍" }, "count": 2,
                      "count_details": { "burst": 0, "normal": 2 }, "me": n % 8 == 0,
                      "me_burst": false, "burst_colors": [] },
                ]);
            }
            if n % 10 == 5 {
                let url = format!("https://cdn.discordapp.com/attachments/{channel_id}/{id}/photo{n}.png");
                v["attachments"] = json!([{ "id": (id + 7).to_string(), "filename": format!("photo{n}.png"),
                    "size": 240_000, "url": url, "proxy_url": url.replace("cdn.", "media."),
                    "width": 800, "height": 600, "content_type": "image/png" }]);
            }
            if n % 7 == 3 && n > 0 {
                v["referenced_message"] = json!({ "id": (id - 1).to_string(), "author": user(ch % 997 + (n - 1) * 13),
                    "content": format!("message {} about {}", n - 1, word(n - 1)), "channel_id": channel_id });
                v["message_reference"] = json!({ "message_id": (id - 1).to_string(), "channel_id": channel_id });
            }
            v
        })
        .collect();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToUi;

    #[test]
    fn ready_is_big_and_parses_through_the_gateway() {
        let frame = ready_frame(DemoParts::default());
        assert!(frame.ends_with(&[0, 0, 0xFF, 0xFF]));
        let mut inflater = crate::gateway::Inflater::new();
        let json = inflater.push(&frame).unwrap().expect("one whole message");
        // A heavy real account's READY is ~15-20 MB.
        assert!(
            (10 << 20..30 << 20).contains(&json.len()),
            "READY is {} bytes",
            json.len()
        );
        let events = crate::gateway::dispatch_bytes(&json, &mut String::new()).unwrap();
        let Some(ToUi::Ready { guilds, dms, .. }) =
            events.iter().find(|e| matches!(e, ToUi::Ready { .. }))
        else {
            panic!("no Ready");
        };
        assert_eq!(guilds.len(), GUILDS);
        assert_eq!(dms.len(), DMS);
        let channels: usize = guilds.iter().map(|g| g.channels.len()).sum();
        assert_eq!(channels, GUILDS * CHANNELS_PER_GUILD);
        assert!(!guilds[3].name.is_empty() && guilds[3].icon_url.is_some());
        assert!(
            !guilds[3].my_roles.is_empty(),
            "my roles from merged_members"
        );
    }

    #[test]
    fn supplemental_parses() {
        let payload = supplemental_payload(&["100", "101"]);
        let json = serde_json::to_vec(&payload).unwrap();
        let events = crate::gateway::dispatch_bytes(&json, &mut String::new()).unwrap();
        let presences = events.iter().find_map(|e| match e {
            ToUi::Presences(p) => Some(p.len()),
            _ => None,
        });
        assert!(presences.unwrap() >= GUILDS * PRESENCES_PER_GUILD as usize);
        let member_events = events
            .iter()
            .filter(|e| matches!(e, ToUi::Members { .. }))
            .count();
        assert_eq!(member_events, GUILDS);
    }

    #[test]
    fn history_for_generated_channels() {
        let id = channel_id(3, 8).to_string();
        assert!(is_big_channel(&id));
        assert!(!is_big_channel("201"));
        let h = history_values(&id).unwrap();
        assert_eq!(h.len() as u64, HISTORY);
        assert_eq!(
            h.last().unwrap()["id"].as_str().unwrap(),
            last_message_id(channel_id(3, 8)).to_string()
        );
        assert!(history_values("201").is_none());
        let dm = (DM_BASE + 4).to_string();
        assert!(history_values(&dm).is_some());
    }
}
