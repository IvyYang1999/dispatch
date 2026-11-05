# Dispatch

Find an agent session on this machine and deliver a message into it.

Claude Code, Codex and Grok Bot sessions on one computer can't talk to each
other. Dispatch gives them (and you) one verb: find a session, hand it a
message, see whether it arrived and what came back.

- **Direct delivery** into a running Claude Code session (its own
  cross-session inbox socket), queued behind the turn it is on.
- **Hook delivery** when the session can't take a message right now: the
  message waits in the store and a Claude Code hook hands it over the next
  time the session speaks, stops or starts.
- **Receipts** read from the session's own transcript: queued, started,
  finished.
- **One dispatch, many recipients**, each with its own delivery state.

Status: M1, single machine, Claude Code first. Codex and Grok Bot adapters,
the MCP server, the plugins and the desktop app follow. Design notes live in
the author's vault; this repository holds the code.

## Build and install

```
cargo build --release
./target/release/dispatch install     # copies the binary to ~/.dispatch/bin, installs the Claude Code hook, registers the MCP server with Codex
```

Then give each agent the plugin, which carries the MCP server, a skill that
says when and how to use it, and the hook:

```
/plugin install dispatch --marketplace IvyYang1999/dispatch     # inside Claude Code, one step
claude plugin marketplace add IvyYang1999/dispatch && claude plugin install dispatch@dispatch   # or from a shell
codex plugin marketplace add IvyYang1999/dispatch   && codex plugin add dispatch
```

The command line does the same things by hand:

```
dispatch sessions                       # who is on this machine, and how a message would reach each
dispatch send --to <id|name> "hello"    # store, then deliver (socket now, or hook later)
dispatch inbox --session <id|name>      # what is waiting for a session
dispatch status <id>                    # each recipient's stage and the delivery log
dispatch reply <id> "…"                 # answer a message back to its sender
dispatch mcp                            # the MCP server over stdio (7 tools: dispatch_whoami, _sessions, _find, _send, _inbox, _reply, _status)
```

Messages live under `~/Library/Application Support/Dispatch/messages`
(`DISPATCH_HOME` overrides it), one Markdown file per recipient, plus a
delivery log per message under `runs/`.

## Releases and updates

`scripts/release.sh <tag> "<notes>"` builds everything for this Mac: the CLI, the
app with the CLI bundled inside it (`Contents/MacOS/dispatch`), the first-install
zip, the updater package (`Dispatch-<version>-<platform>.app.tar.gz` plus its
signature) and `latest.json`. `PUBLISH=1` also creates the GitHub prerelease with
those files attached. The updater key lives outside the repo
(`~/.tauri/dispatch.key`, or `TAURI_SIGNING_PRIVATE_KEY`); its public half is in
`tauri.conf.json`.

Installed apps check `https://github.com/IvyYang1999/dispatch/releases/latest/download/latest.json`
when you press 检查更新 in Settings, verify the package against the public key,
replace themselves and restart. The CLI travels inside the app, so Settings →
一键安装 (which runs the bundled `dispatch install`) brings the command line, the
Claude Code hook and the Codex MCP registration up to date too. Until the
repository is public, the updater endpoint is not reachable from an installed
app; `DISPATCH_UPDATE_URL` points a build at any other `latest.json`.

## Codex

A Codex thread can take a message only through the app-server that is running
it. Dispatch asks the shared daemon (`codex app-server proxy`, `thread/loaded/list`)
which threads it has loaded: those get `codex queue --thread <id> --message …`
and show as 立刻送到; a thread the daemon has not loaded (for example one open
in the Codex desktop app) shows as 待接通 and the message waits in the store.
`dispatch send --allow-resume` adds `--resume`, which lets the daemon load the
thread from history first, at the risk of a second instance of a thread that is
open elsewhere. Receipts come from the thread's rollout file: the queued message,
then `task_started` and `task_complete`. `dispatch codex` prints what this
machine's Codex can do right now.

## Install on another Mac

`scripts/release.sh` builds the CLI and the app in release mode and zips them
as `dist/Dispatch-<version>-macos-<arch>.zip`; the zips are attached to the
GitHub releases of this repository. On the other Mac:

```
unzip Dispatch-*.zip && cd Dispatch-*
xattr -dr com.apple.quarantine Dispatch.app dispatch     # the app is ad-hoc signed, not notarized
mv Dispatch.app /Applications/
./dispatch install                                       # CLI to ~/.dispatch/bin, Claude hook, Codex MCP
```

Then the plugins (see above), and open Dispatch.app. The `安装.txt` inside the
zip says the same in Chinese.

## Desktop app

A Tauri 2 shell over the same core: the feed of dispatches (one row per
dispatch, a status dot per recipient, the thread and delivery log on the
right), the directory of sessions with how a message would reach each, a
composer whose recipient box is the finder, and settings with the one-click
installs. Every recipient has an "open session" button.

```
cd apps/desktop
npm install
npm run tauri dev      # develop
npm run tauri build    # a .app / installer under target/release/bundle
```

## Layout

```
crates/core      dispatch-core: discovery, delivery, store, receipts, hook, MCP server, ops
crates/cli       dispatch: the command line and the MCP entry point
apps/desktop     the Tauri 2 + React desktop app (src-tauri = Rust commands over core, src = the screens)
plugins/claude   Claude Code plugin (.mcp.json, hooks/hooks.json, skills/dispatch)
plugins/codex    Codex plugin (.codex-plugin/plugin.json, .mcp.json, skills/dispatch)
plugins/skill    the skill, written once; scripts/sync-plugins.sh copies it into both
.claude-plugin/marketplace.json, .agents/plugins/marketplace.json   the two marketplaces
```

MIT.
