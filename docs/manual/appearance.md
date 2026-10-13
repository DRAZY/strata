# Appearance

This page covers how Strata looks: color schemes, fonts, sizes, line spacing, the cursor, and the optional extras drawn in the editor. Most of it can be changed from menus, and everything is also a setting you can edit in `settings.json` (see [Settings Reference](settings.md#all-settings)).

## Where the appearance menus are

On macOS, the appearance commands live under **Strata ▸ Settings**: Color Scheme, and Font (Editor Font, Larger, Smaller, Reset Size, Line Spacing, Interface Font, Interface Size). On Windows and Linux the same items are in the **Preferences** menu.

The **View** menu has the rest: Color Scheme (the same menu again), Editor Extras, Cursor, Window Opacity, Ruler, Word Wrap Column, Toggle Minimap, Toggle Tabs, Toggle Status Bar, and Toggle Distraction Free.

## Color schemes

A color scheme sets the syntax colors and the editor colors (background, caret, selection, current line, line numbers, find matches, indent guides, bracket match and whitespace marks). The tabs, side bar, status bar, menus, scrollbar, minimap, terminal and Markdown preview all take their colors from the scheme too. The default is **Strata Dark**.

### Built-in schemes

The menus group the schemes like this, in this order:

| Group | Schemes |
|-------|---------|
| Dark | Strata Dark, One Dark, Dracula, Nord, Solarized Dark, Gruvbox Dark, Tokyo Night, Tokyo Night Storm, Catppuccin Mocha, Catppuccin Macchiato, Catppuccin Frappé, GitHub Dark, Monokai Extended, Ayu Dark, Ayu Mirage, Night Owl, Material Palenight, Rosé Pine, Rosé Pine Moon, Everforest Dark, Kanagawa Wave |
| Light | Strata Light, One Light, Solarized Light, Gruvbox Light, Tokyo Night Day, Catppuccin Latte, GitHub Light, Ayu Light, Night Owl Light, Rosé Pine Dawn, Everforest Light, Kanagawa Lotus |
| High Contrast | High Contrast Dark, High Contrast Light |
| Your Themes | Schemes you add yourself (see [Importing color schemes](#importing-color-schemes)). This group only appears when you have some. |
| Classic | InspiredGitHub, base16-eighties.dark, base16-mocha.dark, base16-ocean.dark, base16-ocean.light |

The credits and licenses for each palette are in **Help ▸ Licenses and Attribution…** ({{key:licenses}}).

### Picking a scheme

There are several ways to choose a scheme:

- **View ▸ Color Scheme**, then a group, then a scheme. The same menu is under **Strata ▸ Settings ▸ Color Scheme** on macOS and **Preferences ▸ Color Scheme** on Windows and Linux.
- The **Theme** menu in the status bar, at the bottom right. It has the same groups plus a **Follow system appearance** checkbox. Hover it to see the current scheme's name.
- The command palette: run **Preferences: Color Scheme…** ({{key:select_color_scheme}}), or choose **Select Color Scheme…** at the bottom of the Color Scheme menu. The list starts on your current scheme and shows each scheme's group beside its name.

In the palette list, moving the selection with the arrow keys previews each scheme right away. Press Enter to keep the one you are on, or Esc to go back to the scheme you had before.

Your choice is saved to `settings.json` straight away.

> [!NOTE]
> If `settings.json` names a scheme that doesn't exist (a typo, or a theme file you removed), the status bar says so and the current scheme stays on screen.

### Follow system appearance

Strata can switch between a light and a dark scheme when your system switches between light and dark mode. Turn it on with **View ▸ Color Scheme ▸ Follow System Appearance (Auto)** or the checkbox in the status bar's Theme menu. When you turn it on, the scheme you're using now becomes the light or dark choice, depending on whether it is a light or a dark scheme.

While it's on, picking a scheme from any menu or the palette fills the matching slot: a light scheme becomes the one used in light mode, a dark scheme the one used in dark mode. If you pick one that doesn't match the system's current mode, the status bar tells you when it will show.

The settings behind this are:

| Setting | What it does |
|---------|--------------|
| `theme_mode` | `"fixed"` (the default) always uses `theme`. `"auto"` (or `"system"`) follows the system, using `light_theme` in light mode and `dark_theme` otherwise. `"dark"` always uses `dark_theme`, and `"light"` always uses `light_theme`. |
| `theme` | The scheme used in fixed mode. Setting it to `"auto"` also follows the system, as in Sublime Text. |
| `dark_theme` | The dark-mode scheme. Default `"Strata Dark"`. |
| `light_theme` | The light-mode scheme. Default `"Strata Light"`. |

```json
{
  "theme_mode": "auto",
  "dark_theme": "Tokyo Night",
  "light_theme": "Catppuccin Latte",
}
```

If the system doesn't report light or dark mode, Auto uses `dark_theme`. Scheme names are matched without regard to case.

When `theme_mode` is `"dark"` or `"light"` and you pick a scheme of the other kind, Strata switches back to fixed mode and uses the scheme you picked.

## Custom colors

You can change individual colors without writing a whole scheme. `theme_overrides` applies to every scheme, and `theme_overrides_by_theme` applies to one scheme at a time (the scheme name is matched without regard to case). Per-scheme colors are applied after the general ones, so they win.

```json
{
  // Over every scheme
  "theme_overrides": { "background": "#1e1e2e", "comment": "#7f849c" },
  // Only for Nord
  "theme_overrides_by_theme": { "Nord": { "selection": "#434c5e" } },
}
```

Changes apply as soon as you save `settings.json`. An unknown name or a color Strata can't read is skipped and reported in the status bar.

### Color names you can override

Editor colors:

| Name | What it colors | Also accepted |
|------|----------------|---------------|
| `background` | Editor background | `bg` |
| `foreground` | Default text | `fg`, `text` |
| `caret` | The cursor | `cursor` |
| `selection` | Selected text background | |
| `line_highlight` | The current line | `current_line`, `line` |
| `gutter` | Line number area background | |
| `gutter_foreground` | Line numbers | `line_number`, `line_numbers`, `gutter_fg` |
| `find_highlight` | Find matches | `find`, `find_match` |
| `guide` | Indent guides | `indent_guide`, `indent_guides`, `guides` |
| `bracket` | Matching bracket | `brackets`, `brackets_foreground`, `bracket_match` |
| `whitespace` | Visible whitespace marks | `invisibles` |
| `accent` | Accent color | |

Syntax colors:

| Name | Also accepted |
|------|---------------|
| `comment` | `comments` |
| `string` | `strings` |
| `escape` | |
| `keyword` | `keywords` |
| `operator` | |
| `function` | `functions` |
| `type` | `types`, `class` |
| `constant` | `constants` |
| `number` | `numbers` |
| `variable` | |
| `parameter` | |
| `property` | |
| `punctuation` | |
| `tag` | |
| `attribute` | |
| `macro` | `macros`, `decorator`, `annotation` |
| `heading` | |
| `link` | |
| `code` | |
| `inserted` | |
| `deleted` | |
| `changed` | |
| `invalid` | |

Names ignore case, and you can write them with dashes or spaces instead of underscores (`line-highlight`, `Line Highlight`).

### Color formats

Colors can be written as:

- Hex: `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
- `rgb(…)` and `rgba(…)`, with numbers or percentages, for example `rgb(30, 30, 46)` or `rgba(255, 0, 0, 0.5)`.
- `hsl(…)` and `hsla(…)`, for example `hsl(240, 21%, 15%)`.
- A few names: `black`, `white`, `transparent`, `red`, `green`, `blue`, `yellow`, `orange`, `purple`, `gray` (or `grey`), `silver`, `cyan` (or `aqua`), `magenta` (or `fuchsia`) and `pink`.
- Sublime Text's `color()` function, which starts from a color and adjusts it: `alpha()` (or `a()`), `blend()`, `blenda()`, `lightness()` (or `l()`) and `saturation()` (or `s()`). For example `color(#1e1e2e blend(#ffffff 90%))` keeps 90% of the first color and mixes in 10% white.

### Turning off italic and underline

`font_options` removes styles from syntax highlighting in every scheme:

```json
{
  "font_options": ["no_italic", "no_underline"],
}
```

`"no_bold"` is accepted too, but has no effect: Strata never draws code in bold.

## Importing color schemes

You can add your own schemes by putting `.tmTheme` or Sublime Text 3/4 `.sublime-color-scheme` files in a folder called `Themes` inside Strata's config folder (create it if it isn't there; [Troubleshooting](troubleshooting.md) shows where the config folder is on each system). Files must sit directly in `Themes`, not in subfolders.

Schemes are loaded when Strata starts, so restart Strata after adding or changing one. Each appears under **Your Themes**, named after its file: `Themes/Midnight.sublime-color-scheme` shows up as **Midnight**. A file with the same name as a built-in scheme replaces it.

For `.sublime-color-scheme` files, Strata reads:

- Comments and trailing commas, as Sublime Text allows.
- `variables`, used through `var(name)`. Variables can refer to other variables.
- `globals`, including `background`, `foreground`, `caret`, `line_highlight`, `selection`, `selection_foreground`, `selection_border`, `inactive_selection`, `gutter`, `gutter_foreground`, `find_highlight`, `guide`, `active_guide`, `stack_guide`, `brackets_foreground`, `accent` and `invisibles` (or `whitespace`). Globals Strata doesn't use are ignored.
- `rules`, each with a `scope` and any of `foreground`, `background` and `font_style` (`bold`, `italic`, `underline`; `stippled_underline` and `squiggly_underline` become a plain underline). When a rule's `foreground` is a list of colors, the first one is used.
- Every color format listed in [Color formats](#color-formats), including `color()` with `alpha()`, `blend()`, `lightness()` and `saturation()`.

Strata decides whether an imported scheme is light or dark from its background color. That matters when you [follow the system appearance](#follow-system-appearance).

### When a scheme has problems

Problems are reported in the status bar when Strata starts. A file that can't be read at all, or isn't valid JSON, is skipped. In a `.sublime-color-scheme` that loads, a rule with a missing or bad scope, or a color Strata can't read, is skipped and the rest of the scheme still works; the status bar shows the first problem and how many more there were. A scheme without a `globals.background` loads but is reported.

## Fonts

Strata has two font settings: the **editor font**, used for your text, and the **interface font**, used for menus, tabs, the side bar and panels.

### Bundled fonts

These fonts ship with Strata, all under the SIL Open Font License 1.1:

| Font | Kind | Notes |
|------|------|-------|
| JetBrains Mono | Monospaced | Draws coding ligatures |
| Fira Code | Monospaced | Draws coding ligatures |
| IBM Plex Mono | Monospaced | |
| Source Code Pro | Monospaced | |
| Inter | Proportional | Meant for the interface |

With JetBrains Mono and Fira Code, character pairs such as `=>`, `!=` and `===` are drawn as ligatures, but every character keeps its own cell, so the cursor and selections still line up.

Only the regular weight of each font is used. Italic text is slanted by Strata, and code is never drawn in bold.

### Choosing a font

Use **Font ▸ Editor Font** and **Font ▸ Interface Font** (under Strata ▸ Settings on macOS, Preferences on Windows and Linux). The menu items show the current font. From the command palette, run **Preferences: Editor Font…** ({{key:select_editor_font}}) or **Preferences: Interface Font…** ({{key:select_ui_font}}).

The list starts with **Default**, which is Strata's built-in font: Hack for the editor and Ubuntu for the interface. Then come the bundled fonts (the monospaced ones first for the editor, Inter first for the interface), and then every font installed on your computer. Type to filter the list and press Enter to pick.

Installed fonts are found in the system and user font folders, and are listed by file name: `SF-Mono-Regular.otf` shows as **SF Mono Regular**. Strata looks for `.ttf`, `.otf` and `.ttc` files in:

- macOS: `/System/Library/Fonts`, `/System/Library/Fonts/Supplemental`, `/Library/Fonts` and `~/Library/Fonts`.
- Windows: `C:\Windows\Fonts` and your user fonts folder (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`).
- Linux: `/usr/share/fonts`, `/usr/local/share/fonts` and `~/.local/share/fonts`.

Emoji and symbol fonts are left out of the list.

### Fonts in settings.json

`font_face` (editor) and `ui_font_face` (interface) take any of these:

- A bundled font's name. Case, spaces, dashes and underscores don't matter, so `"jetbrainsmono"` and `"fira-code"` work.
- An installed font's name as the picker shows it (case doesn't matter).
- The full path to a font file.
- An empty string, for the built-in default.

```json
{
  "font_face": "JetBrains Mono",
  "ui_font_face": "Inter",
}
```

If a font can't be found or its file can't be read, Strata uses the built-in font instead and says so in the status bar. Characters the chosen font lacks are drawn with the built-in fonts. When Chinese, Japanese or Korean text appears, Strata loads a suitable font from your system automatically.

## Font size and zoom

The editor font size is `font_size`, in points (default 14). It also sets the text size in the terminal and the Markdown preview.

| Action | Command | Shortcut |
|--------|---------|----------|
| Larger | View: Zoom In | {{key:increase_font_size}} |
| Smaller | View: Zoom Out | {{key:decrease_font_size}} |
| Back to 14 pt | View: Reset Zoom | {{key:reset_font_size}} |

The menu items are **Font ▸ Larger**, **Smaller** and **Reset Size**. Each step is one point, between 8 and 48.

## Interface size

`ui_font_size` sets the size of the interface text (default 13). Choose a size from **Font ▸ Interface Size** (10 pt to 18 pt), or step it with **Larger** and **Smaller** in the same menu, also in the palette as **Preferences: Interface Text Larger** ({{key:increase_ui_font_size}}) and **Preferences: Interface Text Smaller** ({{key:decrease_ui_font_size}}). The size stays between 9 and 24.

## Line spacing

**Font ▸ Line Spacing** offers four presets:

| Preset | Line height |
|--------|-------------|
| Compact | 1.0× |
| Comfortable | 1.2× |
| Relaxed | 1.4× |
| Spacious | 1.6× |

For finer control, use these settings:

- `line_height`: a multiple of the font's own line height, from 0.8 to 3.0 (default 1.0).
- `line_padding_top` and `line_padding_bottom`: extra points above and below each line, from 0 to 40 (default 0). These are Sublime Text's names.

```json
{
  "line_height": 1.3,
  "line_padding_top": 1,
  "line_padding_bottom": 1,
}
```

Choosing a preset from the menu sets `line_height` and resets both paddings to 0.

## Cursor

**View ▸ Cursor** has three choices in each part:

- Shape: **Line**, **Block** or **Underline** (`caret_style`: `"line"`, `"block"`, `"underline"`).
- Blinking: **Blink**, **Smooth Fade** or **Solid** (`caret_blink`: `"blink"`, `"smooth"`, `"solid"`).
- Width: **Thin**, **Normal** or **Thick** (`caret_width`: 1, 2 or 3 points). In `settings.json` you can use any width from 1 to 6. The width applies to the Line shape.

## Window opacity

**View ▸ Window Opacity** makes the window see-through: 100%, 95%, 90%, 85%, 80%, 70% or 60%. The setting is `window_opacity`, from 0.3 to 1.0 (default 1.0, fully opaque).

## Minimap

The minimap is the small overview of the file at the right edge of the editor. Click or drag in it to scroll. Turn it on or off with **View ▸ Toggle Minimap** ({{key:toggle_minimap}}); the setting is `minimap`. The minimap hides itself when the editor is too narrow for it.

## Editor extras

**View ▸ Editor Extras** turns on or off a set of small visual aids:

| Menu item | What it does | Setting |
|-----------|--------------|---------|
| Show Whitespace in Selection | Dots for spaces and arrows for tabs, only inside selected text | `draw_white_space`: `"selection"` |
| Show All Whitespace | Whitespace marks everywhere | `draw_white_space`: `"all"` |
| Hide Whitespace | No whitespace marks | `draw_white_space`: `"none"` |
| Highlight Trailing Whitespace | Tints spaces and tabs at the end of lines (not where you are typing) | `highlight_trailing_whitespace` |
| Bracket Pair Colors | Colors brackets by how deeply they are nested | `bracket_colors` |
| Color Swatches | A small color bar under color values such as `#22d3ee`, `rgb(…)` or `hsl(…)` | `color_swatches` |
| Sticky Scroll | Keeps the headers of the blocks you are inside pinned at the top while you scroll; click one to jump to it | `sticky_scroll` |
| Smooth Scrolling | Animates scrolling | `smooth_scroll` |
| Git Change Markers | Bars in the gutter for lines added, changed or deleted since the last commit | `git_gutter` |

All of these are on by default, with whitespace shown in selections only.

## Rulers and wrap column

**View ▸ Ruler** draws a vertical guide at column 70, 72, 78, 80, 100 or 120, or none. To have several rulers, list them in `settings.json`:

```json
{
  "rulers": [80, 100],
}
```

**View ▸ Word Wrap Column** makes wrapped lines break at one of the same columns instead of the window edge (**Automatic** wraps at the window edge). The setting is `wrap_column`. Lines still wrap at the window edge if the window is narrower than the column. Word wrap itself is covered in [Editing](editing.md).

## Tabs and status bar

**View ▸ Toggle Tabs** ({{key:toggle_tabs}}) hides or shows the tab bar, and **View ▸ Toggle Status Bar** ({{key:toggle_status_bar}}) hides or shows the status bar. The settings are `show_tabs` and `show_status_bar`.

## Distraction free mode

**View ▸ Toggle Distraction Free** ({{key:toggle_distraction_free}}) goes full screen and shows only your text: the side bar, tabs, status bar, minimap, Markdown preview and build results are hidden, and on Windows and Linux the menu bar too. Run it again to return; the side bar and minimap come back as they were.

**View ▸ Toggle Full Screen** ({{key:toggle_full_screen}}) goes full screen without hiding anything.
