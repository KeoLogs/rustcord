# RustCord

A native, customisable Discord client written in Rust. No webview, no Electron —
the UI is drawn with [egui](https://github.com/emilk/egui), so the layout is
entirely ours to change.

> ⚠️ **Discord ToS:** third-party clients are against Discord's Terms of Service.
> Test with a **bot token** or an **alt account**, never your main. Never use
> RustCord to send messages automatically from a user account (self-botting).

## Status

Early but usable. It can:

- connect as a bot or user account, auto-reconnect with backoff
- show servers (with icons), DMs and group DMs, channels grouped by category
- load history, show new messages live, plus live edits and deletes
- send, reply to, edit and delete messages; copy message text
- show images and attachments inline (click an image for full size), avatars
- typing indicators, both ways
- **movable, resizable layout**: Discord-style by default. Drag the lines
  between panels to resize. Turn on **✏ Edit layout** to drag panels by their
  title bar (drop on an edge to dock, or on another panel to stack as tabs).
  The layout is saved; **↺ Reset layout** restores the default.

Not yet: markdown, mentions, custom emoji, embeds, member list, pins,
settings, plugins, voice.

## Running

You need [Rust](https://rustup.rs) (stable).

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

## Licence

GPL-3.0-only. See [LICENSE](LICENSE).
