# Build Systems

Build systems let you compile, run or test the file you're working on without leaving Strata. Strata picks a suitable build system for you, shows the output in a panel, and turns error messages into links to the right line.

## Building

| Command | Key | What it does |
|---|---|---|
| Tools ▸ Build | {{key:build}} | Runs the build system for the current file. |
| Tools ▸ Build With… | {{key:build_with}} | Lists every build system that fits the current file, with its variants, and runs the one you pick. |
| Tools ▸ Cancel Build | {{key:cancel_build}} | Stops the running build. |

In the Command Palette these are "Tools: Build", "Tools: Build With…" and "Tools: Cancel Build".

Build With… shows entries such as "Cargo" and "Cargo - Test": a build system's main command, then each of its variants. Strata remembers your choice, and Build runs it again as long as it still fits the current file. When it doesn't (say you switched to a file in another language), Build goes back to choosing automatically.

Only one build runs at a time in each window. Starting a build stops one that is still running. Builds also stop when you close the window or quit.

Builds don't take keyboard input. To run a program that asks for input, use the [Terminal](terminal.md).

### Save All on Build

Tools ▸ Save All on Build is on by default: before each build, Strata saves every modified file that already has a name on disk. Untitled files aren't saved. Turn it off in the Tools menu ("Tools: Save All on Build" in the palette, or the `save_all_on_build` setting) if you'd rather save yourself.

## How Strata Chooses a Build System

Each build system says which files it fits:

- by syntax or file extension, such as Rust files or `.py` files, and/or
- by a marker file, such as `Cargo.toml` or `Makefile`, in the file's folder or one of the folders above it. Strata looks up to the folder open in the side bar, and never treats your home folder or the top of the disk as a project.

A build system that fits gets 2 points for matching the file and 2 points for finding its marker file, plus its own `priority`. The highest score wins. On a tie, your own build systems win over the built-in ones, and built-in ones win in the order of the table below.

For example, a `.rs` file inside a Cargo project fits both Cargo and Rust with the same score, and Cargo wins because it comes first. A `.js` file in a folder with `package.json` fits both Node and npm; Node wins, and you can use Build With… to run npm instead.

If nothing fits, the status bar says "No build system for this file".

### Choosing One Yourself

Tools ▸ Build System lists Automatic and every build system by name. Pick one to pin it: Build then always runs that system's main command, as long as it fits the current file; otherwise Build falls back to the usual choice. Pick Automatic to unpin. Picking an entry here (Automatic included) also forgets the last Build With… choice, so Automatic really means "the best fit for this file".

The pinned choice applies to the current window and isn't kept when you quit.

## Built-in Build Systems

| Name | Runs | Used for | Variants |
|---|---|---|---|
| Cargo | `cargo build` | Any file inside a folder with `Cargo.toml` | Run (`cargo run`), Test (`cargo test`), Clippy (`cargo clippy --all-targets`), Check (`cargo check`), Release (`cargo build --release`) |
| Rust | `rustc $file` | Rust files (`.rs`) | Run: compiles the file, then runs the program |
| Go | `go run $file` | Go files (`.go`) | Build (`go build`), Test (`go test`) |
| Python | `python3 -u $file` (on Windows, `py -u $file`) | Python files (`.py`, `.pyw`) | |
| Node | `node $file` | JavaScript files (`.js`, `.mjs`, `.cjs`) | |
| Ruby | `ruby $file` | Ruby files (`.rb`) | |
| Shell | `bash $file` | Shell scripts (`.sh`, `.bash`) | |
| Make | `make` | Any file inside a folder with `Makefile`, `makefile` or `GNUmakefile` | Clean (`make clean`), Test (`make test`) |
| C | `cc $file -o $file_base_name` (on Windows, `gcc` and an `.exe` name) | C files (`.c`) | Run: compiles the file, then runs the program |
| C++ | `c++ -std=c++17 $file -o $file_base_name` (on Windows, `g++` and an `.exe` name) | C++ files (`.cpp`, `.cc`, `.cxx`, `.c++`) | Run: compiles the file, then runs the program |
| npm | `npm run build` | Any file inside a folder with `package.json` | Test (`npm test`), Start (`npm start`) |

Cargo, Make and npm run in the folder that holds their marker file. The others run in the current file's folder.

The tools themselves (Cargo, Python, a C compiler and so on) must be installed. On macOS and Linux, Strata adds the usual tool folders to the search path if they exist: `/opt/homebrew/bin`, `/usr/local/bin`, and `.cargo/bin`, `.local/bin`, `go/bin` and `.bun/bin` in your home folder.

## The Build Results Panel

Starting a build opens the Build Results panel at the bottom of the window. Its header shows:

- a spinner and the elapsed time while the build runs, then how it finished;
- the build's name, such as "Cargo - Test";
- the command that ran (hover over it to see the folder it ran in);
- the number of results found, with ▲ and ▼ buttons to step through them;
- Copy, to copy all the output, Cancel, to stop the build, and × to hide the panel.

Output appears as it is printed, with the program's normal and error output in the order they were written. Lines that look like errors are shown in your scheme's error color and warnings in its warning color. Terminal color codes are removed, progress lines that redraw themselves are shown in their final state, and the panel keeps the last 10,000 lines. The panel follows new output unless you scroll up.

When the build ends, a final line says how:

| Last line | Meaning |
|---|---|
| [Finished in 1.2s] | The command succeeded. |
| [Finished in 1.2s with exit code 1] | The command failed with that exit code. |
| [Finished in 1.2s, terminated by a signal] | The command was killed from outside. |
| [Cancelled] | You cancelled the build. |

To show or hide the panel, use Tools ▸ Build Results ▸ Show Build Results, View ▸ Show Panel ▸ Build Results, or {{key:toggle_build_results}} ("Tools: Build Results: Show/Hide"). Drag its top edge to resize it. It is hidden in distraction-free mode.

## Jumping to Errors

Output lines that the build system recognizes as a file location, such as `src/main.rs:12:5: error…`, are results. Hover over a result to underline it and see its message, and click it to open the file at that line and column. Relative paths are read from the folder the build ran in.

To step through results from the keyboard, use Tools ▸ Build Results ▸ Next Result ({{key:next_result}}) and Previous Result ({{key:prev_result}}). Both wrap around, open the file at the result, and highlight the result in the panel.

Each built-in build system recognizes its tools' messages: Rust compiler locations, the `file:line:column: message` style of C compilers, Go and Make, Python tracebacks, Node stack traces, Ruby and bash errors, and TypeScript compiler errors for npm.

## Custom Build Systems

Your own build systems are JSON files in the `build_systems` folder inside Strata's settings folder:

| System | Folder |
|---|---|
| macOS | `~/Library/Application Support/Strata/build_systems` |
| Windows | `%APPDATA%\Strata\build_systems` |
| Linux | `~/.config/strata/build_systems` (or `$XDG_CONFIG_HOME/strata/build_systems`) |

Each `.json` file holds one build system, or a list of them in square brackets. A build system with the same name as a built-in one (ignoring case) replaces it.

Strata reads the folder again each time you run Build or Build With…, so changes take effect on your next build. Tools ▸ Build System lists the names it knew at the last build.

### Creating One

Choose Tools ▸ Build System ▸ New Build System… ("Tools: Build System: New Build System…" in the palette). Strata creates `untitled.json` (or `untitled-2.json` and so on) in the `build_systems` folder and opens it with this starting point:

```json
{
    "name": "My Build",
    "cmd": ["make"],
    "working_dir": "$folder",
    "selector": { "extensions": ["c", "h"] },
    "file_regex": "^(?P<file>[^:]+):(?P<line>\\d+):(?P<col>\\d+): (?P<message>.*)$"
}
```

Edit it, save, and run Build With… to try it. You can rename the file; only the `.json` extension matters.

> [!NOTE]
> Build system files are plain JSON: comments and trailing commas aren't allowed, and a misspelled field name is an error.

### Fields

| Field | Required | Meaning |
|---|---|---|
| `name` | Yes | The name shown in menus and the palette. |
| `cmd` | Yes | The program and its arguments, as a list of strings: `["cargo", "build"]`. |
| `shell` | No | `true` to run `cmd` through the shell instead (see below). Default `false`. |
| `working_dir` | No | The folder to run in. Default: `$project_path` if the selector has marker files, otherwise `$file_path` for a saved file, otherwise `$folder`. |
| `selector` | No | When the build system fits (see below). Leave it out and the build system fits every file. |
| `file_regex` | No | A regular expression that finds file locations in the output (see below). |
| `env` | No | Extra environment variables, as an object: `{ "RUST_BACKTRACE": "1" }`. Values can use variables, including `$PATH`. |
| `priority` | No | A number added to the build system's score when Strata chooses automatically. Default `0`. Use a higher number to prefer it. |
| `variants` | No | A list of alternative commands, shown in Build With… as "Name - Variant". |

### Selector

| Field | Meaning |
|---|---|
| `syntaxes` | Syntax names, as shown in View ▸ Syntax, such as `"Rust"` or `"Python"` (case doesn't matter). |
| `extensions` | File extensions without the dot, such as `"py"`. |
| `files` | Marker file names, such as `"Cargo.toml"`. |

If `syntaxes` or `extensions` is given, the current file must match one of them (either list will do). If `files` is given, one of those files must exist in the file's folder or a folder above it, up to the folder open in the side bar.

### Variants

Each variant needs a `name` and can change `cmd`, `working_dir`, `env`, `shell` and `file_regex`. Anything a variant doesn't set comes from the main build system; its `env` is added to the main one.

### Running Through the Shell

With `"shell": true`, the parts of `cmd` are joined with spaces and run as one command line by `sh -c` on macOS and Linux, or `cmd /S /C` on Windows. Use this for pipes, `&&`, wildcards, or commands like `npm` that are scripts on Windows. Variable values are quoted for the shell, so paths with spaces work.

Without the shell, the program in the first element runs directly with the other elements as its arguments.

### Variables

You can use these in `cmd`, `working_dir` and `env`, written `$name` or `${name}`:

| Variable | Value |
|---|---|
| `$file` | Full path of the current file. |
| `$file_path` | The folder that contains the current file. |
| `$file_name` | The file's name, such as `main.rs`. |
| `$file_base_name` | The name without its extension, such as `main`. |
| `$file_extension` | The extension without the dot, such as `rs`. |
| `$folder` | The folder open in the side bar. |
| `$project_path` | The folder that holds the selector's marker file; otherwise `$folder`; otherwise `$file_path`. |
| `$project_name` | The last part of `$project_path`. |
| `$exe` | `.exe` on Windows, nothing elsewhere. |

Write `$$` for a literal `$`. Any other name is read from the environment (such as `$HOME` or `$PATH`), and is left as written if it isn't set.

Using a `$file…` variable in a file that has never been saved, or `$folder` when no folder is open, stops the build with a message instead of running it.

### File Regex

`file_regex` is matched against each line of output. Mark its parts with named groups:

| Group | Meaning |
|---|---|
| `(?P<file>…)` | The file path, absolute or relative to the folder the build ran in. Required. |
| `(?P<line>…)` | The line number, counting from 1. Required. |
| `(?P<col>…)` | The column, counting from 1. Optional. |
| `(?P<message>…)` | The message to show when you hover over the result. Optional. |

If the expression has no group named `file`, Strata uses the first four plain groups, in the order file, line, column, message. Remember to double every backslash inside a JSON string, as in `\\d+`.

When a matching line has no message, Strata uses the most recent line that started with "error" or "warning", which suits compilers that print the message above the location.

### A Complete Example

```json
{
  "name": "My Make",
  "cmd": ["make", "-j8"],
  "working_dir": "$project_path",
  "selector": { "syntaxes": ["C", "C++"], "extensions": ["c", "h"], "files": ["Makefile"] },
  "file_regex": "^(?P<file>[^:]+):(?P<line>\\d+):(?P<col>\\d+): (?P<message>.*)$",
  "env": { "CFLAGS": "-O2", "PATH": "/opt/tools/bin:$PATH" },
  "shell": false,
  "priority": 1,
  "variants": [
    { "name": "Clean", "cmd": ["make", "clean"] },
    { "name": "Run", "cmd": ["make run && ./a.out"], "shell": true }
  ]
}
```

This build system fits C files and headers in a folder with a `Makefile`. Because it matches both the file and a marker file (and adds a priority of 1), it scores higher than the built-in Make and C systems when Strata chooses automatically. Build With… offers "My Make", "My Make - Clean" and "My Make - Run".

## When Something Goes Wrong

Problems are reported in the status bar, and problems with a build also appear in the Build Results panel.

| Message | What to do |
|---|---|
| Build systems: untitled.json: … | The file isn't valid JSON, or doesn't have the shape Strata expects (for example, a misspelled field name or a string where a list belongs). Your other build systems still load. |
| Build systems: untitled.json: a build system needs a "name" and a "cmd" | Add the missing field. |
| Build systems: My Make: bad file_regex: … | Fix the regular expression. |
| No build system for this file | Nothing fits the current file. Use Build With…, or add a selector to your build system. |
| $file: this build needs a saved file | Save the file first. |
| $folder: this build needs an open folder | Open a folder, or use `$file_path` instead. |
| My Make: working directory … does not exist | Check `working_dir`. |
| Could not run make: … | The program isn't installed or isn't on your search path. |

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `save_all_on_build` | `true` | Save every modified file before building. Tools ▸ Save All on Build changes it. |

See [Settings](settings.md) for how to edit `settings.json`.
