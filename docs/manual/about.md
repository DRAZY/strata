# About Strata

Strata is a fast, open-source, keyboard-first text editor in the spirit of Sublime Text. It's in alpha, so expect rough edges and frequent changes.

## Version

To see which version you have:

- Choose **Help ▸ About Strata** on Windows and Linux, or **Strata ▸ About Strata** on macOS. The command palette name is **Help: About Strata**.
- Or run `strata --version` in a terminal. See [Command Line](cli.md).

## License

Strata is free and open source under the MIT license.

## Clean-room development

Strata is not derived from Sublime Text. It isn't based on Sublime Text's code or binaries, and it contains no files, themes, icons or fonts taken from Sublime Text. Strata's Sublime compatibility, such as reading `.sublime-syntax` grammars, color schemes and keymaps, is built from public documentation, public file formats and openly licensed community files.

## Third-party software and attribution

Strata is built on open-source projects, and it ships fonts, color schemes and syntax definitions made by others. To see them all, choose **Help ▸ Licenses and Attribution…**, or run **Help: Licenses and Attribution…** from the command palette. The window lists:

- **Open-source libraries** Strata is built on, with their licenses (mostly MIT or Apache-2.0).
- **Syntax definitions**, which come from syntect, the bat project, and Sublime Text community packages, each under its own license. License texts that the bundled grammars ask to be reproduced are included. Strata's own PowerShell and JSX grammars are MIT licensed.
- **Color schemes**, with the project, author and license each palette comes from. Strata's schemes are those projects' published colors applied to Strata's own highlighting rules. The Classic schemes come with syntect (MIT).
- **Fonts**: JetBrains Mono, Fira Code, IBM Plex Mono, Source Code Pro and Inter are bundled under the SIL Open Font License 1.1, and the full license text of each is included. The built-in fallback fonts, Hack and Ubuntu, come with the interface toolkit Strata is built on.

## Supported platforms

Strata supports:

- **macOS** on Apple silicon
- **Windows** on x64

Strata also has code for Linux (for example, it keeps its settings in `~/.config/strata` there), but Linux isn't an officially supported platform yet.

## Getting help

- **This manual**: choose **Help ▸ Documentation** ({{key:documentation}}).
- **Every shortcut**: choose **Help ▸ Keyboard Shortcuts**, or see [Keyboard Shortcuts](shortcuts.md).
- **Something not working?** See [Troubleshooting and FAQ](troubleshooting.md).
