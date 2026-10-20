//! Reading one note the way Obsidian does: `[[wikilinks]]` (with `#heading`,
//! `#^block` and `|alias`), Markdown links to local files, `#tags`, headings,
//! block ids and the YAML frontmatter's `tags` and `aliases`.
//!
//! Links and tags inside fenced code blocks, `$$` math blocks and inline code
//! don't count, as in Obsidian. Everything is byte offsets into the text, plus
//! the 0-based line, so the editor (char columns) and the rename rewrite
//! (byte edits) can both use one scan.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[path#sub|alias]]`
    Wiki,
    /// `[text](path.md#sub)`
    Markdown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub kind: LinkKind,
    /// `![[…]]` or `![…](…)`: shown in place rather than linked.
    pub embed: bool,
    /// The whole link, `!` included.
    pub range: Range<usize>,
    /// The path as written (what a rename rewrites); empty for `[[#heading]]`.
    pub path_range: Range<usize>,
    /// The path, trimmed and (Markdown links) percent-decoded.
    pub path: String,
    /// What follows `#`: a heading (`a#b` for nested ones) or `^block`.
    pub subpath: Option<String>,
    /// `[[path|alias]]`'s alias, or a Markdown link's text.
    pub alias: Option<String>,
    /// The Markdown destination was written in `<…>` (spaces kept as is).
    pub angle: bool,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    /// Without the `#`, as written (Obsidian matches tags ignoring case).
    pub name: String,
    /// `#` included.
    pub range: Range<usize>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub line: usize,
}

/// What a note contains.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub links: Vec<Link>,
    /// Inline `#tags` (frontmatter tags are in `fm_tags`).
    pub tags: Vec<Tag>,
    pub headings: Vec<Heading>,
    /// `^block-id`s and the line each marks.
    pub blocks: Vec<(String, usize)>,
    /// The frontmatter, `---` lines included.
    pub frontmatter: Option<Range<usize>>,
    pub fm_tags: Vec<String>,
    pub aliases: Vec<String>,
}

/// Lines as (byte offset, text without the line break).
pub fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |seg| {
        let start = at;
        at += seg.len();
        let content = seg.strip_suffix('\n').map_or(seg, |s| s.strip_suffix('\r').unwrap_or(s));
        (start, content)
    })
}

/// The frontmatter's range: `---` on the first line, up to the next line
/// that is `---` (or `...`). None without a closing line.
pub fn frontmatter(text: &str) -> Option<Range<usize>> {
    let mut it = lines(text);
    let (_, first) = it.next()?;
    if first.trim_end() != "---" {
        return None;
    }
    for (start, line) in it {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            let end = (start + line.len()).min(text.len());
            // The line break after the closing line belongs to it too.
            let end = text[end..].strip_prefix("\r\n").or_else(|| text[end..].strip_prefix('\n')).map_or(end, |rest| text.len() - rest.len());
            return Some(0..end);
        }
    }
    None
}

/// Read a note.
pub fn parse(text: &str) -> Note {
    let mut note = Note::default();
    let fm = frontmatter(text);
    let body_from = fm.as_ref().map_or(0, |r| r.end);
    if let Some(r) = &fm {
        read_frontmatter(&text[r.clone()], &mut note);
        // Links in property values ("[[Note]]") count, as in Obsidian.
        for (line_no, (start, line)) in lines(&text[r.clone()]).enumerate() {
            let masked = Vec::new();
            note.links.extend(wikilinks(line, start, line_no, &masked));
        }
    }
    note.frontmatter = fm;
    let mut fence: Option<(u8, usize)> = None;
    let mut math = false;
    let mut last_text_line = None;
    for (line_no, (start, line)) in lines(text).enumerate() {
        if start < body_from {
            continue;
        }
        let body = strip_quotes(line);
        let trimmed = body.trim_start_matches([' ', '\t']);
        if let Some((ch, n)) = fence {
            let run = trimmed.bytes().take_while(|&b| b == ch).count();
            if run >= n && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
            continue;
        }
        if trimmed.trim_end() == "$$" {
            math = !math;
            continue;
        }
        if math {
            continue;
        }
        let mut masked = code_spans(line);
        let wiki = wikilinks(line, start, line_no, &masked);
        masked.extend(wiki.iter().map(|l| l.range.start - start..l.range.end - start));
        let md = markdown_links(line, start, line_no, &masked);
        masked.extend(md.iter().map(|(l, _)| l.range.start - start..l.range.end - start));
        // External links aren't notes, but nothing inside them is a tag either.
        note.links.extend(wiki);
        note.links.extend(md.into_iter().filter_map(|(l, local)| local.then_some(l)));
        note.tags.extend(tags(line, start, line_no, &masked));
        if let Some(h) = heading(line, line_no) {
            note.headings.push(h);
        }
        match block_id(line) {
            Some((id, true)) => {
                if let Some(prev) = last_text_line {
                    note.blocks.push((id, prev));
                }
            }
            Some((id, false)) => note.blocks.push((id, line_no)),
            None => {}
        }
        if !line.trim().is_empty() {
            last_text_line = Some(line_no);
        }
    }
    note.links.sort_by_key(|l| l.range.start);
    note
}

/// Is byte `at` inside a fenced code block or `$$` math block?
pub fn in_code_block(text: &str, at: usize) -> bool {
    let body_from = frontmatter(text).map_or(0, |r| r.end);
    let (mut fence, mut math) = (None::<(u8, usize)>, false);
    for (start, line) in lines(text) {
        if start + line.len() >= at && start <= at {
            return fence.is_some() || math;
        }
        if start < body_from {
            continue;
        }
        let trimmed = strip_quotes(line).trim_start_matches([' ', '\t']);
        if let Some((ch, n)) = fence {
            let run = trimmed.bytes().take_while(|&b| b == ch).count();
            if run >= n && trimmed[run..].trim().is_empty() {
                fence = None;
            }
        } else if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
        } else if trimmed.trim_end() == "$$" {
            math = !math;
        }
    }
    fence.is_some() || math
}

/// `> ` prefixes of block quotes and callouts.
fn strip_quotes(mut line: &str) -> &str {
    loop {
        let t = line.trim_start_matches([' ', '\t']);
        match t.strip_prefix('>') {
            Some(rest) => line = rest,
            None => return line,
        }
    }
}

/// A fence line (three or more backticks or tildes): its char and length.
fn fence_open(trimmed: &str) -> Option<(u8, usize)> {
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|&b| b == ch).count();
    // A backtick fence's info string can't contain a backtick.
    (run >= 3 && !(ch == b'`' && trimmed[run..].contains('`'))).then_some((ch, run))
}

/// Inline code spans on one line, as byte ranges in the line (backticks included).
fn code_spans(line: &str) -> Vec<Range<usize>> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' || escaped(b, i) {
            i += 1;
            continue;
        }
        let n = b[i..].iter().take_while(|&&c| c == b'`').count();
        let mut j = i + n;
        let mut closed = None;
        while j < b.len() {
            if b[j] == b'`' {
                let m = b[j..].iter().take_while(|&&c| c == b'`').count();
                if m == n {
                    closed = Some(j + m);
                    break;
                }
                j += m;
            } else {
                j += 1;
            }
        }
        match closed {
            Some(end) => {
                out.push(i..end);
                i = end;
            }
            None => i += n,
        }
    }
    out
}

/// Byte `i` is preceded by an odd number of backslashes.
fn escaped(b: &[u8], i: usize) -> bool {
    b[..i].iter().rev().take_while(|&&c| c == b'\\').count() % 2 == 1
}

fn inside(masked: &[Range<usize>], i: usize) -> bool {
    masked.iter().any(|r| r.contains(&i))
}

/// `[[…]]` and `![[…]]` on one line; `start` is the line's offset in the text.
fn wikilinks(line: &str, start: usize, line_no: usize, masked: &[Range<usize>]) -> Vec<Link> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if !(b[i] == b'[' && b[i + 1] == b'[') || escaped(b, i) || inside(masked, i) {
            i += 1;
            continue;
        }
        let open = i + 2;
        let Some(close) = line[open..].find("]]").map(|k| open + k) else { break };
        let inner = &line[open..close];
        // `[[a [[b]]`: the inner one is the link.
        if let Some(k) = inner.rfind("[[") {
            i = open + k;
            continue;
        }
        if inner.trim().is_empty() || inner.contains('\n') {
            i = close + 2;
            continue;
        }
        let embed = i > 0 && b[i - 1] == b'!' && !escaped(b, i - 1);
        let (target_end, alias) = match inner.find('|') {
            Some(p) => (p, Some(inner[p + 1..].trim().to_owned())),
            None => (inner.len(), None),
        };
        let mut target = &inner[..target_end];
        // In a table `|` is written `\|`: the backslash isn't part of the path.
        if alias.is_some() && target.ends_with('\\') {
            target = &target[..target.len() - 1];
        }
        let (path_raw, subpath) = match target.find('#') {
            Some(h) => (&target[..h], Some(target[h + 1..].trim().to_owned())),
            None => (target, None),
        };
        let path_start = open + (path_raw.len() - path_raw.trim_start().len());
        let path_end = open + path_raw.trim_end().len();
        out.push(Link {
            kind: LinkKind::Wiki,
            embed,
            range: start + if embed { i - 1 } else { i }..start + close + 2,
            path_range: start + path_start..start + path_end.max(path_start),
            path: path_raw.trim().to_owned(),
            subpath: subpath.filter(|s| !s.is_empty()),
            alias: alias.filter(|a| !a.is_empty()),
            angle: false,
            line: line_no,
        });
        i = close + 2;
    }
    out
}

/// `[text](dest)` and `![alt](dest)` on one line, each with whether it
/// points at a local file (no URL scheme).
fn markdown_links(line: &str, start: usize, line_no: usize, masked: &[Range<usize>]) -> Vec<(Link, bool)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'[' || escaped(b, i) || inside(masked, i) {
            i += 1;
            continue;
        }
        // The matching `]`, allowing brackets nested inside (an image in a link).
        let mut depth = 0;
        let mut close = None;
        for (k, &c) in b.iter().enumerate().skip(i) {
            match c {
                b'[' if !escaped(b, k) => depth += 1,
                b']' if !escaped(b, k) => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            i += 1;
            continue;
        };
        if b.get(close + 1) != Some(&b'(') {
            i += 1;
            continue;
        }
        let Some(dest) = destination(line, close + 2) else {
            i += 1;
            continue;
        };
        let embed = i > 0 && b[i - 1] == b'!' && !escaped(b, i - 1);
        let raw = &line[dest.path.clone()];
        let local = crate::links::scheme(raw).is_none() && !raw.starts_with("//") && !raw.starts_with("\\\\");
        let decoded = if dest.angle { raw.to_owned() } else { percent_decode(raw) };
        let (path, subpath) = match decoded.find('#') {
            Some(h) => (decoded[..h].to_owned(), Some(decoded[h + 1..].to_owned())),
            None => (decoded.clone(), None),
        };
        // `path_range` is the part before `#`, as written.
        let raw_path_len = raw.find('#').unwrap_or(raw.len());
        let text = line[i + 1..close].trim();
        out.push((
            Link {
                kind: LinkKind::Markdown,
                embed,
                range: start + if embed { i - 1 } else { i }..start + dest.end,
                path_range: start + dest.path.start..start + dest.path.start + raw_path_len,
                path: path.trim().to_owned(),
                subpath: subpath.filter(|s| !s.is_empty()),
                alias: (!text.is_empty()).then(|| text.to_owned()),
                angle: dest.angle,
                line: line_no,
            },
            local,
        ));
        // Carry on inside the text: `[![img](a.png)](b.md)` has two links.
        i += 1;
    }
    out
}

struct Dest {
    /// The destination itself, without `<>` or the title.
    path: Range<usize>,
    angle: bool,
    /// Just past the closing `)`.
    end: usize,
}

/// A link destination starting at byte `at` (just after `(`).
fn destination(line: &str, at: usize) -> Option<Dest> {
    let b = line.as_bytes();
    let mut i = at;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let (path, angle) = if b.get(i) == Some(&b'<') {
        let end = i + 1 + line[i + 1..].find('>')?;
        let p = i + 1..end;
        i = end + 1;
        (p, true)
    } else {
        let s = i;
        let mut depth = 0;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 1,
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b' ' | b'\t' => break,
                _ => {}
            }
            i += 1;
        }
        (s..i.min(b.len()), false)
    };
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    // An optional title: "…", '…' or (…).
    if let Some(&q) = b.get(i).filter(|&&q| q == b'"' || q == b'\'' || q == b'(') {
        let closer = if q == b'(' { b')' } else { q };
        let end = i + 1 + b[i + 1..].iter().position(|&c| c == closer)?;
        i = end + 1;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
    }
    (b.get(i) == Some(&b')') && !path.is_empty()).then(|| Dest { path, angle, end: i + 1 })
}

/// `%20` and friends back to text; left as is when that isn't valid UTF-8.
pub fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// A path for a Markdown link destination: what would end it or start a
/// subpath is escaped (space, `%`, parentheses, `#`, `<`, `>`, `?`).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '#' => out.push_str("%23"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '?' => out.push_str("%3F"),
            '[' => out.push_str("%5B"),
            ']' => out.push_str("%5D"),
            c if c.is_control() => out.push_str(&format!("%{:02X}", c as u32 & 0xFF)),
            c => out.push(c),
        }
    }
    out
}

/// Can `c` be part of a tag? Letters and digits in any script, `_`, `-`,
/// `/` (nested tags) and symbols such as emoji; not spaces or punctuation.
pub fn tag_char(c: char) -> bool {
    if c.is_alphanumeric() || matches!(c, '_' | '-' | '/') {
        return true;
    }
    if c.is_ascii() || c.is_whitespace() || c.is_control() {
        return false;
    }
    let u = c as u32;
    // Latin-1 punctuation, general and CJK punctuation, full-width ASCII punctuation.
    !matches!(u, 0x00A0..=0x00BF | 0x00D7 | 0x00F7 | 0x2000..=0x206F | 0x2E00..=0x2E7F | 0x3000..=0x303F | 0xFE30..=0xFE4F | 0xFF00..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65)
}

/// Is this (without `#`) a tag? Not empty and not only digits (`#1984` isn't).
pub fn valid_tag(name: &str) -> bool {
    !name.is_empty() && name.chars().all(tag_char) && name.chars().any(|c| !c.is_ascii_digit() && c != '/')
}

/// `#tags` on one line: `#` at the start or after whitespace, not in code or links.
fn tags(line: &str, start: usize, line_no: usize, masked: &[Range<usize>]) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    let mut iter = line.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let after_space = prev.is_none_or(char::is_whitespace);
        prev = Some(c);
        if c != '#' || !after_space || inside(masked, i) {
            continue;
        }
        let body_start = i + 1;
        let mut end = body_start;
        while let Some(&(k, d)) = iter.peek() {
            if !tag_char(d) {
                break;
            }
            end = k + d.len_utf8();
            prev = Some(d);
            iter.next();
        }
        let name = line[body_start..end].trim_end_matches('/');
        if valid_tag(name) {
            out.push(Tag { name: name.to_owned(), range: start + i..start + body_start + name.len(), line: line_no });
        }
    }
    out
}

/// An ATX heading (`# Title`, up to three spaces in).
fn heading(line: &str, line_no: usize) -> Option<Heading> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let t = &line[indent..];
    let level = t.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    // A closing run of #s (after a space) isn't part of the text.
    let mut text = rest.trim();
    let stripped = text.trim_end_matches('#');
    if stripped.len() < text.len() && (stripped.is_empty() || stripped.ends_with([' ', '\t'])) {
        text = stripped.trim_end();
    }
    Some(Heading { level: level as u8, text: text.to_owned(), line: line_no })
}

/// A `^block-id` ending the line; true when it's alone on its line (it
/// then names the block above).
fn block_id(line: &str) -> Option<(String, bool)> {
    let t = line.trim_end();
    let caret = t.rfind('^')?;
    let id = &t[caret + 1..];
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let before = &t[..caret];
    if before.trim().is_empty() {
        return Some((id.to_owned(), true));
    }
    before.ends_with([' ', '\t']).then(|| (id.to_owned(), false))
}

/// The heading text Obsidian compares a `#subpath` with: no link
/// punctuation, single spaces, ignoring case.
pub fn heading_key(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if matches!(c, '#' | '^' | '[' | ']' | '|' | ':' | '%') { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// `tags`, `tag`, `aliases` and `alias` from YAML frontmatter (both `key: a, b`,
/// `key: [a, b]` and a `- item` list on the lines after).
fn read_frontmatter(fm: &str, note: &mut Note) {
    let body: Vec<&str> = lines(fm).map(|(_, l)| l).collect();
    let mut i = 1;
    while i + 1 < body.len() {
        let line = body[i];
        i += 1;
        let Some((key, value)) = line.split_once(':') else { continue };
        if key.starts_with([' ', '\t', '-']) {
            continue;
        }
        let key = key.trim().to_ascii_lowercase();
        let is_tags = key == "tags" || key == "tag";
        if !is_tags && key != "aliases" && key != "alias" {
            continue;
        }
        let value = value.trim();
        let mut items: Vec<String> = Vec::new();
        if let Some(list) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            items.extend(list.split(',').map(unquote));
        } else if !value.is_empty() {
            let v = unquote(value);
            if is_tags {
                items.extend(v.split([',', ' ']).map(str::to_owned));
            } else {
                items.extend(v.split(',').map(|s| s.trim().to_owned()));
            }
        } else {
            while i + 1 < body.len() {
                let t = body[i].trim_start();
                let Some(item) = t.strip_prefix("- ").or_else(|| (t == "-").then_some("")) else { break };
                items.push(unquote(item));
                i += 1;
            }
        }
        for item in items {
            let item = item.trim();
            if is_tags {
                let name = item.strip_prefix('#').unwrap_or(item);
                if valid_tag(name) {
                    note.fm_tags.push(name.to_owned());
                }
            } else if !item.is_empty() {
                note.aliases.push(item.to_owned());
            }
        }
    }
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    let inner = t.strip_prefix('"').and_then(|t| t.strip_suffix('"')).or_else(|| t.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')));
    inner.unwrap_or(t).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<Link> {
        parse(text).links
    }

    fn tag_names(text: &str) -> Vec<String> {
        parse(text).tags.into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn wikilinks_with_headings_blocks_and_aliases() {
        let text = "See [[Note]], [[Folder/Other#Heading|shown]] and ![[pic.png]].\nAlso [[Note#^abc123]] and [[#Local heading]].";
        let l = links(text);
        assert_eq!(l.len(), 5);
        assert_eq!((l[0].path.as_str(), l[0].subpath.as_deref(), l[0].alias.as_deref(), l[0].embed), ("Note", None, None, false));
        assert_eq!(&text[l[0].range.clone()], "[[Note]]");
        assert_eq!(&text[l[0].path_range.clone()], "Note");
        assert_eq!((l[1].path.as_str(), l[1].subpath.as_deref(), l[1].alias.as_deref()), ("Folder/Other", Some("Heading"), Some("shown")));
        assert_eq!(&text[l[1].path_range.clone()], "Folder/Other");
        assert!(l[2].embed);
        assert_eq!(&text[l[2].range.clone()], "![[pic.png]]");
        assert_eq!((l[3].path.as_str(), l[3].subpath.as_deref(), l[3].line), ("Note", Some("^abc123"), 1));
        assert_eq!((l[4].path.as_str(), l[4].subpath.as_deref()), ("", Some("Local heading")));
        assert_eq!(l[4].path_range.len(), 0);
    }

    #[test]
    fn escaped_pipes_in_tables() {
        let text = "| a | b |\n|---|---|\n| [[My Note\\|alias]] | [[x]] |\n";
        let l = links(text);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].path.as_str(), l[0].alias.as_deref()), ("My Note", Some("alias")));
        assert_eq!(&text[l[0].path_range.clone()], "My Note", "the backslash isn't part of the path");
        assert_eq!(l[1].path, "x");
    }

    #[test]
    fn not_links_in_code_or_when_escaped_or_empty() {
        let text = "`[[code]]` and ``a `[[still code]]` b`` \\[[escaped]] [[]] [[ ]]\n```\n[[fenced]] #fencedtag\n```\n~~~js\n[[tilde]]\n~~~\n$$\n[[math]]\n$$\n> ```\n> [[quoted fence]]\n> ```\n[[real]]";
        let l = links(text);
        assert_eq!(l.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["real"]);
        assert!(tag_names(text).is_empty());
        // An unclosed fence hides the rest, as in Obsidian.
        assert!(links("```\n[[a]]\n").is_empty());
        // Nested opener: the inner link wins.
        assert_eq!(links("[[a [[b]]")[0].path, "b");
    }

    #[test]
    fn markdown_links_to_local_files() {
        let text = "[A](My%20Note.md) [B](<Other note.md#Part>) [web](https://x.dev/#a) ![img](att/p.png \"title\") [anchor](#Here) [[w]]";
        let l = links(text);
        let got: Vec<(&str, Option<&str>, LinkKind, bool)> = l.iter().map(|l| (l.path.as_str(), l.subpath.as_deref(), l.kind, l.embed)).collect();
        assert_eq!(
            got,
            [
                ("My Note.md", None, LinkKind::Markdown, false),
                ("Other note.md", Some("Part"), LinkKind::Markdown, false),
                ("att/p.png", None, LinkKind::Markdown, true),
                ("", Some("Here"), LinkKind::Markdown, false),
                ("w", None, LinkKind::Wiki, false),
            ]
        );
        assert_eq!(&text[l[0].path_range.clone()], "My%20Note.md");
        assert_eq!(&text[l[1].path_range.clone()], "Other note.md");
        assert!(l[1].angle);
        assert_eq!(links("[x](a(b).md)")[0].path, "a(b).md", "balanced parentheses");
        assert!(links("[x](mailto:a@b.c) [y](obsidian://open?x)").is_empty());
        let nested = links("[![i](a.png)](b.md)");
        assert_eq!(nested.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["b.md", "a.png"]);
    }

    #[test]
    fn tags_follow_obsidian_rules() {
        let text = "# Heading #intitle\n#start text #nested/tag, #with-dash_and_underscore #1984 #y1984 word#no `#code` [[n#heading]] #über #日本語 #tag.\n";
        assert_eq!(tag_names(text), ["intitle", "start", "nested/tag", "with-dash_and_underscore", "y1984", "über", "日本語", "tag"]);
        assert!(tag_names("## Not a tag\n").is_empty());
        assert!(tag_names("[x](https://a.b/#frag) http://x.y/#z").is_empty());
        let t = &parse("a #t/ b").tags[0];
        assert_eq!(t.name, "t");
        assert_eq!(t.range, 2..4);
        assert!(valid_tag("a") && !valid_tag("123") && !valid_tag("a b") && !valid_tag(""));
    }

    #[test]
    fn frontmatter_tags_aliases_and_links() {
        let text = "---\ntitle: X\ntags: [one, \"#two\"]\naliases:\n  - First Alias\n  - 'Second'\nrelated: \"[[Linked Note]]\"\n---\n# Body #inline\n";
        let n = parse(text);
        assert_eq!(n.fm_tags, ["one", "two"]);
        assert_eq!(n.aliases, ["First Alias", "Second"]);
        assert_eq!(n.links.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["Linked Note"]);
        assert_eq!(n.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["inline"]);
        assert_eq!(n.headings[0].text, "Body #inline");
        assert_eq!(n.headings[0].line, 8);
        assert_eq!(parse("---\ntags: a, b c\nalias: Solo\n---\n").fm_tags, ["a", "b", "c"]);
        assert_eq!(parse("---\ntags: a, b c\nalias: Solo\n---\n").aliases, ["Solo"]);
        assert!(parse("---\nno end\n#t\n").frontmatter.is_none());
        assert_eq!(parse("---\r\ntags: x\r\n---\r\nbody").frontmatter, Some(0..19));
    }

    #[test]
    fn headings_and_block_ids() {
        let n = parse("# One\n## Two ##\n####### no\n#not heading\nPara text ^para-1\n\n- item\n\n^after\n    # indented\n");
        assert_eq!(n.headings.iter().map(|h| (h.level, h.text.as_str())).collect::<Vec<_>>(), [(1, "One"), (2, "Two")]);
        assert_eq!(n.blocks, [("para-1".to_owned(), 4), ("after".to_owned(), 6)]);
        assert_eq!(heading_key("Hello: World [x]"), heading_key("hello world x"));
    }

    #[test]
    fn code_blocks_are_known_by_position() {
        let text = "a [[x\n```\n[[y\n```\n$$\nm\n$$\nz";
        let at = |s: &str| text.find(s).unwrap();
        assert!(!in_code_block(text, at("[[x")));
        assert!(in_code_block(text, at("[[y")));
        assert!(in_code_block(text, at("m\n")));
        assert!(!in_code_block(text, at("z")));
    }

    #[test]
    fn percent_coding_round_trips() {
        assert_eq!(percent_decode("My%20Note%C3%A9.md"), "My Noteé.md");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        let name = "a (b) #c 50%.md";
        assert_eq!(percent_decode(&percent_encode(name)), name);
        assert!(!percent_encode(name).contains([' ', '(', ')', '#']));
    }
}
