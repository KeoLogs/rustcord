//! Unicode symbol sets and "fancy text" for the picker's Symbols tab.
//!
//! Everything here is inserted as plain text (it's not emoji, so it can't
//! be a reaction). Glyphs come from Noto Sans plus the system font fallback;
//! a character no installed font has shows as a box.

/// A named set of single characters.
pub struct SymbolSet {
    pub name: &'static str,
    pub chars: &'static str,
}

/// Common symbol sets, in picker order. Whitespace is ignored.
pub const SETS: &[SymbolSet] = &[
    SymbolSet {
        name: "Arrows",
        chars: "← ↑ → ↓ ↔ ↕ ↖ ↗ ↘ ↙ ⇐ ⇑ ⇒ ⇓ ⇔ ⇕ ⟵ ⟶ ⟷ ⟸ ⟹ ⟺ ↩ ↪ ↺ ↻ \
                ⤴ ⤵ ➔ ➜ ➝ ➞ ➟ ➠ ➡ ➤ ⬅ ⬆ ⬇ ⇄ ⇅ ⇆ ↞ ↠ ↢ ↣ ⇠ ⇢",
    },
    SymbolSet {
        name: "Maths",
        chars: "± × ÷ ≈ ≠ ≡ ≤ ≥ ≪ ≫ ∞ √ ∛ ∑ ∏ ∫ ∮ ∂ ∆ ∇ ∈ ∉ ∋ ∩ ∪ ⊂ ⊃ ⊆ ⊇ ∧ ∨ ¬ ⊕ ⊗ \
                ∀ ∃ ∄ ∅ ∝ ∴ ∵ ∠ ⊥ ∥ ° ′ ″ ‰ ‱ ½ ⅓ ⅔ ¼ ¾ ⅛ ⁰ ¹ ² ³ ⁴ ⁵ ⁶ ⁷ ⁸ ⁹ ⁺ ⁻ ⁿ \
                ₀ ₁ ₂ ₃ ₄ ₅ ₆ ₇ ₈ ₉ α β γ δ ε θ λ μ π σ φ ω Ω",
    },
    SymbolSet {
        name: "Box drawing",
        chars: "─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼ ━ ┃ ┏ ┓ ┗ ┛ ┣ ┫ ┳ ┻ ╋ ═ ║ ╔ ╗ ╚ ╝ ╠ ╣ ╦ ╩ ╬ \
                ╭ ╮ ╯ ╰ ╱ ╲ ╳ ▀ ▄ █ ▌ ▐ ░ ▒ ▓ ▁ ▂ ▃ ▅ ▆ ▇",
    },
    SymbolSet {
        name: "Stars & shapes",
        chars: "★ ☆ ✦ ✧ ✩ ✪ ✫ ✬ ✭ ✮ ✯ ✰ ⁂ ❂ ❖ ◆ ◇ ◈ ● ○ ◎ ◉ ◌ ◍ ■ □ ▢ ▣ ▪ ▫ ▲ △ ▼ ▽ \
                ◀ ▶ ◁ ▷ ◢ ◣ ◤ ◥ ♠ ♣ ♥ ♦ ♤ ♧ ♡ ♢ ✓ ✔ ✗ ✘ ☐ ☑ ☒ • ◦ ‣ ⁃ ❤ ❥ ❦ ❧ ✿ ❀ ❁ ✾ ❄ ❅ ❆",
    },
    SymbolSet {
        name: "Currency",
        chars: "$ ¢ £ ¤ ¥ ₣ ₤ ₦ ₩ ₪ ₫ € ₭ ₮ ₱ ₲ ₴ ₵ ₸ ₹ ₺ ₼ ₽ ₾ ₿",
    },
    SymbolSet {
        name: "Kaomoji parts",
        chars: "ツ ッ ヅ ゞ ゝ ヽ ノ ﾉ ༼ ༽ ʕ ʔ ᴥ ʘ ಠ ಥ ◕ ‿ ◡ ω ε з Д ∀ ´ ｀ ≧ ≦ ⊙ ＾ \
                ゜ ﾟ ・ ･ ｡ ︵ ︶ ╹ ◔ ◑ ᵔ ᴗ ˘ ͜ ʖ ͡ ⌐ ¬ ˙ ⁰ ˚ ꒰ ꒱ ╥ ﹏ ≖ ಡ ಢ",
    },
    SymbolSet {
        name: "Punctuation & signs",
        chars: "« » ‹ › „ “ ” ‘ ’ ‚ — – … · † ‡ § ¶ © ® ™ ℠ № ℃ ℉ ¡ ¿ ‽ ⁇ ⁈ ※ ⁑ ⸮ \
                ♩ ♪ ♫ ♬ ♭ ♮ ♯ ☀ ☁ ☂ ☃ ☄ ☾ ☽ ✉ ✎ ✏ ✂ ☎ ⌘ ⌥ ⇧ ⌫ ⏎ ⚐ ⚑ ☮ ☯ ☢ ☣ ⚠ ♻ ⚡",
    },
];

/// Ready-made kaomoji (inserted whole).
pub const KAOMOJI: &[&str] = &[
    "¯\\_(ツ)_/¯",
    "(╯°□°)╯︵ ┻━┻",
    "┬─┬ノ( º _ ºノ)",
    "( ͡° ͜ʖ ͡°)",
    "ʕ•ᴥ•ʔ",
    "ಠ_ಠ",
    "(ノ◕ヮ◕)ノ*:･ﾟ✧",
    "(っ◔◡◔)っ",
    "(•‿•)",
    "(╥﹏╥)",
    "(≧▽≦)",
    "(｡•́︿•̀｡)",
    "ᕕ( ᐛ )ᕗ",
    "(⌐■_■)",
    "♪~ ᕕ(ᐛ)ᕗ",
    "(◕‿◕✿)",
];

/// The characters of a set.
pub fn chars(set: &SymbolSet) -> impl Iterator<Item = char> + '_ {
    set.chars.chars().filter(|c| !c.is_whitespace())
}

/// `U+1F600`, `u+2603`, `0x2603` or bare hex (`2603`, 2+ digits) → that
/// character, so any code point can be inserted.
pub fn parse_codepoint(s: &str) -> Option<char> {
    let s = s.trim();
    let hex = s
        .strip_prefix("U+")
        .or_else(|| s.strip_prefix("u+"))
        .or_else(|| s.strip_prefix("0x"))
        .unwrap_or(s);
    if hex.len() < 2 || hex.len() > 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(hex, 16).ok()?).filter(|c| !c.is_control())
}

// ---------------------------------------------------------------------------
// Fancy text
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FancyStyle {
    Bold,
    Italic,
    BoldItalic,
    Script,
    BoldScript,
    Fraktur,
    BoldFraktur,
    DoubleStruck,
    Sans,
    SansBold,
    SansItalic,
    Monospace,
    Fullwidth,
    Circled,
    SmallCaps,
}

impl FancyStyle {
    pub const ALL: [FancyStyle; 15] = [
        Self::Bold,
        Self::Italic,
        Self::BoldItalic,
        Self::Script,
        Self::BoldScript,
        Self::Fraktur,
        Self::BoldFraktur,
        Self::DoubleStruck,
        Self::Sans,
        Self::SansBold,
        Self::SansItalic,
        Self::Monospace,
        Self::Fullwidth,
        Self::Circled,
        Self::SmallCaps,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bold => "Bold",
            Self::Italic => "Italic",
            Self::BoldItalic => "Bold italic",
            Self::Script => "Script",
            Self::BoldScript => "Bold script",
            Self::Fraktur => "Fraktur",
            Self::BoldFraktur => "Bold fraktur",
            Self::DoubleStruck => "Double-struck",
            Self::Sans => "Sans",
            Self::SansBold => "Sans bold",
            Self::SansItalic => "Sans italic",
            Self::Monospace => "Monospace",
            Self::Fullwidth => "Fullwidth",
            Self::Circled => "Circled",
            Self::SmallCaps => "Small caps",
        }
    }

    /// Mathematical Alphanumeric Symbols block starts: (A, a, 0).
    fn math_base(self) -> Option<(u32, u32, Option<u32>)> {
        Some(match self {
            Self::Bold => (0x1D400, 0x1D41A, Some(0x1D7CE)),
            Self::Italic => (0x1D434, 0x1D44E, None),
            Self::BoldItalic => (0x1D468, 0x1D482, None),
            Self::Script => (0x1D49C, 0x1D4B6, None),
            Self::BoldScript => (0x1D4D0, 0x1D4EA, None),
            Self::Fraktur => (0x1D504, 0x1D51E, None),
            Self::DoubleStruck => (0x1D538, 0x1D552, Some(0x1D7D8)),
            Self::BoldFraktur => (0x1D56C, 0x1D586, None),
            Self::Sans => (0x1D5A0, 0x1D5BA, Some(0x1D7E2)),
            Self::SansBold => (0x1D5D4, 0x1D5EE, Some(0x1D7EC)),
            Self::SansItalic => (0x1D608, 0x1D622, None),
            Self::Monospace => (0x1D670, 0x1D68A, Some(0x1D7F6)),
            Self::Fullwidth | Self::Circled | Self::SmallCaps => return None,
        })
    }

    /// Letters that live in Letterlike Symbols instead (holes in the math
    /// block).
    fn exception(self, c: char) -> Option<char> {
        Some(match (self, c) {
            (Self::Italic, 'h') => 'ℎ',
            (Self::Script, 'B') => 'ℬ',
            (Self::Script, 'E') => 'ℰ',
            (Self::Script, 'F') => 'ℱ',
            (Self::Script, 'H') => 'ℋ',
            (Self::Script, 'I') => 'ℐ',
            (Self::Script, 'L') => 'ℒ',
            (Self::Script, 'M') => 'ℳ',
            (Self::Script, 'R') => 'ℛ',
            (Self::Script, 'e') => 'ℯ',
            (Self::Script, 'g') => 'ℊ',
            (Self::Script, 'o') => 'ℴ',
            (Self::Fraktur, 'C') => 'ℭ',
            (Self::Fraktur, 'H') => 'ℌ',
            (Self::Fraktur, 'I') => 'ℑ',
            (Self::Fraktur, 'R') => 'ℜ',
            (Self::Fraktur, 'Z') => 'ℨ',
            (Self::DoubleStruck, 'C') => 'ℂ',
            (Self::DoubleStruck, 'H') => 'ℍ',
            (Self::DoubleStruck, 'N') => 'ℕ',
            (Self::DoubleStruck, 'P') => 'ℙ',
            (Self::DoubleStruck, 'Q') => 'ℚ',
            (Self::DoubleStruck, 'R') => 'ℝ',
            (Self::DoubleStruck, 'Z') => 'ℤ',
            _ => return None,
        })
    }

    fn map(self, c: char) -> char {
        if let Some(e) = self.exception(c) {
            return e;
        }
        let shift = |base: u32, from: char| char::from_u32(base + (c as u32 - from as u32));
        let mapped = match self {
            Self::Fullwidth => match c {
                '!'..='~' => char::from_u32(c as u32 - 0x21 + 0xFF01),
                ' ' => Some('\u{3000}'),
                _ => None,
            },
            Self::Circled => match c {
                'A'..='Z' => shift(0x24B6, 'A'),
                'a'..='z' => shift(0x24D0, 'a'),
                '0' => Some('⓪'),
                '1'..='9' => shift(0x2460, '1'),
                _ => None,
            },
            Self::SmallCaps => c
                .is_ascii_lowercase()
                .then(|| {
                    "ᴀʙᴄᴅᴇꜰɢʜɪᴊᴋʟᴍɴᴏᴘǫʀꜱᴛᴜᴠᴡxʏᴢ"
                        .chars()
                        .nth((c as u8 - b'a') as usize)
                })
                .flatten(),
            _ => {
                let (upper, lower, digits) = self.math_base().unwrap_or((0, 0, None));
                match c {
                    'A'..='Z' => shift(upper, 'A'),
                    'a'..='z' => shift(lower, 'a'),
                    '0'..='9' => digits.and_then(|d| shift(d, '0')),
                    _ => None,
                }
            }
        };
        mapped.unwrap_or(c)
    }
}

/// `text` in a fancy style; characters the style has no form for (accents,
/// punctuation, other scripts) stay as they are.
pub fn fancy(text: &str, style: FancyStyle) -> String {
    text.chars().map(|c| style.map(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fancy_styles() {
        assert_eq!(fancy("abc", FancyStyle::BoldScript), "𝓪𝓫𝓬");
        assert_eq!(fancy("abc", FancyStyle::Bold), "𝐚𝐛𝐜");
        assert_eq!(fancy("abc", FancyStyle::DoubleStruck), "𝕒𝕓𝕔");
        assert_eq!(fancy("Hi 42!", FancyStyle::Bold), "𝐇𝐢 𝟒𝟐!");
        assert_eq!(fancy("Hello", FancyStyle::Script), "ℋℯ𝓁𝓁ℴ");
        assert_eq!(fancy("hah", FancyStyle::Italic), "ℎ𝑎ℎ");
        assert_eq!(fancy("CR", FancyStyle::DoubleStruck), "ℂℝ");
        assert_eq!(fancy("Zed", FancyStyle::Fraktur), "ℨ𝔢𝔡");
        assert_eq!(fancy("code 1", FancyStyle::Monospace), "𝚌𝚘𝚍𝚎 𝟷");
        assert_eq!(fancy("Ab 1", FancyStyle::Fullwidth), "Ａｂ\u{3000}１");
        assert_eq!(fancy("Ab09", FancyStyle::Circled), "Ⓐⓑ⓪⑨");
        assert_eq!(fancy("leo", FancyStyle::SmallCaps), "ʟᴇᴏ");
        // Untouched: other scripts, accents, punctuation.
        assert_eq!(fancy("é ë ж ?", FancyStyle::Bold), "é ë ж ?");
    }

    #[test]
    fn every_style_maps_letters() {
        for style in FancyStyle::ALL {
            let out = fancy("abcdefghijklmnopqrstuvwxyz", style);
            assert_eq!(out.chars().count(), 26, "{style:?}");
            let changed = out.chars().zip('a'..='z').filter(|(a, b)| a != b).count();
            // Small caps keeps 'x' (no small-cap form).
            assert!(changed >= 25, "{style:?}: {out}");
        }
    }

    #[test]
    fn codepoints() {
        assert_eq!(parse_codepoint("U+2603"), Some('☃'));
        assert_eq!(parse_codepoint("u+1f600"), Some('😀'));
        assert_eq!(parse_codepoint("0x41"), Some('A'));
        assert_eq!(parse_codepoint("2603"), Some('☃'));
        assert_eq!(parse_codepoint("hello"), None);
        assert_eq!(parse_codepoint("a"), None);
        assert_eq!(parse_codepoint("U+D800"), None); // surrogate
        assert_eq!(parse_codepoint("U+0007"), None); // control
        assert_eq!(parse_codepoint("U+1234567"), None);
    }

    #[test]
    fn sets_have_no_duplicates_within() {
        for set in SETS {
            let all: Vec<char> = chars(set).collect();
            let mut uniq = all.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(all.len(), uniq.len(), "{}", set.name);
        }
    }
}
