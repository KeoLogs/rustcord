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
- `src/config.rs` is the saved `Config` incl. `Modules` (on/off optional
  features). A module that's off must do no work and load nothing. New
  optional features should be modules (add to `Modules` + `MODULES`).
- `src/settings.rs` is the settings page, `src/login.rs` the login screen,
  `src/theme.rs` the Discord palette (use `Palette` colours, not hard-coded
  ones) and fonts, `src/emoji.rs` text/emoji segmentation (has unit tests).
- No preset pickers: one default look (Discord Dark + `default_tree`) with
  "Reset to default" for layout and theme. Other themes (Discord Light
  included) are `.rctheme` files in `examples/themes/`, imported by the
  user; `layout_presets::Preset` is internal (default tree, v2 migration).
- Fonts: Noto Sans Regular + SemiBold are bundled (`assets/fonts/`, OFL) and
  installed by `theme::install_fonts`; system fonts remain the fallback for
  other scripts. egui's `strong()` only changes colour — for real bold use
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
  visibility (`layout::toggle_pane`), « in the channel header hides the
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
