# Review Changes

Review Changes is for checking what a coding agent (Claude Code, Codex, Aider or any other tool) changed in your project, and keeping or undoing each change. The agent runs wherever you like, often in Strata's own [terminal](terminal.md). Strata doesn't use AI for this: it compares your files with Git and shows the differences.

It works in a folder that is a Git repository.

## The Changes list

Choose Tools ▸ Review Changes ▸ Show Changes ("Review: Show Changes" in the command palette), or click **Changes** at the top of the side bar. The list shows every file that differs from the base:

| Mark | Meaning |
|---|---|
| **M** | Modified. |
| **A** | New: the file wasn't there before. |
| **D** | Deleted. |
| **R** | Renamed (or moved): a deleted file and a new one with exactly the same content. |
| +12 −3 | Lines added and removed. "binary" and "large" files show no counts. |
| ● before the name | The file has unsaved edits in Strata. |
| ⚠ before the name | The file changed on disk while its tab has unsaved edits. |

The list updates by itself every few seconds while it's open, so you can watch an agent work. The ⟳ button looks again right away. The first look at a big repository can take a few seconds; after that only files that changed are read again.

The list shows the files as they are on disk. Your unsaved edits show in the file's tab and in the review tab, not in the list, until you save. When an agent changes a file you have unsaved edits in, Strata keeps your edits and marks the tab "Changed on disk", as it always does (see [Files changed on disk](welcome.md#files-changed-on-disk)); the ⚠ in the list points it out.

### What the changes are compared with

| Base | How to get it | What shows |
|---|---|---|
| The last commit (`HEAD`) | The default. | Everything you haven't committed. When a commit is made, its changes drop out of the list. |
| A snapshot | Click **Start Review** at the top of the list (Tools ▸ Review Changes ▸ Start Review). | Only what changes after that moment, even if the agent commits along the way. Files you had already changed before count as the starting point. |

Start a review just before you let an agent loose. **End Review** goes back to comparing with the last commit. The snapshot is kept in memory while the window is open: it isn't written to your repository or to disk, and it ends when you close the window or quit.

Files that Git ignores (`.gitignore`) aren't listed, and neither are the contents of other repositories inside yours (such as submodules) or symbolic links.

## Reviewing a file

Click a file in the list, or press {{key:review_next_change}} (Review: Next Change), to open its review tab: the base on the left, read-only, and the file on the right, editable, with the changed lines and words marked. It is a [Compare tab](compare.md) with Keep and Revert buttons in its header. One review tab is open at a time: moving to another file closes the previous one, unless it has unsaved edits.

| Command | Key | What it does |
|---|---|---|
| Review: Next Change | {{key:review_next_change}} | Go to the next change. After the file's last change, open the next file to review. |
| Review: Previous Change | {{key:review_prev_change}} | Go to the previous change, back into the previous file. |
| Review: Next File | {{key:review_next_file}} | Open the next file to review. |
| Review: Previous File | {{key:review_prev_file}} | Open the previous file to review. |
| Review: Keep Change | {{key:review_keep_change}} | Accept the change at the caret. |
| Review: Revert Change | {{key:review_revert_change}} | Undo the change at the caret. |
| Review: Keep File | {{key:review_keep_file}} | Accept every change in the file. |
| Review: Revert File | {{key:review_revert_file}} | Put the whole file back as it was in the base. |
| Review: Mark File as Reviewed | {{key:review_mark_reviewed}} | Tick the file off (or bring it back), then open the next file. |

Keep and Revert act on the change the caret is in; Next Change puts the caret there. The ▲ and ▼ buttons and {{key:next_difference}} move between the changes of the open file, as in any Compare tab.

### Revert

Reverting a change is an ordinary edit in the file: Undo ({{key:undo}}) brings the change back, and nothing reaches the disk until you save. The text put back is exactly the base's, with its own line endings (CRLF stays CRLF).

| File | Revert File does |
|---|---|
| Modified | Replaces the whole text with the base's. Save to keep it. |
| Deleted | Opens the file with its old text, unsaved. Save to bring it back. |
| New | Asks, then moves the file to the Trash (the Recycle Bin on Windows). You can put it back from there. |
| Renamed | Asks, then moves the new file to the Trash (the Recycle Bin on Windows) and opens the old one with its text, unsaved. Save to finish. |

### Keep

Keeping a change accepts it: it stops showing in the review tab and the file's counts in the list no longer include it. Nothing changes on disk. A file whose changes are all kept (and saved) moves to the reviewed files. Keep isn't undone by Undo; **Clear** at the top of the list forgets what you kept and every tick.

## Marking files as reviewed

Tick the box next to a file, or use Mark Reviewed in the review tab, to set it aside. Reviewed files collapse into a **Reviewed** section at the bottom of the list, and the status bar counts the files still to review ("3 to review"; click it to show the list). If the file changes again afterwards (the agent edits it once more), it comes back to the list. Saving it yourself doesn't untick it.

## Good to know

- Line endings: a repository that stores files with LF and writes them out with CRLF (`core.autocrlf`, or `eol=crlf` in `.gitattributes`) doesn't show every line as changed.
- Binary files are listed but can't be shown or reverted in Strata; use Git for those. Files over 8 MB are listed without line counts, and files over 64 MB can't be opened for review.
- Renames are recognized when the content didn't change. A file that was moved and edited shows as deleted and new.
- Strata runs the `git` command-line program in the background to read the repository. It never changes it (no staging, no commits, no stash) and never runs programs named in the repository's configuration, such as filters or hooks. Review Changes makes no network connections.
