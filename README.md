# RustCord

A native, customisable Discord client written in Rust. No webview, no Electron —
the UI is drawn with [egui](https://github.com/emilk/egui), so the layout is
entirely ours to change.

> ⚠️ **Discord ToS:** third-party clients are against Discord's Terms of Service.
> Test with a **bot token** or an **alt account**, never your main. Never use
> RustCord to send messages automatically from a user account (self-botting).

## Status

Usable day to day. Current version: see `Cargo.toml` and
[CHANGELOG.md](CHANGELOG.md) (x.y.z: x big update, y add-ons and
improvements, z fixes). It has:

- **Login screen**: paste a token (user or bot) or **scan a QR code** with the
  Discord app on your phone; optional "remember me" (kept in the OS
  credential store, see [below](#where-your-login-is-kept))
- **Several accounts** (up to 5 saved): click the user panel to switch;
  right-click an account → *Open in new window* to keep both live (each window
  is one more connection, ~15 MB + caches)
- **Four built-in themes** (Settings → Themes): **RustCord Dark** (default:
  every pane a bordered box with its name cut into the top border, square
  corners, avatars and badges, the bundled **Cascadia Mono** font),
  **RustCord Light**, and the classic flat **Discord Dark** / **Discord
  Light** (Noto Sans, round avatars)
- **GIFs autoplay** in messages and link previews (only on-screen ones keep
  their frames); animated avatars and emoji play on hover; a message that
  is just a GIF link shows only the GIF
- **No OS title bar**: – □ X sit in the top-right pane's header and follow
  when you move panes; drag empty header space to move the window,
  double-click to maximise, drag the edges to resize. Settings → Appearance →
  *Use system title bar* brings the normal bar back (macOS always keeps it)
- **Layout that never crops**: only the chat stretches; lists fold into
  drawers on narrow windows and the window can't shrink below what the layout
  needs
- **Threads**: a thread start is one card with a preview and a › button;
  threads open in a side panel next to the chat (or pop out into a big
  panel); the Threads popup has search, active and archived lists; you only
  join or create a thread when you click
- **Taskbar unread badge** (Windows): mentions + unread DMs as a red count on
  the taskbar icon, capped at 9+; muted servers don't count
- **Paste media** (Ctrl+V images or copied files into the upload cards; never
  sent until you press Enter) and **favourite GIFs** (star a GIF, Favourites
  tab); both are modules
- **Discord-style UI**: servers + profile bar on the left, channels, chat with
  message box, member list on the right
  - server rail: icons that widen smoothly into pills with the server name
    when you drag the rail wider, the white pill (unread / hover / selected),
    tooltips
  - channel list: collapsible categories (▾ / ▸), hover and selected rows,
    DMs with avatars and status; voice / stage channels listed (greyed out,
    joining comes later)
  - monochrome line icons everywhere (bundled Phosphor icon font), tinted
    with the theme's colours like Discord's
  - chat header with the channel topic, inbox, pinned messages and a
    button to hide / show the member list; « / » collapses the channel list
    (from Discord Messenger)
  - user panel like Discord's: avatar with status ring, name and custom
    status, gear; collapses to avatar + gear when narrow; click it for the
    status picker and the account switcher
  - messages: local-time timestamps ("Today at 19:05"), date lines between
    days, hover highlight, a floating Reply / Edit / ⋯ bar, Discord-style
    reply previews, mentions of you highlighted
  - message box: one flat row with + upload, GIF and emoji buttons; the
    "Replying to …" bar, "is typing…" line and upload cards float above it;
    files to upload show as cards with a spoiler toggle (`SPOILER_` file
    names) and remove button; the message text is the comment
  - stickers, forwarded messages, polls and system messages (joins, pins,
    calls, boosts, threads…) in Discord's style; anything RustCord can't
    draw yet says so instead of showing a blank message
  - full Unicode: bundled Noto Sans Symbols / Symbols 2 / Math / Mono subsets
    for symbols, "fancy text", Braille, box drawing and kaomoji, system fonts
    for other scripts (RTL and complex scripts are shaped)
- servers (with icons), DMs and group DMs, channels grouped by category
- live messages, edits and deletes; send, reply, edit, delete, copy
- images, file attachments and **uploads** (⊕ button or drag files onto the window)
- **GIF picker** (module): trending + Tenor search through Discord, like
  Discord's GIF button (user accounts)
- **emoji** in colour (Twemoji + custom server emoji); other scripts (CJK,
  Arabic, Cyrillic…) via system fonts
- typing indicators
- **unread markers**: bold channels with a white pill, red mention badges on
  channels and servers, a red "NEW" line in the chat; opening a channel marks it
  read, right-click a message → *Mark unread*, right-click a server → *Mark as
  read*; syncs with your other devices (user accounts)
- **mutes** from your Discord account: muted channels/servers are greyed out and
  don't badge or notify
- **notifications**: desktop notifications for mentions and DMs, taskbar flash,
  and an inbox of recent mentions (Settings → Notifications)
- **member list** like Discord's: grouped by hoisted role ("ONLINE — 12"),
  names in role colours, status dots (online / idle / do not disturb /
  offline), custom status or "Playing …" under the name, loads more as you
  scroll; compact mode in Settings → Appearance. Bots show who's recently
  active, or the full list if you enable the Server Members + Presence intents
- **role colours** for names in chat, server nicknames, status dots in the DM list
- **profile popouts**: click an avatar or name for a Discord-style card with
  banner colour, status, about me, account/server join dates, roles and your
  **note** (editable; saved only when you press Save); people's Nitro profile
  colours and avatar decorations show too (Settings → Appearance → *Show
  profile themes*)
- gateway **resume**: short disconnects pick up where they left off instead of
  reloading everything
- **Settings** (gear in the profile bar, or Ctrl+,): account, appearance,
  privacy & login, modules, layout, performance (live RAM usage), about.
  Changes wait in a draft until **Save Changes** (or Ctrl+S) on Discord's
  "Careful — you have unsaved changes!" bar; leaving with unsaved changes
  shakes the bar. The UI scale is applied on save, not while dragging.
- **keyboard shortcuts** (Ctrl+/ lists them): Alt+↑/↓ channels,
  Alt+Shift+↑/↓ unread channels, Ctrl+Alt+↑/↓ servers, Ctrl+K, Ctrl+,
- **Layout editor** (Settings → Layout → Edit layout): panels glow on hover,
  hold and drag anywhere to move (they grow as you pick them up), drag gaps
  to resize. Saved between runs; *Reset to default* and copy / import of a
  layout. There are no built-in preset pickers: one default look, everything
  else is yours.
- **Modules**: switch optional parts off to save memory (Settings → Modules),
  or on: e.g. the **Status bar** (connection, channel, typing, RAM; off by
  default)
- gateway **transport compression** (`zlib-stream`): a big account's READY is
  several times smaller on the wire
- **markdown** like Discord's: bold / italic / underline / strike, spoilers,
  inline and fenced code, quotes, headings, lists, `-#` subtext, masked links,
  `<t:…>` timestamps, user / role / channel mentions (click a user for their
  profile, a channel to go there); Settings → Chat → *Disable formatting*
- **embeds and link previews** (YouTube-style video cards, bot embeds with
  fields, colours, authors and footers); *Show embeds* / *Show images in
  embeds* in Settings → Chat (Embeds module)
- **link warnings**: links outside Discord ask first ("Leaving RustCord"),
  with "Trust this domain"; trusted domains are listed in Settings → Chat
- **pinned messages**: pins popup with Jump and Unpin; *Pin message* in the
  message menu where you're allowed to (DMs, or Manage Messages)
- **status picker**: click your avatar → Online / Idle / Do Not Disturb /
  Invisible and *Set custom status* (with "clear after"); synced with your
  account (user accounts)
- **server folders** in the rail, in your Discord order (click to open)
- **Ctrl+K quick switcher** for channels, DMs and servers
- **autocomplete** in the message box: `@` people / roles, `#` channels,
  `:` emoji (incl. server emoji); `@ ON / OFF` reply ping switch
- message menu: pin, copy message link, copy user ID, save image
- **blocked users'** messages hidden behind "N blocked messages — Show"
- **compact** message display (synced), 12-hour time, *Privacy & Safety*
  (DM scanning, DMs from server members; synced), leave server (right-click
  a server)
- opt-in **"Show as"** (Settings → Privacy & Safety): which platform icon
  friends see (Web by default; Desktop / Mobile make RustCord identify as
  Discord's official app, which misrepresents the client to Discord and may
  slightly raise the chance of your account being flagged)

- **custom themes** (Settings → Themes, "Custom themes" module): import a
  `.rctheme` file, a theme folder or a `.zip`; colours, rounded corners,
  square or round avatars and badges, shadows, gradients, glass panels, a
  chat background image. See [Themes](#themes) for the format
- **your own presets** (Settings → Presets, "Presets" module): save your
  layout, theme and modules as a preset card (layout thumbnail, theme name,
  how many modules change) and apply it with one click (Save Changes keeps
  it). Import / Export `.rcpreset` files; right-click Export (or a card's ⋯)
  to copy a preset as one-line code; paste preset or theme code and save it
  as a preset or a theme. See [Presets](#presets)

What's next: lower memory use (modules that are off cost almost nothing),
server settings and roles, the rest of the
[Discord Messenger](https://github.com/DiscordMessenger/dm) features
([docs/PORTING.md](docs/PORTING.md)), theme frames and detached panels, a
mini chat overlay, and **mods** (`.rcmod`, with permission prompts and no
access to your token). Voice comes last.

## Running

You need [Rust](https://rustup.rs) (stable).

```sh
cargo run            # opens the login screen
```

Or skip the login screen with an environment variable:

```sh
# Recommended for testing: a bot account in your own test server.
# The bot needs the MESSAGE CONTENT intent enabled in the Developer Portal.
DISCORD_TOKEN="Bot <your-bot-token>" cargo run

# User account (alt account recommended):
DISCORD_TOKEN="<your-user-token>" cargo run
```

Offline demo with fake data (no token, no Discord connection):

```sh
RUSTCORD_DEMO=1 cargo run
```

On Windows Command Prompt use `set "DISCORD_TOKEN=Bot <token>"` then `cargo run`.
On Windows PowerShell:

```powershell
$env:DISCORD_TOKEN = "Bot <your-bot-token>"; cargo run
```

On Linux you may need GUI dev packages first:
`sudo apt install libxkbcommon-dev libgtk-3-dev`.

**Never commit your token.** `.env` files are git-ignored.

### Where your login is kept

With "Remember me on this PC" on, RustCord keeps each saved account's token
in the operating system's credential store, never in its settings file:

- Windows: Credential Manager (Windows Credentials, entries named
  `RustCord` / `account:<user id>`).
- macOS: the login Keychain (service `RustCord`).
- Linux: the Secret Service (GNOME Keyring, KWallet, KeePassXC…). Without a
  Secret Service running, logins can't be remembered and you log in each time.

The settings file only lists the saved accounts' ids, names and avatars (up
to 5). Turning "Remember me" off in Settings removes every saved login;
logging out removes that account's entry. `DISCORD_TOKEN` is never saved.
Versions before this change kept the token in the settings file; it is moved
into the credential store once, on the first start, and removed from the file.

## Themes

A theme is a folder with a `theme.rctheme` file (TOML, `format = 1`) and an
optional background image. The format is documented at the top of
[`src/theme_file.rs`](src/theme_file.rs). Installed themes live in
RustCord's data folder under `themes/<id>/` (Settings → Themes → *Open
themes folder*).

RustCord has four built-in themes (Settings → Themes; they can't be
deleted): **RustCord Dark**, the default (blocky panes, each a flat dark
box with a 1 px border and its name cut into the top border, small gaps
between them, square corners, avatars and badges, a monospace font;
*Reset to default* goes back to it), **RustCord Light** (the same in light
colours), and **Discord Dark** / **Discord Light** (the classic flat
Discord look: no borders, Noto Sans, round avatars and badges). Every other
theme is a file you import. `examples/themes/` holds `discord-classic` and
`discord-light` (the same as the Discord built-ins, to copy and edit) and
the **test themes** `2000s-messenger`, `frosted-glass` and `frutiger-aero`.
They aren't installed by default, aren't compiled into the binary and aren't
part of release builds. Import one with Settings → Themes → *Import
folder…* (or *Import theme…* and pick its `theme.rctheme`).

## Presets

A preset (`.rcpreset`) is one JSON file: a manifest (name, author,
description), the layout (the same text as Settings → Layout → Copy layout),
the theme (`null` for the default look, else its id, name and the
`.rctheme` text so it travels with the preset) and the module switches. It
never holds account data, tokens or server / channel ids; unknown fields are
refused. Files are capped at 192 KB, theme ids must be plain `[a-z0-9-]`
names, and themes that need a background image are only referenced by id
(import their folder or `.zip` separately). Saved presets live in RustCord's
data folder under `presets/<id>.rcpreset`. The format is documented at the
top of [`src/presets.rs`](src/presets.rs). "Copy preset code" gives
`rcpreset1:` + base64 of the JSON; "Copy theme code" gives `rctheme1:` +
base64 of the `.rctheme` text.

## Layout of the code

| File | What it does |
| --- | --- |
| `src/main.rs` | Reads the token, opens the window, wires up channels |
| `src/app.rs` | The UI state and the four panels: servers, channels, messages, message box |
| `src/layout.rs` | Movable/resizable panel layout (egui_tiles), default layout, saving |
| `src/demo.rs` | Offline demo mode with fake data |
| `src/login.rs` | Login screen |
| `src/settings.rs` | Settings page |
| `src/config.rs` | Saved settings and the module list |
| `src/theme.rs` | Discord colours, bundled font (Noto Sans) and the `bold` font family |
| `src/theme_file.rs` | The `.rctheme` format: parsing, validation, fallbacks to the base theme |
| `src/themes.rs` | Theme folder (import / delete / list), the active theme, gradient / glass / background painting |
| `src/themes_page.rs` | Settings → Themes page: theme cards with previews, import, delete |
| `src/presets.rs` | The `.rcpreset` format, preset / theme codes, the presets folder |
| `src/presets_page.rs` | Settings → Presets page: preset cards, save / import / export, paste box |
| `src/builtin_themes.rs` | The four built-in themes (RustCord Dark / Light, Discord Dark / Light) |
| `src/anim.rs` | GIF autoplay: animated URIs, hover-to-play avatars / emoji, freeing off-screen GIFs |
| `src/timefmt.rs` | Local-time message timestamps and date-divider labels |
| `src/emoji.rs` | Splits text into text + emoji images |
| `src/markdown.rs` | Discord markdown: parser, renderer, parse cache |
| `src/embed.rs` | Embeds / link previews: parsing and drawing |
| `src/links.rs` | Trusted domains and the "Leaving RustCord" link warning |
| `src/pins.rs` | Pinned messages popup |
| `src/perms.rs` | Permission bits (Manage Messages for pinning) |
| `src/settings_proto.rs` | Discord's synced settings (protobuf): status, folders, privacy, compact |
| `src/status.rs` | Status picker and custom status dialog, op 3 presence |
| `src/quick_switcher.rs` | Ctrl+K switcher with fuzzy search |
| `src/autocomplete.rs` | `@` / `#` / `:` suggestions in the message box |
| `src/read_state.rs` | Unread / mention counts per channel |
| `src/notify.rs` | Mutes, mention detection, notification rules, inbox, desktop popups |
| `src/members.rs` | Roles + name colours, member cache, member list (op 14 / GUILD_MEMBER_LIST_UPDATE), list UI |
| `src/presence.rs` | Online status and activities |
| `src/profile.rs` | Profiles, notes and the profile popout card |
| `src/backend.rs` | Tokio runtime thread; runs the gateway and handles UI requests |
| `src/gateway.rs` | WebSocket connection to Discord's gateway (identify / resume, zlib-stream) |
| `src/api.rs` | REST calls: history, send / edit / delete, typing, acks, profiles, notes, pins, settings, leave server |
| `src/types.rs` | Guild / Channel / Message types and UI↔backend messages |
| `src/icons.rs` | Phosphor icon codepoints (monochrome icon font) |
| `src/widgets.rs` | Shared Discord-style widgets: menu rows, icon buttons, menu frame |
| `src/sysmsg.rs` | System messages, stickers and forwarded messages |
| `src/gif_picker.rs` | GIF picker popup and Discord's GIF endpoints' parsing |
| `src/accounts.rs` | Saved accounts, tokens in the OS credential store (redacted `Token`), migration |
| `src/account_menu.rs` | Account switcher menu in the user panel |
| `src/remote_auth.rs`, `src/qr_login.rs` | QR code login (Discord remote auth) and its panel |
| `src/window_frame.rs` | – □ X in the top-right pane header, window drag and edge resize |
| `src/tile_fit.rs`, `src/responsive.rs` | Pane sizes, breakpoints, drawers, the window's minimum size |
| `src/threads.rs`, `src/forum.rs` | Threads store, thread cards and popup, the thread side panel; forums |
| `src/paste.rs` | Ctrl+V images / files into the upload cards |
| `src/badge.rs`, `src/taskbar.rs` | Unread counts shared by the rail and the Windows taskbar badge |
| `src/http_error.rs`, `src/ratelimit.rs` | Typed REST errors and the 429 "Slow down" handling |
| `src/storage.rs` | Storage ids (demo runs never touch your real settings) |

The UI and the network never block each other: the backend sends `ToUi`
events over a channel and calls `request_repaint()`, and the UI sends
`ToBackend` requests the other way.

## Roadmap ideas

- Mods (`.rcmod`): community add-ons such as a music controller, asking for
  permission before they do anything and never seeing your token
- Theme frames, ornaments and panels detached into their own windows
- Mini chat: one chat in a small always-on-top window
- Claude features: summarise a channel, explain a message, draft a reply
  (drafts are always sent by you, never automatically)

## Credits

Feature reference: [Discord Messenger](https://github.com/DiscordMessenger/dm)
by iProgramInCpp (MIT). Emoji graphics: [Twemoji](https://github.com/jdecked/twemoji)
(CC-BY 4.0). Fonts: [Cascadia Mono](https://github.com/microsoft/cascadia-code)
by Microsoft (SIL Open Font License 1.1, `assets/fonts/CascadiaMono-OFL.txt`),
[Noto Sans](https://github.com/notofonts/latin-greek-cyrillic)
and subsets of Noto Sans Symbols, Symbols 2, Math and Mono by the Noto Project
Authors, under the SIL Open Font License 1.1 (`assets/fonts/OFL.txt`). Icons:
[Phosphor Icons](https://phosphoricons.com) (MIT, `assets/fonts/Phosphor-LICENSE.txt`).
See `assets/fonts/README.md` for how the subsets were made.

## Licence

GPL-3.0-only. See [LICENSE](LICENSE).
