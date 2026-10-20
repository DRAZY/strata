//! Which file a link points to, and how to write a link to a file, the way
//! Obsidian does it: paths are relative to the vault folder with `/`,
//! matching ignores case, `.md` is optional for notes, and a bare name (or
//! the end of a path) finds the file wherever it is.
//!
//! When several files match (`[[README]]` with a README in many folders),
//! the one in the linking note's own folder wins, then the one closest to the
//! vault folder, then the first by path, so the answer never depends on the
//! order files were found in.

use std::collections::HashMap;

/// How new links are written (Obsidian's "New link format").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinkFormat {
    /// The name alone when it's unique in the vault, else the full path.
    #[default]
    Shortest,
    /// Relative to the linking note's folder.
    Relative,
    /// From the vault folder.
    Absolute,
}

/// Is `rel` a note (Markdown)?
pub fn is_note(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    crate::markdown::EXTENSIONS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// `rel` without its Markdown extension (notes are linked without it).
pub fn without_md(rel: &str) -> &str {
    if is_note(rel) { rel.rsplit_once('.').map_or(rel, |(stem, _)| stem) } else { rel }
}

fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

/// `.` and `..` resolved; None when `..` climbs out of the vault.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// The files of one vault, for resolving links.
#[derive(Clone, Debug, Default)]
pub struct Files {
    /// Paths relative to the vault folder, with `/`.
    paths: Vec<String>,
    lower: Vec<String>,
    by_path: HashMap<String, u32>,
    /// Lowercase file name, and a note's name without `.md`, to files.
    by_name: HashMap<String, Vec<u32>>,
}

impl Files {
    pub fn new(paths: Vec<String>) -> Self {
        let lower: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
        let mut by_path = HashMap::with_capacity(paths.len());
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::with_capacity(paths.len() * 2);
        for (i, l) in lower.iter().enumerate() {
            let i = i as u32;
            by_path.entry(l.clone()).or_insert(i);
            let name = file_name(l);
            by_name.entry(name.to_owned()).or_default().push(i);
            if is_note(name) {
                by_name.entry(without_md(name).to_owned()).or_default().push(i);
            }
        }
        Files { paths, lower, by_path, by_name }
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn path(&self, i: u32) -> &str {
        &self.paths[i as usize]
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The file at `rel` (ignoring case).
    pub fn find(&self, rel: &str) -> Option<u32> {
        self.by_path.get(&rel.to_lowercase()).copied()
    }

    fn exact(&self, lower: &str) -> Option<u32> {
        self.by_path.get(lower).copied().or_else(|| if is_note(lower) { None } else { self.by_path.get(&format!("{lower}.md")).copied() })
    }

    /// The file `link` (a link path, without `#subpath`) points to from the
    /// note at `from`: the path taken from the note's own folder, then from
    /// the vault folder, then any file whose path ends with the link.
    pub fn resolve(&self, link: &str, from: Option<&str>) -> Option<u32> {
        self.lookup(link, from).map(|(i, _)| i)
    }

    /// Like `resolve`, but only when no tie-break was needed to choose: the
    /// link would find this file in any app that resolves the same way.
    pub fn resolve_unique(&self, link: &str, from: Option<&str>) -> Option<u32> {
        self.lookup(link, from).filter(|(_, unique)| *unique).map(|(i, _)| i)
    }

    fn lookup(&self, link: &str, from: Option<&str>) -> Option<(u32, bool)> {
        let link = link.trim().replace('\\', "/");
        if link.is_empty() {
            return from.and_then(|f| self.find(f)).map(|i| (i, true));
        }
        let lower = link.to_lowercase();
        let from_dir = from.map(parent).unwrap_or("").to_lowercase();
        if let Some(p) = normalize(&format!("{from_dir}/{lower}"))
            && let Some(i) = self.exact(&p)
        {
            return Some((i, true));
        }
        // `./` and `../` paths are relative to the note only.
        if lower.starts_with("./") || lower.starts_with("../") {
            return None;
        }
        let lower = normalize(&lower)?;
        if let Some(i) = self.exact(&lower) {
            return Some((i, true));
        }
        let candidates = self.by_name.get(file_name(&lower))?;
        let ends = |p: &str, tail: &str| p == tail || p.strip_suffix(tail).is_some_and(|head| head.ends_with('/'));
        let with_md = format!("{lower}.md");
        let mut matching: Vec<u32> = candidates
            .iter()
            .copied()
            .filter(|&i| {
                let p = &self.lower[i as usize];
                ends(p, &lower) || (!is_note(&lower) && is_note(p) && ends(p, &with_md)) || (is_note(p) && ends(without_md(p), &lower))
            })
            .collect();
        matching.sort_by_key(|&i| {
            let p = &self.lower[i as usize];
            (parent(p) != from_dir, p.matches('/').count(), p.clone())
        });
        matching.dedup();
        let first = *matching.first()?;
        let same_folder = matching.iter().filter(|&&i| parent(&self.lower[i as usize]) == from_dir).count();
        Some((first, matching.len() == 1 || same_folder == 1))
    }

    /// How to write a link from the note at `from` to file `target`, in
    /// `format`: for a wikilink without `.md`, for a Markdown link with it
    /// (not yet percent-encoded). Always a path that resolves back to `target`.
    pub fn link_path(&self, target: u32, from: &str, format: LinkFormat, markdown: bool) -> String {
        let full = self.path(target);
        let shown = |p: &str| if markdown { p.to_owned() } else { without_md(p).to_owned() };
        let absolute = shown(full);
        let relative = {
            let from_dir: Vec<&str> = parent(from).split('/').filter(|s| !s.is_empty()).collect();
            let to: Vec<&str> = full.split('/').collect();
            let common = from_dir.iter().zip(&to).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            let mut parts: Vec<String> = vec!["..".to_owned(); from_dir.len() - common];
            parts.extend(to[common..].iter().map(|s| s.to_string()));
            shown(&parts.join("/"))
        };
        let shortest = {
            let name = file_name(full);
            let key = name.to_lowercase();
            let key = if is_note(&key) && !markdown { without_md(&key).to_owned() } else { key };
            if self.by_name.get(&key).is_some_and(|v| v.len() == 1) { shown(name) } else { absolute.clone() }
        };
        let preferred = match format {
            LinkFormat::Shortest => shortest,
            LinkFormat::Relative => relative.clone(),
            LinkFormat::Absolute => absolute.clone(),
        };
        // A path that resolves elsewhere from this note (a same-named file in
        // its folder, say) is no good: fall back to one that doesn't.
        [preferred, absolute, relative.clone()]
            .into_iter()
            .find(|p| self.resolve_unique(p, Some(from)) == Some(target))
            .unwrap_or(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Files {
        Files::new(paths.iter().map(|s| s.to_string()).collect())
    }

    fn resolved<'a>(f: &'a Files, link: &str, from: &str) -> Option<&'a str> {
        f.resolve(link, Some(from)).map(|i| f.path(i))
    }

    #[test]
    fn exact_paths_names_and_suffixes_ignoring_case() {
        let f = files(&["Inbox/Idea.md", "Projects/Strata/Plan.md", "attachments/Pic.PNG", "Top.md", "notes/a.b.md"]);
        assert_eq!(resolved(&f, "Projects/Strata/Plan", "Top.md"), Some("Projects/Strata/Plan.md"));
        assert_eq!(resolved(&f, "plan", "Top.md"), Some("Projects/Strata/Plan.md"), "name alone, any case");
        assert_eq!(resolved(&f, "Strata/Plan", "Top.md"), Some("Projects/Strata/Plan.md"), "end of a path");
        assert_eq!(resolved(&f, "rata/Plan", "Top.md"), None, "only whole folder names");
        assert_eq!(resolved(&f, "Plan.md", "Top.md"), Some("Projects/Strata/Plan.md"), ".md is optional");
        assert_eq!(resolved(&f, "pic.png", "Top.md"), Some("attachments/Pic.PNG"));
        assert_eq!(resolved(&f, "pic", "Top.md"), None, "attachments need their extension");
        assert_eq!(resolved(&f, "a.b", "Top.md"), Some("notes/a.b.md"), "dots in note names");
        assert_eq!(resolved(&f, "", "Inbox/Idea.md"), Some("Inbox/Idea.md"), "[[#heading]] is the note itself");
        assert_eq!(resolved(&f, "Missing", "Top.md"), None);
    }

    #[test]
    fn duplicates_prefer_the_same_folder_then_the_shallowest() {
        let f = files(&["README.md", "HomeLab/README.md", "Projects/Strata/README.md", "HomeLab/STATUS.md", "Projects/Strata/STATUS.md", "Projects/TuberX/STATUS.md"]);
        assert_eq!(resolved(&f, "README", "HomeLab/Ideas.md"), Some("HomeLab/README.md"));
        assert_eq!(resolved(&f, "README", "Projects/Strata/Plan.md"), Some("Projects/Strata/README.md"));
        assert_eq!(resolved(&f, "README", "Ideas/x.md"), Some("README.md"), "the vault folder's is shallowest");
        assert_eq!(resolved(&f, "STATUS", "Ideas/x.md"), Some("HomeLab/STATUS.md"), "then by path");
        assert_eq!(resolved(&f, "TuberX/STATUS", "HomeLab/STATUS.md"), Some("Projects/TuberX/STATUS.md"));
    }

    #[test]
    fn spaces_unicode_and_relative_paths() {
        let f = files(&["My Notes/Café Été.md", "Ελληνικά/Σημείωση.md", "a/b/c.md", "a/d.md"]);
        assert_eq!(resolved(&f, "café été", "x.md"), Some("My Notes/Café Été.md"));
        assert_eq!(resolved(&f, "CAFÉ ÉTÉ", "x.md"), Some("My Notes/Café Été.md"), "case ignored beyond ASCII");
        assert_eq!(resolved(&f, "σημείωση", "x.md"), Some("Ελληνικά/Σημείωση.md"));
        assert_eq!(resolved(&f, "../d", "a/b/c.md"), Some("a/d.md"));
        assert_eq!(resolved(&f, "./c", "a/b/x.md"), Some("a/b/c.md"));
        assert_eq!(resolved(&f, "../../../x", "a/b/c.md"), None, "never outside the vault");
        // A path from the note's folder comes first.
        assert_eq!(f.resolve("c.md", Some("a/b/x.md")).map(|i| f.path(i)), Some("a/b/c.md"));
        assert_eq!(f.resolve("../d.md", Some("a/b/x.md")).map(|i| f.path(i)), Some("a/d.md"));
        assert_eq!(f.resolve("My%20Notes", None), None, "decoding is the parser's job");
    }

    #[test]
    fn new_links_follow_the_format_and_always_resolve_back() {
        let f = files(&["Inbox/Idea.md", "Projects/Idea.md", "Projects/Strata/Plan.md", "att/pic.png", "Inbox/Plan.md"]);
        let plan = f.find("Projects/Strata/Plan.md").unwrap();
        let unique = f.find("att/pic.png").unwrap();
        let idea = f.find("Projects/Idea.md").unwrap();
        assert_eq!(f.link_path(unique, "Inbox/Idea.md", LinkFormat::Shortest, false), "pic.png");
        assert_eq!(f.link_path(idea, "Inbox/Idea.md", LinkFormat::Shortest, false), "Projects/Idea", "not unique: the full path");
        assert_eq!(f.link_path(plan, "Inbox/Idea.md", LinkFormat::Relative, false), "../Projects/Strata/Plan");
        assert_eq!(f.link_path(plan, "Inbox/Idea.md", LinkFormat::Absolute, true), "Projects/Strata/Plan.md");
        // "Plan" from Inbox would find Inbox/Plan.md: the full path is used instead.
        assert_eq!(f.link_path(plan, "Inbox/Idea.md", LinkFormat::Shortest, false), "Projects/Strata/Plan");
        for from in ["Inbox/Idea.md", "Projects/Idea.md", "x.md"] {
            for format in [LinkFormat::Shortest, LinkFormat::Relative, LinkFormat::Absolute] {
                for md in [false, true] {
                    let p = f.link_path(plan, from, format, md);
                    assert_eq!(f.resolve_unique(&p, Some(from)), Some(plan), "{p} from {from}");
                }
            }
        }
    }
}
