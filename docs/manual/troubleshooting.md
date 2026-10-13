# Troubleshooting and FAQ

This page explains where Strata keeps its files, how to reset things, and what common messages mean.

## Where Strata keeps its files

Strata keeps your settings, key bindings and session in one folder:

| System | Folder |
|---|---|
| macOS | `~/Library/Application Support/Strata` |
| Windows | `%APPDATA%\Strata` |
| Linux | `$XDG_CONFIG_HOME/strata`, or `~/.config/strata` if that variable isn't set |

To use a different folder, set the `STRATA_CONFIG_DIR` environment variable before starting Strata. A copy of Strata started this way has its own settings and session, and doesn't talk to a copy that uses the normal folder. This is handy for testing a new setup without touching your everyday one.

### What's in the folder

Strata creates most of these only when it needs them.

| Name | What it is |
|---|---|
| `settings.json` | Your settings. See [Settings](settings.md) |
| `keybindings.json` | Your key bindings, created the first time you open Key Bindings. See [Customizing Key Bindings](keybindings.md) |
| `session.json` | Your open windows, tabs, panes, folders and cursor positions, plus recent files, folders and projects |
| `backups/` | The text of unsaved tabs, one `.txt` file per tab |
| `backups.recovered-<time>/` | Backups set aside because a session couldn't be restored. `<time>` is when it happened, in seconds since 1970 |
| `session.lock` | Marks which running Strata owns the session |
| `strata.sock` (macOS, Linux) or `strata.port` (Windows) | How the `strata` command finds the running app. See [Command Line](cli.md) |
| `Snippets/` | Your snippets. See [Snippets](snippets.md) |
| `Syntaxes/` | Your `.sublime-syntax` grammars. See [Languages](languages.md) |
| `Themes/` | Your `.tmTheme` and `.sublime-color-scheme` color schemes. See [Appearance](appearance.md) |
| `build_systems/` | Your build systems, as `.json` files. See [Build Systems](build.md) |

If the folder's path is very long, the `strata.sock` file goes in your system's temporary folder instead.

## Reset settings

Your settings live in `settings.json`. To go back to the defaults, do one of these:

- **While Strata is running**: open **Preferences ▸ Settings** (**Strata ▸ Settings ▸ Settings** on macOS), delete everything in the file, and save. Strata reloads it and uses the default for every setting.
- **With Strata closed**: rename or delete `settings.json`. The next time you open Settings, Strata writes a new file with every setting at its default.

Renaming the file while Strata is running doesn't reset anything until you restart, so quit Strata first.

## Reset key bindings

Rename or delete `keybindings.json`, or delete everything in it and save. Strata switches back to the default shortcuts right away, without a restart. See [Customizing Key Bindings](keybindings.md).

## Unsaved work and hot exit

Strata doesn't ask you to save when you quit, or when you close its last window. It remembers everything instead, and reopens it next time: every window, its tabs and panes, the open folders, every cursor and selection (multiple cursors included), and the text of tabs you haven't saved. This is called hot exit.

Closing one window while others stay open is different: that window won't come back, so Strata asks about its unsaved tabs.

- The session is saved every few seconds while you work, and again when you quit. A crash loses at most a few seconds of typing.
- Unsaved text goes in the `backups/` folder as plain `.txt` files, so you can always open them yourself.
- If a file changed on disk while Strata was closed, its restored tab is marked **Changed on disk**. If the file was deleted, the status bar says **Deleted on disk (save to recreate)**.

To turn hot exit off, set `"hot_exit": false` in your settings. Strata then asks about each unsaved tab when you close, and doesn't reopen your tabs next time. See [all settings](settings.md#all-settings).

### If the session can't be restored

If `session.json` is damaged, or a backup can't be read, Strata doesn't throw anything away. It moves the whole `backups/` folder to `backups.recovered-<time>/` and the status bar says where it went. Open the `.txt` files in that folder to get your text back, then delete the folder when you're done.

If Strata can't move the backups either, it turns session saving off for that run so nothing gets overwritten, and says so in the status bar.

## Settings file has errors

If `settings.json` has a mistake, such as a missing comma, Strata shows the error in the status bar.

- **At startup**, Strata uses the default settings until you fix the file.
- **While running**, Strata keeps your current settings and adds "(keeping current settings)" to the message.

Strata never overwrites a settings file that has errors. While the file is broken, changes you make from menus (like zoom or color scheme) can't be saved, and the status bar asks you to fix the file first. Comments and trailing commas are allowed, so those aren't errors.

Key bindings work the same way: a broken `keybindings.json` keeps the last shortcuts that worked. See [Customizing Key Bindings](keybindings.md).

## Another Strata owns the session

The status bar message "Another Strata window owns the session: this one asks before closing unsaved tabs" means a second copy of Strata started while another copy was already running with the same settings folder. Only one copy saves and restores the session. This usually happens when the running copy didn't answer the new one in time.

The second copy works normally, with two differences: it doesn't reopen or save your session, and it asks before closing tabs with unsaved changes. To get back to normal, quit the extra copy and keep using the first one.

## Messages and logs

Strata doesn't write a log file. Problems show up in the status bar at the bottom of the window.

A few messages, such as the full list of problems in `keybindings.json`, are also printed as text output. To see them, quit Strata and start it from a terminal so the output stays there. On macOS, run the program inside the app with its input redirected, so it stays attached to the terminal instead of starting as a separate app:

```sh
/Applications/Strata.app/Contents/MacOS/strata < /dev/null
```

On Linux, run `strata` from a terminal. On Windows, Strata is a windowed program and these messages aren't shown.

## Fonts not found

If the editor or interface font in your settings isn't installed, Strata falls back to its built-in fonts (Hack for the editor, Ubuntu for the interface) and the status bar says which font wasn't found. Pick another one under **Preferences ▸ Font** (**Strata ▸ Settings ▸ Font** on macOS), or with **Preferences: Editor Font…** and **Preferences: Interface Font…** in the command palette. You can also fix `font_face` and `ui_font_face` in your settings. See [Appearance](appearance.md).

## Color schemes and syntaxes with errors

Strata loads your own color schemes from `Themes/` and syntaxes from `Syntaxes/` when it starts. A file that can't be read, or that uses a pattern Strata's regex engine doesn't support, is skipped, and the status bar names the file and the problem. Everything else still loads.

After you fix or add a file in these folders, restart Strata to load it.

## Files changed on disk

Strata checks the files you're looking at about once a second, and other open files when you switch to them.

- If a file changes on disk and you have no unsaved edits in it, Strata reloads it and says so in the status bar.
- If you do have unsaved edits, Strata keeps them and marks the tab with a small orange dot. The status bar says **Changed on disk** and offers a **Reload** button, which discards your edits and loads the file from disk.
- If you save over a file that changed on disk, Strata asks first.
- If a file is deleted on disk, its tab stays open and the status bar says **Deleted on disk (save to recreate)**. Saving writes it back.

## Very large files

Strata opens large files, but turns a few features off to stay fast:

- Word wrap doesn't apply to files longer than about 8 million characters.
- Lines longer than 20,000 characters are shown without syntax colors.
- Find in Files skips files larger than 50 MB.
- Git change bars aren't shown for files with more than 200,000 lines.

## macOS blocks the first launch

Test builds of Strata for macOS aren't notarized by Apple, so the first time you open one, macOS may block it. Any of these gets past it:

- Right-click (or Control-click) Strata.app in Finder, choose **Open**, then **Open** again.
- On recent macOS versions, try to open Strata once, then go to **System Settings ▸ Privacy & Security** and click **Open Anyway**.
- In a terminal, remove the quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/Strata.app
```

You only need to do this once.

## The strata command is missing

Install it first. On macOS and Linux, run **Strata: Install Shell Command** from the command palette. On Windows, add Strata's folder to your `PATH`. Details are in [Command Line](cli.md).

## FAQ

**Can I use my Sublime Text key bindings?**
Mostly. Strata reads Sublime's keymap format, and many Sublime command names and arguments work as they are. See [Customizing Key Bindings](keybindings.md#commands).

**Can I use Sublime Text color schemes and syntaxes?**
Yes. Put `.tmTheme` or `.sublime-color-scheme` files in `Themes/`, and `.sublime-syntax` files in `Syntaxes/`, then restart Strata.

**Does Strata have plugins or packages?**
No. Strata doesn't have a plugin system.

**Is there a Vim mode?**
No.

**Is there a spell checker?**
No.

**Where do I see all the keyboard shortcuts?**
Open **Help ▸ Keyboard Shortcuts**, or see [Keyboard Shortcuts](shortcuts.md).
