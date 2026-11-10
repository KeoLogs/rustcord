# Bundled fonts

| File | Font | Licence |
|---|---|---|
| CascadiaMono.ttf | Cascadia Mono (Microsoft), the default UI font | SIL OFL 1.1 (CascadiaMono-OFL.txt) |
| NotoSans-Regular.ttf, NotoSans-SemiBold.ttf | Noto Sans | SIL OFL 1.1 (OFL.txt) |
| NotoSansSymbols-Regular.ttf | Noto Sans Symbols | SIL OFL 1.1 |
| NotoSansSymbols2-Subset.ttf | Noto Sans Symbols 2 (subset: arrows, technical, geometric shapes, dingbats, Braille, misc symbols) | SIL OFL 1.1 |
| NotoSansMath-Subset.ttf | Noto Sans Math (subset: math alphanumerics "fancy text", operators, letterlike, arrows) | SIL OFL 1.1 |
| NotoSansMono-Box.ttf | Noto Sans Mono (subset: box drawing + block elements; U+FE31, U+FE32 and U+FF5C mapped to the U+2502 glyph so Discord channel separators like "︱" render) | SIL OFL 1.1 |
| Phosphor.ttf, Phosphor-Fill.ttf | Phosphor Icons (regular + fill) | MIT (Phosphor-LICENSE.txt) |

Cascadia Mono © Microsoft Corporation (https://github.com/microsoft/cascadia-code),
Reserved Font Name "Cascadia Code". It is the unmodified variable font
(release v2407.24 from the official GitHub release, 714 KB, `wght` axis 200–700, default 400 = Regular).
egui reads variable fonts through skrifa: the default instance is the
Regular weight, and `theme::font_definitions` registers the same bytes a
second time with `wght = 600` (`FontTweak::coords`) as the SemiBold `bold`
family, so no second file is needed. Being unmodified, it keeps its name;
a subset or a static instance would be a Modified Version and must not be
called "Cascadia". Themes can switch to Noto Sans with
`font_family = "sans"`; code blocks use Cascadia Mono either way, and
Noto Sans + the symbol fonts below stay fallbacks for glyphs it lacks.

Noto fonts © The Noto Project Authors (https://github.com/notofonts).
Subsets were made with fontTools `pyftsubset`, e.g.:

```sh
pyftsubset NotoSansMath-Regular.ttf --layout-features='*' \
  --unicodes="U+1D400-1D7FF,U+2100-214F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+27C0-27FF,U+2900-2AFF,U+2580-259F,U+266A-266B" \
  --output-file=NotoSansMath-Subset.ttf
pyftsubset NotoSansSymbols2-Regular.ttf --layout-features='*' \
  --unicodes="U+2190-21FF,U+2300-23FF,U+2400-243F,U+25A0-27BF,U+2800-28FF,U+2B00-2BFF,U+1F000-1F0FF,U+1F780-1F7FF,U+1F800-1F8FF,U+1FB00-1FBFF,U+2010-2027,U+2030-205E" \
  --output-file=NotoSansSymbols2-Subset.ttf
pyftsubset NotoSansMono-Regular.ttf --unicodes="U+2500-259F" --output-file=NotoSansMono-Box.ttf
# then add cmap entries U+FE31, U+FE32, U+FF5C -> the glyph of U+2502
```

Everything else (CJK, Arabic, Hebrew, Thai, Devanagari, Korean, Tibetan…)
comes from the system fonts through eframe's system font fallback.
