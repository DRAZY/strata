# Projects and Side Bar

Open a folder and Strata shows it in the side bar, where you can browse files, create and rename them, and search the whole folder. A window can show several folders at once, and a project file remembers the folders together with the files and layout you had open.

## Opening a folder

There are several ways to open a folder:

- **File ▸ Open Folder…** ({{key:open_folder}}) picks a folder with the system dialog. It replaces the folders in the side bar.
- **Project ▸ Add Folder to Project…** ({{key:prompt_add_folder}}) picks a folder and adds it below the ones already there, so one window can show several folders (an app and a library it uses, say).
- **Drag a folder** from your file manager onto the Strata window. (Dropped files open in tabs.)
- **From the command line**, pass a folder: `strata ~/code/myapp`. See [Command Line](cli.md).
- **File ▸ Open Recent** and **Project ▸ Open Recent** list folders you've opened before, shown with a trailing `/`.

Opening a folder replaces the folders that were in the side bar; your open tabs stay where they are. The first folder's name also appears in the window title, followed by "+1", "+2"… when there are more.

Every folder in the side bar counts everywhere a folder does: Find in Files and Goto Anything search all of them, Goto Symbol in Project and Goto Definition look through all of them, and hot exit reopens all of them next time. Build systems use the folder holding the current file (else the first one) for `$folder`, and a new terminal starts in the folder holding the current file.

## The side bar

The side bar sits on the left of the window. It appears while a folder is open, and also for Find in Files when no folder is open. Show or hide it with {{key:toggle_side_bar}} (View ▸ Side Bar ▸ Toggle Side Bar). Drag its right edge to make it wider or narrower.

At the top are two tabs:

| Tab | What it shows |
|---|---|
| Files | The Open Files list and a tree for each folder. |
| Search | Find in Files and Replace in Files. See [Find in Files](find.md#find-in-files). |

### The folder tree

Each folder in the side bar is a tree of its own, in the order you added them; hover over a folder's name to see its full path. The tree lists folders first, then files, each sorted by name without regard to case. Click a folder's arrow or name to expand or collapse it. The file in the current tab is highlighted.

| Do this | To |
|---|---|
| Click a file | Open it as a **preview**: an italic tab that the next previewed file replaces. Handy for looking through files without piling up tabs. |
| Double-click a file | Open it in a regular tab. |
| Right-click a file or folder | Show its menu (see below). |

A preview tab becomes a regular tab as soon as you edit it, save it, double-click its tab, or open the file again any other way. See [Panes, Windows and Tabs](panes.md#preview-tabs).

### What the tree shows

Like Goto Anything and Find in Files, the tree leaves out what `.gitignore` ignores (build output such as `target/` or `node_modules/`, say), along with `.git` folders and `.DS_Store` files. Hidden files (names starting with a dot) are shown. Two settings change this:

- `sidebar_respect_gitignore` (default `true`): set it to `false` to see ignored files too.
- `file_exclude_patterns` (default `[".git", ".DS_Store"]`): names, or globs like `*.pyc`, that are never shown.

See [Settings](settings.md#window).

### Refreshing

The tree keeps itself up to date: every couple of seconds Strata checks the folders you have expanded (only those, so a huge project costs nothing extra) and re-reads any whose contents changed, and it re-reads the tree whenever you come back to the window from another app. So files that a terminal, a build or Git creates, renames or deletes show up on their own. To re-read everything right away, click the ⟳ button at the top right of the Files tab, or choose Project ▸ Refresh Folders ({{key:refresh_folder_list}}).

### Right-click menu

Right-click a file, a folder, or a top folder's name:

| Item | What it does |
|---|---|
| New File… | Ask for a name and create an empty file in that folder (the clicked file's folder, for a file), then open it. A name like `src/util.rs` also creates the missing folders. |
| New Folder… | Ask for a name and create the folder. Nested names like `a/b/c` work. |
| Rename… | Rename the file or folder. Open tabs of renamed files, or of files inside a renamed folder, follow the new name, in every window. Changing only the case of a name (`readme.md` to `README.md`) works too, also on disks that ignore case. Renaming a Markdown note also offers to update the links to it (see [Renaming notes](notes.md#renaming-notes)). Not offered for top folders. |
| Move to Trash… | After asking, move the file or folder (and everything in it) to the Trash (the Recycle Bin on Windows), where you can put it back. Its tabs close, in every window; a tab with unsaved changes stays open, marked "Deleted on disk", so nothing you typed is lost. Not offered for top folders. |
| Copy Path | Copy the full path to the clipboard. |
| Reveal in Finder | Show the item in your file manager. On Windows it's called Show in Explorer, and on Linux Open Containing Folder. |
| Find in Folder… | Switch to the Search tab with that folder (a file's own folder, for a file) in Where, so the search covers just it. |
| Remove Folder from Project | On a top folder only: take it out of the side bar. Nothing on disk changes. |

In the name dialog, press Enter to confirm or Esc to cancel. While a dialog is open, other shortcuts wait. Strata won't overwrite an existing file or folder, and it turns down names that can't work: an empty name, `.` or `..`, a `\` (use `/` between folders in New File and New Folder; a new name in Rename can't contain `/` at all), and on Windows the characters `< > : " | ? *`, names ending in a dot or space, and reserved names like `CON` or `NUL`. The dialog says what's wrong instead.

### Open Files

When more than one tab is open, an **OPEN FILES** section above the tree lists them. Click a name to switch to that tab, or the × to close it. A ● before the name marks unsaved changes. Click the section's heading to collapse it.

Turn the section on or off with View ▸ Side Bar ▸ Toggle Show Open Files ({{key:toggle_open_files}}).

### Revealing the current file

To find the current file in the tree, right-click its tab (or the text in the editor) and choose **Reveal in Side Bar**. Strata opens the folders above it and scrolls it into view. The item is available when the file is inside one of the side bar's folders.

### Closing folders

To take one folder out, right-click its name and choose Remove Folder from Project. Project ▸ Remove all Folders from Project ("Project: Remove All Folders from Project" in the command palette, {{key:close_folder}}) closes every folder and hides the side bar. Your tabs stay open.

## Projects

A project is a small `.strata-project` file that records the side bar's folders, the files you had open, and your pane layout, so you can come back to all of it in one step.

| Command | Shortcut | What it does |
|---|---|---|
| Project ▸ Open Project… | {{key:open_project}} | Pick a `.strata-project` file and open it. |
| Project ▸ Switch Project… | {{key:switch_project}} | Choose from your recent projects in a quick list. If you have none yet, it works like Open Project. |
| Project ▸ Open Recent | | Recent projects, then recent folders. |
| Project ▸ Save Project As… | {{key:save_project_as}} | Save the current folders, open files and layout as a project. |
| Project ▸ Close Project | {{key:close_project}} | Close the project's folders and forget the current project. Tabs stay open. |
| Project ▸ Edit Project | {{key:edit_project}} | Open the current project file in a tab to edit it by hand. |

Save Project As suggests a name based on the first folder, such as `myapp.strata-project`, saved in the folder itself. Edit Project is available once you've opened or saved a project in this session.

### Opening a project

When you open a project, Strata:

1. shows the project's folders in the side bar, in place of the ones there before,
2. switches to the saved pane layout,
3. opens each listed file that still exists, in the current pane.

Tabs you already had open stay open, and the project joins the top of your recent projects.

### The project file

A `.strata-project` file is plain JSON. It must be strict JSON: no comments and no trailing commas. Every field is optional.

```json
{
  "folders": [
    "myapp",
    "../shared-lib"
  ],
  "files": [
    "myapp/src/main.rs",
    "myapp/README.md"
  ],
  "layout": {
    "cols": [0.6, 0.4],
    "rows": [1.0]
  }
}
```

| Field | Meaning |
|---|---|
| `folders` | The folders to show in the side bar, in order. |
| `files` | Files to open, in order. Missing files are skipped. |
| `layout` | The pane layout: `cols` are column widths and `rows` are row heights, each a list of fractions. Up to 4 of each; Strata scales them to add up to 1. |

Relative paths are relative to the folder containing the project file, and absolute paths work too. When Strata saves a project, paths inside the project file's folder are written relative to it and others are written in full. If the project file sits inside the folder it opens, that folder is written as `""`, meaning "this file's own folder".

The project records the layout but not which pane each file was in. Changes you make to the file with Edit Project apply the next time you open the project.

## Open Recent

File ▸ Open Recent lists:

- **Reopen Closed File** ({{key:reopen_last_file}}), which reopens the tab you closed most recently,
- up to 15 recently opened files,
- up to 15 recently opened folders (with a trailing `/`),
- **Clear Items**.

Project ▸ Open Recent lists up to 15 recent projects, then your recent folders, then Clear Items.

**Clear Items** ({{key:clear_recent_files}}, "File: Clear Recent Items") empties the recent files, the recent folders, the recent projects and the list of closed tabs. It does the same from either Open Recent menu.

Files opened as previews don't count as recently opened. The recent lists are shared by every window and kept between sessions.

On macOS, right-click (or Control-click) Strata's Dock icon for **New Window** and the ten most recent files and folders.
