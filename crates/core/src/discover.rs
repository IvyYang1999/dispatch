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
/// Every session on this machine whatever its age, for searches; remembered ten seconds.
static ALL: Mutex<Option<(Instant, Vec<Session>)>> = Mutex::new(None);
/// What Codex's own database says about its threads, kept while the database files are unchanged.
static CODEX_THREADS: Mutex<Option<(Vec<Option<i64>>, HashMap<String, CodexThread>)>> = Mutex::new(None);
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
    let noise = Regex::new(r"(?i)^(This session is being continued|<system-reminder>|Caveat:|<local-command|<command-|\[Image|MemoraX|Base directory|You are|#{1,6} |\{|Filesystem sandboxing|<environment_context>|<user_instructions>|<permissions|Auto mode lets Claude|Any earlier instruction|Here is a list of plugins|(Added )?[Dd]eferred tool namespaces|<INSTRUCTIONS>|<skills_instructions>|<apps_instructions>)").ok()?;
    // a log that cut the message short (Codex writes "…119 tokens truncated…")
    if noise.is_match(&t) || t.contains("tokens truncated") {
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

fn discover_claude(running_ids: &std::collections::HashSet<String>, now: i64, max_age: Option<i64>) -> Vec<Session> {
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
            if !running && max_age.map(|m| now - mtime > m).unwrap_or(false) {
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
                host: None,
                project: None,
                app_name: None,
            });
        }
    }
    out
}

fn discover_codex(codex_running: bool, now: i64, max_age: Option<i64>) -> Vec<Session> {
    let root = home_dir().join(".codex").join("sessions");
    let uuid_re = Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    // Threads the shared daemon has loaded are running whatever the file times say.
    let loaded = crate::codex::state().loaded;
    let threads = codex_threads();
    let mut out = vec![];
    for e in WalkDir::new(&root).max_depth(5).into_iter().flatten() {
        if !e.file_type().is_file() || !e.file_name().to_string_lossy().ends_with(".jsonl") {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let Some(m) = uuid_re.find(&name) else { continue };
        let id = m.as_str().to_lowercase();
        let Some(mtime) = mtime_ms(e.path()) else { continue };
        let is_loaded = loaded.contains(&id);
        if !is_loaded && max_age.map(|m| now - mtime > m).unwrap_or(false) {
            continue;
        }
        let (cwd, title) = head_of(e.path(), mtime);
        let running = is_loaded || (codex_running && now - mtime < CODEX_RUNNING_WINDOW_MS);
        let thread = threads.get(&id);
        out.push(Session {
            harness: Harness::Codex,
            short_id: short_id(&id),
            name: thread.and_then(|t| t.name.clone()),
            cwd,
            // the title Codex shows in its sidebar, else the first message
            title: thread.and_then(|t| t.title.clone()).or(title),
            id,
            last_active: mtime,
            running,
            status: if is_loaded { Some("loaded".into()) } else { None },
            desktop: false,
            host_session_id: None,
            transcript: Some(e.path().to_string_lossy().to_string()),
            host: None,
            project: thread.and_then(|t| t.project.clone()),
            app_name: None,
        });
    }
    out
}

/// A Codex thread as Codex's own database (`~/.codex/state_<n>.sqlite`) records it: the name
/// the user gave it, the title its sidebar shows, and the project it is filed under.
#[derive(Debug, Clone, Default, PartialEq)]
struct CodexThread {
    name: Option<String>,
    title: Option<String>,
    project: Option<String>,
}

/// The newest `state_<n>.sqlite` in Codex's home.
fn codex_state_db() -> Option<PathBuf> {
    let dir = home_dir().join(".codex");
    let re = Regex::new(r"^state_(\d+)\.sqlite$").ok()?;
    fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            re.captures(&name).and_then(|c| c[1].parse::<u32>().ok()).map(|n| (n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

/// Thread names, titles and projects by thread id, read with the `sqlite3` command (read-only),
/// again only when the database or its write-ahead log changed. Empty when there is no
/// database or no `sqlite3`: discovery then goes by the rollout files alone.
fn codex_threads() -> HashMap<String, CodexThread> {
    let Some(db) = codex_state_db() else { return HashMap::new() };
    let wal = PathBuf::from(format!("{}-wal", db.to_string_lossy()));
    let stamp = vec![mtime_ms(&db), mtime_ms(&wal)];
    if let Some((at, map)) = CODEX_THREADS.lock().unwrap().as_ref() {
        if *at == stamp {
            return map.clone();
        }
    }
    let map = read_codex_threads(&db);
    *CODEX_THREADS.lock().unwrap() = Some((stamp, map.clone()));
    map
}

fn read_codex_threads(db: &Path) -> HashMap<String, CodexThread> {
    let sqlite = if Path::new("/usr/bin/sqlite3").exists() { "/usr/bin/sqlite3" } else { "sqlite3" };
    // newer Codex files threads under projects; older databases have neither column nor table
    let queries = [
        "select t.id as id, t.name as name, t.title as title, p.name as project from threads t left join projects p on p.id = t.project_id",
        "select id, name, title from threads",
        "select id, title from threads",
    ];
    for q in queries {
        let Ok(out) = std::process::Command::new(sqlite).arg("-readonly").arg("-json").arg(db).arg(q).stderr(std::process::Stdio::null()).output() else { return HashMap::new() };
        if out.status.success() {
            return parse_codex_threads(&String::from_utf8_lossy(&out.stdout));
        }
    }
    HashMap::new()
}

fn parse_codex_threads(json: &str) -> HashMap<String, CodexThread> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    let text = |r: &serde_json::Value, k: &str| r.get(k).and_then(|v| v.as_str()).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty());
    rows.iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_str()?.to_lowercase();
            Some((id, CodexThread { name: text(r, "name"), title: text(r, "title").and_then(|t| clean_title(&t)), project: text(r, "project") }))
        })
        .collect()
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
            host: None,
            project: None,
            app_name: None,
        })
        .collect()
}

/// All recently active sessions on this machine, running ones first, newest first.
/// Answered from a 3-second cache when one exists; `sessions_now` always looks again.
pub fn local() -> Vec<Session> {
    if let Some((at, list)) = LIST.lock().unwrap().as_ref() {
        if at.elapsed() < LIST_TTL {
            return list.clone();
        }
    }
    let list = sessions_now();
    *LIST.lock().unwrap() = Some((Instant::now(), list.clone()));
    list
}

/// The sessions here, then the sessions on every paired machine that answers (each marked
/// with its host). Remote lists come from a 20-second cache and never block on a machine
/// that just failed to answer.
pub fn sessions() -> Vec<Session> {
    let mut all = local();
    all.extend(crate::peer::remote_sessions());
    all
}

pub fn sessions_now() -> Vec<Session> {
    gather(Some(MAX_AGE_MS))
}

/// Every session this machine has records of, however old, from a 10-second cache.
pub fn all_local() -> Vec<Session> {
    if let Some((at, list)) = ALL.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(10) {
            return list.clone();
        }
    }
    let list = gather(None);
    *ALL.lock().unwrap() = Some((Instant::now(), list.clone()));
    list
}

fn gather(max_age: Option<i64>) -> Vec<Session> {
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
    for s in discover_claude(&running_ids, now, max_age).into_iter().chain(discover_codex(codex_running, now, max_age)).chain(discover_grok(now)) {
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
            host: None,
            project: None,
            app_name: None,
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
    // the address book: names on listed sessions, and the named ones the window left out
    crate::contacts::apply_local(&mut all);
    all.sort_by(|a, b| b.running.cmp(&a.running).then(b.last_active.cmp(&a.last_active)));
    all
}

/// The words of a query, lowercased.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Why a session matches when every word is found in one of its fields: the strongest field any
/// word was found in. The fields, strongest first: id (a prefix of at least four characters),
/// name (its own or the address book's), about (the address book's description), project,
/// folder, title, kind (Claude, Codex), machine.
pub fn reason(s: &Session, words: &[String], contact: Option<&crate::contacts::Contact>) -> Option<&'static str> {
    const LABELS: [&str; 8] = ["id", "name", "about", "project", "cwd", "title", "kind", "host"];
    if words.is_empty() {
        return Some("all");
    }
    let low = |o: Option<&str>| o.map(str::to_lowercase).unwrap_or_default();
    let fields = [
        format!("{}\n{}", low(s.name.as_deref()), low(contact.map(|c| c.name.as_str()))),
        low(contact.and_then(|c| c.description.as_deref())),
        low(s.project.as_deref()),
        low(s.cwd.as_deref()),
        low(s.title.as_deref()),
        s.harness.label().to_lowercase(),
        low(s.host.as_ref().map(|h| h.name.as_str())),
    ];
    let mut best = LABELS.len();
    for w in words {
        let at = if w.chars().count() >= 4 && s.id.starts_with(w.as_str()) { 0 } else { 1 + fields.iter().position(|f| f.contains(w.as_str()))? };
        best = best.min(at);
    }
    LABELS.get(best).copied()
}

/// The sessions every word of the query is found in, each with why, in the order given.
pub fn matching(query: &str, sessions: Vec<Session>) -> Vec<(Session, &'static str)> {
    let words = words(query);
    let book = crate::contacts::list();
    sessions
        .into_iter()
        .filter_map(|s| {
            let host_id = s.host.as_ref().map(|h| h.id.clone());
            let contact = book.iter().find(|c| c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| &h.id) == host_id.as_ref());
            reason(&s, &words, contact).map(|r| (s, r))
        })
        .collect()
}

/// Every session a query could mean, whatever its age: all of this machine's, and what each
/// paired machine finds for the query (one too old to search sends its recent list). With a
/// machine named, only that machine's. Each session once.
pub fn anywhere(query: &str, only: Option<&crate::peer::PeerInfo>) -> Vec<Session> {
    let mut all = if only.is_none() { all_local() } else { vec![] };
    all.extend(crate::peer::remote_find(query, only));
    let mut seen = std::collections::HashSet::new();
    all.retain(|s| seen.insert((s.host.as_ref().map(|h| h.id.clone()), s.id.to_lowercase())));
    all
}

/// One session by id, whatever its age: the recent list first, then its files.
pub fn lookup(id: &str) -> Option<Session> {
    if let Some(s) = local().into_iter().find(|s| s.id.eq_ignore_ascii_case(id)) {
        return Some(s);
    }
    lookup_files(id)
}

/// A session from its files alone: a Claude Code transcript, a Codex rollout, or a Grok bot.
pub fn lookup_files(id: &str) -> Option<Session> {
    let id = id.to_lowercase();
    if crate::model::is_uuid(&id) {
        if let Some(p) = claude::transcript_path(&id, None) {
            let mtime = mtime_ms(&p).unwrap_or(0);
            let (cwd, title) = head_of(&p, mtime);
            return Some(Session {
                harness: Harness::Claude,
                short_id: short_id(&id),
                running: claude::is_running(&id),
                id,
                name: None,
                cwd,
                title,
                last_active: mtime,
                desktop: false,
                host_session_id: None,
                status: None,
                transcript: Some(p.to_string_lossy().to_string()),
                host: None,
                project: None,
                app_name: None,
            });
        }
        if let Some(p) = codex_rollout(&id) {
            let mtime = mtime_ms(&p).unwrap_or(0);
            let (cwd, title) = head_of(&p, mtime);
            let loaded = crate::codex::state().loaded.contains(&id);
            return Some(Session {
                harness: Harness::Codex,
                short_id: short_id(&id),
                id,
                name: None,
                cwd,
                title,
                last_active: mtime,
                running: loaded,
                desktop: false,
                host_session_id: None,
                status: if loaded { Some("loaded".into()) } else { None },
                transcript: Some(p.to_string_lossy().to_string()),
                host: None,
                project: None,
                app_name: None,
            });
        }
    }
    discover_grok(now_ms()).into_iter().find(|s| s.id.eq_ignore_ascii_case(&id))
}

/// The rollout file of a Codex thread, by its id, at any age.
fn codex_rollout(id: &str) -> Option<PathBuf> {
    let root = home_dir().join(".codex").join("sessions");
    WalkDir::new(&root)
        .max_depth(5)
        .into_iter()
        .flatten()
        .find(|e| e.file_type().is_file() && e.file_name().to_string_lossy().to_lowercase().contains(id))
        .map(|e| e.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: Option<&str>, cwd: &str, title: &str) -> Session {
        Session { harness: Harness::Codex, id: id.into(), short_id: short_id(id), name: name.map(str::to_string), cwd: Some(cwd.into()), title: Some(title.into()), last_active: 1, running: false, desktop: false, host_session_id: None, status: None, transcript: None, host: None, project: None, app_name: None }
    }

    #[test]
    fn every_word_must_match_somewhere() {
        let s = session("0199aaaa-1111-4222-8333-444455556666", Some("QA"), "/Users/y/developer/opentrends", "检查发布流程");
        let w = |q: &str| words(q);
        assert_eq!(reason(&s, &w("OpenTrends Codex"), None), Some("cwd"), "a folder word and a kind word");
        assert_eq!(reason(&s, &w("qa opentrends"), None), Some("name"), "the strongest field wins");
        assert_eq!(reason(&s, &w("发布"), None), Some("title"));
        assert_eq!(reason(&s, &w("0199aaaa"), None), Some("id"));
        assert_eq!(reason(&s, &w("opentrends claude"), None), None, "a Codex session is not a Claude one");
        assert_eq!(reason(&s, &w("019"), None), None, "three characters are not an id prefix");
        assert_eq!(reason(&s, &w(""), None), Some("all"));
        let mut p = s.clone();
        p.project = Some("Trends Lab".into());
        p.cwd = None;
        assert_eq!(reason(&p, &w("trends lab"), None), Some("project"));
    }

    #[test]
    fn codex_thread_names_titles_and_projects() {
        let rows = r#"[{"id":"0199AAAA-1111-4222-8333-444455556666","name":"OpenTrends 发布","title":"  检查  一下 发布流程 ","project":"OpenTrends"},
{"id":"0199bbbb-1111-4222-8333-444455556666","name":null,"title":"","project":null}]"#;
        let m = parse_codex_threads(rows);
        assert_eq!(m.len(), 2);
        let a = &m["0199aaaa-1111-4222-8333-444455556666"];
        assert_eq!(a.name.as_deref(), Some("OpenTrends 发布"));
        assert_eq!(a.title.as_deref(), Some("检查 一下 发布流程"));
        assert_eq!(a.project.as_deref(), Some("OpenTrends"));
        assert_eq!(m["0199bbbb-1111-4222-8333-444455556666"], CodexThread::default());
        assert!(parse_codex_threads("").is_empty(), "no rows prints nothing");
    }

    #[test]
    fn titles_skip_boilerplate() {
        // a leading tag is stripped, what follows is the title
        assert_eq!(clean_title("<system-reminder>\nfoo"), Some("foo".into()));
        assert_eq!(clean_title("Caveat: the messages below"), None);
        assert_eq!(clean_title("  帮我  看看 这个 "), Some("帮我 看看 这个".into()));
        // what Codex puts in front of the first prompt
        assert_eq!(clean_title("Any earlier instruction enabling proactive multi-agent work is void"), None);
        assert_eq!(clean_title("Here is a list of plugins that are available but not installed"), None);
        assert_eq!(clean_title("Deferred tool namespaces: - mcp__artifact_sessions"), None);
        assert_eq!(clean_title("Added deferred tool namespaces: - mcp__dispatch"), None);
        assert_eq!(clean_title("<in…119 tokens truncated…可以开始了"), None);
    }
}
