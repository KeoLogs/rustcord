//! Discord permission bits: whether I may see a channel (View Channel),
//! pin / unpin (Manage Messages), react or start threads.
//!
//! Computed the way Discord documents it: the owner has everything; base =
//! @everyone role | my roles; Administrator grants everything; then the
//! channel's overwrites apply in order @everyone, my roles (all denies, then
//! all allows), me.

use serde_json::Value;

use crate::members::Role;

pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const MANAGE_CHANNELS: u64 = 1 << 4;
pub const MANAGE_GUILD: u64 = 1 << 5;
pub const VIEW_AUDIT_LOG: u64 = 1 << 7;
pub const CHANGE_NICKNAME: u64 = 1 << 26;
pub const MANAGE_NICKNAMES: u64 = 1 << 27;
pub const MANAGE_ROLES: u64 = 1 << 28;
pub const MANAGE_WEBHOOKS: u64 = 1 << 29;
pub const MANAGE_EXPRESSIONS: u64 = 1 << 30;
pub const MODERATE_MEMBERS: u64 = 1 << 40;

/// One row of the permission grid (role editor, channel overwrites).
pub struct PermInfo {
    pub bit: u64,
    pub name: &'static str,
    pub desc: &'static str,
    pub group: &'static str,
    /// Also offered as a channel overwrite.
    pub channel: bool,
}

const fn perm(
    bit: u32,
    name: &'static str,
    desc: &'static str,
    group: &'static str,
    channel: bool,
) -> PermInfo {
    PermInfo {
        bit: 1 << bit,
        name,
        desc,
        group,
        channel,
    }
}

/// Every permission Discord shows in the role editor, in its order.
#[rustfmt::skip]
pub const PERMISSIONS: &[PermInfo] = &[
    perm(10, "View Channels", "Lets members see channels (and read their messages) by default.", "General", true),
    perm(4, "Manage Channels", "Create, edit and delete channels.", "General", true),
    perm(28, "Manage Roles", "Create, edit and delete roles lower than their highest role, and change channel permissions.", "General", true),
    perm(43, "Create Expressions", "Add custom emoji, stickers and sounds.", "General", false),
    perm(30, "Manage Expressions", "Edit or remove custom emoji, stickers and sounds.", "General", false),
    perm(7, "View Audit Log", "See a record of who made which changes in this server.", "General", false),
    perm(29, "Manage Webhooks", "Create, edit and delete webhooks, which can post messages from other apps.", "General", true),
    perm(5, "Manage Server", "Change the server's name, view all invites and add apps.", "General", false),
    perm(0, "Create Invite", "Invite new people to this server.", "Membership", true),
    perm(26, "Change Nickname", "Change their own nickname.", "Membership", false),
    perm(27, "Manage Nicknames", "Change other members' nicknames.", "Membership", false),
    perm(1, "Kick Members", "Remove members; they can rejoin with a new invite.", "Membership", false),
    perm(2, "Ban Members", "Permanently ban members and delete their message history.", "Membership", false),
    perm(40, "Time Out Members", "Stop members from sending messages, reacting and speaking for a while.", "Membership", false),
    perm(11, "Send Messages", "Send messages in text channels.", "Text", true),
    perm(38, "Send Messages in Threads", "Send messages in threads.", "Text", true),
    perm(35, "Create Public Threads", "Create threads everyone in the channel can view.", "Text", true),
    perm(36, "Create Private Threads", "Create invite-only threads.", "Text", true),
    perm(14, "Embed Links", "Links they share show embedded content.", "Text", true),
    perm(15, "Attach Files", "Upload files or media.", "Text", true),
    perm(6, "Add Reactions", "Add new emoji reactions to a message.", "Text", true),
    perm(18, "Use External Emoji", "Use emoji from other servers.", "Text", true),
    perm(37, "Use External Stickers", "Use stickers from other servers.", "Text", true),
    perm(17, "Mention @everyone, @here and All Roles", "Notify everyone, or every member of any role.", "Text", true),
    perm(13, "Manage Messages", "Delete other members' messages and pin any message.", "Text", true),
    perm(34, "Manage Threads", "Rename, delete, close and lock threads.", "Text", true),
    perm(16, "Read Message History", "Read messages sent before they opened the channel.", "Text", true),
    perm(12, "Send Text-to-Speech Messages", "Send /tts messages, read aloud to everyone in the channel.", "Text", true),
    perm(46, "Send Voice Messages", "Send voice messages.", "Text", true),
    perm(49, "Create Polls", "Create polls.", "Text", true),
    perm(20, "Connect", "Join voice channels and hear others.", "Voice", true),
    perm(21, "Speak", "Talk in voice channels.", "Voice", true),
    perm(9, "Video", "Share video, screen share or stream a game.", "Voice", true),
    perm(42, "Use Soundboard", "Play sounds from the server soundboard.", "Voice", true),
    perm(45, "Use External Sounds", "Use sounds from other servers.", "Voice", true),
    perm(25, "Use Voice Activity", "Speak without push-to-talk.", "Voice", true),
    perm(8, "Priority Speaker", "Be heard more easily; others are quieter while they speak.", "Voice", true),
    perm(22, "Mute Members", "Mute other members in voice channels for everyone.", "Voice", true),
    perm(23, "Deafen Members", "Deafen other members in voice channels.", "Voice", true),
    perm(24, "Move Members", "Disconnect or move other members between voice channels.", "Voice", true),
    perm(48, "Set Voice Channel Status", "Create and edit a voice channel's status.", "Voice", true),
    perm(31, "Use Application Commands", "Use commands from apps, including slash commands.", "Apps", true),
    perm(39, "Use Activities", "Use Activities in voice channels.", "Apps", true),
    perm(50, "Use External Apps", "Apps installed to their account can post public replies.", "Apps", true),
    perm(32, "Request to Speak", "Ask to speak in Stage channels.", "Stage", true),
    perm(44, "Create Events", "Create scheduled events.", "Events", true),
    perm(33, "Manage Events", "Edit and cancel scheduled events.", "Events", true),
    perm(19, "View Server Insights", "See server insights: growth, engagement and more.", "Advanced", false),
    perm(3, "Administrator", "Every permission, and bypasses channel-specific permissions. Dangerous to grant.", "Advanced", false),
];

/// My server-wide permissions (roles only, no channel overwrites).
pub fn guild_permissions(guild_id: &str, owner_id: &str, roles: &[Role], me: &Me) -> u64 {
    channel_permissions(guild_id, owner_id, roles, &[], me)
}

/// Position of the highest role among `mine` (0 = only @everyone).
pub fn top_role_position(roles: &[Role], mine: &[String]) -> i64 {
    roles
        .iter()
        .filter(|r| mine.contains(&r.id))
        .map(|r| r.position)
        .max()
        .unwrap_or(0)
}

/// Role hierarchy: the owner manages every role, everyone else only roles
/// strictly below their own highest role.
pub fn outranks(is_owner: bool, my_top: i64, target_position: i64) -> bool {
    is_owner || my_top > target_position
}

/// May I act on (kick, ban, time out, rename, change the roles of) a
/// member? Never on the owner or on myself, and only on members whose
/// highest role is below mine.
pub fn can_moderate(
    is_owner: bool,
    my_top: i64,
    me_id: &str,
    owner_id: &str,
    target_id: &str,
    target_top: i64,
) -> bool {
    target_id != me_id && target_id != owner_id && outranks(is_owner, my_top, target_top)
}

pub const ADMINISTRATOR: u64 = 1 << 3;
pub const ADD_REACTIONS: u64 = 1 << 6;
pub const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
pub const MANAGE_MESSAGES: u64 = 1 << 13;
pub const VIEW_CHANNEL: u64 = 1 << 10;

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

/// May I see this channel of a server (VIEW_CHANNEL after roles and
/// overwrites)? When the server's roles aren't known yet (no @everyone
/// role) the answer is "yes", so a channel is never hidden on missing data.
pub fn can_view_channel(
    guild_id: &str,
    owner_id: &str,
    roles: &[Role],
    overwrites: &[Overwrite],
    me: &Me,
) -> bool {
    if !roles.iter().any(|r| r.id == guild_id) {
        return true;
    }
    channel_permissions(guild_id, owner_id, roles, overwrites, me) & VIEW_CHANNEL != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn role(id: &str, perms: u64) -> Role {
        Role::from_json(&json!({ "id": id, "name": id, "permissions": perms.to_string() }))
    }

    fn ow4(id: &str, target: Target, allow: u64, deny: u64) -> Overwrite {
        Overwrite {
            id: id.into(),
            target,
            allow,
            deny,
        }
    }

    /// Discord's documented order: @everyone overwrite, then all role
    /// denies, then all role allows, then the member overwrite.
    #[test]
    fn overwrites_apply_in_discord_order() {
        let mut everyone = role("1", VIEW_CHANNEL | MANAGE_CHANNELS);
        everyone.position = 0;
        let roles = vec![everyone, role("a", 0), role("b", MANAGE_WEBHOOKS)];
        let mine = vec!["a".to_string(), "b".to_string()];
        let me = Me {
            user_id: "u",
            roles: &mine,
        };
        let calc = |o: &[Overwrite]| channel_permissions("1", "owner", &roles, o, &me);
        // @everyone deny removes a base permission.
        let p = calc(&[ow4("1", Target::Role, 0, MANAGE_CHANNELS)]);
        assert_eq!(p & MANAGE_CHANNELS, 0);
        assert_ne!(p & VIEW_CHANNEL, 0);
        // A role allow beats the @everyone deny...
        let p = calc(&[
            ow4("1", Target::Role, 0, MANAGE_CHANNELS),
            ow4("a", Target::Role, MANAGE_CHANNELS, 0),
        ]);
        assert_ne!(p & MANAGE_CHANNELS, 0);
        // ...and beats another of my roles' deny (allows apply after denies).
        let p = calc(&[
            ow4("a", Target::Role, MANAGE_WEBHOOKS, 0),
            ow4("b", Target::Role, 0, MANAGE_WEBHOOKS),
        ]);
        assert_ne!(p & MANAGE_WEBHOOKS, 0);
        // The member overwrite has the last word, both ways.
        let p = calc(&[
            ow4("a", Target::Role, MANAGE_CHANNELS, 0),
            ow4("u", Target::Member, MANAGE_ROLES, MANAGE_CHANNELS),
        ]);
        assert_eq!(p & MANAGE_CHANNELS, 0);
        assert_ne!(p & MANAGE_ROLES, 0);
        // Overwrites for roles I don't have, or other members, do nothing.
        let p = calc(&[
            ow4("zzz", Target::Role, MANAGE_ROLES, VIEW_CHANNEL),
            ow4("other", Target::Member, MANAGE_ROLES, VIEW_CHANNEL),
        ]);
        assert_eq!(p & MANAGE_ROLES, 0);
        assert_ne!(p & VIEW_CHANNEL, 0);
    }

    #[test]
    fn admin_and_owner_ignore_overwrites() {
        let roles = vec![role("1", 0), role("adm", ADMINISTRATOR)];
        let deny_all = [ow4("1", Target::Role, 0, u64::MAX)];
        let mine = vec!["adm".to_string()];
        let admin = Me {
            user_id: "u",
            roles: &mine,
        };
        assert_eq!(
            channel_permissions("1", "owner", &roles, &deny_all, &admin),
            u64::MAX
        );
        let owner = Me {
            user_id: "owner",
            roles: &[],
        };
        assert_eq!(
            channel_permissions("1", "owner", &roles, &deny_all, &owner),
            u64::MAX
        );
        assert_eq!(guild_permissions("1", "owner", &roles, &owner), u64::MAX);
    }

    #[test]
    fn role_hierarchy() {
        let mut roles = vec![role("1", 0), role("low", 0), role("mid", 0), role("top", 0)];
        for (i, r) in roles.iter_mut().enumerate() {
            r.position = i as i64;
        }
        let mine = vec!["mid".to_string(), "low".to_string()];
        let top = top_role_position(&roles, &mine);
        assert_eq!(top, 2);
        assert_eq!(top_role_position(&roles, &[]), 0);
        // Only roles strictly below my highest one.
        assert!(outranks(false, top, 1));
        assert!(!outranks(false, top, 2));
        assert!(!outranks(false, top, 3));
        assert!(outranks(true, 0, 3));
        // Members: never myself or the owner, and only lower members.
        assert!(can_moderate(false, top, "me", "owner", "x", 1));
        assert!(!can_moderate(false, top, "me", "owner", "x", 2));
        assert!(!can_moderate(false, top, "me", "owner", "me", 0));
        assert!(!can_moderate(false, top, "me", "owner", "owner", 0));
        assert!(!can_moderate(true, top, "me", "me", "me", 0));
        assert!(can_moderate(true, 0, "me", "me", "x", 99));
    }

    #[test]
    fn permission_grid_has_no_duplicate_bits() {
        let mut seen = 0u64;
        for p in PERMISSIONS {
            assert_eq!(seen & p.bit, 0, "{} listed twice", p.name);
            assert!(!p.desc.is_empty());
            seen |= p.bit;
        }
        for bit in [MANAGE_GUILD, MANAGE_ROLES, KICK_MEMBERS, MODERATE_MEMBERS] {
            assert_ne!(seen & bit, 0);
        }
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

    fn ow(v: serde_json::Value) -> Vec<Overwrite> {
        Overwrite::parse_list(&v)
    }

    #[test]
    fn view_channel_from_roles_and_overwrites() {
        let roles = vec![role("g", VIEW_CHANNEL), role("staff", 0), role("muted", 0)];
        let staff = vec!["staff".to_string()];
        let me = Me {
            user_id: "u",
            roles: &staff,
        };
        let nobody = Me {
            user_id: "x",
            roles: &[],
        };
        // Visible by default (@everyone has VIEW_CHANNEL).
        assert!(can_view_channel("g", "o", &roles, &[], &me));
        // Staff-only channel: @everyone denied, staff role allowed.
        let staff_only = ow(json!([
            { "id": "g", "type": 0, "allow": "0", "deny": VIEW_CHANNEL.to_string() },
            { "id": "staff", "type": 0, "allow": VIEW_CHANNEL.to_string(), "deny": "0" }
        ]));
        assert!(can_view_channel("g", "o", &roles, &staff_only, &me));
        assert!(!can_view_channel("g", "o", &roles, &staff_only, &nobody));
        // A member overwrite beats role allows.
        let banned_me = ow(json!([
            { "id": "staff", "type": 0, "allow": VIEW_CHANNEL.to_string(), "deny": "0" },
            { "id": "u", "type": 1, "allow": "0", "deny": VIEW_CHANNEL.to_string() }
        ]));
        assert!(!can_view_channel("g", "o", &roles, &banned_me, &me));
        // ...and a member allow beats an @everyone deny.
        let just_me = ow(json!([
            { "id": "g", "type": 0, "allow": "0", "deny": VIEW_CHANNEL.to_string() },
            { "id": "x", "type": 1, "allow": VIEW_CHANNEL.to_string(), "deny": "0" }
        ]));
        assert!(can_view_channel("g", "o", &roles, &just_me, &nobody));
        // Role allow wins over another role's deny (allows apply after denies).
        let mixed = ow(json!([
            { "id": "muted", "type": 0, "allow": "0", "deny": VIEW_CHANNEL.to_string() },
            { "id": "staff", "type": 0, "allow": VIEW_CHANNEL.to_string(), "deny": "0" }
        ]));
        let both = vec!["staff".to_string(), "muted".to_string()];
        let me2 = Me {
            user_id: "u",
            roles: &both,
        };
        assert!(can_view_channel("g", "o", &roles, &mixed, &me2));
    }

    #[test]
    fn view_channel_owner_admin_and_missing_data() {
        let roles = vec![role("g", 0), role("admin", ADMINISTRATOR)];
        let hidden = ow(json!([
            { "id": "g", "type": 0, "allow": "0", "deny": VIEW_CHANNEL.to_string() }
        ]));
        let plain = Me {
            user_id: "u",
            roles: &[],
        };
        // @everyone lacks VIEW_CHANNEL entirely: hidden.
        assert!(!can_view_channel("g", "o", &roles, &[], &plain));
        // The owner and administrators see everything.
        let owner = Me {
            user_id: "o",
            roles: &[],
        };
        assert!(can_view_channel("g", "o", &roles, &hidden, &owner));
        let admins = vec!["admin".to_string()];
        let admin = Me {
            user_id: "a",
            roles: &admins,
        };
        assert!(can_view_channel("g", "o", &roles, &hidden, &admin));
        // Roles not loaded yet (no @everyone): never hide on missing data.
        assert!(can_view_channel("g", "o", &[], &hidden, &plain));
        assert!(can_view_channel(
            "g",
            "o",
            &[role("other", 0)],
            &hidden,
            &plain
        ));
    }
}
