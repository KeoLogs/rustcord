# Changelog

Every update that reaches Leo bumps the version in `Cargo.toml` (shown in
Settings → About and the API user agent): the minor number for each PR /
feature batch, the patch number for fix-only pushes.

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
