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

/// Take a copy out of the box while it is handed over, so a second hook run (the plugin's and
/// the installed one both fire on the same event) does not hand it over as well. The file comes
/// back under its name once marked.
fn claim(path: &str) -> Option<String> {
    let claimed = format!("{path}.handing");
    fs::rename(path, &claimed).ok()?;
    Some(claimed)
}

fn release(claimed: &str) -> Option<String> {
    let back = claimed.strip_suffix(".handing")?.to_string();
    fs::rename(claimed, &back).ok()?;
    Some(back)
}

/// A copy left claimed by a run that died comes back after `age`.
pub fn recover_stale(store: &Store, age: std::time::Duration) -> usize {
    let mut n = 0;
    let Ok(boxes) = fs::read_dir(store.messages_dir()) else { return 0 };
    for b in boxes.flatten() {
        let Ok(files) = fs::read_dir(b.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md.handing") {
                continue;
            }
            let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).map(|e| e >= age).unwrap_or(true);
            if old && release(&f.path().to_string_lossy()).is_some() {
                n += 1;
            }
        }
    }
    n
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
    recover_stale(store, std::time::Duration::from_secs(60));
    let mut letters = store.list(Some(sid), true)?;
    if letters.is_empty() {
        return Ok(quiet);
    }
    letters.sort_by(|a, b| a.created.cmp(&b.created));
    let mut items = String::new();
    let mut n = 0;
    for l in letters.iter_mut() {
        // another run of the hook has this one
        let Some(claimed) = claim(&l.path) else { continue };
        l.path = claimed;
        n += 1;
        items.push_str(
            &t("hook.item")
                .replace("{i}", &n.to_string())
                .replace("{from}", &crate::send::sender_text(l))
                .replace("{created}", &l.created)
                .replace("{id}", &crate::model::short_id(&l.id))
                .replace("{body}", l.body.trim()),
        );
        store.set_status(l, READ_BY_HOOK)?;
        store.append_run(&l.id, "hook", "handed", event)?;
        if let Some(back) = release(&l.path) {
            l.path = back;
        }
    }
    if n == 0 {
        return Ok(quiet);
    }
    let mut text = t("hook.header").replace("{n}", &n.to_string());
    text.push_str(&items);
    text.push_str(&t("hook.footer").replace("{cmd}", &crate::model::self_cmd()));
    Ok(HookOutput { text, to_stderr: event == "Stop", exit_code: if event == "Stop" { 2 } else { 0 } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Letter, UNREAD};

    #[test]
    fn a_copy_another_run_holds_is_skipped_and_a_stale_claim_comes_back() {
        let dir = std::env::temp_dir().join(format!("dispatch-hook-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let sid = "55555555-6666-4777-8888-999999999999";
        let mk = |body: &str| {
            let mut l = Letter {
                id: uuid::Uuid::new_v4().to_string(),
                dispatch_id: uuid::Uuid::new_v4().to_string(),
                from: "cli".into(),
                from_name: "cli".into(),
                to: sid.into(),
                to_name: "T".into(),
                urgency: "普通".into(),
                wake: true,
                status: UNREAD.into(),
                created: crate::store::now_text(),
                deadline: Some("2999-01-01 00:00".into()),
                to_harness: Some("claude".into()),
                from_host: None,
                from_host_name: None,
                to_host: None,
                to_host_name: None,
                from_harness: Some("human".into()),
                body: body.into(),
                path: String::new(),
            };
            s.write_letter(&mut l).unwrap();
            l
        };
        let a = mk("first");
        let b = mk("second");
        // another run has b
        let held = claim(&b.path).unwrap();
        let out = run(&s, &format!(r#"{{"session_id":"{sid}","hook_event_name":"UserPromptSubmit"}}"#)).unwrap();
        assert!(out.text.contains("first") && !out.text.contains("second"), "{}", out.text);
        assert!(out.text.contains("1 message") || out.text.contains("1 条"));
        assert_eq!(s.find(&a.id).unwrap().unwrap().status, READ_BY_HOOK);
        assert!(std::path::Path::new(&a.path).is_file(), "a is back under its name");
        assert!(std::path::Path::new(&held).is_file(), "b is still held");
        // the held copy is invisible to the box until it is released
        assert_eq!(s.list(Some(sid), true).unwrap().len(), 0);
        assert_eq!(recover_stale(&s, std::time::Duration::ZERO), 1);
        assert_eq!(s.list(Some(sid), true).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
