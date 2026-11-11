# Changelog

Every update that reaches Leo bumps the version in `Cargo.toml` (shown in
Settings → About). Version x.y.z:
- x: big update (y and z reset to 0)
- y: add-ons and improvements (z resets to 0)
- z: bug and issue fixes

## Unreleased
- Upload progress: a message with attachments shows a progress bar per
  file while it uploads, and a Cancel link. A cancelled upload shows as
  not sent with Retry and Delete. Files are now streamed from disk instead
  of being read into memory whole.
- Notification sound: RustCord's own short ping plays with each
  notification. Settings → Notifications has the on/off switch, a volume
  slider, a Test button and "Browse…" for your own sound (wav, ogg or mp3,
  1 MB at most). Audio is only loaded while a sound plays.
- Update check: once a day at most, RustCord asks GitHub whether a newer
  release exists and shows a small "Update available" notice with
  "Open release page" and "Dismiss". Nothing is downloaded or installed.
  Settings → About → "Check for updates" turns it off.
- Disk image cache: pictures you've already seen are kept on this PC (up to
  200 MB, least recently used deleted first) so they aren't downloaded
  again. Settings → Performance has the size (0 = off) and a
  "Clear cache (X MB)" button. It uses disk space, not RAM.

## 0.10.0: DM core (history paging, first unread, optimistic send)
- Chat history like Discord's (Brief A): scrolling near the top loads 50
  older messages at a time and the view stays where it was; a "Load more"
  row marks messages missing between two loaded batches; a "Jump to present"
  bar shows when you're well above the newest message. Channels open at the
  first unread message under the red NEW line (at the bottom if there's no
  read state). Sent messages show at once, faded, until Discord confirms
  them (matched by nonce); a failed send turns red with Retry and Delete
  (never retried on its own). Clicking a discord.com message link opens it
  inside RustCord. Also: a character counter near the limit (2,000, or 4,000
  with Nitro), Copy Image / Copy Link on an image's right-click menu, a
  Reconnect button in the connection status, and the Discord Messenger
  credit in Settings → About.

## 0.9.0: Your own presets
- Your own presets (Settings → Presets, "Presets" module): save your layout,
  theme and modules as a preset, apply it from its card (the card shows the
  theme, a layout thumbnail and how many modules change; Save Changes keeps
  it), delete with a confirm, import / export `.rcpreset` files, copy a
  preset or theme as code, and paste preset or theme code to save it as a
  preset or a theme. Presets never contain account data. The demo has two
  example presets.

## 0.8.0: Looks
- Four built-in themes in Settings → Themes, shown as cards next to your
  imported ones, each with Apply; they can't be deleted:
  - **RustCord Dark** (the default): bordered panes with labels,
    monospace, square corners.
  - **RustCord Light**: the same blocky look in light colours.
  - **Discord Dark** and **Discord Light**: the classic flat Discord look
    with Noto Sans and round shapes.
  "Reset to default" goes back to RustCord Dark. They work with the Custom
  themes module off. Old settings load as RustCord Dark.
- GIFs play by themselves in messages and link previews (Tenor, Giphy,
  Klipy and plain `.gif` files), at the same size as before. Only GIFs on
  screen keep their frames in memory; scrolled-away ones are freed.
  Animated avatars and emoji stay still until you hover them.
- A message that is only a GIF link shows just the GIF: the link text is
  hidden (other words in the message stay). Right-click → *Copy Link*
  still copies it.
- Square everything in the RustCord themes: avatars (user panel, members,
  DMs, chat, profile card), server icons and the DM button, mention and
  unread badges, the taskbar badge and status dots. The Discord themes keep
  them round. Themes choose with the new `.rctheme` tokens
  `[style] shape` and `avatar_shape` (`"square"` or `"round"`).
- Members header: the online / offline counts are small square boxes in
  their own spot left of – □ X. Hover one and it widens smoothly to read
  "7 online". A blue bots box shows when the count is exact (the whole
  list is loaded); otherwise it's left out rather than showing a partial
  number.
- The window buttons now stay in a narrow Members header instead of
  floating over it.

## 0.7.0: Memory measuring tools
- Settings → Performance has a memory details panel.
- `scripts/memprobe.ps1` measures memory (Task Manager numbers) and
  frame-time p95, with module profiles and a live read-only mode.
- `RUSTCORD_DEMO_BIG=1` loads a big synthetic account through the real
  READY path; `RUSTCORD_START_MINIMIZED` / `RUSTCORD_START_OFFSCREEN` for
  test launches.
- Release builds use `panic = "abort"` (about 10 MB less RAM, smaller exe).
  A crash in a background task now closes the app.

## 0.6.1: Composer and user panel match the blocky look
- The message box has no bubble any more: one flat row with a plain +,
  the message field, and GIF and emoji icons on the right.
- The message box and the user panel are the same compact height, so the
  bottom row lines up. A message box saved at the old height shrinks once;
  one you made taller keeps its size.
- Replying works again: the "Replying to" bar (and who's typing) floats
  above the message box instead of being cut off inside it. X or Esc
  cancels. Same in the thread panel.

## 0.6.0: Taskbar unread badge
- A red count over RustCord's taskbar icon: your mentions plus unread DMs,
  "9+" past nine. Muted servers and channels don't count; it is always the
  sum of the red badges on the server rail.
- Each account window shows its own account's count on its own icon.
- Turn it off in Settings → Notifications ("Show unread badge on taskbar
  icon"). Off means nothing is created or called.
- Only Windows shows it for now; other systems do nothing yet.

## 0.5.0: Threads (PR1c)
- A thread start is one card in the chat: the thread name, a preview of the
  last message (or the message count) and a › to open it.
- Threads popup (header button): search, active and archived threads, and
  our own empty state when a channel has none.
- An opened thread shows as a side panel next to its channel, with a button
  to pop it out into the big panel. Under 1000 px wide it always opens big.
- Threads you haven't joined show a join bar; joining happens only when you
  click it.
- Archived or locked threads show a read-only note instead of the message
  box.
- Threads are only created when you click Create (message menu or the
  popup).

## 0.4.0: Multiple accounts (PR2)
- Tokens live in the OS credential store, not the settings file; an old
  saved token moves there once.
- Account switcher in the user panel: left-click switches, right-click opens
  the account in its own window or removes it. Up to 5 saved accounts.
- QR code login (approve on your phone).
- Account windows use the same frame as the main window (– □ X in the
  top-right header, drag and edge resize); their X closes only that window.
  Each one keeps its own minimum size for its own layout.

## 0.3.1: Resize fix
- The window can't shrink below what the layout needs (borders and labels
  included), so short windows no longer squash the chat into a strip; lists
  shrink first, the composer and user panel last.

## 0.3.0: Blocky default look (PR1b)
- New default look: bordered panes with small notch labels, near-square
  corners, monospace font (Cascadia Mono). Themes can switch it off
  (`[style.frame]`, `font_family`). The old look is
  `examples/themes/discord-classic`.
- No separate title bar: – □ X sit in the top-right pane header and follow
  tile moves; drag empty header space to move, double-click to maximise,
  edges resize. Settings → Appearance → "Use system title bar" brings the
  Windows bar back. macOS keeps its native bar.

## 0.2.0: Release B, Phase 1 (PR #9)
- Layout resizing fixed (`tile_fit`): only the chat stretches, drawers below
  1200 / 700 px, the free tile editor stays.
- Demo runs use their own storage.
- Typed HTTP errors; hidden channels you can't read.
- Images, embeds and long names fit the chat.
- Search renders markdown, Friends shows Active Now.
- Paste media (Ctrl+V) and favourite GIFs, both switchable in Modules.
- Server rail widens smoothly into pills.
- Preset pickers removed: one default look plus Reset to default; Discord
  Light is an example theme.
- Frame drag on themes with inset cards.
- Profile cards show profile themes and avatar decorations (toggleable).
- Buttons have hover, pressed and pointer states.

## 0.1.0: Release A (PR #8)
- First tracked version.
