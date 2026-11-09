# RustCord

A native, customisable Discord client written in Rust. No webview, no Electron —
the UI is drawn with [egui](https://github.com/emilk/egui), so the layout is
entirely ours to change.

> ⚠️ **Discord ToS:** third-party clients are against Discord's Terms of Service.
> Test with a **bot token** or an **alt account**, never your main. Never use
> RustCord to send messages automatically from a user account (self-botting).

## Status

Usable day to day. It has:

- **Login screen** (token, user or bot), optional "remember me"
- **Discord-style UI**: servers + profile bar on the left, channels, chat with
  message box, member list on the right; Discord dark/light theme
- servers (with icons), DMs and group DMs, channels grouped by category
- live messages, edits and deletes; send, reply, edit, delete, copy
- images, file attachments and **uploads** (➕ button or drag files onto the window)
- **emoji** in colour (Twemoji + custom server emoji); other scripts (CJK,
  Arabic, Cyrillic…) via system fonts
- typing indicators
- **Settings** (⚙ in the profile bar): account, appearance, privacy & login,
  modules, layout, performance (live RAM usage), about
- **Layout editor** (Settings → Layout → Edit layout): panels glow on hover,
  hold and drag to move (they grow as you pick them up), drag gaps to resize.
  Saved between runs.
- **Modules**: switch optional parts off to save memory (Settings → Modules)

Next up: see [docs/PORTING.md](docs/PORTING.md) — notifications, member list,
markdown, embeds, pins, settings sync. Voice comes last.

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
| `src/theme.rs` | Discord colours |
| `src/emoji.rs` | Splits text into text + emoji images |
| `src/backend.rs` | Tokio runtime thread; runs the gateway and handles UI requests |
| `src/gateway.rs` | WebSocket connection to Discord's gateway |
| `src/api.rs` | REST calls: channel history, send message |
| `src/types.rs` | Guild / Channel / Message types and UI↔backend messages |

The UI and the network never block each other: the backend sends `ToUi`
events over a channel and calls `request_repaint()`, and the UI sends
`ToBackend` requests the other way.

## Roadmap ideas

- Group channels by category, DMs list
- Markdown, embeds, images, custom emoji
- Theming (colours, fonts, layout presets) loaded from a config file
- Claude features: summarise a channel, explain a message, draft a reply
  (drafts are always sent by you, never automatically)
- Plugin system

## Credits

Feature reference: [Discord Messenger](https://github.com/DiscordMessenger/dm)
by iProgramInCpp (MIT). Emoji graphics: [Twemoji](https://github.com/jdecked/twemoji)
(CC-BY 4.0).

## Licence

GPL-3.0-only. See [LICENSE](LICENSE).
