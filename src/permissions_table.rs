//! Every Discord permission bit, with the names and descriptions Discord's
//! role editor shows, grouped the way the editor groups them; plus the role
//! hierarchy rules the server-admin pages use to decide what I may touch.
//!
//! Values are from Discord's developer docs ("Bitwise Permission Flags",
//! `topics/permissions`). Bit 47 is not documented and is left out.
//!
//! Hierarchy (same docs, "Permission Hierarchy"):
//! - the owner may do everything;
//! - roles can be edited, reordered, given and taken only if they are below
//!   my highest role (and I have Manage Roles);
//! - a role can only be given permissions I have myself (unless I'm the
//!   owner or an Administrator);
//! - kick / ban / nickname / timeout work only on members whose highest
//!   role is below mine (never the owner, never myself), and Administrators
//!   can't be timed out.
//!
//! "Below" compares `position`; equal positions are broken by id like
//! `members::sort_roles` (the older role, i.e. smaller id, ranks higher).

use std::cmp::Ordering;

use crate::members::Role;

pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const ADMINISTRATOR: u64 = 1 << 3;
pub const MANAGE_CHANNELS: u64 = 1 << 4;
pub const MANAGE_GUILD: u64 = 1 << 5;
pub const VIEW_AUDIT_LOG: u64 = 1 << 7;
pub const CHANGE_NICKNAME: u64 = 1 << 26;
pub const MANAGE_NICKNAMES: u64 = 1 << 27;
pub const MANAGE_ROLES: u64 = 1 << 28;
pub const MANAGE_GUILD_EXPRESSIONS: u64 = 1 << 30;
pub const CREATE_GUILD_EXPRESSIONS: u64 = 1 << 43;
pub const MODERATE_MEMBERS: u64 = 1 << 40;

/// The sections of Discord's role permission editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermGroup {
    General,
    Membership,
    Text,
    Voice,
    Advanced,
}

impl PermGroup {
    pub const ALL: [PermGroup; 5] = [
        PermGroup::General,
        PermGroup::Membership,
        PermGroup::Text,
        PermGroup::Voice,
        PermGroup::Advanced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PermGroup::General => "General Server Permissions",
            PermGroup::Membership => "Membership Permissions",
            PermGroup::Text => "Text Channel Permissions",
            PermGroup::Voice => "Voice Channel Permissions",
            PermGroup::Advanced => "Advanced Permissions",
        }
    }
}

/// One permission bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermInfo {
    /// Bit number (`1 << bit`).
    pub bit: u8,
    /// API name, e.g. `KICK_MEMBERS`.
    pub key: &'static str,
    /// What Discord's editor calls it.
    pub name: &'static str,
    pub description: &'static str,
    pub group: PermGroup,
}

impl PermInfo {
    pub const fn value(&self) -> u64 {
        1 << self.bit
    }

    /// Case-insensitive match on name, API name or description.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.name.to_lowercase().contains(&q)
            || self.key.to_lowercase().replace('_', " ").contains(&q)
            || self.key.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
    }
}

const fn p(
    bit: u8,
    key: &'static str,
    name: &'static str,
    description: &'static str,
    group: PermGroup,
) -> PermInfo {
    PermInfo {
        bit,
        key,
        name,
        description,
        group,
    }
}

use PermGroup::{Advanced, General, Membership, Text, Voice};

/// Every documented permission, in the order Discord's editor lists them.
pub const PERMISSIONS: &[PermInfo] = &[
    // General
    p(10, "VIEW_CHANNEL", "View Channels",
      "Allows members to view channels by default (excluding private channels).", General),
    p(4, "MANAGE_CHANNELS", "Manage Channels",
      "Allows members to create, edit, or delete channels.", General),
    p(28, "MANAGE_ROLES", "Manage Roles",
      "Allows members to create new roles and edit or delete roles lower than their highest role. Also allows changing permissions of individual channels.", General),
    p(43, "CREATE_GUILD_EXPRESSIONS", "Create Expressions",
      "Allows members to add custom emoji, stickers and sounds, and edit or remove the ones they added.", General),
    p(30, "MANAGE_GUILD_EXPRESSIONS", "Manage Expressions",
      "Allows members to edit or remove custom emoji, stickers and sounds added by anyone.", General),
    p(7, "VIEW_AUDIT_LOG", "View Audit Log",
      "Allows members to view a record of who made which changes in this server.", General),
    p(19, "VIEW_GUILD_INSIGHTS", "View Server Insights",
      "Allows members to view Server Insights, which shows data on community growth, engagement and more.", General),
    p(41, "VIEW_CREATOR_MONETIZATION_ANALYTICS", "View Server Subscription Insights",
      "Allows members to view analytics for role subscriptions.", General),
    p(29, "MANAGE_WEBHOOKS", "Manage Webhooks",
      "Allows members to create, edit, or delete webhooks, which can post messages from other apps or sites into this server.", General),
    p(5, "MANAGE_GUILD", "Manage Server",
      "Allows members to change this server's name, switch regions, view all invites, add apps and create and update AutoMod rules.", General),
    p(44, "CREATE_EVENTS", "Create Events",
      "Allows members to create events, and edit and cancel the ones they created.", General),
    p(33, "MANAGE_EVENTS", "Manage Events",
      "Allows members to edit and cancel events created by anyone.", General),
    // Membership
    p(0, "CREATE_INSTANT_INVITE", "Create Invite",
      "Allows members to invite new people to this server.", Membership),
    p(26, "CHANGE_NICKNAME", "Change Nickname",
      "Allows members to change their own nickname, a custom name for just this server.", Membership),
    p(27, "MANAGE_NICKNAMES", "Manage Nicknames",
      "Allows members to change the nicknames of other members.", Membership),
    p(1, "KICK_MEMBERS", "Kick, Approve, and Reject Members",
      "Kick removes other members from this server; they can rejoin if they get another invite. Also allows approving or rejecting join requests.", Membership),
    p(2, "BAN_MEMBERS", "Ban Members",
      "Allows members to permanently ban and delete the message history of other members from this server.", Membership),
    p(40, "MODERATE_MEMBERS", "Time out members",
      "When you put a user in time out they can't send messages, reply, react or speak in voice or stage channels.", Membership),
    // Text
    p(11, "SEND_MESSAGES", "Send Messages and Create Posts",
      "Allows members to send messages in text channels and create posts in forum channels.", Text),
    p(38, "SEND_MESSAGES_IN_THREADS", "Send Messages in Threads and Posts",
      "Allows members to send messages in threads and in posts on forum channels.", Text),
    p(35, "CREATE_PUBLIC_THREADS", "Create Public Threads",
      "Allows members to create threads that everyone in a channel can view.", Text),
    p(36, "CREATE_PRIVATE_THREADS", "Create Private Threads",
      "Allows members to create invite-only threads.", Text),
    p(14, "EMBED_LINKS", "Embed Links",
      "Allows links that members share to show embedded content in text channels.", Text),
    p(15, "ATTACH_FILES", "Attach Files",
      "Allows members to upload files or media in text channels.", Text),
    p(6, "ADD_REACTIONS", "Add Reactions",
      "Allows members to add new emoji reactions to a message.", Text),
    p(18, "USE_EXTERNAL_EMOJIS", "Use External Emoji",
      "Allows members to use emoji from other servers, if they're a Discord Nitro member.", Text),
    p(37, "USE_EXTERNAL_STICKERS", "Use External Stickers",
      "Allows members to use stickers from other servers, if they're a Discord Nitro member.", Text),
    p(17, "MENTION_EVERYONE", "Mention @everyone, @here, and All Roles",
      "Allows members to use @everyone or @here, and to @mention all roles, even if the role's \"Allow anyone to mention this role\" setting is off.", Text),
    p(13, "MANAGE_MESSAGES", "Manage Messages",
      "Allows members to delete messages by other members.", Text),
    p(51, "PIN_MESSAGES", "Pin Messages",
      "Allows members to pin and unpin any message.", Text),
    p(52, "BYPASS_SLOWMODE", "Bypass Slowmode",
      "Allows members to send messages without being affected by slowmode.", Text),
    p(34, "MANAGE_THREADS", "Manage Threads and Posts",
      "Allows members to rename, delete, close and turn on slowmode for threads and posts, and to view private threads.", Text),
    p(16, "READ_MESSAGE_HISTORY", "Read Message History",
      "Allows members to read previous messages sent in channels.", Text),
    p(12, "SEND_TTS_MESSAGES", "Send Text-to-Speech Messages",
      "Allows members to send text-to-speech messages by starting a message with /tts.", Text),
    p(46, "SEND_VOICE_MESSAGES", "Send Voice Messages",
      "Allows members to send voice messages.", Text),
    p(49, "SEND_POLLS", "Create Polls",
      "Allows members to create polls.", Text),
    p(31, "USE_APPLICATION_COMMANDS", "Use Application Commands",
      "Allows members to use commands from applications, including slash commands and context menu commands.", Text),
    p(50, "USE_EXTERNAL_APPS", "Use External Apps",
      "Allows apps that members have added to their account to post messages in this server.", Text),
    // Voice
    p(20, "CONNECT", "Connect",
      "Allows members to join voice channels and hear others.", Voice),
    p(21, "SPEAK", "Speak",
      "Allows members to talk in voice channels.", Voice),
    p(9, "STREAM", "Video",
      "Allows members to share their video, screen share, or stream a game in this server.", Voice),
    p(39, "USE_EMBEDDED_ACTIVITIES", "Use Activities",
      "Allows members to use Activities in this server.", Voice),
    p(42, "USE_SOUNDBOARD", "Use Soundboard",
      "Allows members to send sounds from the server soundboard.", Voice),
    p(45, "USE_EXTERNAL_SOUNDS", "Use External Sounds",
      "Allows members to use sounds from other servers, if they're a Discord Nitro member.", Voice),
    p(25, "USE_VAD", "Use Voice Activity",
      "Allows members to speak in voice channels by simply talking. If off, they must use Push-to-Talk.", Voice),
    p(8, "PRIORITY_SPEAKER", "Priority Speaker",
      "Allows members to be more easily heard in voice channels: others are quieter while they speak.", Voice),
    p(22, "MUTE_MEMBERS", "Mute Members",
      "Allows members to mute other members in voice channels for everyone.", Voice),
    p(23, "DEAFEN_MEMBERS", "Deafen Members",
      "Allows members to deafen other members in voice channels, so they can't speak or hear others.", Voice),
    p(24, "MOVE_MEMBERS", "Move Members",
      "Allows members to disconnect or move other members between voice channels they have access to.", Voice),
    p(48, "SET_VOICE_CHANNEL_STATUS", "Set Voice Channel Status",
      "Allows members to create and edit voice channel statuses.", Voice),
    p(32, "REQUEST_TO_SPEAK", "Request to Speak",
      "Allows members to request to speak in stage channels.", Voice),
    // Advanced
    p(3, "ADMINISTRATOR", "Administrator",
      "Members with this permission will have every permission and will also bypass all channel specific permissions or restrictions. This is a dangerous permission to grant.", Advanced),
];

/// All documented bits OR-ed together.
pub fn all_bits() -> u64 {
    PERMISSIONS.iter().fold(0, |a, p| a | p.value())
}

pub fn by_key(key: &str) -> Option<&'static PermInfo> {
    PERMISSIONS.iter().find(|p| p.key == key)
}

pub fn by_bit(bit: u8) -> Option<&'static PermInfo> {
    PERMISSIONS.iter().find(|p| p.bit == bit)
}

/// The permissions of one editor section, in display order.
pub fn in_group(group: PermGroup) -> impl Iterator<Item = &'static PermInfo> {
    PERMISSIONS.iter().filter(move |p| p.group == group)
}

/// Display names of the bits set in `bits` (unknown bits as "Unknown (n)").
pub fn names(bits: u64) -> Vec<String> {
    (0..64u8)
        .filter(|b| bits & (1u64 << b) != 0)
        .map(|b| match by_bit(b) {
            Some(p) => p.name.to_string(),
            None => format!("Unknown ({b})"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

/// Rank order: `Greater` means `a` is higher in the role list than `b`.
pub fn rank_cmp(a: &Role, b: &Role) -> Ordering {
    let id = |r: &Role| r.id.parse::<u64>().unwrap_or(u64::MAX);
    a.position
        .cmp(&b.position)
        .then_with(|| id(b).cmp(&id(a)))
}

/// Highest of the roles in `ids` (None: only @everyone).
pub fn highest_role<'a>(guild_roles: &'a [Role], ids: &[String]) -> Option<&'a Role> {
    guild_roles
        .iter()
        .filter(|r| ids.contains(&r.id))
        .max_by(|a, b| rank_cmp(a, b))
}

/// May someone with `my_roles` manage `target` by hierarchy alone (not
/// counting the Manage Roles permission or ownership)? Only if one of my
/// roles is strictly above it.
pub fn can_manage(guild_roles: &[Role], my_roles: &[String], target: &Role) -> bool {
    highest_role(guild_roles, my_roles).is_some_and(|mine| rank_cmp(mine, target).is_gt())
}

/// Is my highest role above the target member's highest role? A member
/// with no roles sits at @everyone (position 0), below any real role.
pub fn outranks(guild_roles: &[Role], my_roles: &[String], their_roles: &[String]) -> bool {
    match (
        highest_role(guild_roles, my_roles),
        highest_role(guild_roles, their_roles),
    ) {
        (Some(mine), Some(theirs)) => rank_cmp(mine, theirs).is_gt(),
        (Some(mine), None) => mine.position > 0,
        (None, _) => false,
    }
}

/// Who is acting: me in one server.
#[derive(Debug, Clone, Copy)]
pub struct Actor<'a> {
    pub guild_id: &'a str,
    pub owner_id: &'a str,
    pub user_id: &'a str,
    /// The server's roles (any order).
    pub roles: &'a [Role],
    /// My role ids.
    pub my_roles: &'a [String],
}

impl<'a> Actor<'a> {
    pub fn is_owner(&self) -> bool {
        !self.owner_id.is_empty() && self.owner_id == self.user_id
    }

    /// Server-wide permissions (no channel overwrites). Owner and
    /// Administrator: every bit.
    pub fn permissions(&self) -> u64 {
        crate::perms::channel_permissions(
            self.guild_id,
            self.owner_id,
            self.roles,
            &[],
            &crate::perms::Me {
                user_id: self.user_id,
                roles: self.my_roles,
            },
        )
    }

    pub fn has(&self, perm: u64) -> bool {
        self.permissions() & perm == perm
    }

    pub fn is_admin(&self) -> bool {
        self.has(ADMINISTRATOR)
    }

    /// Bits I may switch on or off in a role.
    pub fn grantable(&self) -> u64 {
        if self.is_owner() || self.is_admin() {
            u64::MAX
        } else {
            self.permissions()
        }
    }

    /// Is this role below me (owner: every role)?
    pub fn above_role(&self, role: &Role) -> bool {
        self.is_owner() || can_manage(self.roles, self.my_roles, role)
    }

    /// Edit name / colour / permissions / position of a role.
    pub fn can_edit_role(&self, role: &Role) -> bool {
        self.has(MANAGE_ROLES) && self.above_role(role)
    }

    pub fn is_everyone(&self, role: &Role) -> bool {
        role.id == self.guild_id
    }

    /// Delete a role (never @everyone or integration-managed roles).
    pub fn can_delete_role(&self, role: &Role, managed: bool) -> bool {
        self.can_edit_role(role) && !self.is_everyone(role) && !managed
    }

    /// Give a role to / take it from a member.
    pub fn can_assign_role(&self, role: &Role, managed: bool) -> bool {
        self.can_delete_role(role, managed)
    }

    /// Hierarchy check for kick / ban / timeout / nickname on someone else.
    pub fn can_act_on(&self, user_id: &str, their_roles: &[String]) -> bool {
        if user_id == self.user_id || (!self.owner_id.is_empty() && user_id == self.owner_id) {
            return false;
        }
        self.is_owner() || outranks(self.roles, self.my_roles, their_roles)
    }

    /// Does this member have Administrator through their roles (or own the
    /// server)?
    pub fn member_is_admin(&self, user_id: &str, their_roles: &[String]) -> bool {
        Actor {
            user_id,
            my_roles: their_roles,
            ..*self
        }
        .is_admin()
    }

    pub fn can_kick(&self, user_id: &str, their_roles: &[String]) -> bool {
        self.has(KICK_MEMBERS) && self.can_act_on(user_id, their_roles)
    }

    pub fn can_ban(&self, user_id: &str, their_roles: &[String]) -> bool {
        self.has(BAN_MEMBERS) && self.can_act_on(user_id, their_roles)
    }

    pub fn can_timeout(&self, user_id: &str, their_roles: &[String]) -> bool {
        self.has(MODERATE_MEMBERS)
            && self.can_act_on(user_id, their_roles)
            && !self.member_is_admin(user_id, their_roles)
    }

    pub fn can_change_nick(&self, user_id: &str, their_roles: &[String]) -> bool {
        if user_id == self.user_id {
            self.is_owner() || self.has(CHANGE_NICKNAME) || self.has(MANAGE_NICKNAMES)
        } else {
            self.has(MANAGE_NICKNAMES) && self.can_act_on(user_id, their_roles)
        }
    }

    /// Delete / rename an emoji: anyone's with Manage Expressions, my own
    /// with Create Expressions.
    pub fn can_manage_emoji(&self, uploader_id: Option<&str>) -> bool {
        self.has(MANAGE_GUILD_EXPRESSIONS)
            || (uploader_id == Some(self.user_id) && self.has(CREATE_GUILD_EXPRESSIONS))
    }

    pub fn can_upload_emoji(&self) -> bool {
        self.has(CREATE_GUILD_EXPRESSIONS) || self.has(MANAGE_GUILD_EXPRESSIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn role(id: &str, position: i64, perms: u64) -> Role {
        Role::from_json(&json!({
            "id": id, "name": id, "position": position, "permissions": perms.to_string()
        }))
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn table_is_complete_and_unique() {
        let mut bits: Vec<u8> = PERMISSIONS.iter().map(|p| p.bit).collect();
        bits.sort();
        let n = bits.len();
        bits.dedup();
        assert_eq!(bits.len(), n, "duplicate bits");
        // 0..=46 and 48..=52 (47 is undocumented).
        let expected: Vec<u8> = (0..=46).chain(48..=52).collect();
        assert_eq!(bits, expected);
        assert_eq!(by_key("KICK_MEMBERS").unwrap().value(), KICK_MEMBERS);
        assert_eq!(by_key("ADMINISTRATOR").unwrap().value(), 8);
        assert_eq!(by_key("MODERATE_MEMBERS").unwrap().value(), 1 << 40);
        assert_eq!(by_key("PIN_MESSAGES").unwrap().value(), 1 << 51);
        assert_eq!(by_key("MANAGE_GUILD_EXPRESSIONS").unwrap().value(), 1 << 30);
        for g in PermGroup::ALL {
            assert!(in_group(g).count() > 0);
        }
        assert_eq!(in_group(PermGroup::Advanced).count(), 1);
        assert_eq!(all_bits().count_ones() as usize, PERMISSIONS.len());
    }

    #[test]
    fn search_and_names() {
        let kick = by_key("KICK_MEMBERS").unwrap();
        assert!(kick.matches("kick"));
        assert!(kick.matches("KICK_MEMBERS"));
        assert!(kick.matches("kick members"));
        assert!(kick.matches(""));
        assert!(!kick.matches("voice"));
        assert_eq!(
            names(KICK_MEMBERS | BAN_MEMBERS | (1 << 47)),
            ["Kick, Approve, and Reject Members", "Ban Members", "Unknown (47)"]
        );
    }

    #[test]
    fn rank_and_highest() {
        let roles = vec![role("1", 0, 0), role("10", 3, 0), role("20", 3, 0), role("30", 1, 0)];
        // Same position: the smaller (older) id ranks higher.
        assert!(rank_cmp(&roles[1], &roles[2]).is_gt());
        assert_eq!(highest_role(&roles, &ids(&["30", "20"])).unwrap().id, "20");
        assert_eq!(highest_role(&roles, &ids(&["10", "20"])).unwrap().id, "10");
        assert!(highest_role(&roles, &[]).is_none());
    }

    #[test]
    fn hierarchy() {
        let roles = vec![
            role("g", 0, 0),
            role("admin", 5, ADMINISTRATOR),
            role("mod", 3, MANAGE_ROLES | KICK_MEMBERS | MODERATE_MEMBERS),
            role("helper", 2, 0),
            role("member", 1, 0),
        ];
        let mine = ids(&["mod"]);
        // can_manage: strictly below only.
        assert!(can_manage(&roles, &mine, &roles[3]));
        assert!(can_manage(&roles, &mine, &roles[4]));
        assert!(!can_manage(&roles, &mine, &roles[2]), "own top role");
        assert!(!can_manage(&roles, &mine, &roles[1]));
        assert!(!can_manage(&roles, &[], &roles[4]));

        let me = Actor {
            guild_id: "g",
            owner_id: "owner",
            user_id: "me",
            roles: &roles,
            my_roles: &mine,
        };
        assert!(me.can_edit_role(&roles[3]));
        assert!(!me.can_edit_role(&roles[2]));
        assert!(me.can_edit_role(&roles[0]), "@everyone is below everything");
        assert!(!me.can_delete_role(&roles[0], false), "never @everyone");
        assert!(!me.can_assign_role(&roles[3], true), "managed");
        assert!(me.can_assign_role(&roles[3], false));
        // Grantable = my permissions.
        assert_eq!(me.grantable() & ADMINISTRATOR, 0);
        assert_ne!(me.grantable() & KICK_MEMBERS, 0);

        // Members.
        assert!(me.can_kick("x", &ids(&["helper"])));
        assert!(me.can_kick("x", &[]));
        assert!(!me.can_kick("x", &ids(&["mod"])), "same top role");
        assert!(!me.can_kick("x", &ids(&["admin"])));
        assert!(!me.can_kick("me", &[]), "myself");
        assert!(!me.can_kick("owner", &[]), "the owner");
        assert!(!me.can_ban("x", &[]), "no Ban Members");
        assert!(me.can_timeout("x", &ids(&["member"])));
        assert!(!me.can_change_nick("x", &[]), "no Manage Nicknames");

        // Owner: everything except acting on themselves.
        let own: Vec<String> = Vec::new();
        let owner = Actor {
            user_id: "owner",
            my_roles: &own,
            ..me
        };
        assert!(owner.can_edit_role(&roles[1]));
        assert!(owner.can_ban("x", &ids(&["admin"])));
        assert!(!owner.can_timeout("x", &ids(&["admin"])), "admins can't be timed out");
        assert!(!owner.can_ban("owner", &[]));
        assert!(owner.can_change_nick("owner", &[]));
        assert_eq!(owner.grantable(), u64::MAX);

        // A role-less member can't act on anyone.
        let nobody = Actor {
            user_id: "n",
            my_roles: &own,
            ..me
        };
        assert!(!nobody.can_act_on("x", &[]));
    }

    #[test]
    fn emoji_permissions() {
        let roles = vec![
            role("g", 0, 0),
            role("creator", 1, CREATE_GUILD_EXPRESSIONS),
            role("manager", 2, MANAGE_GUILD_EXPRESSIONS),
        ];
        let c = ids(&["creator"]);
        let creator = Actor {
            guild_id: "g",
            owner_id: "o",
            user_id: "me",
            roles: &roles,
            my_roles: &c,
        };
        assert!(creator.can_upload_emoji());
        assert!(creator.can_manage_emoji(Some("me")));
        assert!(!creator.can_manage_emoji(Some("other")));
        assert!(!creator.can_manage_emoji(None));
        let m = ids(&["manager"]);
        let manager = Actor {
            my_roles: &m,
            ..creator
        };
        assert!(manager.can_manage_emoji(Some("other")));
    }
}
