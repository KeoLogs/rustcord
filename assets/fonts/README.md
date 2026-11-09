# Bundled fonts

| File | Font | Licence |
|---|---|---|
| NotoSans-Regular.ttf, NotoSans-SemiBold.ttf | Noto Sans | SIL OFL 1.1 (OFL.txt) |
| NotoSansSymbols-Regular.ttf | Noto Sans Symbols | SIL OFL 1.1 |
| NotoSansSymbols2-Subset.ttf | Noto Sans Symbols 2 (subset: arrows, technical, geometric shapes, dingbats, Braille, misc symbols) | SIL OFL 1.1 |
| NotoSansMath-Subset.ttf | Noto Sans Math (subset: math alphanumerics "fancy text", operators, letterlike, arrows) | SIL OFL 1.1 |
| NotoSansMono-Box.ttf | Noto Sans Mono (subset: box drawing + block elements) | SIL OFL 1.1 |
| Phosphor.ttf, Phosphor-Fill.ttf | Phosphor Icons (regular + fill) | MIT (Phosphor-LICENSE.txt) |

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
```

Everything else (CJK, Arabic, Hebrew, Thai, Devanagari, Korean, Tibetan…)
comes from the system fonts through eframe's system font fallback.
