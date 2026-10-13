# Editing

This page covers the tools for changing text: multiple cursors, selections, line commands, comments, case conversion, completion, folding and more. Every command here is also in the Command Palette ({{key:command_palette}}).

## Multiple cursors

Strata can edit in many places at once. Almost every command applies to every cursor and selection, including typing, deleting, line commands, comments and case conversion. The status bar shows how many cursors you have.

### Adding cursors with the mouse

| Gesture | What it does |
|---------|--------------|
| ⌘-click (Ctrl+click on Windows and Linux) | Adds a cursor where you click. |
| Option-drag (Alt+drag on Windows and Linux) | Box (column) selection: one selection per line, in a rectangle. |
| Middle-button drag | Box selection too. |
| Shift+click | Extends the selection to where you click. |
| Double-click | Selects a word. |
| Triple-click | Selects a whole line. |

In a box selection, lines that are too short to reach the left edge of the box are skipped.

### Adding cursors with the keyboard

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Find ▸ Quick Add Next | {{key:find_under_expand}} | With a bare cursor, selects the word under it. Press again to add the next occurrence as another selection. It wraps around at the end of the file. |
| Find ▸ Quick Find All | {{key:find_all_under}} | Selects every occurrence of the selection (or of the word under the cursor) at once. |
| Selection ▸ Add Previous Line | {{key:select_lines_up}} | Adds a cursor on the line above, in the same column. |
| Selection ▸ Add Next Line | {{key:select_lines_down}} | Adds a cursor on the line below, in the same column. |
| Selection ▸ Split into Lines | {{key:split_selection_into_lines}} | Turns a selection that spans several lines into one selection per line. |
| Selection ▸ Single Selection | {{key:single_selection}} | Goes back to just one cursor. Esc does this too. |

When Quick Add Next starts from a bare cursor, it matches whole words only, so adding `item` skips `items`. If you select some text yourself first, it matches that text anywhere, even inside longer words.

> [!TIP]
> Copying with several selections puts each one on its own line in the clipboard. When you paste and the number of lines matches the number of cursors, each cursor gets one line.

### Soft undo

Soft undo steps back through cursor and selection changes as well as edits. If you add one cursor too many with Quick Add Next, **Edit ▸ Undo Selection ▸ Soft Undo** ({{key:soft_undo}}) removes just that cursor. **Soft Redo** ({{key:soft_redo}}) goes forward again.

## Selections

The Selection menu grows or reshapes what you have selected. Each command works on every selection.

| Command | Shortcut | What it selects |
|---------|----------|-----------------|
| Select All | {{key:select_all}} | The whole file. |
| Expand Selection | {{key:expand_selection}} | The next larger unit each time: the word, the inside of the nearest quotes or brackets, then including them, whole lines, the indented block, the block with its header line, and finally the whole file. |
| Expand Selection to Line | {{key:expand_selection_to_line}} | The whole line. Press again to add the next line. |
| Expand Selection to Line Upward | {{key:expand_selection_to_line_upward}} | The whole line. Press again to add the line above. |
| Expand Selection to Word | {{key:expand_selection_to_word}} | The word under each cursor. |
| Expand Selection to Block | {{key:expand_selection_to_block}} | The inside of the nearest `{ }` or indented block. Press again to include the braces or the header line. |
| Expand Selection to Paragraph | {{key:expand_selection_to_paragraph}} | The run of non-blank lines around the cursor. |
| Expand Selection to Brackets | {{key:expand_selection_to_brackets}} | The text inside the nearest brackets. Press again to grow outward. |
| Expand Selection to Indentation | {{key:expand_selection_to_indentation}} | The surrounding lines that are indented at least as deeply. |
| Invert Selection | {{key:invert_selection}} | Everything that isn't selected now. |

To move the cursor to the bracket that matches the one next to it, use **Goto ▸ Jump to Matching Bracket** ({{key:move_to_brackets}}).

## Line commands

These are under **Edit ▸ Line** and **Edit ▸ Text**. With no selection they act on the cursor's line; with a selection they act on every line it touches.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Indent | {{key:indent}} | Adds one level of indentation. |
| Unindent | {{key:unindent}} | Removes one level of indentation. |
| Reindent | {{key:reindent}} | Re-indents the lines to follow the line above, one level deeper after an opening bracket or `:`, one level shallower for a line starting with a closing bracket. |
| Swap Line Up | {{key:swap_line_up}} | Moves the lines up one line. |
| Swap Line Down | {{key:swap_line_down}} | Moves the lines down one line. |
| Duplicate Line | {{key:duplicate_line}} | With a cursor, copies the line below it. With a selection, copies the selected text right after itself. |
| Delete Line | {{key:delete_line}} | Deletes the whole line. |
| Join Lines | {{key:join_lines}} | Joins the line with the next one, replacing the line break and indentation with one space. |
| Insert Line Before | {{key:insert_line_before}} | Opens a new line above, with the same indentation. |
| Insert Line After | {{key:insert_line_after}} | Opens a new line below, with the same indentation, wherever the cursor is in the line. |
| Delete Word Forward | {{key:delete_word_forward}} | Deletes to the end of the word. |
| Delete Word Backward | {{key:delete_word_backward}} | Deletes to the start of the word. |
| Delete to End | {{key:delete_to_line_end}} | Deletes to the end of the line. At the end of a line, it joins the next line. |
| Delete to Beginning | {{key:delete_to_line_start}} | Deletes to the start of the line. |
| Transpose | {{key:transpose}} | Swaps the two characters around the cursor. With several selections, rotates their texts. |
| Trim Trailing Whitespace | {{key:trim_trailing_whitespace}} | Removes spaces and tabs at the ends of lines. With a single cursor and no selection, it trims the whole file. |

Tab with a selection that spans lines indents them, and Shift+Tab unindents. Copy and Cut with nothing selected copy or cut the whole line.

## Auto-closing brackets and quotes

When you type `(`, `[`, `{`, `"`, `'` or a backtick, Strata adds the closing character after the cursor.

- It only does this when the next character is a space, the end of the line, or one of `) ] } , ;`. A quote isn't paired right after a letter or digit, so typing `don't` works as expected.
- If the closing character is already next, typing it moves over it instead of adding another.
- With text selected, typing an opening bracket or quote wraps the selection in the pair.
- Backspace between an empty pair, such as `(|)`, deletes both characters.

## Word completion

As you type a word, Strata shows a list of matching words from the current file, nearest first. Snippets whose trigger matches what you typed appear in the same list, labelled with their description and a ⇥ sign. See [Snippets](snippets.md).

- The list appears once you've typed two letters of a word.
- Use the Up and Down arrows to choose, then Enter or Tab to insert. Esc closes the list. You can also click an entry.
- **Edit ▸ Show Completions** ({{key:auto_complete}}) shows the list on demand. If only one word matches, it's inserted right away.

To stop the list appearing while you type, set `"auto_complete": false` in your settings. Show Completions still works. See [Settings](settings.md).

## Auto indent and smart indent

When you press Enter, the new line keeps the indentation of the line you were on. On top of that, smart indent understands the structure of many languages:

- After a line that opens a block, such as one ending in `{` or Python's `:`, or an open HTML tag, the new line goes one level deeper.
- Pressing Enter between a pair like `{}` or `<div></div>` puts the closing part on its own line, with the cursor on an indented line between.
- After a line like Python's `return` or `pass`, the next line goes one level shallower.
- Typing a closing bracket at the start of a line lines it up with its opening bracket. In Python, finishing `else:`, `elif …:`, `except:` or `finally:` lines it up with its `if` or `try`. Typing `</` in HTML or XML moves back to the level of the tag it closes.
- When you press Enter after a line that ends a block, such as `end` in Ruby or Lua, or `fi`, `done` and `esac` in shell scripts, that line moves back to line up with its block.
- A line left with only spaces after you press Enter is emptied.

Both are on by default. Set `"auto_indent": false` to turn off all automatic indentation, or `"smart_indent": false` to just keep the previous line's indentation.

## Indentation

Each file has its own indentation: tabs or spaces, and a tab width. When you open a file, Strata looks at how it's already indented and follows that. If the file has no indented lines yet, it uses your settings.

You can see and change the current file's indentation in the status bar (it reads, for example, "Spaces: 4" or "Tab Size: 4"), or under **View ▸ Indentation**:

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Indent Using Spaces | | Switches the current file between spaces and tabs. |
| Tab Width: 1–8 | | Sets the tab width for the current file. |
| Guess Settings From Buffer | {{key:detect_indentation}} | Looks at the file's text again and picks tabs or spaces and the width. |
| Convert Indentation to Spaces | {{key:expand_tabs}} | Rewrites leading tabs as spaces. |
| Convert Indentation to Tabs | {{key:unexpand_tabs}} | Rewrites leading spaces as tabs. |

The defaults for new files come from the `tab_size`, `translate_tabs_to_spaces` and `detect_indentation` settings. See [Settings](settings.md).

When you indent with spaces, Tab inserts spaces up to the next tab stop, and Backspace in leading spaces goes back to the previous tab stop.

## Comments

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Edit ▸ Comment ▸ Toggle Comment | {{key:toggle_comment}} | Comments out the lines, or uncomments them if they're all commented already. |
| Edit ▸ Comment ▸ Toggle Block Comment | {{key:toggle_block_comment}} | Wraps the selection (or the line's text) in a block comment such as `/* … */`, or unwraps it. |

Toggle Comment uses the language's line comment, such as `//` or `#`. In languages that only have block comments, such as HTML and CSS, it uses those instead.

## Convert case

**Edit ▸ Convert Case** changes the case of the selected text. With no selection, it acts on the word under the cursor.

| Command | Shortcut | Example result |
|---------|----------|----------------|
| Title Case | {{key:title_case}} | `Hello World` |
| Upper Case | {{key:upper_case}} | `HELLO WORLD` |
| Lower Case | {{key:lower_case}} | `hello world` |
| Swap Case | {{key:swap_case}} | `hELLO wORLD` from `Hello World` |
| lowerCamelCase | {{key:lower_camel_case}} | `helloWorld` |
| UpperCamelCase | {{key:upper_camel_case}} | `HelloWorld` |
| snake_case | {{key:snake_case}} | `hello_world` |
| kebab-case | {{key:kebab_case}} | `hello-world` |

The last four split words at spaces, underscores, hyphens and changes of case, so `HTTPServer`, `http_server` and `http-server` all convert cleanly. With no selection, they act on the whole identifier under the cursor, hyphens included.

## Sorting and permuting

The line commands act on the selected lines. With a single cursor and no selection, they act on the whole file.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Edit ▸ Sort Lines | {{key:sort_lines}} | Sorts lines alphabetically, ignoring case. |
| Edit ▸ Sort Lines (Case Sensitive) | {{key:sort_lines_case_sensitive}} | Sorts lines, uppercase before lowercase. |
| Edit ▸ Permute Lines ▸ Reverse | {{key:reverse_lines}} | Reverses the order of the lines. |
| Edit ▸ Permute Lines ▸ Unique | {{key:unique_lines}} | Removes repeated lines, keeping the first of each. |
| Edit ▸ Permute Lines ▸ Shuffle | {{key:shuffle_lines}} | Puts the lines in random order. |

The selection commands rearrange the texts of two or more selections among themselves, which is handy with multiple cursors.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Edit ▸ Permute Selections ▸ Sort | {{key:sort_selections_case_insensitive}} | Sorts the selections' texts, ignoring case. |
| Edit ▸ Permute Selections ▸ Sort (Case Sensitive) | {{key:sort_selections}} | Sorts them, case sensitive. |
| Edit ▸ Permute Selections ▸ Reverse | {{key:reverse_selections}} | Reverses their order. |
| Edit ▸ Permute Selections ▸ Unique | {{key:unique_selections}} | Empties repeated ones, keeping the first of each. |
| Edit ▸ Permute Selections ▸ Shuffle | {{key:shuffle_selections}} | Puts them in random order. |

## Text tools

**Tools ▸ Text** converts text in place. Each tool works on every selection, or on the whole file if nothing is selected, and is a single undo step. If the text isn't valid input (for example, broken JSON), nothing changes and the status bar says why.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Format JSON | {{key:format_json}} | Pretty-prints JSON with indentation. |
| Minify JSON | {{key:minify_json}} | Removes all unneeded whitespace from JSON. |
| Sort JSON Keys | {{key:sort_json_keys}} | Sorts object keys alphabetically, at every level, and pretty-prints. |
| Base64 Encode | {{key:base64_encode}} | Encodes text as Base64. |
| Base64 Decode | {{key:base64_decode}} | Decodes Base64 (the URL-safe variant too). |
| URL Encode | {{key:url_encode}} | Percent-encodes text for use in a URL. |
| URL Decode | {{key:url_decode}} | Decodes percent-encoding; `+` becomes a space. |
| HTML Escape | {{key:html_escape}} | Turns `& < > " '` into HTML entities. |
| HTML Unescape | {{key:html_unescape}} | Turns named and numeric entities back into characters. |
| Insert UUID | {{key:insert_uuid}} | Inserts a random UUID at each cursor. |
| Insert Date and Time | {{key:insert_date}} | Inserts the local date and time, like `2026-03-14 09:26`, at each cursor. |

## Wrap paragraph

**Edit ▸ Wrap ▸ Wrap Paragraph at Ruler** ({{key:wrap_lines}}) reflows the paragraph around the cursor so no line is longer than the first ruler (80 characters if there are no rulers). A paragraph is a run of non-blank lines. The first line's indentation is kept, and so is a comment marker that every line shares, such as `//` or `#`, so wrapping a comment block keeps it a comment.

Like Sublime Text, the width is the first column in the `rulers` setting (View ▸ Ruler); with no rulers it is the `wrap_width` setting (80 by default). On macOS this command has no shortcut by default, because Sublime gives ⌥⌘Q to Quit and Keep Windows; bind `wrap_lines` in your key bindings to add one. Edit ▸ Wrap also has fixed widths: 70, 72, 78, 80, 100 and 120 characters.

This changes the text itself by adding line breaks. To wrap long lines on screen only, use [word wrap](#word-wrap).

## Pasting

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Edit ▸ Paste and Indent | {{key:paste_and_indent}} | Pastes and shifts the pasted lines so they match the indentation where you paste. The relative indentation inside the pasted text is kept. |
| Edit ▸ Paste from History | {{key:paste_from_history}} | Shows the last 15 pieces of text you copied or cut in Strata (pieces over 4 MB are left out). Pick one to paste it. |
| Edit ▸ Copy as HTML | {{key:copy_as_html}} | Copies the selection, or the whole file, as syntax-highlighted HTML in your current color scheme. Paste it into an email, a document or a slide to keep the colors. Apps that don't take HTML get plain text. |

## The Mark

The mark is a saved position you can come back to, as in Emacs. It's handy for selecting or deleting a large span without dragging. All of these are under **Edit ▸ Mark**.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Set Mark | {{key:set_mark}} | Remembers where each cursor is. |
| Select to Mark | {{key:select_to_mark}} | Selects from the mark to the cursor. |
| Delete to Mark | {{key:delete_to_mark}} | Deletes from the mark to the cursor, and keeps the deleted text for Yank. |
| Swap with Mark | {{key:swap_with_mark}} | Moves the cursor to the mark, and the mark to where the cursor was. |
| Clear Mark | {{key:clear_mark}} | Forgets the mark. |
| Yank | {{key:yank}} | Inserts the text most recently removed with Delete to Mark. |

With multiple cursors, each cursor gets its own mark.

## Macros

A macro records what you type in the editor so you can replay it.

1. Choose **Tools ▸ Record Macro** ({{key:toggle_record_macro}}). The status bar says it's recording.
2. Make your edits.
3. Choose **Tools ▸ Stop Recording Macro** (the same command) to finish.
4. Put the cursor where you want the same edits, and choose **Tools ▸ Playback Macro** ({{key:run_macro}}). It runs at every cursor.

A macro records:

- the text you type and paste
- keys such as the arrows, Home, End, Enter, Tab, Backspace and Delete
- editing commands you run with a keyboard shortcut, such as Duplicate Line or Toggle Comment

It does not record commands you pick from a menu or the Command Palette, mouse clicks, or window commands such as Save. Strata keeps the last macro until you record another one or quit.

## Bookmarks

Bookmarks mark lines you want to come back to. A bookmarked line shows a marker in the gutter. These commands are under **Goto ▸ Bookmarks**, which also lists every bookmark in the file so you can jump to one.

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Toggle Bookmark | {{key:toggle_bookmark}} | Adds or removes a bookmark on each cursor's line. |
| Next Bookmark | {{key:next_bookmark}} | Jumps to the next bookmark, wrapping around. |
| Previous Bookmark | {{key:prev_bookmark}} | Jumps to the previous bookmark. |
| Clear Bookmarks | {{key:clear_bookmarks}} | Removes every bookmark in the file. |
| Select All Bookmarks | {{key:select_all_bookmarks}} | Puts a cursor at every bookmark. |

You can also right-click a line number and choose Add Bookmark or Remove Bookmark. Bookmarks move with the text as you edit.

## Code folding

Folding hides a block of lines under its first line, so you can see the shape of a file. Strata finds blocks from indentation: the lines indented deeper than a line fold under it. For code that isn't indented, a line ending in `{`, `[` or `(` folds down to its closing bracket.

- Click the arrow between the line numbers and the text to fold or unfold that block. The arrows appear when you move the pointer over the line numbers, and stay visible on folded lines.
- Right-click a line number and choose Fold or Unfold.
- Click the folded marker at the end of a folded line to unfold it.

These commands are under **Edit ▸ Code Folding**:

| Command | Shortcut | What it does |
|---------|----------|--------------|
| Fold | {{key:fold}} | Folds the block at the cursor. If you've selected several lines, folds exactly those lines. |
| Unfold | {{key:unfold}} | Unfolds the folds at the cursor or in the selection. |
| Fold All | {{key:fold_all}} | Folds every top-level block. |
| Unfold All | {{key:unfold_all}} | Unfolds everything. |
| Fold Level 2–9 | ⌘K, then ⌘2 to ⌘9 (Ctrl+K, then Ctrl+2 to Ctrl+9 on Windows and Linux) | Unfolds everything, then folds every block nested at that depth or deeper. Level 2 keeps the top level open and folds what's inside it. |

## Word wrap

Word wrap breaks long lines on screen to fit the window, without changing the file.

- **View ▸ Word Wrap** ({{key:toggle_word_wrap}}) turns it on or off for the current file. Strata remembers your choice for that tab.
- Prose files wrap automatically: Markdown, MultiMarkdown, Plain Text, reStructuredText, Textile and LaTeX.
- To wrap every file by default, set `"word_wrap": true` in your settings.
- **View ▸ Word Wrap Column** wraps at a fixed column (70, 72, 78, 80, 100 or 120) instead of the window edge, or at whichever comes first if the window is narrower. **Automatic** wraps at the window edge.

Lines break after the last space that fits. A word too long for a line is cut where the line is full.

> [!NOTE]
> Files larger than about 8 million characters are never wrapped, to keep editing them fast. Files in large-file mode don't wrap unless you turn it on for them; see [Very large files](troubleshooting.md#very-large-files).

## Undo and redo

**Edit ▸ Undo** ({{key:undo}}) and **Edit ▸ Redo** ({{key:redo}}) step through your changes. A run of typing is undone in one step, and so is any single command, even one that changed text at many cursors. Undo also puts the cursors back where they were.

For undoing cursor movements and selections, see [Soft undo](#soft-undo).

Undo history holds up to 256 MB of changed text per file. Replacing all of a 50 MB file holds 50 MB, so after several such edits the oldest steps are dropped; the latest step is always kept.

## Typing in other languages

Strata supports input methods for Chinese, Japanese, Korean and other languages. The text you're composing appears at the cursor, and the input method's candidate window opens next to it. Strata loads CJK fonts from your system the first time such text appears, and CJK characters take two columns when lines wrap.

## On Save Options

**Tools ▸ On Save** has options that apply every time you save:

| Option | Setting | What it does |
|--------|---------|--------------|
| Trim Trailing Whitespace | `trim_trailing_white_space_on_save` | Removes spaces and tabs at the ends of lines. |
| Ensure Newline at End of File | `ensure_newline_at_eof_on_save` | Adds a line break at the end if the file doesn't end with one. |
| Save When Window Loses Focus | `save_on_focus_lost` | Saves every modified file that has a name whenever you switch away from Strata. |

All three are off by default. See [Settings](settings.md).
