# Printing and Exporting

Strata prints a file as clean, syntax-colored pages with line numbers, a header with the file's name and the date, and page numbers. Markdown files print as their rendered preview. The same pages can be saved as a PDF or as an HTML page.

## Printing

Choose File ▸ Print… ("File: Print…" in the Command Palette). A small dialog asks for the options below, then Strata opens the pages as a PDF in your PDF viewer (Preview on macOS), where you choose the printer, check the pages and print. Printing from the viewer gives you the system's full print dialog, page ranges and printer settings on every platform.

Print has no default shortcut, because {{key:goto_anything}} is Goto Anything, as in Sublime Text. You can give it one in your key bindings with the command `print`; see [Key Bindings](keybindings.md).

## Options

| Option | What it does |
|---|---|
| Print in color | Keep the syntax colors. Off, the text is black. |
| Line numbers | Number each line in a gutter. Long lines wrap onto continuation rows, which have no number. |
| Selection only | Print just the lines your selection covers, numbered as in the file. Offered when text is selected; it's on to start with when the selection spans several lines. |
| Rendered Markdown | For Markdown files: print the rendered document, as in the preview. Off, the Markdown source prints like any other file. |
| Paper | US Letter or A4. The default follows your system's region. |

The pages are always on a white background, whatever color scheme you use in the editor. Code uses the bundled Hack font and the GitHub Light colors; tabs follow the file's tab width.

## Exporting

| Command | What it does |
|---|---|
| File ▸ Export ▸ PDF… | Save the pages as a PDF file. |
| File ▸ Export ▸ HTML… | Save the pages as a standalone HTML page, laid out for printing from a browser. |
| File ▸ Export ▸ Markdown as HTML… | Save a Markdown file's rendered document as an HTML page (see [Markdown Preview](markdown.md)). |

Both ask for the same options as Print… first. The PDF is exactly what Print… shows. Its text can be searched and copied, and the fonts are embedded, so it looks the same everywhere.
