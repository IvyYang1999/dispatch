//! Codex sessions: the app-server (the shared daemon over its Unix WebSocket, or a
//! private stdio server for read-only questions), the official `codex queue` command, and
//! receipts read from the thread's rollout file.
//!
//! The one thing to get right (the main Mac's finding): a thread's history being readable
//! does not mean the server we talk to is the one running it. The desktop app reports a
//! chat as active while the default daemon says `notLoaded`. So a message goes to a Codex
//! thread only when `thread/loaded/list` on the shared daemon names it; otherwise it waits
//! in the store as 待接通, and `--resume` is an explicit choice, never the default.

use crate::model::{home_dir, now_ms};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Mutex};
#[cfg(unix)]
use tungstenite::{Message, WebSocket};
use std::time::{Duration, Instant};

/// Finder-launched apps do not inherit the terminal's PATH. Prefer PATH, then standard
/// CLI locations and the installed Codex desktop bundles; use the same executable everywhere.
/// A file this user can run: on Unix an executable bit, on Windows any file (`.exe`, `.cmd`).
pub(crate) fn is_executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `name` as it would be installed on this system: `codex` on Unix, `codex.exe` / `codex.cmd` on Windows too.
pub(crate) fn command_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()]
    } else {
        vec![name.to_string()]
    }
}

pub fn executable() -> Option<PathBuf> {
    let names = command_names("codex");
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).flat_map(|dir| names.iter().map(move |n| dir.join(n))).collect()).unwrap_or_default();
    candidates.extend([home_dir().join(".local/bin/codex"), PathBuf::from("/opt/homebrew/bin/codex"), PathBuf::from("/usr/local/bin/codex")]);
    #[cfg(target_os = "macos")]
    for root in [PathBuf::from("/Applications"), home_dir().join("Applications")] {
        candidates.push(root.join("Codex.app/Contents/Resources/codex"));
        candidates.push(root.join("ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"));
    }
    candidates.into_iter().find(|p| is_executable(p))
}

pub fn command() -> Command {
    Command::new(executable().unwrap_or_else(|| PathBuf::from("codex")))
}

pub fn daemon_socket() -> PathBuf {
    home_dir().join(".codex").join("app-server-control").join("app-server-control.sock")
}

/// A JSON-RPC session with an app-server: `shared` when it is the daemon every Codex client
/// on this machine can share, false when it is a private server we spawned (reads only).
enum Transport {
    #[cfg(unix)]
    Shared(WebSocket<UnixStream>),
    Private { child: Child, stdin: ChildStdin, incoming: mpsc::Receiver<String> },
}

pub struct Server {
    transport: Transport,
    next_id: u64,
    pub shared: bool,
}

impl Drop for Server {
    #[allow(irrefutable_let_patterns)]
    fn drop(&mut self) {
        if let Transport::Private { child, .. } = &mut self.transport {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Server {
    fn initialize(transport: Transport, shared: bool) -> Result<Server> {
        let mut s = Server { transport, next_id: 1, shared };
        s.call("initialize", json!({ "clientInfo": { "name": "dispatch", "version": env!("CARGO_PKG_VERSION"), "title": "Dispatch" }, "capabilities": { "experimentalApi": true } }), Duration::from_secs(10))?;
        s.write(json!({ "method": "initialized", "params": {} }), Duration::from_secs(2))?;
        Ok(s)
    }

    #[cfg(windows)]
    fn shared() -> Result<Server> {
        Err(anyhow!("the Codex daemon socket is not supported on Windows"))
    }

    #[cfg(unix)]
    fn shared() -> Result<Server> {
        let stream = UnixStream::connect(daemon_socket()).context("connect Codex daemon")?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let config = tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(8 * 1024 * 1024));
        let (ws, _) = tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))
            .map_err(|e| anyhow!("Codex WebSocket handshake: {e}"))?;
        Self::initialize(Transport::Shared(ws), true)
    }

    /// Never disguise an unreachable shared daemon as an empty private server.
    pub fn connect() -> Result<Server> {
        if daemon_socket().exists() { return Self::shared(); }
        let mut child = command().arg("app-server")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().context("codex not found on PATH")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let (tx, incoming) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.by_ref().take(8 * 1024 * 1024).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if !line.ends_with('\n') => break,
                    Ok(_) => if tx.send(line).is_err() { break; },
                }
            }
        });
        Self::initialize(Transport::Private { child, stdin, incoming }, false)
    }

    #[cfg_attr(windows, allow(unused_variables))]
    fn write(&mut self, value: Value, timeout: Duration) -> Result<()> {
        match &mut self.transport {
            #[cfg(unix)]
            Transport::Shared(ws) => {
                ws.get_ref().set_write_timeout(Some(timeout))?;
                ws.send(Message::Text(value.to_string().into()))?;
            }
            Transport::Private { stdin, .. } => {
                writeln!(stdin, "{value}")?;
                stdin.flush()?;
            }
        }
        Ok(())
    }

    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let deadline = Instant::now() + timeout;
        self.write(json!({ "id": id, "method": method, "params": params }), timeout)?;
        loop {
            let remaining = deadline.checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero()).ok_or_else(|| anyhow!("{method}: no answer within {timeout:?}"))?;
            let text = match &mut self.transport {
                #[cfg(unix)]
                Transport::Shared(ws) => {
                    ws.get_ref().set_read_timeout(Some(remaining))?;
                    match ws.read()? {
                        Message::Text(text) => text.to_string(),
                        Message::Close(_) => return Err(anyhow!("{method}: server closed connection")),
                        _ => continue,
                    }
                }
                Transport::Private { incoming, .. } => incoming.recv_timeout(remaining).context("Codex response timeout or closed connection")?,
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(e) = msg.get("error") { return Err(anyhow!("{method}: {e}")); }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notifications are not needed for metadata queries; do not retain an unbounded stream.
        }
    }

    /// Thread ids the server has loaded (the only ones a message can reach through it).
    pub fn loaded_ids(&mut self) -> Result<HashSet<String>> {
        let r = self.call("thread/loaded/list", json!({}), Duration::from_secs(10))?;
        let mut out = HashSet::new();
        collect_ids(&r, &mut out);
        Ok(out)
    }

    pub fn list_threads(&mut self, limit: u32) -> Result<Vec<Value>> {
        let r = self.call("thread/list", json!({ "limit": limit }), Duration::from_secs(20))?;
        Ok(r.get("data").or_else(|| r.get("threads")).and_then(|v| v.as_array()).cloned().unwrap_or_default())
    }

    pub fn thread(&mut self, id: &str) -> Result<Value> {
        let r = self.call("thread/read", json!({ "threadId": id, "includeTurns": false }), Duration::from_secs(10))?;
        Ok(r.get("thread").cloned().unwrap_or(r))
    }
}

fn collect_ids(v: &Value, out: &mut HashSet<String>) {
    match v {
        Value::String(s) if looks_like_thread_id(s) => {
            out.insert(s.to_lowercase());
        }
        Value::Array(a) => a.iter().for_each(|x| collect_ids(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                if (k == "id" || k == "threadId") && x.is_string() {
                    if let Some(s) = x.as_str() {
                        if looks_like_thread_id(s) {
                            out.insert(s.to_lowercase());
                        }
                    }
                } else {
                    collect_ids(x, out);
                }
            }
        }
        _ => {}
    }
}

fn looks_like_thread_id(s: &str) -> bool {
    crate::model::is_uuid(s)
}

/// What this machine's Codex can do right now, remembered for a few seconds.
#[derive(Debug, Clone)]
pub struct State {
    pub cli: bool,
    pub daemon: bool,
    pub loaded: HashSet<String>,
    pub at: i64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
const STATE_TTL_MS: i64 = 5000;

pub fn state() -> State {
    if let Some(s) = STATE.lock().unwrap().as_ref() {
        if now_ms() - s.at < STATE_TTL_MS {
            return s.clone();
        }
    }
    let s = state_now();
    *STATE.lock().unwrap() = Some(s.clone());
    s
}

/// Whether the codex command runs here. Asked by starting it, which costs a tenth of a second of
/// processor each time, so a yes is kept for ten minutes (a no is asked again next time: the
/// user may be installing it).
static CLI_OK: Mutex<Option<Instant>> = Mutex::new(None);

fn cli_runs() -> bool {
    if CLI_OK.lock().ok().and_then(|t| *t).map(|at| at.elapsed() < Duration::from_secs(600)).unwrap_or(false) {
        return true;
    }
    let ok = command().arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if let Ok(mut t) = CLI_OK.lock() {
        *t = ok.then(Instant::now);
    }
    ok
}

pub fn state_now() -> State {
    let cli = cli_runs();
    let mut st = State { cli, daemon: false, loaded: HashSet::new(), at: now_ms() };
    if !cli || !daemon_socket().exists() {
        return st;
    }
    if let Ok(mut srv) = Server::shared() {
        st.daemon = true;
        st.loaded = srv.loaded_ids().unwrap_or_default();
    }
    st
}

/// Queue a message on a thread with the official command. `resume` adds `--resume`, which asks
/// the daemon to load the thread from history first: only when the caller explicitly allows it,
/// since it may start a second instance of a thread that is open elsewhere.
#[derive(Debug)]
pub struct QueueOutcome {
    pub detail: String,
    pub resumed: bool,
}

fn queue_outcome(stdout: &str, stderr: &str, resume_requested: bool, loaded: bool) -> QueueOutcome {
    QueueOutcome {
        detail: [stdout.trim(), stderr.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"),
        // Exit zero confirms enqueueing only. Loading requires independent server evidence.
        resumed: resume_requested && loaded && stderr.trim().is_empty(),
    }
}

pub fn queue(thread_id: &str, text: &str, resume: bool) -> Result<QueueOutcome> {
    let mut cmd = command();
    cmd.args(["queue", "--thread", thread_id, "--message", text]);
    if resume { cmd.arg("--resume"); }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("codex not found on PATH")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    fn read_output(stream: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || { let mut bytes = vec![]; let _ = stream.take(1024 * 1024).read_to_end(&mut bytes); bytes })
    }
    let stdout = read_output(stdout);
    let stderr = read_output(stderr);
    let deadline = Instant::now() + Duration::from_secs(30);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break Some(exit),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = child.kill(); let _ = child.wait(); break None; }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
    let Some(exit) = exit else { return Err(anyhow!("codex queue timed out; delivery is unknown, inspect before retrying")); };
    if !exit.success() { return Err(anyhow!("codex queue failed ({exit}): {stdout}\n{stderr}")); }
    let loaded = resume && Server::shared().and_then(|mut server| server.loaded_ids()).map(|ids| ids.contains(thread_id)).unwrap_or(false);
    Ok(queue_outcome(&stdout, &stderr, resume, loaded))
}

/// Put a message in a thread's queue with Codex's own queue (`thread/queue/add`), on the
/// shared daemon when there is one, else a private app-server. Whichever Codex process has the
/// thread open (the desktop app, a terminal, the daemon) picks it up when the thread is idle,
/// about every ten seconds; a thread nobody has open waits until it is opened. Nothing is
/// loaded or resumed here. Returns the queued submission's id.
pub fn queue_add(thread_id: &str, text: &str, client_id: &str) -> Result<String> {
    let mut srv = Server::connect()?;
    queue_add_on(&mut srv, thread_id, text, client_id)
}

fn queue_add_on(srv: &mut Server, thread_id: &str, text: &str, client_id: &str) -> Result<String> {
    let r = srv.call(
        "thread/queue/add",
        json!({ "threadId": thread_id, "clientUserMessageId": client_id, "input": [{ "type": "text", "text": text, "text_elements": [] }] }),
        Duration::from_secs(20),
    )?;
    let q = &r["queuedSubmission"];
    if q["clientUserMessageId"].as_str() != Some(client_id) {
        return Err(anyhow!("Codex answered, but the queue receipt does not match this message; check the thread before retrying"));
    }
    q["id"].as_str().map(str::to_string).ok_or_else(|| anyhow!("Codex answered without a queue id"))
}

/// Where each thread's rollout file is. A file found stays where it is; one not found is looked
/// for again after half a minute. Walking the whole sessions folder for every copy on every
/// refresh was a large part of the app's idle work.
type Rollouts = std::collections::HashMap<String, (Instant, Option<PathBuf>)>;
static ROLLOUTS: Mutex<Option<Rollouts>> = Mutex::new(None);

/// The rollout file of a thread, by its id.
pub fn rollout_path(thread_id: &str) -> Option<PathBuf> {
    let want = thread_id.to_lowercase();
    if let Some((at, found)) = ROLLOUTS.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&want).cloned())) {
        match found {
            Some(p) if p.is_file() => return Some(p),
            None if at.elapsed() < Duration::from_secs(30) => return None,
            _ => {}
        }
    }
    let found = find_rollout(&want);
    if let Ok(mut m) = ROLLOUTS.lock() {
        m.get_or_insert_with(Default::default).insert(want, (Instant::now(), found.clone()));
    }
    found
}

fn find_rollout(want: &str) -> Option<PathBuf> {
    let root = home_dir().join(".codex").join("sessions");
    for e in walkdir::WalkDir::new(root).max_depth(5).into_iter().flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if e.file_type().is_file() && name.ends_with(".jsonl") && name.contains(want) {
            return Some(e.path().to_path_buf());
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    NotFound,
    Queued,
    Started,
    Completed,
}

/// What a rollout line says that following a message needs: a turn starting or ending, or
/// something the user (or Dispatch, as the user) said.
enum Event {
    Started(Option<String>),
    Ended { turn: Option<String>, complete: bool },
    User { turn: Option<String>, text: String },
}

fn events_of(text: &str) -> Vec<Event> {
    let mut out = vec![];
    for line in text.lines() {
        // most lines are tool calls and their output: only these can matter, so only these are parsed
        if !(line.contains("task_started") || line.contains("task_complete") || line.contains("turn_aborted") || line.contains("user_message") || line.contains("\"user\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        let turn = p["turn_id"].as_str().map(str::to_owned);
        if v["type"] == "event_msg" {
            match p["type"].as_str() {
                Some("task_started") => out.push(Event::Started(turn.clone())),
                Some(kind @ ("task_complete" | "turn_aborted")) => out.push(Event::Ended { turn: turn.clone(), complete: kind == "task_complete" }),
                _ => {}
            }
        }
        let user_text = if v["type"] == "event_msg" && p["type"] == "user_message" {
            p["message"].as_str().unwrap_or("").to_string()
        } else if v["type"] == "response_item" && p["type"] == "message" && p["role"] == "user" {
            p["content"].as_array().map(|parts| parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
        } else {
            String::new()
        };
        if !user_text.is_empty() {
            out.push(Event::User { turn, text: user_text });
        }
    }
    out
}

/// How much of the end of a rollout file is read on every change.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// The events in the last 4 MiB of each rollout file, read once per version of the file.
type Tails = std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, std::sync::Arc<Vec<Event>>)>;
static TAILS: Mutex<Option<Tails>> = Mutex::new(None);

fn tail_events(path: &Path) -> Option<std::sync::Arc<Vec<Event>>> {
    let md = std::fs::metadata(path).ok()?;
    let (len, modified) = (md.len(), md.modified().ok());
    if let Some((l, m, ev)) = TAILS.lock().ok().and_then(|t| t.as_ref().and_then(|t| t.get(path).cloned())) {
        if l == len && m == modified {
            return Some(ev);
        }
    }
    let ev = std::sync::Arc::new(events_of(&crate::claude::read_tail(path, TAIL_BYTES).ok()?));
    if let Ok(mut t) = TAILS.lock() {
        let t = t.get_or_insert_with(Default::default);
        if t.len() >= 64 {
            t.clear();
        }
        t.insert(path.to_path_buf(), (len, modified, ev.clone()));
    }
    Some(ev)
}

/// Correlate the user item with its active turn; task_started normally precedes it.
pub fn receipt(thread_id: &str, marker: &str) -> Receipt {
    let Some(path) = rollout_path(thread_id) else { return Receipt::NotFound };
    receipt_at(&path, marker)
}

/// The tail answers for a recent message. One the tail does not hold may simply be older than
/// the tail: a busy thread writes megabytes after it, and "not in the last 4 MiB" is not "never
/// taken". So the part before the tail is searched as well, once.
fn receipt_at(path: &Path, marker: &str) -> Receipt {
    match tail_events(path).map(|ev| receipt_in(&ev, marker)) {
        Some(Receipt::NotFound) | None => receipt_before_tail(path, marker),
        Some(r) => r,
    }
}

/// What is known about one message in the part of a rollout before its tail: how far that part
/// has been searched (the file only grows, so it is never searched twice), the line the message
/// is on once found, and the answer for the file as it was then.
#[derive(Default, Clone)]
struct Older {
    searched_to: u64,
    found_at: Option<u64>,
    answer: Option<(u64, Receipt)>,
}
static OLDER: Mutex<Option<std::collections::HashMap<(PathBuf, String), Older>>> = Mutex::new(None);

fn receipt_before_tail(path: &Path, marker: &str) -> Receipt {
    use std::io::{BufRead, Seek, SeekFrom};
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else { return Receipt::NotFound };
    if len <= TAIL_BYTES {
        return Receipt::NotFound;
    }
    // the tail drops the line it starts in, so every line starting at or before this is not in it
    let head_end = len - TAIL_BYTES;
    let key = (path.to_path_buf(), marker.to_string());
    let mut known = OLDER.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(&key).cloned())).unwrap_or_default();
    if known.searched_to > len {
        known = Older::default();
    }
    if let Some((at, r)) = &known.answer {
        if *at == len {
            return r.clone();
        }
    }
    if known.found_at.is_none() {
        let Ok(mut f) = std::fs::File::open(path) else { return Receipt::NotFound };
        if f.seek(SeekFrom::Start(known.searched_to)).is_err() {
            return Receipt::NotFound;
        }
        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, f);
        let mut line = Vec::new();
        let mut pos = known.searched_to;
        while pos <= head_end {
            line.clear();
            let n = match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => n as u64,
            };
            let text = String::from_utf8_lossy(&line);
            if text.contains(marker) && events_of(&text).iter().any(|e| matches!(e, Event::User { text, .. } if text.contains(marker))) {
                known.found_at = Some(pos);
                break;
            }
            pos += n;
            known.searched_to = pos;
        }
    }
    let answer = match known.found_at {
        None => Receipt::NotFound,
        // from a little before its line (the turn it belongs to starts just above) to the end
        Some(at) => match read_from(path, at.saturating_sub(1024 * 1024)) {
            Some(text) => match receipt_in(&events_of(&text), marker) {
                // the line is there, so the thread took it, whatever the turns around it say
                Receipt::NotFound => Receipt::Queued,
                r => r,
            },
            // more than is worth reading after it: that turn is long over
            None => Receipt::Completed,
        },
    };
    known.answer = Some((len, answer.clone()));
    if let Ok(mut m) = OLDER.lock() {
        let m = m.get_or_insert_with(Default::default);
        if m.len() >= 256 {
            m.clear();
        }
        m.insert(key, known);
    }
    answer
}

/// The file from the first whole line after `start` to its end; None when that is over 64 MiB.
fn read_from(path: &Path, start: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len.saturating_sub(start) > 64 * 1024 * 1024 {
        return None;
    }
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start.min(len)) as usize);
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    Some(if start > 0 { text.find('\n').map(|i| text[i + 1..].to_string()).unwrap_or_default() } else { text.into_owned() })
}

#[cfg(test)]
fn receipt_text(text: &str, marker: &str) -> Receipt {
    receipt_in(&events_of(text), marker)
}

fn receipt_in(events: &[Event], marker: &str) -> Receipt {
    let mut active_turn: Option<&str> = None;
    let mut message_turn: Option<&str> = None;
    let mut result = Receipt::NotFound;
    for e in events {
        match e {
            Event::Started(turn) => active_turn = turn.as_deref(),
            Event::Ended { turn, complete } => {
                if *complete && turn.is_some() && turn.as_deref() == message_turn {
                    return Receipt::Completed;
                }
                if turn.as_deref() == active_turn {
                    active_turn = None;
                }
            }
            Event::User { turn, text } => {
                if text.contains(marker) {
                    // Codex writes task_started before the user item, including steered items.
                    message_turn = turn.as_deref().or(active_turn);
                    result = if message_turn.is_some() { Receipt::Started } else { Receipt::Queued };
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn initialize_enables_experimental_queue_api() {
        let (client, socket) = UnixStream::pair().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let worker = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(socket).unwrap();
            let init: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(init["method"], "initialize");
            let enabled = init["params"]["capabilities"]["experimentalApi"] == true;
            let response = if enabled { json!({"id":init["id"],"result":{}}) }
                else { json!({"id":init["id"],"error":{"code":-32600,"message":"experimentalApi required"}}) };
            ws.send(Message::Text(response.to_string().into())).unwrap();
            if enabled {
                let ready: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(ready["method"], "initialized");
                let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "thread/queue/add");
                ws.send(Message::Text(json!({"id":request["id"],"result":{"queuedSubmission":{"id":"queue-1","clientUserMessageId":"letter-1"}}}).to_string().into())).unwrap();
            }
        });
        let (ws, _) = tungstenite::client("ws://localhost/", client).unwrap();
        let result = Server::initialize(Transport::Shared(ws), true)
            .and_then(|mut server| queue_add_on(&mut server, "thread-1", "test", "letter-1"));
        worker.join().unwrap();
        assert_eq!(result.unwrap(), "queue-1");
    }
    #[cfg(unix)]
    #[test]
    fn websocket_handshake_and_call_deadline() {
        let (client, server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(server).unwrap();
            let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            ws.send(Message::Text(json!({"id":request["id"],"result":{"data":[]}}).to_string().into())).unwrap();
            let _ = ws.read();
            std::thread::sleep(Duration::from_millis(150));
        });
        let (ws, _) = tungstenite::client("ws://localhost/", client).unwrap();
        let mut server = Server { transport: Transport::Shared(ws), next_id: 1, shared: true };
        assert!(server.loaded_ids().unwrap().is_empty());
        let start = Instant::now();
        assert!(server.call("stall", json!({}), Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_millis(500));
        worker.join().unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn queue_add_sends_the_message_and_checks_the_receipt() {
        for (answer_id, ok) in [("letter-1", true), ("someone-else", false)] {
            let (client, server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                let mut ws = tungstenite::accept(server).unwrap();
                let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "thread/queue/add");
                assert_eq!(request["params"]["threadId"], "t-1");
                assert_eq!(request["params"]["input"][0]["text"], "hello");
                ws.send(Message::Text(json!({"id":request["id"],"result":{"queuedSubmission":{"id":"q-9","clientUserMessageId":answer_id}}}).to_string().into())).unwrap();
                let _ = ws.read();
            });
            let (ws, _) = tungstenite::client("ws://localhost/", client).unwrap();
            let mut srv = Server { transport: Transport::Shared(ws), next_id: 1, shared: true };
            let got = queue_add_on(&mut srv, "t-1", "hello", "letter-1");
            assert_eq!(got.is_ok(), ok, "{got:?}");
            if ok {
                assert_eq!(got.unwrap(), "q-9");
            }
            drop(srv);
            worker.join().unwrap();
        }
    }

    #[test]
    fn queue_warning_is_not_a_successful_resume() {
        let out = queue_outcome("Queued fixture message", "Warning: failed to resume the thread", true, false);
        assert!(!out.resumed);
        assert!(out.detail.contains("failed to resume"));
        assert!(out.detail.contains("Queued fixture"));
        assert!(!queue_outcome("Queued", "", true, false).resumed);
        assert!(queue_outcome("Queued", "", true, true).resumed);
    }
    fn event(kind: &str, turn: &str) -> Value {
        json!({"type":"event_msg","payload":{"type":kind,"turn_id":turn}})
    }
    fn user(marker: &str) -> Value {
        json!({"type":"event_msg","payload":{"type":"user_message","message":marker}})
    }
    fn lines(events: &[Value]) -> String {
        events.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")
    }
    #[test]
    fn receipt_start_precedes_message() {
        assert_eq!(receipt_text(&lines(&[event("task_started","a"), user("marker")]), "marker"), Receipt::Started);
    }
    #[test]
    fn receipt_completion_must_match_turn() {
        let mut events = vec![event("task_started","a"),user("marker"),event("task_complete","b")];
        assert_eq!(receipt_text(&lines(&events),"marker"),Receipt::Started);
        events.push(event("task_complete","a"));
        assert_eq!(receipt_text(&lines(&events),"marker"),Receipt::Completed);
    }
    #[test]
    fn queued_message_does_not_borrow_later_turn() {
        assert_eq!(receipt_text(&lines(&[user("marker"),event("task_started","b"),event("task_complete","b")]),"marker"),Receipt::Queued);
    }
    /// A busy thread writes megabytes after a message. The message is then older than the tail
    /// that is read on every change, and it must still be found: taken, and its turn finished.
    #[test]
    fn a_message_older_than_the_tail_is_still_found() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("dispatch-older-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        let filler = json!({"type":"response_item","payload":{"type":"function_call_output","output":"x".repeat(64 * 1024)}}).to_string();
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{}", lines(&[event("task_started", "a"), user("first-marker"), event("task_complete", "a"), event("task_started", "b"), user("second-marker")])).unwrap();
        for _ in 0..80 {
            writeln!(f, "{filler}").unwrap();
        }
        f.flush().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > TAIL_BYTES + 1024 * 1024);
        assert_eq!(receipt_in(&tail_events(&path).unwrap(), "first-marker"), Receipt::NotFound, "the tail alone no longer holds it");
        assert_eq!(receipt_at(&path, "first-marker"), Receipt::Completed);
        assert_eq!(receipt_at(&path, "second-marker"), Receipt::Started, "taken, its turn still running");
        assert_eq!(receipt_at(&path, "never-sent"), Receipt::NotFound, "one that was never taken is still not found");
        // the file grows: the turn ends, and a message arrives that the tail holds
        writeln!(f, "{}", lines(&[event("task_complete", "b"), event("task_started", "c"), user("third-marker")])).unwrap();
        f.flush().unwrap();
        assert_eq!(receipt_at(&path, "second-marker"), Receipt::Completed);
        assert_eq!(receipt_at(&path, "third-marker"), Receipt::Started);
        assert_eq!(receipt_at(&path, "never-sent"), Receipt::NotFound);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn receipt_ignores_assistant_and_event_text() {
        let fake=json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"marker user_message"}]}});
        assert_eq!(receipt_text(&lines(&[fake,event("task_started","a"),event("task_complete","a")]),"marker"),Receipt::NotFound);
    }
}

/// What a Codex thread's own log says about how it runs: the approval policy of its latest
/// turn and which front end started it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Traits {
    /// `never`, `on-request`, `untrusted`, `on-failure`.
    pub approval: Option<String>,
    /// `desktop`, `terminal`, `exec`, or the originator as Codex wrote it.
    pub client: Option<String>,
}

/// Traits by rollout file, valid while its mtime and size hold.
static TRAITS: Mutex<Option<std::collections::HashMap<PathBuf, (i64, u64, Traits)>>> = Mutex::new(None);

/// How far back the approval policy is looked for: every turn writes one, so the last turn's
/// is near the end.
const TRAITS_TAIL: u64 = 512 * 1024;

pub fn traits(path: &Path) -> Traits {
    let Ok(md) = std::fs::metadata(path) else { return Traits::default() };
    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    let size = md.len();
    if let Some((m, s, t)) = TRAITS.lock().unwrap().as_ref().and_then(|c| c.get(path)) {
        if *m == mtime && *s == size {
            return t.clone();
        }
    }
    let t = read_traits(path, size);
    TRAITS.lock().unwrap().get_or_insert_with(Default::default).insert(path.to_path_buf(), (mtime, size, t.clone()));
    t
}

fn read_traits(path: &Path, size: u64) -> Traits {
    use std::io::{Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Traits::default() };
    let mut head = vec![0u8; 64 * 1024];
    let n = f.read(&mut head).unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]).to_string();
    let start = size.saturating_sub(TRAITS_TAIL);
    let mut tail = String::new();
    if f.seek(SeekFrom::Start(start)).is_ok() {
        let mut buf = vec![];
        let _ = f.take(TRAITS_TAIL).read_to_end(&mut buf);
        tail = String::from_utf8_lossy(&buf).to_string();
    }
    parse_traits(&head, &tail)
}

fn parse_traits(head: &str, tail: &str) -> Traits {
    let last = |text: &str, key: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r#""{key}":"([^"]+)""#)).ok()?;
        re.captures_iter(text).last().map(|c| c[1].to_string())
    };
    // the policy can change between turns: the tail's last one, else the head's
    let approval = last(tail, "approval_policy").or_else(|| last(head, "approval_policy"));
    let client = last(head, "originator").map(|o| {
        let l = o.to_lowercase();
        if l.contains("desktop") || l.contains("app") {
            "desktop".to_string()
        } else if l.contains("exec") {
            "exec".to_string()
        } else if l.contains("cli") || l.contains("tui") {
            "terminal".to_string()
        } else {
            o
        }
    });
    Traits { approval, client }
}

#[cfg(test)]
mod traits_tests {
    use super::*;

    #[test]
    fn the_latest_turn_says_the_policy() {
        let head = r#"{"type":"session_meta","payload":{"originator":"codex_cli_rs"}}
{"type":"turn_context","payload":{"approval_policy":"on-request"}}"#;
        let tail = r#"{"type":"turn_context","payload":{"approval_policy":"on-request"}}
{"type":"turn_context","payload":{"approval_policy":"never"}}"#;
        assert_eq!(parse_traits(head, tail), Traits { approval: Some("never".into()), client: Some("terminal".into()) });
        assert_eq!(parse_traits(r#"{"originator":"Codex Desktop"}"#, "").client.as_deref(), Some("desktop"));
        assert_eq!(parse_traits(r#"{"originator":"codex_exec","approval_policy":"untrusted"}"#, "").approval.as_deref(), Some("untrusted"));
        assert_eq!(parse_traits("", ""), Traits::default());
    }
}
