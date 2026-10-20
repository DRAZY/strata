//! Updating links when a note (or any vault file) is renamed: every
//! `[[link]]` and Markdown link that pointed at the old name is rewritten to
//! the new one, keeping its `#heading`, `|alias`, embed `!` and style (a bare
//! name stays a bare name when that's still unambiguous; a path keeps its
//! folders). Links in code are left alone.
//!
//! A rename can also change what *other* links mean: renaming `Plan.md` to
//! `Ideas.md` makes `[[Ideas]]` elsewhere ambiguous. Those links are
//! rewritten to a path that keeps pointing where they pointed before.

use super::parse::{self, LinkKind};
use super::resolve::{Files, LinkFormat, is_note, without_md};
use std::ops::Range;
use std::path::Path;

/// The vault's files before and after one rename.
#[derive(Clone, Debug)]
pub struct Rename {
    pub from: String,
    pub to: String,
    before: Files,
    after: Files,
    format: LinkFormat,
}

impl Rename {
    /// `paths` are the vault's files (either side of the rename); `from` and
    /// `to` are vault-relative.
    pub fn new(paths: &[String], from: &str, to: &str, format: LinkFormat) -> Rename {
        let swap = |p: &String, a: &str, b: &str| if p.eq_ignore_ascii_case(a) || p == a { b.to_owned() } else { p.clone() };
        let mut before: Vec<String> = paths.iter().map(|p| swap(p, to, from)).collect();
        if !before.iter().any(|p| p == from) {
            before.push(from.to_owned());
        }
        before.sort();
        before.dedup();
        let mut after: Vec<String> = before.iter().map(|p| swap(p, from, to)).collect();
        after.sort();
        Rename { from: from.to_owned(), to: to.to_owned(), before: Files::new(before), after: Files::new(after), format }
    }

    /// The renamed file's old and new names, without `.md`, lowercase: links
    /// whose last part is one of these may need a change.
    pub fn names(&self) -> [String; 2] {
        let name = |p: &str| without_md(p.rsplit('/').next().unwrap_or(p)).to_lowercase();
        [name(&self.from), name(&self.to)]
    }

    /// Where the note now at `rel` was before the rename.
    fn before_rel<'a>(&'a self, rel: &'a str) -> &'a str {
        if rel == self.to { &self.from } else { rel }
    }

    /// The link edits in `text`, the note now at `rel` (vault-relative):
    /// byte ranges to replace and their new text, in order.
    pub fn edits(&self, rel: &str, text: &str) -> Vec<(Range<usize>, String)> {
        let src_before = self.before_rel(rel);
        let mut out = Vec::new();
        for l in parse::parse(text).links {
            if l.path.is_empty() {
                continue;
            }
            let md = l.kind == LinkKind::Markdown;
            let Some(old) = self.before.resolve(&l.path, Some(src_before)) else { continue };
            let old_rel = self.before.path(old);
            let new_rel = if old_rel == self.from { self.to.as_str() } else { old_rel };
            let Some(target) = self.after.find(new_rel) else { continue };
            // A link that was unambiguous must stay so; one that already
            // needed a tie-break is left alone while it still finds the same file.
            let unique_before = self.before.resolve_unique(&l.path, Some(src_before)).is_some();
            let now = if unique_before { self.after.resolve_unique(&l.path, Some(rel)) } else { self.after.resolve(&l.path, Some(rel)) };
            if now == Some(target) {
                continue;
            }
            let path = self.new_path(&l.path, target, rel, md);
            let written = if md && !l.angle { parse::percent_encode(&path) } else { path };
            out.push((l.path_range.clone(), written));
        }
        // Links are listed by where they start, but a link inside another's
        // text (`[![i](Old.png)](Old.png)`) has its path earlier: edits must
        // go in text order, and never overlap.
        out.sort_by_key(|(r, _)| r.start);
        out.dedup_by(|b, a| b.0.start < a.0.end);
        out
    }

    /// A path to `target` from `rel` in the style of the old link `old`.
    fn new_path(&self, old: &str, target: u32, rel: &str, md: bool) -> String {
        let full = self.after.path(target);
        let name = full.rsplit('/').next().unwrap_or(full);
        // Keep `.md` if the link had it; a wikilink to a note otherwise goes without.
        let keep_ext = md || !is_note(full) || is_note(old);
        let name = if keep_ext { name } else { without_md(name) };
        let dir = old.rsplit_once('/').map(|(d, _)| d);
        let mut tries = Vec::new();
        if let Some(dir) = dir {
            tries.push(format!("{dir}/{name}"));
        } else {
            tries.push(name.to_owned());
        }
        let generated = self.after.link_path(target, rel, self.format, md);
        let generated = if keep_ext && !md && is_note(full) && !is_note(&generated) { format!("{generated}.md") } else { generated };
        tries.push(generated);
        tries.into_iter().find(|p| self.after.resolve_unique(p, Some(rel)) == Some(target)).unwrap_or_else(|| self.after.link_path(target, rel, LinkFormat::Absolute, md))
    }
}

/// Lets a rename's link updates run through Replace in Files' disk job
/// (atomic writes, kept originals for undo).
pub struct Rewriter {
    pub root: std::path::PathBuf,
    pub rename: Rename,
}

impl crate::replace::Rewrite for Rewriter {
    fn edits(&self, path: &Path, text: &str) -> Vec<(Range<usize>, String)> {
        let Ok(rest) = path.strip_prefix(&self.root) else { return Vec::new() };
        let rel = rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        self.rename.edits(&rel, text)
    }
}

/// `text` with `edits` made (in order, not overlapping; any that aren't are skipped).
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, s) in edits {
        debug_assert!(r.start >= at, "edits out of order");
        if r.start < at || r.end > text.len() {
            continue;
        }
        out.push_str(&text[at..r.start]);
        out.push_str(s);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn rewrite(r: &Rename, rel: &str, text: &str) -> String {
        apply(text, &r.edits(rel, text))
    }

    #[test]
    fn rewrites_every_kind_of_link_keeping_its_parts() {
        let files = paths(&["Plan.md", "Projects/Other.md", "Notes/Old Name.md", "att/pic.png"]);
        let r = Rename::new(&files, "Notes/Old Name.md", "Notes/New Name.md", LinkFormat::Shortest);
        let text = "[[Old Name]] [[old name#Heading|shown]] ![[Old Name#^blk]] [[Notes/Old Name]] [[Old Name.md]]\n[x](Notes/Old%20Name.md#Part) [y](<Notes/Old Name.md>) [[Other]] [[Missing]]\n";
        assert_eq!(
            rewrite(&r, "Plan.md", text),
            "[[New Name]] [[New Name#Heading|shown]] ![[New Name#^blk]] [[Notes/New Name]] [[New Name.md]]\n[x](Notes/New%20Name.md#Part) [y](<Notes/New Name.md>) [[Other]] [[Missing]]\n"
        );
        // Relative Markdown links from inside the folder.
        assert_eq!(rewrite(&r, "Notes/Sibling.md", "[s](Old%20Name.md) [u](../Plan.md)"), "[s](New%20Name.md) [u](../Plan.md)");
        // The renamed note's own links to itself.
        assert_eq!(rewrite(&r, "Notes/New Name.md", "[[Old Name#Top]] [[#Top]]"), "[[New Name#Top]] [[#Top]]");
    }

    #[test]
    fn only_real_links_change() {
        let files = paths(&["A.md", "Old.md"]);
        let r = Rename::new(&files, "Old.md", "New.md", LinkFormat::Shortest);
        let text = "`[[Old]]` [[Old]]\n```\n[[Old]]\n```\n    text Old and [[Old]]\n\\[[Old]] [[Oldish]]\n";
        assert_eq!(rewrite(&r, "A.md", text), "`[[Old]]` [[New]]\n```\n[[Old]]\n```\n    text Old and [[New]]\n\\[[Old]] [[Oldish]]\n");
        let table = "| a |\n|---|\n| [[Old\\|alias]] |\n";
        assert_eq!(rewrite(&r, "A.md", table), "| a |\n|---|\n| [[New\\|alias]] |\n");
    }

    #[test]
    fn ambiguity_from_the_new_name_is_resolved_with_paths() {
        // Renaming x/Plan.md to x/Ideas.md, while y/Ideas.md exists.
        let files = paths(&["x/Plan.md", "y/Ideas.md", "z/Note.md"]);
        let r = Rename::new(&files, "x/Plan.md", "x/Ideas.md", LinkFormat::Shortest);
        // From z/: [[Plan]] meant x/Plan; [[Ideas]] meant y/Ideas, and both must still.
        let out = rewrite(&r, "z/Note.md", "[[Plan]] and [[Ideas]]");
        assert_eq!(out, "[[x/Ideas]] and [[y/Ideas]]");
        // From x/ the bare name now means the renamed note (same folder wins).
        assert_eq!(rewrite(&r, "x/Other.md", "[[Plan]] [[Ideas]]"), "[[Ideas]] [[y/Ideas]]");
    }

    #[test]
    fn links_inside_link_text_are_edited_in_order() {
        let r = Rename::new(&paths(&["A.md", "Old.png"]), "Old.png", "New.png", LinkFormat::Shortest);
        let text = "[![pic](Old.png)](Old.png) and [![[Old.png]]](Old.png)";
        let edits = r.edits("A.md", text);
        assert!(edits.windows(2).all(|w| w[0].0.end <= w[1].0.start), "{edits:?}");
        assert_eq!(apply(text, &edits), "[![pic](New.png)](New.png) and [![[New.png]]](New.png)");
    }

    #[test]
    fn the_index_may_already_know_the_new_name() {
        // A rescan after the rename lists the new path: the result is the same.
        let r = Rename::new(&paths(&["A.md", "New.md"]), "Old.md", "New.md", LinkFormat::Shortest);
        assert_eq!(rewrite(&r, "A.md", "[[Old]]"), "[[New]]");
        assert_eq!(r.names(), ["old".to_owned(), "new".to_owned()]);
    }

    #[test]
    fn rewriter_works_on_absolute_paths() {
        use crate::replace::Rewrite;
        let root = std::path::PathBuf::from("/vault");
        let rw = Rewriter { root: root.clone(), rename: Rename::new(&paths(&["A.md", "Old.md"]), "Old.md", "New.md", LinkFormat::Shortest) };
        assert_eq!(rw.edits(&root.join("A.md"), "[[Old]]"), vec![(2..5, "New".to_owned())]);
        assert!(rw.edits(Path::new("/elsewhere/A.md"), "[[Old]]").is_empty());
    }
}
