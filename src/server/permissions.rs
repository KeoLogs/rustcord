//! Every Discord permission bit, grouped and described the way Discord's
//! role and channel permission editors show them, plus the allow / neutral /
//! deny ("tri-state") math used by channel permission overwrites.
//!
//! Bits and their channel-type columns follow Discord's permissions docs
//! (checked November 2026, up to `BYPASS_SLOWMODE = 1 << 52`; bit 47 is
//! unused). `perms.rs` keeps the effective-permission computation; this
//! module only adds the catalogue and helpers on top of it.

use crate::perms::{self, Me};
use crate::types::{CHANNEL_ANNOUNCEMENT, CHANNEL_CATEGORY, CHANNEL_STAGE, CHANNEL_VOICE, Guild};

pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const ADMINISTRATOR: u64 = perms::ADMINISTRATOR;
pub const MANAGE_CHANNELS: u64 = 1 << 4;
pub const MANAGE_GUILD: u64 = 1 << 5;
pub const ADD_REACTIONS: u64 = 1 << 6;
pub const VIEW_AUDIT_LOG: u64 = 1 << 7;
pub const PRIORITY_SPEAKER: u64 = 1 << 8;
pub const STREAM: u64 = 1 << 9;
pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const SEND_TTS_MESSAGES: u64 = 1 << 12;
pub const MANAGE_MESSAGES: u64 = perms::MANAGE_MESSAGES;
pub const EMBED_LINKS: u64 = 1 << 14;
pub const ATTACH_FILES: u64 = 1 << 15;
pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
pub const MENTION_EVERYONE: u64 = 1 << 17;
pub const USE_EXTERNAL_EMOJIS: u64 = 1 << 18;
pub const VIEW_GUILD_INSIGHTS: u64 = 1 << 19;
pub const CONNECT: u64 = 1 << 20;
pub const SPEAK: u64 = 1 << 21;
pub const MUTE_MEMBERS: u64 = 1 << 22;
pub const DEAFEN_MEMBERS: u64 = 1 << 23;
pub const MOVE_MEMBERS: u64 = 1 << 24;
pub const USE_VAD: u64 = 1 << 25;
pub const CHANGE_NICKNAME: u64 = 1 << 26;
pub const MANAGE_NICKNAMES: u64 = 1 << 27;
/// "Manage Roles" on a server, "Manage Permissions" on a channel.
pub const MANAGE_ROLES: u64 = 1 << 28;
pub const MANAGE_WEBHOOKS: u64 = 1 << 29;
pub const MANAGE_GUILD_EXPRESSIONS: u64 = 1 << 30;
pub const USE_APPLICATION_COMMANDS: u64 = 1 << 31;
pub const REQUEST_TO_SPEAK: u64 = 1 << 32;
pub const MANAGE_EVENTS: u64 = 1 << 33;
pub const MANAGE_THREADS: u64 = 1 << 34;
pub const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
pub const CREATE_PRIVATE_THREADS: u64 = 1 << 36;
pub const USE_EXTERNAL_STICKERS: u64 = 1 << 37;
pub const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
pub const USE_EMBEDDED_ACTIVITIES: u64 = 1 << 39;
pub const MODERATE_MEMBERS: u64 = 1 << 40;
pub const VIEW_CREATOR_MONETIZATION_ANALYTICS: u64 = 1 << 41;
pub const USE_SOUNDBOARD: u64 = 1 << 42;
pub const CREATE_GUILD_EXPRESSIONS: u64 = 1 << 43;
pub const CREATE_EVENTS: u64 = 1 << 44;
pub const USE_EXTERNAL_SOUNDS: u64 = 1 << 45;
pub const SEND_VOICE_MESSAGES: u64 = 1 << 46;
pub const SET_VOICE_CHANNEL_STATUS: u64 = 1 << 48;
pub const SEND_POLLS: u64 = 1 << 49;
pub const USE_EXTERNAL_APPS: u64 = 1 << 50;
pub const PIN_MESSAGES: u64 = 1 << 51;
pub const BYPASS_SLOWMODE: u64 = 1 << 52;

/// Any of these opens "Server Settings" (Discord shows the item when you can
/// manage at least one of its pages).
pub const SERVER_SETTINGS_ANY: u64 = MANAGE_GUILD
    | MANAGE_ROLES
    | MANAGE_CHANNELS
    | MANAGE_GUILD_EXPRESSIONS
    | CREATE_GUILD_EXPRESSIONS
    | MANAGE_WEBHOOKS
    | VIEW_AUDIT_LOG
    | BAN_MEMBERS
    | KICK_MEMBERS
    | MODERATE_MEMBERS;

/// Channel-type columns of Discord's docs: Text, Voice, Stage.
pub const T: u8 = 1;
pub const V: u8 = 2;
pub const S: u8 = 4;

/// Sections of Discord's permission editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermGroup {
    General,
    Membership,
    Text,
    Voice,
    Apps,
    Stage,
    Events,
    Advanced,
}

impl PermGroup {
    pub const ALL: [PermGroup; 8] = [
        PermGroup::General,
        PermGroup::Membership,
        PermGroup::Text,
        PermGroup::Voice,
        PermGroup::Apps,
        PermGroup::Stage,
        PermGroup::Events,
        PermGroup::Advanced,
    ];

    /// Section title in the server role editor.
    pub fn server_label(self) -> &'static str {
        match self {
            PermGroup::General => "General Server Permissions",
            PermGroup::Membership => "Membership Permissions",
            PermGroup::Text => "Text Channel Permissions",
            PermGroup::Voice => "Voice Channel Permissions",
            PermGroup::Apps => "Apps Permissions",
            PermGroup::Stage => "Stage Channel Permissions",
            PermGroup::Events => "Events Permissions",
            PermGroup::Advanced => "Advanced Permissions",
        }
    }

    /// Section title in a channel's permission overwrite editor.
    pub fn channel_label(self) -> &'static str {
        match self {
            PermGroup::General => "General Channel Permissions",
            other => other.server_label(),
        }
    }
}

/// One permission as the editors show it.
#[derive(Debug, Clone, Copy)]
pub struct PermInfo {
    pub bit: u64,
    /// Name in the server role editor.
    pub name: &'static str,
    /// Name in a channel's overwrite editor, when it differs.
    pub channel_name: Option<&'static str>,
    pub description: &'static str,
    pub group: PermGroup,
    /// Channel types it can be overwritten on (`T | V | S`); 0 = server only.
    pub channels: u8,
}

impl PermInfo {
    pub fn label(&self, in_channel: bool) -> &'static str {
        match (in_channel, self.channel_name) {
            (true, Some(n)) => n,
            _ => self.name,
        }
    }
}

const fn p(
    bit: u64,
    name: &'static str,
    description: &'static str,
    group: PermGroup,
    channels: u8,
) -> PermInfo {
    PermInfo {
        bit,
        name,
        channel_name: None,
        description,
        group,
        channels,
    }
}

const fn pc(
    bit: u64,
    name: &'static str,
    channel_name: &'static str,
    description: &'static str,
    group: PermGroup,
    channels: u8,
) -> PermInfo {
    PermInfo {
        bit,
        name,
        channel_name: Some(channel_name),
        description,
        group,
        channels,
    }
}

use PermGroup as G;

/// Every permission, in Discord's editor order.
pub const ALL: &[PermInfo] = &[
    // General
    pc(
        VIEW_CHANNEL,
        "View Channels",
        "View Channel",
        "Allows members to view channels by default (excluding private channels).",
        G::General,
        T | V | S,
    ),
    pc(
        MANAGE_CHANNELS,
        "Manage Channels",
        "Manage Channel",
        "Allows members to create, edit, or delete channels.",
        G::General,
        T | V | S,
    ),
    pc(
        MANAGE_ROLES,
        "Manage Roles",
        "Manage Permissions",
        "Allows members to create new roles and edit or delete roles lower than their highest role. In a channel: change this channel's permissions.",
        G::General,
        T | V | S,
    ),
    p(
        CREATE_GUILD_EXPRESSIONS,
        "Create Expressions",
        "Allows members to add custom emoji, stickers, and sounds in this server.",
        G::General,
        0,
    ),
    p(
        MANAGE_GUILD_EXPRESSIONS,
        "Manage Expressions",
        "Allows members to edit or remove custom emoji, stickers, and sounds in this server.",
        G::General,
        0,
    ),
    p(
        VIEW_AUDIT_LOG,
        "View Audit Log",
        "Allows members to view a record of who made which changes in this server.",
        G::General,
        0,
    ),
    p(
        VIEW_GUILD_INSIGHTS,
        "View Server Insights",
        "Allows members to view Server Insights.",
        G::General,
        0,
    ),
    p(
        MANAGE_WEBHOOKS,
        "Manage Webhooks",
        "Allows members to create, edit, or delete webhooks.",
        G::General,
        T | V | S,
    ),
    p(
        MANAGE_GUILD,
        "Manage Server",
        "Allows members to change this server's name, switch regions, view all invites, and add apps.",
        G::General,
        0,
    ),
    // Membership
    p(
        CREATE_INSTANT_INVITE,
        "Create Invite",
        "Allows members to invite new people to this server.",
        G::Membership,
        T | V | S,
    ),
    p(
        CHANGE_NICKNAME,
        "Change Nickname",
        "Allows members to change their own nickname.",
        G::Membership,
        0,
    ),
    p(
        MANAGE_NICKNAMES,
        "Manage Nicknames",
        "Allows members to change the nicknames of other members.",
        G::Membership,
        0,
    ),
    p(
        KICK_MEMBERS,
        "Kick Members",
        "Allows members to remove other members from this server.",
        G::Membership,
        0,
    ),
    p(
        BAN_MEMBERS,
        "Ban Members",
        "Allows members to permanently ban and delete the message history of other members.",
        G::Membership,
        0,
    ),
    p(
        MODERATE_MEMBERS,
        "Timeout Members",
        "Allows members to temporarily stop users from chatting or joining voice.",
        G::Membership,
        0,
    ),
    // Text
    p(
        SEND_MESSAGES,
        "Send Messages and Create Posts",
        "Allows members to send messages in text channels and create posts in forum channels.",
        G::Text,
        T | V | S,
    ),
    p(
        SEND_MESSAGES_IN_THREADS,
        "Send Messages in Threads and Posts",
        "Allows members to send messages in threads and in posts on forum channels.",
        G::Text,
        T,
    ),
    p(
        CREATE_PUBLIC_THREADS,
        "Create Public Threads",
        "Allows members to create threads that everyone in a channel can view.",
        G::Text,
        T,
    ),
    p(
        CREATE_PRIVATE_THREADS,
        "Create Private Threads",
        "Allows members to create invite-only threads.",
        G::Text,
        T,
    ),
    p(
        EMBED_LINKS,
        "Embed Links",
        "Allows links that members share to show embedded content in text channels.",
        G::Text,
        T | V | S,
    ),
    p(
        ATTACH_FILES,
        "Attach Files",
        "Allows members to upload files or media in text channels.",
        G::Text,
        T | V | S,
    ),
    p(
        ADD_REACTIONS,
        "Add Reactions",
        "Allows members to add new emoji reactions to a message.",
        G::Text,
        T | V | S,
    ),
    p(
        USE_EXTERNAL_EMOJIS,
        "Use External Emoji",
        "Allows members to use emoji from other servers.",
        G::Text,
        T | V | S,
    ),
    p(
        USE_EXTERNAL_STICKERS,
        "Use External Stickers",
        "Allows members to use stickers from other servers.",
        G::Text,
        T | V | S,
    ),
    p(
        MENTION_EVERYONE,
        "Mention @everyone, @here, and All Roles",
        "Allows members to use @everyone, @here, and to @mention all roles.",
        G::Text,
        T | V | S,
    ),
    p(
        MANAGE_MESSAGES,
        "Manage Messages",
        "Allows members to delete messages by other members.",
        G::Text,
        T | V | S,
    ),
    p(
        PIN_MESSAGES,
        "Pin Messages",
        "Allows members to pin or unpin any message.",
        G::Text,
        T,
    ),
    p(
        BYPASS_SLOWMODE,
        "Bypass Slowmode",
        "Allows members to send messages without being affected by slowmode.",
        G::Text,
        T | V | S,
    ),
    p(
        MANAGE_THREADS,
        "Manage Threads and Posts",
        "Allows members to rename, delete, close, and turn on slowmode for threads and posts.",
        G::Text,
        T,
    ),
    p(
        READ_MESSAGE_HISTORY,
        "Read Message History",
        "Allows members to read previous messages sent in channels.",
        G::Text,
        T | V | S,
    ),
    p(
        SEND_TTS_MESSAGES,
        "Send Text-to-Speech Messages",
        "Allows members to send text-to-speech messages by starting a message with /tts.",
        G::Text,
        T | V | S,
    ),
    p(
        SEND_VOICE_MESSAGES,
        "Send Voice Messages",
        "Allows members to send voice messages.",
        G::Text,
        T | V | S,
    ),
    p(
        SEND_POLLS,
        "Create Polls",
        "Allows members to create polls.",
        G::Text,
        T | V | S,
    ),
    // Voice
    p(
        CONNECT,
        "Connect",
        "Allows members to join voice channels and hear others.",
        G::Voice,
        V | S,
    ),
    p(
        SPEAK,
        "Speak",
        "Allows members to talk in voice channels.",
        G::Voice,
        V,
    ),
    p(
        STREAM,
        "Video",
        "Allows members to share their video, screen share, or stream a game.",
        G::Voice,
        V | S,
    ),
    p(
        USE_SOUNDBOARD,
        "Use Soundboard",
        "Allows members to send sounds from the server soundboard.",
        G::Voice,
        V,
    ),
    p(
        USE_EXTERNAL_SOUNDS,
        "Use External Sounds",
        "Allows members to use sounds from other servers.",
        G::Voice,
        V,
    ),
    p(
        USE_VAD,
        "Use Voice Activity",
        "Allows members to speak by simply talking. If off, they must use Push-to-Talk.",
        G::Voice,
        V,
    ),
    p(
        PRIORITY_SPEAKER,
        "Priority Speaker",
        "Allows members to be more easily heard in voice channels.",
        G::Voice,
        V,
    ),
    p(
        MUTE_MEMBERS,
        "Mute Members",
        "Allows members to mute other members in voice channels for everyone.",
        G::Voice,
        V | S,
    ),
    p(
        DEAFEN_MEMBERS,
        "Deafen Members",
        "Allows members to deafen other members in voice channels.",
        G::Voice,
        V,
    ),
    p(
        MOVE_MEMBERS,
        "Move Members",
        "Allows members to disconnect or move other members between voice channels.",
        G::Voice,
        V | S,
    ),
    p(
        SET_VOICE_CHANNEL_STATUS,
        "Set Voice Channel Status",
        "Allows members to create and edit voice channel statuses.",
        G::Voice,
        V,
    ),
    // Apps
    p(
        USE_APPLICATION_COMMANDS,
        "Use Application Commands",
        "Allows members to use commands from applications, including slash commands.",
        G::Apps,
        T | V | S,
    ),
    p(
        USE_EMBEDDED_ACTIVITIES,
        "Use Activities",
        "Allows members to use Activities.",
        G::Apps,
        T | V,
    ),
    p(
        USE_EXTERNAL_APPS,
        "Use External Apps",
        "Allows apps that members have added to their account to post messages in this server.",
        G::Apps,
        T | V | S,
    ),
    // Stage
    p(
        REQUEST_TO_SPEAK,
        "Request to Speak",
        "Allows requests to speak in Stage channels.",
        G::Stage,
        S,
    ),
    // Events
    p(
        CREATE_EVENTS,
        "Create Events",
        "Allows members to create events.",
        G::Events,
        V | S,
    ),
    p(
        MANAGE_EVENTS,
        "Manage Events",
        "Allows members to edit and cancel events.",
        G::Events,
        V | S,
    ),
    // Advanced
    p(
        ADMINISTRATOR,
        "Administrator",
        "Members with this permission have every permission and bypass channel-specific permissions. This is a dangerous permission to grant.",
        G::Advanced,
        0,
    ),
    p(
        VIEW_CREATOR_MONETIZATION_ANALYTICS,
        "View Server Subscription Insights",
        "Allows members to view server subscription insights.",
        G::Advanced,
        0,
    ),
];

/// Catalogue entry for a bit.
pub fn info(bit: u64) -> Option<&'static PermInfo> {
    ALL.iter().find(|p| p.bit == bit)
}

/// The `T | V | S` column a channel type uses. Categories take every
/// channel permission (they sync down to text and voice channels).
pub fn channel_scope(kind: u64) -> u8 {
    match kind {
        CHANNEL_CATEGORY => T | V | S,
        CHANNEL_VOICE => V,
        CHANNEL_STAGE => S,
        // Text, announcement, forum (15), media (16).
        _ => T,
    }
}

/// Permissions that can be overwritten on a channel of `kind`, grouped in
/// editor order (empty groups dropped).
pub fn channel_groups(kind: u64) -> Vec<(PermGroup, Vec<&'static PermInfo>)> {
    let scope = channel_scope(kind);
    PermGroup::ALL
        .iter()
        .map(|g| {
            let items: Vec<_> = ALL
                .iter()
                .filter(|p| p.group == *g && p.channels & scope != 0)
                .collect();
            (*g, items)
        })
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

/// All permissions grouped for a server role editor (for the Roles page).
pub fn server_groups() -> Vec<(PermGroup, Vec<&'static PermInfo>)> {
    PermGroup::ALL
        .iter()
        .map(|g| (*g, ALL.iter().filter(|p| p.group == *g).collect()))
        .collect()
}

/// True if every bit of `bits` is in `perms`.
pub fn has(perms: u64, bits: u64) -> bool {
    perms & bits == bits
}

/// True if any bit of `bits` is in `perms`.
pub fn has_any(perms: u64, bits: u64) -> bool {
    perms & bits != 0
}

/// My server-wide permissions (roles only, no channel overwrites). Owner
/// and Administrator get `u64::MAX`.
pub fn guild_permissions(guild: &Guild, my_id: &str) -> u64 {
    perms::channel_permissions(
        &guild.id,
        &guild.owner_id,
        &guild.roles,
        &[],
        &Me {
            user_id: my_id,
            roles: &guild.my_roles,
        },
    )
}

/// My permissions in one of the server's channels (`u64::MAX` for owner /
/// admin, 0 if the channel isn't in the server).
pub fn channel_permissions_in(guild: &Guild, channel_id: &str, my_id: &str) -> u64 {
    let Some(ch) = guild.channel(channel_id) else {
        return 0;
    };
    perms::channel_permissions(
        &guild.id,
        &guild.owner_id,
        &guild.roles,
        &ch.overwrites,
        &Me {
            user_id: my_id,
            roles: &guild.my_roles,
        },
    )
}

/// True for channels people can be invited to (not categories / DMs).
pub fn invitable_kind(kind: u64) -> bool {
    matches!(
        kind,
        0 | CHANNEL_ANNOUNCEMENT | CHANNEL_VOICE | CHANNEL_STAGE | 15 | 16
    )
}

// ---------------------------------------------------------------------------
// Tri-state overwrite math
// ---------------------------------------------------------------------------

/// One permission in an overwrite: red ✕, grey /, green ✓ in Discord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermState {
    Deny,
    Neutral,
    Allow,
}

/// State of `bit` in an overwrite. If a payload somehow has the bit in both
/// fields, Allow wins (Discord applies deny first, then allow).
pub fn state_of(allow: u64, deny: u64, bit: u64) -> PermState {
    if allow & bit != 0 {
        PermState::Allow
    } else if deny & bit != 0 {
        PermState::Deny
    } else {
        PermState::Neutral
    }
}

/// Set `bit` to `state`, keeping allow and deny disjoint.
pub fn set_state(allow: &mut u64, deny: &mut u64, bit: u64, state: PermState) {
    *allow &= !bit;
    *deny &= !bit;
    match state {
        PermState::Allow => *allow |= bit,
        PermState::Deny => *deny |= bit,
        PermState::Neutral => {}
    }
}

/// Apply one overwrite to a permission set, Discord's way.
pub fn apply_overwrite(perms: u64, allow: u64, deny: u64) -> u64 {
    (perms & !deny) | allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::members::Role;
    use serde_json::json;

    #[test]
    fn tri_state_round_trip() {
        let (mut allow, mut deny) = (0u64, 0u64);
        assert_eq!(state_of(allow, deny, SEND_MESSAGES), PermState::Neutral);
        set_state(&mut allow, &mut deny, SEND_MESSAGES, PermState::Deny);
        assert_eq!((allow, deny), (0, SEND_MESSAGES));
        assert_eq!(state_of(allow, deny, SEND_MESSAGES), PermState::Deny);
        set_state(&mut allow, &mut deny, SEND_MESSAGES, PermState::Allow);
        assert_eq!((allow, deny), (SEND_MESSAGES, 0));
        set_state(&mut allow, &mut deny, VIEW_CHANNEL, PermState::Deny);
        assert_eq!((allow, deny), (SEND_MESSAGES, VIEW_CHANNEL));
        set_state(&mut allow, &mut deny, SEND_MESSAGES, PermState::Neutral);
        assert_eq!((allow, deny), (0, VIEW_CHANNEL));
        // Both set: allow wins (deny is applied first).
        assert_eq!(state_of(CONNECT, CONNECT, CONNECT), PermState::Allow);
        assert_eq!(apply_overwrite(CONNECT, CONNECT, CONNECT), CONNECT);
        assert_eq!(apply_overwrite(CONNECT | SPEAK, 0, SPEAK), CONNECT);
        // High bits survive.
        set_state(&mut allow, &mut deny, BYPASS_SLOWMODE, PermState::Allow);
        assert_eq!(state_of(allow, deny, BYPASS_SLOWMODE), PermState::Allow);
    }

    #[test]
    fn catalogue_is_consistent() {
        let mut seen = 0u64;
        for p in ALL {
            assert_eq!(p.bit.count_ones(), 1, "{}", p.name);
            assert_eq!(seen & p.bit, 0, "duplicate {}", p.name);
            seen |= p.bit;
        }
        // Every bit from 0 to 52 except the unused 47.
        assert_eq!(seen, ((1u64 << 53) - 1) & !(1 << 47));
        assert_eq!(
            info(MANAGE_ROLES).unwrap().label(true),
            "Manage Permissions"
        );
        assert_eq!(info(MANAGE_ROLES).unwrap().label(false), "Manage Roles");
    }

    #[test]
    fn channel_groups_by_type() {
        let bits = |kind| -> Vec<u64> {
            channel_groups(kind)
                .into_iter()
                .flat_map(|(_, v)| v.into_iter().map(|p| p.bit))
                .collect()
        };
        let text = bits(0);
        assert!(text.contains(&SEND_MESSAGES) && text.contains(&MANAGE_THREADS));
        assert!(!text.contains(&CONNECT) && !text.contains(&ADMINISTRATOR));
        assert!(!text.contains(&KICK_MEMBERS));
        let voice = bits(CHANNEL_VOICE);
        assert!(voice.contains(&CONNECT) && voice.contains(&SEND_MESSAGES));
        assert!(!voice.contains(&MANAGE_THREADS) && !voice.contains(&REQUEST_TO_SPEAK));
        let stage = bits(CHANNEL_STAGE);
        assert!(stage.contains(&REQUEST_TO_SPEAK) && !stage.contains(&SPEAK));
        let cat = bits(CHANNEL_CATEGORY);
        assert!(cat.contains(&SPEAK) && cat.contains(&MANAGE_THREADS));
        // Group order follows Discord's editor.
        let groups: Vec<_> = channel_groups(0).into_iter().map(|(g, _)| g).collect();
        assert_eq!(groups[0], PermGroup::General);
        assert_eq!(server_groups().len(), PermGroup::ALL.len());
    }

    #[test]
    fn guild_and_channel_permissions() {
        let mut g = crate::types::Guild::from_json(&json!({
            "id": "1", "name": "S", "owner_id": "owner",
            "roles": [
                { "id": "1", "name": "@everyone", "permissions": (VIEW_CHANNEL | SEND_MESSAGES).to_string() },
                { "id": "mod", "name": "Mod", "permissions": MANAGE_CHANNELS.to_string() }
            ],
            "channels": [
                { "id": "10", "type": 0, "name": "general", "permission_overwrites": [
                    { "id": "1", "type": 0, "allow": "0", "deny": SEND_MESSAGES.to_string() }
                ]}
            ]
        }));
        let p = guild_permissions(&g, "u");
        assert!(has(p, SEND_MESSAGES) && !has(p, MANAGE_CHANNELS));
        assert!(!has(channel_permissions_in(&g, "10", "u"), SEND_MESSAGES));
        assert_eq!(channel_permissions_in(&g, "nope", "u"), 0);
        g.my_roles = vec!["mod".into()];
        assert!(has(guild_permissions(&g, "u"), MANAGE_CHANNELS));
        assert_eq!(guild_permissions(&g, "owner"), u64::MAX);
        let _ = Role::from_json(&json!({}));
    }
}
