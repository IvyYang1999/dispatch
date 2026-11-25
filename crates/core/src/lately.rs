//! What a session was asked lately: the last few things its user said to it, each on one
//! line. A session's title is its first message and soon says little about what it is doing
//! now; what it was asked last does. Used to find the right session (a search also looks in
//! these lines) and, where the session shares its questions, to tell candidates apart.
//!
//! Nothing here runs a model or keeps anything in the background: the lines are cut from the
//! session's own record when a search needs them. Only sessions on this machine whose record
//! was touched in the last two weeks are read: an older one is found by its name, title or
//! folder as before.
//!
//! Reading is the cost: a machine with months of sessions has gigabytes of records, and a
//! search from the command line is a new process every time. So what was read is written down
//! (`cache/asked.json` in Dispatch's own folder, readable by the user only): a record that has
//! not changed is not opened again, and one that has grown is read only from where the last
//! look stopped. The file holds what the transcripts already hold, clipped; deleting it costs
//! one slow search.

use crate::model::Session;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// How many lines are kept per session: what a search looks in. Enough that a long day of
/// short go-aheads ("yes", "merge it") does not push out the question that named the work.
pub const KEEP: usize = 30;
/// A line shorter than this says too little to find a session by ("Signed in", "收到"): it is
/// not kept. Counted in characters, a CJK character as two.
const TELLING: usize = 10;
/// How many are shown beside a candidate.
pub const SHOWN: usize = 3;
const CLIP: usize = 160;
const WINDOW_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// A record is read from its end, a piece at a time, until enough questions are found. A
/// session that talks has them in the first piece; one that works for hours writes tens of
/// megabytes between two questions, so the search goes on, up to a limit.
const PIECE: u64 = 4 * 1024 * 1024;
const LIMIT: u64 = 48 * 1024 * 1024;

/// What is remembered of one record: its length and time when it was looked at, how far it
/// was read (the end of its last whole line), and the questions found.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Kept {
    seen: u64,
    mtime: i64,
    read_to: u64,
    /// A mark of the bytes just before `read_to`: when they are no longer what they were,
    /// this is not the record that was read, however long it is.
    #[serde(default)]
    mark: u64,
    lines: Vec<String>,
}

/// A mark (FNV-1a) of the up to 256 bytes that end at `at`.
fn mark_at(p: &std::path::Path, at: u64) -> u64 {
    use std::io::{Read, Seek, SeekFrom};
    let from = at.saturating_sub(256);
    let mut data = vec![0u8; (at - from) as usize];
    let read = std::fs::File::open(p).and_then(|mut f| f.seek(SeekFrom::Start(from)).and_then(|_| f.read_exact(&mut data)));
    if read.is_err() {
        return 0;
    }
    data.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[derive(Default)]
struct Book {
    loaded: bool,
    dirty: bool,
    records: HashMap<String, Kept>,
}

/// Changes when what is kept of a record changes: a book written under other rules is not used.
const BOOK_VERSION: u64 = 2;

static BOOK: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(Book::default()));

/// Where the book is kept between runs. Tests keep nothing on disk unless they name a file.
fn book_path() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::store::Store::open().root().join("cache").join("asked.json"))
}

fn load_from(path: &std::path::Path) -> HashMap<String, Kept> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return HashMap::new() };
    if v["version"] != BOOK_VERSION {
        return HashMap::new();
    }
    serde_json::from_value(v["records"].clone()).unwrap_or_default()
}

/// Write the book, without the records nobody will ask about again: a file that is gone, or
/// one untouched for longer than a search looks back.
fn save_to(path: &std::path::Path, records: &HashMap<String, Kept>, now: i64) -> std::io::Result<()> {
    use std::io::Write;
    let live: HashMap<&String, &Kept> = records.iter().filter(|(file, k)| now - k.mtime <= WINDOW_MS && std::path::Path::new(file.as_str()).is_file()).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(serde_json::json!({ "version": BOOK_VERSION, "records": live }).to_string().as_bytes())?;
    }
    std::fs::rename(&tmp, path)
}

/// Write down what this run read, when it read anything new. Called once a search is done.
pub fn flush() {
    let Some(path) = book_path() else { return };
    let records = {
        let mut book = BOOK.lock().unwrap();
        if !book.dirty {
            return;
        }
        book.dirty = false;
        book.records.clone()
    };
    let _ = save_to(&path, &records, crate::model::now_ms());
}

/// One line, clipped: enough to recognise what was asked.
fn line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= CLIP {
        flat
    } else {
        format!("{}…", flat.chars().take(CLIP).collect::<String>())
    }
}

/// Whether a line says enough to be worth keeping.
fn telling(l: &str) -> bool {
    l.chars().map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 }).sum::<usize>() >= TELLING
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
        .filter(|l| telling(l) && !l.starts_with("[Image: source:"))
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

/// Read each candidate line once, newest first, and stop as soon as `KEEP` distinct questions
/// are found. Returning parsed questions avoids reparsing the accumulated tail after every
/// chunk and again at the call site. Also where the record's last whole line ended: a later
/// look at a record that has grown starts there.
fn read_questions(p: &std::path::Path, size: u64, harness: crate::model::Harness) -> (Vec<String>, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return (vec![], 0) };
    let mut newest_first: Vec<String> = vec![];
    // the start of a line whose end was in the piece read before this one
    let mut carry: Vec<u8> = vec![];
    let mut end = size;
    let mut read_to = size;
    while end > 0 && size - end < LIMIT {
        let start = end.saturating_sub(PIECE);
        let mut data = vec![0u8; (end - start) as usize];
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut data).is_err() {
            break;
        }
        if end == size {
            // a line still being written has no newline yet: it is read once it is whole
            read_to = match data.iter().rposition(|b| *b == b'\n') {
                Some(i) => start + i as u64 + 1,
                None => start,
            };
        }
        data.extend_from_slice(&carry);
        // unless this piece starts the file, its first line began in the piece before it
        let text = String::from_utf8_lossy(&data);
        let whole_from = if start == 0 { 0 } else { text.find('\n').map(|i| i + 1).unwrap_or(text.len()) };
        for raw in text[whole_from..].rsplit('\n') {
            if user_line(raw, harness) {
                for question in asks_in(raw, harness).into_iter().rev() {
                    if newest_first.last() != Some(&question) {
                        newest_first.push(question);
                    }
                    if newest_first.len() == KEEP {
                        newest_first.reverse();
                        return (newest_first, read_to);
                    }
                }
            }
        }
        // Use the original bytes for a line split across chunks: the split may be inside
        // a UTF-8 character, whose replacement in `text` must not change byte offsets.
        carry = if start == 0 { vec![] } else {
            match data.iter().position(|b| *b == b'\n') {
                Some(i) => data[..i].to_vec(),
                None => data,
            }
        };
        end = start;
    }
    newest_first.reverse();
    (newest_first, read_to)
}

/// The questions in what was added to a record since it was read up to `from`, oldest first,
/// and where the last whole line now ends. None when that stretch cannot be read.
fn read_added(p: &std::path::Path, from: u64, size: u64, harness: crate::model::Harness) -> Option<(Vec<String>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let mut data = vec![0u8; (size - from) as usize];
    f.seek(SeekFrom::Start(from)).ok()?;
    f.read_exact(&mut data).ok()?;
    let whole = data.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
    let text = String::from_utf8_lossy(&data[..whole]);
    let said: Vec<&str> = text.lines().filter(|l| user_line(l, harness)).collect();
    Some((asks_in(&said.join("\n"), harness), from + whole as u64))
}

/// Look at a record: nothing to do when it has not changed, only what was added when it has
/// grown, all of it (from the end, as far back as it takes) the first time or when it is not
/// the file that was read before.
fn look(p: &std::path::Path, harness: crate::model::Harness, size: u64, mtime: i64, before: Option<&Kept>) -> Kept {
    if let Some(k) = before {
        if k.seen == size && k.mtime == mtime {
            return k.clone();
        }
        // records are only ever added to: longer and newer is the same record, grown
        if size >= k.seen && mtime >= k.mtime && k.read_to <= size && size - k.read_to <= LIMIT && k.mark == mark_at(p, k.read_to) {
            if let Some((added, read_to)) = read_added(p, k.read_to, size, harness) {
                let mut lines = k.lines.clone();
                for q in added {
                    if lines.last() != Some(&q) {
                        lines.push(q);
                    }
                }
                if lines.len() > KEEP {
                    lines.drain(..lines.len() - KEEP);
                }
                return Kept { seen: size, mtime, read_to, mark: mark_at(p, read_to), lines };
            }
        }
    }
    let (lines, read_to) = read_questions(p, size, harness);
    Kept { seen: size, mtime, read_to, mark: mark_at(p, read_to), lines }
}

/// Whether a search reads this session's record: here, with a record, active lately.
pub fn readable(s: &Session, now: i64) -> bool {
    s.host.is_none() && s.transcript.is_some() && now - s.last_active <= WINDOW_MS
}

/// What this session was asked lately, newest last; empty when it has no record here.
pub fn asks(s: &Session) -> Vec<String> {
    let Some(path) = s.transcript.as_deref().filter(|_| s.host.is_none()) else { return vec![] };
    // a record Codex packed away is not text: an idle thread is found by its name and title
    if path.ends_with(".zst") {
        return vec![];
    }
    let p = std::path::Path::new(path);
    let Ok(meta) = std::fs::metadata(p) else { return vec![] };
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    let before = {
        let mut book = BOOK.lock().unwrap();
        if !book.loaded {
            book.loaded = true;
            if let Some(file) = book_path() {
                book.records = load_from(&file);
            }
        }
        book.records.get(path).cloned()
    };
    // reading can take a while: not under the lock
    let now = look(p, s.harness, meta.len(), mtime, before.as_ref());
    if before.as_ref() != Some(&now) {
        let mut book = BOOK.lock().unwrap();
        book.records.insert(path.to_string(), now.clone());
        book.dirty = true;
    }
    now.lines
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

    fn said(text: &str) -> String {
        format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}")
    }

    #[test]
    fn a_record_that_grew_is_read_only_from_where_the_last_look_stopped() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let first = format!("{}\n", said("fix the login page"));
        std::fs::write(&path, &first).unwrap();
        let size = |p: &std::path::Path| std::fs::metadata(p).unwrap().len();
        let a = look(&path, Harness::Claude, size(&path), 10, None);
        assert_eq!((a.lines.clone(), a.read_to), (vec!["fix the login page".to_string()], first.len() as u64));
        // unchanged: what was kept is the answer, the file is not consulted
        let untouched = Kept { lines: vec!["KEPT".into()], ..a.clone() };
        assert_eq!(look(&path, Harness::Claude, size(&path), 10, Some(&untouched)), untouched);
        // grown: only the added part is read, and joins what was kept
        let mut text = first.clone() + &said("now the signup form") + "\n";
        std::fs::write(&path, &text).unwrap();
        let b = look(&path, Harness::Claude, size(&path), 20, Some(&untouched));
        assert_eq!(b.lines, vec!["KEPT".to_string(), "now the signup form".to_string()]);
        assert_eq!(b.read_to, text.len() as u64);
        // a line still being written is not read until it is whole, and then once
        text.push_str(&said("and the checkout"));
        std::fs::write(&path, &text).unwrap();
        let c = look(&path, Harness::Claude, size(&path), 30, Some(&b));
        assert_eq!((c.lines.len(), c.read_to), (2, b.read_to));
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        let d = look(&path, Harness::Claude, size(&path), 40, Some(&c));
        assert_eq!(d.lines.last().map(String::as_str), Some("and the checkout"));
        assert_eq!(d.lines.len(), 3);
        // never more than KEEP, the oldest go
        let mut many = text.clone();
        for i in 0..40 {
            many.push_str(&format!("{}\n", said(&format!("question {i}"))));
        }
        std::fs::write(&path, &many).unwrap();
        let e = look(&path, Harness::Claude, size(&path), 50, Some(&d));
        assert_eq!((e.lines.len(), e.lines.last().map(String::as_str)), (KEEP, Some("question 39")));
        // the same length and more, but not the same beginning: not the record that was read
        let other = many.replacen("fix the login page", "FIX THE LOGIN PAGE", 1).replace("question 39", "question XX") + &said("one more thing") + "\n";
        std::fs::write(&path, &other).unwrap();
        let swapped = look(&path, Harness::Claude, size(&path), 55, Some(&e));
        assert_eq!(swapped.lines.last().map(String::as_str), Some("one more thing"));
        assert!(swapped.lines.contains(&"question XX".to_string()) && !swapped.lines.contains(&"question 39".to_string()), "read afresh: {:?}", swapped.lines);
        // a shorter file is not the record that was read: start over
        std::fs::write(&path, format!("{}\n", said("a new beginning"))).unwrap();
        let f = look(&path, Harness::Claude, size(&path), 60, Some(&e));
        assert_eq!(f.lines, vec!["a new beginning".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_was_read_is_kept_between_runs_for_files_still_there() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("here.jsonl");
        std::fs::write(&here, "x").unwrap();
        let now = crate::model::now_ms();
        let kept = |mtime: i64| Kept { seen: 1, mtime, read_to: 1, mark: 7, lines: vec!["fix the login page".into()] };
        let mut records = HashMap::new();
        records.insert(here.to_string_lossy().to_string(), kept(now));
        records.insert(dir.join("gone.jsonl").to_string_lossy().to_string(), kept(now));
        records.insert(dir.join("old.jsonl").to_string_lossy().to_string(), kept(now - 2 * WINDOW_MS));
        std::fs::write(dir.join("old.jsonl"), "x").unwrap();
        let book = dir.join("cache").join("asked.json");
        save_to(&book, &records, now).unwrap();
        let back = load_from(&book);
        assert_eq!(back.len(), 1, "a file that is gone or long untouched is not carried along");
        assert_eq!(back.get(&here.to_string_lossy().to_string()), Some(&kept(now)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&book).unwrap().permissions().mode() & 0o777, 0o600, "the user's questions are for the user only");
        }
        // a book this build does not know, or a broken one, is no book
        std::fs::write(&book, r#"{"version":1,"records":{}}"#).unwrap();
        assert!(load_from(&book).is_empty());
        std::fs::write(&book, "{").unwrap();
        assert!(load_from(&book).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_last_things_the_user_said_are_kept_one_line_each() {
        let mut lines = vec![];
        for i in 0..40 {
            lines.push(format!(r#"{{"type":"user","message":{{"role":"user","content":"question {i}\n  with a second line"}}}}"#));
            lines.push(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"an answer"}]}}"#.to_string());
        }
        // not the user asking: a message from another agent, an image, a tool result
        lines.push(r#"{"type":"user","message":{"role":"user","content":"📨 From Codex release (via Dispatch, 2026-11-08 13:00, id abcd1234):\n\nbuilt\n\n(This is a message from another agent)"}}"#.to_string());
        lines.push(r#"{"type":"user","message":{"role":"user","content":"[Image: source: /tmp/x.png]"}}"#.to_string());
        lines.push(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#.to_string());
        let asks = asks_in(&lines.join("\n"), Harness::Claude);
        assert_eq!(asks.len(), KEEP);
        assert_eq!(asks.last().map(String::as_str), Some("question 39 with a second line"));
        assert_eq!(asks.first().map(String::as_str), Some("question 10 with a second line"));
        // a go-ahead says nothing about the work; a short line in Chinese can
        let short = [said("Signed in"), said("收到"), said("修登录页面"), said("yes do it")].join("\n");
        assert_eq!(asks_in(&short, Harness::Claude), vec!["修登录页面".to_string()]);
        let long = format!(r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#, "长".repeat(400));
        let clipped = asks_in(&long, Harness::Claude);
        assert_eq!(clipped[0].chars().count(), CLIP + 1);
    }

    #[test]
    fn a_record_is_read_again_only_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        // records are written a line at a time, each ending in a newline
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix the login page\"}}\n").unwrap();
        let s = Session { harness: Harness::Claude, id: "a".into(), short_id: "a".into(), name: None, cwd: None, title: None, last_active: crate::model::now_ms(), running: false, desktop: false, host_session_id: None, status: None, transcript: Some(path.to_string_lossy().to_string()), host: None, project: None, app_name: None };
        assert!(readable(&s, crate::model::now_ms()));
        assert_eq!(asks(&s), vec!["fix the login page".to_string()]);
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix the login page\"}}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"now the signup form too\"}}\n").unwrap();
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

    #[test]
    fn backwards_questions_preserve_order_deduplication_and_dispatch_filtering() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-order-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut text = String::new();
        for i in 0..40 {
            for question in [format!("question {i}"), format!("question   {i}")] {
                text.push_str(&serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":question}}).to_string());
                text.push('\n');
            }
            text.push_str(&serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"📨 From Codex release (via Dispatch, 2026-11-08 13:00, id abcd1234):\n\nbuilt\n\n(This is a message from another agent)"}}).to_string());
            text.push('\n');
            if i == 15 {
                // Force a complete non-user line to cross multiple chunk boundaries.
                text.push_str(&serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"文".repeat(PIECE as usize)}}).to_string());
                text.push('\n');
            }
        }
        std::fs::write(&path, &text).unwrap();
        let expected: Vec<String> = (10..40).map(|i| format!("question {i}")).collect();
        assert_eq!(asks_in(&text, Harness::Codex), expected);
        assert_eq!(read_questions(&path, text.len() as u64, Harness::Codex).0, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_question_crossing_utf8_chunk_boundaries_is_not_lost() {
        let dir = std::env::temp_dir().join(format!("dispatch-lately-utf8-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let text = serde_json::json!({"type":"USER_INPUT","content":format!("<USER_REQUEST>{}</USER_REQUEST>", "问".repeat(2 * PIECE as usize / 3))}).to_string();
        std::fs::write(&path, &text).unwrap();
        assert!(text.len() as u64 > 2 * PIECE);
        let expected = vec![format!("{}…", "问".repeat(CLIP))];
        assert_eq!(read_questions(&path, text.len() as u64, Harness::Antigravity).0, expected);
        let _ = std::fs::remove_dir_all(dir);
    }
}
