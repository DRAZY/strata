//! Which language server handles which files: built-in entries for common
//! servers (used only when their program is installed) merged with the
//! `lsp_servers` setting. Strata never downloads or installs a server.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// One entry of the `lsp_servers` setting. Fields left out keep the
/// built-in entry's value (for the built-in names), so `{"rust":
/// {"args": ["--log-file", "/tmp/ra.log"]}}` changes only the arguments.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ServerSetting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// File names the server handles, as globs (`*.rs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_patterns: Option<Vec<String>>,
    /// Files that mark a project's top folder (`Cargo.toml`), tried in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_markers: Option<Vec<String>>,
    /// Sent with `initialize`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<Value>,
    /// Answers to the server's `workspace/configuration` requests, by section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    /// The protocol's language id; by default it follows the file extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    /// Environment variables for the server's process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// A server entry with every field decided.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfig {
    /// The entry's name in `lsp_servers` (`rust`).
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub file_patterns: Vec<String>,
    pub root_markers: Vec<String>,
    pub initialization_options: Option<Value>,
    pub settings: Option<Value>,
    pub language_id: Option<String>,
    pub env: BTreeMap<String, String>,
    /// From Strata (used only if installed) rather than the user's settings.
    pub builtin: bool,
}

impl ServerConfig {
    /// Does this server handle the file?
    pub fn handles(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else { return false };
        self.file_patterns.iter().any(|p| glob_match(p, &name))
    }

    /// The command's short name, for the status bar.
    pub fn display_name(&self) -> String {
        Path::new(&self.command).file_stem().map_or_else(|| self.command.clone(), |s| s.to_string_lossy().into_owned())
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The servers Strata knows how to start.
pub fn builtins() -> Vec<ServerConfig> {
    let entry = |name: &str, command: &str, args: &[&str], patterns: &[&str], markers: &[&str]| ServerConfig {
        name: name.into(),
        command: command.into(),
        args: strings(args),
        file_patterns: strings(patterns),
        root_markers: strings(markers),
        initialization_options: None,
        settings: None,
        language_id: None,
        env: BTreeMap::new(),
        builtin: true,
    };
    vec![
        // Cargo.lock sits at a workspace's top, where rust-analyzer wants to start.
        entry("rust", "rust-analyzer", &[], &["*.rs"], &["Cargo.lock", "Cargo.toml", "rust-project.json"]),
        entry(
            "typescript",
            "typescript-language-server",
            &["--stdio"],
            &["*.ts", "*.tsx", "*.mts", "*.cts", "*.js", "*.jsx", "*.mjs", "*.cjs"],
            &["tsconfig.json", "jsconfig.json", "package.json"],
        ),
        entry("python", "pyright-langserver", &["--stdio"], &["*.py", "*.pyi"], &["pyrightconfig.json", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt"]),
        entry("go", "gopls", &[], &["*.go"], &["go.work", "go.mod"]),
        entry(
            "c",
            "clangd",
            &[],
            &["*.c", "*.h", "*.cc", "*.cpp", "*.cxx", "*.c++", "*.hh", "*.hpp", "*.hxx", "*.h++", "*.m", "*.mm"],
            &[".clangd", "compile_commands.json", "compile_flags.txt"],
        ),
    ]
}

/// Built-ins with the user's entries applied: an entry named like a
/// built-in changes it, other names add servers (tried before the
/// built-ins, so a user's server can take over a file type). Entries
/// with `"enabled": false` are dropped. Also returns problems to report.
pub fn resolve(user: &BTreeMap<String, ServerSetting>) -> (Vec<ServerConfig>, Vec<String>) {
    let mut problems = Vec::new();
    let mut builtins = builtins();
    let mut added = Vec::new();
    for (name, s) in user {
        let base = builtins.iter().position(|b| b.name == *name);
        let mut c = match base {
            Some(i) => builtins[i].clone(),
            None => {
                let (Some(command), Some(patterns)) = (&s.command, &s.file_patterns) else {
                    if s.enabled != Some(false) {
                        problems.push(format!("lsp_servers: \"{name}\" needs \"command\" and \"file_patterns\""));
                    }
                    continue;
                };
                ServerConfig {
                    name: name.clone(),
                    command: command.clone(),
                    args: Vec::new(),
                    file_patterns: patterns.clone(),
                    root_markers: Vec::new(),
                    initialization_options: None,
                    settings: None,
                    language_id: None,
                    env: BTreeMap::new(),
                    builtin: false,
                }
            }
        };
        if let Some(v) = &s.command {
            c.command.clone_from(v);
            // A command the user gave is expected to exist: say so if it doesn't.
            c.builtin = false;
        }
        if let Some(v) = &s.args {
            c.args.clone_from(v);
        }
        if let Some(v) = &s.file_patterns {
            c.file_patterns.clone_from(v);
        }
        if let Some(v) = &s.root_markers {
            c.root_markers.clone_from(v);
        }
        if s.initialization_options.is_some() {
            c.initialization_options.clone_from(&s.initialization_options);
        }
        if s.settings.is_some() {
            c.settings.clone_from(&s.settings);
        }
        if s.language_id.is_some() {
            c.language_id.clone_from(&s.language_id);
        }
        if let Some(v) = &s.env {
            c.env.clone_from(v);
        }
        let enabled = s.enabled != Some(false);
        match base {
            Some(i) if enabled => builtins[i] = c,
            Some(i) => {
                builtins.remove(i);
            }
            None if enabled => added.push(c),
            None => {}
        }
    }
    added.extend(builtins);
    (added, problems)
}

/// The project folder for `file`: the nearest folder above it holding the
/// first of `markers` found anywhere above it (so `Cargo.lock`, at a
/// workspace's top, wins over a member's `Cargo.toml`), else the open
/// folder (of `roots`) that holds it, else the file's own folder.
pub fn find_root(file: &Path, markers: &[String], roots: &[PathBuf]) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("/"));
    for m in markers {
        if let Some(found) = dir.ancestors().find(|d| d.join(m).exists()) {
            return found.to_owned();
        }
    }
    roots.iter().filter(|r| file.starts_with(r)).max_by_key(|r| r.components().count()).cloned().unwrap_or_else(|| dir.to_owned())
}

/// The protocol's language id for a file (`rust`, `typescriptreact`…).
pub fn language_id(path: &Path, fallback: &str) -> String {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let id = match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        _ => fallback,
    };
    id.to_owned()
}

/// The program `command` runs: itself if it's a path, else the first
/// match in the `path` list (with Windows' executable extensions).
pub fn find_executable(command: &str, path: &OsStr) -> Option<PathBuf> {
    let p = Path::new(command);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_owned());
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new()).chain(pathext.split(';').filter(|e| !e.is_empty()).map(str::to_owned)).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(path) {
        for ext in &exts {
            let candidate = dir.join(format!("{command}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// `*` and `?` globs over a file name (`*.rs`, `Makefile`, `*.d.ts`);
/// letters compare case-insensitively on Windows and macOS, whose file
/// systems usually do.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let fold = |s: &str| if cfg!(any(windows, target_os = "macos")) { s.to_lowercase() } else { s.to_owned() };
    let (p, n): (Vec<char>, Vec<char>) = (fold(pattern).chars().collect(), fold(name).chars().collect());
    // Iterative matching with backtracking to the last `*`.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.rsx"));
        assert!(glob_match("*.d.ts", "types.d.ts"));
        assert!(glob_match("Makefile", "Makefile"));
        assert!(glob_match("a?c*", "abcdef"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn user_entries_change_add_and_disable_servers() {
        let user: BTreeMap<String, ServerSetting> = serde_json::from_value(json!({
            "rust": {"args": ["--verbose"], "settings": {"rust-analyzer": {"check": {"command": "clippy"}}}},
            "go": {"enabled": false},
            "zig": {"command": "zls", "file_patterns": ["*.zig"], "root_markers": ["build.zig"]},
            "broken": {"command": "x"},
        }))
        .unwrap();
        let (servers, problems) = resolve(&user);
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["zig", "rust", "typescript", "python", "c"]);
        let rust = &servers[1];
        assert_eq!((rust.command.as_str(), rust.args.clone()), ("rust-analyzer", vec!["--verbose".to_owned()]));
        assert!(rust.builtin, "the command is still the built-in one");
        assert_eq!(rust.settings, Some(json!({"rust-analyzer": {"check": {"command": "clippy"}}})));
        assert!(!servers[0].builtin);
        assert!(servers[0].handles(Path::new("/x/build.zig")));
        assert_eq!(problems, ["lsp_servers: \"broken\" needs \"command\" and \"file_patterns\""]);
        // Nothing set: the built-ins as they are.
        assert_eq!(resolve(&BTreeMap::new()).0, builtins());
    }

    #[test]
    fn roots_prefer_the_first_marker_anywhere_above() {
        let dir = std::env::temp_dir().join(format!("strata-lsp-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let member = dir.join("crates/member/src");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(dir.join("Cargo.lock"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join("crates/member/Cargo.toml"), "").unwrap();
        let file = member.join("lib.rs");
        let markers = strings(&["Cargo.lock", "Cargo.toml"]);
        assert_eq!(find_root(&file, &markers, &[]), dir);
        assert_eq!(find_root(&file, &strings(&["Cargo.toml"]), &[]), dir.join("crates/member"));
        // No marker: the open folder holding it, else the file's folder.
        assert_eq!(find_root(&file, &strings(&["nothing.here"]), std::slice::from_ref(&dir)), dir);
        assert_eq!(find_root(&file, &[], &[]), member);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn executables_are_found_on_the_path() {
        let dir = std::env::temp_dir().join(format!("strata-lsp-exe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "fake-ls.exe" } else { "fake-ls" };
        let exe = dir.join(name);
        std::fs::write(&exe, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(find_executable("fake-ls", dir.as_os_str()), None, "not executable yet");
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([PathBuf::from("/nowhere"), dir.clone()]).unwrap();
        assert_eq!(find_executable("fake-ls", &path), Some(exe.clone()));
        assert_eq!(find_executable("missing-ls", &path), None);
        assert_eq!(find_executable(&exe.to_string_lossy(), OsStr::new("")), Some(exe));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn language_ids_follow_the_extension() {
        assert_eq!(language_id(Path::new("a.tsx"), "x"), "typescriptreact");
        assert_eq!(language_id(Path::new("a.HPP"), "x"), "cpp");
        assert_eq!(language_id(Path::new("a.zig"), "zig"), "zig");
    }
}
