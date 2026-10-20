# Notes

Strata works with notes the way Obsidian does: plain Markdown files in a folder, linked with `[[wikilinks]]`, tagged with `#tags`, with backlinks, daily notes and templates. There is no database and no format of Strata's own, so you can keep the same folder open in Strata and in Obsidian at once, and sync it with iCloud, Dropbox, Syncthing or Git. Strata connects to the network for notes only through [Git sync](#git-sync), which is off until you turn it on for a vault.

## Opening your notes

Open the folder that holds your notes with **File ▸ Open Folder…** ({{key:open_folder}}). That folder is your vault: links are resolved within it, and the notes in it are indexed in the background so links, backlinks and tags work everywhere.

- **Any folder with Markdown files works.** A folder containing `.obsidian` is an Obsidian vault, and Strata follows the settings of Obsidian's that affect links and new notes (see [Obsidian settings Strata follows](#obsidian-settings-strata-follows)). Strata only reads `.obsidian`; it never changes it.
- **A vault inside a bigger folder** (say your home folder is open and the vault is one folder in it) is found by its `.obsidian` folder: links in its notes stay within the vault.
- **A note opened on its own**, outside the side bar's folders, is part of a vault when a folder above it has `.obsidian`.

The index covers every file in the vault except folders whose names start with a dot (`.obsidian`, `.trash`, `.git`), as in Obsidian. In a folder that isn't an Obsidian vault, files your `.gitignore` ignores are left out too, so a code project's `node_modules` isn't read. Indexing happens in the background and never holds up typing; a vault of 10,000 notes takes a second or two the first time. After that, Strata checks every few seconds (and whenever you come back to the window) for notes changed by Obsidian, a sync tool or Git, and reads only those again.

## Links

Write a link to another note with double square brackets. You don't need the `.md`, the folder, or matching capitals:

| You write | It links to |
|---|---|
| `[[Meeting notes]]` | The note `Meeting notes.md`, wherever it is in the vault. |
| `[[Projects/Plan]]` | `Plan.md` in a folder called `Projects`. |
| `[[Plan#Budget]]` | The heading "Budget" in `Plan.md`. `[[Plan#Budget#Q3]]` goes to the "Q3" heading under it. |
| `[[Plan#^task-1]]` | The paragraph in `Plan.md` that ends with the block id `^task-1`. |
| `[[#Budget]]` | A heading in the same note. |
| `[[Plan\|the plan]]` | `Plan.md`, shown as "the plan" in the preview. |
| `![[diagram.png]]` | An embed: the image, or the note, appears in place in the preview. |
| `[Plan](Projects/Plan.md)` | Ordinary Markdown links to notes work too. Spaces are written `%20`, or put the path in angle brackets: `[Plan](<My Plan.md>)`. |

Inside a Markdown table, the bar of a link with display text needs a backslash before it, as in Obsidian: `[[Plan\|the plan]]`.

Links inside code (between backticks, or in a fenced code block) aren't links, as in Obsidian.

In the editor, links are underlined: a solid line for a note that exists, a dashed one for a note that doesn't exist yet. Tags sit on a light tint.

### Following a link

Hold ⌘ (Ctrl on Windows and Linux) and click a link to open the note, at its heading or block if the link names one. While you hold the key, the pointer turns into a hand over links. With the cursor on a link, **Notes: Follow Link** does the same from the keyboard (it has no shortcut; bind `follow_link` in [Key Bindings](keybindings.md) if you like). {{key:jump_back}} takes you back.

Elsewhere, ⌘-click (Ctrl+click) still adds a cursor; only a click on a link follows it.

Links to images open them in Strata's image preview. Links to other files that aren't text (a PDF, say) show the file in Finder (Explorer on Windows) instead of opening it.

### Links to notes that don't exist

Following a link to a missing note asks whether to create it. The new note is empty and goes where Obsidian would put it: in the vault folder, in the current note's folder, or in a folder you chose, as set in Obsidian's **Default location for new notes**. Without Obsidian's settings it goes in the vault folder. A link with a path in it (`[[Ideas/New idea]]`) creates the note at that path. Strata never replaces an existing file.

### How a link finds its note

When a link could mean more than one file, Strata decides the same way every time:

1. The path from the linking note's own folder (so `[[README]]` in `Projects/` finds `Projects/README.md`).
2. The path from the vault folder.
3. Any file whose path ends with the link, ignoring case: the one in the linking note's folder first, then the one with the fewest folders above it, then the first by name.

`.md` is optional for notes; other files need their extension (`[[photo.png]]`).

## Link completion

Type `[[` and a list of notes appears under the cursor. Keep typing to narrow it: the letters of a name in order are enough. Use ↑ and ↓ to choose, Enter or Tab to insert, Esc to close the list.

- Notes with `aliases` in their frontmatter are listed under each alias too; picking one writes `[[Note|alias]]`.
- After a note's name, type `#` to choose one of its headings, or `#^` for its block ids. `[[#` lists the headings of the note you're in.
- Images and other files are listed once you type something.
- The link is written in the shortest form that finds the note: its name when that's unique in the vault, otherwise its path. In an Obsidian vault set to **Use [[Wikilinks]]** off, Strata writes a Markdown link instead, and it follows Obsidian's **New link format** (shortest, relative or absolute).

## Backlinks

Under a note, the backlinks panel lists the other notes that link to it, each with the line that holds the link. Click a note's name to open it, or a line to open it there. The panel also shows the note's tags; click one to see the notes with that tag.

The panel shows for notes in an Obsidian vault, and in any folder whose notes use wikilinks. Hide it with the × at its right or **Notes: Toggle Backlinks** (also Tools ▸ Notes ▸ Backlinks), which shows it again. Drag its top edge to resize it. Backlinks come from the notes as they are on disk, so a link you've typed but not saved yet shows up once it's saved.

## Tags

A tag is `#` followed by letters, digits, `_`, `-` or `/`, at the start of a line or after a space: `#idea`, `#project/strata`, `#2026-goals`. These are not tags, as in Obsidian: a number alone (`#1984`), a `#` in the middle of a word (`issue#12`), headings (`# Title`), and anything in code or in a link. Tags in the frontmatter count too:

```yaml
---
tags: [idea, project/strata]
---
```

Tags don't depend on case: `#Idea` and `#idea` are one tag. **Notes: Find Tag…** lists every tag in the vault with how many notes use it; choose one to see its notes and open one. Clicking a tag in the preview or the backlinks panel does the same.

## Daily notes

**Notes: Open Daily Note** opens today's note, creating it if it doesn't exist. In an Obsidian vault whose Daily notes plugin has settings, Strata uses its folder, date format and template. Otherwise today's note is `Journal/2026-10-19.md` (the date in the `YYYY-MM-DD` format). The date format can hold slashes to make folders, such as `YYYY/MM/YYYY-MM-DD`, and is written the way Obsidian writes it (moment.js formats: `YYYY`, `MM`, `DD`, `ddd`, `dddd`, `MMMM`, `Do`, `W` and so on).

A new daily note starts from the template, if one is set, with its `{{date}}`, `{{time}}` and `{{title}}` filled in (see below).

## Templates

**Notes: New Note from Template…** lists the notes in your templates folder: the one set in Obsidian's Templates plugin, or else a folder called `Templates` in the vault. Choose one, then type a name for the new note. It's created where new notes go, with these filled in:

| In the template | Becomes |
|---|---|
| `{{title}}` | The new note's name. |
| `{{date}}` | Today's date, in the Templates plugin's date format (`YYYY-MM-DD` without one). |
| `{{time}}` | The time, in its time format (`HH:mm` without one). |
| `{{date:dddd, MMMM Do}}`, `{{time:h:mm A}}` | The date or time in the format you give. |

**Notes: New Note…** makes an empty note the same way.

## Renaming notes

Rename a note with **Notes: Rename Note…**, or with Rename… in the side bar's right-click menu. Strata then looks for links to it and shows every line that would change, before and after. Choose **Update Links** to rewrite them, or **Don't Update** to leave them.

- Only real links change: `[[Old]]`, `[[Old#heading|alias]]`, `![[Old]]` and Markdown links to the note keep their heading, alias and style. Text that merely mentions the name, and links in code, are left alone.
- A bare name stays a bare name when it still finds the note; otherwise the link gets the path it needs. If the new name makes another note's links ambiguous (two notes with the same name), those links get paths so they keep pointing where they did.
- Notes open in a tab change in their tab, unsaved, like Find and Replace in Files. The others are rewritten on disk the same careful way: only if unchanged since Strata looked, keeping their encoding and line endings, through an atomic save.
- **Notes: Undo Rename** puts everything back: the links byte for byte, including notes that were open in a tab and have been saved and closed since, and the old name. It's all or nothing: every file is checked first, and if any has changed since the rename, or the old name has been taken again, nothing changes and the status bar says which file is in the way. A file written while the undo runs is never overwritten; if that stops it partway, the status bar says which file is in which state, and anything it couldn't put back stays in a `.strata-trash-…` folder next to it. If Strata stops in the middle of an undo, a notice above your notes says which file was set aside and how far the undo got, with **Put Back** and **Show in Finder** (Explorer on Windows). As with Undo Replace in Files, Put Back only acts on folders this copy of Strata made; others get **Show in Finder** only. The backlinks panel has an Undo Rename button too. Only the last rename can be undone.
- Strata keeps the old text of the files it rewrites, and of the notes it changes in tabs, for the undo, up to 256 MB in all. If a rename would need more than that, the preview says so before you choose Update Links, and that rename can't be undone.

Renaming a folder doesn't update links yet.

## The preview

With the [Markdown preview](markdown.md) open on a note, wikilinks are links you can click (a note that doesn't exist yet is fainter), tags are tinted and clickable, and the frontmatter shows as a small block of its properties. Embeds appear in place: `![[photo.png]]` (and `![](photo.png)`) shows the image, and `![[Note]]`, `![[Note#Heading]]` or `![[Note#^block]]` shows that note, that section or that block. Embedded notes show one level deep: an embed inside an embedded note, or a note embedding itself, shows as a link. Images come only from files in the vault, never from the web.

## Running code blocks

A runbook can be a document you click through. In the [Markdown preview](markdown.md), fenced code blocks in `bash`, `sh`, `zsh`, `fish`, `pwsh`, `powershell`, `cmd`, `python`, `node` (or `js`) and `ruby` have a **▶ Run** button. Blocks in other languages, blocks without a language, indented blocks and blocks missing their closing fence have none. This works in any Markdown file, not only in a vault, and only when you click: opening a file or showing its preview never runs anything, and AI answers and help pages have no Run buttons.

- A shell block goes to the terminal tab you're using, as if you had typed it and pressed Enter (one paste where the shell supports it, as with [Send to Terminal](terminal.md)). With no terminal open, Strata starts one and sends the block once its prompt is ready. If the current tab is a REPL that Run started, the block goes to the last shell tab instead, so it can't end up typed into Python.
- A `python`, `node` or `ruby` block starts that language's REPL (`python3`, `node` or `irb`) in a new terminal tab, through your login shell so it's the same one your own terminal finds. Later blocks in that language go to the same tab, so they share variables.

**Notes: Run Code Block** does the same for the block the cursor is in. **Notes: Run Code Block and Capture Output** runs it in a separate process instead of your terminal: the language's own program (`sh`, `bash`, `python3`, `node`…) runs the block in the file's folder, with no input. Standard output and errors go together, in the order they were printed, into an `output` block right under the code, replacing the one already there. Colors and other terminal codes are left out, at most 256 KB is kept, and a note at the end says when the block failed (`[exit status 1]`), was cut short or was stopped. When the block's program ends, anything it left running in the background (`server &`, say) is stopped too. After `code_block_timeout_seconds` (60 by default) Strata stops the program, its background jobs, and the programs it started that are still running under it. A program that detaches itself on purpose (a daemon, or one started with `setsid`) leaves the block behind and can keep running, as it would after closing a terminal; on Windows, programs started outside the block's job can too. Inserting the output is one edit, so Undo takes it out again; nothing is saved until you save. If the block was edited or deleted while it ran, the output opens in a new tab instead, and the note isn't changed. Output only goes under a block whose fence starts its own line (not one written right after a list marker or in a quote). Both commands are also in Tools ▸ Notes; neither has a shortcut, so bind `run_code_block` or `run_code_block_and_capture` in [Key Bindings](keybindings.md) if you use them often.

### Asking first

Before the first block from a file runs, Strata shows exactly the text it will run and where it goes (the terminal tab, a new REPL tab, or the program and folder for a capture), with **Run** and **Cancel**. Once you choose Run, the file's other blocks run without asking until you quit Strata. Tick **Don't ask again for this file** to skip the question from then on; Strata remembers that in `code-blocks-allowed.json` in its settings folder, never in the file. The file is recognized by its real path, its identity on disk and the exact bytes it holds, so another file put at the same path (by a download, a sync tool or a checkout), or any change to it made outside Strata, asks again, while your own saves from Strata don't. **Notes: Forget Files Allowed to Run Code Blocks** makes every file ask again.

Files outside the folders open in the side bar, and tabs that aren't saved files (such as an AI answer opened in a tab), ask every time, and can't be remembered.

A block holding characters that are invisible or change the order text is shown in (right-to-left overrides and other direction controls, zero-width spaces, soft hyphens, and control characters other than tab, such as a lone carriage return or escape) never runs: what the dialog shows might not be what would run. The dialog spells each one out as ⟨U+202E⟩ and so on, and Run stays off until you remove them.

## Sync conflicts

When two devices change a note at the same time, sync tools keep both versions, the second as a copy next to the note. Strata recognizes the copies Syncthing (`Note.sync-conflict-20261019-143012-ABC1234.md`), Dropbox, Nextcloud and ownCloud (`Note (Andre's MacBook's conflicted copy 2026-10-19).md`) and other tools (`Note (conflict).md`) make, and shows a notice above the note: **Compare** opens the two side by side (see [Compare](compare.md)) so you can bring over what you want to keep, **Open Copy** opens the copy, and **Move Copy to Trash…** removes it once you're done. Only the copy you were shown goes to the Trash: if the sync tool replaces it while Strata is asking, nothing is moved. Opening the copy shows the same notice the other way round. **Notes: Compare with Conflict Copy** does the comparing from the keyboard.

iCloud's copies (`Note 2.md`) aren't recognized: they look like any note named that way.

## Git sync

If your vault is a Git repository with a remote you set up (on GitHub, your own server, or a folder on another disk), Strata can keep it in sync the way many people sync an Obsidian vault with Git: commit what changed, bring in the commits made elsewhere, push. Git sync is off until you turn it on for a vault, and Strata reaches the remote only when you sync or on a timer you set.

### Turning it on

Open a note in the vault (or the vault folder in the side bar) and choose **Notes: Enable Git Sync for This Vault** (also in Tools ▸ Notes). Strata looks at the repository and says which remote and branch it will use before anything happens. Sync needs:

- **A branch with an upstream on a remote.** `git push -u origin main` in a terminal sets one. Strata never creates remotes or branches of its own.
- **A remote reached over SSH, HTTPS or a local path.** Unencrypted `http://` and `git://` addresses, and remote helpers (`name::address`), aren't used.
- **Credentials Git already has**: your credential helper (the macOS keychain, Git Credential Manager on Windows) or your SSH agent. Strata never asks for a password and never shows a sign-in window, and SSH runs in batch mode, so a key with a passphrase needs to be in the agent and the server's host key already known. If you've never pushed from this computer, push once from a terminal first. Strata doesn't store credentials.

Git runs with the repository's own settings, as it does in a terminal: its hooks, filters (such as Git LFS) and credential helpers run when Strata commits, pulls and pushes. That's why Strata asks first, and why sync is meant for repositories you set up yourself. Reading a repository for the gutter, Compare with HEAD and [Review Changes](review.md) still never runs any of them.

Tick **Also sync every … minutes** to sync on a timer while the vault is open in a window. Choose **Enable Git Sync for This Vault** again to change the interval, and **Notes: Disable Git Sync for This Vault** to turn sync off. The vaults are kept in `notes_git_sync` in settings.json (see [Settings](#settings)).

### What a sync does

**Notes: Sync Notes**, or a click on the sync item in the status bar:

1. Saves the vault's notes that have unsaved changes, so what's committed is what you see. The timer doesn't save for you: it skips a vault with unsaved notes and tries again next time.
2. Commits every changed file in the vault; files your `.gitignore` ignores are left out. When the vault is one folder of a bigger repository, only files inside it are committed. The commit message comes from `notes_sync_message`, with the number of files and the date and time filled in.
3. Fetches, and rebases your commits onto the remote's, so the history stays a straight line. When your unpushed commits include a merge Strata made to settle a conflict (see below), it merges instead, so the conflict isn't replayed.
4. Pushes, never forced. If the remote moved on in the meantime, Strata goes round once more.

The status bar shows when the vault last synced ("Synced 14:05"), or "Sync failed" with the reason in its tooltip. Only one sync runs at a time, and the timer waits while a rename's link updates or their undo, Review Changes or Replace in Files is under way. While Git rewrites files during the rebase (usually a split second), saving a file in that repository waits: the status bar asks you to save again, and your text stays in the tab.

When something's in the way, nothing is changed and the status bar says what: the branch has no upstream, the remote can't be reached, it refused the sign-in, a rebase or merge is already under way, the remote's history has nothing in common with yours (it was replaced), files outside the vault have uncommitted changes (commit or stash them first), the remote would replace files this computer ignores, or a hook refused the commit. If Git takes longer than two minutes it is stopped, and a rebase it began is undone. A commit made before the problem stays in your repository and goes up with the next sync.

### Conflicts

If the same note changed here and on the remote, Strata undoes the rebase (`git rebase --abort`), so your files are exactly as they were, pulls and pushes nothing, and lists the notes in conflict:

1. **Compare with origin/main** opens the note in [Compare](compare.md): the remote's version on the fixed side, your file on the side you can edit. Bring over what you want to keep, and save.
2. **Keep My Versions and Sync** records a merge that keeps those notes exactly as they are on your disk, and pushes it. The remote's versions stay in the history. If the remote changed the notes again since you compared, Strata says so and you compare again.

Strata never settles a conflict by itself and never throws text away. If another program (Obsidian, a sync tool) writes a file while Git is working, its text is saved next to the file as a conflict copy, `Note (conflict).md`, before the rebase is undone; see [Sync conflicts](#sync-conflicts). The timer leaves a vault in conflict alone until it's settled; the status bar shows "Sync: conflict", and a click brings the list back.

## Obsidian settings Strata follows

Strata reads these from the vault's `.obsidian` folder, and never writes them. Every folder and file they name must be inside the vault: a path that leads out of it (with `..`, a full path, or through a folder that links elsewhere) is refused, and nothing is read or created there.

| Obsidian setting | File | What Strata does with it |
|---|---|---|
| Default location for new notes | `app.json` (`newFileLocation`, `newFileFolderPath`) | Where notes made from links, New Note and templates go. |
| Use [[Wikilinks]] | `app.json` (`useMarkdownLinks`) | Off: link completion writes Markdown links. |
| New link format | `app.json` (`newLinkFormat`) | How completion and renames write a path: shortest, relative or absolute. |
| Daily notes: folder, date format, template | `daily-notes.json` | Notes: Open Daily Note. |
| Templates: folder, date and time formats | `templates.json` | Notes: New Note from Template…, and `{{date}}` and `{{time}}`. |

Obsidian's attachment folder only matters for files you paste or drop into a note, which Strata doesn't do, so it isn't used.

## Commands

Every notes command is in Tools ▸ Notes and the command palette. None has a key; you can bind any of them in your [key bindings](keybindings.md).

| Command | Command id |
|---|---|
| Notes: Follow Link | `follow_link` |
| Notes: Open Daily Note | `open_daily_note` |
| Notes: New Note… | `new_note` |
| Notes: New Note from Template… | `new_note_from_template` |
| Notes: Find Tag… | `find_tag` |
| Notes: Rename Note… | `rename_note` |
| Notes: Undo Rename | `undo_note_rename` |
| Notes: Toggle Backlinks | `toggle_backlinks` |
| Notes: Compare with Conflict Copy | `compare_conflict_copy` |
| Notes: Run Code Block | `run_code_block` |
| Notes: Run Code Block and Capture Output | `run_code_block_and_capture` |
| Notes: Forget Files Allowed to Run Code Blocks | `forget_code_block_files` |

## Settings

| Setting | Default | What it does |
|---|---|---|
| `notes_backlinks` | `"auto"` | When the backlinks panel shows: `"auto"` (notes in an Obsidian vault, or in a folder whose notes use wikilinks), `"always"` (every note in an open folder) or `"never"`. |
| `code_block_timeout_seconds` | `60` | How long Notes: Run Code Block and Capture Output lets a block run before stopping it. |
| `notes_git_sync` | `{}` | The vaults with [Git sync](#git-sync) turned on, by folder, each with its timer in minutes (`0`: only when you sync): `{"/Users/me/Notes": {"interval_minutes": 15}}`. The Enable and Disable commands change it. |
| `notes_sync_message` | `"Notes sync: {count} changed, {date} {time}"` | The message of the commits Git sync makes. `{count}` becomes the number of changed files, `{date}` the date (2026-10-19) and `{time}` the time (14:05). |

## What isn't here yet

There's no graph view, no list of unlinked mentions, and links aren't updated when a folder is renamed. Names are matched ignoring case but not Unicode normalization, so a name typed with a separate accent mark (rare; some older macOS tools did this) won't match one written with the accented letter.

If links, backlinks or tags don't seem to update, see [Notes don't update](troubleshooting.md#notes-dont-update) in Troubleshooting.
