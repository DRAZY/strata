# Find and Replace

Strata can search the current file, a whole folder, and your project's file names and symbols. This page covers the find bar, Find in Files, Replace in Files, Goto Anything and the other ways to jump around your code.

## The find bar

Open the find bar with {{key:show_find}} (Find ▸ Find…). It appears at the bottom of the window with a Find field, the match count and Previous, Next and Find All buttons. Find always shows just the Find row, even if the bar was showing Replace. If you have text selected on a single line when you open it, that text becomes the search.

Find is always incremental: as you type, Strata selects the first match after the spot where your cursor was when the bar opened, and highlights every match in the file. Find ▸ Incremental Find ({{key:incremental_find}}) opens the same bar.

The label next to the field tells you where you are: "3 of 12", "12 matches" (when the cursor isn't on a match) or "No results". An invalid regular expression shows its error in red instead.

### Search options

Toggle buttons sit to the left of the Find field. Click one, or press its key, to turn it on or off; hover over a button to see its key. The keys work whenever the window has the keyboard, so you can flip an option without leaving the editor.

| Button | Option | Key | What it does |
|---|---|---|---|
| `.*` | Regular expression | {{key:toggle_regex}} | Treat the text as a regular expression. |
| `Aa` | Case sensitive | {{key:toggle_case_sensitive}} | Match upper and lower case exactly. Off by default, so searches ignore case. |
| `"ab"` | Whole word | {{key:toggle_whole_word}} | Only match whole words. |
| `☰` | In selection | {{key:toggle_in_selection}} | Search only inside the selected text. See [In Selection](#in-selection). |
| `⟳` | Wrap | {{key:toggle_wrap}} | Find Next and Find Previous continue from the other end of the file. On by default. With it off they stop at the last (or first) match and the status bar says "No more matches". |
| `▣` | Highlight matches | {{key:toggle_highlight_matches}} | Outline every match in the file (and mark them on the scrollbar) while the bar is open. On by default. |
| `AB` | Preserve case | {{key:toggle_preserve_case}} | Shown on the Replace row: replacements take the case of the text they replace. See [Preserve case](#preserve-case). |

On macOS, Whole Word has no key by default because ⌥⌘W is View ▸ Word Wrap; bind `toggle_whole_word` in [Key Bindings](keybindings.md) if you want one. Wrap and Highlight Matches have no key by default. Every option is also in the command palette ("Find: Toggle In Selection" and so on).

The options, and the last search and replacement text, are saved with the window's session, so they're still set after you quit and reopen Strata.

### In Selection

With In Selection on, the find bar searches only the text that was selected when you turned it on. Select some lines (or make several selections with multiple cursors), then press {{key:toggle_in_selection}} or click `☰`. Next, Previous, Find All, Replace and Replace All then stay inside those ranges, and the match count only counts matches there. A match must fit entirely inside a selected range.

The searched ranges are remembered for each tab, so stepping through matches (which changes the selection) doesn't change what is searched. They follow your edits: replacing text inside them, or typing before them, keeps them covering the same text. To search a different part of the file, select it and turn In Selection off and on again, or reopen the find bar with a selection that spans several lines (or with several selections). If there was nothing selected, the label says "No selection".

The `auto_find_in_selection` setting (off by default) turns In Selection on by itself when you open the find bar while a selection spans several lines. A selection on a single line still becomes the search text instead.

Replace All with In Selection on replaces only inside the selection, as a single undo step, and the status bar says "Replaced N occurrences in the selection".

### Preserve case

With Preserve Case on, each replacement is changed to match the case of the text it replaces:

| Found | Replace with `bar` gives |
|---|---|
| `foo` | `bar` |
| `Foo` | `Bar` |
| `FOO` | `BAR` |
| `fOo` (mixed) | `bar`, as typed |

Words joined by `-` or `_` are matched part by part when the found text and the replacement are joined the same way and have the same number of parts: replacing `foo_bar` with `new_name` turns `FOO_BAR` into `NEW_NAME`, and replacing `foo-bar` with `new-name` turns `Foo-BAR` into `New-NAME`. Preserve Case usually goes with Case sensitive turned off, so that one search finds every spelling. It works in regular expression replacements too, on the whole replacement.

### Keys in the find bar

| Key | In the Find field |
|---|---|
| Enter | Find the next match. |
| Shift+Enter | Find the previous match. |
| Alt+Enter (⌥↩ on macOS) | Find All: put a cursor on every match and close the bar, so you can edit them all at once. |
| Esc | Close the find bar and return to the editor. |

Outside the bar, {{key:find_next}} and {{key:find_prev}} (Find ▸ Find Next and Find Previous) step through matches of the last search. Both wrap around the end of the file. If nothing has been searched yet, Find Next opens the find bar.

### Replace

Open the find bar with its Replace field using {{key:show_replace}} (Find ▸ Replace…). Find ({{key:show_find}}) hides the Replace row again.

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

## Regular expressions

With `.*` turned on, the Find field takes a regular expression. Strata uses a fast regular expression engine that supports the usual syntax (character classes, `\d`, `\w`, `\b`, groups, named groups like `(?P<name>...)`, alternation and repetition) but not look-around or backreferences inside the pattern. `^` and `$` match at the start and end of each line.

In the find bar a pattern can match across lines (for example `\n` matches a line break). Files with Windows line endings (`\r\n`) behave the same as files with Unix ones: `\n` matches the whole `\r\n`, `$` matches before it, `.` doesn't match its `\r`, and a match or replacement never takes half of one. Find in Files searches one line at a time, so its matches never span lines.

### Replacement syntax

The find bar and Replace in Files read the replacement the same way. Refer to captured groups like this:

| Write | To insert |
|---|---|
| `$1`, `$2`… | A numbered group. |
| `${name}` | A named group. |
| `${1}x` | Group 1 followed by `x` (use braces when letters or digits follow the group number). |
| `$$` | A literal dollar sign. |

Backslash forms work too:

| Write | To insert |
|---|---|
| `\1` … `\9` | A numbered group. |
| `\n`, `\t` | A line break or a tab. A line break is the file's own line ending (`\r\n` in a Windows-style file). |
| `\\` | A backslash. |
| `\$` | A dollar sign. |

Without `.*`, the replacement text is inserted exactly as typed.

## Find in Files

Find in Files searches every file in the folders open in the side bar. Open it with {{key:find_in_files}} (Find ▸ Find in Files…, or "Find: Find in Folder…" in the command palette). The side bar switches to its Search tab and puts the cursor in the Find field. A single-line selection in the editor becomes the search text. To search one folder of the tree, right-click it and choose Find in Folder…, which puts that folder in Where.

With no folder open, Find in Files still works: it searches your open files (`<open files>`), and the side bar shows just the Search tab (its × hides it again). If no file is open either, the summary asks you to open a folder or a file. See [Projects and Side Bar](projects.md) for opening folders.

The Search tab has its own `.*`, `Aa` and `"ab"` toggles (separate from the find bar's) and three fields:

| Field | What goes in it |
|---|---|
| Find | The text or regular expression to look for. |
| Where | Which folders and files to search. Leave it empty to search every open folder (or, with none, the open files). |
| Replace | The replacement text, for Replace in Files. |

Press Enter in any of the fields, or click Find, to search. Results stream in while the search runs; a spinner and a Stop button show while it's busy. Find ▸ Cancel Find in Files ({{key:cancel_find_in_files}}) also stops it.

### The Where field

Where is a comma-separated list. Each entry can be:

| Entry | Meaning |
|---|---|
| `src`, `docs/api` | A folder, relative to the open folder (with several, the first one that has it). |
| `/abs/path`, `~/notes` | An absolute folder, or one in your home folder. |
| `src/main.rs` | A single file. |
| `*.rs`, `*.{ts,tsx}` | Include only files matching this pattern. |
| `-*.min.js`, `-target/` | Exclude files or folders matching this pattern (a leading `-`). |
| `<open folders>` | Every folder open in the side bar. `<project>` means the same. |
| `<open files>` | Every file open in a tab. |

How patterns match:

- A pattern without a `/` matches file names, at any depth. `*.rs` finds Rust files in every folder.
- An exclude pattern without a `/` also matches folder names, so `-node_modules` or `-target/` skips those folders entirely. A trailing `/` is ignored.
- A pattern with a `/` matches the path relative to the folder being searched, such as `src/*.rs` or `-tests/fixtures/*`.
- If Where contains only patterns, Strata searches every open folder (or, with none, the open files) with them applied.

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

Results appear in the Search tab, sorted by file path. Each file's path is shown in bold (relative to its folder, after the folder's name when there are several), followed by one row per match with its line number and the line's text, the match highlighted.

Click a match to open the file and select it. If the file is already open in another window, that window comes forward instead.

To step through results from the keyboard, use Find ▸ Find Results ▸ Next Result ({{key:next_find_result}}) and Previous Result ({{key:prev_find_result}}). They wrap around and show "Result 3 of 42" in the status bar; after a new search they start again from its first result. Find ▸ Find Results ▸ Show Panel brings the Search tab back.

## Replace in Files

Replace in Files changes every match of a Find in Files search. Open it with {{key:replace_in_files}} (Find ▸ Replace in Files…). The side bar shows the Search tab; if a selection filled in the Find field, the cursor goes to the Replace field.

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

The file list covers every open folder (skipping `.gitignore`d and hidden files, up to 500,000 files) plus any open files outside it. Without a folder it lists your open files. While a large folder is still being indexed, the panel shows "Indexing…".

Add a suffix to go somewhere inside a file:

| Type | What happens |
|---|---|
| `name` | Open the best-matching file. |
| `:42` | Go to line 42 of the current file. |
| `@name` | Go to a symbol (function, class, heading…) in the current file. |
| `#text` | Search the current file: every line containing the text (ignoring case) is listed, and if none does, the lines that match it fuzzily (its letters in order). Pick one to select the match. |
| `name:42` | Open the file and go to line 42. |
| `name@` | Open the file and show its symbol list. |
| `name@symbol` | Open the file and go to the symbol that best matches `symbol` (fuzzy, like file names). |
| `name#text` | Open the file and select the first match of `text`. |

In the panel, use the Up and Down arrows to choose, Enter or a click to pick, and Esc (or a click outside) to close. While you move through lines, symbols or `#` matches, the editor previews each location; Esc puts your cursor back where it was.

## Go to Line

{{key:goto_line}} (Goto ▸ Goto Line…) opens Goto Anything with `:` already typed. Type a line number and press Enter. Numbers past the end go to the last line.

## Go to Symbol

{{key:goto_symbol}} (Goto ▸ Goto Symbol…) opens Goto Anything with `@` typed, listing the symbols in the current file: functions, classes, structs, types, Markdown headings, CSS selectors and so on. Each row shows the symbol's kind and line number. Type to filter.

Symbols are found by recognizing common declaration patterns line by line, so they cover most code in most languages without a full parser. An occasional symbol may be missed.

### Goto Symbol in Project

{{key:goto_symbol_in_project}} (Goto ▸ Goto Symbol in Project…) lists symbols from every file in the open folders, shown as `name · path:line`. Pick one to open that file at the symbol. It looks at up to 5,000 files with a recognized syntax (each up to 1 MB), skipping `.gitignore`d files. With no folder open, it acts like Go to Symbol. The files are read in the background: the status bar says "Searching the folder for symbols…" and the list opens when it's done; you can keep working meanwhile, and starting another search cancels this one.

## Go to Definition

{{key:goto_definition}} (Goto ▸ Goto Definition…) jumps to where the word under the cursor (or the selected text) is defined. Strata looks for a symbol with exactly that name:

1. in the current file,
2. then in the other open tabs,
3. then in files in the open folders with the same file extension as the current file.

The folder is searched in the background (the status bar says "Searching the folder for …"), so the window never freezes on a big project; a new search cancels one still running. If nothing is found, the status bar says "No definition found for …".

## Go to Reference

{{key:goto_reference}} (Goto ▸ Goto Reference…) searches the open folders for the word under the cursor, as a whole word and case sensitive. The results appear in the side bar's Search tab; step through them with Next Result and Previous Result (see [Find in Files](find.md#find-in-files)). With no folder open, it opens the find bar with the word instead.

## Jump back and forward

Strata remembers where you were before each jump: opening a file from Goto Anything, going to a line or symbol, Go to Definition, clicking a Find in Files result, and so on. It keeps the last 100 places.

| Command | Shortcut |
|---|---|
| Goto ▸ Jump Back | {{key:jump_back}} |
| Goto ▸ Jump Forward | {{key:jump_forward}} |

Places in tabs you've since closed are skipped.

## Other navigation

| Command | Shortcut | What it does |
|---|---|---|
| Goto ▸ Next Modification | {{key:next_modification}} | Jump to the next line changed since the last git commit. See [Git](git.md). |
| Goto ▸ Previous Modification | {{key:prev_modification}} | Jump to the previous changed line. |
| Goto ▸ Switch File ▸ Switch Header/Implementation | {{key:switch_file}} | Switch between a C, C++ or Objective-C header and its source file. |

Switch Header/Implementation looks in the same folder for a file with the same name: from a header (`.h`, `.hh`, `.hpp`, `.hxx`, `.h++`, `.inl`, `.ipp`) to a source file (`.c`, `.cc`, `.cpp`, `.cxx`, `.c++`, `.m`, `.mm`), and back.

## Command palette

The command palette ({{key:command_palette}}, Tools ▸ Command Palette…) runs any command by name. Type part of a name, such as `save all` or `dupl`, and press Enter. It uses the same fuzzy matching as Goto Anything.

Each row shows the command's current shortcut on the right, so the palette is also a handy way to learn keys. Commands are named by area, like "File: Save All", "Find: Replace in Files…" or "Layout: Columns: 2", and "Set Syntax: …" entries switch the current file's language.

For the full list of shortcuts, see [Keyboard Shortcuts](shortcuts.md). To change them, see [Key Bindings](keybindings.md).
