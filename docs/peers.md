# Peers: Dispatch across machines

Dispatch keeps one store per machine. A message to a session on another machine is a
letter that travels into that machine's store, where the local ladder (inbox socket, hook,
Codex daemon, Grok gateway) delivers it as if it had been written there. Replies travel
back the same way. Nothing in the message format, the hook or the ladder changes; a letter
only learns which machine it came from and which it is going to.

No server, no account: machines on the same network find each other with Bonjour / mDNS,
pair once with a six-digit code, and talk directly over an encrypted connection. Machines
on different networks can be given an address by hand (over Tailscale, a VPN, or a port
forward).

## Words

- **Peer**: another machine running Dispatch (the app, or `dispatch serve`).
- **Me**: this machine, with a stable id, a name people read (the computer's name by
  default) and a key pair. `peers.json` in the store folder holds me and the paired peers;
  `peer.key` beside it holds the private key (mode 0600, never leaves the machine).
- **Address**: `<peer name>/<session>`, where the session part is anything `dispatch send`
  already takes (an id, an id prefix, a desktop name, a title fragment). `主力/a1b2c3d4`,
  `Mac mini/release`. A bare session id that only one peer has also resolves.

## Who listens

The listener lives in the process that stays up: the desktop app starts it at launch, and
`dispatch serve` runs it in the foreground where there is no app (a Linux box, a test). It
binds TCP port 47831 (settings key `peer_port`; falls back to a free port when that one is
taken), advertises `_dispatch._tcp` on the local network with the machine's id and name,
and keeps browsing for other machines so the command line can find them without a scan.

Sending needs no listener on the sending side: the command line connects straight to the
peer. Receiving replies does not either, because the sender pulls the peer's state when it
reads the feed (see Mirroring). A listener on both sides only makes replies arrive at once.

## Discovery and pairing

1. Both machines run Dispatch. Each sees the other under "Other machines" in Settings
   (`dispatch peers scan` on the command line), unpaired.
2. On machine A: Settings → Show pairing code (`dispatch peers code`). A six-digit code
   appears, good for two minutes, written to `peer/pairing.json` in A's store folder.
3. On machine B: pick A and enter the code (`dispatch peers pair "<A's name>" 123456`, or
   `dispatch peers pair 192.168.1.20:47831 123456` for a machine mDNS cannot see).
4. B opens a connection to A and completes the Noise handshake, so both now hold each
   other's static public key over an encrypted channel. B sends `Pair { proof }` where
   `proof = HMAC-SHA256(key = code, message = handshake hash)`. A checks the proof against
   the code in `pairing.json` (constant time), stores B as a peer, answers `Paired` with its
   own id and name, and deletes the code. B stores A. Three wrong proofs also delete the
   code, so a code can be guessed at most three times out of a million.

The handshake hash binds the proof to this very connection: someone in the middle cannot
reuse B's proof on their own connection to A.

## Wire

- TCP, one request per connection. Frames are `u16` big-endian length + bytes.
- `Noise_XX_25519_ChaChaPoly_BLAKE2s` with prologue `dispatch/1`, three handshake frames,
  then transport mode. A message is sent as its `u32` length (one frame) followed by the
  encrypted bytes in frames of at most 60000 plaintext bytes each.
- Requests and responses are JSON objects with a `type` field:

| Request | Who may send it | Response |
|---|---|---|
| `hello` | anyone | `welcome { id, name, version }` |
| `pair { proof, id, name }` | anyone, while a code is active | `paired { id, name }` or `error` |
| `sessions` | a paired peer | `sessions { hook_installed, sessions: [{ session, reach }] }` |
| `deliver { letter }` | a paired peer | `delivered { status, route, detail }` |
| `status { dispatch_id }` | a paired peer | `status { copies: [{ letter, kind, stage_key }] }` |
| `cancel { letter_id }` | a paired peer | `ok` |

Every connection ends with the peer's static key checked against `peers.json`; anything
but `hello` and `pair` from an unknown key is answered with an error and closed. Bodies are
capped at 256 KB; a connection gets ten seconds per read and write.

## Sending

`dispatch send --to "主力/release" "…"` (or `dispatch_send` with that address) resolves the
peer and, from the peer's session list, the session. The letter is written to the local
store with `to_host` / `to_host_name` set, then pushed with `deliver`. The peer stores the
same letter (same id, same dispatch id) with `from_host` set to the sender's id as proven by
the handshake, never from the payload, and runs its ladder on it. The response carries the
peer's status for the copy, which becomes the local copy's status, with a run line
`peer <name> <route>`.

When the peer cannot be reached the copy stays queued with `peer unreachable` in its log.
The sweep retries it when the peer shows up again on the network (same five-minute spacing
as local retries); the deadline still applies, so a copy nobody could deliver in time is
marked not delivered like any other.

A letter addressed to a machine arrives as a normal letter for the session on it. The
envelope names the sender and its machine (`Name @ Machine`); the reply instructions are
unchanged. `dispatch reply` on that machine stores the reply with `to_host` set to the
origin and pushes it back the same way.

## Mirroring

The machine that delivers a copy owns its status. When the feed, `status` or the sweep
reads a dispatch that has copies on another machine, it asks that machine (`status`) at
most every ten seconds per dispatch, then:

- updates the local copy's status to the peer's, and remembers the peer's `kind` and
  `stage_key` for display (the origin cannot read the peer's transcripts);
- writes any copy it does not have yet, such as a reply, into the local store with its
  original ids, and delivers it locally when it is addressed to a session here.

A peer that does not answer is left alone for twenty seconds before it is asked again, so
an offline machine never stalls the feed. A reply pushed by the peer and the same reply
pulled by mirroring are the same letter id, so nothing is delivered twice.

## Settings and command line

- Settings → Other machines: this machine's name, listening port, machines seen on the
  network (pair / paired / offline), the pairing code, manual addresses, remove.
- `dispatch peers` lists me and the peers; `peers name <name>`, `peers scan`, `peers code`,
  `peers pair <name-or-address> <code>`, `peers add <name> <host:port>` (a manual address
  for a paired peer), `peers remove <name>`.
- `dispatch serve [--port N]` runs the listener in the foreground.
- `dispatch sessions` shows remote sessions with their machine; `send`, `status`, `inbox`,
  `reply`, `retry`, `cancel` take remote addresses and ids like local ones.

## What it does not do yet

- Push notifications of replies to a sender that is not listening (the sender pulls).
- Relaying through a third machine or a server; machines must reach each other directly.
- Running the listener as a system service without the app (`dispatch serve` is a
  foreground command; a launchd / systemd unit is the user's).
