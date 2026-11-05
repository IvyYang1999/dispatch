//! dispatch-core: find an agent session on this machine and deliver a message into it.
//!
//! - `discover`: the sessions on this machine (Claude Code, Codex), live or recent.
//! - `claude`: a running Claude Code session's inbox socket, its transcript, receipts.
//! - `store`: the message store (one Markdown file per recipient, a run log per message).
//! - `reach`: how a message reaches a session right now (at once, later, not at all).
//! - `hook`: the Claude Code hook that hands waiting messages over.
//! - `send`: the delivery ladder, tying the above together.

pub mod claude;
pub mod codex;
pub mod discover;
pub mod hook;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod reach;
pub mod send;
pub mod store;

pub use model::*;
