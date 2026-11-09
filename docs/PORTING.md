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

## Batch A — read state & notifications
- READY `read_state`, `MESSAGE_ACK` (DI:2710), `PASSIVE_UPDATE_V1` (DI:3036)
- `POST channels/{c}/messages/{m}/ack` (DI:1474, manual=true for mark unread 1503),
  `POST read-states/ack-bulk` (DI:1543)
- Mute: `USER_GUILD_SETTINGS_UPDATE` (DI:2719, `core/state/UserGuildSettings.cpp`)
- Mentions + desktop notifications (`core/state/NotificationManager.cpp`)
- Settings: EnableNotifications, FlashOnNotification (OD:499-510)
- Gateway RESUME (op 6) — DM doesn't do this; we should.

## Batch B — member list & presence
- Op 14 subscribe `{guild_id, typing, activities, threads, channels:{id:[[0,99]]}}` (DI:1596-1637)
- `GUILD_MEMBER_LIST_UPDATE` SYNC/INSERT/DELETE/UPDATE/INVALIDATE (DI:2886, 3137-3212)
- `READY_SUPPLEMENTAL` merged_members/presences (DI:2357), `PRESENCE_UPDATE` (DI:3006)
- Op 8 + `GUILD_MEMBERS_CHUNK` (DI:199-250, 3062)
- Profile popout: `GET users/{id}/profile`, notes `GET/PUT users/@me/notes/{id}`
  (`core/state/ProfileCache.cpp`), role list (`windows/RoleList.cpp`)
- Setting: CompactMemberList

## Batch C — message richness
- Markdown (`core/text/FormattedText.cpp`) + DisableFormatting toggle
- Embeds (`windows/MessageList.cpp`) + ShowEmbedContent / ShowEmbedImages
- Link warning + TrustedDomains (`MessageList::ConfirmOpenLink` 1927, LS:215)
- Pins: `GET channels/{id}/pins`, `PUT/DELETE channels/{c}/messages/pins/{m}` (`windows/PinList.cpp`)
- Mention/emoji autocomplete (`windows/AutoComplete.cpp`, `core/utils/Emoji.cpp`)
- Context menu extras: mark unread, copy link, save image
- Use12HourTime, ShowBlockedMessages, ReplyMentionDefault

## Batch D — account & settings sync
- `PATCH users/@me/settings-proto/1` + `USER_SETTINGS_PROTO_UPDATE` (DI:1876, 2743,
  `core/config/SettingsManager.*`): status picker, custom status, guild folders,
  explicit content filter, DMs from server members, compact/cozy
- Remaining OD tabs: Window (start minimised, tray), Connection (custom API URL)
- Quick switcher Ctrl+K (`windows/QuickSwitcher.cpp`), leave guild (DI:1647)

## Not in Discord Messenger (we'd write from scratch)
Reactions, search, voice (needs DAVE E2EE since March 2026), email/2FA login,
working QR login.
