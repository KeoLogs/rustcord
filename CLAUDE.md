# RustCord — notes for Claude

A native, customisable Discord client in Rust. UI is egui/eframe 0.37
(no webview). See README.md for the file layout.

## Architecture

- `src/main.rs` reads `DISCORD_TOKEN`, opens the window, wires two channels.
- `src/backend.rs` runs a Tokio runtime on its own thread. It owns the gateway
  task and handles `ToBackend` requests from the UI.
- `src/gateway.rs` is a hand-written gateway client (tokio-tungstenite):
  Hello -> Identify (or op 6 Resume) -> heartbeat loop, reconnect with backoff.
  Transport compression is `compress=zlib-stream`: binary frames are fed to
  one `Inflater` per connection (new one on every reconnect / resume); a
  message is complete when the bytes end in `00 00 FF FF`.
  `Session` (session_id, resume_gateway_url, seq) lives in the backend across
  connections; op 9 / close codes 4007/4009 reset it.
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
- `src/config.rs` is the saved `Config` incl. `Modules` (on/off optional
  features). A module that's off must do no work and load nothing. New
  optional features should be modules (add to `Modules` + `MODULES`).
- `src/settings.rs` is the settings page, `src/login.rs` the login screen,
  `src/theme.rs` the Discord palette (use `Palette` colours, not hard-coded
  ones) and fonts, `src/emoji.rs` text/emoji segmentation (has unit tests).
- Fonts: Noto Sans Regular + SemiBold are bundled (`assets/fonts/`, OFL) and
  installed by `theme::install_fonts`; system fonts remain the fallback for
  other scripts. egui's `strong()` only changes colour — for real bold use
  `RichText::bold()` (`theme::Bold` trait) or `theme::bold_font(size)`.
  Stick to the sizes already in use (each size/family adds glyphs to the
  font atlas).
- Times: `Message::time` is unix seconds; format with `src/timefmt.rs`
  (local time zone via `chrono::Local`), never by slicing the ISO string.
- UI look (Discord-like): `layout.rs` sets per-panel margins (channels and
  messages draw their own full-width headers via `header_bar`). Server rail:
  `rail_item` (icon-only below `RAIL_NAMES_WIDTH`). Messages: `message_row`
  (hover background via a placeholder shape, the hover bar is an `Area`
  whose rect is kept in egui memory), `reply_preview`, `date_divider`.
  The 👥 header button hides the Members tile with egui_tiles visibility
  (`layout::toggle_members`); hidden state is saved with the layout.
  Careful: never call `ui`/`ctx` methods inside a `ctx.memory(|m| …)`
  closure — it deadlocks the UI thread.
- `src/read_state.rs`: per-channel unread/mention counts (`ReadState`).
  `src/notify.rs`: mutes (`Mutes`), `mentions_me`, `should_notify`, the
  mentions `Inbox`, and `show_desktop` (notify-rust; called from the backend
  on a blocking thread via `ToBackend::Notify`). Both are pure logic with
  unit tests; `app.rs` wires them up (`track_new_message`, `ack_open_channel`).
  Acks only ever happen because the user opened/is viewing a channel or
  picked a menu item; bots skip ack requests (no read state on Discord).
- `src/members.rs`: roles (`role_color`, `hoisted_role`), `MemberCache`
  (nick + roles per server, capped), the op 14 member list (`MemberList`
  applies GUILD_MEMBER_LIST_UPDATE ops), op 8 builders, `build_rows` (bots'
  locally grouped list) and the list UI. `src/presence.rs`: `Presences`
  (status + activity, capped). `src/profile.rs`: `ProfileCache` (small MRU)
  and the popout card. Gateway ops from the UI go `ToBackend` → backend →
  an mpsc into `gateway::run_session`, sent only after READY/RESUMED. The
  Member list module gates op 14 and presence storage.
- `docs/PORTING.md` is the roadmap of features to port from Discord
  Messenger (MIT, credit kept in README/About).

## Memory

Low RAM is a goal. Renderer is glow (not wgpu). Messages per channel and
cached channels are capped (Config); evicted channels' images are freed with
`ctx.forget_image`. Check `Settings → Performance` or `/proc/<pid>/status`
VmRSS in demo mode before/after big changes.
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
cargo test
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

(`xdotool` drives clicks; drag slowly with small `mousemove` steps. Before
typing with `xdotool type`, focus the window first:
`xdotool windowfocus $(xdotool search --name RustCord | head -1)`.) Use a
release build for RAM numbers. Images show as red warning icons offline;
that's expected. The demo has two people start typing in #general after 4 s
and Sam mentions you after 6 s. Then tell Leo exactly
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
