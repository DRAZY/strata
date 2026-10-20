//! `strata-plugin.json`: what a plugin package says about itself, read and
//! checked before anything of it runs. Every problem is reported in plain
//! words with the field it's in, so a plugin author can fix it from the
//! message alone.

use super::Perm;
use egui::{Key, Modifiers};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The manifest's file name, at the top of the package.
pub const FILE: &str = "strata-plugin.json";
/// Bigger than this isn't a manifest (they're a few hundred bytes).
pub const MAX_BYTES: usize = 64 << 10;
const MAX_COMMANDS: usize = 100;
const MAX_SETTINGS: usize = 100;
const MAX_ARGS: usize = 64;

/// Words title case leaves in lower case (unless they come first).
const MINOR_WORDS: &[&str] = &["a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "into", "of", "on", "or", "the", "to", "with"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: String,
    #[serde(default)]
    title: Option<String>,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    license: String,
    api: String,
    run: RawRun,
    #[serde(default)]
    activation: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    contributes: RawContributes,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    macos: Option<RawCmd>,
    windows: Option<RawCmd>,
    linux: Option<RawCmd>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCmd {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawContributes {
    #[serde(default)]
    commands: Vec<RawCommand>,
    #[serde(default)]
    keybindings: Vec<RawKey>,
    #[serde(default)]
    settings: BTreeMap<String, RawSetting>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    id: String,
    title: String,
    #[serde(default = "yes")]
    menu: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    command: String,
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSetting {
    default: Value,
    #[serde(default)]
    description: String,
}

/// A checked manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Lower case, digits and hyphens; also the installed folder's name.
    pub name: String,
    /// Shown in menus and the palette ("Word Count").
    pub title: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub license: String,
    /// What to run on this platform.
    pub run: Run,
    pub activation: Activation,
    pub permissions: BTreeSet<Perm>,
    pub commands: Vec<Command>,
    pub keys: Vec<KeyBinding>,
    pub settings: BTreeMap<String, Setting>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// A program found on PATH (`python3`), or a path inside the package
    /// (`bin/plugin`), never absolute and never leaving it.
    pub command: String,
    pub args: Vec<String>,
}

impl Run {
    /// The command names a file in the package rather than a program on PATH.
    pub fn in_package(&self) -> bool {
        self.command.contains(['/', '\\'])
    }
}

/// When the plugin starts. Running one of its commands always starts it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activation {
    pub startup: bool,
    /// Globs: a matching file opened, or saved.
    pub open: Vec<String>,
    pub save: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub id: String,
    pub title: String,
    /// Listed under Tools ▸ Plugins ▸ <title>.
    pub menu: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyBinding {
    pub command: String,
    /// As written, for messages.
    pub text: String,
    pub mods: Modifiers,
    pub key: Key,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub default: Value,
    pub description: String,
}

/// Read and check a manifest. Errors name the field and say what's wrong.
pub fn parse(text: &str) -> Result<Manifest, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("{FILE} is larger than {} KB", MAX_BYTES >> 10));
    }
    let raw: Raw = serde_json::from_str(text).map_err(|e| format!("{FILE}: {e}"))?;
    check(raw).map_err(|e| format!("{FILE}: {e}"))
}

fn check(raw: Raw) -> Result<Manifest, String> {
    let name = raw.name;
    if !valid_name(&name) {
        return Err(format!("\"name\" must be 1 to 64 lower-case letters, digits and hyphens, starting with a letter or digit (not \"{}\")", clip(&name)));
    }
    let title = match raw.title {
        Some(t) => {
            check_text("title", &t, 40)?;
            if t.contains([':', '▸']) {
                return Err(format!("\"title\" can't contain ':' or '▸' (\"{t}\")"));
            }
            if let Some(p) = title_case_problem(&t) {
                return Err(format!("\"title\": {p}"));
            }
            t
        }
        None => title_from_name(&name),
    };
    let version = raw.version;
    if version.is_empty() || version.len() > 32 || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
        return Err(format!("\"version\" must be 1 to 32 letters, digits, dots, hyphens or plus signs, like \"1.0.0\" (not \"{}\")", clip(&version)));
    }
    check_text("description", &raw.description, 500)?;
    check_text("author", &raw.author, 200)?;
    check_text("license", &raw.license, 100)?;
    if raw.api != super::API {
        return Err(format!("\"api\" is \"{}\", but this Strata speaks \"{}\"", clip(&raw.api), super::API));
    }
    let run = check_run(raw.run)?;

    let mut activation = Activation::default();
    for a in &raw.activation {
        if a == "startup" {
            activation.startup = true;
        } else if let Some(g) = a.strip_prefix("open:") {
            activation.open.push(check_glob(a, g)?);
        } else if let Some(g) = a.strip_prefix("save:") {
            activation.save.push(check_glob(a, g)?);
        } else {
            return Err(format!("\"activation\" has \"{}\": use \"startup\", \"open:<glob>\" or \"save:<glob>\" (running a command always starts the plugin)", clip(a)));
        }
    }

    let mut permissions = BTreeSet::new();
    for p in &raw.permissions {
        let perm = Perm::from_id(p).ok_or_else(|| format!("\"permissions\" has \"{}\", which isn't one of: {}", clip(p), Perm::ALL.iter().map(|p| p.id()).collect::<Vec<_>>().join(", ")))?;
        if !permissions.insert(perm) {
            return Err(format!("\"permissions\" lists \"{p}\" twice"));
        }
    }

    let c = raw.contributes;
    if c.commands.len() > MAX_COMMANDS {
        return Err(format!("\"contributes.commands\" has more than {MAX_COMMANDS} commands"));
    }
    let mut commands: Vec<Command> = Vec::new();
    for cmd in c.commands {
        if !valid_id(&cmd.id) {
            return Err(format!("command id \"{}\" must be 1 to 64 lower-case letters, digits, dots, hyphens or underscores", clip(&cmd.id)));
        }
        check_text("command title", &cmd.title, 80)?;
        if cmd.title.trim().is_empty() {
            return Err(format!("command \"{}\" needs a \"title\"", cmd.id));
        }
        if let Some(p) = title_case_problem(&cmd.title) {
            return Err(format!("command \"{}\": {p}", cmd.id));
        }
        if commands.iter().any(|o| o.id == cmd.id) {
            return Err(format!("command id \"{}\" is used twice", cmd.id));
        }
        if commands.iter().any(|o| o.title == cmd.title) {
            return Err(format!("command title \"{}\" is used twice", cmd.title));
        }
        commands.push(Command { id: cmd.id, title: cmd.title, menu: cmd.menu });
    }
    if commands.is_empty() && !activation.startup && activation.open.is_empty() && activation.save.is_empty() {
        return Err("the plugin would never start: it has no commands and no \"activation\" events".into());
    }

    let mut keys: Vec<KeyBinding> = Vec::new();
    for k in c.keybindings {
        if !commands.iter().any(|c| c.id == k.command) {
            return Err(format!("key binding \"{}\" is for command \"{}\", which the plugin doesn't contribute", clip(&k.key), clip(&k.command)));
        }
        let stroke = crate::keybindings::parse_stroke(&k.key).map_err(|e| format!("key binding for \"{}\": {e}", k.command))?;
        if let Some(why) = key_problem(stroke.mods, stroke.key) {
            return Err(format!("key binding \"{}\" for \"{}\": {why}", k.key, k.command));
        }
        if keys.iter().any(|o| o.key == stroke.key && crate::keys::same_mods(o.mods, stroke.mods)) {
            return Err(format!("key \"{}\" is bound twice", k.key));
        }
        keys.push(KeyBinding { command: k.command, text: k.key, mods: stroke.mods, key: stroke.key });
    }

    if c.settings.len() > MAX_SETTINGS {
        return Err(format!("\"contributes.settings\" has more than {MAX_SETTINGS} settings"));
    }
    let mut settings = BTreeMap::new();
    for (key, s) in c.settings {
        if !valid_id(&key) {
            return Err(format!("setting \"{}\" must be named with 1 to 64 lower-case letters, digits, dots, hyphens or underscores", clip(&key)));
        }
        check_text("setting description", &s.description, 500)?;
        settings.insert(key, Setting { default: s.default, description: s.description });
    }

    Ok(Manifest { name, title, version, description: raw.description, author: raw.author, license: raw.license, run, activation, permissions, commands, keys, settings })
}

fn check_run(raw: RawRun) -> Result<Run, String> {
    let (platform, specific) = if cfg!(target_os = "macos") {
        ("macOS", raw.macos)
    } else if cfg!(windows) {
        ("Windows", raw.windows)
    } else {
        ("Linux", raw.linux)
    };
    let run = match (specific, raw.command) {
        (Some(c), _) => Run { command: c.command, args: c.args },
        (None, Some(command)) => Run { command, args: raw.args },
        (None, None) => return Err(format!("\"run\" has no command for {platform}")),
    };
    if run.command.trim().is_empty() || run.command.len() > 255 || run.command.contains('\0') {
        return Err("\"run.command\" must be a program name or a path inside the plugin's folder".into());
    }
    if run.in_package() && super::package::command_parts(&run.command).is_none() {
        return Err(format!("\"run.command\" \"{}\" must be a path inside the plugin's folder (not absolute, no \"..\")", clip(&run.command)));
    }
    if run.args.len() > MAX_ARGS || run.args.iter().any(|a| a.len() > 4096 || a.contains('\0')) {
        return Err(format!("\"run.args\" must be at most {MAX_ARGS} arguments of at most 4096 characters"));
    }
    Ok(run)
}

fn check_glob(whole: &str, glob: &str) -> Result<String, String> {
    if glob.is_empty() || glob.len() > 200 {
        return Err(format!("\"activation\" entry \"{}\" needs a glob after the colon, like \"open:*.md\"", clip(whole)));
    }
    globset::Glob::new(glob).map_err(|e| format!("\"activation\" entry \"{}\": {e}", clip(whole)))?;
    Ok(glob.to_owned())
}

fn check_text(field: &str, text: &str, max: usize) -> Result<(), String> {
    if text.chars().count() > max {
        return Err(format!("\"{field}\" is longer than {max} characters"));
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!("\"{field}\" contains control characters"));
    }
    Ok(())
}

/// Lower-case letters, digits and hyphens, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !name.starts_with('-')
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')) && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// "word-count" → "Word Count".
fn title_from_name(name: &str) -> String {
    name.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Palette names and menu items use title case, like Strata's own.
fn title_case_problem(title: &str) -> Option<String> {
    for (i, w) in title.split_whitespace().enumerate() {
        let lower_start = w.chars().next().is_some_and(char::is_lowercase);
        // "macOS", "iPhone": a capital later in the word is the word's own spelling.
        let own_spelling = w.chars().skip(1).any(char::is_uppercase);
        if lower_start && !own_spelling && (i == 0 || !MINOR_WORDS.contains(&w)) {
            return Some(format!("\"{title}\" should be in title case, like Strata's own commands (\"{w}\" starts with a small letter)"));
        }
    }
    None
}

/// Why a plugin may not bind this key, if it may not. Strata's own keys
/// are checked when the plugin loads (they can change); this rules out
/// keys that type text or move around in the editor.
fn key_problem(mods: Modifiers, key: Key) -> Option<&'static str> {
    let (_, _, primary, _) = crate::keys::parts(mods);
    let function = matches!(
        key,
        Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6 | Key::F7 | Key::F8 | Key::F9 | Key::F10 | Key::F11 | Key::F12 | Key::F13 | Key::F14 | Key::F15 | Key::F16 | Key::F17 | Key::F18 | Key::F19 | Key::F20
    );
    let letter_or_digit = key.name().len() == 1 && key.name().chars().all(|c| c.is_ascii_alphanumeric());
    if function {
        return None;
    }
    if !letter_or_digit {
        return Some("plugins may bind letters, digits and function keys only");
    }
    if !primary {
        return Some(if crate::keys::MAC { "a letter or digit needs ⌘ (\"primary\")" } else { "a letter or digit needs Ctrl (\"primary\")" });
    }
    None
}

fn clip(s: &str) -> String {
    let mut out: String = s.chars().take(60).filter(|c| !c.is_control()).collect();
    if s.chars().count() > 60 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({
            "name": "word-count",
            "version": "1.0.0",
            "description": "Counts words",
            "api": "strata.plugin/1",
            "run": {"command": "python3", "args": ["word_count.py"]},
            "activation": ["open:*.md"],
            "permissions": ["read_document", "notify"],
            "contributes": {
                "commands": [{"id": "count", "title": "Count Words in the File"}],
                "keybindings": [{"command": "count", "key": "primary+alt+w"}],
                "settings": {"include_numbers": {"default": true, "description": "Count numbers as words"}}
            }
        })
    }

    fn with(edit: impl FnOnce(&mut Value)) -> Result<Manifest, String> {
        let mut v = base();
        edit(&mut v);
        parse(&v.to_string())
    }

    fn err(edit: impl FnOnce(&mut Value)) -> String {
        with(edit).expect_err("should be refused")
    }

    #[test]
    fn a_good_manifest() {
        let m = with(|_| {}).unwrap();
        assert_eq!((m.name.as_str(), m.title.as_str(), m.version.as_str()), ("word-count", "Word Count", "1.0.0"));
        assert_eq!(m.run, Run { command: "python3".into(), args: vec!["word_count.py".into()] });
        assert!(!m.run.in_package());
        assert_eq!(m.activation.open, ["*.md"]);
        assert_eq!(m.permissions, BTreeSet::from([Perm::ReadDocument, Perm::Notify]));
        assert_eq!(m.commands[0].title, "Count Words in the File");
        assert!(m.commands[0].menu);
        assert_eq!(m.keys[0].key, Key::W);
        assert_eq!(m.settings["include_numbers"].default, json!(true));
    }

    #[test]
    fn platform_commands_win_over_the_general_one() {
        let m = with(|v| v["run"] = json!({"command": "sh", "args": ["a"], "macos": {"command": "./mac.sh"}, "windows": {"command": "win.cmd"}, "linux": {"command": "bin/linux"}})).unwrap();
        let want = if cfg!(target_os = "macos") {
            "./mac.sh"
        } else if cfg!(windows) {
            "win.cmd"
        } else {
            "bin/linux"
        };
        assert_eq!(m.run.command, want);
        assert!(m.run.args.is_empty());
        let e = err(|v| v["run"] = json!({"other_os_only": {"command": "x"}}));
        assert!(e.contains("unknown field"), "{e}");
    }

    #[test]
    fn clear_errors_for_each_field() {
        type Edit = Box<dyn FnOnce(&mut Value)>;
        let cases: Vec<(Edit, &str)> = vec![
            (Box::new(|v| v["name"] = json!("Word Count")), "\"name\" must be"),
            (Box::new(|v| v["name"] = json!("-x")), "\"name\" must be"),
            (Box::new(|v| v["name"] = json!("a/../b")), "\"name\" must be"),
            (Box::new(|v| v["version"] = json!("")), "\"version\""),
            (Box::new(|v| v["api"] = json!("strata.plugin/2")), "this Strata speaks \"strata.plugin/1\""),
            (Box::new(|v| v["title"] = json!("word counter")), "title case"),
            (Box::new(|v| v["title"] = json!("Words: Counted")), "can't contain ':'"),
            (Box::new(|v| v["run"] = json!({})), "no command for"),
            (Box::new(|v| v["run"] = json!({"command": "/usr/bin/python3"})), "inside the plugin's folder"),
            (Box::new(|v| v["run"] = json!({"command": "../evil/run.sh"})), "inside the plugin's folder"),
            (Box::new(|v| v["run"] = json!({"command": "C:\\evil.exe"})), "inside the plugin's folder"),
            (Box::new(|v| v["run"] = json!({"command": "  "})), "\"run.command\""),
            (Box::new(|v| v["activation"] = json!(["always"])), "use \"startup\""),
            (Box::new(|v| v["activation"] = json!(["open:"])), "needs a glob"),
            (Box::new(|v| v["activation"] = json!(["open:[x"])), "\"activation\" entry"),
            (Box::new(|v| v["permissions"] = json!(["root"])), "isn't one of: read_document"),
            (Box::new(|v| v["permissions"] = json!(["notify", "notify"])), "twice"),
            (Box::new(|v| v["contributes"]["commands"][0]["title"] = json!("count words")), "title case"),
            (Box::new(|v| v["contributes"]["commands"][0]["id"] = json!("Count")), "command id"),
            (Box::new(|v| v["contributes"]["commands"] = json!([{"id": "a", "title": "A"}, {"id": "a", "title": "B"}])), "used twice"),
            (Box::new(|v| v["contributes"]["keybindings"][0]["command"] = json!("other")), "doesn't contribute"),
            (Box::new(|v| v["contributes"]["keybindings"][0]["key"] = json!("w")), "needs"),
            (Box::new(|v| v["contributes"]["keybindings"][0]["key"] = json!("primary+left")), "letters, digits and function keys"),
            (Box::new(|v| v["contributes"]["keybindings"][0]["key"] = json!("primary+zz")), "unknown key"),
            (Box::new(|v| v["contributes"]["settings"] = json!({"Bad Name": {"default": 1}})), "setting \"Bad Name\""),
            (Box::new(|v| v["contributes"]["settings"] = json!({"x": {}})), "missing field `default`"),
            (Box::new(|v| v["extra"] = json!(1)), "unknown field `extra`"),
            (Box::new(|v| v["description"] = json!("bell\u{7}")), "control characters"),
        ];
        for (edit, want) in cases {
            let e = err(edit);
            assert!(e.starts_with("strata-plugin.json: "), "{e}");
            assert!(e.contains(want), "{e:?} should mention {want:?}");
        }
    }

    #[test]
    fn a_plugin_that_can_never_start_is_refused() {
        let e = err(|v| {
            v["activation"] = json!([]);
            v["contributes"] = json!({});
        });
        assert!(e.contains("never start"), "{e}");
        // Startup alone is enough.
        assert!(with(|v| {
            v["activation"] = json!(["startup"]);
            v["contributes"] = json!({});
        })
        .is_ok());
    }

    #[test]
    fn titles_allow_small_words_and_own_spellings() {
        assert!(title_case_problem("Sort Lines Uniquely").is_none());
        assert!(title_case_problem("Open in a New Tab").is_none());
        assert!(title_case_problem("Copy for macOS").is_none());
        assert!(title_case_problem("a Thing").is_some(), "the first word is capitalized");
        assert!(title_case_problem("Sort lines").is_some());
        assert_eq!(title_from_name("sort-unique-2"), "Sort Unique 2");
    }

    #[test]
    fn function_keys_need_no_modifier() {
        assert!(with(|v| v["contributes"]["keybindings"][0]["key"] = json!("f19")).is_ok());
        assert!(with(|v| v["contributes"]["keybindings"][0]["key"] = json!("primary+shift+9")).is_ok());
    }

    #[test]
    fn huge_or_broken_files() {
        assert!(parse(&" ".repeat(MAX_BYTES + 1)).unwrap_err().contains("larger than"));
        let e = parse("{\"name\": ").unwrap_err();
        assert!(e.starts_with("strata-plugin.json: ") && e.contains("line 1"), "{e}");
    }
}
