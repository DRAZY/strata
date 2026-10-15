# Security policy

## Supported versions

Strata is in its first release-candidate cycle. Security fixes go into the
latest release (including release candidates) and `main`; older builds are
not patched, so please update before reporting.

| Version | Supported |
|---|---|
| 1.0.x release candidates and later | Yes |
| 0.x test builds | No |

## Reporting a vulnerability

**Please don't open a public issue for security problems.** Report them
privately through GitHub instead:

1. Go to the repository's **Security** tab.
2. Choose **Report a vulnerability** (GitHub Security Advisories).
3. Describe the problem, the Strata version and commit (Help ▸ About Strata),
   your operating system, and the steps or file that reproduce it.

You should get an acknowledgement within a week. We'll work with you on a fix
and a disclosure date, and credit you in the advisory and the changelog unless
you'd rather stay anonymous.

## Scope

Strata opens files from anywhere, so anything a crafted file, folder or link
can make it do is in scope. Areas that deserve particular attention:

- **The built-in terminal.** It runs your shell with your permissions; that is
  by design. In scope: escape sequences that make Strata itself misbehave
  (crashes, hangs, reading or writing the clipboard against the
  `terminal_allow_clipboard_read` / `_write` settings, injecting input the user
  didn't type), and anything that runs a program without the user asking.
- **The single-instance IPC socket.** The `strata` command talks to the
  running app over `strata.sock` in the settings folder (a user-only folder and
  socket, with the peer's user checked) or, on Windows, a localhost port with a
  random token stored in the per-user `strata.port`. In scope: another user or
  process opening files, running commands or reading anything through it.
- **Link handling.** Links in Markdown files, the preview and the manual may
  only open `http`, `https` and `mailto` URLs in the browser, open local text
  files in Strata, or show other local files in the file manager (see
  `src/links.rs`). In scope: any link that launches a program, opens another
  URL scheme, or reaches outside those rules.
- **Updates.** The opt-in updater downloads releases from GitHub and checks
  their signature before installing. In scope: anything that gets an unsigned
  or tampered update installed, or makes Strata contact other hosts.
- **File handling:** saves that lose or corrupt data, symlink and permission
  handling during atomic saves, and crashes or hangs on crafted files
  (syntax definitions, color schemes, snippets, settings, session files).

Out of scope: programs you run yourself in the terminal or as a build system
doing what they do, and attacks that need someone who already controls your
user account.
