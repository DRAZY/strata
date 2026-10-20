# Settings Reference

Strata keeps your preferences in one file, `settings.json`. You can change them in the Settings window, or edit the file directly. This page explains both and lists every setting.

## The Settings window

Press {{key:settings}}, or choose **Strata ▸ Settings ▸ Settings…** on macOS (**Preferences ▸ Settings…** on Windows and Linux), or run **Preferences: Settings…** from the command palette.

The window lists every setting by category: Appearance, Editor, Files & Saving, Find & Spelling, Terminal & Build, Side Bar, Help and Advanced. Each row has the setting's name, a short description, its control, and its key in `settings.json` (click the key to copy it). The search box at the top ({{key:show_find}}) looks through every category at once, matching names, keys and descriptions.

- **Changes apply at once**, in every open window, and are saved to `settings.json` straight away, the same way a menu toggle is: only that setting is written, and your comments and the rest of the file are kept (see [When Strata writes the file](#when-strata-writes-the-file)).
- **A bar in the margin** marks settings that differ from their default; **Reset** next to the name puts the default back.
- **Numbers** stay within the range Strata uses. If `settings.json` holds a value outside it, the row says so.
- **Color overrides** and other settings too structured for a form have an **Edit in settings.json** button that opens the file at that setting. Settings for one language at a time are edited in `settings.json` too.
- **If `settings.json` has an error**, the window shows it and doesn't let you change anything until it's fixed, so the broken file is never overwritten. Fix the file and save it; the window follows.

Use Tab to move between controls, and Esc to clear the search or close the window. On platforms with separate windows it opens in its own window; otherwise it opens over the editor.

## Opening settings.json

To edit the file itself, click **Open settings.json** in the Settings window, choose **Strata ▸ Settings ▸ Settings (JSON)** on macOS (**Preferences ▸ Settings (JSON)** on Windows and Linux), or run **Preferences: Settings (JSON)** from the command palette. To give it a shortcut, bind `edit_settings` in [Key Bindings](keybindings.md).

The file lives in Strata's config folder, next to `keybindings.json`, your session and the `Themes`, `Syntaxes` and `Snippets` folders. On macOS that is `~/Library/Application Support/Strata`, on Windows `%APPDATA%\Strata`, and on Linux `~/.config/strata` (or `$XDG_CONFIG_HOME/strata`). [Troubleshooting](troubleshooting.md) has the details.

If the file doesn't exist yet, Strata writes it first, with every setting filled in at its current value (on a fresh install, the defaults). That way you can see every setting and change the ones you want.

## How the file works

The file is JSON, with two relaxations: comments (`//` and `/* */`) and trailing commas are allowed.

```json
{
  // Bigger text
  "font_size": 16,
  "rulers": [80, 100,],
  "theme": "Nord",
}
```

You only need the settings you want to change. Anything you leave out uses its default.

### Changes apply when you save

When you save `settings.json` in Strata, the new settings take effect immediately and the status bar says "Settings reloaded". Strata also notices when another program changes the file, within about a second. Every open window picks up the change.

If the default indentation changed (`tab_size` or `translate_tabs_to_spaces`), it is applied to every open file, replacing indentation you set for a tab by hand. Files whose indentation was detected, or comes from a syntax's settings or an `.editorconfig`, keep it (see [Which setting wins](#which-setting-wins)).

### When the file has an error

If `settings.json` can't be read, for example because of a missing quote or comma, nothing is lost:

- At startup, Strata uses the default settings and shows the error in the status bar.
- If you save a broken file while Strata is running, Strata keeps the settings it already had and shows the error with "(keeping current settings)".
- Strata never overwrites a file it can't read. If you change a setting from a menu while the file is broken, the status bar says "Could not write settings: settings.json has errors; fix them first so your settings aren't overwritten". Fix the error and save, and everything works again.

A value of the wrong type (for example `"font_size": "big"`) counts as an error for the whole file.

### When Strata writes the file

The Settings window, and menus and commands that change a setting (View ▸ Editor Extras, the Color Scheme menu, zooming, the font pickers and so on), save it to `settings.json` right away. When Strata writes the file:

- Only the setting that changed is touched: its value is replaced where it is, or, if the file doesn't mention it yet, it is added after the last key. Comments, blank lines, the order of keys and keys Strata doesn't know are all kept, so you can keep notes or settings for other tools there.
- Settings left at their defaults are not written out.
- If the file doesn't exist yet (opening it from the menu creates it), it starts with every setting and its current value.

## Settings that are not in settings.json

Some things you change apply to one file only and are not settings:

- **Tab width and spaces vs tabs** from **View ▸ Indentation** or the indentation menu in the status bar change the current file only. `tab_size` and `translate_tabs_to_spaces` are the defaults for new files and for files whose indentation can't be detected. See [Which setting wins](#which-setting-wins).
- **Word wrap** from **View ▸ Word Wrap** ({{key:toggle_word_wrap}}) turns wrapping on or off for the current file. `word_wrap` is the default for all files.
- **Syntax** from **View ▸ Syntax** applies to the current file (see [Languages and Syntaxes](languages.md)).
- **Encoding and line endings** belong to each file (see [Encodings and Line Endings](encodings.md)).

Key bindings are kept in their own file, `keybindings.json` (see [Key Bindings](keybindings.md)). Open tabs, panes, the side bar and recent files are part of your session, not your settings.

## Settings changed from menus

Every setting is in the Settings window, and many also have a menu item:

| Menu | Settings |
|------|----------|
| View ▸ Color Scheme, the status bar's Theme menu | `theme`, `theme_mode`, `dark_theme`, `light_theme` |
| Strata ▸ Settings ▸ Font (Preferences ▸ Font on Windows and Linux) | `font_face`, `font_size`, `line_height` (and resets `line_padding_top` and `line_padding_bottom`), `ui_font_face`, `ui_font_size` |
| View ▸ Editor Extras | `draw_white_space`, `highlight_trailing_whitespace`, `bracket_colors`, `color_swatches`, `sticky_scroll`, `smooth_scroll`, `git_gutter` |
| View ▸ Cursor | `caret_style`, `caret_blink`, `caret_width` |
| View ▸ Window Opacity | `window_opacity` |
| View ▸ Toggle Minimap, Toggle Tabs, Toggle Status Bar | `minimap`, `show_tabs`, `show_status_bar` |
| View ▸ Markdown Preview | `markdown_preview` |
| Tools ▸ Notes ▸ Backlinks, and the × in the backlinks panel | `notes_backlinks` |
| View ▸ Word Wrap Column, View ▸ Ruler | `wrap_column`, `rulers` |
| Tools ▸ Save All on Build | `save_all_on_build` |
| Tools ▸ On Save | `trim_trailing_white_space_on_save`, `ensure_newline_at_eof_on_save`, `save_on_focus_lost` |
| View ▸ Terminal, and resizing the terminal panel | `terminal_visible`, `terminal_height` |
| View ▸ Spell Check, View ▸ Dictionary, and Add Word to Dictionary in the editor's right-click menu | `spell_check`, `dictionary`, `added_words` |

## Settings for one syntax

`syntax_settings` gives a language its own settings. Its keys are syntax names, as the status bar shows them (case doesn't matter), and each holds the settings that differ for that syntax:

```json
{
  "syntax_settings": {
    "Python": { "tab_size": 4, "rulers": [79] },
    "YAML": { "tab_size": 2, "translate_tabs_to_spaces": true },
    "Markdown": { "word_wrap": true, "spell_check": true },
  },
}
```

These settings can be set per syntax: `tab_size`, `translate_tabs_to_spaces`, `detect_indentation`, `word_wrap`, `wrap_column` (`0` wraps at the window edge), `wrap_width`, `rulers`, `trim_trailing_white_space_on_save`, `ensure_newline_at_eof_on_save`, `auto_indent`, `smart_indent`, `spell_check`, `auto_complete`, `draw_white_space` and `highlight_trailing_whitespace`. Other keys are ignored. A value of the wrong type is an error, as anywhere in the file.

A tab picks up its syntax's settings when it opens and again whenever its syntax changes, for example from the syntax menu in the status bar.

### Built-in syntax settings

Strata comes with a few settings of its own for some syntaxes. Like a syntax's settings, they take precedence over the rest of `settings.json`, and your `syntax_settings` for that syntax take precedence over them:

| Syntax | Built-in settings |
|--------|-------------------|
| Makefile | `"translate_tabs_to_spaces": false`, `"detect_indentation": false` (recipes must start with a tab) |
| Go | `"translate_tabs_to_spaces": false` (gofmt indents with tabs) |
| YAML | `"tab_size": 2`, `"translate_tabs_to_spaces": true` |
| Markdown, MultiMarkdown, Plain Text, reStructuredText, Textile, LaTeX | `"word_wrap": true` |

For example, `"syntax_settings": {"Markdown": {"word_wrap": false}}` stops Markdown files from wrapping.

## EditorConfig

Strata follows [EditorConfig](https://editorconfig.org) files, so a project's `.editorconfig` sets the indentation and line endings of the files in it. When a file opens, Strata reads every `.editorconfig` from the file's folder up to the top of the disk, stopping at one that says `root = true`. Sections whose glob matches the file apply; later sections, and files in nearer folders, take precedence. All of the spec's glob patterns work: `*`, `**`, `?`, `[abc]`, `[!abc]`, `{a,b}` and `{1..10}`.

| Property | What Strata does |
|----------|------------------|
| `indent_style` | `space` or `tab`: indent with spaces or tabs. |
| `indent_size`, `tab_width` | The indentation width. Strata has one width for indentation and tab stops: `indent_size` when it's a number, otherwise `tab_width`. |
| `end_of_line` | `lf` or `crlf`: new files start with it, and saving converts the file's line breaks to it, unless you picked a line ending for that tab in the status bar. `cr` isn't supported and is ignored. |
| `charset` | `utf-8`, `utf-8-bom`, `latin1`, `utf-16le` or `utf-16be` (UTF-16 is written with a byte order mark): the encoding of new files, and of a file saved somewhere for the first time. Existing files keep the encoding they were read in. |
| `trim_trailing_whitespace` | `true` or `false`: overrides `trim_trailing_white_space_on_save`. |
| `insert_final_newline` | `true` or `false`: overrides `ensure_newline_at_eof_on_save`. With `false`, Strata doesn't add a final newline, but doesn't remove one either. |
| `max_line_length` | A number: a ruler at that column, the column Wrap Paragraph at Ruler uses, and the soft-wrap column when word wrap is on. `off` changes nothing. |

A property set to `unset` is removed, as if no section had set it. Other properties are ignored.

Strata notices within about a second when an `.editorconfig` is created, changed or deleted, and at once when you save one in Strata. Saving a file under a new name, or renaming it in the side bar, applies the `.editorconfig` settings of its new place. The status bar's indentation menu says when the indentation came from an `.editorconfig`, and which one.

To ignore `.editorconfig` files, set `"editorconfig": false`.

## Which setting wins

Each file's settings come from several places. From lowest to highest precedence:

1. Strata's defaults.
2. Your `settings.json`.
3. Strata's [built-in syntax settings](#built-in-syntax-settings).
4. Your `syntax_settings` for the file's syntax.
5. The indentation detected in the file, when `detect_indentation` is on (the width only when the file indents with spaces).
6. `.editorconfig`.
7. What you set for that tab: the indentation from the status bar, View ▸ Indentation or Guess Settings From Buffer; word wrap from View ▸ Word Wrap; the line ending from the status bar; spell check from View ▸ Spell Check.

A choice made for a tab stays when its syntax changes or the settings change, except that a new `tab_size` or `translate_tabs_to_spaces` in `settings.json` replaces indentation set by hand. View ▸ Spell Check also saves the new state as `spell_check`; a tab whose syntax's settings say otherwise keeps the choice for itself.

## All settings

Every setting Strata reads, with its default value. Text values are written in quotes, as they appear in the file.

### Fonts and text size

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `font_face` | text | `""` | The editor font: a bundled font's name, an installed font's name, or the path to a font file. Empty means the built-in font (Hack). See [Fonts](appearance.md#fonts). |
| `font_size` | number | `14` | Editor text size in points. Also used for the terminal and the Markdown preview. Zoom In and Zoom Out change it in 1-point steps between 8 and 48; Reset Zoom sets it back to 14. |
| `ui_font_face` | text | `""` | The interface font, used for menus, tabs, the side bar and panels. Same values as `font_face`. Empty means the built-in font (Ubuntu). |
| `ui_font_size` | number | `14` | Interface text size in points, from 9 to 24: menus, panels, the Settings window and dialogs. View ▸ Interface Size offers Small (12), Default (14), Large (16) and Larger (18). |
| `font_options` | list of text | `[]` | Styles to drop from syntax highlighting: `"no_italic"`, `"no_underline"`. `"no_bold"` is accepted but changes nothing, since code is never bold. |
| `line_height` | number | `1.0` | Line height as a multiple of the font's own, from 0.8 to 3.0. |
| `line_padding_top` | number | `0` | Extra space above each line, in points (0 to 40). |
| `line_padding_bottom` | number | `0` | Extra space below each line, in points (0 to 40). |

### Color scheme

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `theme` | text | `"Strata Dark"` | The color scheme used when `theme_mode` is `"fixed"`. Any name from the Color Scheme menu; case doesn't matter. `"auto"` follows the system appearance. See [Color schemes](appearance.md#color-schemes). |
| `theme_mode` | text | `"fixed"` | `"fixed"` uses `theme`. `"auto"` (or `"system"`) follows the system's light or dark mode with `light_theme` and `dark_theme`. `"dark"` always uses `dark_theme`; `"light"` always uses `light_theme`. |
| `dark_theme` | text | `"Strata Dark"` | The scheme for dark mode (and for `"dark"`). |
| `light_theme` | text | `"Strata Light"` | The scheme for light mode (and for `"light"`). |
| `theme_overrides` | object | `{}` | Colors laid over every scheme, for example `{"background": "#1e1e2e", "comment": "#7f849c"}`. See [Custom colors](appearance.md#custom-colors) for the names and color formats. |
| `theme_overrides_by_theme` | object | `{}` | Colors for one scheme at a time, for example `{"Nord": {"selection": "#434c5e"}}`. Applied after `theme_overrides`. |

### Cursor

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `caret_style` | text | `"line"` | Cursor shape: `"line"`, `"block"` or `"underline"`. |
| `caret_blink` | text | `"blink"` | `"blink"`, `"smooth"` (fades in and out) or `"solid"` (no blinking). |
| `caret_width` | number | `2` | Width of the line cursor in points, from 1 to 6. |

### Editor display

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `draw_white_space` | text | `"selection"` | Where to show dots for spaces and arrows for tabs: `"selection"` (inside selected text), `"all"` or `"none"`. |
| `highlight_trailing_whitespace` | true/false | `true` | Tint spaces and tabs at the ends of lines. |
| `bracket_colors` | true/false | `true` | Color bracket pairs by nesting depth. |
| `color_swatches` | true/false | `true` | Show a small color bar under color values such as `#22d3ee` or `rgb(…)`. |
| `sticky_scroll` | true/false | `true` | Pin the headers of the blocks you are inside at the top while scrolling. |
| `smooth_scroll` | true/false | `true` | Animate scrolling. |
| `git_gutter` | true/false | `true` | Show bars in the gutter for lines changed since the last Git commit. |
| `minimap` | true/false | `true` | Show the minimap at the right edge of the editor. |
| `rulers` | list of numbers | `[]` | Draw vertical guides at these columns, for example `[80, 100]`. |
| `word_wrap` | true/false | `false` | Soft-wrap every file. Markdown, MultiMarkdown, Plain Text, reStructuredText, Textile and LaTeX files wrap even when this is off (see [Built-in syntax settings](#built-in-syntax-settings)). View ▸ Word Wrap overrides it for one file. |
| `wrap_column` | number or `null` | `null` | Wrap at this column instead of the window edge (`null` means the window edge). Lines still wrap at the window edge if it comes first. |
| `large_file_mb` | number | `20` | Files bigger than this many megabytes open in large-file mode: as Plain Text, without the minimap, word wrap, bracket colors or sticky scroll. Choosing a syntax still highlights them. Files with a line longer than 10,000 characters always open this way. See [Very large files](troubleshooting.md#very-large-files). |
| `large_file_warning_mb` | number | `500` | Ask before opening a text file bigger than this many megabytes. Binary files open in the hex view without asking (see [Binary files](encodings.md#binary-files)). |

### Window

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `window_opacity` | number | `1.0` | From 0.3 (see-through) to 1.0 (opaque). |
| `help_in_own_window` | true/false | `true` | Open the documentation in its own window, so you can read along while you work in the editor. The Pop Out / Dock button in the help toolbar switches it. |
| `show_tabs` | true/false | `true` | Show the tab bar. |
| `show_status_bar` | true/false | `true` | Show the status bar. |
| `markdown_preview` | true/false | `false` | Show the live Markdown preview beside the editor. See [Markdown](markdown.md). |
| `notes_backlinks` | text | `"auto"` | When the backlinks panel shows under a note: `"auto"` (notes in an Obsidian vault, or in a folder whose notes use wikilinks), `"always"` or `"never"`. See [Notes](notes.md#backlinks). |
| `sidebar_respect_gitignore` | true/false | `true` | The side bar's tree hides files and folders that `.gitignore` (and `.ignore`, `.git/info/exclude` and your global Git excludes) ignore, as Goto Anything and Find in Files do. See [Projects and Side Bar](projects.md#what-the-tree-shows). |
| `file_exclude_patterns` | list of text | `[".git", ".DS_Store"]` | Names the side bar never shows, files or folders, as globs: for example `["*.pyc", "node_modules", ".git", ".DS_Store"]`. |

### AI helpers

These settings choose the model the [AI helpers](ai.md) use. Nothing is sent anywhere while `ai_enabled` is `false`. API keys are not settings: they're kept in the system keychain (see [API keys](ai.md#api-keys)).

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `ai_enabled` | true/false | `false` | Turn on the AI helpers. Until you do, their commands only point you here, and Strata sends nothing anywhere. |
| `ai_provider` | text | `"openai"` | Which kind of provider answers: `"openai"` (any OpenAI-compatible server: Ollama, LM Studio, OpenRouter, OpenAI and others), `"anthropic"` (the Anthropic API), `"claude_cli"` or `"codex_cli"` (the `claude` or `codex` command-line tool, with your own login). See [Providers](ai.md#providers). |
| `ai_base_url` | text | `"http://localhost:11434/v1"` | With `"openai"`: the server's address, up to and including `/v1`. The default is Ollama on this computer; LM Studio is `http://localhost:1234/v1`. Addresses that aren't on this computer or your local network must use `https://`. |
| `ai_model` | text | `""` | With `"openai"`: the model to use, as the server names it (for example `llama3.2`). Test Connection lists the server's models. |
| `ai_anthropic_model` | text | `"claude-sonnet-5-5"` | With `"anthropic"`: the model to use. |
| `ai_cli_model` | text | `""` | With `"claude_cli"` or `"codex_cli"`: a model to ask the tool for. Empty uses the tool's own default. |
| `ai_max_prompt_kb` | whole number | `200` | The most a helper sends, in KB (8 to 4096). Bigger reviews are cut to fit, and the panel says what was left out. |
| `ai_timeout_seconds` | whole number | `180` | How long Strata waits when a provider sends nothing, before it stops the request (10 to 3600). Command-line tools print their whole answer at the end, so for them this covers the whole answer. |

### Updates

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `update_check` | text | `"ask"` | Whether Strata checks GitHub for a new version: `"auto"` (every time Strata opens, then every `update_check_interval_hours` while it runs), `"off"` (manually only), or `"ask"` (Strata asks once whether to turn checks on). Help ▸ Check for Updates… works whatever this is. See [Updates](about.md#updates). |
| `update_check_interval_hours` | whole number | `4` | With `update_check` set to `"auto"`: how many hours Strata waits between checks while it stays open: `1`, `2`, `4`, `6` or `12`. Strata also checks every time it opens. |
| `update_channel` | text | `"stable"` | Which versions to offer: `"stable"` releases only, or `"prerelease"` to also offer prereleases (release candidates and betas). |

### Editing

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `tab_size` | whole number | `4` | Default indentation width for new files and for files whose indentation isn't detected. |
| `translate_tabs_to_spaces` | true/false | `true` | Indent with spaces (`true`) or tab characters (`false`) by default. |
| `detect_indentation` | true/false | `true` | When a file opens, detect whether it uses tabs or spaces, and how wide, and follow it. |
| `auto_indent` | true/false | `true` | Pressing Enter keeps the current line's indentation. |
| `smart_indent` | true/false | `true` | Language-aware indentation: indent after an opening bracket, a Python or YAML colon, or an opening tag, and line up closing brackets, `end`, `fi` and closing tags. Needs `auto_indent`. See [Languages and Syntaxes](languages.md#smart-indentation). |
| `auto_complete` | true/false | `true` | Offer word completions while you type. {{key:auto_complete}} shows completions even when this is off. |
| `wrap_width` | whole number | `80` | The column used by Edit ▸ Wrap ▸ Wrap Paragraph at Ruler ({{key:wrap_lines}}) when `rulers` is empty (otherwise the first ruler is used). |
| `syntax_settings` | object | `{}` | Settings for one syntax, by name: for example `{"Python": {"tab_size": 4}, "Markdown": {"spell_check": true}}`. See [Settings for one syntax](#settings-for-one-syntax). |
| `editorconfig` | true/false | `true` | Follow `.editorconfig` files. See [EditorConfig](#editorconfig). |

### Language servers

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `lsp_enabled` | true/false | `true` | Use language servers for completions, problems, hover information, rename and formatting, when one is installed for the file's language. Strata never downloads or installs a server. See [Language Servers](languages.md#language-servers). |
| `lsp_trust_all` | true/false | `false` | Start language servers in any project folder without asking. Servers can run a project's own code (build scripts, procedural macros, configured tools), so by default Strata asks once per folder. See [Trusting a project](languages.md#trusting-a-project). |
| `lsp_servers` | object | `{}` | Changes to the built-in language servers, and servers of your own, by name: for example `{"rust": {"args": ["--log-file", "/tmp/ra.log"]}, "go": {"enabled": false}}`. See [Setting up servers](languages.md#setting-up-servers). |

### Find and spelling

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `auto_find_in_selection` | true/false | `false` | Opening the find bar while a selection spans several lines turns on In Selection, so the search stays inside it. See [In Selection](find.md#in-selection). |
| `spell_check` | true/false | `false` | Underline misspelled words in comments, strings and prose. View ▸ Spell Check ({{key:toggle_spell_check}}) turns it on and off. See [Spell Check](editing.md#spell-check). |
| `dictionary` | text | `"en_US"` | The dictionary spell check uses: `"en_US"` (American English) or `"en_GB"` (British English). |
| `added_words` | list of text | `[]` | Words spell check always accepts, for example `["Strata", "egui"]`. Add Word to Dictionary in the right-click menu adds to it. A word in lower case also covers its capitalized form. |
| `spelling_selector` | text | see below | Which text is checked, as a scope selector (see [Spell Check](editing.md#spell-check)). The default checks comments and strings in code, and prose elsewhere apart from code, links and tags. |

The default `spelling_selector` is `"source string.quoted - punctuation - meta.preprocessor.include, source comment - source comment.block.preprocessor, -(source, constant, keyword, storage, support, variable, markup.underline.link, markup.raw, meta.tag)"`.

### Saving

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `trim_trailing_white_space_on_save` | true/false | `false` | Remove spaces and tabs at the ends of lines when saving. |
| `ensure_newline_at_eof_on_save` | true/false | `false` | Add a line break at the end of the file when saving, if it doesn't end with one. |
| `save_on_focus_lost` | true/false | `false` | Save every modified file that has a name when the Strata window loses focus. |
| `auto_save_delay_ms` | number | `0` | Auto save: after this many milliseconds without typing (for example `1000`), save the files you edited. `0` turns it off. Only files Save would write without asking are saved: never an untitled tab, a read-only file, a file changed or deleted by another program, one with decoding errors, or text its encoding can't store. The trim and newline options above apply (trimming skips the lines with a cursor, so a space you just typed stays), and the status bar says "Auto-saved". |
| `hot_exit` | true/false | `true` | Quit without asking: open tabs, including unsaved text, come back next time. When `false`, Strata asks about unsaved changes on quit and starts with a fresh window. |

### Build and terminal

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `save_all_on_build` | true/false | `true` | Save every modified file before running a build. See [Build Systems](build.md). |
| `terminal_visible` | true/false | `false` | Whether the terminal panel is open. Strata updates this itself; when it's `true`, a shell starts when Strata launches. See [Terminal](terminal.md). |
| `terminal_height` | number | `240` | Height of the terminal panel in points. Strata remembers it when you resize the panel. |
| `terminal_allow_clipboard_read` | true/false | `false` | Let programs in the terminal read the clipboard with the OSC 52 escape sequence. Off by default because any program (or a file you `cat`) could then read what you copied. See [Terminal](terminal.md#copy-paste-and-clear). |
| `terminal_allow_clipboard_write` | true/false | `true` | Let programs in the terminal set the clipboard with OSC 52, as most terminals do (handy over SSH or in tmux). The status bar says so each time. Turn it off to block it. See [Terminal](terminal.md#copy-paste-and-clear). |
| `terminal_option_as_meta` | true/false | `false` on macOS, `true` elsewhere | Alt (Option on macOS) works as the Meta key in the terminal: Alt+B sends Esc followed by B. When `false`, Alt or Option types the character your keyboard layout gives (é, @, [ and so on); Alt with keys that type nothing, such as Backspace and the arrows, still reaches the program. See [Terminal](terminal.md#alt-and-option-as-meta). |
| `code_block_timeout_seconds` | whole number | `60` | How long a code block run with Notes: Run Code Block and Capture Output may take before Strata stops it, its background jobs and the programs still running under it (1 to 3600). The output so far still goes into the `output` block. See [Notes](notes.md#running-code-blocks). |
