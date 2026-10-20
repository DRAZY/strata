//! What the editor marks in a note: links (found or not) and tags, per line
//! as char ranges. Each line's marks are kept with a hash of its text, so a
//! line edited since they were made shows none rather than misplaced ones.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A link to a file that exists.
    Link,
    /// A link to a file that doesn't (yet).
    Broken,
    Tag,
}

/// One line's marks, with the hash of the text they were made for.
type LineMarks = (u64, Vec<(Range<usize>, Mark)>);

#[derive(Default, Debug)]
pub struct NoteMarks {
    lines: HashMap<usize, LineMarks>,
}

pub fn hash_line(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl NoteMarks {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Marks for `line`, whose text is `text`, as char ranges in the line.
    pub fn insert(&mut self, line: usize, text: &str, marks: Vec<(Range<usize>, Mark)>) {
        self.lines.insert(line, (hash_line(text), marks));
    }

    /// The marks on `line` if its text is still `text`.
    pub fn for_line(&self, line: usize, text: &str) -> &[(Range<usize>, Mark)] {
        match self.lines.get(&line) {
            Some((h, m)) if *h == hash_line(text) => m,
            _ => &[],
        }
    }

    /// The link (not tag) covering char `col` of `line`.
    pub fn link_at(&self, line: usize, text: &str, col: usize) -> Option<(Range<usize>, Mark)> {
        self.for_line(line, text).iter().find(|(r, m)| *m != Mark::Tag && r.start <= col && col < r.end).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_belong_to_the_text_they_were_made_for() {
        let mut m = NoteMarks::default();
        m.insert(3, "see [[a]] #t", vec![(4..9, Mark::Link), (10..12, Mark::Tag)]);
        assert_eq!(m.for_line(3, "see [[a]] #t").len(), 2);
        assert!(m.for_line(3, "see [[ab]] #t").is_empty(), "edited since");
        assert!(m.for_line(4, "see [[a]] #t").is_empty());
        assert_eq!(m.link_at(3, "see [[a]] #t", 4), Some((4..9, Mark::Link)));
        assert_eq!(m.link_at(3, "see [[a]] #t", 9), None);
        assert_eq!(m.link_at(3, "see [[a]] #t", 11), None, "tags aren't links");
    }
}
