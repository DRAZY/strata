# Projects and Sidebar

Open a folder and Strata shows it in the sidebar, where you can browse files, create and rename them, and search the whole folder. A project file remembers a folder together with the files and layout you had open.

## Opening a Folder

Each window shows one folder in its sidebar. There are several ways to open one:

- **File ▸ Open Folder…** ({{key:open_folder}}) picks a folder with the system dialog. Project ▸ Add Folder to Project… does the same.
- **Drag a folder** from your file manager onto the Strata window. (Dropped files open in tabs.)
- **From the command line**, pass a folder: `strata ~/code/myapp`. See [Command Line](cli.md).
- **File ▸ Open Recent** and **Project ▸ Open Recent** list folders you've opened before, shown with a trailing `/`.

Opening a folder replaces the one that was in the sidebar; your open tabs stay where they are. The folder's name also appears in the window title.

## The Sidebar

The sidebar sits on the left of the window and only appears while a folder is open. Show or hide it with {{key:toggle_side_bar}} (View ▸ Side Bar ▸ Toggle Side Bar). Drag its right edge to make it wider or narrower.

At the top are two tabs:

| Tab | What it shows |
|---|---|
| Files | The Open Files list and the folder tree. |
| Search | Find in Files and Replace in Files. See [Find in Files](find.md#find-in-files). |

### The folder tree

The tree lists folders first, then files, each sorted by name without regard to case. Click a folder's arrow or name to expand or collapse it. The file in the current tab is highlighted.

| Do this | To |
|---|---|
| Click a file | Open it as a **preview**: an italic tab that the next previewed file replaces. Handy for looking through files without piling up tabs. |
| Double-click a file | Open it in a regular tab. |
| Right-click a file or folder | Show its menu (see below). |

A preview tab becomes a regular tab as soon as you edit it, save it, double-click its tab, or open the file again any other way. See [Panes, Windows and Tabs](panes.md#preview-tabs).

The tree shows every file and folder except `.git` folders and `.DS_Store` files. Unlike Find in Files and Goto Anything, it doesn't hide `.gitignore`d or hidden files.

### Refreshing

Strata reads each folder's contents when you first expand it and doesn't watch for changes made outside the app. To pick up new, renamed or deleted files, click the ⟳ button at the top right of the Files tab, or choose Project ▸ Refresh Folders ({{key:refresh_folder_list}}). Files you create or rename from the sidebar menu show up right away.

### Right-click menu

Right-click a file, a folder, or the top folder's name:

| Item | What it does |
|---|---|
| New File… | Ask for a name and create an empty file in that folder (the clicked file's folder, for a file), then open it. A name like `src/util.rs` also creates the missing folders. |
| New Folder… | Ask for a name and create the folder. Nested names like `a/b/c` work. |
| Rename… | Rename the file or folder. Open tabs of renamed files, or of files inside a renamed folder, follow the new name. Not offered for the top folder. |
| Copy Path | Copy the full path to the clipboard. |
| Reveal in Finder | Show the item in your file manager. On Windows it's called Show in Explorer, and on Linux Open Containing Folder. |
| Find in Folder… | Switch to the Search tab, ready for a Find in Files search. |

In the name dialog, press Enter to confirm or Esc to cancel. Strata won't overwrite an existing file or folder; the dialog shows the error instead.

> [!NOTE]
> Find in Folder… doesn't fill in the Where field. To search only that folder, type its path (relative to the open folder) in Where.

There's no Delete item: delete files in your file manager or the [terminal](terminal.md), then refresh.

### Open Files

When more than one tab is open, an **OPEN FILES** section above the tree lists them. Click a name to switch to that tab, or the × to close it. A ● before the name marks unsaved changes. Click the section's heading to collapse it.

Turn the section on or off with View ▸ Side Bar ▸ Toggle Show Open Files ({{key:toggle_open_files}}).

### Revealing the current file

To find the current file in the tree, right-click its tab (or the text in the editor) and choose **Reveal in Side Bar**. Strata opens the folders above it and scrolls it into view. The item is available when the file is inside the open folder.

### Closing the folder

Project ▸ Remove all Folders from Project ("Project: Close Folder" in the Command Palette, {{key:close_folder}}) closes the folder and hides the sidebar. Your tabs stay open.

## Projects

A project is a small `.strata-project` file that records a folder, the files you had open, and your pane layout, so you can come back to all of it in one step.

| Command | Shortcut | What it does |
|---|---|---|
| Project ▸ Open Project… | {{key:open_project}} | Pick a `.strata-project` file and open it. |
| Project ▸ Switch Project… | {{key:switch_project}} | Choose from your recent projects in a quick list. If you have none yet, it works like Open Project. |
| Project ▸ Open Recent | | Recent projects, then recent folders. |
| Project ▸ Save Project As… | {{key:save_project_as}} | Save the current folder, open files and layout as a project. |
| Project ▸ Close Project | {{key:close_project}} | Close the project's folder and forget the current project. Tabs stay open. |
| Project ▸ Edit Project | {{key:edit_project}} | Open the current project file in a tab to edit it by hand. |

Save Project As suggests a name based on the folder, such as `myapp.strata-project`, saved in the folder itself. Edit Project is available once you've opened or saved a project in this session.

### Opening a project

When you open a project, Strata:

1. opens the project's folder in the sidebar,
2. switches to the saved pane layout,
3. opens each listed file that still exists, in the current pane.

Tabs you already had open stay open, and the project joins the top of your recent projects.

### The project file

A `.strata-project` file is plain JSON. It must be strict JSON: no comments and no trailing commas. Every field is optional.

```json
{
  "folders": [
    "myapp"
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
| `folders` | The folder to show in the sidebar. Only the first entry is used. |
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

**Clear Items** ({{key:clear_recent_files}}, "File: Clear Recent Items") empties the recent files, the recent folders and the list of closed tabs. Recent projects stay listed.

Files opened as previews don't count as recently opened. The recent lists are shared by every window and kept between sessions.
