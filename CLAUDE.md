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
  connections; op 9 / close codes 4007/4009 reset it. When the UI drops its
  sender (log out, "Show as" restart) the gateway closes with code 1000
  (`SessionEnd::Shutdown`) so Discord ends that session at once.
- `src/api.rs` is the REST client (reqwest).
- `src/app.rs` is the UI. It never blocks: it drains `ToUi` events in
  `App::logic` and sends `ToBackend` requests. `State::pane_ui` draws each
  panel (`Pane` enum).
- `src/layout.rs` is the egui_tiles layout: `Pane` enum, `default_tree()`
  (Discord-style), `LayoutBehavior`. The tree is saved via eframe storage.
  A new panel = new `Pane` variant + a case in `State::pane_ui` + a spot in
  `default_tree()`. Changing `Pane` can invalidate saved layouts (they fall
  back to the default if they fail to deserialize).
- `src/layout_v3.rs` (saved column layout: `mode`, column order, user
  panel dock, rail style, stacked rail+sidebar, pixel widths; v2 tree
  migration, export/import, `load`/`save`) and `src/responsive.rs`
  (§3.2 size tables, §3.3 breakpoints with ±16 px hysteresis, `compute`
  width → docked/drawer/hidden columns) are pure and tested. Panes draw
  through `layout::draw_pane` / `draw_column` (clipped, container-agnostic).
- REST failures are a typed `http_error::HttpError` (status, Discord code);
  match on `http_status` / `is_unavailable` / `is_no_access`, never on
  error text. Its Display is user-facing; never show raw HTTP lines.
- `src/demo.rs` fakes the backend when `RUSTCORD_DEMO=1`. Demo runs use
  their own storage key (`storage::DEMO_APP_ID`, `src/storage.rs`), so they
  never read or write the real config, layout, window state or themes.
- `src/accounts.rs`: saved logins. Tokens live only in the OS credential
  store (`keyring`; `Vault`, `SecretStore` trait, `MemoryStore` for tests
  and demo), keyed `account:<user id>`; Config holds only `SavedAccount`
  metadata (max 5), `last_account` and `per_account` (Discord-side data;
  app settings stay shared). A token is an `accounts::Token` (redacted
  Debug, no Display/Serialize): never format it into logs, toasts or errors.
- Account switcher (`src/account_menu.rs`, user panel menu): left-click
  switches in place (`log_out(None, false)` → `start_session`), right-click
  "Open in new window" makes an `AccountWindow` (app.rs): a deferred
  viewport with its own `State`, layout copy and backend thread
  (`spawn_session`; the backend repaints only its own `ViewportId`). Both
  windows draw through `chat_screen` + `session_logic`. The main window
  syncs config / theme into each window every frame and takes back config
  updates; Settings always open in the main window. Closing a window drops
  its State, so the gateway closes with 1000. `window_ui` wraps each
  account window in `window_frame::begin_pass` / `end_pass` like the main
  window (frame state is per viewport), so its – □ X act on that window only.
- QR login: `src/remote_auth.rs` (Discord remote auth gateway v2, RSA-OAEP
  via aws-lc-rs; pure parse/crypto helpers are tested) runs on its own small
  thread while `src/qr_login.rs` (the login screen's "Scan QR code" panel)
  is open; dropping the panel closes the socket and frees the QR texture.
  The token arrives as `ToUi::QrLogin(QrEvent::Token)` and takes the same
  path as a pasted one. The user always approves on their phone.
- `src/config.rs` is the saved `Config` incl. `Modules` (on/off optional
  features). A module that's off must do no work and load nothing. New
  optional features should be modules (add to `Modules` + `MODULES`).
- `src/settings.rs` is the settings page, `src/login.rs` the login screen,
  `src/theme.rs` the Discord palette (use `Palette` colours, not hard-coded
  ones) and fonts, `src/emoji.rs` text/emoji segmentation (has unit tests).
- No preset pickers: one default layout (`default_tree`) with "Reset to
  default". The only exception: exactly four built-in themes
  (`src/builtin_themes.rs`, `Config::builtin_theme`): RustCord Dark (the
  default, what "Reset to default" applies), RustCord Light, Discord Dark,
  Discord Light. They show as cards in Settings → Themes, can't be deleted
  and work with the Custom themes module off. Don't add more; every other
  theme is a `.rctheme` file in `examples/themes/`, imported by the user.
  The Discord built-ins must match `examples/themes/discord-classic` /
  `discord-light` (tested). `layout_presets::Preset` is internal (default
  tree, v2 migration).
- Shapes: `.rctheme` `[style] shape` (badges, status dots, count boxes) and
  `avatar_shape` (`"square"` ≤ 2 px corners, or `"round"`). Read them via
  `theme::shape()` / `theme::avatar_shape()` (`Shape::radius`,
  `Shape::dot`); never hard-code a circle for an avatar, badge or dot.
- GIFs: `src/anim.rs`. Message / embed GIFs autoplay through
  `anim::gif_image` (only while on screen; `anim::sweep` at the end of
  every pass frees the rest, frame textures included). Avatars and emoji
  stay still and use `anim::on_hover`. Links that became a GIF embed are
  hidden (`embed::hide_gif_links`); "Copy Link" stays in the message menu.
- Fonts: Cascadia Mono (variable, the default UI font; a theme's
  `font_family = "sans"` picks Noto Sans instead) and Noto Sans Regular +
  SemiBold are bundled (`assets/fonts/`, OFL) and installed by
  `theme::install_fonts`; system fonts remain the fallback for other scripts. egui's `strong()` only changes colour — for real bold use
  `RichText::bold()` (`theme::Bold` trait) or `theme::bold_font(size)`.
  Stick to the sizes already in use (each size/family adds glyphs to the
  font atlas).
- Times: `Message::time` is unix seconds; format with `src/timefmt.rs`
  (local time zone via `chrono::Local`), never by slicing the ISO string.
  `timefmt::set_12_hour` (from Settings → Chat) switches every formatter,
  including markdown `<t:…>` timestamps; use `timefmt::hm()` for new ones.
- UI look (Discord-like): `layout.rs` sets per-panel margins (channels and
  messages draw their own full-width headers via `header_bar`). Server rail:
  `rail_item` via `widgets::RailButton` (icon fixed at the left; widening
  the rail stretches it into a pill and fades the name in, no snap).
  Layout sizing: `src/tile_fit.rs` (per-pane min/max, chat absorbs
  resizes, drawers below the `responsive::Tier` breakpoints). Messages: `message_row`
  (hover background via a placeholder shape, the hover bar is an `Area`
  whose rect is kept in egui memory), `reply_preview`, `date_divider`.
  The members header button hides the Members tile with egui_tiles
  visibility (`layout::toggle_pane`); its count boxes (`members::count_boxes`)
  sit right-aligned left of – □ X, bots only when exact. « in the channel header hides the
  Channels tile (» in the chat header brings it back); hidden state is saved
  with the layout. Members draws the same 48 px `header_bar` as the chat.
- Icons: `src/icons.rs` (Phosphor icon font, MIT, bundled; regular weight
  is a fallback of the proportional + bold families, filled weight via
  `icons::fill_font`). Never use emoji as UI icons (they render coloured).
  Shared widgets in `src/widgets.rs`: `menu_item` / `dot_item` /
  `menu_separator` / `menu_frame` for every context menu and popup
  (`egui::Popup::menu(..).frame(widgets::menu_frame(pal))`), `icon_button`.
  Colours only from `Palette` (a theme system will replace it);
  `Palette::menu_bg()` is the menu/tooltip/modal surface.
- Symbol fonts: `theme::SYMBOL_FONTS` (Noto Sans Symbols / Symbols 2 / Math /
  Mono-box subsets, see `assets/fonts/README.md`) come before egui's fonts;
  scripts come from eframe's system font fallback. #showcase in the demo is
  the Unicode test.
- Settings (`src/settings.rs`): edits go into `SettingsPage::draft` and are
  applied by the app on `SettingsAction::Save` (Save Changes / Ctrl+S) with
  the synced changes to PATCH. Never apply config or PATCH Discord while
  the user is still editing. Zoom applies on save; the theme previews live.
  Changing "Show as" restarts the session (fresh IDENTIFY, never RESUME).
- Messages that aren't plain text: `src/sysmsg.rs` (system lines by message
  `type`, stickers, forwards); `message_content` ends with an "unsupported
  content" line so no message renders blank.
- GIF picker module: `src/gif_picker.rs`, `ToBackend::LoadGifs` →
  `api.gifs` (Discord's `/gifs/trending` and `/gifs/search`, Tenor) →
  `ToUi::Gifs`. Textures are freed when the popup closes. The emoji button
  is a hook (`ComposerAction::OpenEmojiPicker`) for `src/emoji_picker.rs`.
  Favourites: the star on each GIF (`GifAction::ToggleFavourite` →
  `ConfigUpdate::FavouriteGif`) edits `Config::favourite_gifs` (local only,
  no settings-proto sync); the Favourites tab lists them.
  Careful: never call `ui`/`ctx` methods inside a `ctx.memory(|m| …)`
  closure — it deadlocks the UI thread.
- Paste media: `src/paste.rs`. Ctrl+V in the focused message box with an
  image (`Event::PasteImage`) or copied files (`arboard` file list; on
  Windows a V-key edge check, since a file-only clipboard gives egui no
  event) adds them to the upload cards; it never sends. Images are encoded
  to PNG by the backend (`ToBackend::SavePastedImage` → `ToUi::FilesPicked`).
  `State::add_files` refuses files over `paste::upload_limit` (Nitro from
  READY `premium_type`, server boosts from `Guild::premium_tier`) with a toast.
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
- Messages render through `src/markdown.rs`: `State::md_cache`
  (`ParseCache`, keyed by message id; reply snippets use `reply:<id>`) is
  filled in a first pass in `messages_pane`, then drawn with a `ChatLookup`
  (nicknames, role colours, channel names, `user_names` learned from
  messages). Use `markdown::to_plain_text` for any one-line text
  (notifications, inbox, pins, reply previews). Embeds (`src/embed.rs`) draw
  under the content when the Embeds module is on; their image URIs are freed
  with the message (`forget_message_images`).
- Every link a user clicks goes through `State::open_link` →
  `links::check` (trusted domains in `Config::trusted_domains`, the "Leaving
  RustCord" modal otherwise). Never call `ctx.open_url` on message data
  directly.
- Pins: `src/pins.rs` popup under the pin button (`State::pins`), `perms.rs`
  decides who may pin (DM, or Manage Messages from roles + overwrites).
  `State::jump_to` scrolls to a message (or loads `messages?around=`).
- Synced settings: `src/settings_proto.rs` (`UserSettings`, from READY or
  `ToBackend::LoadUserSettings`). Change them only from user actions via a
  `patch_*` → `apply_patch` (optimistic) → `ToBackend::PatchUserSettings`.
  Status changes also send op 3 (`ToBackend::SetPresence`,
  `status::presence_op`). The server rail uses `settings_proto::ordered_guilds`
  for folders. Synced compact mode lands in `Config::compact_messages` via
  `ConfigUpdate` (State only has `&Config`; the app applies
  `state.config_updates` each frame).
- `src/quick_switcher.rs` (Ctrl+K, fuzzy `score`/`rank`), `src/autocomplete.rs`
  (`find_query` / `apply` are pure + tested; candidates are gathered in
  `State::update_autocomplete`), `src/status.rs` (status menu, custom status
  dialog). Autocomplete and Embeds are modules.
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

## Versions

Every update pushed for Leo bumps `version` in `Cargo.toml` (x.y.z) and
adds a `CHANGELOG.md` entry. Leo's scheme: x = big update, y = add-ons and
improvements, z = bug and issue fixes. y resets to 0 when x goes up; z
resets to 0 when x or y goes up. Settings → About shows it.
Each update also refreshes `README.md` (Status list, code table, What's
next) so it matches what the app does.
- Default look (PR1b): blocky bordered panes with notch labels
  (`[style.frame]` in .rctheme), no OS title bar: – □ X live in the
  top-right pane header (`src/window_frame.rs`); "Use system title bar"
  restores the native frame. The old flat look is the built-in Discord Dark
  theme (same as `examples/themes/discord-classic`).

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
and Sam mentions you after 6 s. #welcome has the Batch C showcase (markdown,
embeds, links, an image, pins, blocked messages); the profile bar avatar
opens the status picker; Ctrl+K opens the switcher; #showcase has the
Unicode / stickers / system message tests. `RUSTCORD_DEMO_LAYOUT=alt` starts
the demo in a custom arrangement (like Leo's) to check other layouts. Then
tell Leo exactly what to check when he runs it locally against Discord.

## Hard rules

- Never commit tokens or `.env` files.
- No self-bot behaviour: nothing may send messages, react, join servers or
  take any other action from a user account automatically. Claude/AI features
  may read, summarise or draft, but the user always presses send.
- Licence is GPL-3.0-only.

## Working with Leo

- When showing code changes in chat, give full file contents, not diffs.
- Work on a branch and open a PR; don't push straight to `main`.
