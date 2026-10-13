# Command Line

The `strata` command opens files and folders in Strata from a terminal. It can also wait for you to finish editing, so you can use Strata as the editor for git and other tools.

## Synopsis

```sh
strata [options] [file[:line[:column]] | folder]...
```

Some examples:

```sh
strata notes.md                 # open a file
strata src/main.rs:120          # open at line 120
strata src/main.rs:120:8        # open at line 120, column 8
strata ~/projects/website       # open a folder in the sidebar
strata -n todo.txt              # open in a new window
strata -w message.txt           # wait until you close the tab
```

## Options

| Option | What it does |
|---|---|
| `-n`, `--new-window` | Open the paths in a new window |
| `-w`, `--wait` | Wait until the files you opened are closed before returning |
| `-h`, `--help` | Show a short help text and exit |
| `-v`, `--version` | Show the version and exit |
| `--` | Treat everything after it as a path, even names that start with `-` |

You can give any number of paths, and mix files and folders. An unknown option prints the help text and exits with an error.

## Opening at a line and column

Add `:line` or `:line:column` to a file name to put the cursor there. Lines and columns start at 1, like in compiler messages, so you can paste a location straight from an error.

- A line past the end of the file goes to the last line.
- Line 0 is treated as line 1.

Some files really have names that end in a colon and a number. If a file with the full name exists, such as `notes:12`, Strata opens that file. Only when no such file exists is the number read as a line.

## New files and folders

- **A file that doesn't exist yet** opens in an empty tab. The file is created the first time you save, so `strata new-idea.md` is a quick way to start a new file.
- **A folder** opens in the sidebar. See [Projects](projects.md).

## When Strata is already running

Only one copy of Strata runs at a time. If it's already open, the `strata` command hands your paths to it, brings it to the front, and returns.

- Files open in the window you used last.
- A file that's already open in any window comes forward in that window instead of opening twice.
- With `-n`, the files open in a new window. If the current window is empty (one blank tab and no folder), Strata uses it instead.
- `strata` with no paths just brings Strata to the front. `strata -n` opens a new empty window.

## Waiting for files to close

With `-w`, the command doesn't return until every file it opened has been closed, in any window. Folders don't count. This is what tools that launch an editor expect.

To make Strata your editor for git commit messages:

```sh
git config --global core.editor "strata -w"
```

To use it for other programs that read the `EDITOR` or `VISUAL` variables, add this to your shell profile:

```sh
export EDITOR="strata -w"
export VISUAL="strata -w"
```

Write your message, save, and close the tab. The command returns and the tool carries on.

> [!NOTE]
> On macOS, when you run it from a terminal, `-w` works whether or not Strata is already running. On Windows and Linux, if Strata isn't running yet, the command starts Strata itself, so it returns when you quit Strata rather than when you close the tab. Keep Strata open to avoid this.

## Install the strata command

### macOS

Choose **Strata ▸ Install Shell Command…**, or run **Strata: Install Shell Command** from the command palette. Strata creates a link named `strata` that points to the app:

1. It tries `/usr/local/bin/strata` first.
2. If that folder isn't writable, it uses `~/.local/bin/strata` instead. If that folder isn't on your `PATH`, the status bar tells you to add it.

If both fail, the status bar shows a `sudo ln -sf …` command you can run in a terminal to create the link yourself.

Strata replaces an older `strata` link, but it never replaces a real file with that name. If you move Strata.app, run Install Shell Command again.

When you run `strata` in a terminal on macOS, Strata starts as a normal app with its Dock icon, and the command returns right away (unless you used `-w`).

### Linux

Run **Strata: Install Shell Command** from the command palette. It works the same way as on macOS: a link in `/usr/local/bin`, or in `~/.local/bin` if that isn't writable.

### Windows

There's nothing to install. Running **Strata: Install Shell Command** shows which folder holds Strata's program file. Add that folder to your `PATH`:

1. Open **Settings**, search for "environment variables", and choose **Edit environment variables for your account**.
2. Select **Path**, choose **Edit**, then **New**, and paste the folder.
3. Open a new terminal window and run `strata`.

Strata is a windowed program on Windows, so a terminal may not show the text printed by `strata --help` or `strata --version`.

## Opening files on macOS

Besides the command line, you can open files and folders in Strata from:

- **Finder**: right-click a file or folder and choose **Open With ▸ Strata**. Strata is offered for text, code and most other files, and for folders, but it never makes itself the default app.
- **The Dock**: drop files or folders on Strata's icon.
- **Terminal**: `open -a Strata file.txt`.
- **A Strata window**: drag files or folders onto it.

Files open in tabs and folders open in the sidebar, as they do from the command line. A file that's already open comes forward in its window.

## Exit status

`strata` exits with 0 on success and 2 for an unknown option. Other failures, such as Strata not starting, return 1.
