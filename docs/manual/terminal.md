# Terminal

Strata has a built-in terminal in a panel at the bottom of the window. It runs your usual shell, can hold several shells as tabs, and uses the colors of your color scheme.

## Opening the terminal

There are three ways to show the terminal:

- Choose View ▸ Terminal, or press {{key:toggle_terminal}}. In the command palette this is "View: Toggle Terminal".
- Choose Tools ▸ New Terminal, or press {{key:new_terminal}} ("Terminal: New Terminal" in the palette). This always starts another shell in a new tab.
- Click the + button in the terminal's tab row.

Toggle Terminal works like a switch:

- If the panel is showing, it hides the panel and gives the keyboard back to the editor. Your shells keep running while the panel is hidden.
- If the panel is hidden and a shell is already running, it shows the panel again and puts the keyboard in the terminal.
- If no shell is running yet, it starts one.

The panel is hidden in distraction-free mode and comes back when you leave it.

## Shell and starting folder

| System | Shell |
|---|---|
| macOS | The shell in your `SHELL` environment variable, started as a login shell (so your profile sets up `PATH` as it does in Terminal.app). If that isn't set, Strata uses `/bin/zsh`, then `/bin/bash`, then `/bin/sh`. |
| Linux | The shell in `SHELL`, else `/bin/zsh`, `/bin/bash` or `/bin/sh`. It is started as a regular (not login) shell. |
| Windows | Windows PowerShell (`powershell.exe -NoLogo`). If PowerShell can't be started, the program in `COMSPEC` (normally `cmd.exe`). |

A new terminal starts in:

1. the side bar folder that holds the current file (or the first side bar folder), if one is open;
2. otherwise the folder of the current file;
3. otherwise your home folder.

Strata tells programs in the terminal what it supports by setting `TERM=xterm-256color`, `COLORTERM=truecolor` and `TERM_PROGRAM=Strata`.

## Tabs

Each shell gets a tab in the row at the top of the panel. The tab shows the title the running program sets, or the shell's name (such as "zsh") if it hasn't set one. Hover over a tab to see its full title.

| To do this | Do this |
|---|---|
| Open a new tab | Click +, press {{key:new_terminal}}, or choose New Terminal from a right-click menu. |
| Switch tabs | Click a tab. |
| Close a tab | Click the × next to its name, or right-click it and choose Close Terminal. |
| Hide the panel | Click the × at the far right of the tab row, or press {{key:toggle_terminal}}. |

Right-clicking a tab shows:

| Item | What it does |
|---|---|
| Close Terminal | Closes that tab and stops its shell. |
| Close Other Terminals | Closes every tab except this one. Available when there is more than one tab. |
| New Terminal | Starts another shell in a new tab. |

When a shell exits (for example, you type `exit`), its tab closes by itself. When the last tab closes, the panel hides.

If a program rings the terminal bell, its tab name and the panel's border flash briefly.

## Resizing the panel

Drag the top edge of the panel up or down. Strata remembers the height when you let go, and new windows and future sessions use it.

## Colors and text

The terminal takes its background, text, cursor and selection colors from the current color scheme, so it matches the editor. Its 16 basic colors come from the scheme's own terminal palette when the scheme has one; otherwise Strata uses a palette tuned for light or dark backgrounds. See [Appearance](appearance.md) to change the color scheme.

Programs can use all 256 xterm colors and full 24-bit ("truecolor") colors. Programs that change the palette themselves (for example, with a theme script) are honored.

Text uses the editor's monospace font at the editor's font size. Bold, italic, underlined, strikethrough, dim and inverse text are shown. The cursor can be a block, a bar or an underline, as the program asks; when the terminal doesn't have the keyboard, the cursor is drawn as an outline. The window opacity setting also applies to the terminal background.

## Selecting and scrolling

| Action | Result |
|---|---|
| Drag with the mouse | Select text. |
| Double-click | Select a word. |
| Triple-click | Select a line. |
| Click | Clear the selection. |
| Mouse wheel or trackpad | Scroll back through earlier output. |
| Shift+PageUp, Shift+PageDown | Scroll back or forward a page. |

The terminal keeps up to 10,000 lines of scrollback. As soon as you type, the view jumps back to the bottom and the selection is cleared.

In full-screen programs such as `less` or `vim`, the wheel scrolls the program's own view instead of the scrollback. Programs that ask for mouse reporting receive wheel events. Mouse clicks are always used for selecting text and are not sent to programs.

## Copy, paste and clear

| Action | macOS | Windows and Linux |
|---|---|---|
| Copy the selection | ⌘C | Ctrl+Shift+C, or Ctrl+C when text is selected |
| Paste | ⌘V | Ctrl+V or Ctrl+Shift+V |
| Clear the scrollback | ⌘K | Right-click ▸ Clear (no key) |

On Windows and Linux, Ctrl+C with nothing selected is sent to the shell as usual, so it still interrupts the running program. Ctrl+V always pastes; the shell never receives it. On Windows, Ctrl+Insert copies and Shift+Insert pastes as well.

Pasted text that spans several lines is sent with the line breaks a terminal expects. If the running program asks for "bracketed paste" (most modern shells do), the paste is marked so the shell doesn't run each line as you paste it.

Clear removes the scrollback and asks the shell to redraw its prompt at the top. It does nothing while a full-screen program is running.

Programs can also put text on the clipboard with the standard terminal escape sequence for that (OSC 52), which is handy for copying from a remote shell or tmux. Each time one does, the status bar says "A program in the terminal set the clipboard", so a program (or a file you print with `cat`) can't quietly swap what you're about to paste. To block this, set `"terminal_allow_clipboard_write": false` in your settings; the status bar then says when a program tried.

Reading the clipboard the same way is blocked by default, because any program, or a file you print with `cat`, could otherwise read what you copied, such as a password. When a program tries, the status bar says Strata blocked it. To allow it, set `"terminal_allow_clipboard_read": true` in your settings.

## Right-click menu

Right-click inside the terminal for:

| Item | What it does |
|---|---|
| Copy | Copies the selection. Available when text is selected. Right-clicking keeps your selection. |
| Paste | Pastes the clipboard into the shell. |
| Select All | Selects all the scrollback and the screen. |
| Clear | Clears the scrollback (⌘K on macOS). |
| New Terminal | Starts another shell in a new tab. |
| Close Terminal | Closes the current tab. If it was the last one, the panel hides. |
| Hide Panel | Hides the terminal; the shells keep running. |

## Keys in the terminal

While the terminal has the keyboard, most keys go to the shell, so that programs like `bash`, `vim` and `tmux` work as they do in any terminal. Only some of Strata's shortcuts still work:

| System | Keys the terminal keeps | Keys Strata keeps |
|---|---|---|
| macOS | Plain keys, function keys, and every Ctrl and Option (⌥) combination | Shortcuts that use ⌘ |
| Windows and Linux | Plain keys, function keys, Alt combinations, and Ctrl combinations without Shift | Shortcuts that use Ctrl+Shift |

Toggle Terminal ({{key:toggle_terminal}}) and New Terminal ({{key:new_terminal}}) always work, whatever keys they are bound to.

So on macOS the command palette, Save and Build still work from the terminal, while Ctrl+G and Ctrl+R go to the shell. On Windows and Linux, Ctrl+Shift+P (the command palette) works, but Ctrl+S, Ctrl+B and F4 go to the shell. Editing commands, such as Toggle Comment, never act while the terminal has the keyboard.

On macOS, while the terminal has the keyboard, menu items whose shortcut isn't a ⌘ shortcut show no shortcut, so the menu bar doesn't take those keys from the shell.

### Alt and Option as Meta

The Alt key (Option on macOS) works as the Meta key: Alt+B sends Esc followed by B, which shells use for word movement and editing. On macOS this means Option can't type the special characters it normally types (such as accented letters), so use the editor for those and paste them.

### Making your own shortcuts work in the terminal

To make one of your own key bindings work while the terminal has the keyboard, give it the `terminal_focus` context in your `keybindings.json`:

```json
[
  { "keys": ["ctrl+alt+t"], "command": "toggle_terminal", "context": [{"key": "terminal_focus"}] }
]
```

See [Key Bindings](keybindings.md) for how to edit key bindings.

## Going back to the editor

Click in the editor, or press {{key:toggle_terminal}} to hide the terminal and return the keyboard to the editor. Esc and Tab go to the shell, so they don't leave the terminal.

## Closing windows and quitting

Closing a terminal tab, closing a window, or quitting Strata stops the shells in it, along with any programs they started. Strata doesn't ask first, so finish or save anything running in the terminal before you close it.

Each window has its own terminal panel and tabs. See [Multiple windows](panes.md#multiple-windows).

## Settings

Strata saves these for you as you use the terminal. You can also set them in `settings.json` (see [Settings](settings.md)).

| Setting | Default | Meaning |
|---|---|---|
| `terminal_visible` | `false` | Whether the terminal panel was open. When it's `true` at launch, a shell starts in the first window. It changes each time you show or hide the panel. |
| `terminal_height` | `240` | Height of the panel in points. It is saved when you finish resizing the panel. Values between 80 and 2000 are used. |

The terminal also follows `font_size` and `window_opacity`.

## Limitations

- The terminal is always a panel at the bottom of the window. There are no split terminals, and terminal tabs can't be moved into editor panes or other windows.
- Tabs can't be renamed or reordered.
- Mouse clicks aren't passed to terminal programs; only the scroll wheel is.
- Closing a tab or window doesn't warn about programs still running in it.
