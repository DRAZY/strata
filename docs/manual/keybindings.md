# Customizing Key Bindings

You can change any shortcut in Strata by editing one file, `keybindings.json`. It uses a common JSON keymap format, and many command names from other editors' keymaps are accepted too, so you can often paste existing bindings straight in.

## Open your key bindings

Use any of these:

- **Preferences ▸ Key Bindings** (Windows and Linux)
- **Strata ▸ Settings ▸ Key Bindings** (macOS)
- The command palette ({{key:command_palette}}): **Preferences: Key Bindings**

Strata splits the window into two panes. The left pane shows **Default Key Bindings**: every built-in shortcut for your platform, read-only. Commands that have no shortcut are listed too, commented out, so this is also the place to look up command names. The right pane is your own `keybindings.json`.

The first time you open it, Strata creates `keybindings.json` with a short guide in comments. To change a shortcut, copy a line from the left pane into your file, edit its keys, and save.

> [!TIP]
> To see the shortcuts that are in effect right now, including your changes, open **Help ▸ Keyboard Shortcuts**. See [Keyboard Shortcuts](shortcuts.md).

## The file format

`keybindings.json` is a JSON list. Each entry is an object with:

| Field | Required | What it does |
|---|---|---|
| `keys` | yes | A list with one key, or two for a chord: `["primary+shift+k"]` |
| `command` | yes | The command to run, by name |
| `args` | no | Arguments for commands that take them |
| `context` | no | Conditions that limit where the binding works |

Comments (`//` and `/* */`) and trailing commas are allowed.

```json
[
  // Delete the current line
  { "keys": ["primary+shift+k"], "command": "delete_line" },

  // A chord: press Cmd+K (Ctrl+K), then Cmd+T (Ctrl+T)
  { "keys": ["primary+k", "primary+t"], "command": "title_case" },

  // A command with arguments
  { "keys": ["primary+shift+p"], "command": "show_overlay", "args": {"overlay": "command_palette"} },

  // Remove a default shortcut
  { "keys": ["primary+shift+d"], "command": "" },
]
```

Your entries are applied on top of the defaults. You only need to list what you want to change.

## Writing keys

A key is written as modifiers and a key name joined by `+`, such as `ctrl+shift+k` or `primary+alt+up`. Upper and lower case don't matter.

### Modifiers

| Name | Key |
|---|---|
| `primary` | Cmd on macOS, Ctrl on Windows and Linux |
| `ctrl` or `control` | Ctrl. On Windows and Linux this is the same key as `primary` |
| `alt`, `option` or `opt` | Alt, or Option on a Mac |
| `shift` | Shift |
| `super`, `cmd`, `command` or `meta` | Cmd. macOS only |

Use `primary` when you want one file that works the same way on every computer. On Windows and Linux, a binding that uses `super`, `cmd`, `command` or `meta` is skipped, and the status bar tells you to use `ctrl` or `primary` instead.

### Key names

Letters and digits are written as themselves: `a` to `z`, `0` to `9`.

| Key | Names you can use |
|---|---|
| Arrow keys | `up`, `down`, `left`, `right` |
| Enter | `enter`, `return`, `keypad_enter` |
| Tab | `tab` |
| Escape | `escape`, `esc` |
| Space | `space` |
| Backspace | `backspace` |
| Delete | `delete`, `forward_delete`, `del` |
| Insert | `insert`, `ins` |
| Home, End | `home`, `end` |
| Page Up, Page Down | `pageup`, `page_up`, `pagedown`, `page_down` |
| Function keys | `f1` to `f35` |
| Plus | `plus`, `keypad_plus`, or `+` (so `ctrl++` works) |
| Minus | `minus`, `keypad_minus`, `-` |
| Equals | `equals`, `equal`, `=` |
| Comma | `comma`, `,` |
| Period | `period`, `keypad_period`, `keypad_decimal`, `.` |
| Slash | `slash`, `keypad_divide`, `/` |
| Backslash | `backslash`, `\` |
| Semicolon | `semicolon`, `;` |
| Backtick | `backquote`, `backtick`, `grave`, or the backtick character itself |
| Quote | `quote`, `'` |
| Brackets | `open_bracket`, `[`, `close_bracket`, `]` |
| Other punctuation | `{`, `}`, `:`, `\|`, `?`, `!` |
| Keypad digits | `keypad0` to `keypad9` |

Keypad keys count as the matching main-keyboard keys: `keypad5` is the same as `5`, and `keypad_plus` is the same as `plus`. Keypad keys without a main-keyboard twin, such as `keypad_multiply`, aren't supported.

### Shifted keys

Strata first matches the character a key types, then the physical key. So for a shifted key you can write the plain key plus `shift`: `primary+shift+]` matches Cmd+Shift+] even though that combination types `}`. The same goes for digits: `ctrl+shift+1` works even though Shift+1 types `!`.

## Chords

A chord is two keys pressed one after the other. Upper Case ({{key:upper_case}}) is one: primary+K, then primary+U. Write both keys in the list:

```json
{ "keys": ["primary+k", "primary+u"], "command": "upper_case" }
```

- Chords can have at most two keys. A longer list is skipped with a message.
- After the first key, Strata waits for the second. Press Escape to cancel.
- If the second key doesn't finish a chord, it does whatever it normally does.
- A key can't be both a shortcut of its own and the start of a chord. Binding `primary+k` alone turns off every chord that starts with it, and the status bar says so.

## Commands

The `command` can be any of these:

- **A Strata command name.** Every command is listed in the Default Key Bindings pane, including the ones with no shortcut. For example `save`, `goto_line`, `toggle_side_bar`, `duplicate_line`, `upper_case`, `toggle_terminal` and `key_bindings`.
- **A common alias.** Command names used by other editors' keymaps are also accepted, with their arguments, so existing keymaps mostly work. See the two tables below.
- **`""`, `"unbound"` or `"noop"`.** See [Unbinding](#unbinding).

A command Strata doesn't recognize is skipped and reported in the status bar with its line number.

### Commands that take arguments

These commands need `args`, or do something different depending on them.

| Command | Arguments | What it runs |
|---|---|---|
| `focus_group` | `{"group": 0}` to `3` | Focus pane 1 to 4 |
| `move_to_group` | `{"group": 0}` to `3` | Move the file to pane 1 to 4 |
| `select_by_index` or `select_tab` | `{"index": 0}` to `8` | Select the pane's 1st to 9th tab. Index 8 always selects the last tab |
| `fold_by_level` | `{"level": 1}` to `9` | Fold everything deeper than that level. Level 1 folds everything |
| `scroll_lines` | `{"amount": 1}` | Scroll one line. A positive amount scrolls up, a negative one scrolls down |
| `show_overlay` | `{"overlay": "command_palette"}` | Command palette |
| `show_overlay` | `{"overlay": "goto"}` | Goto Anything. Add `"text": ":"` for Goto Line or `"text": "@"` for Goto Symbol |
| `show_panel` | `{"panel": "find"}` | Find. Also `replace`, `find_in_files`, `incremental_find`, and `output.exec` (show or hide build results) |
| `toggle_comment` | `{"block": true}` | Toggle block comment. Without it, or with `false`, toggles a line comment |
| `sort_lines` | `{"case_sensitive": true}` | Sort lines, case sensitive. Without it, case insensitive |
| `select_lines` | `{"forward": false}` | Add a cursor on the previous line. With `true` or no args, the next line |
| `delete_word` | `{"forward": false}` | Delete the word before the cursor. With `true` or no args, the word after |
| `expand_selection` | `{"to": "line"}` | Expand to line. Also `brackets`, `word`, `paragraph`, `indentation`, `block`. `smart`, `scope` or no args expand step by step |
| `permute_lines` | `{"operation": "reverse"}` | Also `unique` and `shuffle` |
| `permute_selection` | `{"operation": "sort"}` | Also `sort_case_insensitive`, `reverse`, `unique`, `shuffle` |
| `move_to` | `{"to": "brackets"}` | Jump to the matching bracket |
| `clear_bookmarks` | `{"name": "mark"}` | Clear the mark. Without it, clears bookmarks |
| `new_pane` | `{"move": false}` | Add an empty pane. With `true` or no args, move the file to a new pane |
| `focus_neighboring_group` | `{"forward": false}` | Focus the previous pane. With `true` or no args, the next one |
| `move_to_neighboring_group` | `{"forward": false}` | Move the file to the previous pane. With `true` or no args, the next one |
| `build` | `{"select": true}` | Build With (choose a build system). Without it, Build |
| `toggle_setting` | `{"setting": "word_wrap"}` | Toggle word wrap. Only `word_wrap` is supported |
| `set_layout` | `cols`, `rows` and `cells` | The standard layouts: single, 2 to 4 columns, 2 or 3 rows, and the 4-pane grid |
| `run_macro_file` | `{"file": "res://Packages/Default/Delete Line.sublime-macro"}` | The standard line macros: Delete Line, Add Line, Add Line Before, Delete to Hard EOL, Delete to Hard BOL, Delete to BOL |
| `edit_settings` | a `base_file` ending in `.sublime-keymap` | Open Key Bindings. Without it, open Settings |

Pane and tab numbers in `args` start at 0.

### Other command aliases

These names take no arguments and run the Strata command shown.

| Alias | Strata command |
|---|---|
| `prompt_open_file`, `prompt_open` | `open_file` |
| `prompt_open_folder`, `prompt_add_folder` | `open_folder` |
| `prompt_save_as` | `save_as` |
| `close` | `close_file` |
| `close_folder_list` | `close_folder` |
| `prompt_open_project_or_workspace` | `open_project` |
| `prompt_select_workspace` | `switch_project` |
| `close_workspace` | `close_project` |
| `save_project_and_workspace_as` | `save_project_as` |
| `close_pane` | `close_group` |
| `redo_or_repeat` | `redo` |

Commands that come from another editor's plugins don't exist in Strata, so bindings for them are skipped and reported.

## Unbinding

To remove a default shortcut, bind its keys to an empty command:

```json
{ "keys": ["primary+shift+d"], "command": "" }
```

`"unbound"` does the same thing, and so does `"command": null`. The key goes back to doing nothing special, so it reaches whatever has the keyboard, such as the editor. If nothing was bound to that key, the status bar says so.

To make a key do nothing at all, bind it to `"noop"`. Strata swallows the key so nothing else sees it.

```json
{ "keys": ["ctrl+q"], "command": "noop" }
```

## Contexts

A `context` limits a binding to certain situations. It's a list of conditions, and all of them must hold for the binding to work.

```json
{ "keys": ["ctrl+k"], "command": "toggle_terminal", "context": [{"key": "terminal_focus"}] }
```

Each condition has:

- `key`: what to check (see the table).
- `operator`: `equal` (the default) or `not_equal`.
- `operand`: `true` (the default) or `false`.

So `{"key": "panel_visible", "operand": false}` and `{"key": "panel_visible", "operator": "not_equal"}` both mean "when no panel is open".

| Context key | True when |
|---|---|
| `terminal_focus` | A terminal has the keyboard |
| `panel_has_focus` | A panel has the keyboard: the terminal, or the find bar |
| `panel_visible` | The find bar or the terminal panel is open |
| `overlay_visible` or `overlay_has_focus` | The command palette or another quick panel (Goto Anything, Goto Line, and so on) is open |

Other context keys, such as `selector` or `setting.*`, aren't supported. A binding that uses one is skipped and reported.

### The terminal keeps most keys

When a terminal has the keyboard, plain keys and most Ctrl and Alt combinations go to the shell, so programs like editors and pagers work normally. Only shortcuts that use Cmd (macOS) or Ctrl+Shift (Windows and Linux) reach Strata, plus Toggle Terminal and New Terminal. To make any other key work in the terminal, add a second binding with the `terminal_focus` context.

### Editing commands need the editor

Commands that act on text, such as Duplicate Line or Fold, only run when the editor has the keyboard. In the find bar or another text field, their keys do whatever that field does. Window commands, such as Save or the command palette, work everywhere except where the terminal keeps the key.

## Which binding wins

- **Your file wins over the defaults**, and later entries in your file win over earlier ones.
- **Binding a command replaces its default keys.** If you bind `command_palette` to a new key, the old key stops opening the palette. This only applies within the same context: a binding with a `context` doesn't remove the command's ordinary shortcut.
- **Taking a key from another command is reported.** If your binding uses a key that already ran something else, the status bar names the line and what that key used to do. The binding still works. These messages are only there so a pasted keymap doesn't silently replace shortcuts you rely on.

When several lines have problems, the status bar shows the first one and how many more there are.

## Saving and reloading

Changes apply as soon as you save `keybindings.json`. You don't need to restart, and Strata also notices if another program changes the file. When the new keymap loads cleanly, the status bar says "Key bindings reloaded".

If the file isn't valid JSON, or isn't a list, Strata keeps the last key bindings that worked and shows the problem in the status bar with its line number. Fix the line and save again.

A single bad entry, such as an unknown command or key, doesn't stop the rest of the file from working. That entry is skipped and reported.

If you delete `keybindings.json`, Strata goes back to the default shortcuts. Opening Key Bindings again creates a fresh file. See [Reset key bindings](troubleshooting.md#reset-key-bindings) in Troubleshooting.

## Where your shortcuts show up

Strata uses one keymap everywhere, so your changes appear in:

- The menus, including the native menu bar on macOS. Chords are shown as text next to the item, because a menu can't use them as accelerators.
- The command palette.
- Right-click menus.
- **Help ▸ Keyboard Shortcuts**.

A command's first shortcut is the one shown. A binding with a `context` isn't shown in menus.

## Keys outside the keymap

A few standard keys are handled outside the keymap:

- **Cut, Copy and Paste** (primary+X, primary+C, primary+V). They aren't listed in the defaults; unbound, they cut, copy and paste. If you bind one of these keys (with or without Shift), your binding runs instead wherever it applies, and the menu item stops showing the key. Where your binding doesn't apply (for example a binding for the editor while you type in the find bar), the key still cuts, copies or pastes. Paste and Indent ({{key:paste_and_indent}}) works this way too.
- **Quit Strata** and, on macOS, **Quit and Keep Windows** (Cmd+Q and Cmd+Option+Q). If you bind one of these keys to a command, the menu item gives up the key and your binding gets it.

Undo, Redo and Select All are ordinary commands (`undo`, `redo`, `select_all`) and can be rebound like any other.

## See every shortcut

For the complete, current list of shortcuts on your computer, open **Help ▸ Keyboard Shortcuts** or read [Keyboard Shortcuts](shortcuts.md).
