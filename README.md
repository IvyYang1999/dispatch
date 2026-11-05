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

## Layout

```
crates/core   dispatch-core: discovery, delivery, store, receipts, hook
crates/cli    dispatch: the command line (also the courier and, later, the MCP server)
```

## Build

```
cargo build --release
./target/release/dispatch sessions
./target/release/dispatch send --to <session id or name> "hello"
./target/release/dispatch hook install
```

Messages live under `~/Library/Application Support/Dispatch/messages`
(`DISPATCH_HOME` overrides it), one Markdown file per recipient, plus a
delivery log per message under `runs/`.

MIT.
