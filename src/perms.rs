//! Discord permission bits: just enough to know whether I may pin / unpin
//! messages in a channel (Manage Messages).
//!
//! Computed the way Discord documents it: the owner has everything; base =
//! @everyone role | my roles; Administrator grants everything; then the
//! channel's overwrites apply in order @everyone, my roles (all denies, then
//! all allows), me.

use serde_json::Value;

use crate::members::Role;

pub const ADMINISTRATOR: u64 = 1 << 3;
pub const MANAGE_MESSAGES: u64 = 1 << 13;

/// Overwrite target: a role or a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Role,
    Member,
}

/// One channel permission overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    pub id: String,
    pub target: Target,
    pub allow: u64,
    pub deny: u64,
}

/// Permission bits are sent as decimal strings (older payloads: numbers).
pub fn parse_bits(v: &Value) -> u64 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64())
        .unwrap_or(0)
}

impl Overwrite {
    pub fn parse_list(v: &Value) -> Vec<Overwrite> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| {
                        Some(Overwrite {
                            id: o["id"].as_str()?.to_string(),
                            // 0 = role, 1 = member ("role"/"member" in old payloads).
                            target: match &o["type"] {
                                Value::Number(n) if n.as_u64() == Some(1) => Target::Member,
                                Value::String(s) if s == "member" => Target::Member,
                                _ => Target::Role,
                            },
                            allow: parse_bits(&o["allow"]),
                            deny: parse_bits(&o["deny"]),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// What I'm computing permissions for.
pub struct Me<'a> {
    pub user_id: &'a str,
    pub roles: &'a [String],
}

/// My permissions in a channel of a server.
pub fn channel_permissions(
    guild_id: &str,
    owner_id: &str,
    roles: &[Role],
    overwrites: &[Overwrite],
    me: &Me,
) -> u64 {
    if !owner_id.is_empty() && owner_id == me.user_id {
        return u64::MAX;
    }
    // @everyone's role id is the server id.
    let mut base = roles
        .iter()
        .find(|r| r.id == guild_id)
        .map_or(0, |r| r.permissions);
    for r in roles.iter().filter(|r| me.roles.contains(&r.id)) {
        base |= r.permissions;
    }
    if base & ADMINISTRATOR != 0 {
        return u64::MAX;
    }
    let mut p = base;
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.target == Target::Role && o.id == guild_id)
    {
        p = (p & !o.deny) | o.allow;
    }
    let (mut allow, mut deny) = (0, 0);
    for o in overwrites
        .iter()
        .filter(|o| o.target == Target::Role && me.roles.contains(&o.id))
    {
        allow |= o.allow;
        deny |= o.deny;
    }
    p = (p & !deny) | allow;
    if let Some(o) = overwrites
        .iter()
        .find(|o| o.target == Target::Member && o.id == me.user_id)
    {
        p = (p & !o.deny) | o.allow;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn role(id: &str, perms: u64) -> Role {
        Role::from_json(&json!({ "id": id, "name": id, "permissions": perms.to_string() }))
    }

    #[test]
    fn manage_messages_from_roles_and_overwrites() {
        let roles = vec![
            role("1", 0),
            role("mod", MANAGE_MESSAGES),
            role("admin", ADMINISTRATOR),
        ];
        let mine = vec!["mod".to_string()];
        let me = Me {
            user_id: "u",
            roles: &mine,
        };
        let p = channel_permissions("1", "owner", &roles, &[], &me);
        assert_ne!(p & MANAGE_MESSAGES, 0);

        // A role overwrite denying it in this channel.
        let ow = Overwrite::parse_list(&json!([
            { "id": "mod", "type": 0, "allow": "0", "deny": MANAGE_MESSAGES.to_string() }
        ]));
        assert_eq!(
            channel_permissions("1", "owner", &roles, &ow, &me) & MANAGE_MESSAGES,
            0
        );

        // ...but a member overwrite allows it again.
        let ow = Overwrite::parse_list(&json!([
            { "id": "mod", "type": 0, "allow": "0", "deny": MANAGE_MESSAGES.to_string() },
            { "id": "u", "type": 1, "allow": MANAGE_MESSAGES.to_string(), "deny": "0" }
        ]));
        assert_ne!(
            channel_permissions("1", "owner", &roles, &ow, &me) & MANAGE_MESSAGES,
            0
        );

        // Nobody special.
        let none = Me {
            user_id: "u",
            roles: &[],
        };
        assert_eq!(
            channel_permissions("1", "owner", &roles, &[], &none) & MANAGE_MESSAGES,
            0
        );
        // Owner and administrators have everything.
        let owner = Me {
            user_id: "owner",
            roles: &[],
        };
        assert_eq!(
            channel_permissions("1", "owner", &roles, &ow, &owner),
            u64::MAX
        );
        let admin_roles = vec!["admin".to_string()];
        let admin = Me {
            user_id: "a",
            roles: &admin_roles,
        };
        assert_eq!(
            channel_permissions("1", "owner", &roles, &ow, &admin),
            u64::MAX
        );
    }

    #[test]
    fn everyone_overwrite() {
        let roles = vec![role("1", MANAGE_MESSAGES)];
        let me = Me {
            user_id: "u",
            roles: &[],
        };
        let ow = Overwrite::parse_list(&json!([
            { "id": "1", "type": "role", "allow": "0", "deny": MANAGE_MESSAGES }
        ]));
        assert_eq!(
            channel_permissions("1", "", &roles, &ow, &me) & MANAGE_MESSAGES,
            0
        );
    }
}
