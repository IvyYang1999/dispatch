//! A message that waits for its session to be opened. A Claude Code session that is not
//! running takes nothing until it runs again; a Codex thread takes what is in its queue only
//! while some Codex window (or terminal) has it open. On the machine where the session lives,
//! the app opens such a session itself, at once: in its own app when it has one (the Codex
//! app, the Claude app), else in a Terminal window behind the user's windows. It asks the user
//! only when it cannot: the setting says remind only, opening failed, or the session was opened
//! and still has not taken the message.
//!
//! Opening never starts a second copy of a session: a Claude Code session counts only when no
//! process runs it, and a Codex thread whose log changed in the last two minutes is being run
//! somewhere and is left alone (it takes the message when its turn ends).

use crate::model::{short_id, Harness, Session};
use crate::store::{self, Letter, Run, Store};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

/// How long a message waits before it counts as waiting for its session to be opened. A Codex
/// thread that is open and idle takes its queue within about ten seconds; a Claude Code session
/// that is not running is known at once.
pub const AFTER_SECS: i64 = 15;
const AFTER_SECS_CLAUDE: i64 = 3;
/// Opened for its message this long ago and still has not taken it: the user is asked.
pub const STUCK_SECS: i64 = 120;
/// A Codex thread whose log changed this recently is open and working somewhere.
const BUSY_SECS: u64 = 120;

/// What to do about a message waiting for its session: `auto` (remind, and open the session
/// when nobody is at this machine), `ask` (remind only), `off`.
pub fn mode() -> &'static str {
    match crate::settings::get("wake").and_then(|v| v.as_str().map(str::to_string)).as_deref() {
        Some("ask") => "ask",
        Some("off") => "off",
        _ => "auto",
    }
}

pub fn set_mode(mode: &str) -> anyhow::Result<&'static str> {
    let m = match mode {
        "auto" => "auto",
        "ask" => "ask",
        "off" => "off",
        other => return Err(anyhow::anyhow!("wake mode is auto, ask or off, not \"{other}\"")),
    };
    crate::settings::set("wake", json!(m))?;
    Ok(m)
}

fn age_secs(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| (chrono::Local::now().fixed_offset() - t).num_seconds()).unwrap_or(0)
}

/// When the copy was handed to Codex's queue, if it was.
fn queued_at(runs: &[Run]) -> Option<&str> {
    runs.iter().rev().find(|r| r.action == "codex" && r.result == "queued").map(|r| r.at.as_str())
}

/// Whether this copy was already opened for once (by the user from the reminder, or by the app).
pub fn woken(runs: &[Run]) -> bool {
    runs.iter().any(|r| r.action == "wake" && r.result == "opened")
}

/// When it was last opened for this copy.
fn woken_at(runs: &[Run]) -> Option<&str> {
    runs.iter().rev().find(|r| r.action == "wake" && r.result == "opened").map(|r| r.at.as_str())
}

/// Whether some Codex process is running this thread now: its log changed in the last two minutes.
pub fn codex_busy(thread_id: &str) -> bool {
    crate::codex::rollout_path(thread_id)
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < Duration::from_secs(BUSY_SECS))
        .unwrap_or(false)
}

/// Still worth opening a session for: within the copy's deadline. An old copy whose receipt
/// cannot be found any more is not news, and no session is opened for it.
fn current(l: &Letter) -> bool {
    chrono::Local::now().naive_local() <= l.deadline_at()
}

/// What the Codex thread did with this copy, and it never goes backwards: the first time the
/// thread is seen to have taken it, and the time its turn is seen finished, are written into the
/// copy's log. A thread's log moves on and old lines fall out of what is read, so "not found
/// now" must not undo "taken then": that sent reminders for messages read hours before.
pub fn codex_receipt(store: &Store, l: &Letter, runs: &[Run]) -> crate::codex::Receipt {
    use crate::codex::Receipt;
    let noted = |result: &str| runs.iter().any(|r| r.action == "receipt" && r.result == result);
    if noted("completed") {
        return Receipt::Completed;
    }
    let taken = noted("taken");
    match crate::codex::receipt(&l.to, &short_id(&l.id)) {
        Receipt::Completed => {
            let _ = store.append_run(&l.id, "receipt", "completed", "");
            Receipt::Completed
        }
        // taken once, and its log no longer shows it (the file is gone or was replaced): long over
        Receipt::NotFound if taken => Receipt::Completed,
        Receipt::NotFound => Receipt::NotFound,
        r => {
            if !taken {
                let _ = store.append_run(&l.id, "receipt", "taken", "");
            }
            r
        }
    }
}

/// A Codex copy in the thread's queue for a while with nobody running the thread: the part of
/// "is it waiting" that needs no receipt.
pub fn codex_left_alone(l: &Letter, runs: &[Run]) -> bool {
    l.status == store::DELIVERED_CODEX && current(l) && queued_at(runs).map(|at| age_secs(at) >= AFTER_SECS).unwrap_or(false) && !codex_busy(&l.to)
}

/// A Codex copy sitting in the thread's queue, untaken for a while, with nobody running the thread.
pub fn codex_waiting(store: &Store, l: &Letter, runs: &[Run]) -> bool {
    codex_left_alone(l, runs) && matches!(codex_receipt(store, l, runs), crate::codex::Receipt::NotFound)
}

/// The link that opens this session in its own app, when there is one: any Codex thread opens in
/// the Codex app; a Claude Code session opens in the Claude app when it lives there. A Claude Code
/// session started in a terminal has none (it is resumed in a terminal, which the user starts).
pub fn open_url(s: &Session) -> Option<String> {
    match s.harness {
        Harness::Codex => Some(format!("codex://threads/{}", s.id)),
        Harness::Claude => s.host_session_id.as_deref().filter(|_| s.desktop).map(|h| format!("claude://code/continue?session={h}")),
        _ => None,
    }
}

/// The command that resumes this session in a terminal, and the folder to run it in: what is
/// left when it has no app to open in (or that app is missing). None when its folder is gone.
pub fn open_command(s: &Session) -> Option<(String, Option<String>)> {
    let cwd = s.cwd.clone().filter(|c| std::path::Path::new(c).is_dir());
    match s.harness {
        Harness::Claude => cwd.map(|c| (format!("claude --resume {}", s.id), Some(c))),
        Harness::Codex => Some((format!("codex resume {}", s.id), cwd)),
        _ => None,
    }
}

/// The copies on this machine waiting for their session to be opened, oldest first.
pub fn waiting(store: &Store) -> Vec<Value> {
    let Ok(letters) = store.list(None, false) else { return vec![] };
    let candidates: Vec<Letter> = letters.into_iter().filter(|l| l.to_host.is_none() && (l.status == store::DELIVERED_CODEX || l.status == store::UNREAD) && current(l)).collect();
    if candidates.is_empty() {
        return vec![];
    }
    let local = crate::discover::local();
    let mut out = vec![];
    for l in candidates {
        let Some(s) = local.iter().find(|s| s.id.eq_ignore_ascii_case(&l.to)).cloned().or_else(|| crate::discover::lookup(&l.to)) else { continue };
        let runs = store.runs(&l.id);
        let since = match s.harness {
            Harness::Codex if codex_waiting(store, &l, &runs) => queued_at(&runs).map(str::to_string),
            Harness::Claude if l.status == store::UNREAD && !s.running => runs.first().map(|r| r.at.clone()).filter(|at| age_secs(at) >= AFTER_SECS_CLAUDE),
            _ => None,
        };
        let Some(since) = since else { continue };
        out.push(json!({
            "letter_id": l.id,
            "from_name": l.from_name,
            "from_host_name": l.from_host_name,
            "since": since,
            "woken": woken(&runs),
            // opened for it a while ago and still waiting: something is off, the user is asked
            "stuck": woken_at(&runs).map(|at| age_secs(at) >= STUCK_SECS).unwrap_or(false),
            "session": {
                "id": s.id,
                "name": s.display_name(),
                "app_name": s.name,
                "harness": s.harness.label(),
                "desktop": s.desktop,
                "avatar": crate::contacts::for_session(&s.id, None).and_then(|c| c.avatar),
                "url": open_url(&s),
                "command": open_command(&s).map(|(c, _)| c),
                "cwd": open_command(&s).and_then(|(_, d)| d),
            },
        }));
    }
    out.sort_by(|a, b| a["since"].as_str().cmp(&b["since"].as_str()));
    out
}

/// Note on every copy waiting for this session that it was opened for them (`by`: `user` from
/// the reminder, `auto` when nobody was at the machine).
pub fn opened(store: &Store, session_id: &str, by: &str) -> usize {
    let mut n = 0;
    for w in waiting(store) {
        if w["session"]["id"].as_str().map(|id| id.eq_ignore_ascii_case(session_id)).unwrap_or(false) {
            if let Some(id) = w["letter_id"].as_str() {
                if store.append_run(id, "wake", "opened", by).is_ok() {
                    n += 1;
                }
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(action: &str, result: &str, secs_ago: i64) -> Run {
        let at = (chrono::Local::now() - chrono::Duration::seconds(secs_ago)).to_rfc3339();
        Run { at, action: action.into(), result: result.into(), detail: String::new() }
    }

    /// The reminder that would not go away: a message the thread took hours ago, whose line has
    /// since left what is read of the log. Once taken is written down, it is never waiting again.
    #[test]
    fn a_message_once_taken_never_waits_again() {
        let dir = std::env::temp_dir().join(format!("dispatch-taken-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let mut l = Letter {
            id: uuid::Uuid::new_v4().to_string(),
            dispatch_id: uuid::Uuid::new_v4().to_string(),
            from: "aaaaaaaa-1111-4222-8333-444455556666".into(),
            from_name: "A".into(),
            // a thread with no log on this machine: every lookup says "not found"
            to: uuid::Uuid::new_v4().to_string(),
            to_name: "B".into(),
            urgency: "普通".into(),
            wake: true,
            status: store::DELIVERED_CODEX.into(),
            created: store::now_text(),
            deadline: None,
            to_harness: Some("codex".into()),
            from_host: None,
            from_host_name: None,
            to_host: None,
            to_host_name: None,
            from_harness: Some("claude".into()),
            signature: None,
            body: "hello".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        let queued = vec![run("codex", "queued", 60)];
        assert!(codex_waiting(&s, &l, &queued), "queued a minute ago and never taken: it waits");
        let mut taken = queued.clone();
        taken.push(run("receipt", "taken", 30));
        assert_eq!(codex_receipt(&s, &l, &taken), crate::codex::Receipt::Completed);
        assert!(!codex_waiting(&s, &l, &taken), "taken once: not waiting, whatever the log shows now");
        let mut done = queued.clone();
        done.push(run("receipt", "completed", 30));
        assert!(!codex_waiting(&s, &l, &done));
        assert!(s.runs(&l.id).iter().all(|r| r.action != "receipt"), "a lookup that finds nothing writes nothing");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_session_opens_in_its_own_app_when_it_has_one() {
        let mut s = Session {
            harness: Harness::Codex,
            id: "019a0000-1111-7222-8333-444455556666".into(),
            short_id: "019a0000".into(),
            name: None,
            cwd: None,
            title: None,
            last_active: 0,
            running: false,
            desktop: false,
            host_session_id: None,
            status: None,
            transcript: None,
            host: None,
            project: None,
            app_name: None,
        };
        assert_eq!(open_url(&s).as_deref(), Some("codex://threads/019a0000-1111-7222-8333-444455556666"));
        s.harness = Harness::Claude;
        assert_eq!(open_url(&s), None, "a terminal session has no link");
        s.desktop = true;
        s.host_session_id = Some("local_abc".into());
        assert_eq!(open_url(&s).as_deref(), Some("claude://code/continue?session=local_abc"));
    }

    #[test]
    fn opened_once_is_remembered_and_a_fresh_queue_does_not_count_yet() {
        assert!(!woken(&[run("codex", "queued", 30)]));
        assert!(woken(&[run("codex", "queued", 30), run("wake", "opened", 5)]));
        assert_eq!(queued_at(&[run("store", "written", 40), run("codex", "queued", 30)]).map(|at| age_secs(at) >= AFTER_SECS), Some(true));
        assert_eq!(queued_at(&[run("codex", "queued", 3)]).map(|at| age_secs(at) >= AFTER_SECS), Some(false));
    }

    #[test]
    fn the_mode_is_auto_unless_set() {
        let dir = std::env::temp_dir().join(format!("dispatch-wake-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        assert_eq!(mode(), "auto");
        set_mode("ask").unwrap();
        assert_eq!(mode(), "ask");
        assert!(set_mode("sometimes").is_err());
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
