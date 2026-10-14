# Snippets

A snippet is a small template you insert by typing a short word and pressing Tab. It can have fields to fill in, which you step through with Tab. Snippet files use the `.sublime-snippet` XML format, so existing snippet collections work unchanged.

## Using snippets

There are three ways to insert a snippet.

**Type its trigger and press Tab.** For example, in a Rust file, type `fn` and press Tab to get a function skeleton. The trigger has to be a separate word: typing `myfn` and Tab doesn't expand `fn`. If no snippet matches, Tab indents as usual.

**Pick it from the completion list.** As you type, snippets whose trigger starts with what you've typed show up in the [word completion](editing.md#word-completion) list, with their description and a ⇥ sign. A trigger typed in full is listed first. Choose it with Enter or Tab.

**Choose Tools ▸ Snippets…** ({{key:snippets}}). This lists every snippet that applies where the cursor is, including ones without a trigger. Pick one to insert it. If you select some text first, the snippet replaces it, and snippets that use the selected text (such as the Markdown `link` snippet) wrap it.

Snippets only appear in the languages they're meant for. Python snippets don't expand in a Rust file. See [Triggers and scopes](#triggers-and-scopes).

With several cursors, the snippet is inserted at every cursor, and you fill in the fields in all the copies at once.

## Moving through fields

After a snippet is inserted, the first field is selected so you can type over it. The status bar shows which field you're on, for example "Snippet field 1 of 2".

| Key | What it does |
|-----|--------------|
| Tab | Moves to the next field. |
| Shift+Tab | Moves back to the previous field. |
| Esc | Stops stepping through fields. The text and selection stay as they are. |

After the last numbered field, Tab moves to the final position, marked `$0` in the snippet, and you're done. If a snippet has no `$0`, the cursor ends up at the end of the snippet.

Field navigation also ends if you move the cursor outside the snippet, switch to another tab, or undo the insertion.

## The snippet file format

A snippet is a small XML file with the extension `.sublime-snippet`. Here is a complete example:

```xml
<snippet>
    <content><![CDATA[
/// ${1:Describe what it does}.
pub fn ${2:name}(${3:arg}: ${4:u32}) -> ${5:bool} {
    ${0:todo!()}
}
]]></content>
    <tabTrigger>pfn</tabTrigger>
    <scope>source.rust</scope>
    <description>Public function with a doc comment</description>
</snippet>
```

| Element | Required | What it holds |
|---------|----------|---------------|
| `content` | Yes | The text to insert. Wrap it in `<![CDATA[` and `]]>` so you can use `<`, `>` and `&` freely. A line break right after `<![CDATA[` and right before `]]>` is ignored. |
| `tabTrigger` | No | The word you type before pressing Tab. Without one, the snippet is only available from Tools ▸ Snippets…. |
| `scope` | No | Where the snippet applies, as a scope selector. Without one, it applies everywhere. |
| `description` | No | The name shown in Tools ▸ Snippets… and the completion list. Without one, the trigger or the file name is shown. |

Line breaks in the content keep the indentation of the line where you insert the snippet. A tab character in the content becomes one level of indentation, which is spaces if the file is indented with spaces.

To type a literal `$`, `\` or `}` in the content, put a backslash before it: `\$`, `\\`, `\}`. A `$` that isn't followed by a field number or a name is kept as it is.

## Triggers and scopes

The scope says which languages, or which parts of a file, a snippet works in. Every position in a file has a list of scope names, from the language down to the token, such as `source.python` and then `string.quoted` inside a Python string. A snippet applies when its scope selector matches that list where the trigger starts.

| Selector | Matches |
|----------|---------|
| `source.python` | Python files. A name also matches longer names that start with it and a dot, so `source.shell` matches `source.shell.bash`, and `source` matches any programming language. |
| `source.js, source.ts` | Either one. A comma separates alternatives; a vertical bar works too. |
| `source.python string` | Inside a string in a Python file. Names separated by spaces must appear in that order, from outer to inner. |
| `text.html - source` | HTML, but not inside embedded code such as a `<script>` block. A minus excludes what follows it. |
| `source.js & comment` | Only where both match. |
| (empty) | Everywhere. |

Some common language scopes:

| Language | Scope |
|----------|-------|
| Rust | `source.rust` |
| Python | `source.python` |
| JavaScript | `source.js` |
| TypeScript | `source.ts` |
| Go | `source.go` |
| CSS | `source.css` |
| JSON | `source.json` |
| Shell scripts | `source.shell` |
| HTML | `text.html.basic` |
| Markdown | `text.html.markdown` |
| Plain text | `text.plain` |

If the text before the cursor ends with more than one trigger, the longest one wins. If two snippets have the same trigger, your own snippet wins over a built-in one.

## Fields

Fields are the places you fill in, numbered in the order Tab visits them.

| Syntax | Meaning |
|--------|---------|
| `$1`, `$2` … | An empty field. The cursor stops here. |
| `${1:default}` | A field with placeholder text, which is selected so you can type over it or press Tab to keep it. |
| `${1}` | The same as `$1`. |
| `$0` | The final cursor position, after all the numbered fields. |

**Nested fields.** A placeholder can contain other fields:

```text
<div${1: class="${2:name}"}>
```

Tab first selects ` class="name"` (field 1). Press Tab again to edit just `name` (field 2). If you type over field 1 instead, field 2 disappears with it and Tab skips it.

**Mirrored fields.** Use the same number more than once, and every copy changes as you type:

```text
<${1:div}>$0</${1}>
```

When you type over `div`, both tags change together, because Strata puts a cursor in each copy. The placeholder only needs to be given once.

Fields in a snippet are numbered independently of anything else in the file, and gaps in the numbering are fine.

## Variables

Variables insert information about the file and the cursor. Write `$NAME`, or `${NAME:default}` to use a default when the variable is empty. Unknown variables are treated as empty.

| Variable | Value |
|----------|-------|
| `TM_SELECTED_TEXT` | The selected text the snippet replaces. Empty when you insert by trigger, since a trigger only expands when nothing is selected. |
| `SELECTION` | The same as `TM_SELECTED_TEXT`. |
| `TM_CURRENT_LINE` | The text of the current line, not counting the trigger or selection being replaced. |
| `TM_CURRENT_WORD` | The word around the snippet's position, not counting the trigger or selection. |
| `TM_LINE_INDEX` | The column where the snippet starts, counting from 0. |
| `TM_LINE_NUMBER` | The line number, counting from 1. |
| `TM_FILENAME` | The file's name, such as `main.rs`. Empty for a file that hasn't been saved yet. |
| `TM_FILEPATH` | The file's full path. Empty for an unsaved file. |
| `TM_DIRECTORY` | The folder the file is in. Empty for an unsaved file. |
| `TM_SOFT_TABS` | `YES` if the file is indented with spaces, `NO` with tabs. |
| `TM_TAB_SIZE` | The file's tab width, such as `4`. |

For example, `console.log(${1:$TM_SELECTED_TEXT});` logs the selected text, or gives you an empty field if nothing was selected.

## Transformations

A transformation rewrites a value with a regular expression. It has the form `${SOURCE/regex/format/flags}`, where the source is a variable name or a field number.

```text
${TM_FILENAME/(.*)\..+$/$1/}
```

This gives the file name without its extension: `main.rs` becomes `main`.

When the source is a field number, the result is a copy of that field, transformed:

```text
${1:name} → ${1/(.*)/\U$1/}
```

Here the second part shows field 1 in capitals. A transformed copy updates when you Tab away from the field, not while you type.

The **regex** finds the text to replace. Use `\/` for a literal slash. Look-around and back-references aren't supported.

The **format** is the replacement text:

| In the format | Meaning |
|---------------|---------|
| `$1`, `${1}` | What the regex's first group matched (`$0` is the whole match). |
| `\u`, `\l` | Make the next character upper or lower case. |
| `\U`, `\L` | Make everything after it upper or lower case, until `\E`. |
| `\E` | Stop changing case. |
| `(?1:yes:no)` | `yes` if group 1 matched something, otherwise `no`. The `:no` part is optional. |
| `\n`, `\t` | A line break, a tab. |

The **flags** are optional letters:

| Flag | Meaning |
|------|---------|
| `g` | Replace every match, not just the first. |
| `i` | Ignore case. |
| `m` | `^` and `$` match at each line. |
| `s` | `.` also matches line breaks. |
| `x` | Ignore whitespace in the regex, so you can space it out. |

If the regex isn't valid, the value is inserted unchanged.

## Your Snippets folder

Your own snippets go in a folder named `Snippets` inside Strata's settings folder:

| System | Folder |
|--------|--------|
| macOS | `~/Library/Application Support/Strata/Snippets` |
| Windows | `%APPDATA%\Strata\Snippets` |
| Linux | `~/.config/strata/Snippets` (or `$XDG_CONFIG_HOME/strata/Snippets`) |

See [Troubleshooting](troubleshooting.md) for more about the settings folder.

- Strata reads every `.sublime-snippet` file in the folder and in subfolders, so you can organize snippets by language, or drop in a whole collection.
- Changes take effect right away. Strata notices new, changed and deleted files within a couple of seconds, and immediately when you save a snippet in Strata.
- If a snippet file has a mistake, Strata skips it and shows the file and the problem in the status bar, such as `Snippet error: … missing <content>`.
- A snippet of yours with the same trigger as a built-in snippet takes its place.

## Creating a snippet

Choose **Tools ▸ Developer ▸ New Snippet…** ({{key:new_snippet}}). Strata opens a new tab with a template, with the sample content selected so you can type over it. The template explains the optional `tabTrigger`, `scope` and `description` lines; remove the `<!--` and `-->` around the ones you want.

When you save, the Save dialog starts in your Snippets folder. Give the file a name ending in `.sublime-snippet`, and the snippet is ready to use as soon as it's saved.

## Built-in snippets

Strata comes with these snippets. You can replace any of them with your own snippet that uses the same trigger.

| Trigger | Description | Languages |
|---------|-------------|-----------|
| `fn` | Function | Rust |
| `impl` | Impl block | Rust |
| `test` | Test function | Rust |
| `match` | Match expression | Rust |
| `struct` | Struct | Rust |
| `def` | Function | Python |
| `class` | Class | Python |
| `ifmain` | `if __name__ == "__main__"` | Python |
| `for` | For loop | Python |
| `fn` | Function | JavaScript, TypeScript, JSX, TSX |
| `arrow` | Arrow function | JavaScript, TypeScript, JSX, TSX |
| `log` | console.log | JavaScript, TypeScript, JSX, TSX |
| `imp` | Import | JavaScript, TypeScript, JSX, TSX |
| `func` | Function | Go |
| `iferr` | if err != nil | Go |
| `link` | Link | Markdown |
| `img` | Image | Markdown |
| `code` | Code fence | Markdown |
| `table` | Table | Markdown |
| `html5` | HTML5 document | HTML |
| `a` | Link | HTML |
| `div` | div | HTML |
| `shebang` | #!/usr/bin/env | Shell scripts |
| `if` | if … fi | Shell scripts |
| `for` | for … done | Shell scripts |
