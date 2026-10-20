# Comparing Files

Compare shows two versions of a text side by side: the lines line up, with blank filler rows where one side has lines the other doesn't, changed lines are tinted, and the words that changed inside a line are marked more strongly. Both sides scroll together.

The left side is a fixed text: another tab, a file, or the file as last committed to Git. The right side is your file itself, and you can edit it right there: the comparison updates as you type, and saving saves the file.

## Starting a comparison

| Command | Shortcut | What it does |
|---|---|---|
| File ▸ Compare ▸ Compare with… | {{key:compare_with}} | Compare the current file with another open tab, the version at Git `HEAD`, or a file you choose. |
| File ▸ Compare ▸ Compare with HEAD | {{key:compare_with_head}} | Compare the current file with its last committed version ("Git: Compare with HEAD" in the command palette). |

You can also right-click a tab and choose **Compare with…**, or right-click a file in the side bar and choose **Compare with Open File…**, which opens the file if needed and asks what to compare it with.

"Compare with…" lists the other open tabs first, then "Git: the version at HEAD" when the file is in a repository, then "Choose a File…". The comparison opens in a new tab named after both sides, like `old.rs ↔ new.rs`, next to the file's own tab.

When the left side is another open tab, it follows that tab: edit either file and the comparison updates. A file chosen from disk, or the HEAD version, stays as it was when you opened the comparison.

## Reading a comparison

| Mark | Meaning |
|---|---|
| Red tint, `−` in the gutter | Lines only on the left (removed). |
| Green tint, `+` in the gutter | Lines only on the right (added). |
| Red on the left and green on the right, side by side | A changed line. The words that differ are tinted more strongly. |
| Hatched row | Filler: the other side has lines here that this side doesn't. |

The header above the two sides says how many changes there are and how many lines were removed and added, for example "2 of 5 changes · −3 +7". The strip at the right edge is an overview of the whole comparison: each change is a mark, the frame shows what's on screen, and clicking or dragging in it scrolls there.

## Moving between changes

| Command | Shortcut | What it does |
|---|---|---|
| Compare: Next Difference | {{key:next_difference}} | Go to the next change, wrapping around at the end. |
| Compare: Previous Difference | {{key:prev_difference}} | Go to the previous change. |

The caret lands on the first line of the change on the right side, and the change is scrolled into view. The ▲ and ▼ buttons in the header do the same. Outside a Compare tab, these commands jump between the lines you've changed since the last Git commit, like Goto ▸ Next Modification (and say "Not in a Git repository" or "Save the file first" when there's nothing to compare against).

## Options

| Command | What it does |
|---|---|
| Compare: Swap Sides | Show the editable side on the left instead. The colors follow: the fixed side always reads as the old version. |
| Compare: Toggle Ignore Whitespace | Treat lines that differ only in spaces and tabs as the same, and don't mark whitespace-only changes inside a line. |

Both are also buttons in the header (**Swap Sides** and the **Ignore whitespace** checkbox) and items in File ▸ Compare.

## Editing in a comparison

The right side is another view of the file's buffer, so everything you can do in an editor works there: typing, selecting with the mouse or keyboard, multiple cursors, undo, the Edit and Selection menus, find, and save. Edits show up at once in the file's own tab, and undo works from either one. The comparison is recomputed as you type; for very large files it waits for a short pause in your typing.

Closing a Compare tab never asks about unsaved changes when the file is still open in another tab; that tab keeps them. Compare tabs aren't kept by hot exit: the file's own tab (and any unsaved text) is.

The comparison works line by line, matching lines that are unique on both sides first (a "patience" diff) and finding the shortest set of changes between them, so moved blocks and repeated lines like `}` don't produce confusing results. Two 50,000-line files compare in a few milliseconds.
