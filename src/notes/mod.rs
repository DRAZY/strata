//! Notes: Obsidian-compatible notes on plain Markdown files. A folder with
//! Markdown in it works as a vault; one with `.obsidian/` is an Obsidian
//! vault, whose link and new-note settings are followed (read only). There
//! is no format of Strata's own: notes stay plain files that Obsidian, sync
//! tools and Git see unchanged.
//!
//! This module is the part without a window: reading notes (`parse`),
//! resolving links (`resolve`), the background index (`index`), Obsidian's
//! settings (`config`), dates (`dates`), link updates on rename (`rename`)
//! and sync conflict copies (`conflicts`). The window glue is
//! `app/notes_ui.rs`.

pub mod config;
pub mod conflicts;
pub mod dates;
pub mod index;
pub mod marks;
pub mod parse;
pub mod rename;
pub mod resolve;

/// Fill a template's `{{title}}`, `{{date}}`, `{{time}}` and
/// `{{date:FORMAT}}` / `{{time:FORMAT}}` (moment.js formats), as Obsidian's
/// Templates and Daily notes plugins do. Other `{{…}}` are left as written.
pub fn fill_template(template: &str, title: &str, now: &dates::DateTime, date_format: &str, time_format: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = after[..close].trim();
        let (name, format) = match inner.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner, None),
        };
        match name.to_ascii_lowercase().as_str() {
            "title" if format.is_none() => out.push_str(title),
            "date" => out.push_str(&dates::format(format.filter(|f| !f.is_empty()).unwrap_or(date_format), now)),
            "time" => out.push_str(&dates::format(format.filter(|f| !f.is_empty()).unwrap_or(time_format), now)),
            _ => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_title_date_and_time() {
        let now = dates::DateTime { hour: 14, minute: 5, ..dates::DateTime::date(2026, 10, 19) };
        let t = "# {{title}}\nCreated {{date}} at {{time}}\n{{date:dddd, MMMM Do}} {{ time : h:mm A }} {{unknown}} {{Title}} {{not closed";
        assert_eq!(fill_template(t, "My Note", &now, "YYYY-MM-DD", "HH:mm"), "# My Note\nCreated 2026-10-19 at 14:05\nMonday, October 19th 2:05 PM {{unknown}} My Note {{not closed");
    }
}
