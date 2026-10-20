# Markdown Preview

Strata can show a live preview of a Markdown file beside the editor, drawn in your color scheme and updated as you type. You can also save any Markdown file as a standalone HTML page.

## Showing the preview

Choose View ▸ Markdown Preview or press {{key:toggle_markdown_preview}} ("Markdown: Toggle Preview" in the command palette). Do the same again to hide it, or right-click the preview and choose Hide Preview.

The preview opens as a panel on the right side of the window. Drag its left edge to make it wider or narrower.

The preview always shows the file you're working on. When you switch to a file that isn't Markdown, the panel stays open and says "open a .md file to see it here"; switch back to a Markdown file and the preview returns. The panel is hidden in distraction-free mode.

The preview is on or off for every window at once, and Strata remembers your choice.

## Which files are Markdown

A file is previewed when either:

- its extension is `.md`, `.markdown`, `.mdown`, `.mkd` or `.mkdn` (upper or lower case), or
- its syntax is set to Markdown, for example with View ▸ Syntax ▸ Markdown on a file without an extension.

## What the preview shows

The preview understands GitHub-flavored Markdown:

| Element | How it appears |
|---|---|
| Headings | Six sizes, in the scheme's heading color. Level 1 and 2 headings are underlined with a rule. |
| Bold, italic, strikethrough | As styled text. |
| Inline code | In the monospace font on a tinted background. |
| Code blocks | On a tinted panel, with syntax highlighting when the opening fence names a language (for example `rust` right after the three backticks). The language name is shown in the top-right corner, or on its own line above the code when the first line is long enough to reach it. Shell, Python, Node and Ruby blocks also get a **▶ Run** button (see [Running code blocks](notes.md#running-code-blocks)). |
| Links | In the link color. Hover to see where a link goes. |
| Bullet and numbered lists | Nested lists use different bullet shapes; numbered lists keep their starting number. |
| Task lists | `- [ ]` and `- [x]` items show empty and checked boxes. The boxes are for display only; edit the text to check them. |
| Tables | With header row, striped rows and the column alignment from the `:---:` row. Wide tables shrink their widest columns to fit. |
| Block quotes | Indented with a colored bar. |
| Alerts | GitHub alerts (`> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]`, `> [!CAUTION]`) show their label and a bar in their own color. |
| Horizontal rules | A thin line. |
| Footnotes | References appear as small raised labels like [1]; the notes themselves appear smaller and dimmed where they are defined. |
| Images | Not loaded. Each image shows a picture symbol and its alt text, or its file name if it has no alt text. In notes, images from the vault are shown (see [Notes](notes.md#the-preview)). |
| HTML | Shown as dimmed source text, not rendered. HTML comments are hidden, and `<br>` starts a new line. |

Text sizes follow the editor's font size, and all colors come from the current color scheme.

## Scrolling

As you scroll the editor, the preview follows, keeping the part of the document at the top of the editor at the top of the preview. You can scroll the preview on its own too; the editor doesn't follow it.

The preview updates on every edit. Very large files (more than about a quarter of a million characters) update a moment after you pause typing, to keep the editor responsive.

## Links

Click a link in the preview to follow it. Hover over a link first to see where it really goes; the tooltip shows the address, whatever the link text says.

| Link | What happens |
|---|---|
| An anchor such as `#installation` | The preview scrolls to the heading with that anchor. Anchors are made the GitHub way: lowercase, spaces become hyphens, punctuation is dropped. |
| A web or mail address (`http:`, `https:`, `mailto:`) | Opens in your default browser or mail app. |
| A relative path to a text file, such as `docs/setup.md` | Opens the file in a tab. The path is relative to the Markdown file's folder. |
| A relative path to anything else (an image, a PDF, an app, a folder) | Shown in Finder (Explorer on Windows, the file manager on Linux), not opened, so a link can't start a program. |
| Any other kind of link (`file:`, `smb:`, `vnc:`, an app's own scheme …) or a network path (`//server/share`) | Not followed. The status bar shows the link so you can copy it and open it yourself if you trust it. |
| A path that doesn't exist | The status bar says "Link target not found". |

Links inside tables work the same way.

## Right-click menu

Right-click anywhere in the preview for:

| Item | What it does |
|---|---|
| Copy Markdown | Copies the whole document's Markdown source. |
| Copy as HTML | Copies the whole document as formatted HTML, ready to paste into an email or a rich-text editor. Apps that only take plain text get the Markdown. |
| Export as HTML… | Saves the document as an HTML page (see below). |
| Hide Preview | Hides the preview panel. |

The first three are available when the current file is Markdown.

Edit ▸ Copy as HTML is a different command: it copies the selected source code (or the whole file) with syntax highlighting, for any language.

## Exporting to HTML

Choose File ▸ Export ▸ Markdown as HTML… (or "Markdown: Export as HTML…" in the command palette, or Export as HTML… in the preview's right-click menu). The menu item is available when the current file is Markdown; from the palette, a file that isn't Markdown gets a message in the status bar instead. Strata asks where to save, suggesting the file's name with `.html` in the same folder, and the status bar confirms when the file is written.

The page is exported from the text in the editor, so unsaved changes are included.

To print a Markdown file as it looks in the preview, or save it as a PDF, use File ▸ Print… or File ▸ Export ▸ PDF…; see [Printing and Exporting](printing.md).

The exported page:

- is a single, standalone HTML file with its styles built in, so it needs no other files apart from any images it shows.
- is titled with the document's first heading, or the file name if there is no heading.
- uses a clean, GitHub-like style with a readable column width, and switches between light and dark colors to match the reader's system setting.
- includes tables, task lists, strikethrough and footnotes.
- keeps images as links to their original paths. They aren't copied into the page, so keep the HTML file where those paths still work.
- never runs scripts. HTML written in the Markdown keeps plain formatting tags such as `<b>`, `<br>`, `<details>`, images and tables, without event handlers or styles; anything else, such as `<script>`, `<iframe>` or `<style>`, appears as text. Links and images keep only web, mail and relative addresses, so a `javascript:` link becomes inert. Copy as HTML and printing work the same way.

Code blocks are exported as plain monospace text, without syntax highlighting. Alerts keep their label (Note, Tip, Important, Warning or Caution) and their color, in light and dark mode.

## The setting

| Setting | Default | Meaning |
|---|---|---|
| `markdown_preview` | `false` | Whether the preview panel is shown. Toggling the preview changes it for you. |

See [Settings](settings.md) for how to edit `settings.json`.
