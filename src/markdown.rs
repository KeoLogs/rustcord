//! Discord-flavoured markdown: a pure parser, an egui renderer and a small
//! parse cache.
//!
//! The parser follows Discord's rules (which come from simple-markdown) and
//! was cross-checked against Discord Messenger's `FormattedText.cpp` (MIT):
//!
//! - Blocks: paragraphs (newlines are kept), `> ` / `>>> ` quotes, `#`/`##`/
//!   `###` headings, `-# ` subtext, `-`/`*`/`1.` lists (indent to nest) and
//!   fenced code blocks (```` ```lang ````), which may also start mid-line.
//! - Inline: `**bold**`, `*italic*`/`_italic_`, `__underline__`,
//!   `~~strike~~`, `||spoiler||`, `` `code` ``, bare and `<autolinked>`
//!   URLs, `[masked](https://links)`, `<@user>`, `<@&role>`, `<#channel>`,
//!   `@everyone`/`@here`, `<:custom:emoji>`, `<t:unix:STYLE>` timestamps,
//!   `</slash command:id>` and backslash escapes.
//!
//! Parsing never panics; anything that doesn't match a rule stays text.
//! The renderer never opens links itself: clicks come back as
//! [`MarkdownAction`]s so the app can show its link warning.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, Local, TimeZone};
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Shape, Stroke,
    TextFormat, TextWrapMode, Ui, epaint::RectShape, text::LayoutJob, vec2,
};

use crate::emoji::{self, Segment};
use crate::theme::Palette;
use crate::timefmt;

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// One or more lines; line breaks are [`Inline::LineBreak`]. An empty
    /// paragraph is a blank line.
    Paragraph(Vec<Inline>),
    /// `#`, `##` or `###` (level 1..=3).
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    /// `-# small grey text`.
    Subtext(Vec<Inline>),
    /// `> line` or `>>> rest of message`. Quotes don't nest.
    Quote(Vec<Block>),
    /// Consecutive list lines. Nesting is flattened into `depth`.
    List(Vec<ListItem>),
    Code {
        lang: Option<String>,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// 0 = top level.
    pub depth: u8,
    pub marker: ListMarker,
    pub content: Vec<Inline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMarker {
    Bullet,
    /// The number to show (already renumbered like Discord: `1. 1. 1.` shows
    /// 1, 2, 3; the first item sets the start).
    Number(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    LineBreak,
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Underline(Vec<Inline>),
    Strike(Vec<Inline>),
    Spoiler(Vec<Inline>),
    Code(String),
    /// `content: None` for bare / `<autolinked>` URLs (show the URL itself).
    Link {
        url: String,
        content: Option<Vec<Inline>>,
    },
    User(String),
    Role(String),
    Channel(String),
    Everyone,
    Here,
    /// `</name:id>` slash command mention.
    Command {
        name: String,
        id: String,
    },
    CustomEmoji {
        name: String,
        id: String,
        animated: bool,
    },
    /// A Unicode emoji (one grapheme) with its Twemoji image URL.
    Emoji {
        text: String,
        url: String,
    },
    Timestamp {
        unix: i64,
        style: TimestampStyle,
    },
}

/// `<t:unix:STYLE>` styles. The default (no style letter) is `f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimestampStyle {
    /// `t` — 16:20
    ShortTime,
    /// `T` — 16:20:30
    LongTime,
    /// `d` — 20/04/2021
    ShortDate,
    /// `D` — 20 April 2021
    LongDate,
    /// `f` — 20 April 2021 16:20
    #[default]
    ShortDateTime,
    /// `F` — Tuesday, 20 April 2021 16:20
    LongDateTime,
    /// `R` — 2 months ago
    Relative,
}

impl TimestampStyle {
    pub fn from_letter(c: &str) -> Option<Self> {
        Some(match c {
            "t" => Self::ShortTime,
            "T" => Self::LongTime,
            "d" => Self::ShortDate,
            "D" => Self::LongDate,
            "f" => Self::ShortDateTime,
            "F" => Self::LongDateTime,
            "R" => Self::Relative,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Block parser
// ---------------------------------------------------------------------------

/// Deepest nesting of bold/italic/... spans. Deeper delimiters stay text, so
/// hostile input can't overflow the stack.
const MAX_DEPTH: u8 = 12;
/// Deepest list nesting.
const MAX_LIST_DEPTH: usize = 5;

/// Parse a message into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let text = normalize_newlines(text);
    parse_blocks(&text, false)
}

/// "Formatting off": no markup, no blocks — just the raw text, with
/// mentions, emoji, timestamps and URLs still recognised so they stay
/// readable and clickable.
pub fn plain(text: &str) -> Vec<Block> {
    let text = normalize_newlines(text);
    if text.is_empty() {
        return Vec::new();
    }
    vec![Block::Paragraph(parse_inline(
        &text,
        Opts {
            markup: false,
            links: true,
            depth: 0,
        },
    ))]
}

fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        text.into()
    }
}

#[derive(Default)]
struct BlockBuilder {
    blocks: Vec<Block>,
    para: Option<String>,
    items: Vec<ListItem>,
    indents: Vec<usize>,
    counters: Vec<Option<u64>>,
}

impl BlockBuilder {
    fn para_line(&mut self, line: &str) {
        self.flush_list();
        match &mut self.para {
            Some(p) => {
                p.push('\n');
                p.push_str(line);
            }
            None => self.para = Some(line.to_string()),
        }
    }

    fn flush_para(&mut self) {
        if let Some(p) = self.para.take() {
            self.blocks
                .push(Block::Paragraph(parse_inline(&p, Opts::default())));
        }
    }

    fn flush_list(&mut self) {
        if !self.items.is_empty() {
            self.blocks
                .push(Block::List(std::mem::take(&mut self.items)));
        }
        self.indents.clear();
        self.counters.clear();
    }

    fn flush(&mut self) {
        self.flush_para();
        self.flush_list();
    }

    fn list_item(&mut self, indent: usize, number: Option<u64>, content: &str) {
        self.flush_para();
        while self.indents.last().is_some_and(|&top| indent < top) {
            self.indents.pop();
        }
        let depth = match self.indents.last() {
            None => {
                self.indents.push(indent);
                0
            }
            Some(&top) if indent > top && self.indents.len() < MAX_LIST_DEPTH => {
                self.indents.push(indent);
                self.indents.len() - 1
            }
            Some(_) => self.indents.len() - 1,
        };
        self.counters.resize(depth + 1, None);
        let marker = match number {
            Some(n) => {
                let shown = self.counters[depth].map_or(n, |prev| prev.saturating_add(1));
                self.counters[depth] = Some(shown);
                ListMarker::Number(shown)
            }
            None => {
                self.counters[depth] = None;
                ListMarker::Bullet
            }
        };
        self.items.push(ListItem {
            depth: depth as u8,
            marker,
            content: parse_inline(content, Opts::default()),
        });
    }
}

fn parse_blocks(text: &str, in_quote: bool) -> Vec<Block> {
    let mut b = BlockBuilder::default();
    let len = text.len();
    let mut pos = 0;
    while pos < len {
        let line_end = text[pos..].find('\n').map_or(len, |i| pos + i);
        let line = &text[pos..line_end];
        let next = (line_end + 1).min(len);

        if !in_quote {
            let trimmed = line.trim_start_matches(' ');
            if trimmed.starts_with(">>> ") {
                let start = pos + (line.len() - trimmed.len()) + 4;
                b.flush();
                b.blocks
                    .push(Block::Quote(parse_blocks(&text[start..], true)));
                pos = len;
                continue;
            }
            if quote_content(line).is_some() {
                b.flush();
                let mut inner = String::new();
                let mut p = pos;
                while p < len {
                    let end = text[p..].find('\n').map_or(len, |i| p + i);
                    let Some(content) = quote_content(&text[p..end]) else {
                        break;
                    };
                    if !inner.is_empty() || p != pos {
                        inner.push('\n');
                    }
                    inner.push_str(content);
                    p = (end + 1).min(len);
                }
                b.blocks.push(Block::Quote(parse_blocks(&inner, true)));
                pos = p;
                continue;
            }
        }

        if let Some(f) = find_fence(text, pos, line_end) {
            let prefix = text[pos..f.start].trim_end();
            if !prefix.trim().is_empty() {
                b.para_line(prefix);
            }
            b.flush();
            b.blocks.push(Block::Code {
                lang: f.lang,
                code: f.code,
            });
            pos = f.end;
            if text[pos..].starts_with('\n') {
                pos += 1;
            }
            continue;
        }

        if let Some(block) = heading(line) {
            b.flush();
            b.blocks.push(block);
        } else if let Some((indent, number, content)) = list_line(line) {
            b.list_item(indent, number, content);
        } else {
            b.para_line(line);
        }
        pos = next;
    }
    b.flush();
    b.blocks
}

/// `> text` (leading spaces allowed). `>` must be followed by a space and
/// not be the start of `>>>`.
fn quote_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if trimmed.starts_with(">>> ") {
        return None;
    }
    trimmed.strip_prefix("> ")
}

fn heading(line: &str) -> Option<Block> {
    if let Some(rest) = line.strip_prefix("-# ") {
        let content = rest.trim();
        return (!content.is_empty())
            .then(|| Block::Subtext(parse_inline(content, Opts::default())));
    }
    let hashes = line.bytes().take_while(|&c| c == b'#').count();
    if !(1..=3).contains(&hashes) || !line[hashes..].starts_with(' ') {
        return None;
    }
    let content = line[hashes + 1..].trim();
    (!content.is_empty()).then(|| Block::Heading {
        level: hashes as u8,
        content: parse_inline(content, Opts::default()),
    })
}

/// `- item`, `* item` or `12. item`, with optional indentation.
/// Returns (indent, number for ordered items, content).
fn list_line(line: &str) -> Option<(usize, Option<u64>, &str)> {
    let indent = line.bytes().take_while(|&c| c == b' ').count();
    let rest = &line[indent..];
    let (number, content) = if let Some(c) = rest.strip_prefix("- ").or(rest.strip_prefix("* ")) {
        (None, c)
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) || !rest[digits..].starts_with(". ") {
            return None;
        }
        (Some(rest[..digits].parse().ok()?), &rest[digits + 2..])
    };
    let content = content.trim();
    (!content.is_empty()).then_some((indent, number, content))
}

struct Fence {
    start: usize,
    end: usize,
    lang: Option<String>,
    code: String,
}

/// Find a closed ```` ``` ```` code block starting on `text[from..line_end]`.
fn find_fence(text: &str, from: usize, line_end: usize) -> Option<Fence> {
    let mut search = from;
    while search < line_end {
        let a = search + text[search..line_end].find("```")?;
        search = a + 1;
        let backslashes = text[from..a]
            .bytes()
            .rev()
            .take_while(|&c| c == b'\\')
            .count();
        if backslashes % 2 == 1 {
            continue;
        }
        let after = a + 3;
        // Optional language: letters etc. directly followed by a newline.
        let lang_len = text[after..]
            .bytes()
            .take_while(|&c| c.is_ascii_alphanumeric() || b"_+-.#".contains(&c))
            .count();
        let with_lang = (lang_len > 0 && text[after + lang_len..].starts_with('\n')).then(|| {
            (
                Some(text[after..after + lang_len].to_string()),
                after + lang_len + 1,
            )
        });
        for (lang, body_start) in with_lang.into_iter().chain([(None, after)]) {
            // Leading newlines are dropped; the body needs one real character.
            let first = body_start
                + text[body_start..]
                    .bytes()
                    .take_while(|&c| c == b'\n')
                    .count();
            let Some(ch) = text[first..].chars().next() else {
                continue;
            };
            let Some(close) = text[first + ch.len_utf8()..].find("```") else {
                continue;
            };
            let close = first + ch.len_utf8() + close;
            let code = text[first..close].trim_end_matches('\n').to_string();
            return Some(Fence {
                start: a,
                end: close + 3,
                lang,
                code,
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Inline parser
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Opts {
    /// Bold/italic/code/escapes/masked links. Off for [`plain`].
    markup: bool,
    /// URLs and masked links. Off inside masked link text.
    links: bool,
    depth: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            markup: true,
            links: true,
            depth: 0,
        }
    }
}

impl Opts {
    fn deeper(self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self
        }
    }
}

fn char_len_at(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(1, char::len_utf8)
}

fn parse_inline(s: &str, o: Opts) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < s.len() {
        // Backslash escapes: `\` + any char that isn't a letter, digit or space.
        if o.markup
            && s.as_bytes()[i] == b'\\'
            && let Some(c) = s[i + 1..].chars().next()
            && !c.is_ascii_alphanumeric()
            && !c.is_whitespace()
        {
            text.push(c);
            i += 1 + c.len_utf8();
            continue;
        }
        if let Some((node, len)) = match_at(s, i, o) {
            flush_text(&mut text, &mut out);
            out.push(node);
            i += len;
            continue;
        }
        let len = char_len_at(s, i);
        text.push_str(&s[i..i + len]);
        i += len;
    }
    flush_text(&mut text, &mut out);
    out
}

/// Text runs are split into text and Unicode emoji here, once, so the
/// renderer doesn't redo it every frame.
fn flush_text(text: &mut String, out: &mut Vec<Inline>) {
    if text.is_empty() {
        return;
    }
    for seg in emoji::unicode_segments(text) {
        out.push(match seg {
            Segment::Text(t) => Inline::Text(t),
            Segment::Emoji { url, alt } => Inline::Emoji { text: alt, url },
        });
    }
    text.clear();
}

fn match_at(s: &str, i: usize, o: Opts) -> Option<(Inline, usize)> {
    let rest = &s[i..];
    let nest = o.markup && o.depth < MAX_DEPTH;
    match rest.as_bytes()[0] {
        b'\n' => Some((Inline::LineBreak, 1)),
        b'`' if o.markup => inline_code(rest),
        b'*' | b'_' if nest => emphasis(s, i, o),
        b'~' if nest => {
            let close = find_close(rest, "~~", None)?;
            let inner = parse_inline(&rest[2..close], o.deeper());
            Some((Inline::Strike(inner), close + 2))
        }
        b'|' if nest => {
            let close = find_close(rest, "||", None)?;
            let inner = parse_inline(&rest[2..close], o.deeper());
            Some((Inline::Spoiler(inner), close + 2))
        }
        b'<' => angle(rest, o),
        b'[' if o.markup && o.links && o.depth < MAX_DEPTH => masked_link(rest, o),
        b'h' | b'H' if o.links => bare_url(rest),
        b'@' if rest.starts_with("@everyone") => Some((Inline::Everyone, 9)),
        b'@' if rest.starts_with("@here") => Some((Inline::Here, 5)),
        _ => None,
    }
}

/// `rest` starts with `delim`. Find the first closing `delim` after a
/// non-empty body, skipping escaped characters; optionally the closer must
/// not be followed by `not_before`.
fn find_close(rest: &str, delim: &str, not_before: Option<u8>) -> Option<usize> {
    if !rest.starts_with(delim) {
        return None;
    }
    let b = rest.as_bytes();
    let open = delim.len();
    let mut p = open;
    while p < b.len() {
        if p > open
            && rest[p..].starts_with(delim)
            && not_before.is_none_or(|c| b.get(p + delim.len()) != Some(&c))
        {
            return Some(p);
        }
        if b[p] == b'\\' && p + 1 < b.len() {
            p += 1 + char_len_at(rest, p + 1);
        } else {
            p += char_len_at(rest, p);
        }
    }
    None
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `*italic*`: opener followed by non-space, closer not preceded by space
/// and not followed by another `*`. `**` inside is allowed.
fn em_star(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if b.get(1).is_none_or(|c| c.is_ascii_whitespace()) {
        return None;
    }
    let mut p = 1;
    let mut nonempty = false;
    loop {
        let c = *b.get(p)?;
        if nonempty && c == b'*' && b.get(p + 1) != Some(&b'*') {
            return Some(p);
        }
        if rest[p..].starts_with("**") {
            p += 2;
        } else if c == b'\\' && p + 1 < b.len() {
            p += 1 + char_len_at(rest, p + 1);
        } else if rest[p..].starts_with(char::is_whitespace) {
            // Whitespace must be followed by something that isn't a lone `*`.
            let ws = rest[p..]
                .find(|ch: char| !ch.is_whitespace())
                .unwrap_or(rest.len() - p);
            let q = p + ws;
            let c = *b.get(q)?;
            if rest[q..].starts_with("**") {
                p = q + 2;
            } else if c == b'\\' && q + 1 < b.len() {
                p = q + 1 + char_len_at(rest, q + 1);
            } else if c != b'*' && c != b'\\' {
                p = q + char_len_at(rest, q);
            } else {
                return None;
            }
        } else if c == b'*' || c == b'\\' {
            return None;
        } else {
            p += char_len_at(rest, p);
        }
        nonempty = true;
    }
}

/// `_italic_`: closer must be followed by a non-word character.
fn em_underscore(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    let mut p = 1;
    let mut nonempty = false;
    loop {
        let c = *b.get(p)?;
        if nonempty && c == b'_' && !b.get(p + 1).is_some_and(|&n| is_word(n)) {
            return Some(p);
        }
        if rest[p..].starts_with("__") {
            p += 2;
        } else if c == b'\\' && p + 1 < b.len() {
            p += 1 + char_len_at(rest, p + 1);
        } else if c == b'_' || c == b'\\' {
            return None;
        } else {
            p += char_len_at(rest, p);
        }
        nonempty = true;
    }
}

/// `*`, `**`, `_` and `__` compete like in simple-markdown: the longest match
/// wins, ties go to italic, then bold, then underline.
fn emphasis(s: &str, i: usize, o: Opts) -> Option<(Inline, usize)> {
    let rest = &s[i..];
    // (total length, body start, body end, kind) — kind 0 italic, 1 bold, 2 underline
    let mut best: Option<(usize, usize, usize, u8)> = None;
    let mut consider = |cand: Option<(usize, usize, usize, u8)>| {
        if let Some(c) = cand
            && best.is_none_or(|b| c.0 > b.0)
        {
            best = Some(c);
        }
    };
    if rest.starts_with('*') {
        consider(em_star(rest).map(|c| (c + 1, 1, c, 0)));
        consider(find_close(rest, "**", Some(b'*')).map(|c| (c + 2, 2, c, 1)));
    } else {
        let boundary = !s[..i].bytes().next_back().is_some_and(is_word);
        if boundary {
            consider(em_underscore(rest).map(|c| (c + 1, 1, c, 0)));
        }
        consider(find_close(rest, "__", Some(b'_')).map(|c| (c + 2, 2, c, 2)));
    }
    let (total, start, end, kind) = best?;
    let inner = parse_inline(&rest[start..end], o.deeper());
    let node = match kind {
        0 => Inline::Italic(inner),
        1 => Inline::Bold(inner),
        _ => Inline::Underline(inner),
    };
    Some((node, total))
}

/// `` `code` `` / ``` ``co`de`` ```: the closing run must be the same length.
fn inline_code(rest: &str) -> Option<(Inline, usize)> {
    let b = rest.as_bytes();
    let n = b.iter().take_while(|&&c| c == b'`').count();
    let mut p = n;
    loop {
        let q = p + rest[p..].find('`')?;
        let m = b[q..].iter().take_while(|&&c| c == b'`').count();
        if m == n {
            let mut code = &rest[n..q];
            if code.len() >= 2
                && code.starts_with(' ')
                && code.ends_with(' ')
                && !code.trim().is_empty()
            {
                code = &code[1..code.len() - 1];
            }
            return Some((Inline::Code(code.to_string()), q + m));
        }
        p = q + m;
    }
}

fn is_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit())
}

fn starts_with_http(s: &str) -> Option<usize> {
    let lower = |n: usize| s.get(..n).map(str::to_ascii_lowercase);
    if lower(8).as_deref() == Some("https://") {
        Some(8)
    } else if lower(7).as_deref() == Some("http://") {
        Some(7)
    } else {
        None
    }
}

/// Everything in `<...>`: mentions, emoji, timestamps, commands, autolinks.
fn angle(rest: &str, o: Opts) -> Option<(Inline, usize)> {
    let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let end = line.find('>')?;
    let inner = &rest[1..end];
    let total = end + 1;
    let node = if let Some(id) = inner.strip_prefix("@&") {
        is_id(id).then(|| Inline::Role(id.into()))?
    } else if let Some(id) = inner.strip_prefix("@!").or(inner.strip_prefix('@')) {
        is_id(id).then(|| Inline::User(id.into()))?
    } else if let Some(id) = inner.strip_prefix('#') {
        is_id(id).then(|| Inline::Channel(id.into()))?
    } else if let Some(t) = inner.strip_prefix("t:") {
        let (num, style) = match t.split_once(':') {
            Some((num, letter)) => (num, TimestampStyle::from_letter(letter)?),
            None => (t, TimestampStyle::default()),
        };
        let digits = num.strip_prefix('-').unwrap_or(num);
        if digits.is_empty() || digits.len() > 17 || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Inline::Timestamp {
            unix: num.parse().ok()?,
            style,
        }
    } else if let Some(cmd) = inner.strip_prefix('/') {
        let (name, id) = cmd.rsplit_once(':')?;
        if name.trim().is_empty() || name.contains(['<', ':']) || !is_id(id) {
            return None;
        }
        Inline::Command {
            name: name.into(),
            id: id.into(),
        }
    } else if inner.starts_with(':') || inner.starts_with("a:") {
        let animated = inner.starts_with('a');
        let (name, id) = inner[if animated { 2 } else { 1 }..].split_once(':')?;
        if name.is_empty() || !name.bytes().all(is_word) || !is_id(id) {
            return None;
        }
        Inline::CustomEmoji {
            name: name.into(),
            id: id.into(),
            animated,
        }
    } else if o.links
        && starts_with_http(inner).is_some_and(|n| inner.len() > n)
        && !inner.contains(char::is_whitespace)
    {
        Inline::Link {
            url: inner.into(),
            content: None,
        }
    } else {
        return None;
    };
    Some((node, total))
}

/// A bare `http(s)://` URL. Trailing punctuation is left out, and a trailing
/// `)` only stays if it closes a `(` inside the URL (Wikipedia links).
fn bare_url(rest: &str) -> Option<(Inline, usize)> {
    let scheme = starts_with_http(rest)?;
    let mut end = rest
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    while let Some(last) = rest[..end].chars().next_back() {
        let strip = match last {
            '.' | ',' | ':' | ';' | '"' | '\'' | ']' | '!' | '?' => true,
            ')' => rest[..end].matches('(').count() < rest[..end].matches(')').count(),
            _ => false,
        };
        if !strip || end <= scheme {
            break;
        }
        end -= last.len_utf8();
    }
    (end > scheme).then(|| {
        (
            Inline::Link {
                url: rest[..end].into(),
                content: None,
            },
            end,
        )
    })
}

/// `[text](https://url)`, `[text](<https://url>)`, optional `"title"`.
fn masked_link(rest: &str, o: Opts) -> Option<(Inline, usize)> {
    let b = rest.as_bytes();
    let mut depth = 0usize;
    let mut p = 0;
    let close = loop {
        let c = *b.get(p)?;
        match c {
            b'\\' if p + 1 < b.len() => {
                p += 1 + char_len_at(rest, p + 1);
                continue;
            }
            b'\n' => return None,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break p;
                }
            }
            _ => {}
        }
        p += char_len_at(rest, p);
    };
    let text = &rest[1..close];
    if text.trim().is_empty() || !rest[close + 1..].starts_with('(') {
        return None;
    }
    let mut p = close + 2;
    let url = if rest[p..].starts_with('<') {
        let e = rest[p + 1..].find(['>', '\n'])?;
        if b[p + 1 + e] != b'>' {
            return None;
        }
        let url = &rest[p + 1..p + 1 + e];
        p += e + 2;
        url
    } else {
        let start = p;
        let mut parens = 0usize;
        while let Some(&c) = b.get(p) {
            match c {
                b'(' => parens += 1,
                b')' if parens == 0 => break,
                b')' => parens -= 1,
                c if c.is_ascii_whitespace() => break,
                _ => {}
            }
            p += char_len_at(rest, p);
        }
        &rest[start..p]
    };
    // Optional title: `[text](url "title")`.
    let after = rest[p..].trim_start_matches(' ');
    p = rest.len() - after.len();
    if rest[p..].starts_with('"') {
        let e = rest[p + 1..].find(['"', '\n'])?;
        if b[p + 1 + e] != b'"' {
            return None;
        }
        p += e + 2;
        p = rest.len() - rest[p..].trim_start_matches(' ').len();
    }
    if !rest[p..].starts_with(')')
        || !starts_with_http(url).is_some_and(|n| url.len() > n)
        || url.contains(char::is_whitespace)
    {
        return None;
    }
    let content = parse_inline(
        text,
        Opts {
            links: false,
            ..o.deeper()
        },
    );
    Some((
        Inline::Link {
            url: url.into(),
            content: Some(content),
        },
        p + 1,
    ))
}

// ---------------------------------------------------------------------------
// Helpers on parsed messages
// ---------------------------------------------------------------------------

/// Most emoji Discord still shows big.
const JUMBO_MAX: usize = 30;

/// True if the message is only emoji and whitespace (Discord shows them big).
pub fn is_jumbo(blocks: &[Block]) -> bool {
    let mut count = 0;
    for block in blocks {
        let Block::Paragraph(inlines) = block else {
            return false;
        };
        for inline in inlines {
            match inline {
                Inline::Emoji { .. } | Inline::CustomEmoji { .. } => count += 1,
                Inline::LineBreak => {}
                Inline::Text(t) if t.trim().is_empty() => {}
                _ => return false,
            }
        }
    }
    (1..=JUMBO_MAX).contains(&count)
}

/// Names for mentions. Implement this on whatever holds the current server's
/// members / roles / channels; anything unknown falls back to Discord's
/// placeholders (`@unknown-user`, `@deleted-role`, `#unknown`).
pub trait Lookup {
    fn user_name(&self, _id: &str) -> Option<String> {
        None
    }
    /// Role name and colour (None = no colour, use the accent).
    fn role(&self, _id: &str) -> Option<(String, Option<Color32>)> {
        None
    }
    fn channel_name(&self, _id: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
/// A [`Lookup`] that knows nothing.
pub struct NoLookup;
#[cfg(test)]
impl Lookup for NoLookup {}

fn user_label(lookup: &dyn Lookup, id: &str) -> String {
    format!(
        "@{}",
        lookup
            .user_name(id)
            .unwrap_or_else(|| "unknown-user".into())
    )
}

fn role_label(lookup: &dyn Lookup, id: &str) -> (String, Option<Color32>) {
    match lookup.role(id) {
        Some((name, color)) => (format!("@{name}"), color),
        None => ("@deleted-role".into(), None),
    }
}

fn channel_label(lookup: &dyn Lookup, id: &str) -> String {
    format!(
        "#{}",
        lookup.channel_name(id).unwrap_or_else(|| "unknown".into())
    )
}

/// Flatten parsed blocks to readable text, e.g. for reply previews and
/// notifications. Mentions are resolved, spoilers are hidden.
pub fn to_plain_text(blocks: &[Block], lookup: &dyn Lookup) -> String {
    fn inlines(out: &mut String, items: &[Inline], lookup: &dyn Lookup) {
        for item in items {
            match item {
                Inline::Text(t) | Inline::Code(t) => out.push_str(t),
                Inline::LineBreak => out.push('\n'),
                Inline::Bold(c) | Inline::Italic(c) | Inline::Underline(c) | Inline::Strike(c) => {
                    inlines(out, c, lookup)
                }
                Inline::Spoiler(_) => out.push_str("[spoiler]"),
                Inline::Link { url, content } => match content {
                    Some(c) => inlines(out, c, lookup),
                    None => out.push_str(url),
                },
                Inline::User(id) => out.push_str(&user_label(lookup, id)),
                Inline::Role(id) => out.push_str(&role_label(lookup, id).0),
                Inline::Channel(id) => out.push_str(&channel_label(lookup, id)),
                Inline::Everyone => out.push_str("@everyone"),
                Inline::Here => out.push_str("@here"),
                Inline::Command { name, .. } => {
                    out.push('/');
                    out.push_str(name);
                }
                Inline::CustomEmoji { name, .. } => {
                    out.push(':');
                    out.push_str(name);
                    out.push(':');
                }
                Inline::Emoji { text, .. } => out.push_str(text),
                Inline::Timestamp { unix, style } => {
                    out.push_str(&format_timestamp(*unix, *style, &Local::now()));
                }
            }
        }
    }
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            Block::Paragraph(c) | Block::Subtext(c) | Block::Heading { content: c, .. } => {
                inlines(&mut out, c, lookup);
            }
            Block::Quote(inner) => out.push_str(&to_plain_text(inner, lookup)),
            Block::List(items) => {
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push('\n');
                    }
                    inlines(&mut out, &item.content, lookup);
                }
            }
            Block::Code { code, .. } => out.push_str(code),
        }
    }
    out
}

/// Format `<t:unix:style>` like Discord (UK English; 24-hour clock unless
/// "Use 12-hour time" is on, see `timefmt::set_12_hour`).
/// `now` sets the time zone and the reference for relative times.
pub fn format_timestamp<Tz: TimeZone>(
    unix: i64,
    style: TimestampStyle,
    now: &DateTime<Tz>,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(utc) = DateTime::from_timestamp(unix, 0) else {
        return "Invalid Date".into();
    };
    let t = utc.with_timezone(&now.timezone());
    match style {
        TimestampStyle::ShortTime => t.format(timefmt::hm()).to_string(),
        TimestampStyle::LongTime => t.format(timefmt::hms()).to_string(),
        TimestampStyle::ShortDate => t.format("%d/%m/%Y").to_string(),
        TimestampStyle::LongDate => t.format("%-d %B %Y").to_string(),
        TimestampStyle::ShortDateTime => t
            .format(&format!("%-d %B %Y {}", timefmt::hm()))
            .to_string(),
        TimestampStyle::LongDateTime => t
            .format(&format!("%A, %-d %B %Y {}", timefmt::hm()))
            .to_string(),
        TimestampStyle::Relative => relative_time(unix - now.timestamp()),
    }
}

/// moment.js-style relative time ("in 5 minutes", "a month ago").
fn relative_time(delta_secs: i64) -> String {
    let s = delta_secs.unsigned_abs() as f64;
    let (min, hour, day) = (60.0, 3600.0, 86400.0);
    let n = |x: f64| x.round().max(1.0) as u64;
    let phrase = if s < 45.0 {
        "a few seconds".to_string()
    } else if s < 90.0 {
        "a minute".to_string()
    } else if s < 45.0 * min {
        format!("{} minutes", n(s / min))
    } else if s < 90.0 * min {
        "an hour".to_string()
    } else if s < 22.0 * hour {
        format!("{} hours", n(s / hour))
    } else if s < 36.0 * hour {
        "a day".to_string()
    } else if s < 26.0 * day {
        format!("{} days", n(s / day))
    } else if s < 45.0 * day {
        "a month".to_string()
    } else if s < 320.0 * day {
        format!("{} months", n(s / (30.4 * day)))
    } else if s < 548.0 * day {
        "a year".to_string()
    } else {
        format!("{} years", n(s / (365.25 * day)))
    };
    if delta_secs > 0 {
        format!("in {phrase}")
    } else {
        format!("{phrase} ago")
    }
}

// ---------------------------------------------------------------------------
// Parse cache
// ---------------------------------------------------------------------------

/// Parsed messages keyed by message id. An edit (new content hash) or a
/// change of the formatting setting re-parses. Capped: the least recently
/// used quarter is dropped when full.
pub struct ParseCache {
    cap: usize,
    tick: u64,
    map: HashMap<String, CacheEntry>,
}

struct CacheEntry {
    hash: u64,
    last_used: u64,
    blocks: Arc<[Block]>,
}

impl Default for ParseCache {
    fn default() -> Self {
        Self::new(512)
    }
}

impl ParseCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            tick: 0,
            map: HashMap::new(),
        }
    }

    /// Parsed blocks for a message; `formatted == false` uses [`plain`].
    pub fn get(&mut self, message_id: &str, content: &str, formatted: bool) -> Arc<[Block]> {
        let mut h = std::hash::DefaultHasher::new();
        content.hash(&mut h);
        formatted.hash(&mut h);
        let hash = h.finish();
        self.tick += 1;
        if let Some(e) = self.map.get_mut(message_id)
            && e.hash == hash
        {
            e.last_used = self.tick;
            return e.blocks.clone();
        }
        let blocks: Arc<[Block]> = if formatted {
            parse(content)
        } else {
            plain(content)
        }
        .into();
        if self.map.len() >= self.cap && !self.map.contains_key(message_id) {
            self.evict();
        }
        self.map.insert(
            message_id.to_string(),
            CacheEntry {
                hash,
                last_used: self.tick,
                blocks: blocks.clone(),
            },
        );
        blocks
    }

    fn evict(&mut self) {
        let mut ages: Vec<u64> = self.map.values().map(|e| e.last_used).collect();
        ages.sort_unstable();
        let cutoff = ages[(ages.len() / 4).min(ages.len() - 1)];
        self.map.retain(|_, e| e.last_used > cutoff);
    }

    pub fn remove(&mut self, message_id: &str) {
        self.map.remove(message_id);
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

/// What the renderer needs from the app.
pub struct RenderCtx<'a> {
    pub palette: &'a Palette,
    pub lookup: &'a dyn Lookup,
    /// Keys spoiler reveal state.
    pub message_id: &'a str,
    /// Emoji as images (Emoji images module). Off = text.
    pub emoji_images: bool,
    pub emoji_size: f32,
    /// Size for emoji-only messages.
    pub jumbo_emoji_size: f32,
    /// A registered bold font family, if any. Without one, bold text is drawn
    /// in the stronger text colour.
    pub bold_family: Option<FontFamily>,
    /// Lower-case words to highlight (search results); empty = none.
    pub highlight: &'a [String],
}

impl<'a> RenderCtx<'a> {
    pub fn new(palette: &'a Palette, lookup: &'a dyn Lookup, message_id: &'a str) -> Self {
        Self {
            palette,
            lookup,
            message_id,
            emoji_images: true,
            emoji_size: 20.0,
            jumbo_emoji_size: 44.0,
            bold_family: None,
            highlight: &[],
        }
    }
}

/// Something the user clicked. The app decides what to do (link warning,
/// profile popout, switching channel...).
#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownAction {
    Link(String),
    /// With the pill's rect, to anchor a profile popout.
    User {
        id: String,
        rect: Rect,
    },
    Role(String),
    Channel(String),
}

#[derive(Default)]
pub struct RenderOutput {
    pub action: Option<MarkdownAction>,
    /// Union of the plain text labels, e.g. for a right-click menu.
    pub response: Option<Response>,
}

impl RenderOutput {
    fn add_response(&mut self, r: Response) {
        self.response = Some(match self.response.take() {
            Some(prev) => prev | r,
            None => r,
        });
    }
}

/// Draw parsed blocks.
pub fn render(ui: &mut Ui, blocks: &[Block], ctx: &RenderCtx) -> RenderOutput {
    render_with(ui, blocks, ctx, |_| {})
}

/// Like [`render`], but `trailing` is drawn inline after the last line (for
/// "(edited)", the ⋯ menu button and so on).
pub fn render_with(
    ui: &mut Ui,
    blocks: &[Block],
    ctx: &RenderCtx,
    trailing: impl FnOnce(&mut Ui),
) -> RenderOutput {
    let mut r = Renderer {
        ctx,
        out: RenderOutput::default(),
        spoiler_counter: 0,
        emoji_size: if is_jumbo(blocks) {
            ctx.jumbo_emoji_size
        } else {
            ctx.emoji_size
        },
    };
    let mut trailing = Some(trailing);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        r.blocks(ui, blocks, &mut trailing);
        if let Some(t) = trailing.take() {
            ui.horizontal_wrapped(t);
        }
    });
    r.out
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    /// Heading level 1..=3, 0 = none.
    heading: u8,
    subtext: bool,
    spoiler: Option<u32>,
    link: Option<u32>,
}

enum AtomKind<'a> {
    Text(&'a str),
    Code(&'a str),
    Break,
    User(&'a str),
    Role(&'a str),
    Channel(&'a str),
    Everyone,
    Here,
    Command(&'a str),
    CustomEmoji {
        name: &'a str,
        id: &'a str,
        animated: bool,
    },
    Emoji {
        text: &'a str,
        url: &'a str,
    },
    Timestamp(i64, TimestampStyle),
}

struct Atom<'a> {
    kind: AtomKind<'a>,
    style: Style,
}

struct Renderer<'c, 'a> {
    ctx: &'c RenderCtx<'a>,
    out: RenderOutput,
    spoiler_counter: u32,
    emoji_size: f32,
}

/// Flatten the inline tree into styled atoms.
fn flatten<'b>(
    items: &'b [Inline],
    style: Style,
    atoms: &mut Vec<Atom<'b>>,
    links: &mut Vec<&'b str>,
    spoilers: &mut u32,
) {
    for item in items {
        let mut push = |kind| atoms.push(Atom { kind, style });
        match item {
            Inline::Text(t) => push(AtomKind::Text(t)),
            Inline::Code(t) => push(AtomKind::Code(t)),
            Inline::LineBreak => push(AtomKind::Break),
            Inline::User(id) => push(AtomKind::User(id)),
            Inline::Role(id) => push(AtomKind::Role(id)),
            Inline::Channel(id) => push(AtomKind::Channel(id)),
            Inline::Everyone => push(AtomKind::Everyone),
            Inline::Here => push(AtomKind::Here),
            Inline::Command { name, .. } => push(AtomKind::Command(name)),
            Inline::CustomEmoji { name, id, animated } => push(AtomKind::CustomEmoji {
                name,
                id,
                animated: *animated,
            }),
            Inline::Emoji { text, url } => push(AtomKind::Emoji { text, url }),
            Inline::Timestamp { unix, style: ts } => push(AtomKind::Timestamp(*unix, *ts)),
            Inline::Bold(c) => flatten(
                c,
                Style {
                    bold: true,
                    ..style
                },
                atoms,
                links,
                spoilers,
            ),
            Inline::Italic(c) => flatten(
                c,
                Style {
                    italic: true,
                    ..style
                },
                atoms,
                links,
                spoilers,
            ),
            Inline::Underline(c) => flatten(
                c,
                Style {
                    underline: true,
                    ..style
                },
                atoms,
                links,
                spoilers,
            ),
            Inline::Strike(c) => flatten(
                c,
                Style {
                    strike: true,
                    ..style
                },
                atoms,
                links,
                spoilers,
            ),
            Inline::Spoiler(c) => {
                let s = Style {
                    // Nested spoilers reveal together with the outer one.
                    spoiler: style.spoiler.or(Some(*spoilers)),
                    ..style
                };
                if style.spoiler.is_none() {
                    *spoilers += 1;
                }
                flatten(c, s, atoms, links, spoilers);
            }
            Inline::Link { url, content } => {
                links.push(url);
                let s = Style {
                    link: Some(links.len() as u32 - 1),
                    ..style
                };
                match content {
                    Some(c) => flatten(c, s, atoms, links, spoilers),
                    None => atoms.push(Atom {
                        kind: AtomKind::Text(url),
                        style: s,
                    }),
                }
            }
        }
    }
}

impl Renderer<'_, '_> {
    fn pal(&self) -> &Palette {
        self.ctx.palette
    }

    fn blocks(
        &mut self,
        ui: &mut Ui,
        blocks: &[Block],
        trailing: &mut Option<impl FnOnce(&mut Ui)>,
    ) {
        let last = blocks.len().saturating_sub(1);
        for (n, block) in blocks.iter().enumerate() {
            let mut t = if n == last { trailing.take() } else { None };
            match block {
                Block::Paragraph(c) => self.inline_lines(ui, c, Style::default(), &mut t),
                Block::Heading { level, content } => {
                    ui.add_space(4.0);
                    let style = Style {
                        heading: *level,
                        bold: true,
                        ..Style::default()
                    };
                    self.inline_lines(ui, content, style, &mut t);
                }
                Block::Subtext(c) => {
                    let style = Style {
                        subtext: true,
                        ..Style::default()
                    };
                    self.inline_lines(ui, c, style, &mut t);
                }
                Block::Quote(inner) => self.quote(ui, inner, &mut t),
                Block::List(items) => self.list(ui, items, &mut t),
                Block::Code { code, .. } => self.code_block(ui, code),
            }
            // Couldn't place it inline (e.g. after a code block): put it back.
            if let Some(t) = t {
                *trailing = Some(t);
            }
        }
    }

    fn quote(&mut self, ui: &mut Ui, inner: &[Block], trailing: &mut Option<impl FnOnce(&mut Ui)>) {
        let bar = self.pal().muted.gamma_multiply(0.5);
        ui.horizontal_top(|ui| {
            let (bar_rect, _) = ui.allocate_exact_size(vec2(4.0, 1.0), Sense::hover());
            ui.add_space(8.0);
            let inner_rect = ui
                .vertical(|ui| self.blocks(ui, inner, trailing))
                .response
                .rect;
            let rect = Rect::from_x_y_ranges(bar_rect.x_range(), inner_rect.y_range());
            ui.painter().rect_filled(rect, CornerRadius::same(2), bar);
        });
    }

    fn list(
        &mut self,
        ui: &mut Ui,
        items: &[ListItem],
        trailing: &mut Option<impl FnOnce(&mut Ui)>,
    ) {
        let last = items.len().saturating_sub(1);
        let text = self.pal().text;
        for (n, item) in items.iter().enumerate() {
            let mut t = if n == last { trailing.take() } else { None };
            ui.horizontal_top(|ui| {
                ui.add_space(4.0 + f32::from(item.depth) * 20.0);
                let row_h = ui.text_style_height(&egui::TextStyle::Body);
                match item.marker {
                    ListMarker::Bullet => {
                        let (rect, _) = ui.allocate_exact_size(vec2(16.0, row_h), Sense::hover());
                        let c = rect.center();
                        let p = ui.painter();
                        match item.depth % 3 {
                            0 => {
                                p.circle_filled(c, 2.5, text);
                            }
                            1 => {
                                p.circle_stroke(c, 2.5, Stroke::new(1.0, text));
                            }
                            _ => {
                                p.rect_filled(Rect::from_center_size(c, vec2(5.0, 5.0)), 0, text);
                            }
                        }
                    }
                    ListMarker::Number(num) => {
                        ui.add_sized(
                            vec2(22.0, row_h),
                            egui::Label::new(format!("{num}.")).selectable(false),
                        );
                    }
                }
                ui.vertical(|ui| self.inline_lines(ui, &item.content, Style::default(), &mut t));
            });
            if let Some(t) = t {
                *trailing = Some(t);
            }
        }
    }

    fn code_block(&mut self, ui: &mut Ui, code: &str) {
        let pal = *self.pal();
        egui::Frame::new()
            .fill(pal.sidebar)
            .stroke(Stroke::new(1.0, pal.servers))
            .corner_radius(4)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                ui.add(
                    egui::Label::new(egui::RichText::new(code).font(font).color(pal.text))
                        .wrap_mode(TextWrapMode::Wrap),
                );
            });
    }

    /// Inline content, one `horizontal_wrapped` row per line.
    fn inline_lines(
        &mut self,
        ui: &mut Ui,
        items: &[Inline],
        base: Style,
        trailing: &mut Option<impl FnOnce(&mut Ui)>,
    ) {
        let mut atoms = Vec::new();
        let mut links = Vec::new();
        flatten(
            items,
            base,
            &mut atoms,
            &mut links,
            &mut self.spoiler_counter,
        );
        let lines: Vec<&[Atom]> = atoms.split(|a| matches!(a.kind, AtomKind::Break)).collect();
        let last = lines.len() - 1;
        for (n, line) in lines.into_iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                let spacing = ui.spacing().item_spacing.x;
                ui.spacing_mut().item_spacing.x = 0.0;
                if line.is_empty() {
                    // Blank line: keep its height.
                    ui.add(egui::Label::new(" ").selectable(false));
                } else {
                    self.line(ui, line, &links);
                }
                if n == last
                    && let Some(t) = trailing.take()
                {
                    ui.spacing_mut().item_spacing.x = spacing;
                    ui.add_space(4.0);
                    t(ui);
                }
            });
        }
    }

    fn spoiler_id(&self, idx: u32) -> egui::Id {
        egui::Id::unique(("rc-spoiler", self.ctx.message_id, idx))
    }

    fn hidden(&self, ui: &Ui, style: &Style) -> bool {
        style
            .spoiler
            .is_some_and(|i| !ui.data(|d| d.get_temp::<bool>(self.spoiler_id(i)).unwrap_or(false)))
    }

    fn spoiler_bg(&self) -> Color32 {
        let pal = self.pal();
        if pal.dark { pal.servers } else { pal.selected }
    }

    /// Text colour for bold when there's no bold font.
    fn bold_color(&self) -> Color32 {
        self.pal().unread
    }

    fn text_format(&self, ui: &Ui, style: &Style, code: bool, hidden: bool) -> TextFormat {
        let pal = self.pal();
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mut size = body.size;
        match style.heading {
            1 => size *= 1.5,
            2 => size *= 1.25,
            _ => {}
        }
        if style.subtext {
            size *= 0.8;
        }
        let family = if code {
            egui::TextStyle::Monospace.resolve(ui.style()).family
        } else if style.bold
            && let Some(f) = &self.ctx.bold_family
        {
            f.clone()
        } else {
            body.family
        };
        let mut color = if style.link.is_some() {
            ui.visuals().hyperlink_color
        } else if style.subtext {
            pal.muted
        } else if style.bold && self.ctx.bold_family.is_none() {
            self.bold_color()
        } else {
            pal.text
        };
        let mut background = if code {
            pal.input
        } else {
            Color32::TRANSPARENT
        };
        if style.spoiler.is_some() {
            if hidden {
                color = self.spoiler_bg();
                background = self.spoiler_bg();
            } else if !code {
                background = if pal.dark {
                    Color32::from_white_alpha(20)
                } else {
                    Color32::from_black_alpha(20)
                };
            }
        }
        let line = |on: bool| {
            if on && !hidden {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            }
        };
        TextFormat {
            font_id: FontId::new(if code { size * 0.9 } else { size }, family),
            color,
            background,
            italics: style.italic,
            underline: line(style.underline),
            strikethrough: line(style.strike),
            valign: ui.text_valign(),
            ..Default::default()
        }
    }

    /// Text shown for an atom when it's drawn as text (hidden spoilers,
    /// emoji images off).
    fn atom_text<'b>(&self, kind: &AtomKind<'b>) -> std::borrow::Cow<'b, str> {
        let lookup = self.ctx.lookup;
        match kind {
            AtomKind::Text(t) | AtomKind::Code(t) => (*t).into(),
            AtomKind::Break => "\n".into(),
            AtomKind::User(id) => user_label(lookup, id).into(),
            AtomKind::Role(id) => role_label(lookup, id).0.into(),
            AtomKind::Channel(id) => channel_label(lookup, id).into(),
            AtomKind::Everyone => "@everyone".into(),
            AtomKind::Here => "@here".into(),
            AtomKind::Command(name) => format!("/{name}").into(),
            AtomKind::CustomEmoji { name, .. } => format!(":{name}:").into(),
            AtomKind::Emoji { text, .. } => (*text).into(),
            AtomKind::Timestamp(unix, style) => {
                format_timestamp(*unix, *style, &Local::now()).into()
            }
        }
    }

    fn line(&mut self, ui: &mut Ui, atoms: &[Atom], links: &[&str]) {
        let mut job = LayoutJob::default();
        let mut key: Option<(Option<u32>, Option<u32>)> = None;
        for atom in atoms {
            let hidden = self.hidden(ui, &atom.style);
            let as_text = match atom.kind {
                AtomKind::Text(_) | AtomKind::Code(_) => true,
                AtomKind::Emoji { .. } | AtomKind::CustomEmoji { .. } => {
                    hidden || !self.ctx.emoji_images || atom.style.link.is_some()
                }
                _ => hidden,
            };
            if !as_text {
                self.flush(ui, &mut job, key.take(), links);
                self.widget(ui, atom);
                continue;
            }
            let k = (atom.style.spoiler, atom.style.link);
            if key.is_some_and(|prev| prev != k) {
                self.flush(ui, &mut job, key, links);
            }
            key = Some(k);
            let code = matches!(atom.kind, AtomKind::Code(_));
            let text = self.atom_text(&atom.kind);
            let text = if code {
                text.replace('\n', " ").into()
            } else {
                text
            };
            let format = self.text_format(ui, &atom.style, code, hidden);
            if hidden || code || self.ctx.highlight.is_empty() {
                job.append(&text, 0.0, format);
            } else {
                self.append_highlighted(&mut job, &text, format);
            }
        }
        self.flush(ui, &mut job, key, links);
    }

    /// Append `text` with the search terms in `ctx.highlight` marked like
    /// Discord's search hits.
    fn append_highlighted(&self, job: &mut LayoutJob, text: &str, format: TextFormat) {
        let pal = self.pal();
        let hit = TextFormat {
            color: pal.strong,
            background: pal.accent.gamma_multiply(0.45),
            ..format.clone()
        };
        let mut at = 0;
        for r in crate::search::match_ranges(text, self.ctx.highlight) {
            if r.start > at {
                job.append(&text[at..r.start], 0.0, format.clone());
            }
            job.append(&text[r.clone()], 0.0, hit.clone());
            at = r.end;
        }
        if at < text.len() {
            job.append(&text[at..], 0.0, format);
        }
    }

    /// Draw a run of text that shares the same spoiler / link.
    fn flush(
        &mut self,
        ui: &mut Ui,
        job: &mut LayoutJob,
        key: Option<(Option<u32>, Option<u32>)>,
        links: &[&str],
    ) {
        if job.is_empty() {
            return;
        }
        let job = std::mem::take(job);
        let (spoiler, link) = key.unwrap_or_default();
        let hidden = spoiler
            .is_some_and(|i| !ui.data(|d| d.get_temp::<bool>(self.spoiler_id(i)).unwrap_or(false)));
        if let (true, Some(i)) = (hidden, spoiler) {
            let r = ui
                .add(
                    egui::Label::new(job)
                        .sense(Sense::click())
                        .selectable(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() {
                ui.data_mut(|d| d.insert_temp(self.spoiler_id(i), true));
            }
            return;
        }
        if let Some(url) = link.and_then(|i| links.get(i as usize)) {
            let shown = job.text.clone();
            let r = ui
                .add(
                    egui::Label::new(job)
                        .sense(Sense::click())
                        .selectable(false),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                let y = r.rect.bottom() - 1.0;
                let color = ui.visuals().hyperlink_color;
                ui.painter()
                    .hline(r.rect.x_range(), y, Stroke::new(1.0, color));
            }
            let r = if shown != *url {
                r.on_hover_text(*url)
            } else {
                r
            };
            if r.clicked() {
                self.out.action = Some(MarkdownAction::Link(url.to_string()));
            }
            return;
        }
        let r = ui.add(egui::Label::new(job).sense(Sense::click()));
        self.out.add_response(r);
    }

    /// Non-text atoms: mention pills, emoji images, timestamps.
    fn widget(&mut self, ui: &mut Ui, atom: &Atom) {
        let pal = *self.pal();
        let lookup = self.ctx.lookup;
        match atom.kind {
            AtomKind::Emoji { text, url } => {
                let size = self.emoji_size;
                ui.add(egui::Image::from_uri(url.to_string()).fit_to_exact_size(vec2(size, size)))
                    .on_hover_text(text);
            }
            AtomKind::CustomEmoji { name, id, animated } => {
                let url = crate::emoji::custom_emoji_url(id, animated);
                let size = self.emoji_size;
                let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
                // Still; animated emoji play while hovered (over the still
                // frame, which stays while the animated copy loads).
                egui::Image::from_uri(url.as_str()).paint_at(ui, rect);
                if let std::borrow::Cow::Owned(anim) = crate::anim::on_hover(ui, rect, &url) {
                    egui::Image::from_uri(anim)
                        .show_loading_spinner(false)
                        .paint_at(ui, rect);
                }
                resp.on_hover_text(format!(":{name}:"));
            }
            AtomKind::Timestamp(unix, style) => {
                let now = Local::now();
                let text = format_timestamp(unix, style, &now);
                let full = format_timestamp(unix, TimestampStyle::LongDateTime, &now);
                let bg = pal.input;
                self.pill(ui, &text, &atom.style, pal.text, bg, bg, Sense::hover())
                    .on_hover_text(full);
            }
            ref kind => {
                let (label, color) = match kind {
                    AtomKind::Role(id) => role_label(lookup, id),
                    other => (self.atom_text(other).into_owned(), None),
                };
                let accent = color.unwrap_or(pal.accent);
                let fg = if color.is_some() || !pal.dark {
                    accent
                } else {
                    accent.lerp_to_gamma(Color32::WHITE, 0.6)
                };
                let bg = accent.gamma_multiply(if color.is_some() { 0.15 } else { 0.3 });
                let r = self.pill(ui, &label, &atom.style, fg, bg, accent, Sense::click());
                if r.clicked() {
                    self.out.action = match kind {
                        AtomKind::User(id) => Some(MarkdownAction::User {
                            id: id.to_string(),
                            rect: r.rect,
                        }),
                        AtomKind::Role(id) => Some(MarkdownAction::Role(id.to_string())),
                        AtomKind::Channel(id) => Some(MarkdownAction::Channel(id.to_string())),
                        _ => self.out.action.take(),
                    };
                }
            }
        }
    }

    /// A small rounded label. On hover the background becomes `hover_bg`
    /// with white text (like Discord's mention pills).
    #[allow(clippy::too_many_arguments)]
    fn pill(
        &self,
        ui: &mut Ui,
        text: &str,
        style: &Style,
        fg: Color32,
        bg: Color32,
        hover_bg: Color32,
        sense: Sense,
    ) -> Response {
        let mut format = self.text_format(ui, style, false, false);
        format.color = fg;
        format.background = Color32::TRANSPARENT;
        format.underline = Stroke::NONE;
        let mut job = LayoutJob::single_section(text.to_string(), format);
        job.wrap.max_width = f32::INFINITY;
        let galley = ui.painter().layout_job(job);
        let pad = vec2(3.0, 0.0);
        ui.add_space(1.0);
        let (rect, r) = ui.allocate_exact_size(galley.size() + 2.0 * pad, sense);
        ui.add_space(1.0);
        let clickable = sense.senses_click();
        let hovered = clickable && r.hovered();
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            painter.add(Shape::Rect(RectShape::filled(
                rect,
                CornerRadius::same(3),
                if hovered { hover_bg } else { bg },
            )));
            let color = if hovered { Color32::WHITE } else { fg };
            painter.galley_with_override_text_color(rect.min + pad, galley, color);
        }
        if clickable {
            r.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            r
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    fn para(items: Vec<Inline>) -> Vec<Block> {
        vec![Block::Paragraph(items)]
    }

    /// Inline content of a single-paragraph message.
    fn inl(s: &str) -> Vec<Inline> {
        match parse(s).as_slice() {
            [Block::Paragraph(items)] => items.clone(),
            other => panic!("expected one paragraph for {s:?}, got {other:?}"),
        }
    }

    fn link(url: &str) -> Inline {
        Inline::Link {
            url: url.into(),
            content: None,
        }
    }

    // ---- blocks ----

    #[test]
    fn empty_message() {
        assert!(parse("").is_empty());
        assert!(plain("").is_empty());
    }

    #[test]
    fn plain_paragraph() {
        assert_eq!(parse("hello world"), para(vec![t("hello world")]));
    }

    #[test]
    fn lines_and_blank_lines_are_kept() {
        assert_eq!(
            parse("a\nb\n\nc"),
            para(vec![
                t("a"),
                Inline::LineBreak,
                t("b"),
                Inline::LineBreak,
                Inline::LineBreak,
                t("c")
            ])
        );
    }

    #[test]
    fn crlf_is_normalised() {
        assert_eq!(parse("a\r\nb"), parse("a\nb"));
    }

    #[test]
    fn headings() {
        for (src, level) in [("# Big", 1), ("## Mid", 2), ("### Small", 3)] {
            let name = src.trim_start_matches('#').trim();
            assert_eq!(
                parse(src),
                vec![Block::Heading {
                    level,
                    content: vec![t(name)]
                }]
            );
        }
    }

    #[test]
    fn not_headings() {
        assert_eq!(parse("#### four"), para(vec![t("#### four")]));
        assert_eq!(parse("#nospace"), para(vec![t("#nospace")]));
        assert_eq!(parse("# "), para(vec![t("# ")]));
        assert_eq!(parse("text # not"), para(vec![t("text # not")]));
        assert_eq!(parse("\\# escaped"), para(vec![t("# escaped")]));
    }

    #[test]
    fn heading_with_formatting_and_following_text() {
        assert_eq!(
            parse("# **hi** there\nbody"),
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![Inline::Bold(vec![t("hi")]), t(" there")]
                },
                Block::Paragraph(vec![t("body")])
            ]
        );
    }

    #[test]
    fn subtext() {
        assert_eq!(parse("-# small"), vec![Block::Subtext(vec![t("small")])]);
        assert_eq!(parse("-#nospace"), para(vec![t("-#nospace")]));
    }

    #[test]
    fn single_line_quotes() {
        assert_eq!(
            parse("> one\n> two\nafter"),
            vec![
                Block::Quote(para(vec![t("one"), Inline::LineBreak, t("two")])),
                Block::Paragraph(vec![t("after")])
            ]
        );
    }

    #[test]
    fn quote_needs_a_space() {
        assert_eq!(parse(">no"), para(vec![t(">no")]));
        assert_eq!(parse(">> x"), para(vec![t(">> x")]));
    }

    #[test]
    fn multi_line_quote_takes_the_rest() {
        assert_eq!(
            parse("before\n>>> a\nb\n> c"),
            vec![
                Block::Paragraph(vec![t("before")]),
                Block::Quote(para(vec![
                    t("a"),
                    Inline::LineBreak,
                    t("b"),
                    Inline::LineBreak,
                    t("> c")
                ]))
            ]
        );
    }

    #[test]
    fn quotes_do_not_nest() {
        assert_eq!(parse("> > x"), vec![Block::Quote(para(vec![t("> x")]))]);
    }

    #[test]
    fn quote_can_hold_blocks() {
        assert_eq!(
            parse("> # Title\n> - item"),
            vec![Block::Quote(vec![
                Block::Heading {
                    level: 1,
                    content: vec![t("Title")]
                },
                Block::List(vec![ListItem {
                    depth: 0,
                    marker: ListMarker::Bullet,
                    content: vec![t("item")]
                }])
            ])]
        );
    }

    #[test]
    fn escaped_quote() {
        assert_eq!(parse("\\> not"), para(vec![t("> not")]));
    }

    #[test]
    fn code_block_with_language() {
        assert_eq!(
            parse("```rust\nfn main() {}\n```"),
            vec![Block::Code {
                lang: Some("rust".into()),
                code: "fn main() {}".into()
            }]
        );
    }

    #[test]
    fn code_block_without_language() {
        assert_eq!(
            parse("```\nline 1\n  line 2\n```"),
            vec![Block::Code {
                lang: None,
                code: "line 1\n  line 2".into()
            }]
        );
    }

    #[test]
    fn one_line_code_block() {
        assert_eq!(
            parse("```js code```"),
            vec![Block::Code {
                lang: None,
                code: "js code".into()
            }]
        );
    }

    #[test]
    fn language_only_block_is_content() {
        assert_eq!(
            parse("```js\n```"),
            vec![Block::Code {
                lang: None,
                code: "js".into()
            }]
        );
    }

    #[test]
    fn code_block_mid_line() {
        assert_eq!(
            parse("look: ```x = 1``` done"),
            vec![
                Block::Paragraph(vec![t("look:")]),
                Block::Code {
                    lang: None,
                    code: "x = 1".into()
                },
                Block::Paragraph(vec![t(" done")])
            ]
        );
    }

    #[test]
    fn code_block_content_is_not_parsed() {
        assert_eq!(
            parse("```\n> **not** a quote\n# nor heading <@1>\n```"),
            vec![Block::Code {
                lang: None,
                code: "> **not** a quote\n# nor heading <@1>".into()
            }]
        );
    }

    #[test]
    fn unclosed_code_block_is_text() {
        let blocks = parse("```rust\nfn x");
        assert!(matches!(blocks.as_slice(), [Block::Paragraph(_)]));
        assert_eq!(to_plain_text(&blocks, &NoLookup), "```rust\nfn x");
    }

    #[test]
    fn empty_code_block_is_not_a_block() {
        assert!(
            !parse("``````")
                .iter()
                .any(|b| matches!(b, Block::Code { .. }))
        );
    }

    #[test]
    fn escaped_fence() {
        assert!(
            !parse("\\```a```")
                .iter()
                .any(|b| matches!(b, Block::Code { .. }))
        );
    }

    #[test]
    fn code_block_in_quote() {
        assert_eq!(
            parse("> ```\n> code\n> ```"),
            vec![Block::Quote(vec![Block::Code {
                lang: None,
                code: "code".into()
            }])]
        );
    }

    #[test]
    fn code_block_then_text() {
        assert_eq!(
            parse("```\na\n```\nafter"),
            vec![
                Block::Code {
                    lang: None,
                    code: "a".into()
                },
                Block::Paragraph(vec![t("after")])
            ]
        );
    }

    #[test]
    fn bullet_lists() {
        let bullet = |c: &str| ListItem {
            depth: 0,
            marker: ListMarker::Bullet,
            content: vec![t(c)],
        };
        assert_eq!(
            parse("- a\n* b\ntext"),
            vec![
                Block::List(vec![bullet("a"), bullet("b")]),
                Block::Paragraph(vec![t("text")])
            ]
        );
    }

    #[test]
    fn numbered_lists_renumber_from_start() {
        let blocks = parse("3. a\n1. b\n1. c");
        let Block::List(items) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let nums: Vec<_> = items.iter().map(|i| i.marker).collect();
        assert_eq!(
            nums,
            vec![
                ListMarker::Number(3),
                ListMarker::Number(4),
                ListMarker::Number(5)
            ]
        );
    }

    #[test]
    fn nested_lists() {
        let blocks = parse("- a\n  - b\n    1. c\n  - d\n- e");
        let Block::List(items) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let depths: Vec<u8> = items.iter().map(|i| i.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 1, 0]);
        assert_eq!(items[2].marker, ListMarker::Number(1));
    }

    #[test]
    fn list_nesting_is_capped() {
        let src: String = (0..20)
            .map(|n| format!("{}- x\n", " ".repeat(n * 2)))
            .collect();
        let blocks = parse(&src);
        let Block::List(items) = &blocks[0] else {
            panic!()
        };
        assert!(items.iter().all(|i| (i.depth as usize) < MAX_LIST_DEPTH));
    }

    #[test]
    fn not_lists() {
        assert_eq!(parse("-nospace"), para(vec![t("-nospace")]));
        assert_eq!(parse("1.nospace"), para(vec![t("1.nospace")]));
        assert_eq!(parse("- "), para(vec![t("- ")]));
        assert_eq!(parse("\\- escaped"), para(vec![t("- escaped")]));
        assert_eq!(parse("1234567890. long"), para(vec![t("1234567890. long")]));
    }

    #[test]
    fn list_items_are_formatted() {
        let blocks = parse("- **bold** item");
        let Block::List(items) = &blocks[0] else {
            panic!()
        };
        assert_eq!(
            items[0].content,
            vec![Inline::Bold(vec![t("bold")]), t(" item")]
        );
    }

    // ---- inline ----

    #[test]
    fn bold_italic_underline_strike_spoiler() {
        assert_eq!(inl("**b**"), vec![Inline::Bold(vec![t("b")])]);
        assert_eq!(inl("*i*"), vec![Inline::Italic(vec![t("i")])]);
        assert_eq!(inl("_i_"), vec![Inline::Italic(vec![t("i")])]);
        assert_eq!(inl("__u__"), vec![Inline::Underline(vec![t("u")])]);
        assert_eq!(inl("~~s~~"), vec![Inline::Strike(vec![t("s")])]);
        assert_eq!(inl("||sp||"), vec![Inline::Spoiler(vec![t("sp")])]);
    }

    #[test]
    fn triple_star_is_bold_italic() {
        assert_eq!(
            inl("***x***"),
            vec![Inline::Italic(vec![Inline::Bold(vec![t("x")])])]
        );
    }

    #[test]
    fn triple_underscore_is_underline_italic() {
        assert_eq!(
            inl("___x___"),
            vec![Inline::Italic(vec![Inline::Underline(vec![t("x")])])]
        );
    }

    #[test]
    fn nesting() {
        assert_eq!(
            inl("**bold *both* ~~gone __u__~~**"),
            vec![Inline::Bold(vec![
                t("bold "),
                Inline::Italic(vec![t("both")]),
                t(" "),
                Inline::Strike(vec![t("gone "), Inline::Underline(vec![t("u")])])
            ])]
        );
        assert_eq!(
            inl("||**hidden** bold||"),
            vec![Inline::Spoiler(vec![
                Inline::Bold(vec![t("hidden")]),
                t(" bold")
            ])]
        );
    }

    #[test]
    fn surrounding_text() {
        assert_eq!(
            inl("a **b** c"),
            vec![t("a "), Inline::Bold(vec![t("b")]), t(" c")]
        );
    }

    #[test]
    fn unclosed_delimiters_stay_text() {
        for s in [
            "**a", "*a", "__a", "_a", "~~a", "||a", "`a", "a**", "**", "*", "||||", "~~~~",
        ] {
            assert_eq!(inl(s), vec![t(s)], "{s}");
        }
    }

    #[test]
    fn star_italic_whitespace_rules() {
        assert_eq!(inl("x * a*"), vec![t("x * a*")]);
        assert_eq!(inl("*a *"), vec![t("*a *")]);
        assert_eq!(inl("*a b*"), vec![Inline::Italic(vec![t("a b")])]);
        assert_eq!(inl("2 * 3 * 4"), vec![t("2 * 3 * 4")]);
    }

    #[test]
    fn underscores_inside_words_are_text() {
        assert_eq!(inl("snake_case_name"), vec![t("snake_case_name")]);
        assert_eq!(inl("a_b_ c"), vec![t("a_b_ c")]);
        assert_eq!(
            inl("x _it_ y"),
            vec![t("x "), Inline::Italic(vec![t("it")]), t(" y")]
        );
    }

    #[test]
    fn formatting_across_lines() {
        assert_eq!(
            inl("**a\nb**"),
            vec![Inline::Bold(vec![t("a"), Inline::LineBreak, t("b")])]
        );
        assert_eq!(
            inl("||a\nb||"),
            vec![Inline::Spoiler(vec![t("a"), Inline::LineBreak, t("b")])]
        );
    }

    #[test]
    fn inline_code() {
        assert_eq!(inl("`x`"), vec![Inline::Code("x".into())]);
        assert_eq!(
            inl("a `**not bold**` b"),
            vec![t("a "), Inline::Code("**not bold**".into()), t(" b")]
        );
        assert_eq!(inl("``a`b``"), vec![Inline::Code("a`b".into())]);
        assert_eq!(inl("`` `x` ``"), vec![Inline::Code("`x`".into())]);
        assert_eq!(inl("`<@123>`"), vec![Inline::Code("<@123>".into())]);
    }

    #[test]
    fn mismatched_backticks() {
        assert_eq!(inl("``a`"), vec![t("`"), Inline::Code("a".into())]);
        assert_eq!(inl("``"), vec![t("``")]);
    }

    #[test]
    fn escapes() {
        assert_eq!(inl("\\*not italic\\*"), vec![t("*not italic*")]);
        assert_eq!(inl("\\_\\_x\\_\\_"), vec![t("__x__")]);
        assert_eq!(inl("\\`x\\`"), vec![t("`x`")]);
        assert_eq!(inl("\\<@123>"), vec![t("<@123>")]);
        assert_eq!(inl("\\\\"), vec![t("\\")]);
        // Letters, digits and spaces can't be escaped.
        assert_eq!(inl("\\n \\1 \\ x"), vec![t("\\n \\1 \\ x")]);
        assert_eq!(inl("trailing\\"), vec![t("trailing\\")]);
    }

    #[test]
    fn escaped_closer_inside_span() {
        assert_eq!(inl("**a\\**b**"), vec![Inline::Bold(vec![t("a**b")])]);
    }

    #[test]
    fn bare_urls() {
        assert_eq!(
            inl("see https://example.com/a_b_c?x=1."),
            vec![t("see "), link("https://example.com/a_b_c?x=1"), t(".")]
        );
        assert_eq!(inl("http://x.y"), vec![link("http://x.y")]);
        assert_eq!(inl("HTTPS://X.Y"), vec![link("HTTPS://X.Y")]);
        assert_eq!(inl("https://"), vec![t("https://")]);
        assert_eq!(inl("ftp://x.y"), vec![t("ftp://x.y")]);
    }

    #[test]
    fn url_parentheses() {
        assert_eq!(
            inl("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            vec![link(
                "https://en.wikipedia.org/wiki/Rust_(programming_language)"
            )]
        );
        assert_eq!(
            inl("(https://a.com)"),
            vec![t("("), link("https://a.com"), t(")")]
        );
    }

    #[test]
    fn url_inside_formatting() {
        assert_eq!(
            inl("**https://a.com**"),
            vec![Inline::Bold(vec![link("https://a.com")])]
        );
    }

    #[test]
    fn autolink() {
        assert_eq!(
            inl("<https://a.com/x y>"),
            vec![t("<"), link("https://a.com/x"), t(" y>")]
        );
        assert_eq!(inl("<https://a.com/x>"), vec![link("https://a.com/x")]);
    }

    #[test]
    fn masked_links() {
        assert_eq!(
            inl("[click **me**](https://a.com)"),
            vec![Inline::Link {
                url: "https://a.com".into(),
                content: Some(vec![t("click "), Inline::Bold(vec![t("me")])])
            }]
        );
        assert_eq!(
            inl("[x](<https://a.com/(y)>)"),
            vec![Inline::Link {
                url: "https://a.com/(y)".into(),
                content: Some(vec![t("x")])
            }]
        );
        assert_eq!(
            inl("[x](https://a.com \"title\")"),
            vec![Inline::Link {
                url: "https://a.com".into(),
                content: Some(vec![t("x")])
            }]
        );
        assert_eq!(
            inl("[a [b] c](https://a.com/(x))"),
            vec![Inline::Link {
                url: "https://a.com/(x)".into(),
                content: Some(vec![t("a [b] c")])
            }]
        );
    }

    #[test]
    fn masked_link_text_has_no_links() {
        assert_eq!(
            inl("[https://evil.com](https://a.com)"),
            vec![Inline::Link {
                url: "https://a.com".into(),
                content: Some(vec![t("https://evil.com")])
            }]
        );
    }

    #[test]
    fn not_masked_links() {
        assert_eq!(
            inl("[x](javascript:alert(1))"),
            vec![t("[x](javascript:alert(1))")]
        );
        assert_eq!(
            inl("[](https://a.com)"),
            vec![t("[]("), link("https://a.com"), t(")")]
        );
        assert_eq!(
            inl("[x] (https://a.com)"),
            vec![t("[x] ("), link("https://a.com"), t(")")]
        );
        assert_eq!(
            inl("[x](https://a.com"),
            vec![t("[x]("), link("https://a.com")]
        );
        assert_eq!(inl("[unclosed"), vec![t("[unclosed")]);
    }

    #[test]
    fn mentions() {
        assert_eq!(inl("<@123>"), vec![Inline::User("123".into())]);
        assert_eq!(inl("<@!123>"), vec![Inline::User("123".into())]);
        assert_eq!(inl("<@&9>"), vec![Inline::Role("9".into())]);
        assert_eq!(inl("<#42>"), vec![Inline::Channel("42".into())]);
        assert_eq!(
            inl("hi @everyone and @here!"),
            vec![t("hi "), Inline::Everyone, t(" and "), Inline::Here, t("!")]
        );
    }

    #[test]
    fn bad_mentions_stay_text() {
        for s in [
            "<@abc>",
            "<@>",
            "<#>",
            "<@&x>",
            "<@123",
            "<@12 3>",
            "<@123456789012345678901>",
        ] {
            assert_eq!(inl(s), vec![t(s)], "{s}");
        }
    }

    #[test]
    fn mention_inside_formatting() {
        assert_eq!(
            inl("**<@1>**"),
            vec![Inline::Bold(vec![Inline::User("1".into())])]
        );
    }

    #[test]
    fn custom_emoji() {
        assert_eq!(
            inl("<:pepe:123><a:dance:456>"),
            vec![
                Inline::CustomEmoji {
                    name: "pepe".into(),
                    id: "123".into(),
                    animated: false
                },
                Inline::CustomEmoji {
                    name: "dance".into(),
                    id: "456".into(),
                    animated: true
                }
            ]
        );
        assert_eq!(inl("<:bad name:1>"), vec![t("<:bad name:1>")]);
        assert_eq!(inl("<::1>"), vec![t("<::1>")]);
        assert_eq!(inl("<:x:y>"), vec![t("<:x:y>")]);
    }

    #[test]
    fn unicode_emoji_are_split_out() {
        let items = inl("hi 😀!");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], t("hi "));
        assert!(
            matches!(&items[1], Inline::Emoji { text, url } if text == "😀" && url.ends_with("1f600.png"))
        );
        assert_eq!(items[2], t("!"));
    }

    #[test]
    fn timestamps() {
        assert_eq!(
            inl("<t:1618953630>"),
            vec![Inline::Timestamp {
                unix: 1618953630,
                style: TimestampStyle::ShortDateTime
            }]
        );
        assert_eq!(
            inl("<t:1618953630:R>"),
            vec![Inline::Timestamp {
                unix: 1618953630,
                style: TimestampStyle::Relative
            }]
        );
        assert_eq!(
            inl("<t:-5:d>"),
            vec![Inline::Timestamp {
                unix: -5,
                style: TimestampStyle::ShortDate
            }]
        );
        for s in [
            "<t:1:x>",
            "<t:>",
            "<t:abc>",
            "<t:1:>",
            "<t:99999999999999999999>",
        ] {
            assert_eq!(inl(s), vec![t(s)], "{s}");
        }
    }

    #[test]
    fn slash_commands() {
        assert_eq!(
            inl("</config set:123>"),
            vec![Inline::Command {
                name: "config set".into(),
                id: "123".into()
            }]
        );
        assert_eq!(inl("</:1>"), vec![t("</:1>")]);
    }

    #[test]
    fn plain_mode_keeps_markup_but_resolves_entities() {
        assert_eq!(
            plain("**hi** <@1> `x` https://a.com\n> q"),
            para(vec![
                t("**hi** "),
                Inline::User("1".into()),
                t(" `x` "),
                link("https://a.com"),
                Inline::LineBreak,
                t("> q")
            ])
        );
    }

    // ---- odd input ----

    #[test]
    fn odd_inputs_do_not_panic() {
        let odd = [
            "*",
            "**",
            "***",
            "****",
            "_",
            "__",
            "___",
            "~",
            "~~",
            "|",
            "||",
            "`",
            "```",
            "````",
            "``````",
            "\\",
            "\\\\\\",
            "<",
            "<>",
            "<@",
            "<@!",
            "<@&",
            "<#",
            "<t:",
            "<:",
            "<a:",
            "</",
            "[",
            "[]",
            "[](",
            "[x](",
            "[x](<",
            "[x](<a",
            "> ",
            ">>> ",
            ">>>",
            "# ",
            "-# ",
            "- ",
            "1. ",
            "\n",
            "\n\n\n",
            "**\n**",
            "||\n||",
            "*\n*",
            "é*é*é",
            "**é**",
            "_é_",
            "`é`",
            "😀**😀**😀",
            "[😀](https://😀.com)",
            "<:😀:1>",
            "<t:😀>",
            "\u{0}*\u{0}*",
            "a\\😀",
            "**a*b**c*",
            "*a**b*c**",
            "_a__b_c__",
            "||a~~b||c~~",
            "> ```\n```",
            "```\n> a",
            "- ```a```",
            "# ```x```",
            "https://a.com/**b**",
            "@everyon",
            "@her",
            "\r",
            "\r\r\n",
            "1. a\n   - b\n1. c",
            "  >>> x",
            "[a](https://b.com \"t",
            "[a](https://b.com \"t\" x)",
        ];
        for s in odd {
            let blocks = parse(s);
            let _ = to_plain_text(&blocks, &NoLookup);
            let _ = plain(s);
            let _ = is_jumbo(&blocks);
        }
    }

    #[test]
    fn random_inputs_do_not_panic() {
        const PIECES: &[&str] = &[
            "*",
            "**",
            "_",
            "__",
            "~~",
            "||",
            "`",
            "```",
            "\\",
            "<@1>",
            "<",
            ">",
            "> ",
            "# ",
            "-# ",
            "- ",
            "1. ",
            "\n",
            " ",
            "a",
            "é",
            "😀",
            "[",
            "]",
            "(",
            ")",
            "https://x.y",
            "<t:1:R>",
            "<:e:1>",
            "@here",
            "\"",
            ":",
            "#",
            "!",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..3000 {
            let mut s = String::new();
            for _ in 0..(seed % 40) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push_str(PIECES[(seed % PIECES.len() as u64) as usize]);
            }
            let blocks = parse(&s);
            let _ = to_plain_text(&blocks, &NoLookup);
            let _ = plain(&s);
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let s = format!("{}x{}", "||".repeat(5000), "||".repeat(5000));
        let _ = parse(&s);
        let s = format!("{}x{}", "**_~~".repeat(2000), "~~_**".repeat(2000));
        let _ = parse(&s);
        let s = "[".repeat(5000) + "x" + &"]".repeat(5000) + "(https://a.b)";
        let _ = parse(&s);
    }

    #[test]
    fn long_hostile_input_is_fast() {
        let start = std::time::Instant::now();
        let s = "**a __b ~~c ||d `e *f _g ".repeat(300);
        let _ = parse(&s);
        let s = "*a ".repeat(3000);
        let _ = parse(&s);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    // ---- helpers ----

    struct Names;
    impl Lookup for Names {
        fn user_name(&self, id: &str) -> Option<String> {
            (id == "1").then(|| "leo".into())
        }
        fn role(&self, id: &str) -> Option<(String, Option<Color32>)> {
            (id == "2").then(|| ("mods".into(), None))
        }
        fn channel_name(&self, id: &str) -> Option<String> {
            (id == "3").then(|| "general".into())
        }
    }

    #[test]
    fn plain_text_resolves_and_hides() {
        let blocks = parse("**hi** <@1> <@9> <@&2> <#3> ||secret|| <:pog:5>\n> quoted\n- item");
        assert_eq!(
            to_plain_text(&blocks, &Names),
            "hi @leo @unknown-user @mods #general [spoiler] :pog:\nquoted\nitem"
        );
    }

    #[test]
    fn jumbo_detection() {
        assert!(is_jumbo(&parse("😀 <:a:1>")));
        assert!(is_jumbo(&parse("😀\n😀")));
        assert!(!is_jumbo(&parse("😀 hi")));
        assert!(!is_jumbo(&parse("# 😀")));
        assert!(!is_jumbo(&parse("")));
        assert!(!is_jumbo(&parse(&"😀".repeat(31))));
    }

    #[test]
    fn timestamp_formats() {
        let now = Utc.with_ymd_and_hms(2021, 4, 20, 16, 20, 30).unwrap();
        let ts = now.timestamp();
        let f = |style| format_timestamp(ts, style, &now);
        assert_eq!(f(TimestampStyle::ShortTime), "16:20");
        assert_eq!(f(TimestampStyle::LongTime), "16:20:30");
        assert_eq!(f(TimestampStyle::ShortDate), "20/04/2021");
        assert_eq!(f(TimestampStyle::LongDate), "20 April 2021");
        assert_eq!(f(TimestampStyle::ShortDateTime), "20 April 2021 16:20");
        assert_eq!(
            f(TimestampStyle::LongDateTime),
            "Tuesday, 20 April 2021 16:20"
        );
        assert_eq!(f(TimestampStyle::Relative), "a few seconds ago");
        assert_eq!(
            format_timestamp(i64::MAX / 2, TimestampStyle::ShortDate, &now),
            "Invalid Date"
        );
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(-30), "a few seconds ago");
        assert_eq!(relative_time(60), "in a minute");
        assert_eq!(relative_time(-5 * 60), "5 minutes ago");
        assert_eq!(relative_time(3600), "in an hour");
        assert_eq!(relative_time(-5 * 3600), "5 hours ago");
        assert_eq!(relative_time(86400), "in a day");
        assert_eq!(relative_time(-3 * 86400), "3 days ago");
        assert_eq!(relative_time(30 * 86400), "in a month");
        assert_eq!(relative_time(-90 * 86400), "3 months ago");
        assert_eq!(relative_time(400 * 86400), "in a year");
        assert_eq!(relative_time(-3 * 365 * 86400), "3 years ago");
    }

    #[test]
    fn timestamp_style_letters() {
        for (c, s) in [
            ("t", TimestampStyle::ShortTime),
            ("T", TimestampStyle::LongTime),
            ("d", TimestampStyle::ShortDate),
            ("D", TimestampStyle::LongDate),
            ("f", TimestampStyle::ShortDateTime),
            ("F", TimestampStyle::LongDateTime),
            ("R", TimestampStyle::Relative),
        ] {
            assert_eq!(TimestampStyle::from_letter(c), Some(s));
        }
        assert_eq!(TimestampStyle::from_letter("x"), None);
    }

    // ---- cache ----

    #[test]
    fn cache_reuses_and_reparses() {
        let mut cache = ParseCache::new(8);
        let a = cache.get("1", "**a**", true);
        let b = cache.get("1", "**a**", true);
        assert!(Arc::ptr_eq(&a, &b));
        let edited = cache.get("1", "**b**", true);
        assert!(!Arc::ptr_eq(&a, &edited));
        assert_eq!(
            edited[0],
            Block::Paragraph(vec![Inline::Bold(vec![t("b")])])
        );
        let raw = cache.get("1", "**b**", false);
        assert_eq!(raw[0], Block::Paragraph(vec![t("**b**")]));
        assert_eq!(cache.len(), 1);
        cache.remove("1");
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_is_capped_and_keeps_recent() {
        let mut cache = ParseCache::new(8);
        for i in 0..8 {
            cache.get(&i.to_string(), "x", true);
        }
        // Touch "0" so it's the most recently used.
        cache.get("0", "x", true);
        for i in 8..40 {
            cache.get(&i.to_string(), "x", true);
            assert!(cache.len() <= 8);
        }
        assert!(cache.map.contains_key("39"));
        cache.clear();
        assert!(cache.is_empty());
    }
}
