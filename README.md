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
