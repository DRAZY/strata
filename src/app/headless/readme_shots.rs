//! The README's screenshots: polished scenes over small sample projects
//! written to a temporary folder (none of Strata's own files, no personal
//! paths in view).
//!
//! `STRATA_README_SHOTS=<folder> cargo test --release readme_shots` writes
//! `<name>.ppm` at 3200×2000 (1600×1000 points at 2×); convert and shrink
//! them for docs/screenshots/ as CONTRIBUTING.md describes.

use super::Shooter;
use crate::app::StrataApp;
use crate::commands::Cmd;
use egui::{Context, Event};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const W: u32 = 1600;
const H: u32 = 1000;

/// A small Rust command-line project, committed to git and then edited, so
/// the gutter has change bars and the status bar a branch.
const TALLY: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        r#"[package]
name = "tally"
version = "0.4.0"
edition = "2024"
description = "Sum up time logs by project and week"
license = "MIT"

[dependencies]
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["std"] }
clap = { version = "4", features = ["derive"] }
"#,
    ),
    (".gitignore", "/target\n"),
    (
        "README.md",
        r#"# tally

Sum up plain-text time logs by project and week.

> [!TIP]
> Keep one log per week and `tally` reads them all with a glob.

## Install

```sh
cargo install --path .
```

## Log format

Each line is a date, a duration and a project, with an optional note:

```text
2026-10-12  1h30  strata   README screenshots
2026-10-12  45m   inbox
2026-10-13  2h    strata   release workflow
```

## Usage

| Command | What it does |
|---|---|
| `tally week.log` | Totals per project |
| `tally --by-day *.log` | One row per day |
| `tally --since 2026-10-01` | Skip older entries |

## Roadmap

- [x] Durations like `1h30`, `45m` and `2h`
- [x] Totals per project and per day
- [ ] CSV export
- [ ] Budgets with a warning when a project runs over

Licensed under the MIT license.
"#,
    ),
    (
        "src/main.rs",
        r#"mod entry;
mod parse;
mod report;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

/// Sum up time logs by project and week.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Log files to read.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// One row per day instead of per project.
    #[arg(long)]
    by_day: bool,
    /// Skip entries before this date (YYYY-MM-DD).
    #[arg(long)]
    since: Option<chrono::NaiveDate>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut entries = Vec::new();
    for path in &args.files {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        entries.extend(parse::parse_log(&text)?);
    }
    if let Some(since) = args.since {
        entries.retain(|e| e.date >= since);
    }
    let table = if args.by_day { report::by_day(&entries) } else { report::by_project(&entries) };
    print!("{table}");
    Ok(())
}
"#,
    ),
    (
        "src/entry.rs",
        r#"use chrono::NaiveDate;

/// One line of a time log.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub date: NaiveDate,
    pub minutes: u32,
    pub project: String,
    pub note: Option<String>,
}
"#,
    ),
    (
        "src/parse.rs",
        r#"use crate::entry::Entry;
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;

/// Parse a whole log: one entry per line; blank lines and `#` comments are skipped.
pub fn parse_log(text: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = parse_line(line).with_context(|| format!("line {}", n + 1))?;
        entries.push(entry);
    }
    Ok(entries)
}

/// `2026-10-12  1h30  strata  README screenshots`
pub fn parse_line(line: &str) -> Result<Entry> {
    let mut fields = line.split_whitespace();
    let (Some(date), Some(time), Some(project)) = (fields.next(), fields.next(), fields.next()) else {
        bail!("expected a date, a duration and a project");
    };
    let note: Vec<&str> = fields.collect();
    Ok(Entry {
        date: NaiveDate::parse_from_str(date, "%Y-%m-%d")?,
        minutes: parse_duration(time)?,
        project: project.to_owned(),
        note: (!note.is_empty()).then(|| note.join(" ")),
    })
}

/// `1h30`, `45m`, `2h` or `90` (minutes).
pub fn parse_duration(text: &str) -> Result<u32> {
    let (hours, rest) = match text.split_once('h') {
        Some((h, m)) => (h.parse::<u32>()?, m),
        None => (0, text),
    };
    let minutes = match rest.trim_end_matches('m') {
        "" => 0,
        m => m.parse::<u32>()?,
    };
    if minutes >= 60 && hours > 0 {
        bail!("{text}: minutes past 59 after hours");
    }
    Ok(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1h30").unwrap(), 90);
        assert_eq!(parse_duration("45m").unwrap(), 45);
        assert_eq!(parse_duration("2h").unwrap(), 120);
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert!(parse_duration("1h75").is_err());
    }
}
"#,
    ),
    (
        "src/report.rs",
        r#"use crate::entry::Entry;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Total minutes per project, largest first.
pub fn by_project(entries: &[Entry]) -> String {
    let mut totals: BTreeMap<&str, u32> = BTreeMap::new();
    for e in entries {
        *totals.entry(&e.project).or_default() += e.minutes;
    }
    let mut rows: Vec<(&str, u32)> = totals.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    table("project", rows.iter().map(|(p, m)| (p.to_string(), *m)))
}

/// Total minutes per day, oldest first.
pub fn by_day(entries: &[Entry]) -> String {
    let mut totals = BTreeMap::new();
    for e in entries {
        *totals.entry(e.date).or_insert(0) += e.minutes;
    }
    table("day", totals.into_iter().map(|(d, m)| (d.format("%a %d %b").to_string(), m)))
}

fn table(heading: &str, rows: impl Iterator<Item = (String, u32)>) -> String {
    let mut out = format!("{heading:<16} {:>8}\n", "time");
    let mut sum = 0;
    for (label, minutes) in rows {
        sum += minutes;
        let _ = writeln!(out, "{label:<16} {:>8}", hours(minutes));
    }
    let _ = writeln!(out, "{:<16} {:>8}", "total", hours(sum));
    out
}

fn hours(minutes: u32) -> String {
    format!("{}h{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hours_and_minutes() {
        assert_eq!(super::hours(95), "1h35");
    }
}
"#,
    ),
    (
        "tests/cli.rs",
        r#"use std::process::Command;

#[test]
fn totals_a_sample_week() {
    let out = Command::new(env!("CARGO_BIN_EXE_tally")).arg("examples/week.log").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("strata"), "{text}");
    assert!(text.lines().last().unwrap().starts_with("total"));
}
"#,
    ),
    (
        "examples/week.log",
        "# Week 42\n2026-10-12  1h30  strata   README screenshots\n2026-10-12  45m   inbox\n2026-10-13  2h    strata   release workflow\n2026-10-14  50m   reading\n",
    ),
];

/// Edits made after the commit (uncommitted, so the gutter marks them).
const TALLY_EDITS: &[(&str, &str, &str)] = &[
    (
        "src/parse.rs",
        "    let note: Vec<&str> = fields.collect();\n",
        "    let note: Vec<&str> = fields.collect();\n    if project.starts_with('#') {\n        bail!(\"a project name can't start with #\");\n    }\n",
    ),
    ("src/parse.rs", "/// `1h30`, `45m`, `2h` or `90` (minutes).", "/// `1h30`, `45m`, `2h` or a plain number of minutes (`90`)."),
    ("src/report.rs", "fn hours(minutes: u32) -> String {\n    format!(\"{}h{:02}\", minutes / 60, minutes % 60)", "fn hours(minutes: u32) -> String {\n    if minutes < 60 {\n        return format!(\"{minutes}m\");\n    }\n    format!(\"{}h{:02}\", minutes / 60, minutes % 60)"),
];

/// What a coding agent changed in a clean copy of tally (CSV export), for
/// Review Changes.
const AGENT_EDITS: &[(&str, &str, &str)] = &[
    (
        "src/main.rs",
        "    since: Option<chrono::NaiveDate>,\n}",
        "    since: Option<chrono::NaiveDate>,\n    /// Print comma-separated values instead of a table.\n    #[arg(long)]\n    csv: bool,\n}",
    ),
    (
        "src/main.rs",
        "    let table = if args.by_day { report::by_day(&entries) } else { report::by_project(&entries) };\n",
        "    let table = if args.csv {\n        report::csv(&entries)\n    } else if args.by_day {\n        report::by_day(&entries)\n    } else {\n        report::by_project(&entries)\n    };\n",
    ),
    (
        "src/report.rs",
        "fn table(heading: &str",
        r#"/// One line per entry (date, minutes, project, note) under a header.
pub fn csv(entries: &[Entry]) -> String {
    let mut out = String::from("date,minutes,project,note\n");
    for e in entries {
        let note = e.note.as_deref().unwrap_or_default().replace('"', "\"\"");
        let _ = writeln!(out, "{},{},{},\"{note}\"", e.date, e.minutes, e.project);
    }
    out
}

fn table(heading: &str"#,
    ),
    ("README.md", "- [ ] CSV export", "- [x] CSV export"),
    ("README.md", "| `tally --since 2026-10-01` | Skip older entries |\n", "| `tally --since 2026-10-01` | Skip older entries |\n| `tally --csv week.log` | One CSV line per entry |\n"),
];

/// A new test file the agent added.
const AGENT_TEST: &str = r#"use std::process::Command;

#[test]
fn csv_has_a_header_and_a_line_per_entry() {
    let out = Command::new(env!("CARGO_BIN_EXE_tally")).args(["--csv", "examples/week.log"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("date,minutes,project,note"));
    assert_eq!(lines.count(), 4);
}
"#;

/// The language server's answers in the LSP scene, as rust-analyzer would
/// give them for parse.rs with a `?` left out.
fn lsp_answers() -> serde_json::Value {
    let method = |name: &str, sig: &str, doc: &str| serde_json::json!({"label": name, "kind": 2, "detail": sig, "documentation": {"kind": "markdown", "value": doc}});
    serde_json::json!({
        "problems": [{"find": "parse_duration(time),", "severity": 1, "source": "rustc", "message": "mismatched types\nexpected `u32`, found `Result<u32, Error>`"}],
        "hover": {"parse_duration": "```rust\ntally::parse\n```\n\n```rust\npub fn parse_duration(text: &str) -> Result<u32>\n```\n\n---\n\n`1h30`, `45m`, `2h` or a plain number of minutes (`90`)."},
        "completions": [
            method("checked_mul", "const fn(self, rhs: u32) -> Option<u32>", "Checked integer multiplication. Computes `self * rhs`, returning `None` if overflow occurred."),
            method("checked_add", "const fn(self, rhs: u32) -> Option<u32>", "Checked integer addition. Computes `self + rhs`, returning `None` if overflow occurred."),
            method("checked_sub", "const fn(self, rhs: u32) -> Option<u32>", "Checked integer subtraction. Computes `self - rhs`, returning `None` if overflow occurred."),
            method("checked_div", "const fn(self, rhs: u32) -> Option<u32>", "Checked integer division. Computes `self / rhs`, returning `None` if `rhs == 0`."),
            method("checked_pow", "const fn(self, exp: u32) -> Option<u32>", "Checked exponentiation. Computes `self.pow(exp)`, returning `None` if overflow occurred."),
            method("checked_rem", "const fn(self, rhs: u32) -> Option<u32>", "Checked integer remainder. Computes `self % rhs`, returning `None` if `rhs == 0`."),
        ],
    })
}

/// A small TypeScript web app with a Python script, for the light scheme.
const ORBIT: &[(&str, &str)] = &[
    ("package.json", "{\n  \"name\": \"orbit\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"scripts\": { \"dev\": \"vite\", \"test\": \"vitest\" }\n}\n"),
    ("tsconfig.json", "{\n  \"compilerOptions\": { \"strict\": true, \"target\": \"ES2022\", \"module\": \"ESNext\" }\n}\n"),
    ("README.md", "# orbit\n\nA tiny launch tracker.\n"),
    (
        "src/api.ts",
        r#"import type { Launch, LaunchStatus } from "./types";

const BASE = "https://api.example.com/v2";

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(`${BASE}${path}`, { signal });
  if (!res.ok) {
    throw new ApiError(res.status, `${path}: ${res.statusText}`);
  }
  return (await res.json()) as T;
}

/** Upcoming launches, soonest first. */
export async function upcoming(limit = 20): Promise<Launch[]> {
  const launches = await get<Launch[]>(`/launches?limit=${limit}`);
  return launches.sort((a, b) => a.window.start.localeCompare(b.window.start));
}

/** Launches grouped by status, for the dashboard columns. */
export function byStatus(launches: Launch[]): Map<LaunchStatus, Launch[]> {
  const groups = new Map<LaunchStatus, Launch[]>();
  for (const launch of launches) {
    const list = groups.get(launch.status) ?? [];
    list.push(launch);
    groups.set(launch.status, list);
  }
  return groups;
}

export const isScrubbed = (l: Launch) => l.status === "scrubbed";
"#,
    ),
    (
        "src/types.ts",
        r#"export type LaunchStatus = "scheduled" | "go" | "scrubbed" | "success" | "failure";

export interface Launch {
  id: string;
  name: string;
  vehicle: string;
  pad: string;
  status: LaunchStatus;
  window: { start: string; end: string };
}
"#,
    ),
    ("src/main.ts", "import { upcoming, byStatus } from \"./api\";\n\nconst launches = await upcoming();\nconsole.table([...byStatus(launches)].map(([s, l]) => ({ status: s, count: l.length })));\n"),
    (
        "scripts/seed.py",
        r#""""Write a week of sample launches to fixtures/launches.json."""

from __future__ import annotations

import json
import random
from dataclasses import asdict, dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

VEHICLES = ["Falcon 9", "Electron", "Vulcan", "Ariane 6", "New Glenn"]
PADS = {"Falcon 9": "SLC-40", "Electron": "LC-1B", "Vulcan": "SLC-41"}


@dataclass
class Launch:
    id: str
    name: str
    vehicle: str
    pad: str
    status: str
    start: str


def sample(n: int, seed: int = 42) -> list[Launch]:
    rng = random.Random(seed)
    now = datetime.now(timezone.utc).replace(minute=0, second=0, microsecond=0)
    launches = []
    for i in range(n):
        vehicle = rng.choice(VEHICLES)
        start = now + timedelta(hours=rng.randint(6, 7 * 24))
        launches.append(
            Launch(
                id=f"L{i:04}",
                name=f"Mission {i + 1}",
                vehicle=vehicle,
                pad=PADS.get(vehicle, "TBD"),
                status=rng.choices(["scheduled", "go", "scrubbed"], [6, 3, 1])[0],
                start=start.isoformat(),
            )
        )
    return sorted(launches, key=lambda l: l.start)


if __name__ == "__main__":
    out = Path("fixtures/launches.json")
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps([asdict(l) for l in sample(24)], indent=2))
    print(f"wrote {out}")
"#,
    ),
];

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    let _ = std::fs::remove_dir_all(root);
    for (name, text) in files {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=Sample", "-c", "user.email=sample@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(ok, "git {args:?}");
}

/// Replace the first `from` with `to` in each file (which must contain it).
fn edit(root: &Path, edits: &[(&str, &str, &str)]) {
    for (file, from, to) in edits {
        let p = root.join(file);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains(from), "{file}: {from}");
        std::fs::write(&p, text.replacen(from, to, 1)).unwrap();
    }
}

/// The tally project at `dir`, committed.
fn tally_repo(dir: &Path) {
    write_tree(dir, TALLY);
    git(dir, &["init", "-q"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "Parse and total time logs"]);
}

/// The sample projects, under a fresh temporary folder.
fn projects() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join("strata-readme-shots");
    let tally = base.join("tally");
    tally_repo(&tally);
    edit(&tally, TALLY_EDITS);
    let orbit = base.join("orbit");
    write_tree(&orbit, ORBIT);
    (tally, orbit)
}

/// A window on `folder` in `scheme`, with the README's look.
fn window(sh: &mut Shooter, ctx: &Context, folder: Option<&Path>, scheme: &str) -> StrataApp {
    let mut app = StrataApp::new(folder.map(Path::to_path_buf), None);
    app.settings.theme_mode = "fixed".into();
    app.settings.theme = scheme.into();
    app.settings.font_face = "JetBrains Mono".into();
    app.settings.ui_font_face = "Inter".into();
    app.settings.font_size = 14.0;
    app.settings.minimap = true;
    app.settings.help_in_own_window = false;
    app.apply_settings();
    sh.run(ctx, &mut app, 3, vec![]);
    app
}

fn open(app: &mut StrataApp, path: PathBuf) -> usize {
    app.open_path(path, false);
    app.active
}

/// Let background work land (git changes, highlighting), then clear the status line.
fn settle(sh: &mut Shooter, ctx: &Context, app: &mut StrataApp, until: impl Fn(&StrataApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        sh.run(ctx, app, 1, vec![]);
        if until(app) {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    for _ in 0..4 {
        sh.run(ctx, app, 1, vec![]);
        std::thread::sleep(Duration::from_millis(20));
    }
    app.status.clear();
}

/// Char ranges of each match of `word` in tab `i` within lines `lines` (1-based, inclusive).
fn occurrences(app: &StrataApp, i: usize, word: &str, lines: std::ops::RangeInclusive<usize>) -> Vec<std::ops::Range<usize>> {
    let b = &app.docs[i].editor.buffer;
    let text = b.slice(0..b.len_chars());
    let mut out = Vec::new();
    let mut offset = 0;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if lines.contains(&(n + 1)) {
            for (at, _) in line.match_indices(word) {
                let start = offset + line[..at].chars().count();
                out.push(start..start + word.chars().count());
            }
        }
        offset += line.chars().count();
    }
    out
}

#[test]
fn readme_shots() {
    let Some(dir) = std::env::var_os("STRATA_README_SHOTS").map(PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let Some(mut sh) = Shooter::new(W, H, 2.0) else { return };
    let (tally, orbit) = projects();
    let shot = |name: &str| dir.join(format!("{name}.ppm"));

    // Hero: a Rust project in the side bar, three cursors, git change bars, the minimap.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, Some(&tally), "Tokyo Night");
        open(&mut app, tally.join("src/main.rs"));
        open(&mut app, tally.join("src/report.rs"));
        let i = open(&mut app, tally.join("src/parse.rs"));
        let _ = app.sidebar.reveal(&tally.join("src/parse.rs"));
        app.goto_line(1);
        // `minutes` in parse_duration's body: a cursor on each.
        let b = &app.docs[i].editor.buffer;
        let first = b.slice(0..b.len_chars()).lines().position(|l| l.starts_with("pub fn parse_duration")).unwrap() + 1;
        let ranges = occurrences(&app, i, "minutes", first + 5..=first + 14);
        assert!(ranges.len() >= 3, "{ranges:?}");
        app.docs[i].editor.select_ranges(&ranges, ranges.len() - 1);
        settle(&mut sh, &ctx, &mut app, |a| !a.docs[a.active].git_changes.is_empty());
        // From the blank line above parse_line, so no sticky header covers the top.
        let top = app.docs[i].view.top_line() as isize;
        app.docs[i].view.scroll_lines(17 - top);
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 3, vec![], &shot("hero"));
    }

    // A light scheme: TypeScript and Python side by side.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "GitHub Light");
        app.settings.minimap = false;
        open(&mut app, orbit.join("src/types.ts"));
        open(&mut app, orbit.join("src/api.ts"));
        app.run(&ctx, Cmd::LayoutColumns2);
        open(&mut app, orbit.join("scripts/seed.py"));
        app.move_to_pane(1);
        app.goto_line(24);
        app.focus_pane(0);
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 3, vec![], &shot("light-split"));
    }

    // Markdown with its live preview.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "Catppuccin Mocha");
        open(&mut app, tally.join("README.md"));
        app.settings.markdown_preview = true;
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 3, vec![], &shot("markdown"));
    }

    // The terminal under the editor.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, Some(&tally), "Strata Dark");
        app.settings.terminal_height = 400.0;
        open(&mut app, tally.join("src/main.rs"));
        app.goto_line(25);
        let script = std::env::temp_dir().join("strata-readme-term.sh");
        std::fs::write(&script, TERMINAL_SCRIPT).unwrap();
        let shell = ("/bin/sh".to_owned(), vec![script.display().to_string()]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &tally, Some(shell), 100, 16).unwrap());
        app.terminal.visible = true;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            sh.run(&ctx, &mut app, 1, vec![]);
            if app.terminal.active_terminal().unwrap().screen_text().contains("Cargo.toml") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 3, vec![], &shot("terminal"));
    }

    // The command palette: every command, with its keys.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, Some(&tally), "One Dark");
        open(&mut app, tally.join("src/main.rs"));
        let _ = app.sidebar.reveal(&tally.join("src/main.rs"));
        settle(&mut sh, &ctx, &mut app, |_| true);
        app.dev_palette = Some("cmd:toggle".into());
        sh.shoot(&ctx, &mut app, 4, vec![], &shot("command-palette"));
    }

    // Compare with the last commit.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "Kanagawa Wave");
        let i = open(&mut app, tally.join("src/parse.rs"));
        settle(&mut sh, &ctx, &mut app, |a| !a.docs[a.active].git_changes.is_empty());
        app.compare_with_head(i);
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 4, vec![], &shot("compare"));
    }

    // The Settings window.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "Strata Dark");
        open(&mut app, tally.join("src/report.rs"));
        settle(&mut sh, &ctx, &mut app, |_| true);
        app.open_settings_window();
        sh.shoot(&ctx, &mut app, 6, vec![], &shot("settings"));
    }

    // The manual.
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "Strata Light");
        open(&mut app, tally.join("src/report.rs"));
        settle(&mut sh, &ctx, &mut app, |_| true);
        app.open_help(Some("find.md"), false);
        sh.shoot(&ctx, &mut app, 6, vec![], &shot("help"));
    }

    // Review Changes: what an agent changed in a clean copy of tally, the
    // Changes list and one file's changes side by side.
    {
        let repo = std::env::temp_dir().join("strata-readme-shots/review/tally");
        tally_repo(&repo);
        edit(&repo, AGENT_EDITS);
        std::fs::write(repo.join("tests/csv.rs"), AGENT_TEST).unwrap();
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, Some(&repo), "Nord");
        app.settings.minimap = false;
        app.review_show();
        settle(&mut sh, &ctx, &mut app, |a| a.review.scan.as_ref().is_some_and(|s| s.files.len() == 4));
        app.review_set_reviewed(&repo.join("README.md"), true);
        app.open_review(&repo.join("src/main.rs"));
        // The committed version is read in the background.
        settle(&mut sh, &ctx, &mut app, |a| a.review.pending_open.is_none());
        settle(&mut sh, &ctx, &mut app, |_| true);
        sh.shoot(&ctx, &mut app, 4, vec![], &shot("review"));
    }

    // Language servers: a problem under the pointer with the server's hover
    // information, and its completions while typing (the test server, so no
    // real one is needed).
    {
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, None, "Dracula");
        app.settings.minimap = false;
        app.settings.auto_complete = true;
        // Named like the real server, as the status bar shows the command's name.
        let mut server = crate::lsp::fake::config("rust-analyzer", &["*.rs"], lsp_answers());
        let named = std::env::temp_dir().join("strata-readme-shots/bin/rust-analyzer");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&named);
        std::os::unix::fs::symlink(&server.command, &named).unwrap();
        server.command = named.display().to_string();
        app.lsp.hub.borrow_mut().use_servers(vec![server]);
        app.settings.lsp_trust_all = true;
        app.open_folder(tally.clone());
        let i = open(&mut app, tally.join("src/parse.rs"));
        let _ = app.sidebar.reveal(&tally.join("src/parse.rs"));
        // The `?` after parse_duration(time), left out.
        let find = |app: &StrataApp, needle: &str| {
            let b = &app.docs[i].editor.buffer;
            b.byte_to_char(b.slice(0..b.len_chars()).find(needle).unwrap())
        };
        let at = find(&app, "parse_duration(time)?") + "parse_duration(time)".len();
        app.docs[i].editor.select_ranges(&[at..at + 1], 0);
        app.docs[i].editor.insert_text("");
        let top = app.docs[i].view.top_line() as isize;
        app.docs[i].view.scroll_lines(17 - top);
        settle(&mut sh, &ctx, &mut app, |a| a.docs[i].editor.markers().iter().any(|m| m.style == 1));
        // Where the pointer will rest: on parse_duration in that line.
        let call = find(&app, "parse_duration(time),") + 4;
        app.docs[i].editor.select_ranges(&[call..call], 0);
        sh.run(&ctx, &mut app, 2, vec![]);
        let pointer = app.docs[i].view.caret_rect().unwrap().center();
        // Typing in parse_duration: the server's completions.
        let body = find(&app, "    Ok(hours * 60 + minutes)");
        app.docs[i].editor.select_ranges(&[body..body], 0);
        app.docs[i].editor.insert_text("\n");
        app.docs[i].editor.select_ranges(&[body..body], 0);
        sh.run(&ctx, &mut app, 1, vec![Event::Text("    let total = hours.".into())]);
        sh.run(&ctx, &mut app, 1, vec![Event::Text("ch".into())]);
        settle(&mut sh, &ctx, &mut app, |a| a.completion.labels().iter().any(|l| l == "checked_add"));
        // Then the pointer rests on the problem.
        sh.run(&ctx, &mut app, 1, vec![Event::PointerMoved(pointer)]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && ctx.memory(|m| m.area_rect(egui::Id::new("lsp-hover"))).is_none() {
            sh.run(&ctx, &mut app, 1, vec![]);
            std::thread::sleep(Duration::from_millis(40));
        }
        // The server's information lands under the problem, and the box grows to fit.
        for _ in 0..12 {
            sh.run(&ctx, &mut app, 1, vec![]);
            std::thread::sleep(Duration::from_millis(40));
        }
        app.status.clear();
        sh.shoot(&ctx, &mut app, 3, vec![], &shot("language-server"));
    }

    // Terminal links: ⌘ (Ctrl) held over a compiler error's location.
    {
        let report = tally.join("src/report.rs");
        let good = std::fs::read_to_string(&report).unwrap();
        let bad = good.replacen("return format!(\"{minutes}m\");", "return minutes;", 1);
        std::fs::write(&report, &bad).unwrap();
        let line_of = |needle: &str| bad.lines().position(|l| l.contains(needle)).unwrap() + 1;
        let (sig, cond, ret) = (line_of("fn hours("), line_of("if minutes < 60"), line_of("return minutes;"));
        let script = LINK_SCRIPT.replace("@SIG", &sig.to_string()).replace("@COND", &cond.to_string()).replace("@RET", &ret.to_string());
        let ctx = Context::default();
        let mut app = window(&mut sh, &ctx, Some(&tally), "Gruvbox Dark");
        app.settings.terminal_height = 420.0;
        open(&mut app, tally.join("src/main.rs"));
        open(&mut app, report.clone());
        app.goto_line(ret - 1);
        let path = std::env::temp_dir().join("strata-readme-links.sh");
        std::fs::write(&path, script).unwrap();
        let shell = ("/bin/sh".to_owned(), vec![path.display().to_string()]);
        app.terminal.tabs.push(crate::terminal::Terminal::spawn(Some(ctx.clone()), &tally, Some(shell), 100, 18).unwrap());
        app.terminal.visible = true;
        settle(&mut sh, &ctx, &mut app, |a| a.terminal.active_terminal().unwrap().screen_text().contains("could not compile"));
        let location = format!("src/report.rs:{ret}:16");
        let t = app.terminal.active_terminal().unwrap();
        let (row, col) = t.screen_text().lines().enumerate().find_map(|(r, l)| l.find(&location).map(|c| (r, c))).unwrap();
        let (cw, ch) = t.cell_size();
        let body = app.terminal.body.as_ref().unwrap().rect;
        let p = body.min + egui::vec2(6.0 + (col + 6) as f32 * cw, 4.0 + (row as f32 + 0.5) * ch);
        let held = if crate::keys::MAC { egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND } else { egui::Modifiers::CTRL | egui::Modifiers::COMMAND };
        sh.run(&ctx, &mut app, 1, vec![Event::ModifiersChanged(held), Event::PointerMoved(p)]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && app.terminal.active_terminal().unwrap().hovered_link().is_none() {
            sh.run(&ctx, &mut app, 1, vec![Event::PointerMoved(p)]);
            std::thread::sleep(Duration::from_millis(20));
        }
        app.status.clear();
        sh.shoot(&ctx, &mut app, 3, vec![Event::PointerMoved(p)], &shot("terminal-links"));
        std::fs::write(&report, good).unwrap();
    }
}

/// What the terminal links scene runs: a build that fails with a rustc
/// error (`@SIG`, `@COND` and `@RET` are line numbers in report.rs).
const LINK_SCRIPT: &str = r#"
G='\033[1;32m'; C='\033[36m'; B='\033[1;34m'; M='\033[35m'; E='\033[1;31m'; W='\033[1m'; R='\033[0m'
printf "${C}tally${R} ${M}main${R} ${B}❯${R} cargo build\n"
printf "${G}   Compiling${R} tally v0.4.0\n"
printf "${E}error[E0308]${R}${W}: mismatched types${R}\n"
printf "  ${B}-->${R} src/report.rs:@RET:16\n"
printf "   ${B}|${R}\n"
printf "${B}%-3s|${R} fn hours(minutes: u32) -> String {\n" @SIG
printf "   ${B}|${R}                           ${B}------${R} ${B}expected \`String\` because of return type${R}\n"
printf "${B}%-3s|${R}     if minutes < 60 {\n" @COND
printf "${B}%-3s|${R}         return minutes;\n" @RET
printf "   ${B}|${R}                ${E}^^^^^^^${R} ${E}expected \`String\`, found \`u32\`${R}\n"
printf "   ${B}|${R}\n"
printf "${B}help${R}: try using a conversion method: \`.to_string()\`\n\n"
printf "${W}For more information about this error, try \`rustc --explain E0308\`.${R}\n"
printf "${E}error${R}${W}: could not compile \`tally\` (bin \"tally\") due to 1 previous error${R}\n"
printf "${C}tally${R} ${M}main${R} ${B}❯${R} "
sleep 30
"#;

/// What the terminal scene runs: a prompt, a test run with cargo's colors,
/// a colored listing, and a fresh prompt.
const TERMINAL_SCRIPT: &str = r#"
G='\033[1;32m'; C='\033[36m'; B='\033[1;34m'; M='\033[35m'; D='\033[2m'; R='\033[0m'
prompt() { printf "${C}tally${R} ${M}main${R} ${B}❯${R} %s\n" "$1"; }
prompt "cargo test"
printf "${G}   Compiling${R} tally v0.4.0\n"
printf "${G}    Finished${R} \`test\` profile [unoptimized + debuginfo] target(s) in 1.84s\n"
printf "${G}     Running${R} unittests src/main.rs\n\n"
printf "running 2 tests\n"
printf "test parse::tests::durations ... ${G}ok${R}\n"
printf "test report::tests::hours_and_minutes ... ${G}ok${R}\n\n"
printf "test result: ${G}ok${R}. 2 passed; 0 failed; 0 ignored; finished in 0.00s\n\n"
prompt "ls"
CLICOLOR=1 CLICOLOR_FORCE=1 ls -G 2>/dev/null || ls --color=always
printf "${C}tally${R} ${M}main${R} ${B}❯${R} "
sleep 30
"#;

