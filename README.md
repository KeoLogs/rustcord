# RustCord

A native, customisable Discord client written in Rust. No webview, no Electron —
the UI is drawn with [egui](https://github.com/emilk/egui), so the layout is
entirely ours to change.

> ⚠️ **Discord ToS:** third-party clients are against Discord's Terms of Service.
> Test with a **bot token** or an **alt account**, never your main. Never use
> RustCord to send messages automatically from a user account (self-botting).

## Status

Early scaffold. Currently it can:

- connect to the Discord gateway (heartbeat, identify, auto-reconnect with backoff)
- list your servers along the top, and text channels as tabs below them
- load the last 50 messages of a channel and show new ones live
- send messages

Not yet: DMs, categories, markdown, embeds, attachments, emoji, replies, voice.

## Running

You need [Rust](https://rustup.rs) (stable).

```sh
# Recommended for testing: a bot account in your own test server.
# The bot needs the MESSAGE CONTENT intent enabled in the Developer Portal.
DISCORD_TOKEN="Bot <your-bot-token>" cargo run

# User account (alt account recommended):
DISCORD_TOKEN="<your-user-token>" cargo run
```

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
| `src/app.rs` | The egui UI: server bar, channel tabs, messages, composer |
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
