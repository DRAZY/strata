# Welcome to Strata

Strata is a fast, open-source editor for code and text. It opens instantly, handles everything from a quick note to a 500 MB log file, and puts every command a keystroke away: multiple cursors, Goto Anything, project-wide search, split panes, a built-in terminal, Markdown preview and more. Your work is saved as you go, so you can quit at any time and pick up exactly where you left off.

This page walks you through the window, opening and saving files, and the two shortcuts worth learning first. To find out more about Strata itself, see [About Strata](about.md).

## The window at a glance

A Strata window has a few parts. Each one can be shown or hidden.

| Part | What it does | Show or hide |
|------|--------------|--------------|
| Tabs | One tab per open file, along the top of each pane. | View ▸ Toggle Tabs |
| Side bar | The folder you opened, plus open files and search results. | View ▸ Side Bar ▸ Toggle Side Bar ({{key:toggle_side_bar}}) |
| Editor | Where you type. You can split it into several panes. | |
| Minimap | A small overview of the whole file on the right; click or drag it to scroll. | View ▸ Toggle Minimap ({{key:toggle_minimap}}) |
| Status bar | Cursor position, number of cursors and selected characters, Git branch, word count, and menus for syntax, encoding, line endings, indentation and color scheme. Messages from Strata show here too. | View ▸ Toggle Status Bar ({{key:toggle_status_bar}}) |

On macOS, Strata uses the system menu bar at the top of the screen. On Windows and Linux, the menu bar is inside the window. Both have the same menus: File, Edit, Selection, Find, View, Goto, Tools, Project, Window and Help.

Settings live under Strata ▸ Settings on macOS, and under the Preferences menu on Windows and Linux. See [Settings](settings.md) for every option and [Appearance](appearance.md) for color schemes, fonts and the minimap.

## Opening files and folders

There are several ways to open things:

- **File ▸ Open…** ({{key:open_file}}) opens one or more files.
- **File ▸ Open Folder…** ({{key:open_folder}}) shows a folder in the side bar. From there you can open any file in it, and Goto Anything searches it. See [Projects and Side Bar](projects.md).
- **Drag and drop** files onto the window to open them in tabs. Drop a folder to show it in the side bar.
- **File ▸ Open Recent** lists your recently opened files and folders. It also has **Reopen Closed File** ({{key:reopen_last_file}}) and **Clear Items**.
- **From the command line**, you can open files and folders in Strata. See [Command Line](cli.md).

If a file is already open, Strata switches to its tab instead of opening it twice.

## New files

**File ▸ New File** ({{key:new_file}}) opens an empty tab. Until you save it, the tab is named after its first line of text, or "untitled" if it's empty.

**File ▸ New Window** ({{key:new_window}}) opens another window. See [Panes, Windows and Tabs](panes.md).

## Tabs

Click a tab to switch to it, and drag tabs to reorder them or move them to another pane. A dot on a tab means it has unsaved changes. To close a tab, click its close button, middle-click it, or use **File ▸ Close File** ({{key:close_file}}).

To move between tabs, use {{key:next_view}} and {{key:prev_view}}. You can also press ⌘1 through ⌘9 (Alt+1 through Alt+9 on Windows and Linux) to jump to a tab by position; 9 always picks the last tab.

Right-click a tab for more options, such as Close Other Tabs, Close Tabs to the Right, Close Unmodified Tabs and Copy File Path.

When you click a file once in the side bar, it opens as a preview tab that the next preview replaces. Split panes, preview tabs and multiple windows are covered in [Panes, Windows and Tabs](panes.md).

## Saving

| Command | Shortcut | What it does |
|---------|----------|--------------|
| File ▸ Save | {{key:save}} | Saves the current file. A new file asks for a name first. |
| File ▸ Save As… | {{key:save_as}} | Saves the current file under a new name. |
| File ▸ Save All | {{key:save_all}} | Saves every modified file that already has a name. Untitled tabs are skipped. |
| File ▸ Revert File | {{key:revert}} | Throws away unsaved changes and reloads the file from disk. Strata asks first. |

Saves are atomic. Strata writes the new text to a temporary file next to the original, then swaps it into place. If something goes wrong partway, your old file is left intact. File permissions are kept, and saving a symbolic link writes to the file it points to.

Tools ▸ On Save has three options that apply to every save: trim trailing whitespace, make sure the file ends with a newline, and save all files whenever the Strata window loses focus. See [Editing](editing.md#on-save-options).

Strata may ask before saving in a few cases:

- The file was changed by another program since you opened it. Choose **Save Anyway** to replace those changes, or **Cancel**.
- Some bytes couldn't be decoded when the file was opened (they show as �).
- The file's encoding can't hold some of the characters you typed. Strata offers to save it as UTF-8 instead. See [Encodings and Line Endings](encodings.md).

### Files changed on disk

Strata checks the files you're looking at about once a second, and also when you switch to a tab.

- If a file changes on disk and you have no unsaved edits in it, Strata reloads it quietly and says so in the status bar.
- If you do have unsaved edits, Strata never overwrites them. The tab gets an orange dot, and the status bar shows **Changed on disk** with a **Reload** button. Click Reload to discard your edits and load the disk version. If you save instead, Strata asks before replacing the other program's changes.
- If the file is deleted or moved away, the status bar shows **Deleted on disk (save to recreate)**. Your text stays in the tab as unsaved, so it isn't lost.

## Hot exit

Quitting Strata never asks you to save. Everything comes back the next time you start it:

- every window, with its position and size
- the open tabs and the panes they're in
- the text of unsaved files, including untitled ones
- each tab's cursor position, syntax and word wrap choice
- the folder in the side bar

Strata also saves this session every few seconds while you work, so a crash loses at most a few seconds of typing. Files without unsaved changes are read fresh from disk. If a file with unsaved changes was edited by another program while Strata was closed, its tab comes back flagged as changed on disk.

> [!NOTE]
> Hot exit is on by default. To turn it off, set `"hot_exit": false` in your settings. Strata then asks about each unsaved file when you quit, like most editors. If you run a second copy of Strata while one is already open, that second copy also asks before closing unsaved tabs.

## Two keys to remember

You can reach almost everything in Strata from two places.

**The command palette** ({{key:command_palette}}) lists every command, with its shortcut. Start typing part of a name, such as "dup" for Edit: Duplicate Line or "wrap" for View: Toggle Word Wrap, then press Enter. It's also the quickest way to find out what a shortcut is.

**Goto Anything** ({{key:goto_anything}}) finds files by name in your open folder and your open tabs. Type a few letters of the file name; they don't need to be next to each other. You can add more:

- `:42` goes to line 42 of the current file. `name:42` opens a file at line 42.
- `@name` jumps to a symbol (a function, class, heading and so on) in the current file. `file@name` does the same in another file.

See [Find and Replace](find.md) for search, symbols and Go to Definition.

## Where to go next

- [Editing](editing.md): multiple cursors, line commands, comments, case conversion, folding and more.
- [Snippets](snippets.md): insert templates by typing a short trigger and pressing Tab.
- [Find and Replace](find.md): find, replace, Find in Files, Goto Anything and symbols.
- [Projects and Side Bar](projects.md): the side bar, project files and recent projects.
- [Panes, Windows and Tabs](panes.md): split layouts, groups, preview tabs and multiple windows.
- [Comparing Files](compare.md) and [Printing and Exporting](printing.md).
- [Terminal](terminal.md), [Build Systems](build.md) and [Git Integration](git.md).
- [Markdown Preview](markdown.md): live preview and HTML export.
- [Appearance](appearance.md) and [Settings](settings.md): make Strata look and behave the way you like.
- [Keyboard Shortcuts](shortcuts.md) and [Customizing Key Bindings](keybindings.md).
- [Languages](languages.md) and [Encodings and Line Endings](encodings.md).
- [Troubleshooting and FAQ](troubleshooting.md): where Strata keeps its files, crash reports, and common questions.
- [About Strata](about.md): what Strata is, its license and credits.

## Getting help

You can open this manual at any time:

- **Help ▸ Documentation** ({{key:documentation}}) opens it. In the command palette, it's **Help: Documentation**.
- **Help ▸ Keyboard Shortcuts** shows every shortcut as it's currently set up, including your own changes. You can also open it [here](shortcuts.md).

### Using the manual

| To | Do this |
|---|---|
| Search every page | Type in the search field at the top ({{key:show_find}} puts the cursor there). Press Enter to open the best match. |
| Go back or forward | Click ← or →, or press Alt+← and Alt+→. |
| Read along while you work | Click **Pop Out** to move the manual into its own window, or **Dock** to put it back inside the editor window. |
| Close it | Press Esc (first clearing the search, if there is one). |

The manual opens in its own window by default. To keep it inside the editor window instead, click **Dock** once, or set `"help_in_own_window": false` in your [settings](settings.md#window).
