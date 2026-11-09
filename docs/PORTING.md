# Porting from Discord Messenger

[Discord Messenger](https://github.com/DiscordMessenger/dm) (C++, Win32, MIT,
© iProgramInCpp) is the reference for RustCord's feature set. We port its
*features and protocol knowledge*, not its Win32 UI. Paths below are in that
repo's `src/`. DI = `core/DiscordInstance.cpp`, LS =
`core/config/LocalSettings.{hpp,cpp}`, OD = `windows/OptionsDialog.cpp`.

## Already in RustCord
Gateway connect/identify/heartbeat/reconnect · guilds, channels, categories,
DMs · history · send / reply / edit / delete · typing · attachments + uploads ·
avatars · token login screen (DM: `windows/LogonDialog.cpp` — token-only;
email/password/QR are unimplemented there too).

**Batch A — read state & notifications** (`src/read_state.rs`, `src/notify.rs`):
- READY `read_state`, `MESSAGE_ACK` (DI:2710), `PASSIVE_UPDATE_V1` (DI:3036)
- `POST channels/{c}/messages/{m}/ack` with `{"token": null}` on view (DI:1487),
  `{"manual": true, "mention_count": n}` for mark unread (DI:1459/1503),
  `POST read-states/ack-bulk` for "Mark as read" on a server (DI:1543)
- Mute + notification levels: READY `user_guild_settings`,
  `USER_GUILD_SETTINGS_UPDATE` (DI:2719, `core/state/UserGuildSettings.cpp`);
  category overrides cover their channels
- Mentions (user, role via READY `merged_members` / GUILD_CREATE `members`,
  @everyone with suppress settings, DMs) + desktop notifications
  (`core/state/NotificationManager.cpp`), via `notify-rust`
- Settings: EnableNotifications, FlashOnNotification (OD:499-510) in
  Settings → Notifications; 🔔 mentions inbox. All of it is the Notifications
  module except unread markers.
- Gateway RESUME (op 6), RESUMED, op 9 with the resumable flag — DM doesn't do this.

Not done from Batch A: changing mute / notification levels from RustCord (needs
`PATCH users/@me/guilds/{id}/settings`; read-only for now), notification sound.

**Batch B — member list & presence** (`src/members.rs`, `src/presence.rs`,
`src/profile.rs`):
- Op 14 subscribe `{guild_id, typing, activities, threads, channels:{id:[[0,99],…]}}`
  (DI:1596-1637) when a server channel is shown, more ranges as you scroll
- `GUILD_MEMBER_LIST_UPDATE` SYNC/INSERT/DELETE/UPDATE/INVALIDATE (DI:2886,
  3137-3212); only the visible server's list is kept
- Roles (`core/models/Guild.cpp`), `GUILD_ROLE_*`; role colours for names in chat
- `READY_SUPPLEMENTAL` merged_members/presences (DI:2357), `PRESENCE_UPDATE` (DI:3006)
- Op 8 + `GUILD_MEMBERS_CHUNK` (DI:199-250, 3062): `user_ids` for message
  authors' roles (users and bots); `query:""` whole list for bots with the
  opt-in Server Members + Presence intents
- Profile popout: `GET users/{id}/profile`, notes `GET/PUT users/@me/notes/{id}`,
  `USER_NOTE_UPDATE` (`core/state/ProfileCache.cpp`), role chips (`windows/RoleList.cpp`)
- Setting: CompactMemberList (Settings → Appearance)

Not done from Batch B: mutual servers/friends in the popout, per-server
avatars/banners, op 37 (newer bulk subscribe), member search.

**Polish pass** (UI look and feel, gateway compression):
- Chat header with channel icon, name and topic, 🔔 / 📌 / 👥 buttons; a
  collapsible channel list ("«"-style ideas from DM's `windows/ChannelView`),
  reply preview with a connector line (`windows/MessageList.cpp`), and an
  optional status bar module (DM's `windows/StatusBar.cpp`)
- Gateway `compress=zlib-stream` transport compression (not in Discord
  Messenger; written from Discord's gateway docs)
- Local-time timestamps ("Today at …") and date dividers

**Batch C — message richness** (`src/markdown.rs`, `src/embed.rs`,
`src/links.rs`, `src/pins.rs`, `src/autocomplete.rs`) — done:
- Markdown (`core/text/FormattedText.cpp`, cross-checked with Discord's
  simple-markdown rules) incl. spoilers, code blocks, quotes, headings, lists,
  subtext, masked links, `<t:…>` timestamps; mentions resolved through a
  `Lookup` (nicknames, role colours, channels, `mentions` names). Plain text
  (`to_plain_text`) for reply previews, notifications, the inbox and pins.
  DisableFormatting → Settings → Chat
- Embeds (`windows/MessageList.cpp`) incl. MESSAGE_UPDATE link previews;
  ShowEmbedContent = the Embeds module / "Show embeds", ShowEmbedImages
- Link warning + TrustedDomains (`MessageList::ConfirmOpenLink` 1927, LS:215)
  for message links, embed links and attachments
- Pins: `GET channels/{id}/pins` (old array and new `items` shapes),
  `PUT/DELETE channels/{c}/messages/pins/{m}` (`windows/PinList.cpp`),
  `CHANNEL_PINS_UPDATE`; Jump loads `messages?around=` when needed. Pinning
  needs a DM or Manage Messages (`src/perms.rs`: roles + channel overwrites)
- Autocomplete for `@` (members, roles, @everyone/@here), `#`, `:` emoji
  (`windows/AutoComplete.cpp`; Unicode via the `emojis` crate + server emoji)
- Context menu: mark unread, copy message link, copy user ID, save image
  (native save dialog, `rfd`)
- Use12HourTime, ShowBlockedMessages (READY `relationships` type 2,
  RELATIONSHIP_ADD/REMOVE), ReplyMentionDefault (`allowed_mentions.replied_user`)

**Batch D — account & settings sync** (`src/settings_proto.rs`,
`src/status.rs`, `src/quick_switcher.rs`) — done:
- Settings proto from READY `user_settings_proto` or
  `GET users/@me/settings-proto/1`; `PATCH` (DI:1876) only from user actions;
  `USER_SETTINGS_PROTO_UPDATE` (DI:2743)
- Status picker + custom status (with "clear after"); also gateway op 3 so
  it shows at once
- Server folders in the rail (read-only: Discord's order, open / close)
- Explicit content filter, "Allow DMs from server members" (Privacy &
  Safety), compact / cozy (Appearance)
- Quick switcher Ctrl+K (`windows/QuickSwitcher.cpp`), leave server (DI:1647)

Not done from C/D: creating / editing folders and dragging servers between
them, per-server "Allow DMs" overrides, the remaining Options tabs (Window:
start minimised / tray; Connection: custom API URL), reactions in embeds,
video playback (embeds show the thumbnail and open the link). Jumping to an
old pin replaces the cached history with the messages around it (new
messages still append; older history isn't paged back in).

## Remaining gaps
- Reactions (show / add / remove)
- Search
- Threads and forum channels
- Voice (needs DAVE E2EE since March 2026)
- Loading older history when scrolling up
- Stickers, polls, GIF picker, emoji picker
- Friends list and friend requests; blocking / unblocking from RustCord
- Email / 2FA / QR login

## Not in Discord Messenger (we'd write from scratch)
Reactions, search, voice (needs DAVE E2EE since March 2026), email/2FA login,
working QR login.
