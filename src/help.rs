//! The user manual shown in Help ▸ Documentation: Markdown pages embedded in
//! the binary, a Keyboard Shortcuts page generated from the keymap in effect,
//! links between pages, and search across all of them.
//!
//! Pages write shortcuts as `{{key:command_id}}` (an id keybindings.json
//! accepts); they're replaced with the key in effect right now, so the manual
//! shows this platform's keys and follows the user's rebinding.

use crate::keybindings::{self, Target, What};
use crate::markdown::{self, Block, Inline};

/// File name of the generated shortcuts page (links use it like any page).
pub const SHORTCUTS: &str = "shortcuts.md";
pub const WELCOME: &str = "welcome.md";

/// A manual page: its file name (links use it), the section it's listed
/// under in the contents, and its Markdown (None: generated).
pub struct Page {
    pub id: &'static str,
    pub section: &'static str,
    source: Option<&'static str>,
}

macro_rules! page {
    ($section:literal, $file:literal) => {
        Page { id: $file, section: $section, source: Some(include_str!(concat!("../docs/manual/", $file))) }
    };
}

/// Every page, in the order of the table of contents.
pub static PAGES: &[Page] = &[
    page!("Getting Started", "welcome.md"),
    page!("Using Strata", "editing.md"),
    page!("Using Strata", "find.md"),
    page!("Using Strata", "snippets.md"),
    page!("Using Strata", "projects.md"),
    page!("Using Strata", "panes.md"),
    page!("Using Strata", "compare.md"),
    page!("Using Strata", "printing.md"),
    page!("Using Strata", "terminal.md"),
    page!("Using Strata", "markdown.md"),
    page!("Using Strata", "notes.md"),
    page!("Using Strata", "build.md"),
    page!("Using Strata", "git.md"),
    page!("Using Strata", "review.md"),
    page!("Using Strata", "ai.md"),
    page!("Customizing", "appearance.md"),
    page!("Customizing", "settings.md"),
    page!("Customizing", "keybindings.md"),
    Page { id: SHORTCUTS, section: "Customizing", source: None },
    page!("Customizing", "languages.md"),
    page!("Customizing", "encodings.md"),
    page!("Reference", "cli.md"),
    page!("Reference", "troubleshooting.md"),
    page!("Reference", "about.md"),
];

pub fn page(id: &str) -> Option<&'static Page> {
    PAGES.iter().find(|p| p.id == id)
}

impl Page {
    /// The page's Markdown with shortcuts filled in from the keymap in effect.
    pub fn text(&self) -> String {
        match self.source {
            Some(src) => expand_keys(src).0,
            None => shortcuts_markdown(),
        }
    }

    /// The first heading, for the contents and the window title.
    pub fn title(&self) -> String {
        let src = self.source.map_or_else(|| "# Keyboard Shortcuts".to_owned(), str::to_owned);
        src.lines().find_map(|l| l.strip_prefix("# ")).map_or_else(|| self.id.trim_end_matches(".md").to_owned(), |t| t.trim().to_owned())
    }
}

// ---- shortcuts -------------------------------------------------------------------

/// A shortcut as bold text. Not inline code: the monospace font has no ⌃.
fn keys(s: &str) -> String {
    let mut out = String::from("**");
    for c in s.chars() {
        if "\\`*_[]<>|#!~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out + "**"
}

/// Inline code that survives backticks in the text.
fn code(s: &str) -> String {
    let s = s.replace('|', "\\|");
    if s.contains('`') { format!("`` {s} ``") } else { format!("`{s}`") }
}

/// The live shortcut for a keybindings.json id, as Markdown, or None if
/// the id isn't a command.
pub fn key_for(id: &str) -> Option<String> {
    match keybindings::resolve(id, &serde_json::Map::new()) {
        Ok(What::Run(t)) => Some(keybindings::current().label(t).map_or_else(|| "*no shortcut*".to_owned(), |l| keys(&l))),
        _ => None,
    }
}

/// Replace every `{{key:id}}`. Also returns the ids that aren't commands.
pub fn expand_keys(src: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(src.len());
    let mut unknown = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("{{key:") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 6..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[at..]);
            rest = "";
            break;
        };
        let id = after[..end].trim();
        match key_for(id) {
            Some(k) => out.push_str(&k),
            None => {
                unknown.push(id.to_owned());
                out.push_str(&code(id));
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    (out, unknown)
}

/// Contents sections of the shortcuts page, by the prefix of command names.
const CATEGORIES: &[&str] = &[
    "File", "Edit", "Selection", "Find", "Goto", "LSP", "View", "Code Folding", "Layout", "Tools", "Review", "AI", "Markdown", "Notes", "Terminal", "Project", "Preferences", "Help", "Strata",
];

/// Keyboard Shortcuts, from the keymap in effect: defaults plus keybindings.json.
pub fn shortcuts_markdown() -> String {
    let keymap = keybindings::current();
    let platform = if crate::keys::MAC {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    };
    let mut out = format!(
        "# Keyboard Shortcuts\n\n\
         Every command and the keys that run it on {platform}, including any changes in your `keybindings.json` \
         (see [Customizing Key Bindings](keybindings.md)). This page is made from the key bindings in effect, so it \
         always matches what Strata does. Commands without a key can be bound by their id; every command is also in \
         the Command Palette ({}).\n",
        key_for("command_palette").unwrap_or_default()
    );
    let mut groups: Vec<(String, Vec<(String, Target)>)> = Vec::new();
    for t in keybindings::all_targets() {
        let name = t.name();
        let (cat, rest) = match name.split_once(": ") {
            Some((c, r)) => (c.to_owned(), r.to_owned()),
            None => ("General".to_owned(), name.clone()),
        };
        match groups.iter_mut().find(|g| g.0 == cat) {
            Some(g) => g.1.push((rest, t)),
            None => groups.push((cat, vec![(rest, t)])),
        }
    }
    let rank = |c: &str| CATEGORIES.iter().position(|x| *x == c).unwrap_or(if c == "General" { 0 } else { CATEGORIES.len() });
    groups.sort_by_key(|g| (rank(&g.0), g.0.clone()));
    for (cat, items) in groups {
        out.push_str(&format!("\n## {cat}\n\n| Command | Shortcut | Command id |\n|:--|:--|:--|\n"));
        for (name, t) in items {
            let keys = keymap.labels(t);
            let keys = if keys.is_empty() { "—".to_owned() } else { keys.iter().map(|k| self::keys(k)).collect::<Vec<_>>().join(" or ") };
            let id = match t.id() {
                (id, Some(args)) => format!("{id} {args}"),
                (id, None) => id.to_owned(),
            };
            out.push_str(&format!("| {} | {keys} | {} |\n", name.replace('|', "\\|"), code(&id)));
        }
    }
    let ctx = keymap.context_bindings();
    if !ctx.is_empty() {
        out.push_str("\n## Context Bindings\n\nThese keys only work in the context shown.\n\n| Keys | Command | Context |\n|:--|:--|:--|\n");
        for (keys, what, when) in ctx {
            out.push_str(&format!("| {} | {} | {} |\n", self::keys(&keys), what.replace('|', "\\|"), code(&when)));
        }
    }
    let (cmd, find_all, replace_all) = if crate::keys::MAC { ("⌘", "⌥ ↩", "⌘ ↩") } else { ("Ctrl", "Alt+Enter", "Ctrl+Enter") };
    let (cut, copy, paste) = if crate::keys::MAC { ("⌘ X", "⌘ C", "⌘ V") } else { ("Ctrl+X", "Ctrl+C", "Ctrl+V") };
    out.push_str(&format!(
        "\n## Other Keys\n\nThese keys aren't commands, so they don't appear above.\n\n\
         | Where | Keys | What they do |\n|:--|:--|:--|\n\
         | Everywhere | {} {} {} | Cut, copy and paste, unless you bind those keys to something else |\n\
         | Find bar | **Enter** / **Shift+Enter** | Find the next / previous match |\n\
         | Find bar | {} | Find All: a cursor on every match |\n\
         | Replace field | {} | Replace all matches |\n\
         | Snippet | **Tab** / **Shift+Tab** | Next / previous field; **Esc** finishes |\n\
         | Editor | **{}-click** | Add a cursor |\n\
         | Help | **Esc** | Close the help (or clear the search) |\n\
         | Help | {} | Search the documentation |\n\
         | Help | **Alt+←** / **Alt+→** | Back / forward |\n",
        keys(cut),
        keys(copy),
        keys(paste),
        keys(find_all),
        keys(replace_all),
        cmd,
        key_for("show_find").unwrap_or_default(),
    ));
    out
}

// ---- links -----------------------------------------------------------------------

/// Where a link in a page goes.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// A manual page (by id), maybe at a heading.
    Page(String, Option<String>),
    /// A web (or other scheme) URL.
    External(String),
    /// Not a page and not a URL.
    Broken(String),
}

pub fn resolve_link(current: &str, url: &str) -> Link {
    let scheme = url.split_once(':').is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    if scheme {
        return Link::External(url.to_owned());
    }
    let (path, anchor) = match url.split_once('#') {
        Some((p, a)) => (p, Some(a.to_owned()).filter(|a| !a.is_empty())),
        None => (url, None),
    };
    let path = path.trim_start_matches("./");
    let id = if path.is_empty() { current } else { path };
    match page(id) {
        Some(p) => Link::Page(p.id.to_owned(), anchor),
        None => Link::Broken(url.to_owned()),
    }
}

// ---- search ----------------------------------------------------------------------

/// One heading's worth of a page, as plain text.
pub struct Section {
    pub page: &'static str,
    pub page_title: String,
    /// The heading (the page title for text before the first heading).
    pub heading: String,
    /// Anchor of the heading; None for the top of the page.
    pub slug: Option<String>,
    pub text: String,
}

fn inline_text(i: &Inline, out: &mut String) {
    out.push_str(&i.text);
    out.push(' ');
}

fn block_text(b: &Block, out: &mut String) {
    match b {
        Block::Heading { text, .. } | Block::Paragraph(text) => inline_text(text, out),
        Block::Code { code, .. } => {
            out.push_str(code);
            out.push(' ');
        }
        Block::Quote { blocks, .. } | Block::Footnote { blocks, .. } => blocks.iter().for_each(|b| block_text(b, out)),
        Block::List { items, .. } => items.iter().flat_map(|i| &i.blocks).for_each(|b| block_text(b, out)),
        Block::Table { head, rows, .. } => head.iter().chain(rows.iter().flatten()).for_each(|c| inline_text(c, out)),
        Block::Html(h) | Block::Embed { target: h, .. } => out.push_str(h),
        Block::Rule => {}
    }
}

/// Every page split at its headings, for search.
pub fn sections() -> Vec<Section> {
    let mut out = Vec::new();
    for p in PAGES {
        let title = p.title();
        let mut cur = Section { page: p.id, page_title: title.clone(), heading: title.clone(), slug: None, text: String::new() };
        for b in markdown::parse(&p.text()) {
            if let Block::Heading { level, text, slug } = &b.block
                && *level > 1
            {
                let next = Section { page: p.id, page_title: title.clone(), heading: text.text.clone(), slug: Some(slug.clone()), text: String::new() };
                out.push(std::mem::replace(&mut cur, next));
                continue;
            }
            if !matches!(b.block, Block::Heading { level: 1, .. }) {
                block_text(&b.block, &mut cur.text);
            }
        }
        out.push(cur);
    }
    out.retain(|s| !s.text.trim().is_empty() || s.slug.is_some());
    out
}

/// A search result: where, and a bit of the text around the first match.
#[derive(Debug, Clone)]
pub struct Hit {
    pub page: &'static str,
    pub page_title: String,
    pub heading: String,
    pub slug: Option<String>,
    pub snippet: String,
}

/// Sections containing every word of `query` (any case), best first.
pub fn search(sections: &[Section], query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let phrase = words.join(" ");
    let mut scored: Vec<(i64, usize)> = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        let heading = s.heading.to_lowercase();
        let title = s.page_title.to_lowercase();
        let text = s.text.to_lowercase();
        if !words.iter().all(|w| heading.contains(w) || title.contains(w) || text.contains(w)) {
            continue;
        }
        let mut score: i64 = 0;
        for w in &words {
            score += if heading.contains(w) { 12 } else { 0 } + if title.contains(w) { 4 } else { 0 };
            score += text.matches(w.as_str()).count().min(6) as i64;
        }
        if heading.contains(&phrase) {
            score += 25;
        }
        if text.contains(&phrase) {
            score += 6;
        }
        scored.push((score, i));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(60)
        .map(|(_, i)| {
            let s = &sections[i];
            Hit { page: s.page, page_title: s.page_title.clone(), heading: s.heading.clone(), slug: s.slug.clone(), snippet: snippet(&s.text, &words) }
        })
        .collect()
}

/// About a line of `text` around the first word that matches.
fn snippet(text: &str, words: &[String]) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    // Lowercasing can change byte lengths; fall back to the start then.
    let at = words.iter().filter_map(|w| lower.find(w.as_str())).min().filter(|_| lower.len() == flat.len()).unwrap_or(0);
    let chars: Vec<(usize, char)> = flat.char_indices().collect();
    let pos = chars.iter().position(|&(b, _)| b >= at).unwrap_or(0);
    let start = pos.saturating_sub(50);
    let end = (pos + 110).min(chars.len());
    let mut s: String = chars[start..end].iter().map(|&(_, c)| c).collect();
    if start > 0 {
        s = format!("…{}", s.trim_start());
    }
    if end < chars.len() {
        s = format!("{}…", s.trim_end().trim_end_matches('…'));
    }
    s
}

/// The page's top-level heading anchors.
#[cfg(test)]
pub fn anchors(blocks: &[markdown::TopBlock]) -> Vec<&str> {
    blocks
        .iter()
        .filter_map(|b| match &b.block {
            Block::Heading { slug, .. } => Some(slug.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links_in(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Heading { text, .. } | Block::Paragraph(text) => out.extend(text.links.iter().cloned()),
                Block::Quote { blocks, .. } | Block::Footnote { blocks, .. } => links_in(blocks, out),
                Block::List { items, .. } => items.iter().for_each(|i| links_in(&i.blocks, out)),
                Block::Table { head, rows, .. } => head.iter().chain(rows.iter().flatten()).for_each(|c| out.extend(c.links.iter().cloned())),
                _ => {}
            }
        }
    }

    #[test]
    fn every_page_parses_with_a_title_and_known_shortcuts() {
        assert_eq!(PAGES.len(), 24);
        for p in PAGES {
            if let Some(src) = p.source {
                let (_, unknown) = expand_keys(src);
                assert!(unknown.is_empty(), "{}: unknown {{{{key:…}}}} ids {unknown:?}", p.id);
                assert!(src.starts_with("# "), "{} starts with its title", p.id);
                assert!(!src.contains("<img") && !src.contains("](http://"), "{}", p.id);
            }
            let blocks = markdown::parse(&p.text());
            assert!(blocks.len() > 3, "{} has content", p.id);
            assert!(matches!(&blocks[0].block, Block::Heading { level: 1, .. }), "{}", p.id);
            assert!(!p.title().is_empty());
            let a = anchors(&blocks);
            let mut seen = Vec::new();
            for s in &a {
                assert!(!seen.contains(s), "{}: two headings make #{s}", p.id);
                seen.push(*s);
            }
        }
    }

    #[test]
    fn every_link_and_anchor_resolves() {
        let mut checked = 0;
        for p in PAGES {
            let blocks: Vec<Block> = markdown::parse(&p.text()).into_iter().map(|b| b.block).collect();
            let mut links = Vec::new();
            links_in(&blocks, &mut links);
            for url in links {
                match resolve_link(p.id, &url) {
                    Link::Page(target, anchor) => {
                        if let Some(anchor) = anchor {
                            let tb = markdown::parse(&page(&target).unwrap().text());
                            assert!(anchors(&tb).contains(&anchor.as_str()), "{}: link {url} has no heading #{anchor} in {target}", p.id);
                        }
                        checked += 1;
                    }
                    Link::External(u) => assert!(u.starts_with("https://") || u.starts_with("mailto:"), "{}: {u}", p.id),
                    Link::Broken(u) => panic!("{}: broken link {u}", p.id),
                }
            }
        }
        assert!(checked > 30, "pages link to each other ({checked})");
        assert_eq!(resolve_link("find.md", "#goto-anything"), Link::Page("find.md".into(), Some("goto-anything".into())));
        assert_eq!(resolve_link("a.md", "snippets.md#fields"), Link::Page("snippets.md".into(), Some("fields".into())));
        assert!(matches!(resolve_link("a.md", "nope.md"), Link::Broken(_)));
        assert!(matches!(resolve_link("a.md", "https://example.com"), Link::External(_)));
    }

    #[test]
    fn every_setting_is_documented() {
        let settings = page("settings.md").unwrap().source.unwrap();
        let serde_json::Value::Object(fields) = serde_json::to_value(crate::session::Settings::default()).unwrap() else { panic!() };
        assert!(fields.len() > 40);
        for name in fields.keys() {
            assert!(settings.contains(&format!("`{name}`")), "settings.md doesn't document `{name}`");
        }
    }

    #[test]
    fn shortcuts_page_lists_every_command_with_live_keys() {
        let md = shortcuts_markdown();
        for &c in crate::commands::Cmd::ALL {
            assert!(md.contains(&format!("`{}", c.id())), "{} missing", c.id());
        }
        for c in crate::app::menu_model::EDITOR_COMMANDS {
            let (id, _) = Target::Editor(c.id).id();
            assert!(md.contains(&format!("`{id}`")), "{id} missing");
        }
        let palette = crate::keybindings::current().label(Target::Cmd(crate::commands::Cmd::CommandPalette)).unwrap();
        assert!(md.contains(&palette));
        assert!(md.contains("## File") && md.contains("## Edit") && md.contains("## Help"));
        // It parses into tables.
        assert!(markdown::parse(&md).iter().filter(|b| matches!(b.block, Block::Table { .. })).count() > 8);
        // Rebinding shows up.
        let (k, _) = crate::keybindings::Keymap::with_user(r#"[ { "keys": ["ctrl+alt+shift+h"], "command": "documentation" } ]"#).unwrap();
        crate::keybindings::install(k);
        let md = shortcuts_markdown();
        let label = crate::keys::label(crate::keys::CTRL_ALT.plus(egui::Modifiers::SHIFT), egui::Key::H);
        assert!(md.contains(&label), "{label}");
        assert_eq!(key_for("documentation"), Some(keys(&label)));
        crate::keybindings::install(crate::keybindings::Keymap::defaults());
        assert_eq!(key_for("documentation"), Some(keys(&crate::keys::label(crate::keys::NONE, egui::Key::F1))));
        assert_eq!(key_for("no_such_command"), None);
    }

    #[test]
    fn key_placeholders_expand() {
        let (s, unknown) = expand_keys("Press {{key:command_palette}} or {{key:bogus}}. {{key:");
        assert_eq!(unknown, ["bogus"]);
        assert!(!s.contains("{{key:command_palette}}") && s.contains("`bogus`") && s.ends_with("{{key:"));
        // Punctuation keys survive as bold text.
        for k in ["⌃ `", "⌘ \\", "Ctrl+[", "⌥ ⌘ *", "Ctrl+|"] {
            let b = markdown::parse(&keys(k));
            let Block::Paragraph(p) = &b[0].block else { panic!("{k}") };
            assert_eq!(p.text, k);
            assert!(p.runs.iter().all(|r| r.style.bold), "{k}");
        }
    }

    #[test]
    fn search_finds_the_right_pages() {
        let all = sections();
        let top = |q: &str| search(&all, q).into_iter().take(5).map(|h| h.page).collect::<Vec<_>>();
        assert!(top("snippet fields").contains(&"snippets.md"), "{:?}", top("snippet fields"));
        assert!(top("hot exit").iter().any(|p| *p == "welcome.md" || *p == "troubleshooting.md"));
        assert!(top("Where").contains(&"find.md") || top("open folders").contains(&"find.md"));
        assert!(top("keybindings.json").contains(&"keybindings.md"));
        assert!(top("terminal").contains(&"terminal.md"));
        assert!(top("TOML").iter().any(|p| *p == "languages.md" || *p == "build.md"));
        assert!(search(&all, "zzqqxx").is_empty());
        assert!(search(&all, "  ").is_empty());
        let hit = &search(&all, "build system")[0];
        assert!(!hit.snippet.is_empty());
    }
}
