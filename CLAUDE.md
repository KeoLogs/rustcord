# RustCord — notes for Claude

A native, customisable Discord client in Rust. UI is egui/eframe 0.37
(no webview). See README.md for the file layout.

## Architecture

- `src/main.rs` reads `DISCORD_TOKEN`, opens the window, wires two channels.
- `src/backend.rs` runs a Tokio runtime on its own thread. It owns the gateway
  task and handles `ToBackend` requests from the UI.
- `src/gateway.rs` is a hand-written gateway client (tokio-tungstenite):
  Hello -> Identify -> heartbeat loop, reconnect with backoff.
- `src/api.rs` is the REST client (reqwest).
- `src/app.rs` is the UI. It never blocks: it drains `ToUi` events in
  `App::logic` and sends `ToBackend` requests. `State::pane_ui` draws each
  panel (`Pane` enum).
- `src/layout.rs` is the egui_tiles layout: `Pane` enum, `default_tree()`
  (Discord-style), `LayoutBehavior`. The tree is saved via eframe storage.
  A new panel = new `Pane` variant + a case in `State::pane_ui` + a spot in
  `default_tree()`. Changing `Pane` can invalidate saved layouts (they fall
  back to the default if they fail to deserialize).
- `src/demo.rs` fakes the backend when `RUSTCORD_DEMO=1`.
- `src/types.rs` holds Guild/Channel/Message plus the `ToUi` / `ToBackend` enums.
  Payloads are parsed leniently from `serde_json::Value` because bot and user
  payloads differ.

Rule: network code lives in the backend; the UI only renders state and sends
requests. New features add variants to `ToUi` / `ToBackend` rather than doing
I/O in `app.rs`.

## egui 0.37 API notes

- `eframe::App` uses `fn ui(&mut self, ui: &mut egui::Ui, frame)` plus
  `fn logic(&mut self, ctx, frame)`; there is no `update`.
- Panels: `egui::Panel::top("id").show(ui, ...)`, `egui::Panel::bottom`,
  `egui::CentralPanel::default_margins().show(ui, ...)`.

## Before finishing any change

```sh
cargo fmt
cargo clippy -- -D warnings
cargo build
```

CI (`.github/workflows/build.yml`) runs fmt + clippy, then builds on Linux,
Windows and macOS. Keep it green.

## Testing limits in cloud sessions

Cloud sessions usually cannot reach Discord. They can still check the UI
with demo mode in a virtual display:

```sh
RUSTCORD_DEMO=1 LIBGL_ALWAYS_SOFTWARE=1 HOME=/tmp/rc-home \
  xvfb-run -s "-screen 0 1280x800x24" bash -c \
  './target/debug/rustcord & sleep 12; xdotool mousemove 70 85 click 1; sleep 2; import -window root /tmp/shot.png'
```

(`xdotool` drives clicks; drag slowly with small `mousemove` steps.) Images
show as red warning icons offline; that's expected. Then tell Leo exactly
what to check when he runs it locally against Discord.

## Hard rules

- Never commit tokens or `.env` files.
- No self-bot behaviour: nothing may send messages, react, join servers or
  take any other action from a user account automatically. Claude/AI features
  may read, summarise or draft, but the user always presses send.
- Licence is GPL-3.0-only.

## Working with Leo

- When showing code changes in chat, give full file contents, not diffs.
- Work on a branch and open a PR; don't push straight to `main`.
