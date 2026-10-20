# AI Helpers

Strata can ask a language model you choose to summarize a review, explain a change or a piece of code, flag risky edits, and draft a commit message. The helpers are optional and off until you turn them on. Strata never sends anything anywhere until you've turned them on, picked a provider, and (for a provider outside your computer or network) seen exactly what will be sent and said yes.

## Turning them on

Open Settings ({{key:settings}}) and choose **AI Helpers**, or choose Tools ▸ AI ▸ Provider Settings…. Turn on **Turn on AI helpers**, pick a provider, and fill in its details. **Test Connection** checks that the provider answers (and lists its models) without sending any of your text. For a remote provider with a saved key, it first says that it will send the key to that host, and asks; it doesn't ask again for that provider (or once you've allowed the provider's requests).

## Providers

| Provider | Setting | What you need |
|---|---|---|
| Any OpenAI-compatible server | `"openai"` | Its address (`ai_base_url`, up to and including `/v1`) and a model name (`ai_model`). A key if the server wants one. |
| Anthropic API | `"anthropic"` | An Anthropic API key. The model is `ai_anthropic_model`. |
| `claude` command line | `"claude_cli"` | Claude Code installed and signed in. |
| `codex` command line | `"codex_cli"` | Codex installed and signed in. |

Some OpenAI-compatible addresses:

| Server | Address |
|---|---|
| Ollama on this computer | `http://localhost:11434/v1` (the default) |
| LM Studio on this computer | `http://localhost:1234/v1` |
| Ollama or LM Studio on another computer at home or work | `http://192.168.1.20:11434/v1` (that computer's address) |
| OpenRouter | `https://openrouter.ai/api/v1` |
| OpenAI | `https://api.openai.com/v1` |

An address that isn't on your computer or local network must start with `https://`, so your code and key are never sent unencrypted.

The command-line providers run the tool you already use, with your own login: Strata starts it, writes the prompt to it, and shows what it prints. Strata never reads or reuses those tools' login files. Both run in an empty temporary folder that's removed afterwards. `claude` runs with no tools at all, so it can only answer from the text it was given; `codex` runs with a read-only sandbox and is told to answer from the text alone. Strata finds them on your PATH and in the usual install folders (as it does for build systems).

## Local and remote providers

A provider is **local** when its address is on this computer or your local network: `localhost`, a name ending in `.local` (found on your network with mDNS), or a private, loopback or link-local IP address written out in full (such as `127.0.0.1`, `10.0.0.5`, `172.16.4.2`, `192.168.1.20`, `::1`, `fd00::5`). The answer panel labels the provider "local" or "remote".

Strata judges this from the address as you typed it, never from what a name looks up to: a name like `ollama.example.com` counts as remote even if your network happens to resolve it to a private address, because that can change. To use a server at home or work as a local provider, type its IP address or its `.local` name. Requests to local providers never go through a proxy, so a proxy setting can't carry them off your network.

The Anthropic API and the command-line tools are always remote: the tools send your text to their vendor.

## What's sent, and asking first

Before the first request to a remote provider, Strata shows exactly what it will send (the instructions and all the material, in full, scrollable) and where it will go, and asks:

- **Allow Once** sends this request.
- **Always for This Provider** sends it and doesn't ask again for this provider. A provider is its kind and address (or its program): if you point the setting at another server, Strata asks again.
- **Cancel** sends nothing.

To be asked again for every provider, choose Tools ▸ AI ▸ Forget Allowed Providers. The answers are kept in `ai-consent.json` in Strata's settings folder.

Local providers aren't asked about: the text stays on your computer or network.

Each helper sends only what it's about:

| Helper | What it sends |
|---|---|
| AI: Summarize Changes | The changes of every file in the Changes list of [Review Changes](review.md), compared with its base (the last commit, or the start of the review). |
| AI: Summarize File Changes | The changes in the open review tab's file. |
| AI: Explain Change | The change at the caret in the open review tab, with a few lines around it. |
| AI: Flag Risks | The same as Summarize Changes, asking about bugs, security problems, data loss and breaking changes. |
| AI: Draft Commit Message | Every change since the last commit, even during a review that started later. |
| AI: Explain Selection | The selected text, and the file's name and language. |

The changes go as diffs. Unsaved edits in a tab of a changed file count. Strata never sends:

- files that look like they hold secrets, wherever they are and whatever Git says about them (see the list below). The panel lists them by name, "left out (looks like a secret)", so you know; their names aren't sent;
- files Git ignores (`.gitignore`), even ones Git still tracks (a committed `config.yml` later added to `.gitignore`);
- binary files, symbolic links (or files reached through a linked folder), and files Git converts with a filter (Git LFS, encryption filters, `working-tree-encoding`). The panel says how many were left out; their names aren't sent;
- a file that changed between Review Changes listing it and Strata reading it (for example, replaced by a link to a file elsewhere): Strata reads each file from the repository's top folder without following links, checks it's still exactly the file that was listed, and otherwise leaves it out and says so;
- anything that isn't part of the helper you chose.

Explain Selection and the review-tab helpers check the file the same way: no secrets, no links, nothing ignored, filtered or binary.

A file counts as a secret when its name, or the name of any folder it's in (counted from the repository's top folder; outside a repository, anywhere in its path), matches one of these. Letter case doesn't matter. So `.env/production.json`, `credentials/account.json` and `.ssh/config` are never sent, just like `.env`.

| Pattern | For example |
|---|---|
| `.env`, `.env.*` | `.env`, `.env.local`, `.env.production`, `.env/production.json` |
| `*.pem`, `*.key` | `server.pem`, `tls.key` |
| `id_rsa*`, `id_ed25519*` | `id_rsa`, `id_ed25519.pub`, `id_rsa/archive.txt` |
| `*.p12`, `*.pfx`, `*.keystore` | `cert.p12`, `release.keystore` |
| `.npmrc`, `.pypirc` | package registry logins |
| `credentials*` | `credentials`, `credentials.json`, `credentials/account.json` |
| `.ssh`, `.aws`, `.gnupg`, `.kube`, `secrets` | everything in these folders: `.ssh/config`, `.aws/config`, `secrets/db.txt` |

### The size limit

A helper sends at most `ai_max_prompt_kb` (200 KB unless you change it). When a review is bigger, files are sent in list order until the limit, the last one cut short, and the rest left out. The question and the panel both say so ("Over the 200 KB limit: 12 of 30 files sent"), and the model is told that files were left out.

## The answer panel

Answers stream into a panel at the bottom of the window, rendered as Markdown, with the provider and the time taken in its header. While a request runs, a spinner shows and **Cancel** (or Tools ▸ AI ▸ Cancel Request) stops it at once; the text so far stays. **Copy** copies the answer and **Open in New Tab** puts it in a new, untitled tab. Strata doesn't commit: copy a drafted commit message into your Git tool, or the terminal. × hides the panel (and stops a request). AI: Toggle Panel brings it back.

Requests never hold up the editor: they run in the background, and you can keep working. A new request replaces the one running. When a provider sends nothing for `ai_timeout_seconds` (180 unless you change it), Strata stops waiting and says so.

## API keys

Keys are kept in the system's own keychain: the macOS Keychain (items named "Strata AI"), or the Windows Credential Manager (credentials named "Strata AI/…"). They're never written to `settings.json`, never logged, and never shown again after you save them. To add one, paste it in Settings ▸ AI Helpers and click **Save Key**; **Remove Key** deletes it.

A key is saved for one address (for example `https://openrouter.ai`) and is only ever sent there: change the address and Strata uses that address's key, if you've saved one. Keys reach the network through the system's `curl` program (the same one updates use), handed over privately rather than on its command line.

On Linux there's no keychain Strata can rely on, so keys can't be saved there; local servers and the command-line tools need none.

## When something goes wrong

Messages in the panel say what happened and what to try:

| Message | What to do |
|---|---|
| Couldn't connect … Is the server running? | Start Ollama or LM Studio (and load a model), or check the address. |
| … needs an API key (401) / didn't accept the API key (401) | Save the right key for that address in Settings ▸ AI Helpers. |
| … has no such address or model (404) | Check the address ends in `/v1` and the model name is one Test Connection lists. |
| … is limiting requests (429) | Wait a little and try again. |
| … sent nothing for 180 seconds | The model may still be loading: try again, or raise `ai_timeout_seconds`. |
| Couldn't find the claude command | Install the tool and sign in, or check it's on your PATH. |
| claude failed: … | The tool's own message follows: often you need to sign in again in a terminal. |

## Commands

Every helper is in Tools ▸ AI and the command palette. None has a key; you can bind any of them in your [key bindings](keybindings.md).

| Command | Command id |
|---|---|
| AI: Summarize Changes | `ai_summarize_changes` |
| AI: Summarize File Changes | `ai_summarize_file` |
| AI: Explain Change | `ai_explain_change` |
| AI: Flag Risks | `ai_flag_risks` |
| AI: Draft Commit Message | `ai_draft_commit_message` |
| AI: Explain Selection | `ai_explain_selection` |
| AI: Cancel Request | `ai_cancel` |
| AI: Toggle Panel | `ai_toggle_panel` |
| AI: Test Connection | `ai_test_connection` |
| AI: Provider Settings… | `ai_settings` |
| AI: Forget Allowed Providers | `ai_forget_allowed` |

All the settings are in the [Settings Reference](settings.md#ai-helpers).
