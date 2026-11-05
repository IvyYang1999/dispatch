//! Codex sessions: the app-server (the shared daemon through `codex app-server proxy`, or a
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
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn daemon_socket() -> PathBuf {
    home_dir().join(".codex").join("app-server-control").join("app-server-control.sock")
}

/// A JSON-RPC session with an app-server: `shared` when it is the daemon every Codex client
/// on this machine can share, false when it is a private server we spawned (reads only).
pub struct Server {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    pub shared: bool,
    pub notifications: Vec<Value>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn spawn(args: &[&str], shared: bool) -> Result<Server> {
        let mut child = Command::new("codex")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("codex not found on PATH")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let mut s = Server { child, stdin, reader: BufReader::new(stdout), next_id: 1, shared, notifications: vec![] };
        s.call("initialize", json!({ "clientInfo": { "name": "dispatch", "version": env!("CARGO_PKG_VERSION"), "title": "Dispatch" } }), Duration::from_secs(10))?;
        let _ = writeln!(s.stdin, "{}", json!({ "method": "initialized", "params": {} }));
        Ok(s)
    }

    /// The shared daemon when it is up, else a private server (which never has threads loaded).
    pub fn connect() -> Result<Server> {
        if daemon_socket().exists() {
            if let Ok(s) = Server::spawn(&["app-server", "proxy"], true) {
                return Ok(s);
            }
        }
        Server::spawn(&["app-server"], false)
    }

    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.stdin, "{}", json!({ "id": id, "method": method, "params": params }))?;
        self.stdin.flush()?;
        let deadline = Instant::now() + timeout;
        let mut line = String::new();
        loop {
            if Instant::now() > deadline {
                return Err(anyhow!("{method}: no answer within {timeout:?}"));
            }
            line.clear();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("{method}: the server closed the connection"));
            }
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(e) = msg.get("error") {
                    return Err(anyhow!("{method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(&e.to_string())));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            if msg.get("method").is_some() {
                self.notifications.push(msg);
            }
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

pub fn state_now() -> State {
    let cli = Command::new("codex").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    let mut st = State { cli, daemon: false, loaded: HashSet::new(), at: now_ms() };
    if !cli || !daemon_socket().exists() {
        return st;
    }
    if let Ok(mut srv) = Server::spawn(&["app-server", "proxy"], true) {
        st.daemon = true;
        st.loaded = srv.loaded_ids().unwrap_or_default();
    }
    st
}

/// Queue a message on a thread with the official command. `resume` adds `--resume`, which asks
/// the daemon to load the thread from history first: only when the caller explicitly allows it,
/// since it may start a second instance of a thread that is open elsewhere.
pub fn queue(thread_id: &str, text: &str, resume: bool) -> Result<String> {
    let mut cmd = Command::new("codex");
    cmd.args(["queue", "--thread", thread_id, "--message", text]);
    if resume {
        cmd.arg("--resume");
    }
    let out = cmd.output().context("codex not found on PATH")?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        Err(anyhow!("codex queue failed ({}): {}", out.status, if stderr.is_empty() { stdout } else { stderr }))
    }
}

/// The rollout file of a thread, by its id.
pub fn rollout_path(thread_id: &str) -> Option<PathBuf> {
    let root = home_dir().join(".codex").join("sessions");
    let want = thread_id.to_lowercase();
    for e in walkdir::WalkDir::new(root).max_depth(5).into_iter().flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if e.file_type().is_file() && name.ends_with(".jsonl") && name.contains(&want) {
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

/// Follow a message through the rollout: the user message that carries `marker`, then
/// `task_started` and `task_complete` events after it.
pub fn receipt(thread_id: &str, marker: &str) -> Receipt {
    let Some(path) = rollout_path(thread_id) else { return Receipt::NotFound };
    let Ok(text) = crate::claude::read_tail(&path, 4 * 1024 * 1024) else { return Receipt::NotFound };
    let mut seen = false;
    let mut started = false;
    for line in text.lines() {
        if !seen {
            if line.contains(marker) && (line.contains("\"role\":\"user\"") || line.contains("user_message")) {
                seen = true;
            }
            continue;
        }
        if line.contains("\"task_started\"") {
            started = true;
        } else if line.contains("\"task_complete\"") && started {
            return Receipt::Completed;
        }
    }
    if !seen {
        Receipt::NotFound
    } else if started {
        Receipt::Started
    } else {
        Receipt::Queued
    }
}
