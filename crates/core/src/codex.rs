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
    let exe = executable();
    let mut cmd = Command::new(exe.clone().unwrap_or_else(|| PathBuf::from("codex")));
    if let Some(path) = tool_path(exe.as_deref()) {
        cmd.env("PATH", path);
    }
    cmd
}

/// The folders a version manager keeps its current `node` in, newest version first.
fn node_dirs(home: &Path) -> Vec<PathBuf> {
    fn version(p: &Path) -> Vec<u64> {
        p.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_start_matches('v').split('.').filter_map(|n| n.parse().ok()).collect()
    }
    let mut dirs = vec![];
    for (root, bin) in [(home.join(".nvm/versions/node"), "bin"), (home.join(".local/share/fnm/node-versions"), "installation/bin"), (home.join("Library/Application Support/fnm/node-versions"), "installation/bin")] {
        let mut versions: Vec<PathBuf> = std::fs::read_dir(&root).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        versions.sort_by_key(|p| std::cmp::Reverse(version(p)));
        dirs.extend(versions.into_iter().map(|v| v.join(bin)));
    }
    dirs
}

/// `PATH` for a program we start. The app is opened by Finder, the Dock or its own updater with
/// the system's short PATH, and a command installed by npm is a script that asks `env` for
/// `node`: found, it still cannot start. So the folder the program is in comes first (npm puts
/// its commands beside `node`), then the PATH we have, then where Node usually lives. `None`
/// when that adds nothing.
pub(crate) fn tool_path(program: Option<&Path>) -> Option<std::ffi::OsString> {
    tool_path_in(program, &home_dir(), std::env::var_os("PATH"))
}

fn tool_path_in(program: Option<&Path>, home: &Path, current: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    let mut dirs: Vec<PathBuf> = vec![];
    if let Some(p) = program {
        dirs.extend(p.parent().map(Path::to_path_buf));
        // `bin/gbot` is a link into `lib/node_modules/<package>/…`: `node` is in that prefix's bin
        if let Ok(real) = std::fs::canonicalize(p) {
            dirs.extend(real.parent().map(Path::to_path_buf));
            dirs.extend(real.ancestors().find(|a| a.file_name().map(|n| n == "node_modules").unwrap_or(false)).and_then(|m| m.parent()).and_then(|lib| lib.parent()).map(|prefix| prefix.join("bin")));
        }
    }
    let had: Vec<PathBuf> = current.as_ref().map(|c| std::env::split_paths(c).collect()).unwrap_or_default();
    dirs.extend(had.iter().cloned());
    dirs.extend([".local/bin", ".local/node/bin", ".npm-global/bin", ".volta/bin", ".asdf/shims", ".local/share/mise/shims"].map(|d| home.join(d)));
    dirs.extend(node_dirs(home));
    if cfg!(windows) {
        dirs.extend(std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("npm")));
        dirs.extend(std::env::var_os("ProgramFiles").map(|d| PathBuf::from(d).join("nodejs")));
    } else {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    let mut kept: Vec<PathBuf> = vec![];
    for d in dirs {
        if !d.as_os_str().is_empty() && !kept.contains(&d) && (had.contains(&d) || d.is_dir()) {
            kept.push(d);
        }
    }
    if kept == had {
        return None;
    }
    std::env::join_paths(kept).ok()
}

/// Whether `name` can be started with this `PATH`.
#[cfg(test)]
pub(crate) fn on_path(name: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|d| command_names(name).iter().any(|n| is_executable(&d.join(n))))
}

pub fn daemon_socket() -> PathBuf {
    home_dir().join(".codex").join("app-server-control").join("app-server-control.sock")
}

/// A JSON-RPC session with an app-server: `shared` when it is the daemon every Codex client
/// on this machine can share, false when it is a private server we spawned (reads only).
enum Transport {
    #[cfg(unix)]
    Shared(WebSocket<UnixStream>),
    Private { child: Child, stdin: Option<ChildStdin>, incoming: mpsc::Receiver<String> },
}

pub struct Server {
    transport: Transport,
    next_id: u64,
    pub shared: bool,
    /// What the server said of its own while we waited for an answer: requests it needs
    /// answered and turn and status news. Kept (a few) for whoever follows the thread.
    backlog: std::collections::VecDeque<Value>,
}

impl Drop for Server {
    #[allow(irrefutable_let_patterns)]
    fn drop(&mut self) {
        if let Transport::Private { child, stdin, .. } = &mut self.transport {
            // closing its input lets the server finish writing and leave; killed if it does not
            drop(stdin.take());
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Server {
    fn initialize(transport: Transport, shared: bool) -> Result<Server> {
        let mut s = Server { transport, next_id: 1, shared, backlog: Default::default() };
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
        Self::private()
    }

    /// An app-server of our own over stdio, gone when this is dropped.
    fn private() -> Result<Server> {
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
        Self::initialize(Transport::Private { child, stdin: Some(stdin), incoming }, false)
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
                let stdin = stdin.as_mut().context("the Codex server's input is closed")?;
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
            let Some(text) = self.read_text(remaining).with_context(|| format!("{method}: no answer"))? else { continue };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
            if msg.get("method").is_none() && msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(e) = msg.get("error") { return Err(anyhow!("{method}: {e}")); }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Most notifications do not matter to a question; what someone following the thread
            // needs is kept, a few of them.
            if worth_keeping(&msg) && self.backlog.len() < 64 {
                self.backlog.push_back(msg);
            }
        }
    }

    /// One message from the server; None when nothing came within `timeout` (or a frame that is
    /// not a message did). Errors only when the server is gone.
    fn read_text(&mut self, timeout: Duration) -> Result<Option<String>> {
        Ok(match &mut self.transport {
            #[cfg(unix)]
            Transport::Shared(ws) => {
                ws.get_ref().set_read_timeout(Some(timeout))?;
                match ws.read() {
                    Ok(Message::Text(text)) => Some(text.to_string()),
                    Ok(Message::Close(_)) => return Err(anyhow!("the Codex server closed the connection")),
                    Ok(_) => None,
                    Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => None,
                    Err(e) => return Err(e.into()),
                }
            }
            Transport::Private { incoming, .. } => match incoming.recv_timeout(timeout) {
                Ok(text) => Some(text),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(anyhow!("the Codex server is gone")),
            },
        })
    }

    /// The next thing the server says of its own (a notification, or a request it needs
    /// answered), or None when it says nothing for `timeout`. Errors when it is gone.
    fn next(&mut self, timeout: Duration) -> Result<Option<Value>> {
        if let Some(m) = self.backlog.pop_front() {
            return Ok(Some(m));
        }
        let text = self.read_text(timeout)?;
        Ok(text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|m| m.get("method").is_some()))
    }

    /// Answer a request the server made.
    fn respond(&mut self, id: Value, result: Value) -> Result<()> {
        self.write(json!({ "id": id, "result": result }), Duration::from_secs(5))
    }

    fn respond_error(&mut self, id: Value, message: &str) -> Result<()> {
        self.write(json!({ "id": id, "error": { "code": -32000, "message": message } }), Duration::from_secs(5))
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

/// What a server says while we wait on something else that someone following a thread needs:
/// its requests (they wait for an answer) and turn and status news.
fn worth_keeping(msg: &Value) -> bool {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    msg.get("id").is_some() || method.starts_with("turn/") || method == "thread/status/changed"
}

/// Codex's home: `$CODEX_HOME` when set, else `~/.codex`.
fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home_dir().join(".codex"))
}

/// Whether some Codex process (the app, a terminal, the daemon) has this thread open now.
/// Every Codex process that loads a thread holds a lock on `thread-writer-locks/<id>.lock`
/// for as long as it has it, so that no two of them write one thread. The probe takes Codex's
/// coordination lock first, as Codex does before it publishes a thread: a Codex opening the
/// thread at that moment waits a few milliseconds instead of failing. None: cannot tell.
pub fn thread_open(thread_id: &str) -> Option<bool> {
    thread_open_in(&codex_home(), thread_id)
}

#[cfg(unix)]
fn thread_open_in(home: &Path, thread_id: &str) -> Option<bool> {
    use std::fs::{OpenOptions, TryLockError};
    if !looks_like_thread_id(thread_id) {
        return None;
    }
    let dir = home.join("thread-writer-locks");
    // never create anything in Codex's home: no coordination file means no Codex locked a thread here
    let coordination = match OpenOptions::new().read(true).write(true).open(dir.join(".coordination.lock")) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match coordination.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let open = match OpenOptions::new().read(true).write(true).open(dir.join(format!("{}.lock", thread_id.to_lowercase()))) {
        Ok(f) => match f.try_lock() {
            Ok(()) => Some(false),
            Err(TryLockError::WouldBlock) => Some(true),
            Err(_) => None,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    };
    // the thread's lock goes with its file handle, then coordination
    drop(coordination);
    open
}

#[cfg(not(unix))]
fn thread_open_in(_home: &Path, _thread_id: &str) -> Option<bool> {
    None
}

/// What came of running a thread nobody had open.
#[derive(Debug, Default)]
pub struct RunOutcome {
    /// Turns that started while we had the thread.
    pub turns: u32,
    /// Requests for approval or input declined, nobody being there to answer them.
    pub declined: u32,
    /// Still running when the time was up, or when the user took the thread back: stopped.
    pub interrupted: bool,
    /// The user took the thread back while we had it: we left at once.
    pub handed_back: bool,
    /// Queued messages we had to tell Codex to start: the queue did not go by itself.
    pub started: u32,
    /// A queued message was told to start and still no turn began: we left the thread free
    /// rather than hold it over a queue that does not move.
    pub stalled: bool,
}

/// What a background run does while no turn is running.
#[derive(Debug, PartialEq, Eq)]
enum Idle {
    Wait,
    /// Tell Codex to start this queued message.
    Start(String),
    Leave,
}

/// How long a resumed thread is given to start its queue by itself.
const QUEUE_GRACE: Duration = Duration::from_secs(5);
/// How long a queued message told to start is given to begin its turn.
const START_GRACE: Duration = Duration::from_secs(60);
/// How long a thread with an empty queue is watched before we leave it.
const EMPTY_GRACE: Duration = Duration::from_secs(20);

/// While no turn is running: `queued` is the thread's queue in order, `quiet` how long nothing
/// has been running, `told` the queued messages already told to start and `since_told` how
/// long ago the last one was.
///
/// A resumed thread starts its queue by itself, at once. Not after a turn was interrupted:
/// what is left in the queue then stays put, and resuming the thread again does not move it
/// (seen with Codex 0.151 and 0.171). So a message still queued after a few quiet seconds is
/// told to start, once; and if that starts nothing either, we leave instead of holding the
/// thread's lock over it for an hour.
fn when_idle(queued: &[String], quiet: Duration, told: &HashSet<String>, since_told: Option<Duration>) -> Idle {
    let Some(first) = queued.first() else {
        return if quiet >= EMPTY_GRACE { Idle::Leave } else { Idle::Wait };
    };
    if quiet < QUEUE_GRACE {
        return Idle::Wait;
    }
    if !told.contains(first) {
        return Idle::Start(first.clone());
    }
    if since_told.map(|d| d >= START_GRACE).unwrap_or(true) { Idle::Leave } else { Idle::Wait }
}

/// Why something was declined, as the agent reads it.
const NOBODY_THERE: &str = "Dispatch is running this thread in the background because a message arrived while nobody had it open. Nobody is there to approve this. Carry on without it, and say in your answer what still needs the user's approval.";

/// The answer to a request the server makes of its client, when nobody is there: approvals are
/// declined (the agent carries on and says what it could not do), the time is told, anything
/// else is refused.
fn unattended_answer(method: &str) -> Option<Value> {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => Some(json!({ "decision": "decline" })),
        "execCommandApproval" | "applyPatchApproval" => Some(json!({ "decision": { "denied": { "rejection": NOBODY_THERE } } })),
        "mcpServer/elicitation/request" => Some(json!({ "action": "decline", "content": null })),
        "currentTime/read" => Some(json!({ "currentTimeAt": now_ms() / 1000 })),
        _ => None,
    }
}

/// Run a thread nobody has open, so what waits in its queue is taken now instead of when
/// someone next opens it. An app-server of our own resumes the thread, and resuming starts
/// what is queued, as opening the thread in the app does. The server stays until the thread
/// is idle with nothing left in its queue (or `max` is up: the turn is stopped), then leaves,
/// and the thread is free for the app again; what happened is in the thread's history.
///
/// Nobody is at the thread to approve anything: such a request is declined, and the agent
/// carries on and says what it could not do. Only for a thread `thread_open` says nobody has:
/// two processes writing one thread is what Codex's lock is there to stop.
pub fn run_unopened(thread_id: &str, max: Duration) -> Result<RunOutcome> {
    run_unopened_until(thread_id, max, &std::sync::atomic::AtomicBool::new(false))
}

/// `run_unopened`, until `stop` is set: the user wants the thread back. The turn under way is
/// interrupted and the server leaves at once, which frees the thread's lock, so the user can
/// type in it or archive it in Codex. What is still in the thread's queue stays there.
pub fn run_unopened_until(thread_id: &str, max: Duration, stop: &std::sync::atomic::AtomicBool) -> Result<RunOutcome> {
    use std::sync::atomic::Ordering;
    let mut out = RunOutcome::default();
    if stop.load(Ordering::Relaxed) {
        out.handed_back = true;
        return Ok(out);
    }
    let mut srv = Server::private()?;
    let deadline = Instant::now() + max;
    srv.call("thread/resume", json!({ "threadId": thread_id, "excludeTurns": true }), Duration::from_secs(60))?;
    let mut turn: Option<String> = None;
    // a resumed thread starts its queue at once; one whose queue someone else emptied meanwhile has nothing to do
    let mut quiet_since = Instant::now();
    let mut asked_queue = Instant::now();
    let mut told: HashSet<String> = HashSet::new();
    let mut told_at: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            if let Some(t) = turn.take() {
                let _ = srv.call("turn/interrupt", json!({ "threadId": thread_id, "turnId": t }), Duration::from_secs(10));
                out.interrupted = true;
            }
            out.handed_back = true;
            // leaving drops the server: it closes and the thread's lock goes with it
            return Ok(out);
        }
        if Instant::now() >= deadline {
            if let Some(t) = turn.take() {
                let _ = srv.call("turn/interrupt", json!({ "threadId": thread_id, "turnId": t }), Duration::from_secs(10));
                out.interrupted = true;
            }
            return Ok(out);
        }
        // a short wait, so that being told to stop is noticed within a second
        let Some(msg) = srv.next(Duration::from_secs(1))? else {
            if turn.is_none() && asked_queue.elapsed() >= QUEUE_GRACE {
                asked_queue = Instant::now();
                let queued = queue_ids(&mut srv, thread_id);
                match when_idle(&queued, quiet_since.elapsed(), &told, told_at.map(|t| t.elapsed())) {
                    Idle::Wait => {}
                    Idle::Start(id) => {
                        // the turn it starts is announced like any other (`turn/started`)
                        let _ = srv.call("thread/queue/start", json!({ "threadId": thread_id, "queuedSubmissionId": id }), Duration::from_secs(30));
                        told.insert(id);
                        told_at = Some(Instant::now());
                        out.started += 1;
                    }
                    Idle::Leave => {
                        out.stalled = !queued.is_empty();
                        return Ok(out);
                    }
                }
            }
            continue;
        };
        let method = msg["method"].as_str().unwrap_or("").to_string();
        if let Some(id) = msg.get("id").cloned() {
            match unattended_answer(&method) {
                Some(answer) => {
                    if method != "currentTime/read" {
                        out.declined += 1;
                    }
                    srv.respond(id, answer)?;
                }
                None => {
                    out.declined += 1;
                    srv.respond_error(id, NOBODY_THERE)?;
                }
            }
            continue;
        }
        let params = &msg["params"];
        if params["threadId"].as_str().map(|t| !t.eq_ignore_ascii_case(thread_id)).unwrap_or(false) {
            continue;
        }
        match method.as_str() {
            "turn/started" => {
                out.turns += 1;
                turn = params["turn"]["id"].as_str().map(str::to_string);
            }
            "turn/completed" => {
                turn = None;
                quiet_since = Instant::now();
                // the next queued message starts by itself; leave when there is none
                std::thread::sleep(Duration::from_millis(500));
                // (a turn may have started from the queue while we asked: it is in the backlog then)
                if queue_len(&mut srv, thread_id) == 0 && !srv.backlog.iter().any(|m| m["method"] == "turn/started") {
                    return Ok(out);
                }
            }
            _ => {}
        }
    }
}

/// The messages waiting in the thread's queue, by their queue ids, in order (none when it
/// cannot be read).
fn queue_ids(srv: &mut Server, thread_id: &str) -> Vec<String> {
    srv.call("thread/queue/list", json!({ "threadId": thread_id }), Duration::from_secs(10))
        .ok()
        .and_then(|r| r["data"].as_array().map(|a| a.iter().filter_map(|q| q["id"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

/// How many messages wait in the thread's queue (0 when it cannot be read).
fn queue_len(srv: &mut Server, thread_id: &str) -> usize {
    queue_ids(srv, thread_id).len()
}

/// What this machine's Codex can do right now, remembered for a few seconds.
#[derive(Debug, Clone)]
pub struct State {
    pub cli: bool,
    pub daemon: bool,
    pub loaded: HashSet<String>,
    pub at: i64,
}

static STATE: crate::snapshot::Snapshot<State> = crate::snapshot::Snapshot::new();
const STATE_TTL: Duration = Duration::from_secs(5);

pub fn state() -> State {
    STATE.get(STATE_TTL, |_| state_now())
}

/// Peer listings never wait for the CLI or app-server. Sending still uses `state`.
pub(crate) fn state_snapshot() -> Option<State> {
    STATE.read(STATE_TTL, |_| state_now())
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
    fn a_node_script_is_given_a_path_with_its_node_on_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("dispatch-path-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        // npm's layout under a version manager: bin/node, bin/gbot -> ../lib/node_modules/…
        let prefix = home.join(".nvm/versions/node/v22.3.0");
        let script = prefix.join("lib/node_modules/grok-bot-cli/bin/gbot.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(home.join(".nvm/versions/node/v9.11.2/bin")).unwrap();
        for file in [&script, &prefix.join("bin/node")] {
            std::fs::write(file, "#!/usr/bin/env node\n").unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // the command the app found is a link somewhere else entirely
        let link = root.join("elsewhere/gbot");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&script, &link).unwrap();
        let finder = std::ffi::OsString::from("/usr/bin:/bin");
        assert!(!on_path("node", &finder) || is_executable(Path::new("/usr/bin/node")), "the test needs a PATH without node");

        let path = tool_path_in(Some(&link), &home, Some(finder.clone())).expect("a wider PATH");
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let real_bin = std::fs::canonicalize(prefix.join("bin")).unwrap();
        assert!(dirs.contains(&real_bin), "node's folder is on it: {dirs:?}");
        assert!(on_path("node", &path));
        // what we had stays, in order, after the program's own folders
        let usr = dirs.iter().position(|d| d == Path::new("/usr/bin")).unwrap();
        assert!(dirs.iter().position(|d| d == &real_bin).unwrap() < usr);
        assert_eq!(dirs[usr + 1], Path::new("/bin"));
        // with no program, the newest installed Node is found before an old one
        let path = tool_path_in(None, &home, Some(finder)).unwrap();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let new = dirs.iter().position(|d| d.ends_with("v22.3.0/bin")).unwrap();
        let old = dirs.iter().position(|d| d.ends_with("v9.11.2/bin")).unwrap();
        assert!(new < old, "v22 before v9: {dirs:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_thread_is_open_while_some_codex_holds_its_lock() {
        let home = std::env::temp_dir().join(format!("dispatch-codex-{}", uuid::Uuid::new_v4()));
        let id = "01a1f721-bdd7-7f63-8ac6-c71d709fa43c";
        // no Codex ever locked a thread here
        assert_eq!(thread_open_in(&home, id), Some(false));
        let dir = home.join("thread-writer-locks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".coordination.lock"), "").unwrap();
        assert_eq!(thread_open_in(&home, id), Some(false), "no lock file: nobody has it");
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(format!("{id}.lock"))).unwrap();
        lock.lock().unwrap();
        assert_eq!(thread_open_in(&home, id), Some(true), "a Codex holds it");
        assert_eq!(thread_open_in(&home, &id.to_uppercase()), Some(true));
        lock.unlock().unwrap();
        assert_eq!(thread_open_in(&home, id), Some(false), "left behind, not held: nobody has it");
        // a Codex in the middle of opening a thread holds coordination: the probe waits, then says it cannot tell
        let coordination = std::fs::OpenOptions::new().read(true).write(true).open(dir.join(".coordination.lock")).unwrap();
        coordination.lock().unwrap();
        assert_eq!(thread_open_in(&home, id), None);
        drop(coordination);
        assert_eq!(thread_open_in(&home, "not-a-thread"), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Against a real Codex: `CODEX_HOME` with a thread nobody has open and a message in its
    /// queue, the thread's id in `DISPATCH_PROBE_THREAD`.
    #[test]
    #[ignore]
    fn probe_run_unopened() {
        let id = std::env::var("DISPATCH_PROBE_THREAD").unwrap();
        assert_eq!(thread_open(&id), Some(false));
        let out = run_unopened(&id, Duration::from_secs(120)).unwrap();
        eprintln!("{out:?}");
        assert!(out.turns >= 1, "resuming started the queued message");
        assert_eq!(thread_open(&id), Some(false), "left the thread free");
        let mut srv = Server::private().unwrap();
        assert_eq!(queue_len(&mut srv, &id), 0);
    }

    #[test]
    fn a_queue_that_does_not_go_by_itself_is_told_to_start_once() {
        let secs = Duration::from_secs;
        let q = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let none = HashSet::new();
        // nothing queued: watched for a while, then left
        assert_eq!(when_idle(&[], secs(3), &none, None), Idle::Wait);
        assert_eq!(when_idle(&[], secs(20), &none, None), Idle::Leave);
        // just resumed: the queue normally starts by itself
        assert_eq!(when_idle(&q(&["a", "b"]), secs(2), &none, None), Idle::Wait);
        // it did not (the turn before was interrupted): the first one is told to start
        assert_eq!(when_idle(&q(&["a", "b"]), secs(5), &none, None), Idle::Start("a".into()));
        // told already: not told again, and given time to begin
        let told: HashSet<String> = ["a".to_string()].into();
        assert_eq!(when_idle(&q(&["a", "b"]), secs(30), &told, Some(secs(25))), Idle::Wait);
        // still nothing: leave the thread free rather than hold it
        assert_eq!(when_idle(&q(&["a", "b"]), secs(70), &told, Some(secs(60))), Idle::Leave);
        // its turn ran and it left the queue: the next one, if that does not go by itself either
        assert_eq!(when_idle(&q(&["b"]), secs(1), &told, Some(secs(40))), Idle::Wait);
        assert_eq!(when_idle(&q(&["b"]), secs(6), &told, Some(secs(45))), Idle::Start("b".into()));
    }

    /// Against a real Codex: a thread nobody has open whose queue was left behind by an
    /// interrupted turn (run `probe_hand_back` on it first). Resuming does not move such a
    /// queue; the run has to start it. The thread's id in `DISPATCH_PROBE_THREAD`.
    #[test]
    #[ignore]
    fn probe_queue_left_by_an_interrupt() {
        let id = std::env::var("DISPATCH_PROBE_THREAD").unwrap();
        assert_eq!(thread_open(&id), Some(false));
        let before = {
            let mut srv = Server::private().unwrap();
            queue_len(&mut srv, &id)
        };
        assert!(before >= 1, "a message is waiting in the queue");
        let began = Instant::now();
        let out = run_unopened(&id, Duration::from_secs(std::env::var("DISPATCH_PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(90))).unwrap();
        eprintln!("{out:?} after {:.1}s", began.elapsed().as_secs_f32());
        let mut srv = Server::private().unwrap();
        let after = queue_len(&mut srv, &id);
        eprintln!("queue: {before} -> {after}");
        assert!(out.turns >= 1, "a turn began for what was queued");
        assert!(after < before, "and it left the queue");
        assert_eq!(thread_open(&id), Some(false), "left the thread free");
    }

    /// Against a real Codex: the user takes back a thread the app is running. `CODEX_HOME` with
    /// a thread nobody has open and two messages in its queue, the first one long enough to
    /// still be running after a few seconds; the thread's id in `DISPATCH_PROBE_THREAD`.
    #[test]
    #[ignore]
    fn probe_hand_back() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let id = std::env::var("DISPATCH_PROBE_THREAD").unwrap();
        assert_eq!(thread_open(&id), Some(false));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (stop2, id2) = (stop.clone(), id.clone());
        let run = std::thread::spawn(move || run_unopened_until(&id2, Duration::from_secs(300), &stop2).unwrap());
        // let it take the first message and get going; while it runs, the thread is not free
        let began = Instant::now();
        while thread_open(&id) != Some(true) && began.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert_eq!(thread_open(&id), Some(true), "the background run holds the thread");
        std::thread::sleep(Duration::from_secs(8));
        let asked = Instant::now();
        stop.store(true, Ordering::Relaxed);
        let out = run.join().unwrap();
        eprintln!("{out:?}, left {:.1}s after being told", asked.elapsed().as_secs_f32());
        assert!(out.handed_back && out.turns >= 1);
        assert!(asked.elapsed() < Duration::from_secs(20), "it leaves soon after being told");
        assert_eq!(thread_open(&id), Some(false), "the thread is free: the user can type in it or archive it");
        let mut srv = Server::private().unwrap();
        let left = queue_len(&mut srv, &id);
        eprintln!("still in the thread's queue: {left}");
        assert!(left >= 1, "the message that had not started is still waiting");
    }

    #[test]
    fn nobody_there_declines_approvals_and_tells_the_time() {
        assert_eq!(unattended_answer("item/commandExecution/requestApproval").unwrap()["decision"], "decline");
        assert_eq!(unattended_answer("item/fileChange/requestApproval").unwrap()["decision"], "decline");
        assert!(unattended_answer("execCommandApproval").unwrap()["decision"]["denied"]["rejection"].as_str().unwrap().contains("Dispatch"));
        assert_eq!(unattended_answer("mcpServer/elicitation/request").unwrap()["action"], "decline");
        assert!(unattended_answer("currentTime/read").unwrap()["currentTimeAt"].as_i64().unwrap() > 1_700_000_000);
        assert!(unattended_answer("account/chatgptAuthTokens/refresh").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn initialize_enables_experimental_queue_api() {
        let (client, socket) = UnixStream::pair().unwrap();
        // long enough that a busy machine running the whole suite does not time the read out
        // (it did once, at two seconds); the test is about what is said, not how fast
        socket.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
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
        let mut server = Server { transport: Transport::Shared(ws), next_id: 1, shared: true, backlog: Default::default() };
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
            let mut srv = Server { transport: Transport::Shared(ws), next_id: 1, shared: true, backlog: Default::default() };
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
