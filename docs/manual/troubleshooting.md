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
| `Logs/` | The log: `strata.log`, plus `strata.1.log` and `strata.2.log` before it. See [Logs](#logs) |
| `Crashes/` | A report for each time Strata crashed. See [If Strata crashes](#if-strata-crashes) |
| `Print/` | PDFs made for File ▸ Print…, deleted after a day. See [Printing](printing.md) |
| `strata.sock` (macOS, Linux) or `strata.port` (Windows) | How the `strata` command finds the running app. See [Command Line](cli.md) |
| `Snippets/` | Your snippets. See [Snippets](snippets.md) |
| `Syntaxes/` | Your `.sublime-syntax` grammars. See [Languages](languages.md) |
| `Themes/` | Your `.tmTheme` and `.sublime-color-scheme` color schemes. See [Appearance](appearance.md) |
| `build_systems/` | Your build systems, as `.json` files. See [Build Systems](build.md) |

If the folder's path is very long, the `strata.sock` file goes in a folder only you can open, `strata-<your user id>`, inside `$XDG_RUNTIME_DIR` (Linux) or your system's temporary folder. Strata answers a `strata` command only when it runs as the same user.

### Who can read the folder

The folder holds the text of your unsaved tabs, the paths of files you've opened, and crash reports, so on macOS and Linux only your account can open it: Strata creates it, and the folders it keeps inside it, readable by you alone, and makes the files it writes there (session, backups, logs, crash reports, settings) readable by you alone. If an older copy of Strata left them readable by others, Strata fixes that when it starts. Your own snippets, syntaxes and color schemes keep the permissions you gave them. When you choose the folder with `STRATA_CONFIG_DIR`, Strata leaves that folder's own permissions alone (it may be shared) and only restricts what it keeps inside it.

On Windows, `%APPDATA%` is already private to your account, so Strata doesn't change any permissions there.

## Reset settings

Your settings live in `settings.json`. To go back to the defaults, do one of these:

- **While Strata is running**: open **Preferences ▸ Settings (JSON)** (**Strata ▸ Settings ▸ Settings (JSON)** on macOS), delete everything in the file, and save. Strata reloads it and uses the default for every setting.
- **With Strata closed**: rename or delete `settings.json`. The next time you open Settings, Strata writes a new file with every setting at its default.

Renaming the file while Strata is running doesn't reset anything until you restart, so quit Strata first.

## Reset key bindings

Rename or delete `keybindings.json`, or delete everything in it and save. Strata switches back to the default shortcuts right away, without a restart. See [Customizing Key Bindings](keybindings.md).

## Unsaved work and hot exit

Strata doesn't ask you to save when you quit, or when you close its last window. It remembers everything instead, and reopens it next time: every window, its tabs and panes, the open folders, every cursor and selection (multiple cursors included), and the text of tabs you haven't saved. This is called hot exit.

Closing one window while others stay open is different: that window won't come back, so Strata asks about its unsaved tabs.

- The session is saved every few seconds while you work, and again when you quit. If Strata crashes, it saves your unsaved tabs before it closes. See [If Strata crashes](#if-strata-crashes).
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

Problems show up in the status bar at the bottom of the window. Strata also writes them to a log file, with the time each one happened.

### Logs

The log is `strata.log` in the `Logs` folder inside Strata's folder (see [Where Strata keeps its files](#where-strata-keeps-its-files)):

| System | Log file |
|---|---|
| macOS | `~/Library/Application Support/Strata/Logs/strata.log` |
| Windows | `%APPDATA%\Strata\Logs\strata.log` |
| Linux | `~/.config/strata/Logs/strata.log` (or under `$XDG_CONFIG_HOME/strata`) |

- **Help ▸ Show Latest Log** opens it in a tab. **Help ▸ Open Log Folder** shows the folder in Finder, Explorer or your file manager. Both are in the command palette too.
- The log records when Strata starts (its version and your system), problems loading settings, key bindings, color schemes and syntaxes, files that couldn't be opened or saved, sessions that couldn't be restored, terminals and builds that couldn't start, and crashes.
- It holds file paths and error messages, never the text of your files or what's on the clipboard.
- When `strata.log` reaches 1 MB it becomes `strata.1.log`, the previous one becomes `strata.2.log`, and the oldest is deleted. The log never takes more than about 3 MB.
- Times are in UTC.

To log more detail, set the `STRATA_LOG` environment variable to `debug` before starting Strata. `STRATA_LOG=debug,wgpu_core=info` also shows messages from one of the libraries Strata uses.

The same lines also appear as text output when Strata runs in a terminal. On macOS, start the program inside the app with its input redirected, so it stays attached to the terminal instead of starting as a separate app:

```sh
/Applications/Strata.app/Contents/MacOS/strata < /dev/null
```

On Linux, run `strata` from a terminal.

### If Strata crashes

If Strata quits unexpectedly, it first saves your unsaved tabs and writes a crash report.

- **With hot exit on** (the default), unsaved tabs are saved with the session, the same way as when you quit, and reopen next time.
- **With hot exit off**, unsaved text goes in a new `backups.recovered-<time>/` folder, one `.txt` file per tab.
- If even that fails, the session Strata saves every few seconds still has your work up to a few seconds before the crash.

The crash report is a text file in the `Crashes` folder, named `strata-crash-<date>-<time>.txt` (UTC). It contains Strata's version, your system, where in Strata's code the crash happened, and the last 200 lines of the log. Like the log, it doesn't contain the text of your files: your home folder is written `~`, and the error message is cut short, with any text it quotes left out.

The next time Strata starts, a notice in the corner says that it quit unexpectedly and what happened to your unsaved work:

- **Show Report** opens the report in a tab.
- **Copy Report** copies it to the clipboard.
- **Report on GitHub…** opens the page for a new issue in your browser.
- **Dismiss** closes the notice.

If something goes wrong in the background, such as a search or a build, Strata logs it and keeps running. A build that stops this way says so in the build panel.

### Report a problem

Strata never sends anything by itself. To report a problem:

1. Choose **Help ▸ Report a Problem…**. A new tab opens with a template, your Strata version and system, and the end of the log. Your home folder is shown as `~`.
2. Fill in what happened and how to make it happen again.
3. Read it through and remove anything you'd rather not share, such as file names.
4. Copy it into a new issue at <https://github.com/DRAZY/strata/issues/new>. If it's about a crash, attach the crash report too.

## Fonts not found

If the editor or interface font in your settings isn't installed, Strata falls back to its built-in fonts (Hack for the editor, Ubuntu for the interface) and the status bar says which font wasn't found. Pick another one under **Preferences ▸ Font** (**Strata ▸ Settings ▸ Font** on macOS), or with **Preferences: Editor Font…** and **Preferences: Interface Font…** in the command palette. You can also fix `font_face` and `ui_font_face` in your settings. See [Appearance](appearance.md).

## Color schemes and syntaxes with errors

Strata loads your own color schemes from `Themes/` and syntaxes from `Syntaxes/` when it starts. A file that can't be read, or that uses a pattern Strata's regex engine doesn't support, is skipped, and the status bar names the file and the problem. Everything else still loads.

After you fix or add a file in these folders, restart Strata to load it.

## Files changed on disk

Strata checks the files you're looking at about once a second, and other open files when you switch to them.

- If a file changes on disk and you have no unsaved edits in it, Strata reloads it and says so in the status bar.
- If you do have unsaved edits, Strata keeps them and marks the tab with a small orange dot. The status bar says **Changed on disk** and offers a **Reload** button, which asks before discarding your edits and loading the file from disk. Undo brings the edits back.
- If you save over a file that changed on disk, Strata asks first.
- If a file is deleted on disk, its tab stays open and the status bar says **Deleted on disk (save to recreate)**. Saving writes it back.

## Very large files

Strata stays responsive on files of hundreds of megabytes and on minified files whose whole text is one line. Opening a 50 MB file takes a fraction of a second, and typing, scrolling and jumping to the end stay instant.

**Large-file mode.** A file bigger than 20 MB (the `large_file_mb` setting), or with a line longer than 10,000 characters, opens in large-file mode. The status bar says so when it opens. In this mode the file shows as Plain Text, and the minimap, word wrap, bracket colors and sticky scroll are off for it. To highlight it anyway, choose its syntax in the status bar's syntax menu (or **View ▸ Syntax**): highlighting then works in the background, a screenful at a time, so the editor never waits for it. **View ▸ Word Wrap** still turns wrapping on for that file, unless it has a line longer than 200,000 characters.

**Asking first.** Opening a file bigger than 500 MB (the `large_file_warning_mb` setting) asks first, since all of it is read into memory. So does opening a file that looks binary (it has a NUL byte near the start and isn't UTF-16): shown as text it's gibberish, and saving it could damage it.

**What else stays fast, and how:**

- Syntax highlighting works on the lines you see. When you jump far into a big file, those lines are colored at once from a best guess, and a background thread parses up to them and corrects the colors within moments.
- Very long lines are laid out only where you're looking, so a 5 MB line scrolls and edits as quickly as a short one.
- Unsaved changes are backed up for hot exit on a background thread, and a tab's backup is only rewritten when that tab changed.
- The status bar's word count appears for files up to 1 million characters and selections up to 4 million.
- Undo keeps up to 256 MB of changed text. Past that, the oldest steps are dropped (the newest step is always kept, however big).

**Limits that remain:**

- Word wrap doesn't apply to files longer than about 8 million characters.
- Lines longer than 20,000 characters are shown without syntax colors.
- Find in Files skips files larger than 50 MB.
- Git change bars aren't shown for files with more than 200,000 lines.
- Strata reads the whole file when it opens it (about a second for 500 MB).

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

**Can I bring key bindings from another editor?**
Often, yes. `keybindings.json` uses a common JSON keymap format, and many command names used by other editors' keymaps are accepted as aliases, with their arguments. Bindings for commands Strata doesn't have are skipped and reported. See [Customizing Key Bindings](keybindings.md#commands).

**Can I add color schemes and syntaxes?**
Yes. Put `.tmTheme` or `.sublime-color-scheme` files in `Themes/`, and `.sublime-syntax` files in `Syntaxes/`, then restart Strata. See [Importing color schemes](appearance.md#importing-color-schemes) and [Adding your own syntaxes](languages.md#adding-your-own-syntaxes).

**Does Strata have plugins or packages?**
Not yet. You can add snippets, syntaxes, color schemes and build systems as files, but there's no plugin system.

**Is there a Vim mode?**
No.

**Is there a spell checker?**
Yes. Turn it on with **View ▸ Spell Check** ({{key:toggle_spell_check}}). See [Spell check](editing.md#spell-check).

**Where do I see all the keyboard shortcuts?**
Open **Help ▸ Keyboard Shortcuts**, or see [Keyboard Shortcuts](shortcuts.md).
