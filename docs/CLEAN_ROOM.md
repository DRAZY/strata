# Clean-room policy

Sublime Text is proprietary software whose license forbids reverse engineering.
To keep this project legally clean:

## Allowed sources
- Using Sublime Text as a normal user and describing what it does.
- Public documentation (sublimetext.com docs, changelogs, forum posts).
- Openly licensed community packages and grammars (respect each license).
- File-format compatibility (`.sublime-syntax`, `.tmTheme`, `.sublime-keymap`)
  implemented from public specs.

## Not allowed
- Decompiled or disassembled code from the Sublime Text binaries.
- Copying files out of `Sublime Text.app` (bundled packages, themes, icons, fonts).
- Sublime's name, logo, or trademarked branding.

## Provenance of the initial spec
The initial feature inventory (docs/FEATURES.md) came from public knowledge of
the editor plus non-code bundle metadata: the Info.plist file-type list and the
names of bundled language packages. No code was decompiled.
