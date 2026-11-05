//! The Claude Code hook that hands waiting messages over.
//!
//! Installed into `~/.claude/settings.json` as three entries (UserPromptSubmit,
//! Stop, SessionStart) that run `dispatch hook run`. The hook reads the
//! session id from the event on stdin, lists that session's unread messages,
//! prints them (stdout for prompt/start events, stderr with exit 2 for Stop so
//! the session keeps working on them) and marks them read.

use crate::claude::claude_home;
use crate::i18n::t;
use crate::store::{Store, READ_BY_HOOK};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;

const EVENTS: [&str; 3] = ["UserPromptSubmit", "Stop", "SessionStart"];
const MARK: &str = " hook run";

fn command_for(bin: &str) -> String {
    format!("\"{bin}\"{MARK}")
}

fn is_ours(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hs| hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).map(|c| c.contains(MARK) && c.contains("dispatch")).unwrap_or(false)))
        .unwrap_or(false)
}

pub fn installed() -> bool {
    let Some(v) = fs::read_to_string(claude_home().join("settings.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return false };
    EVENTS.iter().all(|ev| v.pointer(&format!("/hooks/{ev}")).and_then(|a| a.as_array()).map(|a| a.iter().any(is_ours)).unwrap_or(false))
}

/// Add the three entries, replacing any earlier ones of ours (so the hook follows the
/// binary when it moves). Backs the settings file up first.
pub fn install(bin: &str) -> Result<()> {
    let path = claude_home().join("settings.json");
    let mut v: Value = match fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).context("settings.json is not valid JSON")?,
        Err(_) => json!({}),
    };
    if path.exists() {
        let bak = claude_home().join(format!("settings.json.bak-dispatch-{}", chrono::Local::now().format("%Y%m%dT%H%M%S")));
        fs::copy(&path, &bak)?;
    }
    let hooks = v.as_object_mut().context("settings.json root is not an object")?.entry("hooks").or_insert(json!({}));
    let hooks = hooks.as_object_mut().context("hooks is not an object")?;
    for ev in EVENTS {
        let arr = hooks.entry(ev).or_insert(json!([]));
        let arr = arr.as_array_mut().context("hook list is not an array")?;
        arr.retain(|e| !is_ours(e));
        arr.push(json!({ "hooks": [{ "type": "command", "command": command_for(bin), "timeout": 20 }] }));
    }
    fs::create_dir_all(claude_home())?;
    fs::write(&path, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

/// Remove exactly our entries.
pub fn uninstall() -> Result<()> {
    let path = claude_home().join("settings.json");
    let Ok(t) = fs::read_to_string(&path) else { return Ok(()) };
    let mut v: Value = serde_json::from_str(&t)?;
    if let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for ev in EVENTS {
            if let Some(arr) = hooks.get_mut(ev).and_then(|a| a.as_array_mut()) {
                arr.retain(|e| !is_ours(e));
            }
        }
    }
    fs::write(&path, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

pub struct HookOutput {
    pub text: String,
    pub to_stderr: bool,
    pub exit_code: i32,
}

/// Handle one hook event: `input` is the JSON Claude Code pipes to the hook.
pub fn run(store: &Store, input: &str) -> Result<HookOutput> {
    let quiet = HookOutput { text: String::new(), to_stderr: false, exit_code: 0 };
    let Ok(ev) = serde_json::from_str::<Value>(input) else { return Ok(quiet) };
    let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) else { return Ok(quiet) };
    let event = ev.get("hook_event_name").and_then(|v| v.as_str()).unwrap_or("");
    if ev.get("stop_hook_active").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Ok(quiet);
    }
    let mut letters = store.list(Some(sid), true)?;
    if letters.is_empty() {
        return Ok(quiet);
    }
    letters.sort_by(|a, b| a.created.cmp(&b.created));
    let mut text = t("hook.header").replace("{n}", &letters.len().to_string());
    for (i, l) in letters.iter_mut().enumerate() {
        text.push_str(
            &t("hook.item")
                .replace("{i}", &(i + 1).to_string())
                .replace("{from}", &l.from_name)
                .replace("{created}", &l.created)
                .replace("{id}", &crate::model::short_id(&l.id))
                .replace("{body}", l.body.trim()),
        );
        store.set_status(l, READ_BY_HOOK)?;
        store.append_run(&l.id, "hook", "handed", event)?;
    }
    text.push_str(&t("hook.footer").replace("{cmd}", &crate::model::self_cmd()));
    Ok(HookOutput { text, to_stderr: event == "Stop", exit_code: if event == "Stop" { 2 } else { 0 } })
}
