//! Demo mode (`RUSTCORD_DEMO=1`): canned replies for the admin screens so
//! every tab can be seen offline. Stateless: loads return fixed data, and
//! writes echo the request back the way Discord would (the page applies
//! the reply to what it shows), so nothing leaves the machine.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::{Kind, Method, Request};
use crate::perms;

/// The demo user's "Manager" role: every server tool, no Administrator.
pub const MANAGER_PERMISSIONS: u64 = perms::VIEW_CHANNEL
    | perms::MANAGE_GUILD
    | perms::MANAGE_ROLES
    | perms::MANAGE_CHANNELS
    | perms::KICK_MEMBERS
    | perms::BAN_MEMBERS
    | perms::MODERATE_MEMBERS
    | perms::MANAGE_NICKNAMES
    | perms::CHANGE_NICKNAME
    | perms::MANAGE_EXPRESSIONS
    | perms::MANAGE_WEBHOOKS
    | perms::VIEW_AUDIT_LOG
    | perms::CREATE_INSTANT_INVITE
    | perms::MANAGE_MESSAGES;

/// Ids for things "created" in the demo.
static NEXT_ID: AtomicU64 = AtomicU64::new(880_000_000_000_000_000);

fn new_id() -> String {
    NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string()
}

/// The demo backend's answer to an admin request.
pub fn respond(r: &Request) -> Result<Value, String> {
    if r.method == Method::Get {
        return Ok(load(r));
    }
    let mut v = r.body.clone().unwrap_or(Value::Null);
    match r.method {
        Method::Delete => return Ok(Value::Null),
        Method::Post => {
            let id = new_id();
            if let Some(o) = v.as_object_mut() {
                if r.kind == Kind::CreateInvite {
                    // An invite: a code, its channel and no expiry maths.
                    let channel = r.path.split('/').nth(2).unwrap_or("");
                    o.insert("code".into(), json!(format!("demo{}", &id[14..])));
                    o.insert(
                        "channel".into(),
                        json!({ "id": channel, "name": "channel", "type": 0 }),
                    );
                    o.insert("inviter".into(), json!({ "username": "you" }));
                    o.insert("uses".into(), json!(0));
                }
                o.insert("id".into(), json!(id));
            }
        }
        _ => {
            if let Some(o) = v.as_object_mut()
                && !r.key.is_empty()
            {
                o.insert("id".into(), json!(r.key));
            }
        }
    }
    Ok(v)
}

fn load(r: &Request) -> Value {
    match r.kind {
        Kind::LoadGuild => json!({
            "id": r.guild_id, "name": "RustCord Test Server",
            "description": "A place to test RustCord. Be nice.",
            "icon": null, "banner": null,
            "system_channel_id": "200", "afk_channel_id": "230", "afk_timeout": 900,
            "verification_level": 1, "default_message_notifications": 1,
            "explicit_content_filter": 2,
            "rules_channel_id": "200", "public_updates_channel_id": "203",
            "preferred_locale": "en-GB", "features": ["COMMUNITY"],
        }),
        Kind::LoadRoles => json!([
            { "id": r.guild_id, "name": "@everyone", "color": 0, "position": 0,
              "permissions": "104324673" },
            { "id": "500", "name": "Admin", "color": 0xf1c40f, "position": 6, "hoist": true,
              "permissions": "8" },
            { "id": "505", "name": "Manager", "color": 0, "position": 5,
              "permissions": MANAGER_PERMISSIONS.to_string() },
            { "id": "501", "name": "Moderators", "color": 0x3498db, "position": 4, "hoist": true,
              "mentionable": true, "permissions": "8192" },
            { "id": "502", "name": "Bots", "color": 0x9b59b6, "position": 3, "hoist": true,
              "managed": true, "permissions": "0" },
            { "id": "503", "name": "Contributor", "color": 0x2ecc71, "position": 2,
              "permissions": "8192" },
            { "id": "504", "name": "Rustacean", "color": 0xe67e22, "position": 1,
              "permissions": "0" },
        ]),
        Kind::RoleMemberSearch => members(&r.path),
        Kind::LoadChannels => json!([
            { "id": "200", "name": "welcome", "type": 0, "position": 0,
              "topic": "Start here.", "permission_overwrites": [
                  { "id": r.guild_id, "type": 0, "allow": "0", "deny": "2048" }] },
            { "id": "300", "name": "TEXT CHANNELS", "type": 4, "position": 1 },
            { "id": "201", "name": "general", "type": 0, "position": 0, "parent_id": "300",
              "topic": "Chat about anything. Be nice, no spoilers without tags.",
              "rate_limit_per_user": 5, "permission_overwrites": [
                  { "id": "501", "type": 0, "allow": "8192", "deny": "0" },
                  { "id": "950000000000000010", "type": 1, "allow": "0", "deny": "2048" }] },
            { "id": "202", "name": "memes", "type": 0, "position": 1, "parent_id": "300",
              "nsfw": true },
            { "id": "204", "name": "showcase", "type": 0, "position": 2, "parent_id": "300" },
            { "id": "205", "name": "help", "type": 15, "position": 3, "parent_id": "300",
              "topic": "Ask anything about RustCord." },
            { "id": "206", "name": "history", "type": 0, "position": 4, "parent_id": "300" },
            { "id": "301", "name": "Dev", "type": 4, "position": 2 },
            { "id": "203", "name": "rust", "type": 0, "position": 0, "parent_id": "301",
              "topic": "RustCord development" },
            { "id": "302", "name": "Voice Channels", "type": 4, "position": 3 },
            { "id": "230", "name": "General", "type": 2, "position": 0, "parent_id": "302",
              "bitrate": 64000, "user_limit": 0 },
        ]),
        Kind::ChannelMemberSearch => members(&r.path),
        Kind::SearchMembers => members(&r.path),
        Kind::LoadBans => json!([
            { "user": { "id": "970000000000000011", "username": "spam_bot_9000",
                        "global_name": "FREE NITRO" }, "reason": "Spam links" },
            { "user": { "id": "970000000000000012", "username": "raider",
                        "global_name": "Raider" }, "reason": null },
        ]),
        Kind::LoadInvites => json!([
            { "code": "rustcord", "channel": { "id": "200", "name": "welcome", "type": 0 },
              "inviter": { "username": "sam" }, "uses": 42, "max_uses": 0, "max_age": 0,
              "expires_at": null, "temporary": false },
            { "code": "aB3xK9q", "channel": { "id": "201", "name": "general", "type": 0 },
              "inviter": { "username": "alex" }, "uses": 3, "max_uses": 10,
              "max_age": 604800, "expires_at": "2026-12-01T18:00:00+00:00",
              "temporary": false },
        ]),
        Kind::LoadEmojis => json!([
            { "id": "123", "name": "pepe", "animated": false,
              "user": { "id": "300000000000000002", "username": "sam" } },
            { "id": "124", "name": "party_cat", "animated": true,
              "user": { "id": "520000000000000003", "username": "alex" } },
            { "id": "125", "name": "ferris", "animated": false,
              "user": { "id": "460000000000000001", "username": "you" } },
        ]),
        Kind::LoadStickers => json!([
            { "id": "810", "name": "Ferris Wave", "description": "Ferris the crab waving",
              "tags": "wave", "format_type": 1,
              "user": { "id": "300000000000000002", "username": "sam" } },
            { "id": "811", "name": "Ship It", "description": "", "tags": "rocket",
              "format_type": 1, "user": { "id": "460000000000000001", "username": "you" } },
        ]),
        Kind::LoadAudit | Kind::LoadAuditMore => audit(&r.path),
        Kind::LoadRules => json!([
            { "id": "700", "name": "No scam links", "enabled": true, "event_type": 1,
              "trigger_type": 1,
              "trigger_metadata": { "keyword_filter": ["free nitro", "*.scam.example*"],
                                    "allow_list": ["nitro boost"] },
              "actions": [{ "type": 1, "metadata": { "custom_message": "That looks like a scam link." } },
                          { "type": 2, "metadata": { "channel_id": "203" } }],
              "exempt_roles": ["501"], "exempt_channels": [] },
            { "id": "701", "name": "Block suspected spam", "enabled": false, "event_type": 1,
              "trigger_type": 3, "trigger_metadata": {},
              "actions": [{ "type": 1 }], "exempt_roles": [], "exempt_channels": [] },
        ]),
        _ => Value::Null,
    }
}

/// Demo audit log, filtered by the `user_id` / `action_type` in `path`.
fn audit(path: &str) -> Value {
    let param = |name: &str| {
        path.split_once(&format!("{name}="))
            .map(|(_, v)| v.split('&').next().unwrap_or("").to_string())
    };
    if param("before").is_some() {
        return json!({ "audit_log_entries": [] });
    }
    let (sam, alex, you, troll) = (
        "300000000000000002",
        "520000000000000003",
        "460000000000000001",
        "950000000000000010",
    );
    // Ids are snowflakes a few hours apart.
    let all = [
        json!({ "id": "1437000000000000000", "user_id": alex, "target_id": troll,
                "action_type": 22, "reason": "Spam after two warnings" }),
        json!({ "id": "1436900000000000000", "user_id": sam, "target_id": "201",
                "action_type": 11, "changes": [
                    { "key": "rate_limit_per_user", "old_value": 0, "new_value": 5 },
                    { "key": "topic", "old_value": "Chat",
                      "new_value": "Chat about anything. Be nice, no spoilers without tags." }] }),
        json!({ "id": "1436800000000000000", "user_id": you, "target_id": "504",
                "action_type": 30, "changes": [
                    { "key": "name", "new_value": "Rustacean" },
                    { "key": "color", "new_value": 15105570 }] }),
        json!({ "id": "1436700000000000000", "user_id": sam, "target_id": you,
                "action_type": 25, "changes": [
                    { "key": "$add", "new_value": [{ "id": "505", "name": "Manager" }] }] }),
        json!({ "id": "1436600000000000000", "user_id": alex, "target_id": "0",
                "action_type": 42, "changes": [{ "key": "code", "old_value": "oldInvite" }] }),
        json!({ "id": "1436500000000000000", "user_id": sam, "target_id": "100",
                "action_type": 1, "changes": [
                    { "key": "verification_level", "old_value": 0, "new_value": 1 }] }),
    ];
    let entries: Vec<Value> = all
        .into_iter()
        .filter(|e| param("user_id").is_none_or(|u| e["user_id"] == u.as_str()))
        .filter(|e| param("action_type").is_none_or(|a| e["action_type"].to_string() == a))
        .collect();
    json!({
        "audit_log_entries": entries,
        "users": [
            { "id": sam, "username": "sam", "global_name": "Sam" },
            { "id": alex, "username": "alex", "global_name": "Alex" },
            { "id": you, "username": "you", "global_name": "You" },
            { "id": troll, "username": "troll", "global_name": "xX_Troll_Xx" },
        ],
    })
}

/// Demo members; `path` may carry a `query=` to filter by.
pub fn members(path: &str) -> Value {
    let query = path
        .split_once("query=")
        .map(|(_, q)| q.split('&').next().unwrap_or("").replace("%20", " "))
        .unwrap_or_default()
        .to_lowercase();
    let all = [
        ("460000000000000001", "you", "You", "", vec!["503", "505"]),
        (
            "300000000000000002",
            "sam",
            "Sam",
            "Sam (owner)",
            vec!["500"],
        ),
        ("520000000000000003", "alex", "Alex", "", vec!["501", "503"]),
        ("610000000000000004", "maya", "Maya", "", vec!["503", "504"]),
        ("390000000000000007", "olly", "Olly", "", vec!["503"]),
        ("950000000000000010", "troll", "xX_Troll_Xx", "", vec![]),
        (
            "700000000000000020",
            "helper",
            "Helper Bot",
            "",
            vec!["502"],
        ),
    ];
    Value::Array(
        all.iter()
            .filter(|(_, user, global, nick, _)| {
                query.is_empty()
                    || user.contains(&query)
                    || global.to_lowercase().contains(&query)
                    || nick.to_lowercase().contains(&query)
            })
            .map(|(id, user, global, nick, roles)| {
                json!({
                    "user": { "id": id, "username": user, "global_name": global,
                              "bot": *user == "helper" },
                    "nick": if nick.is_empty() { Value::Null } else { json!(nick) },
                    "roles": roles,
                    "joined_at": "2025-03-14T09:30:00+00:00",
                    "communication_disabled_until": Value::Null,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_echo_with_an_id() {
        let post = Request::new("100", Kind::SaveGuild, Method::Post, "/x".into())
            .body(json!({ "name": "a" }));
        let v = respond(&post).unwrap();
        assert_eq!(v["name"], "a");
        assert!(v["id"].is_string());
        let patch = Request::new("100", Kind::SaveGuild, Method::Patch, "/x".into())
            .key("7")
            .body(json!({ "name": "b" }));
        assert_eq!(respond(&patch).unwrap(), json!({ "id": "7", "name": "b" }));
        let del = Request::new("100", Kind::SaveGuild, Method::Delete, "/x".into());
        assert_eq!(respond(&del).unwrap(), Value::Null);
    }
}
