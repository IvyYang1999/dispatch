//! What a session was asked lately: the last few things its user said to it, each on one
//! line. A session's title is its first message and soon says little about what it is doing
//! now; what it was asked last does. Used to find the right session (a search also looks in
//! these lines) and, where the session shares its questions, to tell candidates apart.
//!
//! Nothing here runs a model or keeps anything in the background: the lines are cut from the
//! tail of the session's own record when a search needs them, and remembered until that file
//! changes. Only sessions on this machine whose record was touched in the last two weeks are
//! read: an older one is found by its name, title or folder as before.

use crate::model::Session;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// How many lines are kept per session: what a search looks in.
pub const KEEP: usize = 12;
/// How many are shown beside a candidate.
pub const SHOWN: usize = 3;
const CLIP: usize = 160;
const WINDOW_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// A record is read from its end, a piece at a time, until enough questions are found. A
/// session that talks has them in the first piece; one that works for hours writes tens of
/// megabytes between two questions, so the search goes on, up to a limit.
const PIECE: u64 = 4 * 1024 * 1024;
const LIMIT: u64 = 48 * 1024 * 1024;

type Stamp = (Option<std::time::SystemTime>, u64);
static CACHE: LazyLock<Mutex<HashMap<String, (Stamp, Vec<String>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// One line, clipped: enough to recognise what was asked.
fn line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= CLIP {
        flat
    } else {
        format!("{}…", flat.chars().take(CLIP).collect::<String>())
    }
}

/// The lines in a stretch of a record: what the user said, newest last. Messages another agent
/// sent through Dispatch are not the user asking, and a pasted image says nothing.
pub fn asks_in(text: &str, harness: crate::model::Harness) -> Vec<String> {
    // most of a record is the agent's own work and what its tools printed: only the lines
    // that can be the user speaking are parsed at all
    let said: Vec<&str> = text.lines().filter(|l| user_line(l, harness)).collect();
    let turns = crate::transcript::parse(&said.join("\n"), harness, usize::MAX);
    let mut out: Vec<String> = turns
        .iter()
        .filter(|t| t.role == "user" && !t.via_dispatch)
        .map(|t| line(&t.text))
        .filter(|l| !l.is_empty() && !l.starts_with("[Image: source:"))
        .collect();
    out.dedup();
    if out.len() > KEEP {
        out.drain(..out.len() - KEEP);
    }
    out
}

/// A line of a record that may hold something the user said, told without parsing it: most
/// of a record is the agent's work, what its tools printed and (Claude Code) whole requests,
/// megabytes a line. A Claude Code user entry that is not a tool's result, or what was typed
/// while the agent worked; a Codex user message; an Antigravity user step. Inside a message
/// these markers are escaped, so text cannot fake them.
fn user_line(l: &str, harness: crate::model::Harness) -> bool {
    use crate::model::Harness as H;
    match harness {
        H::Codex => l.contains(r#""user_message""#) || l.contains(r#""role":"user""#),
        H::Antigravity => l.contains(r#""USER_INPUT""#),
        _ => (l.contains(r#""type":"user""#) && !l.contains(r#""type":"tool_result""#)) || l.contains(r#""queued_command""#),
    }
}

/// The user's lines of a record, read backwards from its end until they hold `KEEP` questions
/// (or the limit, or the start of the file): the lines, oldest first.
fn user_lines(p: &std::path::Path, size: u64, harness: crate::model::Harness) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return vec![] };
    let mut newest_first: Vec<String> = vec![];
    // the start of a line whose end was in the piece read before this one
    let mut carry: Vec<u8> = vec![];
    let mut end = size;
    while end > 0 && size - end < LIMIT {
        let start = end.saturating_sub(PIECE);
        let mut data = vec![0u8; (end - start) as usize];
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut data).is_err() {
            break;
        }
        data.extend_from_slice(&carry);
        // unless this piece starts the file, its first line began in the piece before it
        let whole_from = if start == 0 { 0 } else { data.iter().position(|b| *b == b'\n').map(|i| i + 1).unwrap_or(data.len()) };
        for raw in data[whole_from..].split(|b| *b == b'\n').rev() {
            if raw.is_empty() {
                continue;
            }
            let line = String::from_utf8_lossy(raw);
            if user_line(&line, harness) {
                newest_first.push(line.into_owned());
            }
        }
        carry = data[..whole_from.saturating_sub(1).min(data.len())].to_vec();
        if whole_from == data.len() && start > 0 {
            // no line ended in this piece: all of it belongs to the line being carried
            carry = data;
        }
        end = start;
        let so_far: Vec<&str> = newest_first.iter().rev().map(String::as_str).collect();
        if asks_in(&so_far.join("\n"), harness).len() >= KEEP {
            break;
        }
    }
    newest_first.reverse();
    newest_first
}

/// Whether a search reads this session's record: here, with a record, active lately.
pub fn readable(s: &Session, now: i64) -> bool {
    s.host.is_none() && s.transcript.is_some() && now - s.last_active <= WINDOW_MS
}

/// What this session was asked lately, newest last; empty when it has no record here.
pub fn asks(s: &Session) -> Vec<String> {
    let Some(path) = s.transcript.as_deref().filter(|_| s.host.is_none()) else { return vec![] };
    let p = std::path::Path::new(path);
    let Ok(meta) = std::fs::metadata(p) else { return vec![] };
    let stamp: Stamp = (meta.modified().ok(), meta.len());
    if let Some((at, lines)) = CACHE.lock().unwrap().get(path) {
        if *at == stamp {
            return lines.clone();
        }
    }
    let lines = asks_in(&user_lines(p, meta.len(), s.harness).join("\n"), s.harness);
    let mut cache = CACHE.lock().unwrap();
    if cache.len() > 2000 {
        cache.clear();
    }
    cache.insert(path.to_string(), (stamp, lines.clone()));
    lines
}

/// The newest few, for a list of candidates.
pub fn shown(s: &Session) -> Vec<String> {
    let all = asks(s);
    all[all.len().saturating_sub(SHOWN)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    #[test]
    fn the_last_things_the_user_said_are_kept_one_line_each() {
        let mut lines = vec![];
        for i in 0..20 {
            lines.push(format!(r#"{{"type":"user","message":{{"role":"user","content":"question {i}\n  with a second line"}}}}"#));
            lines.push(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"an answer"}]}}"#.to_string());
        }
        // not the user asking: a message from another agent, an image, a tool result
        lines.push(r#"{"type":"user","message":{"role":"user","content":"📨 From Codex release (via Dispatch, 2026-11-08 13:00, id abcd1234):\n\nbuilt\n\n(This is a message from another agent)"}}"#.to_string());
        lines.push(r#"{"type":"user","message":{"role":"user","content":"[Image: source: /tmp/x.png]"}}"#.to_string());
        lines.push(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#.to_string());
        let asks = asks_in(&lines.join("\n"), Harness::Claude);
        assert_eq!(asks.len(), KEEP);
        assert_eq!(asks.last().map(String::as_str), Some("question 19 with a second line"));
        assert_eq!(asks.first().map(String::as_str), Some("question 8 with a second line"));
        let long = format!(r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#, "长".repeat(400));
        let clipped = asks_in(&long, Harness::Claude);
        assert_eq!(clipped[0].chars().count(), CLIP + 1);
    }

    #[test]
    fn a_record_is_read_again_only_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, r#"{"type":"user","message":{"role":"user","content":"fix the login page"}}"#).unwrap();
        let s = Session { harness: Harness::Claude, id: "a".into(), short_id: "a".into(), name: None, cwd: None, title: None, last_active: crate::model::now_ms(), running: false, desktop: false, host_session_id: None, status: None, transcript: Some(path.to_string_lossy().to_string()), host: None, project: None, app_name: None };
        assert!(readable(&s, crate::model::now_ms()));
        assert_eq!(asks(&s), vec!["fix the login page".to_string()]);
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix the login page\"}}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"now the signup form too\"}}").unwrap();
        assert_eq!(shown(&s), vec!["fix the login page".to_string(), "now the signup form too".to_string()]);
        // a long stretch of the agent's own work after the last question: read further back
        let mut big = String::from("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"the one question, long ago\"}}\n");
        let work = format!("{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}}}\n", "w".repeat(200_000));
        for _ in 0..50 {
            big.push_str(&work);
        }
        std::fs::write(&path, &big).unwrap();
        assert!(big.len() as u64 > 2 * PIECE, "the question is not in the last piece");
        assert_eq!(asks(&s), vec!["the one question, long ago".to_string()]);
        // one on another machine, or left alone for a month, is not read
        let mut away = s.clone();
        away.host = Some(crate::model::Host { id: "p".into(), name: "mini".into() });
        assert!(!readable(&away, crate::model::now_ms()) && asks(&away).is_empty());
        let mut old = s.clone();
        old.last_active -= 30 * 24 * 60 * 60 * 1000;
        assert!(!readable(&old, crate::model::now_ms()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
