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
    let tail = read_tail(&file, 512 * 1024).ok()?;
    let re = regex::Regex::new(r#""permissionMode":"([A-Za-z]+)""#).ok()?;
    let mode = re.captures_iter(&tail).last().map(|c| c[1].to_string());
    MODES.lock().unwrap().insert(file, (mtime, size, mode.clone()));
    mode
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Accept,
    Hold,
    Refuse,
}

/// What the session would do with a message from us. Claude Code holds messages while
/// the session runs in bypassPermissions unless `crossSessionInbound` says otherwise;
/// an unknown mode counts as holding.
pub fn gate(session_id: &str, cwd: Option<&str>) -> Gate {
    let settings = read_json(&claude_home().join("settings.json"), 1024 * 1024);
    match settings.as_ref().and_then(|s| s.get("crossSessionInbound")).and_then(|v| v.as_str()) {
        Some("accept") => return Gate::Accept,
        Some("refuse") => return Gate::Refuse,
        Some("hold") => return Gate::Hold,
        _ => {}
    }
    match mode(session_id, cwd).as_deref() {
        None | Some("bypassPermissions") => Gate::Hold,
        _ => Gate::Accept,
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

/// Follow the message through the transcript: port of Dark Console's refresh.
pub fn receipt(session_id: &str, cwd: Option<&str>, msg_id: &str) -> Result<Receipt> {
    let file = transcript_path(session_id, cwd).context("transcript not found")?;
    let tail = read_tail(&file, 4 * 1024 * 1024)?;
    let rows: Vec<Value> = tail.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let Some(start) = rows
        .iter()
        .position(|r| r.get("type").and_then(|v| v.as_str()) == Some("user") && r.get("uuid").and_then(|v| v.as_str()) == Some(msg_id))
    else {
        return Ok(Receipt::NotFound);
    };
    let mut chain: std::collections::HashSet<String> = std::collections::HashSet::new();
    chain.insert(msg_id.to_string());
    let mut answered = false;
    for row in &rows[start + 1..] {
        let Some(parent) = row.get("parentUuid").and_then(|v| v.as_str()) else { continue };
        if !chain.contains(parent) {
            continue;
        }
        let ty = row.get("type").and_then(|v| v.as_str());
        if ty == Some("user") && !row.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false) {
            break;
        }
        if let Some(u) = row.get("uuid").and_then(|v| v.as_str()) {
            chain.insert(u.to_string());
        }
        if ty == Some("assistant") {
            answered = true;
            if row.pointer("/message/stop_reason").and_then(|v| v.as_str()) == Some("end_turn") {
                return Ok(Receipt::Completed { turn_id: row.get("uuid").and_then(|v| v.as_str()).unwrap_or("").to_string() });
            }
        }
    }
    Ok(if answered { Receipt::Started } else { Receipt::Queued })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_claude_code() {
        assert_eq!(project_slug("/Users/creator/vaults/main-2026-10-05/项目/开发项目/Agent信箱"), "-Users-creator-vaults-main-2026-10-05---------Agent--");
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
