//! A running Claude Code session: its live record, its inbox socket, its transcript.
//!
//! Claude Code lists each live session in `~/.claude/sessions/<pid>.json` with a
//! Unix socket (`messagingSocketPath`) and, since 2.1, a key file
//! `<pid>.<sha256(socket)>.key` whose `peerToken` a sender presents first. A
//! message written to the socket queues behind the session's current turn
//! (priority "next"). A session in bypass-permissions mode holds it for a
//! while instead, waiting for an approval the app has no place for, so a
//! message only counts as handed over once the transcript shows it queued.

use crate::model::{home_dir, is_uuid, now_ms};
use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SENDER: &str = "Dispatch";

#[derive(Debug, Clone)]
pub struct LiveInbox {
    pub session_id: String,
    pub pid: i32,
    pub socket: PathBuf,
    pub token: Option<String>,
    pub cwd: Option<String>,
    pub desktop: bool,
    pub host_session_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub updated_at: i64,
}

pub fn claude_home() -> PathBuf {
    if let Ok(h) = std::env::var("DISPATCH_CLAUDE_HOME") {
        return h.into();
    }
    home_dir().join(".claude")
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // kill(pid, 0) says whether the process exists; EPERM means it does, under another user.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn alive(pid: i32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
        .unwrap_or(false)
}

/// Whether this system can write to a session's inbox socket (Unix sockets). Elsewhere the
/// store and the hook carry every message.
pub fn inbox_supported() -> bool {
    cfg!(unix)
}

fn read_json(path: &Path, max: u64) -> Option<Value> {
    let md = fs::metadata(path).ok()?;
    if !md.is_file() || md.len() > max {
        return None;
    }
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn read_token(dir: &Path, pid: i32, socket: &str) -> Option<String> {
    let hash = hex::encode(Sha256::digest(socket.as_bytes()));
    let v = read_json(&dir.join(format!("{pid}.{hash}.key")), 64 * 1024)?;
    let t = v.get("peerToken")?.as_str()?;
    (t.len() >= 16 && t.len() <= 128 && t.chars().all(|c| c.is_ascii_hexdigit())).then(|| t.to_string())
}

/// Every live session with a usable inbox, newest record first per session.
pub fn live_records() -> Vec<LiveInbox> {
    let dir = claude_home().join("sessions");
    let Ok(entries) = fs::read_dir(&dir) else { return vec![] };
    let mut out: Vec<LiveInbox> = vec![];
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".json") else { continue };
        let Ok(pid) = stem.parse::<i32>() else { continue };
        let Some(r) = read_json(&e.path(), 256 * 1024) else { continue };
        let Some(sid) = r.get("sessionId").and_then(|v| v.as_str()) else { continue };
        if !is_uuid(sid) || r.get("pid").and_then(|v| v.as_i64()) != Some(pid as i64) || !alive(pid) {
            continue;
        }
        let Some(socket) = r.get("messagingSocketPath").and_then(|v| v.as_str()) else { continue };
        let sp = PathBuf::from(socket);
        if !sp.is_absolute() {
            continue;
        }
        #[cfg(unix)]
        {
            let Ok(md) = fs::symlink_metadata(&sp) else { continue };
            use std::os::unix::fs::FileTypeExt;
            if !md.file_type().is_socket() {
                continue;
            }
        }
        let inbox = LiveInbox {
            session_id: sid.to_lowercase(),
            pid,
            socket: sp,
            token: read_token(&dir, pid, socket),
            cwd: r.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
            desktop: r.get("entrypoint").and_then(|v| v.as_str()).map(|s| s.starts_with("claude-desktop")).unwrap_or(false),
            host_session_id: r
                .get("hostSessionId")
                .and_then(|v| v.as_str())
                .filter(|s| s.starts_with("local_"))
                .map(str::to_string),
            name: r.get("name").and_then(|v| v.as_str()).map(str::to_string),
            status: r.get("status").and_then(|v| v.as_str()).map(str::to_string),
            updated_at: r.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0),
        };
        match out.iter_mut().find(|x| x.session_id == inbox.session_id) {
            Some(prev) if prev.updated_at >= inbox.updated_at => {}
            Some(prev) => *prev = inbox,
            None => out.push(inbox),
        }
    }
    out
}

/// The live inbox of one session, or None when it is not running or has no inbox.
pub fn find_live(session_id: &str) -> Option<LiveInbox> {
    let want = session_id.to_lowercase();
    live_records().into_iter().find(|r| r.session_id == want)
}

/// The live record for a process id, if that process is a Claude Code session.
pub fn session_for_pid(pid: i32) -> Option<LiveInbox> {
    live_records().into_iter().find(|r| r.pid == pid)
}

#[cfg(unix)]
fn parent_of(pid: i32) -> Option<i32> {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(windows)]
fn parent_of(pid: i32) -> Option<i32> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').ParentProcessId")])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn my_parent() -> Option<i32> {
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id() as i32)
    }
    #[cfg(windows)]
    {
        parent_of(std::process::id() as i32)
    }
}

/// The Claude Code session this process was started by (an MCP server or a hook is its
/// child, sometimes through a shell): walk up the parents until one has a live record.
pub fn caller_session() -> Option<LiveInbox> {
    let mut pid = my_parent()?;
    for _ in 0..6 {
        if pid <= 1 {
            return None;
        }
        if let Some(r) = session_for_pid(pid) {
            return Some(r);
        }
        pid = parent_of(pid)?;
    }
    None
}

/// Is the session's process alive at all (with or without an inbox)?
pub fn is_running(session_id: &str) -> bool {
    let want = session_id.to_lowercase();
    let dir = claude_home().join("sessions");
    let Ok(entries) = fs::read_dir(&dir) else { return false };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".json") else { continue };
        let Ok(pid) = stem.parse::<i32>() else { continue };
        let Some(r) = read_json(&e.path(), 256 * 1024) else { continue };
        if r.get("sessionId").and_then(|v| v.as_str()).map(|s| s.to_lowercase()) == Some(want.clone()) && alive(pid) {
            return true;
        }
    }
    false
}

/// What goes down the socket: the auth line (when there is a key), then the message as a user turn.
pub fn frames(inbox: &LiveInbox, text: &str, msg_id: &str) -> String {
    let mut lines = String::new();
    if let Some(t) = &inbox.token {
        lines.push_str(&serde_json::json!({ "type": "auth", "token": t }).to_string());
        lines.push('\n');
    }
    lines.push_str(
        &serde_json::json!({
            "type": "user",
            "session_id": inbox.session_id,
            "uuid": msg_id,
            "msg_id": msg_id,
            "from": SENDER,
            "priority": "next",
            "message": { "role": "user", "content": text }
        })
        .to_string(),
    );
    lines.push('\n');
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The frames were written: the session may have it, so never send again.
    Sent,
    /// Nothing was written (a stale record): the message can go another way.
    Unreachable,
}

#[cfg(windows)]
pub fn send(_inbox: &LiveInbox, _text: &str, _msg_id: &str, _timeout: Duration) -> SendOutcome {
    SendOutcome::Unreachable
}

#[cfg(unix)]
pub fn send(inbox: &LiveInbox, text: &str, msg_id: &str, timeout: Duration) -> SendOutcome {
    let Ok(mut s) = UnixStream::connect(&inbox.socket) else { return SendOutcome::Unreachable };
    let _ = s.set_write_timeout(Some(timeout));
    let _ = s.set_read_timeout(Some(timeout));
    if s.write_all(frames(inbox, text, msg_id).as_bytes()).is_err() {
        return SendOutcome::Unreachable;
    }
    let _ = s.shutdown(std::net::Shutdown::Write);
    // The session sends nothing back on success; drain whatever it might.
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    SendOutcome::Sent
}

// ---- Transcripts ----

/// Claude Code's project folder for a cwd: every non-alphanumeric character becomes '-'.
pub fn project_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `~/.claude/projects/<slug>/<id>.jsonl`, by cwd when known, else by searching every project.
pub fn transcript_path(session_id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let projects = claude_home().join("projects");
    if let Some(cwd) = cwd {
        let p = projects.join(project_slug(cwd)).join(format!("{session_id}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
    }
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    for e in fs::read_dir(&projects).ok()?.flatten() {
        let p = e.path().join(format!("{session_id}.jsonl"));
        if let Ok(md) = fs::metadata(&p) {
            let m = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().map(|(_, t)| m > *t).unwrap_or(true) {
                best = Some((p, m));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// The end of a file, whole lines only.
pub fn read_tail(path: &Path, bytes: u64) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let size = f.metadata()?.len();
    let start = size.saturating_sub(bytes);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(if start > 0 {
        match text.find('\n') {
            Some(i) => text[i + 1..].to_string(),
            None => String::new(),
        }
    } else {
        text
    })
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Did the session queue the message? It writes the queue entry to its transcript right
/// away, even mid-turn; a held or refused message leaves no trace until it is let in.
pub fn queued(session_id: &str, cwd: Option<&str>, msg_id: &str, text: &str, timeout: Duration) -> bool {
    let want = squash(text);
    let since = now_ms() - 5000;
    let deadline = Instant::now() + timeout;
    let mut file: Option<PathBuf> = None;
    loop {
        if file.is_none() {
            file = transcript_path(session_id, cwd);
        }
        if let Some(f) = &file {
            let tail = read_tail(f, 1024 * 1024).unwrap_or_default();
            for line in tail.lines() {
                if !line.contains(msg_id) && !line.contains("\"enqueue\"") {
                    continue;
                }
                let Ok(r) = serde_json::from_str::<Value>(line) else { continue };
                if r.get("uuid").and_then(|v| v.as_str()) == Some(msg_id)
                    || r.pointer("/origin/msg_id").and_then(|v| v.as_str()) == Some(msg_id)
                {
                    return true;
                }
                if r.get("type").and_then(|v| v.as_str()) == Some("queue-operation")
                    && r.get("operation").and_then(|v| v.as_str()) == Some("enqueue")
                    && r.get("content").and_then(|v| v.as_str()).map(squash) == Some(want.clone())
                {
                    let ts = r
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.timestamp_millis())
                        .unwrap_or(0);
                    if ts >= since {
                        return true;
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Modes already read, keyed by transcript path, valid while the file's mtime and size hold.
static MODES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, (i64, u64, Option<String>)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// The permission mode of the session's latest turn, as its transcript records it.
/// Every turn writes it, so the last half megabyte is enough.
pub fn mode(session_id: &str, cwd: Option<&str>) -> Option<String> {
    let file = transcript_path(session_id, cwd)?;
    let md = fs::metadata(&file).ok()?;
    let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
    let size = md.len();
    if let Some((m, s, mode)) = MODES.lock().unwrap().get(&file) {
        if *m == mtime && *s == size {
            return mode.clone();
        }
    }
    let cached = MODES.lock().unwrap().get(&file).cloned();
    let mode = match cached {
        // the file only grew: a newer mode can only be in the new bytes, else the old one stands
        Some((_, old_size, Some(old))) if size >= old_size => mode_in_range(&file, old_size, size).or(Some(old)),
        _ => mode_from_file(&file),
    };
    MODES.lock().unwrap().insert(file, (mtime, size, mode.clone()));
    mode
}

const MODE_CHUNK: u64 = 512 * 1024;
/// How far back a transcript is read for its mode before giving up (the record is written
/// once per mode change, so it is usually near the start of a long file).
const MODE_SCAN_MAX: u64 = 64 * 1024 * 1024;

fn last_mode_in(text: &str) -> Option<String> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r#""permissionMode":"([A-Za-z]+)""#).unwrap());
    RE.captures_iter(text).last().map(|c| c[1].to_string())
}

fn read_range(path: &Path, from: u64, to: u64) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = vec![0u8; (to - from) as usize];
    f.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

/// The last `permissionMode` written between two offsets.
fn mode_in_range(path: &Path, from: u64, to: u64) -> Option<String> {
    if to <= from {
        return None;
    }
    // a record straddling the old end is still seen
    let from = from.saturating_sub(4096);
    last_mode_in(&read_range(path, from, to)?)
}

/// The last `permissionMode` anywhere in the file: read backwards in half-megabyte pieces
/// (each overlapping the next by a few KB so a record on the seam is not missed).
pub fn mode_from_file(path: &Path) -> Option<String> {
    let size = fs::metadata(path).ok()?.len();
    let mut end = size;
    let mut scanned = 0u64;
    while end > 0 && scanned < MODE_SCAN_MAX {
        let start = end.saturating_sub(MODE_CHUNK);
        let text = read_range(path, start, (end + 4096).min(size))?;
        if let Some(m) = last_mode_in(&text) {
            return Some(m);
        }
        scanned += end - start;
        end = start;
    }
    None
}

/// Claude Code's `crossSessionInbound` in the user's settings: `accept`, `hold`, `refuse`, or None.
pub fn inbound_setting() -> Option<String> {
    read_json(&claude_home().join("settings.json"), 1024 * 1024)?.get("crossSessionInbound")?.as_str().map(str::to_string)
}

/// Set it, or clear it with None. The settings file is backed up first, like the hook does.
pub fn set_inbound(value: Option<&str>) -> Result<()> {
    let path = claude_home().join("settings.json");
    let mut v: Value = match fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).context("settings.json is not valid JSON")?,
        Err(_) => serde_json::json!({}),
    };
    if path.exists() {
        let bak = claude_home().join(format!("settings.json.bak-dispatch-{}", chrono::Local::now().format("%Y%m%dT%H%M%S")));
        fs::copy(&path, &bak)?;
    }
    let obj = v.as_object_mut().context("settings.json root is not an object")?;
    match value {
        Some(x) => {
            obj.insert("crossSessionInbound".into(), serde_json::json!(x));
        }
        None => {
            obj.remove("crossSessionInbound");
        }
    }
    fs::create_dir_all(claude_home())?;
    fs::write(&path, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Accept,
    Hold,
    Refuse,
}

/// What the session would do with a message from us. Claude Code holds messages while
/// the session runs in bypassPermissions unless `crossSessionInbound` says otherwise;
/// an unknown mode counts as holding, but is reported as unknown, not as bypassing.
pub fn gate(session_id: &str, cwd: Option<&str>) -> Gate {
    gate_reason(session_id, cwd).0
}

/// The gate and why: `crossSessionInbound=accept|hold|refuse`, `bypassPermissions`, `mode
/// unknown`, or `mode=<the mode>`.
pub fn gate_reason(session_id: &str, cwd: Option<&str>) -> (Gate, String) {
    let settings = read_json(&claude_home().join("settings.json"), 1024 * 1024);
    match settings.as_ref().and_then(|s| s.get("crossSessionInbound")).and_then(|v| v.as_str()) {
        Some("accept") => return (Gate::Accept, "crossSessionInbound=accept".into()),
        Some("refuse") => return (Gate::Refuse, "crossSessionInbound=refuse".into()),
        Some("hold") => return (Gate::Hold, "crossSessionInbound=hold".into()),
        _ => {}
    }
    match mode(session_id, cwd).as_deref() {
        None => (Gate::Hold, "mode unknown".into()),
        Some("bypassPermissions") => (Gate::Hold, "bypassPermissions".into()),
        Some(m) => (Gate::Accept, format!("mode={m}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    /// The message is not in the last 4 MiB of the transcript: held, refused, or out of range.
    NotFound,
    /// Found, nothing after it yet.
    Queued,
    /// The session has started answering it.
    Started,
    /// The session ended the turn it started on this message.
    Completed { turn_id: String },
}

/// One transcript row, as much of it as following a message needs.
struct Row {
    user: bool,
    assistant: bool,
    meta: bool,
    end_turn: bool,
    uuid: Option<String>,
    parent: Option<String>,
}

/// `message.stop_reason`, whatever else the message holds (and whatever it is: an object, or not).
#[derive(Default)]
struct StopReason(Option<String>);

impl<'de> serde::Deserialize<'de> for StopReason {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = StopReason;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a message")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<StopReason, A::Error> {
                let mut out = None;
                while let Some(key) = map.next_key::<std::borrow::Cow<str>>()? {
                    if key == "stop_reason" {
                        out = map.next_value::<Option<String>>().unwrap_or(None);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(StopReason(out))
            }
            fn visit_str<E>(self, _: &str) -> std::result::Result<StopReason, E> {
                Ok(StopReason(None))
            }
            fn visit_unit<E>(self) -> std::result::Result<StopReason, E> {
                Ok(StopReason(None))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<StopReason, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(StopReason(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// The fields of a row that are read; the rest (the text, the tool output) is skipped unparsed.
#[derive(serde::Deserialize)]
struct RawRow {
    #[serde(rename = "type", default)]
    ty: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(rename = "parentUuid", default)]
    parent: Option<String>,
    #[serde(rename = "isMeta", default)]
    meta: Option<bool>,
    #[serde(default)]
    message: StopReason,
}

fn rows_of(tail: &str) -> Vec<Row> {
    tail.lines()
        .filter_map(|l| serde_json::from_str::<RawRow>(l).ok())
        .map(|r| Row {
            user: r.ty.as_deref() == Some("user"),
            assistant: r.ty.as_deref() == Some("assistant"),
            meta: r.meta.unwrap_or(false),
            end_turn: r.message.0.as_deref() == Some("end_turn"),
            uuid: r.uuid,
            parent: r.parent,
        })
        .collect()
}

/// The last 4 MiB of each transcript, read once per version of the file (its size and time).
/// Every copy delivered to a session asks about the same file, and the app asks every few
/// seconds; parsing it for each of them was most of the app's idle work.
type Tails = std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, std::sync::Arc<Vec<Row>>)>;
static TAILS: std::sync::LazyLock<std::sync::Mutex<Tails>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
/// Where a session's transcript was last found, so the projects folder is not searched each time.
static TRANSCRIPTS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, PathBuf>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn transcript_of(session_id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    if cwd.is_none() {
        if let Some(p) = TRANSCRIPTS.lock().ok().and_then(|m| m.get(session_id).cloned()) {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let p = transcript_path(session_id, cwd)?;
    if let Ok(mut m) = TRANSCRIPTS.lock() {
        m.insert(session_id.to_string(), p.clone());
    }
    Some(p)
}

fn tail_rows(file: &Path) -> Result<std::sync::Arc<Vec<Row>>> {
    let md = fs::metadata(file)?;
    let (len, modified) = (md.len(), md.modified().ok());
    if let Some((l, m, rows)) = TAILS.lock().ok().and_then(|t| t.get(file).cloned()) {
        if l == len && m == modified {
            return Ok(rows);
        }
    }
    let rows = std::sync::Arc::new(rows_of(&read_tail(file, 4 * 1024 * 1024)?));
    if let Ok(mut t) = TAILS.lock() {
        // a handful of sessions are live at once; do not keep every file ever asked about
        if t.len() >= 64 {
            t.clear();
        }
        t.insert(file.to_path_buf(), (len, modified, rows.clone()));
    }
    Ok(rows)
}

/// Follow the message through the transcript: port of Dark Console's refresh.
pub fn receipt(session_id: &str, cwd: Option<&str>, msg_id: &str) -> Result<Receipt> {
    let file = transcript_of(session_id, cwd).context("transcript not found")?;
    Ok(receipt_in(&tail_rows(&file)?, msg_id))
}

fn receipt_in(rows: &[Row], msg_id: &str) -> Receipt {
    let Some(start) = rows.iter().position(|r| r.user && r.uuid.as_deref() == Some(msg_id)) else {
        return Receipt::NotFound;
    };
    let mut chain: std::collections::HashSet<&str> = std::collections::HashSet::new();
    chain.insert(msg_id);
    let mut answered = false;
    for row in &rows[start + 1..] {
        let Some(parent) = row.parent.as_deref() else { continue };
        if !chain.contains(parent) {
            continue;
        }
        if row.user && !row.meta {
            break;
        }
        if let Some(u) = row.uuid.as_deref() {
            chain.insert(u);
        }
        if row.assistant {
            answered = true;
            if row.end_turn {
                return Receipt::Completed { turn_id: row.uuid.clone().unwrap_or_default() };
            }
        }
    }
    if answered {
        Receipt::Started
    } else {
        Receipt::Queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_followed_through_the_rows_after_it() {
        let m = "11111111-2222-4333-8444-555555555555";
        let user = format!(r#"{{"type":"user","uuid":"{m}","parentUuid":"p0","message":{{"role":"user","content":"hello"}}}}"#);
        let other = r#"{"type":"user","uuid":"zz","parentUuid":"p0","message":"a plain string, as some rows carry"}"#;
        let working = r#"{"type":"assistant","uuid":"a1","parentUuid":"11111111-2222-4333-8444-555555555555","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"on it"}]}}"#;
        let tool = r#"{"type":"user","isMeta":true,"uuid":"t1","parentUuid":"a1","message":{"content":[{"type":"tool_result","content":"x"}]}}"#;
        let done = r#"{"type":"assistant","uuid":"a2","parentUuid":"t1","message":{"stop_reason":"end_turn","content":[]}}"#;
        let state = |lines: &[&str]| receipt_in(&rows_of(&lines.join("\n")), m);
        assert_eq!(state(&[other]), Receipt::NotFound);
        assert_eq!(state(&[other, &user]), Receipt::Queued);
        assert_eq!(state(&[&user, working, "not json at all"]), Receipt::Started);
        assert_eq!(state(&[&user, working, tool, done]), Receipt::Completed { turn_id: "a2".into() });
        // the next thing the person says ends the chain: a later end_turn is not this message's
        let next = r#"{"type":"user","uuid":"u2","parentUuid":"a1","message":{"content":"and another thing"}}"#;
        let late = r#"{"type":"assistant","uuid":"a3","parentUuid":"u2","message":{"stop_reason":"end_turn"}}"#;
        assert_eq!(state(&[&user, working, next, late]), Receipt::Started);
    }

    #[test]
    fn a_transcript_is_read_once_per_version_of_the_file() {
        let dir = std::env::temp_dir().join(format!("dispatch-tail-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        fs::write(&file, "{\"type\":\"user\",\"uuid\":\"m1\"}\n").unwrap();
        let first = tail_rows(&file).unwrap();
        let again = tail_rows(&file).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &again), "unchanged file: the parsed rows are reused");
        fs::write(&file, "{\"type\":\"user\",\"uuid\":\"m1\"}\n{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"m1\"}\n").unwrap();
        let grown = tail_rows(&file).unwrap();
        assert_eq!(grown.len(), 2, "the file grew: read again");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn slug_matches_claude_code() {
        assert_eq!(project_slug("/Users/creator/vaults/main-2026-10-05/项目/开发项目/Agent信箱"), "-Users-creator-vaults-main-2026-10-05---------Agent--");
    }

    #[test]
    fn the_mode_is_found_however_far_back_and_the_newest_wins() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("dispatch-mode-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        let mut text = String::from("{\"type\":\"user\",\"permissionMode\":\"auto\",\"message\":{\"content\":\"hi\"}}\n");
        let filler = format!("{{\"type\":\"assistant\",\"message\":{{\"content\":\"{}\"}}}}\n", "x".repeat(1000));
        for _ in 0..700 {
            text.push_str(&filler);
        }
        fs::write(&file, &text).unwrap();
        assert!(fs::metadata(&file).unwrap().len() > 512 * 1024, "the mode line is outside the old half-megabyte window");
        assert_eq!(mode_from_file(&file).as_deref(), Some("auto"));
        assert_eq!(mode_in_range(&file, 8192, fs::metadata(&file).unwrap().len()), None, "nothing newer after the first line");
        let before = fs::metadata(&file).unwrap().len();
        fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b"{\"type\":\"user\",\"permissionMode\":\"bypassPermissions\"}\n").unwrap();
        assert_eq!(mode_in_range(&file, before, fs::metadata(&file).unwrap().len()).as_deref(), Some("bypassPermissions"), "the appended bytes carry the newer mode");
        assert_eq!(mode_from_file(&file).as_deref(), Some("bypassPermissions"));
        fs::write(&file, "{\"type\":\"user\"}\n").unwrap();
        assert_eq!(mode_from_file(&file), None, "no record: unknown, not bypass");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn frames_carry_auth_then_user() {
        let inbox = LiveInbox {
            session_id: "s".into(),
            pid: 1,
            socket: "/tmp/x".into(),
            token: Some("abcd".into()),
            cwd: None,
            desktop: false,
            host_session_id: None,
            name: None,
            status: None,
            updated_at: 0,
        };
        let f = frames(&inbox, "hi", "m1");
        let lines: Vec<&str> = f.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"auth\""));
        assert!(lines[1].contains("\"priority\":\"next\""));
    }
}
