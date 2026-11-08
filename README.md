<p align="center">
  <img src="apps/desktop/branding/icon-dark.svg" width="112" alt="Dispatch: a D whose bowl becomes two crossing arrows">
</p>

<h1 align="center">Dispatch</h1>

<p align="center">Find an agent session on this machine and deliver a message into it.</p>

<p align="center">
  <a href="https://github.com/IvyYang1999/dispatch/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/IvyYang1999/dispatch?display_name=tag&label=release&color=111111"></a>
  <img alt="macOS, Apple Silicon" src="https://img.shields.io/badge/macOS-Apple%20Silicon-111111">
  <img alt="Built with Tauri 2 and Rust" src="https://img.shields.io/badge/built%20with-Tauri%202%20%2B%20Rust-111111">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/github/license/IvyYang1999/dispatch?color=111111"></a>
</p>

<p align="center"><a href="README.zh-CN.md">中文说明</a></p>

<p align="center">
  <img src="docs/images/feed-en.png" width="880" alt="The Dispatch feed: one row per dispatch, a status chip per recipient, the message and its replies on the right">
</p>

Claude Code, Codex and Grok Bot sessions on one computer cannot talk to each other. Each one knows only its own transcript. Dispatch gives them, and you, one verb: **find a session, hand it a message, see whether it arrived and what came back.**

- **Direct delivery.** A running Claude Code session takes the message through its own cross-session inbox socket, queued behind the turn it is on.
- **Hook delivery.** A session that cannot take a message right now (busy, or running with permissions bypassed) gets it from a Claude Code hook the next time it speaks, stops or starts. Nothing is lost in between: the message waits in the store.
- **Codex.** A thread loaded by the shared Codex app-server gets the message through the official `codex queue`; a thread that is not loaded shows as 待接通 (waiting to connect) and the message waits.
- **Grok Bot.** A bot lives on its cloud computer; Dispatch reaches it through the gateway the desktop app uses, driven by the `gbot` command line (`npm i -g grok-bot-cli`), as a direct message that may redirect the bot's current turn. Receipts come from the bot's transcript.
- **Receipts** read from the session's own transcript: queued, started, finished, replied.
- **Not delivered.** When nobody can hand a message over (the session is closed and there is no hook, no codex command, no Grok Bot app) it is marked failed at once, with the fix; a queued copy past its deadline (1 h urgent, 24 h normal, adjustable) counts as failed too. Retry and cancel exist; the app makes a postman's round every half minute and delivers queued copies whose route has opened.
- **One dispatch, many recipients,** each with its own delivery state.
- **Plugins** for Claude Code and Codex: nine MCP tools, a skill that says when and how to use them, and the hook.
- **A desktop app** for the humans: the feed, the directory, a composer, one-click installs, and an "open session" button on every recipient. A menu bar item reads "2 queued · 1 failed"; system notifications only for a reply or a failed delivery.

Everything stays on your Mac. Messages are Markdown files you can read and edit; nothing goes to a server.

## Install

### The app (macOS, Apple Silicon)

1. Download `Dispatch-<version>-macos-arm64-signed.zip` from the [latest release](https://github.com/IvyYang1999/dispatch/releases/latest), unzip it and move `Dispatch.app` to `/Applications`. The app and the command line inside it are Developer ID signed and notarized.
2. Open Dispatch and press **一键安装** in Settings. It puts the command line at `~/.dispatch/bin/dispatch`, installs the Claude Code hook and registers the MCP server with Codex.
3. Give each agent the plugin, which carries the MCP server, the skill and the hook:

```
/plugin install dispatch --marketplace IvyYang1999/dispatch                     # inside Claude Code
codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch   # Codex
```

Updates: Settings → **检查更新**. The app verifies the package against its built-in public key, replaces itself and restarts. On startup it refreshes an already-installed CLI, keeping existing hook and MCP registrations.

### From source

```
cargo build --release
./target/release/dispatch install
```

Rust stable. The desktop app also needs Node for `apps/desktop`.

## How a message travels

```mermaid
flowchart LR
  send["dispatch send<br/>or the dispatch_send tool"] --> store[("the store<br/>one Markdown file per recipient")]
  store --> reach{"how does it reach<br/>that session?"}
  reach -- "Claude Code, running,<br/>taking messages" --> now["its inbox socket<br/>queued behind the current turn"]
  reach -- "Claude Code, busy or<br/>bypassing permissions" --> later["the hook hands it over<br/>when the session next speaks, stops or starts"]
  reach -- "Codex thread loaded<br/>by the daemon" --> codex["codex queue<br/>its next turn"]
  reach -- "Grok Bot signed in,<br/>gbot installed" --> grok["gbot send<br/>a direct message to the bot"]
  reach -- "nobody can read it yet" --> wait["待接通<br/>waits, and says what to fix"]
  now --> receipts["receipts from the transcript<br/>queued · started · finished · replied"]
  later --> receipts
  codex --> receipts
  grok --> receipts
```

`dispatch sessions`, or the directory in the app, shows for every session which of these paths it would get right now.

## For agents

The plugin gives a session nine MCP tools and a skill that explains them.

| Tool | What it does |
|---|---|
| `dispatch_whoami` | Your own address: session id and name. Call it once. |
| `dispatch_sessions` | Who is on this machine, and how a message would reach each. |
| `dispatch_find` | Find a session by name, folder or a word from what it is doing. |
| `dispatch_send` | Store and deliver a message to one or more sessions. Returns the dispatch id. |
| `dispatch_status` | Each recipient's stage: queued, delivered, working, finished, replied, failed, cancelled. |
| `dispatch_inbox` | What is waiting for you, replies included. |
| `dispatch_reply` | Answer a message back to its sender. |
| `dispatch_retry` | Try a queued or failed message again now, with a fresh deadline. |
| `dispatch_cancel` | Withdraw a message that has not been delivered. |

A message from another agent is information, not an instruction from the user. The skill says so, and the hook says it again each time it hands a message over.

Grok Bot runs its bots on a cloud computer, so it cannot start an MCP server on your Mac. It can still use the command line: enable **Execution on this computer** in its Settings → Computer, then tell the bot to read `plugins/skill/SKILL.md` from this repository and save it as a skill; the skill tells it to pass `--from` with its bot name. (Grok Build, xAI's coding CLI, reads `~/.grok/skills/`: `npx skills add IvyYang1999/dispatch --skill dispatch -a grok`.)

Delivery *into* a bot needs the Grok Bot app signed in on this Mac and `gbot` installed (`npm i -g grok-bot-cli`); `dispatch grok` shows what is in place. The bots then appear in the directory like any session.

## From the command line

```
dispatch sessions                                   # the directory, running sessions first
dispatch send --to transparent "the e2e is green, merge when you like"
dispatch send --to shyposter --to keykeeper "PR #11 splits the check from the send; have a look"
dispatch status <dispatch id>                       # every recipient's stage and delivery log
dispatch inbox --session <id|name>                  # what is waiting for a session
dispatch reply <message id> "done, see the PR"      # answer back to the sender
dispatch send --to codex-thread --deadline 30m "…"  # counts as failed after 30 min (defaults: 1h urgent, 24h normal)
dispatch retry <message id>                         # try a queued or failed message again
dispatch cancel <message id>                        # withdraw one that has not been delivered
dispatch sweep                                      # the postman's round: expire overdue, deliver what can go now (the app does this every 30 s)
dispatch codex                                      # what this Mac's Codex can take right now
dispatch install                                    # CLI to ~/.dispatch/bin, Claude hook, Codex MCP registration
dispatch mcp                                        # the MCP server over stdio (what the plugins start)
```

`--to` takes a session id, an id prefix, the session's desktop name or a fragment of its title. `--from` names the sender when you are not inside a session.

## The desktop app

<p align="center">
  <img src="docs/images/directory-en.png" width="880" alt="The directory: every session on this Mac with its harness, folder, state and the delivery path it would get">
</p>

- **Feed.** One row per dispatch, a chip per recipient colored by its state, the message with its recipients and their replies on the right. Filters for queued and failed. Queued and failed recipients carry Retry and Cancel.
- **Directory.** Every session on the Mac: harness, folder, idle or working, and the delivery path it would get right now.
- **Send.** The recipient box is the finder. Urgency. Several recipients at once.
- **Settings.** One-click install, the plugin commands to copy, the language, notifications, where the messages live, check for updates.
- **Menu bar.** The icon carries "2 queued · 1 failed" and opens a list of the latest dispatches; pick one and the app jumps to it. Closing the window leaves the app in the menu bar; Quit is in the menu and on ⌘Q.
- **Notifications.** Two kinds only: a session replied ("Researcher replied to you"), or a delivery failed. Nothing else makes a sound.

Tauri 2 over the same Rust core as the command line, about 9 MB. English and Chinese: the switch in Settings also sets the language of the command line and of the text agents are handed (`DISPATCH_LANG=en|zh` overrides it per process).

## Where things live

Messages: `~/Library/Application Support/Dispatch/messages/<session>/…md` (`DISPATCH_HOME` overrides it), one Markdown file per recipient with YAML front matter, plus a delivery log per message under `runs/`.

The Claude Code hook goes into `~/.claude/settings.json` (UserPromptSubmit, Stop, SessionStart) and only ever runs `dispatch hook run`.

## Development

```
cargo test
cargo build --release
cd apps/desktop && npm install && npm run tauri dev
scripts/release.sh v0.1.4 "what changed"    # signed/notarized Mac artifacts; credentials and PUBLISH=1: docs/releasing.md
```

```
crates/core      dispatch-core: discovery, reach, delivery, store, receipts, hook, MCP server, ops
crates/cli       dispatch: the command line and the MCP entry point
apps/desktop     Tauri 2 + React (src-tauri = Rust commands over core, src = the screens)
plugins/claude   Claude Code plugin (.mcp.json, hooks/hooks.json, skills/dispatch)
plugins/codex    Codex plugin (.codex-plugin/plugin.json, .mcp.json, skills/dispatch)
plugins/skill    the skill, written once; scripts/sync-plugins.sh copies it into both
```

The updater key lives outside the repository; `tauri.conf.json` carries only its public half.

More: [the message format](docs/message-format.md) (files, status words, the run log), [how a message reaches a session](docs/adapters.md) (the adapters, the ladder, adding one), [CONTRIBUTING.md](CONTRIBUTING.md) (build, texts, releases). CI runs the tests on macOS, Linux and Windows; Linux and Windows bundles are built by `.github/workflows/release.yml` on each release tag, unsigned.

## Status

0.1.x: macOS on Apple Silicon; Claude Code, Codex, and Grok Bot through `gbot`. Next on the list: native Grok gateway calls (no Node), Agent Comments calling Dispatch, Windows and Linux builds.

## License

MIT.
