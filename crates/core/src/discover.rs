//! The sessions on this machine.
//!
//! - Claude Code: live records (`~/.claude/sessions`) and transcripts
//!   (`~/.claude/projects/<slug>/<id>.jsonl`; mtime = last activity; the first
//!   lines carry cwd and the first user message).
//! - Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*-<uuid>.jsonl`. Codex does not
//!   put the id on its process command line, so "running" = a codex process
//!   exists and the log was touched in the last 15 minutes.

use crate::claude;
use crate::model::{home_dir, now_ms, short_id, Harness, Session};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

const MAX_AGE_MS: i64 = 48 * 60 * 60 * 1000;
const CODEX_RUNNING_WINDOW_MS: i64 = 15 * 60 * 1000;
/// A list younger than this is answered from memory: the app polls, and `ps` plus a few dozen
/// file heads are not free.
const LIST_TTL: Duration = Duration::from_secs(3);

static LIST: Mutex<Option<(Instant, Vec<Session>)>> = Mutex::new(None);
/// Parsed transcript heads, keyed by path, valid while the file's mtime and size hold.
static HEADS: LazyLock<Mutex<HashMap<PathBuf, (i64, u64, Option<String>, Option<String>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn mtime_ms(p: &Path) -> Option<i64> {
    fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// cwd and title from a transcript's first megabyte, remembered until the file changes.
fn head_of(path: &Path, mtime: i64) -> (Option<String>, Option<String>) {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if let Some((m, s, cwd, title)) = HEADS.lock().unwrap().get(path) {
        if *m == mtime && *s == size {
            return (cwd.clone(), title.clone());
        }
    }
    let (cwd, title) = parse_head(&read_head(path, 1024 * 1024));
    HEADS.lock().unwrap().insert(path.to_path_buf(), (mtime, size, cwd.clone(), title.clone()));
    (cwd, title)
}

fn read_head(p: &Path, bytes: usize) -> String {
    let Ok(mut f) = fs::File::open(p) else { return String::new() };
    let mut buf = vec![0u8; bytes];
    let n = f.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

fn unescape(s: &str) -> Option<String> {
    serde_json::from_str::<String>(&format!("\"{s}\"")).ok()
}

/// Compaction, resume and harness boilerplate says nothing about the session.
pub fn clean_title(s: &str) -> Option<String> {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = Regex::new(r"^<[^>]+>\s*").ok()?.replace(&t, "").trim().to_string();
    if t.is_empty() {
        return None;
    }
    let noise = Regex::new(r"(?i)^(This session is being continued|<system-reminder>|Caveat:|<local-command|<command-|\[Image|MemoraX|Base directory|You are|#{1,6} |\{|Filesystem sandboxing|<environment_context>|<user_instructions>|<permissions|Auto mode lets Claude)").ok()?;
    if noise.is_match(&t) {
        return None;
    }
    if Regex::new(r"(?i)^[a-z0-9_-]{6,}$").ok()?.is_match(&t) {
        return None;
    }
    let chars: Vec<char> = t.chars().collect();
    Some(if chars.len() > 48 { format!("{}…", chars[..48].iter().collect::<String>()) } else { t })
}

fn parse_head(text: &str) -> (Option<String>, Option<String>) {
    let cwd = Regex::new(r#""cwd":"((?:[^"\\]|\\.)*)""#).ok().and_then(|re| re.captures(text).and_then(|c| unescape(&c[1]).or_else(|| Some(c[1].to_string()))));
    let mut title = None;
    // No closing quote required: a first message longer than the cap still yields its opening words.
    if let Ok(re) = Regex::new(r#""(?:text|content)":"((?:[^"\\]|\\.){2,400})"#) {
        for c in re.captures_iter(text) {
            if let Some(raw) = unescape(&c[1]) {
                if let Some(t) = clean_title(&raw) {
                    title = Some(t);
                    break;
                }
            }
        }
    }
    (cwd, title)
}

/// Every process's command line, one per line.
#[cfg(unix)]
fn ps_commands() -> String {
    std::process::Command::new("ps")
        .args(["-axo", "command"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

#[cfg(windows)]
fn ps_commands() -> String {
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { $_.CommandLine }"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn discover_claude(running_ids: &std::collections::HashSet<String>, now: i64) -> Vec<Session> {
    let root = claude::claude_home().join("projects");
    let mut out = vec![];
    let Ok(dirs) = fs::read_dir(&root) else { return out };
    for d in dirs.flatten() {
        let Ok(files) = fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".jsonl") else { continue };
            if !crate::model::is_uuid(id) {
                continue;
            }
            let path = f.path();
            let Some(mtime) = mtime_ms(&path) else { continue };
            let running = running_ids.contains(&id.to_lowercase());
            if !running && now - mtime > MAX_AGE_MS {
                continue;
            }
            let (cwd, title) = head_of(&path, mtime);
            out.push(Session {
                harness: Harness::Claude,
                id: id.to_lowercase(),
                short_id: short_id(id),
                name: None,
                cwd,
                title,
                last_active: mtime,
                running,
                desktop: false,
                host_session_id: None,
                status: None,
                transcript: Some(path.to_string_lossy().to_string()),
            });
        }
    }
    out
}

fn discover_codex(codex_running: bool, now: i64) -> Vec<Session> {
    let root = home_dir().join(".codex").join("sessions");
    let uuid_re = Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    // Threads the shared daemon has loaded are running whatever the file times say.
    let loaded = crate::codex::state().loaded;
    let mut out = vec![];
    for e in WalkDir::new(&root).max_depth(5).into_iter().flatten() {
        if !e.file_type().is_file() || !e.file_name().to_string_lossy().ends_with(".jsonl") {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let Some(m) = uuid_re.find(&name) else { continue };
        let id = m.as_str().to_lowercase();
        let Some(mtime) = mtime_ms(e.path()) else { continue };
        if now - mtime > MAX_AGE_MS {
            continue;
        }
        let (cwd, title) = head_of(e.path(), mtime);
        let is_loaded = loaded.contains(&id);
        let running = is_loaded || (codex_running && now - mtime < CODEX_RUNNING_WINDOW_MS);
        out.push(Session {
            harness: Harness::Codex,
            short_id: short_id(&id),
            id,
            name: None,
            cwd,
            title,
            last_active: mtime,
            running,
            status: if is_loaded { Some("loaded".into()) } else { None },
            desktop: false,
            host_session_id: None,
            transcript: Some(e.path().to_string_lossy().to_string()),
        });
    }
    out
}

/// Grok Bot: the bots the signed-in app can reach through its gateway. They live in the cloud,
/// so they are always "running"; the list itself is remembered for a minute in `grok::state`.
fn discover_grok(now: i64) -> Vec<Session> {
    let st = crate::grok::state();
    st.bots
        .iter()
        .filter(|b| !b.is_group)
        .map(|b| Session {
            harness: Harness::Grok,
            id: b.id.clone(),
            short_id: short_id(&b.id),
            name: Some(b.name.clone()),
            cwd: None,
            title: Some(b.description.clone()).filter(|d| !d.is_empty()),
            last_active: now,
            running: true,
            desktop: true,
            host_session_id: None,
            status: Some("cloud".into()),
            transcript: None,
        })
        .collect()
}

/// All recently active sessions on this machine, running ones first, newest first.
/// Answered from a 3-second cache when one exists; `sessions_now` always looks again.
pub fn sessions() -> Vec<Session> {
    if let Some((at, list)) = LIST.lock().unwrap().as_ref() {
        if at.elapsed() < LIST_TTL {
            return list.clone();
        }
    }
    let list = sessions_now();
    *LIST.lock().unwrap() = Some((Instant::now(), list.clone()));
    list
}

pub fn sessions_now() -> Vec<Session> {
    let now = now_ms();
    let procs = ps_commands();
    let mut running_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(re) = Regex::new(r"(?i)--(?:resume|session-id)[= ]([0-9a-f-]{36})") {
        for c in re.captures_iter(&procs) {
            running_ids.insert(c[1].to_lowercase());
        }
    }
    let live = claude::live_records();
    for r in &live {
        running_ids.insert(r.session_id.clone());
    }
    let codex_running = Regex::new(r"\bcodex\b").map(|re| re.is_match(&procs)).unwrap_or(false);

    let mut by_id: HashMap<String, Session> = HashMap::new();
    for s in discover_claude(&running_ids, now).into_iter().chain(discover_codex(codex_running, now)).chain(discover_grok(now)) {
        match by_id.get_mut(&s.id) {
            Some(prev) => {
                // a session can appear under several project slugs after a cwd change
                let running = prev.running || s.running;
                if s.last_active > prev.last_active {
                    *prev = s;
                }
                prev.running = running;
            }
            None => {
                by_id.insert(s.id.clone(), s);
            }
        }
    }
    // Live records know more than transcripts: the name, the cwd, the desktop id, the status.
    for r in live {
        let entry = by_id.entry(r.session_id.clone()).or_insert_with(|| Session {
            harness: Harness::Claude,
            id: r.session_id.clone(),
            short_id: short_id(&r.session_id),
            name: None,
            cwd: None,
            title: None,
            last_active: r.updated_at,
            running: true,
            desktop: false,
            host_session_id: None,
            status: None,
            transcript: None,
        });
        entry.running = true;
        entry.desktop = r.desktop;
        entry.host_session_id = r.host_session_id.clone();
        entry.status = r.status.clone();
        if r.name.is_some() {
            entry.name = r.name.clone();
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
        if r.updated_at > entry.last_active {
            entry.last_active = r.updated_at;
        }
        if entry.transcript.is_none() {
            entry.transcript = claude::transcript_path(&r.session_id, r.cwd.as_deref()).map(|p| p.to_string_lossy().to_string());
        }
    }
    let mut all: Vec<Session> = by_id.into_values().collect();
    all.sort_by(|a, b| b.running.cmp(&a.running).then(b.last_active.cmp(&a.last_active)));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_skip_boilerplate() {
        // a leading tag is stripped, what follows is the title
        assert_eq!(clean_title("<system-reminder>\nfoo"), Some("foo".into()));
        assert_eq!(clean_title("Caveat: the messages below"), None);
        assert_eq!(clean_title("  帮我  看看 这个 "), Some("帮我 看看 这个".into()));
    }
}
