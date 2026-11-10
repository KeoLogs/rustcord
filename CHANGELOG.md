# Changelog

Every update that reaches Leo bumps the version in `Cargo.toml` (shown in
Settings → About). Version x.y.z:
- x: big update (y and z reset to 0)
- y: add-ons and improvements (z resets to 0)
- z: bug and issue fixes

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
