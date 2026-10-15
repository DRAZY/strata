# Encodings and Line Endings

Every file has an encoding (how its characters are stored as bytes) and a line ending style. Strata detects both when it opens a file, keeps them when you save, and lets you change them.

## How Strata detects the encoding

When you open a file, Strata looks at its bytes in this order:

1. **A byte order mark (BOM).** A file starting with a UTF-8, UTF-16 LE or UTF-16 BE byte order mark opens in that encoding, with BOM.
2. **UTF-16 without a BOM.** Mostly-English text saved as UTF-16 has a zero byte in every other position. If the start of the file looks like that and the whole file decodes cleanly, it opens as UTF-16 LE or UTF-16 BE.
3. **UTF-8.** If the file is valid UTF-8 (plain ASCII included), it opens as UTF-8.
4. **A guess among the older encodings.** Anything else is not Unicode, so Strata guesses its encoding from which bytes appear and how often, the way web browsers do for pages that don't name theirs. It recognizes Japanese (Shift_JIS, EUC-JP), Chinese (GBK, Big5), Korean (EUC-KR), Cyrillic (Windows 1251, KOI8), Greek, Hebrew, Arabic, Thai, Turkish, Central European and Western text. If the guessed encoding can't read every byte of the file, the file opens as **Western (Windows 1252)** instead, which has a character for every byte, so nothing is lost or replaced.

A new file, and a file path that doesn't exist yet, starts as UTF-8.

When the encoding was guessed, and the guess is anything other than Western text, or the file has too few non-English characters to be sure, the status bar says so, for example "Opened as Japanese (Shift_JIS) — Reopen with Encoding…". If the text looks wrong, use [Reopen with Encoding](#reopen-with-encoding) to read it again with the right one.

## Supported encodings

These are the encodings in the menus, in menu order:

| Encoding | Notes |
|----------|-------|
| UTF-8 | The default |
| UTF-8 with BOM | |
| UTF-16 LE | |
| UTF-16 LE with BOM | |
| UTF-16 BE | |
| UTF-16 BE with BOM | |
| Western (Windows 1252) | Used when no better guess reads the whole file |
| Western (ISO 8859-1) | |
| Western (ISO 8859-15) | |
| Western (Mac Roman) | |
| Western (ISO 8859-3) | |
| Celtic (ISO 8859-14) | |
| Nordic (ISO 8859-10) | |
| Central European (Windows 1250) | |
| Central European (ISO 8859-2) | |
| Romanian (ISO 8859-16) | |
| Baltic (Windows 1257) | |
| Baltic (ISO 8859-4) | |
| Estonian (ISO 8859-13) | |
| Cyrillic (Windows 1251) | |
| Cyrillic (ISO 8859-5) | |
| Cyrillic (KOI8-R) | |
| Cyrillic (KOI8-U) | |
| Cyrillic (Mac) | |
| Greek (Windows 1253) | |
| Greek (ISO 8859-7) | |
| Turkish (Windows 1254) | |
| Hebrew (Windows 1255) | |
| Hebrew (ISO 8859-8) | |
| Arabic (Windows 1256) | |
| Arabic (ISO 8859-6) | |
| Vietnamese (Windows 1258) | |
| Thai (Windows 874) | |
| Japanese (Shift_JIS) | |
| Japanese (EUC-JP) | |
| Japanese (ISO-2022-JP) | |
| Chinese Simplified (GBK) | |
| Chinese Simplified (GB18030) | |
| Chinese Traditional (Big5) | |
| Korean (EUC-KR) | |

## The encoding in the status bar

The status bar shows the current file's encoding, for example **UTF-8**. Click it for two submenus, **Reopen with Encoding** and **Save with Encoding**, which work like the File menu items below.

If some bytes in the file couldn't be read in its encoding, the status bar also shows **Decoding errors**. Those bytes are shown as the replacement character �. Hover the label for a hint; usually the fix is to reopen the file with the right encoding.

## Reopen with Encoding

**File ▸ Reopen with Encoding** reads the file from disk again, decoded with the encoding you pick. Use it when a file opened with the wrong encoding. The cursor stays on the same line and column where possible.

The file must have no unsaved changes. If it has some, the status bar says "Save or revert your changes before reopening with another encoding". Save the file, or use **File ▸ Revert File** ({{key:revert}}) to throw the changes away, and then reopen. An untitled tab that has never been saved can't be reopened.

## Save with Encoding

**File ▸ Save with Encoding** converts the file to the encoding you pick and saves it straight away. From then on, normal saves use that encoding. For an untitled tab, Strata asks where to save first.

## Characters an encoding can't store

Many encodings can only store some characters. For example, Western (Windows 1252) has no Chinese characters, and no single-byte encoding has emoji. Before saving, Strata checks that every character fits. If one doesn't, nothing is written, and a dialog names the first character that can't be saved and where it is (line and column). You can then:

- choose **Save as UTF-8**, which switches the file to UTF-8 and saves it, or
- choose **Cancel**, which saves nothing, so you can change the text or pick another encoding.

UTF-8 and UTF-16 can store every character, so this never happens with them.

## Saving text with decoding errors

If a file has decoding errors (see [The encoding in the status bar](#the-encoding-in-the-status-bar)), saving it writes the replacement character � in place of the bytes that couldn't be read, so the original bytes are lost. Strata warns you first with an **Overwrite file?** dialog; choose **Save Anyway** to go ahead or **Cancel** to stop. The same dialog appears when the file was changed by another program since you opened it.

## Line endings

Strata supports two line ending styles:

- **LF** (Unix), used on macOS and Linux.
- **CRLF** (Windows).

When a file opens, Strata looks at the end of the first line to decide which style it uses. A new file, or a file with only one line, uses your system's style: CRLF on Windows, LF elsewhere.

The status bar shows **LF** or **CRLF**. Click it to convert the file to **LF (macOS, Linux)** or **CRLF (Windows)**. The same choice is in **View ▸ Line Endings** as **Unix** and **Windows**. Converting changes every line break in the file, the cursor keeps its place, and a single Undo reverses it.

### Files with mixed line endings

Strata doesn't change line endings on its own. If a file mixes LF and CRLF, each line keeps the ending it has, and the status bar shows the style of the first line. New lines you type use that style. To make the whole file consistent, choose LF or CRLF from the status bar or View ▸ Line Endings; this converts every line, even when the style you pick is already the one shown.

## How saving works

Saving is designed so that a crash or a full disk can't leave you with a half-written file:

- Strata writes the new contents to a temporary file in the same folder, makes sure it reaches the disk, and then replaces the original with it in one step.
- If the original is a symbolic link, the file it points to is updated and the link stays a link.
- The original file's permissions are kept.
- If Strata can't create a file in that folder but can write to the file itself, it writes the file in place instead.

The file keeps its encoding (with or without BOM) and its line endings.

Two settings can tidy a file as it saves: `trim_trailing_white_space_on_save` and `ensure_newline_at_eof_on_save`, also under **Tools ▸ On Save**. A final line break added this way uses the file's line ending style. See [Settings Reference](settings.md#all-settings).

## Unsaved files and hot exit

When Strata restores unsaved tabs after a restart, each one keeps the encoding it had, so the next save writes the same encoding as before.

## Binary files

Strata doesn't have a separate viewer for binary files. If you open one, it is shown as text: usually as Western (Windows 1252), since binary data is rarely valid UTF-8. Saving it from Strata is not recommended.
