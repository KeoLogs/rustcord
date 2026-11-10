# Changelog

Every update that reaches Leo bumps the version in `Cargo.toml` (shown in
Settings → About). Version x.y.z:
- x: big update (y and z reset to 0)
- y: add-ons and improvements (z resets to 0)
- z: bug and issue fixes

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
