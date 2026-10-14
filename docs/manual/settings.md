# Settings Reference

Strata keeps your preferences in one file, `settings.json`. This page explains how that file works and lists every setting it accepts.

## Opening settings.json

Open it with **Strata ▸ Settings ▸ Settings** on macOS, or **Preferences ▸ Settings** on Windows and Linux. You can also run **Preferences: Settings** from the command palette, or press {{key:edit_settings}}.

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

If the default indentation changed (`tab_size` or `translate_tabs_to_spaces`), it is applied to every open file.

### When the file has an error

If `settings.json` can't be read, for example because of a missing quote or comma, nothing is lost:

- At startup, Strata uses the default settings and shows the error in the status bar.
- If you save a broken file while Strata is running, Strata keeps the settings it already had and shows the error with "(keeping current settings)".
- Strata never overwrites a file it can't read. If you change a setting from a menu while the file is broken, the status bar says "Could not write settings: settings.json has errors; fix them first so your settings aren't overwritten". Fix the error and save, and everything works again.

A value of the wrong type (for example `"font_size": "big"`) counts as an error for the whole file.

### When Strata writes the file

Menus and commands that change a setting (View ▸ Editor Extras, the Color Scheme menu, zooming, the font pickers and so on) save it to `settings.json` right away. When Strata writes the file:

- Only the setting that changed is touched: its value is replaced where it is, or, if the file doesn't mention it yet, it is added after the last key. Comments, blank lines, the order of keys and keys Strata doesn't know are all kept, so you can keep notes or settings for other tools there.
- Settings left at their defaults are not written out.
- If the file doesn't exist yet (Preferences ▸ Settings creates it), it starts with every setting and its current value.

## Settings that are not in settings.json

Some things you change apply to one file only and are not settings:

- **Tab width and spaces vs tabs** from **View ▸ Indentation** or the indentation menu in the status bar change the current file only. `tab_size` and `translate_tabs_to_spaces` are the defaults for new files and for files whose indentation can't be detected.
- **Word wrap** from **View ▸ Word Wrap** ({{key:toggle_word_wrap}}) turns wrapping on or off for the current file. `word_wrap` is the default for all files.
- **Syntax** from **View ▸ Syntax** applies to the current file (see [Languages and Syntaxes](languages.md)).
- **Encoding and line endings** belong to each file (see [Encodings and Line Endings](encodings.md)).

Key bindings are kept in their own file, `keybindings.json` (see [Key Bindings](keybindings.md)). Open tabs, panes, the side bar and recent files are part of your session, not your settings.

## Settings changed from menus

Many settings have a menu item, so you rarely need to edit the file for them:

| Menu | Settings |
|------|----------|
| View ▸ Color Scheme, the status bar's Theme menu | `theme`, `theme_mode`, `dark_theme`, `light_theme` |
| Strata ▸ Settings ▸ Font (Preferences ▸ Font on Windows and Linux) | `font_face`, `font_size`, `line_height` (and resets `line_padding_top` and `line_padding_bottom`), `ui_font_face`, `ui_font_size` |
| View ▸ Editor Extras | `draw_white_space`, `highlight_trailing_whitespace`, `bracket_colors`, `color_swatches`, `sticky_scroll`, `smooth_scroll`, `git_gutter` |
| View ▸ Cursor | `caret_style`, `caret_blink`, `caret_width` |
| View ▸ Window Opacity | `window_opacity` |
| View ▸ Toggle Minimap, Toggle Tabs, Toggle Status Bar | `minimap`, `show_tabs`, `show_status_bar` |
| View ▸ Markdown Preview | `markdown_preview` |
| View ▸ Word Wrap Column, View ▸ Ruler | `wrap_column`, `rulers` |
| Tools ▸ Save All on Build | `save_all_on_build` |
| Tools ▸ On Save | `trim_trailing_white_space_on_save`, `ensure_newline_at_eof_on_save`, `save_on_focus_lost` |
| View ▸ Terminal, and resizing the terminal panel | `terminal_visible`, `terminal_height` |
| View ▸ Spell Check, View ▸ Dictionary, and Add Word to Dictionary in the editor's right-click menu | `spell_check`, `dictionary`, `added_words` |

## All settings

Every setting Strata reads, with its default value. Text values are written in quotes, as they appear in the file.

### Fonts and text size

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `font_face` | text | `""` | The editor font: a bundled font's name, an installed font's name, or the path to a font file. Empty means the built-in font (Hack). See [Fonts](appearance.md#fonts). |
| `font_size` | number | `14` | Editor text size in points. Also used for the terminal and the Markdown preview. Zoom In and Zoom Out change it in 1-point steps between 8 and 48; Reset Zoom sets it back to 14. |
| `ui_font_face` | text | `""` | The interface font, used for menus, tabs, the side bar and panels. Same values as `font_face`. Empty means the built-in font (Ubuntu). |
| `ui_font_size` | number | `13` | Interface text size in points, from 9 to 24. |
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
| `word_wrap` | true/false | `false` | Soft-wrap every file. Markdown, MultiMarkdown, Plain Text, reStructuredText, Textile and LaTeX files wrap even when this is off. View ▸ Word Wrap overrides it for one file. |
| `wrap_column` | number or `null` | `null` | Wrap at this column instead of the window edge (`null` means the window edge). Lines still wrap at the window edge if it comes first. |
| `large_file_mb` | number | `20` | Files bigger than this many megabytes open in large-file mode: as Plain Text, without the minimap, word wrap, bracket colors or sticky scroll. Choosing a syntax still highlights them. Files with a line longer than 10,000 characters always open this way. See [Very large files](troubleshooting.md#very-large-files). |
| `large_file_warning_mb` | number | `500` | Ask before opening a file bigger than this many megabytes. Strata also asks before opening a file that looks binary. |

### Window

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `window_opacity` | number | `1.0` | From 0.3 (see-through) to 1.0 (opaque). |
| `help_in_own_window` | true/false | `true` | Open the documentation in its own window, so you can read along while you work in the editor. The Pop Out / Dock button in the help toolbar switches it. |
| `show_tabs` | true/false | `true` | Show the tab bar. |
| `show_status_bar` | true/false | `true` | Show the status bar. |
| `markdown_preview` | true/false | `false` | Show the live Markdown preview beside the editor. See [Markdown](markdown.md). |

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
| `hot_exit` | true/false | `true` | Quit without asking: open tabs, including unsaved text, come back next time. When `false`, Strata asks about unsaved changes on quit and starts with a fresh window. |

### Build and terminal

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `save_all_on_build` | true/false | `true` | Save every modified file before running a build. See [Build Systems](build.md). |
| `terminal_visible` | true/false | `false` | Whether the terminal panel is open. Strata updates this itself; when it's `true`, a shell starts when Strata launches. See [Terminal](terminal.md). |
| `terminal_height` | number | `240` | Height of the terminal panel in points. Strata remembers it when you resize the panel. |
| `terminal_allow_clipboard_read` | true/false | `false` | Let programs in the terminal read the clipboard with the OSC 52 escape sequence. Off by default because any program (or a file you `cat`) could then read what you copied. Writing to the clipboard is always allowed. See [Terminal](terminal.md#copy-paste-and-clear). |
