//! Grok Bot: xAI and Cursor's always-on agents. The bots run on cloud computers; the desktop
//! app on this Mac only holds the signed-in session. A message reaches a bot through the
//! gateway the app itself uses (`POST /api/sendPrompt`), and this first version drives that
//! through the third-party `gbot` command line (npm `grok-bot-cli`), which already does the
//! session decryption and the gateway calls. Receipts are read from the bot's transcript tail.
//!
//! What the bot gets is a direct message from its user: the docs say it takes priority over
//! background work and may redirect the current turn, so the reach text says so.

use crate::model::{home_dir, now_ms};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_group: bool,
}

/// The `gbot` command: `DISPATCH_GBOT` (tests and odd installs), then PATH and the usual npm
/// global folders, since the app is launched by Finder without the terminal's PATH.
pub fn executable() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DISPATCH_GBOT") {
        return Some(PathBuf::from(p));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).flat_map(|dir| crate::codex::command_names("gbot").into_iter().map(move |n| dir.join(n))).collect()).unwrap_or_default();
    let home = home_dir();
    candidates.extend([
        home.join(".local/bin/gbot"),
        home.join(".local/node/bin/gbot"),
        home.join(".npm-global/bin/gbot"),
        home.join(".volta/bin/gbot"),
        PathBuf::from("/opt/homebrew/bin/gbot"),
        PathBuf::from("/usr/local/bin/gbot"),
    ]);
    candidates.into_iter().find(|p| crate::codex::is_executable(p))
}

/// The desktop app, in /Applications or ~/Applications.
pub fn app_dir() -> Option<PathBuf> {
    [home_dir().join("Applications/Grok Bot.app"), PathBuf::from("/Applications/Grok Bot.app")].into_iter().find(|p| p.is_dir())
}

/// Where the app keeps its session (`sand-secrets.json`, `gateway-descriptor.json`, `desktop-status.json`).
pub fn data_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Grok Bot")
}

pub fn signed_in() -> bool {
    data_dir().join("sand-secrets.json").is_file()
}

/// What this Mac's Grok Bot can do right now. Listing the bots is a network call through the
/// app's gateway, so it is remembered for a minute; `state_now` always asks again.
#[derive(Debug, Clone)]
pub struct State {
    pub cli: Option<PathBuf>,
    pub app: bool,
    pub signed_in: bool,
    pub bots: Vec<Bot>,
    pub error: Option<String>,
    pub at: i64,
}

impl State {
    /// Everything in place to deliver: app, session, command line, and the gateway answered.
    pub fn ready(&self) -> bool {
        self.app && self.signed_in && self.cli.is_some() && self.error.is_none()
    }

    pub fn bot(&self, id_or_name: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.id.eq_ignore_ascii_case(id_or_name)).or_else(|| self.bots.iter().find(|b| b.name.eq_ignore_ascii_case(id_or_name)))
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
const STATE_TTL_MS: i64 = 60_000;

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
    let mut st = State { cli: executable(), app: app_dir().is_some(), signed_in: signed_in(), bots: vec![], error: None, at: now_ms() };
    if !st.app || !st.signed_in || st.cli.is_none() {
        return st;
    }
    match list_bots() {
        Ok(bots) => st.bots = bots,
        Err(e) => st.error = Some(format!("{e:#}")),
    }
    st
}

/// Run `gbot` with a deadline; both streams captured.
fn run(args: &[&str], timeout: Duration) -> Result<(String, String)> {
    let exe = executable().ok_or_else(|| anyhow!("gbot not found"))?;
    let mut child = Command::new(&exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("start {}", exe.display()))?;
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    fn drain(stream: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = vec![];
            let _ = stream.take(4 * 1024 * 1024).read_to_end(&mut bytes);
            bytes
        })
    }
    let (out, err) = (drain(stdout), drain(stderr));
    let deadline = Instant::now() + timeout;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break Some(exit),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    let Some(exit) = exit else { return Err(anyhow!("gbot {} timed out after {timeout:?}", args.join(" "))) };
    if !exit.success() {
        return Err(anyhow!("gbot {} failed ({exit}): {}", args.join(" "), [stdout.trim(), stderr.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")));
    }
    Ok((stdout, stderr))
}

/// The JSON `gbot` prints: the last JSON value on stdout (it may print a notice first).
fn json_of(stdout: &str) -> Result<Value> {
    let trimmed = stdout.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }
    for (i, c) in trimmed.char_indices() {
        if c == '{' || c == '[' {
            if let Ok(v) = serde_json::from_str::<Value>(&trimmed[i..]) {
                return Ok(v);
            }
        }
    }
    Err(anyhow!("gbot printed no JSON: {}", trimmed.chars().take(200).collect::<String>()))
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match &v[*k] {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// The array inside whatever shape `gbot` chose: a bare array, or an object holding one.
fn array_of(v: &Value, keys: &[&str]) -> Vec<Value> {
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    keys.iter().find_map(|k| v[*k].as_array().cloned()).unwrap_or_default()
}

pub fn parse_bots(stdout: &str) -> Result<Vec<Bot>> {
    let v = json_of(stdout)?;
    Ok(array_of(&v, &["bots", "agents", "data", "items"])
        .iter()
        .filter_map(|b| {
            let id = str_of(b, &["id", "agentId", "agent_id"])?;
            let name = str_of(b, &["name", "title"]).unwrap_or_else(|| id.clone());
            Some(Bot {
                id,
                name,
                description: str_of(b, &["description", "title"]).unwrap_or_default(),
                is_group: b["isGroup"].as_bool().or_else(|| b["group"].as_bool()).unwrap_or(false),
            })
        })
        .collect())
}

pub fn list_bots() -> Result<Vec<Bot>> {
    let (out, _) = run(&["bots", "list", "--json"], Duration::from_secs(25))?;
    parse_bots(&out)
}

#[derive(Debug, Clone)]
pub struct SendOutcome {
    /// The gateway's id for our message, when `gbot` reported one.
    pub message_id: Option<String>,
    pub detail: String,
}

pub fn parse_send(stdout: &str) -> SendOutcome {
    let v = json_of(stdout).unwrap_or(Value::Null);
    let message_id = str_of(&v, &["messageId", "message_id", "id"]).or_else(|| str_of(&v["message"], &["id"]));
    let detail = if v.is_null() { stdout.trim().chars().take(200).collect() } else { v.to_string().chars().take(200).collect() };
    SendOutcome { message_id, detail }
}

/// Send a message to a bot (by the name `gbot` resolves, or its id).
pub fn send(target: &str, text: &str) -> Result<SendOutcome> {
    let (out, _) = run(&["send", target, text, "--json"], Duration::from_secs(45))?;
    Ok(parse_send(&out))
}

static THREADS: Mutex<Vec<(String, i64, Vec<Value>)>> = Mutex::new(Vec::new());
const THREAD_TTL_MS: i64 = 20_000;

/// The bot's recent transcript entries, oldest first. Cached for 20 seconds per bot: the app's
/// watcher and the feed ask often, and each read is a gateway call.
pub fn thread(target: &str) -> Result<Vec<Value>> {
    let now = crate::model::now_ms();
    if let Ok(c) = THREADS.lock() {
        if let Some((_, _, entries)) = c.iter().find(|(t, at, _)| t == target && now - at < THREAD_TTL_MS) {
            return Ok(entries.clone());
        }
    }
    let (out, _) = run(&["thread", target, "--json"], Duration::from_secs(25))?;
    let v = json_of(&out)?;
    let entries = array_of(&v, &["entries", "items", "data", "transcript", "messages"]);
    if let Ok(mut c) = THREADS.lock() {
        c.retain(|(t, _, _)| t != target);
        c.push((target.to_string(), now, entries.clone()));
    }
    Ok(entries)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    NotFound,
    /// Our message is in the bot's transcript, nothing from the bot after it yet.
    Delivered,
    /// The bot wrote something after our message.
    Answered,
}

fn entry_text(v: &Value) -> String {
    if let Some(s) = str_of(v, &["text", "prompt", "preview"]) {
        return s;
    }
    match &v["content"] {
        Value::String(s) => return s.clone(),
        Value::Array(parts) => return parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => {}
    }
    match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => v["message"].as_str().unwrap_or("").to_string(),
    }
}

/// Entries the bot itself wrote, as far as the shapes seen so far tell them apart from ours.
fn from_bot(v: &Value) -> bool {
    if v["kind"] == "send-message" || v["kind"] == "assistant" {
        return true;
    }
    if let Some(role) = v["role"].as_str().or_else(|| v["author"]["type"].as_str()).or_else(|| v["author"].as_str()).or_else(|| v["source"].as_str()) {
        return !matches!(role.to_ascii_lowercase().as_str(), "user" | "human" | "you" | "prompt");
    }
    v["prompt"].is_null() && !v["message"].is_null()
}

pub fn receipt_entries(entries: &[Value], message_id: Option<&str>, marker: &str) -> Receipt {
    let ours = entries.iter().position(|e| {
        message_id.map(|id| str_of(e, &["id", "messageId"]).as_deref() == Some(id)).unwrap_or(false) || entry_text(e).contains(marker)
    });
    let Some(i) = ours else { return Receipt::NotFound };
    if entries[i + 1..].iter().any(|e| from_bot(e) && !entry_text(e).contains(marker)) {
        Receipt::Answered
    } else {
        Receipt::Delivered
    }
}

/// Where our message stands in the bot's transcript: `target` is the bot's id or name,
/// `marker` the text that identifies our message (its short id in the envelope).
pub fn receipt(target: &str, message_id: Option<&str>, marker: &str) -> Receipt {
    let st = state();
    let name = st.bot(target).map(|b| b.name.clone()).unwrap_or_else(|| target.to_string());
    match thread(&name) {
        Ok(entries) => receipt_entries(&entries, message_id, marker),
        Err(_) => Receipt::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bots_parse_from_array_or_object() {
        let a = parse_bots(r#"[{"id":"b1","name":"Researcher","description":"reads"},{"id":"g1","name":"Launch","isGroup":true}]"#).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "Researcher");
        assert!(a[1].is_group);
        let b = parse_bots("Loaded 1 bot\n{\"bots\":[{\"id\":\"b2\",\"title\":\"Writer\"}]}").unwrap();
        assert_eq!(b[0].name, "Writer");
    }

    #[test]
    fn send_parse_reads_message_id() {
        assert_eq!(parse_send(r#"{"messageId":"m-9","envelopeId":"e-1"}"#).message_id.as_deref(), Some("m-9"));
        assert_eq!(parse_send("Sent to bot Researcher").message_id, None);
    }

    #[test]
    fn receipts_follow_the_transcript() {
        let entries = vec![
            json!({"id":"m-1","prompt":"earlier question"}),
            json!({"id":"m-9","prompt":"📨 From x (via Dispatch, now, id abcd1234): hello"}),
        ];
        assert_eq!(receipt_entries(&entries, Some("m-9"), "abcd1234"), Receipt::Delivered);
        assert_eq!(receipt_entries(&entries, None, "abcd1234"), Receipt::Delivered);
        assert_eq!(receipt_entries(&entries, None, "zzzz"), Receipt::NotFound);
        let mut answered = entries.clone();
        answered.push(json!({"id":"m-10","kind":"send-message","message":{"type":"text","content":"Got it."}}));
        assert_eq!(receipt_entries(&answered, Some("m-9"), "abcd1234"), Receipt::Answered);
        let mut only_user = entries.clone();
        only_user.push(json!({"id":"m-11","role":"user","text":"another prompt"}));
        assert_eq!(receipt_entries(&only_user, Some("m-9"), "abcd1234"), Receipt::Delivered);
    }
}
