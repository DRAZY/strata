# Panes, Windows and Tabs

Strata can show several files side by side in panes, keep many files open as tabs, and spread your work across several windows. This page also covers full screen, distraction-free mode, and showing or hiding parts of the window.

## Tabs

Every open file has a tab. Each pane has its own row of tabs, and the current tab in the focused pane has an accent line along its top. Hover over a tab to see the file's full path. When two open files share a name, their tabs add the folder name, like `mod.rs — parser`.

### Opening and closing tabs

| Command | Shortcut | What it does |
|---|---|---|
| File ▸ New File | {{key:new_file}} | Open an empty, untitled tab. Double-clicking the empty part of a tab bar does the same in that pane. |
| File ▸ Open… | {{key:open_file}} | Open one or more files. |
| File ▸ Close File | {{key:close_file}} | Close the current tab. |
| File ▸ Close All Files | {{key:close_all}} | Close every tab in the window. |
| Reopen Closed File | {{key:reopen_last_file}} | Reopen the tab you closed most recently. It's in File ▸ Open Recent; press it again to go further back. |

You can also close a tab by clicking its ×, or with a middle click. If the file has unsaved changes, Strata asks whether to save it first. New tabs open in the focused pane, just after its current tab. Opening a file that's already open switches to its tab instead (or brings forward the window that has it).

Closing the last tab in a window leaves a fresh untitled tab. With several panes, closing a pane's last tab leaves that pane empty.

### Switching tabs

| Command | Shortcut | What it does |
|---|---|---|
| View: Next Tab | {{key:next_view}} | The next tab in the current pane, wrapping around. |
| View: Previous Tab | {{key:prev_view}} | The previous tab in the current pane. |
| Goto ▸ Switch File ▸ Next File in Stack | {{key:next_view_in_stack}} | The tab you used before this one. |
| Goto ▸ Switch File ▸ Previous File in Stack | {{key:prev_view_in_stack}} | Walk the same list the other way. |

The "stack" orders every tab in the window by when you last used it. Press {{key:next_view_in_stack}} once to flip between your two most recent files. Press it again within about a second and a half to keep walking further back; after a pause, the next press starts from the top again.

To jump straight to a tab by position in the current pane, press ⌘1 through ⌘9 on macOS, or Alt+1 through Alt+9 on Windows and Linux. 9 always goes to the last tab. The Window menu also lists every tab in the window.

When a pane has more tabs than fit, its tab bar scrolls: the current tab is always scrolled into view, and the mouse wheel (or a two-finger swipe) over the tab bar scrolls it sideways. A ⌄ button at the right end then lists all of the pane's tabs; pick one to switch to it.

### Rearranging tabs

Drag a tab along the tab bar to reorder it. Drag it onto a tab in another pane to move it there, or drop it on another pane's text area to move it to the end of that pane's tabs.

### Tab markers

| Marker | Meaning |
|---|---|
| A dot where the × would be | The file has unsaved changes. Point at the dot to get the × back. |
| A small orange dot at the left edge | Conflict: the file changed on disk while you had unsaved changes. Strata keeps your text and doesn't reload it. |
| An italic title | A preview tab (see below). |

### Preview tabs

Clicking a file once in the side bar opens it as a preview, shown with an italic title. The next file you preview in that pane replaces it, so you can look through files without collecting tabs.

A preview tab becomes a regular tab when you:

- edit it,
- save it,
- double-click its tab,
- double-click the file in the side bar, or open it any other way (File ▸ Open…, Goto Anything, a Find in Files result).

### Tab right-click menu

Right-click a tab for:

| Item | What it does |
|---|---|
| Close | Close this tab. |
| Close Other Tabs | Close every other tab in this pane. |
| Close Tabs to the Right | Close the tabs after this one in this pane. |
| Close Unmodified Tabs | Close the tabs in this pane that have no unsaved changes. |
| Move to New Window | Move this tab into a new window of its own. Available when the window has more than one tab. |
| Split View | Open another view of this file in the next pane, adding a second column if there's only one pane (see below). |
| Compare with… | Compare this file side by side with another tab, a file, or its Git `HEAD` version. See [Comparing Files](compare.md). |
| New File | Open an untitled tab in this pane. |
| Open File… | Open files into this pane. |
| Copy File Path | Copy the file's full path. |
| Copy Relative Path | Copy the file's path inside the side bar folder that holds it, such as `src/main.rs`. Available when the file is in an open folder. |
| Copy Permalink | Copy a GitHub, GitLab or Bitbucket link to the file at the current commit, at the cursor's line or the selected lines. Available when the file is in a Git repository with such a remote. See [Git Integration](git.md#copy-permalink). |
| Reveal in Side Bar | Show the file in the side bar's folder tree. Available when the file is inside the open folder. |
| Reveal in Finder | Show the file in your file manager (Show in Explorer on Windows, Open Containing Folder on Linux). |

When closing several tabs, Strata asks about each one with unsaved changes and stops if you choose Cancel.

## Several views of one file

A file can be open in more than one tab at a time, for example to look at the top of a long file while you edit the bottom, or to keep two distant parts of it side by side.

| Command | Shortcut | What it does |
|---|---|---|
| File ▸ New View into File | {{key:clone_file}} | Open another tab on the current file, in the same pane. |
| Split View | | From a tab's right-click menu: open another view in the next pane, adding a second column if there's only one pane. |

Views of one file share its text: an edit in one shows up in the others at once, and their cursors, selections, bookmarks and folds move along with the text. Each view keeps its own cursors and selections, scroll position, folds and word wrap.

They are one file in every other way too:

- There's one undo history. Undo and redo work from any view, and step back through edits made in all of them, newest first.
- Saving any view saves the file, and every view shows the same unsaved dot. Save As, Reopen with Encoding, line endings and Revert apply to every view.
- Closing a view never asks about unsaved changes while another view of the file is still open; the last one asks as usual.
- Opening the file again (from the side bar, Goto Anything or a search result) uses the view you're in, or the one you used last.
- Find in Files and Replace in Files count and change the file once.
- With hot exit on, every view comes back next time, with its own cursors and pane, and unsaved text is kept once.

Moving a view to another window with Move to New Window gives it its own copy of the text there: views stay in step only within a window.

## Layouts

Split the editor area into panes with View ▸ Layout. Each pane, also called a group, has its own tabs.

| Layout | Shortcut | Panes |
|---|---|---|
| Single | {{key:layout_single}} | One pane. |
| Columns: 2 | {{key:layout_columns_2}} | Two side by side. |
| Columns: 3 | {{key:layout_columns_3}} | Three side by side. |
| Columns: 4 | {{key:layout_columns_4}} | Four side by side. |
| Rows: 2 | {{key:layout_rows_2}} | Two stacked. |
| Rows: 3 | {{key:layout_rows_3}} | Three stacked. |
| Grid: 4 | {{key:layout_grid_4}} | Two by two. |

Panes are numbered left to right, then top to bottom. When you switch to a layout with fewer panes, the tabs from the panes that go away move into the last remaining pane.

**Split Right** ({{key:split_right}}, "Layout: Split Right" in the command palette) moves the current file into the pane to its right, adding a column if there isn't one.

Drag the line between two panes to resize them. Each pane keeps at least a small minimum size. The focused pane is marked with a thin accent line along its top, and an empty pane shows a hint until you drag a tab into it. Click a pane to focus it.

## Groups

The View menu's Groups, Focus Group and Move File to Group submenus manage panes directly.

| Command | Shortcut | What it does |
|---|---|---|
| View ▸ Groups ▸ New Group | {{key:new_group}} | Add a pane: another column, or another row if you're in a rows layout. Up to 4. |
| View ▸ Groups ▸ Close Group | {{key:close_group}} | Remove the focused pane. Its tabs move into the pane before it. |
| View ▸ Groups ▸ Move File to New Group | {{key:move_to_new_group}} | Add a pane and move the current file into it. |
| View ▸ Focus Group ▸ Next | {{key:focus_next_group}} | Focus the next pane, wrapping around. |
| View ▸ Focus Group ▸ Previous | {{key:focus_prev_group}} | Focus the previous pane. |
| View ▸ Move File to Group ▸ Next | {{key:move_to_next_group}} | Move the current file to the next pane. |
| View ▸ Move File to Group ▸ Previous | {{key:move_to_prev_group}} | Move the current file to the previous pane. |

To focus pane 1 to 4 directly, press Ctrl+1 to Ctrl+4 (⌃1 to ⌃4 on macOS), or use View ▸ Focus Group ▸ Group 1–4. To move the current file to pane 1 to 4, press Ctrl+Shift+1 to Ctrl+Shift+4 (⌃⇧1 to ⌃⇧4 on macOS), or use View ▸ Move File to Group ▸ Group 1–4. Moving a file to a pane that doesn't exist yet switches to a layout with that many columns.

With hot exit on (see below), your layout, the pane sizes and which pane each tab is in are saved with the window and restored next time.

## Multiple windows

Strata can have several windows open at once, each with its own tabs.

| Command | Shortcut | What it does |
|---|---|---|
| File ▸ New Window | {{key:new_window}} | Open a new, empty window. |
| File ▸ Close Window | {{key:close_window}} | Close the current window. |
| Move to New Window | | From a tab's right-click menu: move that tab into a new window. |

From the command line, `strata -n` opens files in a new window. See [Command Line](cli.md).

### What each window has

Each window has its own tabs, panes and layout, side bar folder, find bar, command palette, terminal panel and builds.

These are shared by every window:

- settings (a change made in one window applies to all of them at once),
- recent files, folders and projects,
- the clipboard history used by Edit ▸ Paste from History,
- the saved session.

A file is open in only one window at a time. Opening a file that another window already has, from any window, brings that window forward with the file's tab selected.

### The Window menu

The Window menu lists the tabs in the current window (pick one to switch to it) and, when there's more than one window, every window by title so you can bring one forward. On macOS it also has the standard Minimize, Zoom and Bring All to Front items, and the menu bar always belongs to the window you're working in.

### Closing windows and quitting

Closing a window that isn't the last one asks about each of its tabs with unsaved changes, then closes it.

Closing the last window quits Strata, on every platform, including macOS. Quitting (Strata ▸ Quit Strata on macOS, File ▸ Exit on Windows and Linux) works like this:

- With **hot exit** on (the default), Strata saves every window, its tabs and any unsaved text, without asking. Next time you start Strata, all of your windows come back where they were.
- With hot exit off, Strata asks about each unsaved tab, and quitting stops if you choose Cancel.

On macOS, Strata ▸ Quit and Keep Windows (⌥⌘Q) does the same as Quit. Hot exit is controlled by the `hot_exit` setting; see [Settings](settings.md).

## Full screen and distraction-free mode

| Command | Shortcut | What it does |
|---|---|---|
| View ▸ Toggle Full Screen | {{key:toggle_full_screen}} | Fill the screen with the window. |
| View ▸ Toggle Distraction Free | {{key:toggle_distraction_free}} | Full screen with only the text. |

Distraction-free mode hides the side bar, tabs, status bar, minimap and build results, plus the menu bar on Windows and Linux. Use the same shortcut, or the command palette's "View: Toggle Distraction Free Mode", to come back; the side bar and minimap return as they were.

## Showing and hiding parts of the window

| Command | Shortcut | What it does |
|---|---|---|
| View ▸ Side Bar ▸ Toggle Side Bar | {{key:toggle_side_bar}} | Show or hide the side bar. |
| View ▸ Side Bar ▸ Toggle Show Open Files | {{key:toggle_open_files}} | Show or hide the Open Files list in the side bar. |
| View ▸ Toggle Tabs | {{key:toggle_tabs}} | Show or hide the tab bars. |
| View ▸ Toggle Status Bar | {{key:toggle_status_bar}} | Show or hide the status bar. |
| View ▸ Toggle Minimap | {{key:toggle_minimap}} | Show or hide the minimap. |

Tabs, status bar and minimap are settings (`show_tabs`, `show_status_bar` and `minimap`), so they apply to every window and are remembered. The side bar is shown or hidden per window. For the side bar itself, see [Projects and Side Bar](projects.md).
