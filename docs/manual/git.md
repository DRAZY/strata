# Git Integration

When a file belongs to a Git repository, Strata marks the lines you've changed since the last commit and shows the current branch. It's a light touch: Strata shows changes but doesn't run Git commands that change anything.

## Change bars

Lines that differ from the last commit get a colored bar at the left edge of the line-number gutter:

| Mark | Meaning |
|---|---|
| Green bar | Added lines. |
| Blue bar | Modified lines. |
| Red triangle | Lines were deleted here, between two lines. |

The colors are a little darker on light color schemes. The same marks appear as small ticks at the left of the scrollbar and along the left edge of the minimap, so you can see where the changes are in a long file. A folded section shows a bar when there are changes inside it.

The bars compare the text in the editor with the file as it was in the last commit (Git's `HEAD`). That means:

- they update as you type, before you save;
- staging a change with `git add` doesn't remove its bar, but committing it does;
- a new file that isn't committed yet gets no bars, and neither do files outside a repository or untitled tabs.

For very large files, or files whose differences are huge, Strata skips the comparison or marks a large changed area as modified, to stay fast.

### Turning change bars off

Choose View ▸ Editor Extras ▸ Git Change Markers to turn the bars on or off. This sets `git_gutter` in your settings:

| Setting | Default | Meaning |
|---|---|---|
| `git_gutter` | `true` | Show Git change bars in the gutter, scrollbar and minimap. |

See [Settings](settings.md) for how to edit `settings.json`.

## Branch in the status bar

When the current file is in a repository, the status bar shows its branch, for example "⎇ main". If you've checked out a commit rather than a branch, it shows the short commit hash instead.

## Jumping between changes

| Command | Key |
|---|---|
| Goto ▸ Next Modification | {{key:next_modification}} |
| Goto ▸ Previous Modification | {{key:prev_modification}} |

These move the cursor to the start of the next or previous changed area, wrapping around at the end of the file. If the file has no changes since the last commit, the status bar says "No changes since the last commit". Each jump is added to your navigation history, so Goto ▸ Jump Back returns to where you were.

They work even when the change bars are turned off.

## Compare with HEAD

To see the whole file next to its last committed version, choose File ▸ Compare ▸ Compare with HEAD ("Git: Compare with HEAD" in the command palette, {{key:compare_with_head}}). The committed text is on the left and your file, still editable, on the right, with the changed lines and words marked. See [Comparing Files](compare.md).

## Show Unsaved Changes

To see what you've changed since you last saved (rather than since the last commit), right-click in the editor and choose Show Unsaved Changes…. Strata opens a read-only tab named "Unsaved changes: …" with a unified diff of the file on disk against the text in the editor. This works for any saved file, in a repository or not, and is available when the file has unsaved changes.

## How it works

Strata runs the `git` command-line program in the background to find the repository, the branch and each file's committed text, so Git must be installed and on your search path. Without it, Strata works normally but shows no change bars or branch.

The work happens in the background and never makes the editor wait. Strata compares a file again shortly after each edit, right after you save, and every few seconds while the file is visible, so commits, checkouts and branch switches made in a terminal show up on their own within a few seconds.

Strata never asks for passwords and doesn't contact remote servers; it only reads your local repository.

## What isn't supported

Strata doesn't have its own Git interface. To commit, stage, switch branches, pull, push, view history or blame, or discard changes, use Git in the [terminal](terminal.md) or another Git tool. The side bar doesn't color files by their Git status.
