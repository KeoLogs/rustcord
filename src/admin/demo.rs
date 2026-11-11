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
            if let Some(o) = v.as_object_mut() {
                o.insert("id".into(), json!(new_id()));
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
        _ => Value::Null,
    }
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
