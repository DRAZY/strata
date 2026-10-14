# About Strata

Strata is a fast, open-source code and text editor for macOS and Windows, written in Rust and built for people who like to keep their hands on the keyboard.

It opens in a blink, stays smooth on files of hundreds of megabytes, and gives you the tools you reach for every day without getting in the way. Strata is in alpha, so expect some rough edges and frequent improvements.

## What Strata does

| Area | What you get |
|---|---|
| Editing | [Multiple cursors](editing.md#multiple-cursors) and box selection, smart indentation, auto-closing brackets, word completion, code folding, macros, bookmarks, case conversion and line sorting. |
| Navigation | [Goto Anything](find.md#goto-anything) for files, lines and symbols, the [command palette](find.md#command-palette), Go to Definition, and jumping back and forward. |
| Search | A find bar with regular expressions and In Selection, plus [Find in Files and Replace in Files](find.md#find-in-files) across whole folders, with undo. |
| Layout | [Tabs, split panes and multiple windows](panes.md), several views of one file, and a distraction-free mode. |
| Projects | A [side bar](projects.md) with one or more folders, and project files that remember your folders and layout. |
| Built-in tools | A [terminal](terminal.md), [build systems](build.md) with clickable errors, a live [Markdown preview](markdown.md), and [file comparison](compare.md). |
| Languages | Syntax highlighting for about 190 [languages](languages.md) and file formats, with symbols and smart indentation for many of them. |
| Look and feel | 35 [color schemes](appearance.md#color-schemes) that can follow your system's light or dark mode, bundled coding fonts, and a minimap. |
| Writing | [Spell check](editing.md#spell-check) for comments, strings and prose, with US and UK English dictionaries built in. |
| Git | [Change bars](git.md) in the gutter for lines changed since the last commit, the current branch in the status bar, and Compare with HEAD. |
| Files | [Hot exit](welcome.md#hot-exit), so quitting never asks and nothing is lost; 40 [encodings](encodings.md); atomic saves; [printing and PDF export](printing.md); and a [large-file mode](troubleshooting.md#very-large-files). |
| Customizing | [Settings](settings.md) and [key bindings](keybindings.md) in plain JSON files that apply as soon as you save, plus your own [snippets](snippets.md), syntaxes and color schemes. |

## Principles

- **Fast.** Strata starts quickly, types without lag, and keeps big and minified files responsive by only doing work for what's on screen.
- **Keyboard first.** Every command is in the command palette and can have a shortcut. The mouse works everywhere too, but you never need it.
- **Local and private.** Strata has no accounts and no telemetry, and it doesn't check for updates or download anything. It only goes online when you ask it to, for example by clicking a link, which opens in your browser. Logs and crash reports stay on your computer, and Strata never sends them anywhere; see [Report a problem](troubleshooting.md#report-a-problem).
- **Yours to shape.** Settings, key bindings, snippets, syntaxes, color schemes and build systems are all plain files you can edit, copy and share.

Snippets, syntaxes and color schemes use widely shared file formats (`.sublime-snippet`, `.sublime-syntax`, `.tmTheme` and `.sublime-color-scheme`), so existing collections work unchanged.

## Version

To see which version you have:

- Choose **Help ▸ About Strata** on Windows and Linux, or **Strata ▸ About Strata** on macOS. The command palette name is **Help: About Strata**.
- Or run `strata --version` in a terminal. See [Command Line](cli.md).

## License

Strata is free and open source under the MIT License. You can use, copy, change and share it, including for commercial work. The full text is in the `LICENSE` file that comes with the source code.

Strata is original work, written from scratch. Its support for existing file formats is built from public documentation and openly licensed files.

## Third-party software and attribution

Strata is built on open-source projects, and it ships fonts, color schemes and syntax definitions made by others. To see them all, choose **Help ▸ Licenses and Attribution…**, or click **Licenses and Attribution…** in the About window. The window lists:

- **Open-source libraries** Strata is built on, with their licenses (mostly MIT or Apache-2.0), including the terminal emulator (alacritty_terminal, Apache-2.0) and the Markdown parser (pulldown-cmark, MIT).
- **All Rust crates**: every crate compiled into Strata, with its version and license.
- **Syntax definitions**, which come from syntect, the bat project and community grammar packages, each under its own license. License texts that the bundled grammars ask to be reproduced are included. Strata's own PowerShell and JSX grammars are MIT licensed.
- **Color schemes**, with the project, author and license each palette comes from. Strata's schemes are those projects' published colors applied to Strata's own highlighting rules. The Classic schemes come with syntect (MIT).
- **Fonts**: JetBrains Mono, Fira Code, IBM Plex Mono, Source Code Pro and Inter are bundled under the SIL Open Font License 1.1, and the full license text of each is included. The built-in fallback fonts, Hack and Ubuntu, come with the interface toolkit Strata is built on.
- **Dictionaries**: the English spell-check dictionaries come from SCOWL.

## Supported platforms

| Platform | Status |
|---|---|
| macOS on Apple silicon | Supported |
| Windows on x64 | Supported |
| Linux | Not officially supported yet. The code has Linux support (settings live in `~/.config/strata`, for example), but it isn't tested or packaged |

## Getting help

- **This manual**: choose **Help ▸ Documentation** ({{key:documentation}}).
- **Every shortcut**: choose **Help ▸ Keyboard Shortcuts**, or see [Keyboard Shortcuts](shortcuts.md).
- **Something not working?** See [Troubleshooting and FAQ](troubleshooting.md), or choose **Help ▸ Report a Problem…**.
