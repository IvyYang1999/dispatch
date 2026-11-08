//! dispatch-core: find an agent session on this machine and deliver a message into it.
//!
//! - `discover`: the sessions on this machine (Claude Code, Codex), live or recent.
//! - `claude`: a running Claude Code session's inbox socket, its transcript, receipts.
//! - `store`: the message store (one Markdown file per recipient, a run log per message).
//! - `reach`: how a message reaches a session right now (at once, later, not at all).
//! - `hook`: the Claude Code hook that hands waiting messages over.
//! - `send`: the delivery ladder, tying the above together.
//! - `i18n`: the language of everything a person reads (English, Chinese).
//! - `settings`: the settings file the app writes and the CLI and hook read.
//! - `watch`: what changed in the feed, for the menu bar item and notifications.
//! - `peer`: other machines running Dispatch: pairing, the wire, remote sessions, mirroring.

pub mod claude;
pub mod codex;
pub mod discover;
pub mod grok;
pub mod hook;
pub mod i18n;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod peer;
pub mod reach;
pub mod send;
pub mod settings;
pub mod store;
pub mod watch;

pub use model::*;
