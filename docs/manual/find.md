# Find and Replace

Strata can search the current file, a whole folder, and your project's file names and symbols. This page covers the find bar, Find in Files, Replace in Files, Goto Anything and the other ways to jump around your code.

## The Find Bar

Open the find bar with {{key:show_find}} (Find ▸ Find…). It appears at the bottom of the window with a Find field, the match count and Previous, Next and Find All buttons. If you have text selected on a single line when you open it, that text becomes the search.

Find is always incremental: as you type, Strata selects the first match after the spot where your cursor was when the bar opened, and highlights every match in the file. Find ▸ Incremental Find ({{key:incremental_find}}) opens the same bar.

The label next to the field tells you where you are: "3 of 12", "12 matches" (when the cursor isn't on a match) or "No results". An invalid regular expression shows its error in red instead.

### Search options

Three toggle buttons sit to the left of the Find field. Click one to turn it on or off.

| Button | Option | What it does |
|---|---|---|
| `.*` | Regular expression | Treat the text as a regular expression. |
| `Aa` | Case sensitive | Match upper and lower case exactly. Off by default, so searches ignore case. |
| `"ab"` | Whole word | Only match whole words. |

The options have no keyboard shortcuts. They stay as you set them while Strata is running.

### Keys in the find bar

| Key | In the Find field |
|---|---|
| Enter | Find the next match. |
| Shift+Enter | Find the previous match. |
| Alt+Enter (⌥↩ on macOS) | Find All: put a cursor on every match and close the bar, so you can edit them all at once. |
| Esc | Close the find bar and return to the editor. |

Outside the bar, {{key:find_next}} and {{key:find_prev}} (Find ▸ Find Next and Find Previous) step through matches of the last search. Both wrap around the end of the file. If nothing has been searched yet, Find Next opens the find bar.

### Replace

Open the find bar with its Replace field using {{key:show_replace}} (Find ▸ Replace…).

| Key or button | What it does |
|---|---|
| Enter in the Replace field, or the Replace button | Replace the current match and move to the next one. |
| Ctrl+Enter (⌘↩ on macOS) in the Replace field, or the Replace All button | Replace every match in the file. |
| {{key:replace_next}} (Find ▸ Replace Next) | Replace the current match and move on, without opening the bar. |

Replace All is a single undo step, and the status bar tells you how many occurrences were replaced.

### Use the selection

| Command | Shortcut | What it does |
|---|---|---|
| Find ▸ Use Selection for Find | {{key:slurp_find_string}} | Put the selected text in the Find field (escaped if regular expressions are on). |
| Find ▸ Use Selection for Replace | {{key:slurp_replace_string}} | Put the selected text in the Replace field. |

### Quick find

These commands search for the selection, or the word at the cursor, without opening the bar.

| Command | Shortcut | What it does |
|---|---|---|
| Find ▸ Quick Find | {{key:find_under}} | Select the next occurrence of the selected text. It needs a selection. The search is case sensitive, and Find Next continues from it. |
| Find ▸ Quick Add Next | {{key:find_under_expand}} | Add the next occurrence as another selection. With nothing selected, it first selects the word under each cursor; after that it matches whole words only. |
| Find ▸ Quick Find All | {{key:find_all_under}} | Select every occurrence at once. With nothing selected, it uses the word at the cursor and matches whole words. |

Quick Add Next and Quick Find All are the quickest way to get multiple cursors. See [Editing](editing.md) for working with them.

## Regular Expressions

With `.*` turned on, the Find field takes a regular expression. Strata uses a fast regular expression engine that supports the usual syntax (character classes, `\d`, `\w`, `\b`, groups, named groups like `(?P<name>...)`, alternation and repetition) but not look-around or backreferences inside the pattern. `^` and `$` match at the start and end of each line.

In the find bar a pattern can match across lines (for example `\n` matches a line break). Find in Files searches one line at a time, so its matches never span lines.

### Replacement syntax

In the Replace field, refer to captured groups like this:

| Write | To insert |
|---|---|
| `$1`, `$2`… | A numbered group. |
| `${name}` | A named group. |
| `${1}x` | Group 1 followed by `x` (use braces when letters or digits follow the group number). |
| `$$` | A literal dollar sign. |

Replace in Files also understands the Sublime Text forms:

| Write | To insert |
|---|---|
| `\1` … `\9` | A numbered group. |
| `\n`, `\t` | A line break or a tab. |
| `\\` | A backslash. |
| `\$` | A dollar sign. |

> [!NOTE]
> The `\1`, `\n` and `\t` forms only work in Replace in Files. In the find bar's Replace field, use `$1` for groups.

Without `.*`, the replacement text is inserted exactly as typed.

## Find in Files

Find in Files searches every file in the folder open in the sidebar. Open it with {{key:find_in_files}} (Find ▸ Find in Files…, or "Find: Find in Folder…" in the Command Palette). The sidebar switches to its Search tab and puts the cursor in the Find field. A single-line selection in the editor becomes the search text.

> [!NOTE]
> Find in Files lives in the sidebar, so it needs an open folder. See [Projects and Sidebar](projects.md) for opening one.

The Search tab has its own `.*`, `Aa` and `"ab"` toggles (separate from the find bar's) and three fields:

| Field | What goes in it |
|---|---|
| Find | The text or regular expression to look for. |
| Where | Which folders and files to search. Leave it empty to search the whole open folder. |
| Replace | The replacement text, for Replace in Files. |

Press Enter in any of the fields, or click Find, to search. Results stream in while the search runs; a spinner and a Stop button show while it's busy. Find ▸ Cancel Find in Files ({{key:cancel_find_in_files}}) also stops it.

### The Where field

Where is a comma-separated list. Each entry can be:

| Entry | Meaning |
|---|---|
| `src`, `docs/api` | A folder, relative to the open folder. |
| `/abs/path`, `~/notes` | An absolute folder, or one in your home folder. |
| `src/main.rs` | A single file. |
| `*.rs`, `*.{ts,tsx}` | Include only files matching this pattern. |
| `-*.min.js`, `-target/` | Exclude files or folders matching this pattern (a leading `-`). |
| `<open folders>` | The folder open in the sidebar. `<project>` means the same. |
| `<open files>` | Every file open in a tab. |

How patterns match:

- A pattern without a `/` matches file names, at any depth. `*.rs` finds Rust files in every folder.
- An exclude pattern without a `/` also matches folder names, so `-node_modules` or `-target/` skips those folders entirely. A trailing `/` is ignored.
- A pattern with a `/` matches the path relative to the folder being searched, such as `src/*.rs` or `-tests/fixtures/*`.
- If Where contains only patterns, Strata searches the whole open folder with them applied.

For example, `src, tests, *.rs, -*_generated.rs` searches Rust files under `src` and `tests`, skipping generated ones.

An entry that names a folder or file that doesn't exist stops the search with a "No such folder" message.

### What gets searched

- Files and folders that your `.gitignore` excludes are skipped, even if the folder isn't a git repository.
- Hidden files and folders (names starting with a dot) are skipped.
- Binary files and files larger than 50 MB are skipped.
- Files listed by name in Where, or through `<open files>`, are searched even if they would otherwise be ignored.
- If a file is open in a tab with unsaved changes, Strata searches the text in the tab, not the copy on disk.

The search stops after 10,000 matches and the summary says so. The summary line reads like "42 matches in 7 files · 1,300 scanned".

### Results

Results appear in the Search tab, sorted by file path. Each file's path is shown in bold, followed by one row per match with its line number and the line's text, the match highlighted.

Click a match to open the file and select it. If the file is already open in another window, that window comes forward instead.

To step through results from the keyboard, use Find ▸ Find Results ▸ Next Result ({{key:next_find_result}}) and Previous Result ({{key:prev_find_result}}). They wrap around and show "Result 3 of 42" in the status bar. Find ▸ Find Results ▸ Show Panel brings the Search tab back.

## Replace in Files

Replace in Files changes every match of a Find in Files search. Open it with {{key:replace_in_files}} (Find ▸ Replace in Files…). The sidebar shows the Search tab; if a selection filled in the Find field, the cursor goes to the Replace field.

1. Fill in Find, Where and Replace, and set the options.
2. Click **Replace…**. Strata runs the search again so it works from fresh results.
3. A dialog asks "Replace N occurrences across M files?", shows the search and replacement, and says which files are open in tabs and which will be rewritten on disk. Press Enter or click Replace to go ahead, or Esc or Cancel to stop.

If the Replace field is empty, matches are deleted (the dialog says so). If the search hits the 10,000-match limit, Strata refuses to replace and asks you to narrow Where first.

### Open tabs and files on disk

- **Files open in a tab** change in the tab and are left unsaved. Each tab's changes are one undo step, so Edit ▸ Undo in that tab reverts them. Save when you're happy.
- **Every other file** is rewritten on disk in the background, keeping its encoding (including a byte order mark), line endings and permissions. A progress line shows "Replacing… 12 of 40 files", and Stop cancels the rest.

When it finishes, the Search tab and status bar report what happened, for example "Replaced 57 occurrences in 9 files (2 files in open tabs, unsaved)". The search then runs again so the results are current.

### Skipped files

A file on disk is left alone, and listed under "N skipped" with the reason, when:

- it changed since the search (different size, date or number of matches),
- it isn't valid in its encoding, so saving would lose bytes,
- it's read-only,
- it's binary, larger than 50 MB, or can't be read,
- the new text can't be written in the file's encoding,
- it had unsaved changes in a tab that has since been closed,
- writing failed, or you pressed Stop before Strata got to it.

### Undo Replace in Files

After a replace, an **Undo Replace in Files** button appears in the Search tab. It restores every file the last replace rewrote on disk. A file you've changed since the replace is left alone and listed as skipped.

- Undo covers only the most recent Replace in Files, and only until you close the window or its folder.
- Files in open tabs aren't part of it: use Undo in each tab.
- Strata keeps up to 256 MB of original file contents for undo. Files past that are still replaced, and the report says how many can't be undone.

## Goto Anything

Goto Anything ({{key:goto_anything}}, Goto ▸ Goto Anything…) opens a quick panel at the top of the window that finds files by fuzzy name. Type a few letters from anywhere in the path, in order: `srmain` finds `src/main.rs`. Separate words with spaces to match several parts in any order. Matches are ranked, with matched letters highlighted.

The file list covers the open folder (skipping `.gitignore`d and hidden files, up to 500,000 files) plus any open files outside it. Without a folder it lists your open files. While a large folder is still being indexed, the panel shows "Indexing…".

Add a suffix to go somewhere inside a file:

| Type | What happens |
|---|---|
| `name` | Open the best-matching file. |
| `:42` | Go to line 42 of the current file. |
| `@name` | Go to a symbol (function, class, heading…) in the current file. |
| `name:42` | Open the file and go to line 42. |
| `name@` | Open the file and show its symbol list. |

In the panel, use the Up and Down arrows to choose, Enter or a click to pick, and Esc (or a click outside) to close. While you move through lines or symbols, the editor previews each location; Esc puts your cursor back where it was.

## Go to Line

{{key:goto_line}} (Goto ▸ Goto Line…) opens Goto Anything with `:` already typed. Type a line number and press Enter. Numbers past the end go to the last line.

## Go to Symbol

{{key:goto_symbol}} (Goto ▸ Goto Symbol…) opens Goto Anything with `@` typed, listing the symbols in the current file: functions, classes, structs, types, Markdown headings, CSS selectors and so on. Each row shows the symbol's kind and line number. Type to filter.

Symbols are found by recognizing common declaration patterns line by line, so they cover most code in most languages without a full parser. An occasional symbol may be missed.

### Goto Symbol in Project

{{key:goto_symbol_in_project}} (Goto ▸ Goto Symbol in Project…) lists symbols from every file in the open folder, shown as `name · path:line`. Pick one to open that file at the symbol. It looks at up to 5,000 files with a recognized syntax (each up to 1 MB), skipping `.gitignore`d files. With no folder open, it acts like Go to Symbol.

## Go to Definition

{{key:goto_definition}} (Goto ▸ Goto Definition…) jumps to where the word under the cursor (or the selected text) is defined. Strata looks for a symbol with exactly that name:

1. in the current file,
2. then in the other open tabs,
3. then in files in the open folder with the same file extension as the current file.

If nothing is found, the status bar says "No definition found for …".

## Go to Reference

{{key:goto_reference}} (Goto ▸ Goto Reference…) searches the open folder for the word under the cursor, as a whole word and case sensitive. The results appear in the sidebar's Search tab; step through them with Next Result and Previous Result (see [Find in Files](find.md#find-in-files)). With no folder open, it opens the find bar with the word instead.

## Jump Back and Forward

Strata remembers where you were before each jump: opening a file from Goto Anything, going to a line or symbol, Go to Definition, clicking a Find in Files result, and so on. It keeps the last 100 places.

| Command | Shortcut |
|---|---|
| Goto ▸ Jump Back | {{key:jump_back}} |
| Goto ▸ Jump Forward | {{key:jump_forward}} |

Places in tabs you've since closed are skipped.

## Other Navigation

| Command | Shortcut | What it does |
|---|---|---|
| Goto ▸ Next Modification | {{key:next_modification}} | Jump to the next line changed since the last git commit. See [Git](git.md). |
| Goto ▸ Previous Modification | {{key:prev_modification}} | Jump to the previous changed line. |
| Goto ▸ Switch File ▸ Switch Header/Implementation | {{key:switch_file}} | Switch between a C, C++ or Objective-C header and its source file. |

Switch Header/Implementation looks in the same folder for a file with the same name: from a header (`.h`, `.hh`, `.hpp`, `.hxx`, `.h++`, `.inl`, `.ipp`) to a source file (`.c`, `.cc`, `.cpp`, `.cxx`, `.c++`, `.m`, `.mm`), and back.

## Command Palette

The Command Palette ({{key:command_palette}}, Tools ▸ Command Palette…) runs any command by name. Type part of a name, such as `save all` or `dupl`, and press Enter. It uses the same fuzzy matching as Goto Anything.

Each row shows the command's current shortcut on the right, so the palette is also a handy way to learn keys. Commands are named by area, like "File: Save All", "Find: Replace in Files…" or "Layout: Columns: 2", and "Set Syntax: …" entries switch the current file's language.

For the full list of shortcuts, see [Keyboard Shortcuts](shortcuts.md). To change them, see [Key Bindings](keybindings.md).
