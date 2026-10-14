//! What a clicked link may do. Links come from documents Strata didn't write
//! (a Markdown file from anywhere), so only web and mail links leave the app;
//! a relative link to a local file opens it in Strata (text) or shows it in the
//! file manager (anything else); every other scheme (file:, smb:, vnc:,
//! custom app handlers...) is refused with the target in the status bar, never
//! handed to the operating system to launch.

use std::path::{Path, PathBuf};

/// Where a link goes, decided without touching the file system.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// `#slug`: a heading in the same document.
    Anchor(String),
    /// An http, https or mailto URL: the default browser / mail app opens it.
    Web(String),
    /// A local path (relative links resolved against the document's folder).
    Local(PathBuf),
    /// Not allowed; the message says why and shows the target.
    Refused(String),
}

/// Schemes handed to the operating system.
const OPEN_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The URL scheme of `url`, lowercased, if it has one ("C:\x" is a Windows
/// path, not a scheme, so one-letter schemes don't count).
pub fn scheme(url: &str) -> Option<String> {
    let (s, _) = url.split_once(':')?;
    let ok = s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    ok.then(|| s.to_ascii_lowercase())
}

/// Shorten a link for a status message.
fn shown(url: &str) -> String {
    let s: String = url.chars().filter(|c| !c.is_control()).take(200).collect();
    if url.chars().count() > 200 { format!("{s}…") } else { s }
}

/// Decide what clicking `url` does. `base_dir` is the folder of the document
/// the link is in (relative links resolve against it).
pub fn classify(url: &str, base_dir: Option<&Path>) -> Target {
    let url = url.trim();
    if let Some(anchor) = url.strip_prefix('#') {
        return Target::Anchor(anchor.to_owned());
    }
    if let Some(s) = scheme(url) {
        return if OPEN_SCHEMES.contains(&s.as_str()) && !url.chars().any(char::is_control) {
            Target::Web(url.to_owned())
        } else {
            Target::Refused(format!("Strata doesn't open {s}: links: {}", shown(url)))
        };
    }
    // "//host/x" and "\\host\share" name network shares; even checking whether
    // one exists would connect to that host (and on Windows send credentials).
    if url.starts_with("//") || url.starts_with("\\\\") || url.chars().any(char::is_control) {
        return Target::Refused(format!("Strata doesn't open network or malformed links: {}", shown(url)));
    }
    let path = url.split('#').next().unwrap_or(url).replace("%20", " ");
    if path.is_empty() {
        return Target::Refused(format!("Empty link: {}", shown(url)));
    }
    Target::Local(base_dir.map(Path::to_path_buf).unwrap_or_default().join(path))
}

/// Open an allowed web or mail link in the default browser / mail app. Returns
/// a status message when the link isn't one Strata opens.
pub fn open_web(ctx: &egui::Context, url: &str) -> Result<(), String> {
    match classify(url, None) {
        Target::Web(u) => {
            ctx.open_url(egui::OpenUrl::new_tab(u));
            Ok(())
        }
        Target::Refused(msg) => Err(msg),
        _ => Err(format!("Not a web link: {}", shown(url))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_and_mail_links_open() {
        for u in ["https://example.com/a?b#c", "http://x.org", "HTTPS://EXAMPLE.COM", "mailto:someone@example.com", "  https://padded.example  "] {
            assert!(matches!(classify(u, None), Target::Web(_)), "{u}");
        }
        assert_eq!(classify(" https://a.b ", None), Target::Web("https://a.b".into()));
    }

    #[test]
    fn other_schemes_are_refused_and_named() {
        for u in [
            "file:///Applications/Calculator.app",
            "smb://evil.example/share",
            "vnc://host",
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<script>",
            "x-apple.systempreferences:com.apple.preference.security",
            "ms-msdt:/id",
            "search-ms:query=x",
            "tel:123",
            "custom-app://do-something",
        ] {
            match classify(u, None) {
                Target::Refused(msg) => assert!(msg.contains(u.split(':').next().unwrap().to_ascii_lowercase().as_str()) && msg.contains(&u[..u.len().min(10)]), "{u}: {msg}"),
                t => panic!("{u} was allowed: {t:?}"),
            }
        }
        // Control characters can't sneak into an otherwise allowed URL.
        assert!(matches!(classify("https://a.b/\nfile:///x", None), Target::Refused(_)));
    }

    #[test]
    fn relative_links_resolve_against_the_document() {
        let base = Path::new("/docs/notes");
        assert_eq!(classify("other.md", Some(base)), Target::Local(base.join("other.md")));
        assert_eq!(classify("sub/my%20file.txt#part", Some(base)), Target::Local(base.join("sub/my file.txt")));
        assert_eq!(classify("#heading", Some(base)), Target::Anchor("heading".into()));
        // A Windows drive letter is a path, not a scheme.
        assert_eq!(scheme(r"C:\x\y.txt"), None);
        assert!(matches!(classify(r"C:\x\y.txt", None), Target::Local(_)));
    }

    #[test]
    fn network_paths_are_refused() {
        for u in ["//evil.example/share/x.txt", r"\\evil.example\share\x.txt"] {
            assert!(matches!(classify(u, Some(Path::new("/d"))), Target::Refused(_)), "{u}");
        }
    }

    #[test]
    fn open_web_only_sends_allowed_links() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            assert!(open_web(ui.ctx(), "https://example.com").is_ok());
            assert!(open_web(ui.ctx(), "file:///etc/passwd").is_err());
            assert!(open_web(ui.ctx(), "notes.md").is_err());
        });
        out.textures_delta.clear();
        let opened: Vec<&str> = out.platform_output.commands.iter().filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(u) => Some(u.url.as_str()),
            _ => None,
        }).collect();
        assert_eq!(opened, ["https://example.com"]);
    }
}
