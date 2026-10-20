# Plugins

A plugin adds commands, status bar items, panels and more to Strata. Each plugin is a separate program, written in any language, that Strata starts and talks to over its standard input and output, the way it talks to language servers. A plugin can't crash Strata, and it can only ask Strata for what you allowed it to.

Strata has no online plugin directory: you install a plugin from a folder or a `.zip` file you got from someone you trust. Two small examples come with Strata's source, in `examples/plugins`: **Word Count** (Python) shows the current file's word count in the status bar, and **Sort Unique** (Node) sorts lines and removes duplicates.

## Installing a plugin

Choose Tools ▸ Plugins ▸ **Install from Folder…** or **Install from Zip…** (also in the command palette, {{key:command_palette}}). Pick the plugin's folder (the one holding `strata-plugin.json`) or its zip file. Strata checks the package, copies it into the `Plugins` folder in its settings folder (on macOS `~/Library/Application Support/Strata/Plugins`, on Windows `%APPDATA%\Strata\Plugins`), and opens the Plugins manager on it. Nothing runs yet.

Installing a plugin that's already installed replaces it with the new version: the running copy is stopped first, and if any of its files differ, Strata asks for permission again before it runs. A failed install changes nothing.

Strata refuses a package that:

- has no valid `strata-plugin.json` at its top (see [The manifest](#the-manifest)), or names a program to run that isn't in it;
- contains links (symbolic links or junctions), or, in a zip, paths that are absolute or use `..`, names Windows can't hold, the same name twice, encrypted entries, or compression other than stored and deflate;
- has more than 1,000 files and folders, folders nested more than 16 deep, a file over 16 MB, or more than 64 MB in all (a zip itself may be at most 64 MB, and its sizes are counted as it's unpacked, never trusted from its headers).

A folder's `.git` folder and `.DS_Store` files aren't copied. A zip made by zipping the plugin's folder works as it is: the folder at its top is taken off.

## Permission to run

The first time a plugin is about to start, Strata shows what it is and what it asks to do, and waits for you:

- its name, version, author and description, the program Strata will run and the folder it runs in;
- each permission it asks for, in plain words (the table below);
- that Strata can't sandbox the program.

**Allow and Start** remembers your answer for this exact package, by its name and a SHA-256 of every file in it. If any of its files change (an update, an edit, a file added), Strata asks again. What runs is always that exact package: when a plugin starts, Strata copies it into a read-only folder named by its SHA-256 (`Plugins/.snapshots`), asks about that copy, checks the copy once more right before starting it, and runs it from there, so files changed in the installed folder meanwhile never run unapproved. A copy that was changed, or isn't read-only any more, is refused and removed, and a plugin whose copy can't be made read-only doesn't start. Strata keeps these copies only in a real folder of its own that no one else may write to (not a link), and says so if that isn't the case. A running plugin holds a lock on its copy; copies nothing was allowed for are removed when Strata starts, once they're a day old and no Strata is running them.

A plugin run by an interpreter (`python3`, `node`) depends on that interpreter and on libraries installed on your computer, which aren't part of the package Strata checks; the dialog says so. Strata does start plugins without the environment variables that make interpreters or the system load code from elsewhere (such as `PYTHONPATH`, `PYTHONSTARTUP`, `NODE_OPTIONS`, `NODE_PATH`, `RUBYLIB`, `PERL5LIB`, `LD_PRELOAD` and `DYLD_*`), and tells Python to ignore your user site folder and the current folder. **Don't Start** (or Esc) leaves it stopped; it won't ask again on its own until you quit, but running one of its commands or choosing **Start** in the Plugins manager asks again. Your answers are kept in `plugins.json` in the settings folder.

| Permission | What the dialog says | What it allows through Strata |
|---|---|---|
| `read_document` | Read the text of the files you have open | `document/read`, and the `document/…` notifications |
| `edit_document` | Change the text of files you have open (you can undo each change) | `document/applyEdits` |
| `read_workspace` | Read files in the folders open in the side bar | `workspace/readFile`, `workspace/folders` |
| `write_workspace` | Create and replace files in the folders open in the side bar, including scripts and settings your tools run | `workspace/writeFile`, `workspace/folders` |
| `panel` | Show a panel of text, and lists to pick from | `ui/showPanel`, `ui/pick` |
| `notify` | Show messages and items in the status bar | `ui/notify`, `ui/statusItem` |
| `run_process` | Run any program on your computer | Nothing: the plugin does this itself |
| `network` | Reach the internet and computers on your network | Nothing: the plugin does this itself |

Strata checks every message a plugin sends against the permissions you gave it before doing anything: a request it isn't allowed is answered with an error (`-32010`) and noted in the plugin's log. But a plugin is a program running on your computer with your account's access, and Strata can't sandbox a native program. Whatever it does on its own, outside Strata (reading and changing your files, starting other programs, using the network), isn't limited by these permissions, which is why `run_process` and `network` are shown as "Strata can't limit this". A plugin that asks for them is telling you it does those things. Install plugins only from people you trust.

## The Plugins manager

Tools ▸ Plugins ▸ **Manage Plugins…** lists every installed plugin. For the selected one it shows:

- what it is, its folder, and whether it's running (or why it stopped);
- **Turned on**: a plugin that's turned off never starts, and its commands and keys are gone until you turn it on again;
- **Start**, **Stop** and **Restart**, and **Remove…**, which deletes Strata's copy of it and forgets what you allowed it (its data folder, `PluginData/<name>`, stays);
- the permissions it asks for, its commands and keys (and any key it couldn't have, with the reason), and its settings with their defaults;
- its log: what it wrote with `log`, what it printed on its standard error, requests Strata refused, and when it started and stopped. The log is kept in memory only (the last 1,000 lines), never on disk, because a plugin may quote your documents in it. **Copy Log** copies it.

## When plugins run

A plugin starts when something it waits for happens, never before you've allowed it:

- you run one of its commands;
- a file matching one of its `open:` patterns opens, or one matching a `save:` pattern is saved;
- Strata starts, if it asks to (`startup`; please use this only when a plugin really needs it).

It keeps running until you quit Strata, stop it, turn it off or remove it. If it stops on its own, Strata starts it again after 1 second, then 2 seconds; after the third stop within five minutes, Strata leaves it stopped and says so (start it again from the Plugins manager). A plugin that doesn't answer `initialize` within 10 seconds is stopped, and a command that doesn't finish within 30 seconds is reported in the status bar. Stopping a plugin also ends every program it started.

A plugin can't slow Strata down by sending too much: Strata takes a few requests from it each frame (fewer when they're large, such as reading big documents) and makes the plugin wait for the rest, keeps at most 1,000 lines of its log, and makes it wait for room before taking a request whose answer could be large. Notifications to a plugin that stops reading are dropped, saying so in its log; answers to its requests never are.

## Commands, keys and settings

A plugin's commands appear in the command palette as "Title: Command" (for example **Sort Unique: Sort Lines Uniquely**) and in Tools ▸ Plugins ▸ *Title*.

A plugin may give its commands keys, but never one Strata already uses, including keys you set in your [key bindings](keybindings.md): such a key simply isn't bound, and the Plugins manager says why. If two plugins want the same key, the first by name gets it. Plugins can bind letters and digits with ⌘ (Ctrl), with or without Shift and ⌥ (Alt), and function keys. Plugin commands can't be bound in `keybindings.json` yet.

A plugin's settings go in the `plugins` section of `settings.json`, under the plugin's name (see [Settings](settings.md#plugins)):

```json
"plugins": {
    "word-count": {"count_numbers": false},
    "sort-unique": {"ignore_case": true}
}
```

Each setting the plugin lists has a default, which is used until you set it. A value of the wrong type keeps the default, and a name the plugin doesn't list is ignored; both are noted in its log. A running plugin gets new values as soon as you save `settings.json`.

## Writing a plugin

A plugin is a folder with a `strata-plugin.json` and the program it describes. Strata runs the program in that folder, sends it JSON-RPC 2.0 messages on its standard input and reads its answers on its standard output. Everything here is version `strata.plugin/1` of the API.

### The manifest

```json
{
  "name": "word-count",
  "title": "Word Count",
  "version": "1.0.0",
  "description": "Shows how many words the current file has in the status bar.",
  "author": "Your Name",
  "license": "MIT",
  "api": "strata.plugin/1",
  "run": {"command": "python3", "args": ["word_count.py"], "windows": {"command": "py", "args": ["-3", "word_count.py"]}},
  "activation": ["open:*"],
  "permissions": ["read_document", "notify", "panel"],
  "contributes": {
    "commands": [{"id": "show", "title": "Show Word Count"}],
    "keybindings": [{"command": "show", "key": "primary+alt+shift+w"}],
    "settings": {"count_numbers": {"default": true, "description": "Count numbers as words."}}
  }
}
```

| Field | Required | What it is |
|---|---|---|
| `name` | yes | 1 to 64 lower-case letters, digits and hyphens. It names the installed folder and the plugin's settings. |
| `title` | no | How menus and the palette name it, in title case, without `:`. Made from the name if left out (`word-count` → "Word Count"). |
| `version` | yes | Letters, digits, dots, hyphens and plus signs, such as `1.0.0`. |
| `description`, `author`, `license` | no | Shown in the permission dialog and the manager. |
| `api` | yes | `"strata.plugin/1"`. |
| `run` | yes | `command` and `args`. The command is a program found on your PATH (`python3`, `node`) or a path inside the plugin's folder (`bin/plugin`, `./run.sh`), never absolute and never leaving the folder. `macos`, `windows` and `linux` (each with `command` and `args`) replace it on that system. |
| `activation` | no | `"startup"`, `"open:<glob>"` and `"save:<glob>"`. A glob without `/` matches the file's name (`*.md`); one with `/` matches its whole path. A plugin starts anyway when one of its commands runs. |
| `permissions` | no | The permissions above, each at most once. |
| `contributes.commands` | no | Up to 100 of `{"id", "title", "menu"}`: `id` is lower-case letters, digits, `.`, `-` and `_`; `title` is in title case (like "Sort Lines Uniquely"); `"menu": false` leaves it out of Tools ▸ Plugins. |
| `contributes.keybindings` | no | `{"command", "key"}`, with keys written as in [key bindings](keybindings.md) (`primary` is ⌘ on macOS and Ctrl elsewhere). |
| `contributes.settings` | no | Up to 100, by name: `{"default", "description"}`. |

A plugin with no commands needs at least one activation event. Strata explains any problem with the manifest when you install it, naming the field.

### Running

The program starts in its approved, read-only copy (see [Permission to run](#permission-to-run)), with Strata's environment minus the variables listed there, plus `STRATA_PLUGIN_API=strata.plugin/1`, `PYTHONUNBUFFERED=1`, `PYTHONDONTWRITEBYTECODE=1`, `PYTHONNOUSERSITE=1` and `PYTHONSAFEPATH=1`. Strata looks for programs on your PATH and in the usual install folders, as for build systems.

A plugin can't write into its own folder. For files of its own, use the folder Strata gives it in `initialize` (`storagePath`).

Standard output is only for messages. Anything else (debugging, errors) goes to standard error, which Strata puts in the plugin's log.

### Messages

Every message is a JSON object after a header giving its length in bytes, as language servers do:

```text
Content-Length: 52\r\n
\r\n
{"jsonrpc":"2.0","method":"log","params":{"message":"hi"}}
```

Requests have an `id` (a number or a string) and get an answer with the same `id`: a `result`, or an `error` with a `code` and a `message`. Notifications have no `id` and get no answer. Both sides may send requests at any time, so a plugin waiting for one answer must keep reading and handle what else arrives meanwhile (the examples show how). A message may be at most 64 MB. Requests the window answers are smaller still: up to 40 MB for `document/applyEdits`, 4 MB for `ui/pick`, 2 MB for `ui/showPanel` and 256 KB for the others; bigger ones are refused with `-32014`.

Positions are 0-based: `{"line": 3, "character": 10}`, where `character` counts Unicode characters (code points), not bytes or UTF-16 units. A character past the end of a line means the end of that line (before its line break); a line past the last one is an error.

Documents are identified by an `id` that stays the same while the tab is open, and have a `version` that goes up with every change. A document is described as:

```json
{"id": 7, "path": "/Users/me/notes/todo.md", "name": "todo.md", "version": 12, "syntax": "Markdown"}
```

`path` is `null` for a tab that was never saved.

### From Strata to the plugin

#### initialize

The first message, a request. Params:

| Field | What it is |
|---|---|
| `api` | `"strata.plugin/1"` |
| `strata` | `{"version": "…"}`, Strata's version |
| `plugin` | `{"name", "version"}` from the manifest |
| `permissions` | The permissions you allowed, by id |
| `settings` | The plugin's settings: defaults with your values over them |
| `storagePath` | A folder for the plugin's own files (kept when it's updated or removed), or `null` |
| `workspaceFolders` | The folders open in the side bar, with `read_workspace` or `write_workspace`; otherwise `null` |

Answer within 10 seconds. The result may be `{"api": "strata.plugin/1"}`; a plugin that answers with another `api`, or with an error, isn't started. After the answer Strata sends the `initialized` notification, then the plugin's first command (if one started it), the open documents and the one in front of each window.

#### initialized

A notification: Strata got the answer to `initialize`. No params.

#### command/execute

A request to run one of the plugin's commands. Params: `command` (its id) and `document` (the current tab for a plugin with `read_document` or `edit_document`; otherwise, and when there's no text tab in front, `null`). The result is ignored; an error is shown in the status bar. Answer within 30 seconds.

#### document/opened, document/changed, document/saved, document/closed

Notifications, sent only with `read_document`. Params: `{"document": …}` (for `document/closed`, just `{"id"}`). A plugin hears of every open document when it starts. `document/changed` comes at most every 150 ms for a document while you type. They never include the text: use `document/read`.

#### document/activated

A notification, sent only with `read_document`: a document came to the front of a window (a window opened, or you switched to another tab). Params: `{"document": …}`, after its `document/opened`. A plugin hears of the document in front of each window when it starts. Each window has its own document in front, so a plugin that shows something about "the current file" should show it with `ui/statusItem`'s `document`.

#### settings/changed

A notification with `{"settings": …}`, the plugin's settings after you saved `settings.json` with new values for it.

#### shutdown and exit

When Strata stops the plugin it sends the `shutdown` request (answer it within 0.8 seconds) and then the `exit` notification, and closes the plugin's input. A plugin still running two seconds or so later is ended, together with every program it started.

### From the plugin to Strata

Each of these is a request unless it says otherwise. A request without its permission is refused with `-32010`.

#### document/read

Permission: `read_document`. Params: `document` (an id; leave it out for the tab that's in front). Result:

| Field | What it is |
|---|---|
| `document` | The document, as above |
| `text` | Its whole text, with its own line breaks |
| `selections` | Each selection as `{"start", "end", "active"}` positions (`active` is where the cursor is) |
| `lineEnding` | `"\n"` or `"\r\n"` |

Errors: `-32012` (no such open document), `-32014` (larger than 16 MB).

#### document/applyEdits

Permission: `edit_document`. Params: `document` (an id), `version` (the version the edits were made for, from `document/read` or an event), and `edits`, a list of `{"range": {"start", "end"}, "newText"}`. The edits are applied together as one change, so one Undo takes them all back, and the cursors stay on the same text. Line breaks in `newText` become the document's own. Result: `{"applied": true, "version": <the new version>}`.

Errors: `-32011` (the document changed since that version: read it again), `-32016` (it's read-only), `-32012` (not open), `-32602` (edits that overlap, a range that ends before it starts, a line past the end), `-32014` (more than 100,000 edits, or more than 16 MB of new text).

#### workspace/folders

Permission: `read_workspace` or `write_workspace`. No params. Result: `{"folders": [paths]}`, the folders open in the side bar of every window.

#### workspace/readFile

Permission: `read_workspace`. Params: `path`, absolute. Result: `{"text"}`.

The file must be inside a folder open in the side bar, reached without going through any link (a link to somewhere else in the folder is refused too), have no other names (a hard-linked file could also live outside the folders, so it's refused), and not be inside a `.git` folder, whatever the path calls it (on Windows, names Windows would change, such as short `GIT~1` names, names ending in a dot or space, and `name:stream`, are refused). Errors: `-32013` (outside the folders, through a link, or in `.git`), `-32017` (not a file, or not UTF-8 text), `-32014` (larger than 16 MB).

#### workspace/writeFile

Permission: `write_workspace`. Params: `path` (absolute) and `text`. Creates the file or replaces it whole (a new file renamed over the old one, keeping its permissions). Result: `{"written": true}`.

The same rules as reading apply, and the folder the file goes in must already exist. A link at the file's name is refused, never written through, and so is a folder. Only the file's permission bits are kept (not setuid or setgid). Errors: `-32013`, `-32017`, `-32014` (as for reading).

#### ui/showPanel

Permission: `panel`. Params: `title`, and `markdown` (drawn like Strata's help) or `text` (shown as it is, in a fixed-width font), at most 1 MB. The panel opens at the bottom of the window, replacing the plugin panel there before; links in it to `https://` and `http://` addresses open in your browser. Result: `null`.

#### ui/notify

Permission: `notify`. Params: `message` (its first line, up to 300 characters, is shown) and `level` (`"info"`, `"warning"` or `"error"`). The message appears in the status bar, after the plugin's title. Result: `null`. It may also be sent as a notification.

#### ui/statusItem

Permission: `notify`. Params: `id` (the plugin's own name for the item), `text` (one line, up to 60 characters; empty removes the item), `tooltip`, `command` (one of the plugin's command ids, run when the item is clicked), and `document` (the id of a document the plugin was told of, which takes `read_document`; leave it out for an item every window shows). An item with a `document` shows only in windows with that document in front, in place of the plugin's item with the same `id` for every document, and goes away when the document is closed; set one per document to show, say, each file's word count in its own window. A plugin may use up to 4 item ids; items are removed when it stops. Result: `null`. Errors: `-32015` (a fifth id), `-32602` (a `command` that isn't the plugin's), `-32012` (a `document` that isn't open, or that the plugin wasn't told of).

#### ui/pick

Permission: `panel`. Params: `items`, 1 to 5,000 entries, each text or `{"label", "detail"}`, and `placeholder`, shown in the list's search field. Strata shows the list where the command palette appears, and answers when it closes: `{"index": n}` for the chosen item, or `null` if it was closed without a choice. It must be a request, not a notification. Errors: `-32015` (another list or dialog is open).

#### log

No permission needed. Params: `message` and `level` (`"debug"`, `"info"`, `"warning"` or `"error"`). Adds a line to the plugin's log in the Plugins manager. Usually sent as a notification; as a request it's answered with `null`.

### Errors

| Code | Meaning |
|---|---|
| `-32700` | A message that isn't JSON |
| `-32600` | Not a valid request |
| `-32601` | Strata has no such method |
| `-32602` | Missing or wrong parameters |
| `-32603` | Something went wrong in Strata |
| `-32010` | The plugin wasn't allowed this |
| `-32011` | The document changed since the version given |
| `-32012` | The document isn't open |
| `-32013` | Outside the open folders, through a link, or in `.git` |
| `-32014` | Too large |
| `-32015` | Busy: another list is open, or too many status items |
| `-32016` | The document is read-only |
| `-32017` | A file couldn't be read or written |

### Trying it out

Install one of the examples from `examples/plugins`, open the Plugins manager, and watch its log while you use it. While writing your own, install it again from its folder after each change. Strata asks for permission again each time, since the package changed; the same happens if you edit the installed copy and choose **Restart**.

## Commands

| Command | Command id |
|---|---|
| Plugins: Manage Plugins… | `manage_plugins` |
| Plugins: Install from Folder… | `install_plugin_from_folder` |
| Plugins: Install from Zip… | `install_plugin_from_zip` |
