# The message format

Dispatch keeps every message as files a person can read and edit. Nothing lives in a database,
and nothing leaves the machine.

```
~/Library/Application Support/Dispatch/        macOS (Linux: ~/.local/share/Dispatch, Windows: %APPDATA%\Dispatch)
├── messages/<recipient short id>/<stamp>-来自<sender>-<urgency>-<letter short id>.md
├── runs/<letter id>.jsonl
└── settings.json
```

`DISPATCH_HOME` moves the whole folder; `dispatch where` prints it.

## One copy per recipient

A dispatch to three sessions is three files that share one `dispatch_id`. Each file is one
recipient's copy, with YAML front matter and the message body:

```markdown
---
id: 7d3f2c1a-…                 this copy
dispatch_id: 2b9e…             the dispatch it belongs to (replies share it)
from: 6ba60ee0-…               the sender's session id, or app / cli / a bot id
from_name: 🪞 Transparent
to: a77bc4d3-…                 the recipient's session id (or bot id)
to_name: 📷 ShyPoster
to_harness: claude             claude / codex / grok: what the recipient runs on
from_harness: grok             claude / codex / grok / human: what the sender runs on, when known
urgency: 普通                   普通 or 紧急 (always stored in Chinese)
wake: true
status: 未读                    see below
created: 2026-11-08 09:15
deadline: 2026-11-09 09:15     past it, an undelivered copy counts as not delivered
from_host: 9f1c…               only when the sender is on another machine: that machine's id
from_host_name: Mac
to_host: 7503…                 only when the recipient is on another machine
to_host_name: Mac mini
signature: 9f2c…               on a copy that came from another machine: that machine's signature, checked on arrival
---
The message body, as the sender wrote it.
```

The front matter is what the Agent Comments mailbox hook already read, plus `id`,
`dispatch_id`, `from_name`, `to_harness`, `from_harness` and `deadline`. Old files without the last three still
parse: the harness is looked up, the deadline defaults to a day (an hour when urgent) after
`created`. The four `*_host` lines appear only on copies that crossed to or from another machine
(see [peers](peers.md)): the machine that delivers a copy owns its status, and the other machine
mirrors it.

## Status words

`status` is the recipient copy's state as the store knows it. It is stored in Chinese whatever
the UI language; the CLI, the app and the MCP tools translate it.

| status | meaning |
|---|---|
| `未读` | Queued: stored, not yet handed over. The hook or the sweep will. |
| `已送达（直投）` | Written into a running Claude Code session's inbox socket and seen queued in its transcript. |
| `已送达（Codex 已入队）` | `codex queue` accepted it for a thread the daemon has loaded. |
| `已送达（Grok 直投）` | Sent to a Grok bot as a direct message through its gateway. |
| `已读（hook代标）` | The Claude Code hook handed it over at the session's next turn. |
| `已送达（派发页）` | A reply to a person who wrote from the app or the command line (`from` was `app`, `cli` or `mcp`): shown in the feed, nothing to hand over. |
| `已回复` | The recipient answered with `dispatch_reply`; the answer is a copy addressed back to the sender, same `dispatch_id`. |
| `没送到` | Not delivered: nobody could hand it over (no hook and the session is closed, no codex, no Grok Bot app), or its deadline passed. Still pending: a late hook pass or `dispatch retry` delivers it. |
| `已取消` | Withdrawn with `dispatch cancel`. Nothing delivers it any more. |
| `待批准` | From another machine and held until someone here approves it (`dispatch retry`) or rejects it (`dispatch cancel`): the machine is not confirmed yet, or its receiving rule says to ask. Not pending: no hook or sweep hands it over. |

"Pending" (what the hook hands over, what `dispatch inbox` lists by default) means `未读` or `没送到`.

Beside `messages/` and `runs/`, the store folder holds `peers.json` (this machine's id, name and
public key, and the paired machines), `peer.key` (its private key, mode 0600), `peer/` (an
active pairing code, the machines seen on the network, the listener's heartbeat) and
`contacts.json` (the address book: `name`, `session_id`, `harness`, optional `host`, `cwd`,
`description`, `provisional`, `set_by`, `updated`). A contact's session is listed and addressable
whatever its age; an exact contact name wins over any fragment match when a recipient is
resolved, and descriptions are searched like names. A `provisional` contact was named
automatically from the words a sender found the session by (`set_by: auto`); it never replaces
a confirmed name and only fills in a session that has none. `icons/` beside it caches the
installed clients' icons for the app.

## Stages and receipts

What the UI shows per recipient (`kind` in `dispatch_status`) combines the stored status with a
receipt read from the recipient's own transcript after a direct delivery:

| kind | when |
|---|---|
| `queued` | status `未读`; the stage text carries the deadline |
| `delivered` | a delivered status, or the transcript shows it queued behind the current turn |
| `working` | the transcript shows the session started the turn that holds it |
| `replied` | a reply copy exists, or the transcript shows that turn finished |
| `failed` | status `没送到` |
| `cancelled` | status `已取消` |

## The run log

`runs/<letter id>.jsonl` is append-only, one JSON object per delivery event. Nothing in it is
ever rewritten, so a retry never erases how the first attempt went.

```json
{"at":"2026-11-08T09:15:02+08:00","action":"store","result":"written","detail":"…/messages/a77bc4d3/….md"}
{"at":"2026-11-08T09:15:02+08:00","action":"inbox","result":"skipped","detail":"not running"}
{"at":"2026-11-08T10:40:11+08:00","action":"hook","result":"handed","detail":"UserPromptSubmit"}
```

| action | results |
|---|---|
| `store` | `written` |
| `inbox` (Claude socket) | `written`, `queued` (seen in the transcript), `unconfirmed`, `unreachable`, `skipped` |
| `codex` | `queued`, `failed`, `skipped` |
| `grok` | `sent` (detail: the gateway message id), `failed`, `skipped` |
| `hook` | `handed` (detail: the hook event) |
| `reach` | `failed` (nothing could deliver it; detail says why) |
| `deadline` | `failed` (the deadline passed) |
| `retry` | `started` (detail `sweep` when the postman did it) |
| `cancel` | `by` (detail: who) |
| `reply` | `sent` (detail: the reply copy's id) |

## settings.json

```json
{ "lang": "zh", "notify": true, "theme": "system", "peer_port": 47831 }
```

`lang` (`zh` / `en`) is the language of everything a person reads, across the app, the CLI and
the text handed to agents; `DISPATCH_LANG` overrides it per process. `notify` turns the app's
system notifications (a reply came in, a delivery failed) on or off. `theme` is the app's
appearance. `peer_port` is the port the listener for other machines binds (default 47831; a
taken port falls back to a free one).
