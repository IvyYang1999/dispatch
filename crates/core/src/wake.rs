//! A message that waits for its session to be opened. A Claude Code session that is not
//! running takes nothing until it runs again; a Codex thread takes what is in its queue only
//! while some Codex window (or terminal) has it open. On the machine where the session lives,
//! the app takes care of it itself, at once. A Codex thread nobody has open is run in the
//! background by a Codex of the app's own (`codex::run_unopened`), which takes its queue as
//! opening it would, with no window. Any other session is opened in its own app when it has one
//! (the Claude app), else in a Terminal window behind the user's windows. It asks the user only
//! when it cannot: the setting says remind only, opening failed, or the session was opened and
//! still has not taken the message.
//!
//! The user can take a Codex thread back from the app while it runs in the background, or keep
//! the app off a session altogether (`set_handed_back`): the run stops at once and frees the
//! thread, and from then on the app only reminds about messages for that session, until the user
//! says it may open it again.
//!
//! Neither ever starts a second copy of a session: a Claude Code session counts only when no
//! process runs it, and a Codex thread that some Codex holds (its lock says so) is left to it:
//! it takes the message when its turn ends.

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

/// The sessions the user took back from the app (`wake_handed_back` in the settings): the app
/// does not run them in the background or open them by itself until the user says so again.
/// Their messages stay where they are and the user is reminded of them.
pub fn handed_back_list() -> Vec<String> {
    crate::settings::get("wake_handed_back").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).collect())).unwrap_or_default()
}

pub fn handed_back(session_id: &str) -> bool {
    handed_back_list().iter().any(|id| id.eq_ignore_ascii_case(session_id))
}

/// Take a session back from the app (`on`), or let the app open it by itself again.
pub fn set_handed_back(session_id: &str, on: bool) -> anyhow::Result<()> {
    let id = session_id.trim().to_lowercase();
    if id.is_empty() {
        anyhow::bail!("no session given");
    }
    let mut list = handed_back_list();
    list.retain(|x| *x != id);
    if on {
        list.push(id);
    }
    crate::settings::set("wake_handed_back", json!(list))?;
    Ok(())
}

fn age_secs(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| (chrono::Local::now().fixed_offset() - t).num_seconds()).unwrap_or(0)
}

/// When the copy was handed to Codex's queue, if it was.
fn queued_at(runs: &[Run]) -> Option<&str> {
    runs.iter().rev().find(|r| r.action == "codex" && r.result == "queued").map(|r| r.at.as_str())
}

/// The last thing noted about opening the session for this copy: `opened`, or `released` when
/// the user took the session back from the app or let the app have it again after that.
fn last_wake(runs: &[Run]) -> Option<&Run> {
    runs.iter().rev().find(|r| r.action == "wake" && matches!(r.result.as_str(), "opened" | "released"))
}

/// Whether this copy was already opened for once (by the user from the reminder, or by the app)
/// and that still stands. Taking the session back, or letting the app open it again, starts
/// over: a copy the app had opened the session for, and that was still untaken when the user
/// took the session back, is opened for again once the user says so.
pub fn woken(runs: &[Run]) -> bool {
    last_wake(runs).map(|r| r.result == "opened").unwrap_or(false)
}

/// When it was last opened for this copy, while that still stands.
fn woken_at(runs: &[Run]) -> Option<&str> {
    last_wake(runs).filter(|r| r.result == "opened").map(|r| r.at.as_str())
}

/// Take a session back from the app (`on`), or let the app open it by itself again, and note
/// it on every copy still pending for the session, whether or not it counts as waiting this
/// very moment (a thread someone has open is not "waiting", and still has these copies in its
/// queue). Returns how many copies were noted.
pub fn hand_back(store: &Store, session_id: &str, on: bool) -> anyhow::Result<usize> {
    set_handed_back(session_id, on)?;
    let id = session_id.trim();
    let mut n = 0;
    for l in store.list(None, false)?.into_iter().filter(|l| l.to_host.is_none() && l.to.eq_ignore_ascii_case(id) && (l.status == store::DELIVERED_CODEX || l.status == store::UNREAD) && current(l)) {
        if store.append_run(&l.id, "wake", "released", if on { "handed back by the user" } else { "the user lets Dispatch open it again" }).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

/// Whether some Codex process has this thread now: it holds the thread's lock (when that can be
/// told), else its log changed in the last two minutes.
pub fn codex_busy(thread_id: &str) -> bool {
    if let Some(open) = crate::codex::thread_open(thread_id) {
        return open;
    }
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
    // a turn that was stopped does not start again: written down once, like one that finished
    if noted("stopped") {
        return Receipt::Stopped;
    }
    let taken = noted("taken");
    match crate::codex::receipt(&l.to, &short_id(&l.id)) {
        Receipt::Completed => {
            let _ = store.append_run(&l.id, "receipt", "completed", "");
            Receipt::Completed
        }
        Receipt::Stopped => {
            let _ = store.append_run(&l.id, "receipt", "stopped", "");
            Receipt::Stopped
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
        // no `claude` program here: there is nothing a terminal could run
        Harness::Claude => cwd.and_then(|c| Some((crate::claude::resume_command(&s.id)?, Some(c)))),
        Harness::Codex => Some((format!("codex resume {}", s.id), cwd)),
        _ => None,
    }
}

/// The copies on this machine waiting for their session to be opened, oldest first.
pub fn waiting(store: &Store) -> Vec<Value> {
    let Ok(letters) = store.list(None, false) else { return vec![] };
    // a quiet copy waits for its session to speak: no session is opened for it
    let candidates: Vec<Letter> = letters.into_iter().filter(|l| l.wake && l.to_host.is_none() && (l.status == store::DELIVERED_CODEX || l.status == store::UNREAD) && current(l)).collect();
    if candidates.is_empty() {
        return vec![];
    }
    let local = crate::discover::local();
    let handed = handed_back_list();
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
            // the user took this session back: the app only reminds
            "handed": handed.iter().any(|id| id.eq_ignore_ascii_case(&s.id)),
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
            review: None,
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
        // one whose turn was stopped was taken too: it is not waiting to be opened for, and it
        // stays stopped although the thread's log (none here) no longer shows it
        let mut stopped = taken.clone();
        stopped.push(run("receipt", "stopped", 20));
        assert_eq!(codex_receipt(&s, &l, &stopped), crate::codex::Receipt::Stopped);
        assert!(!codex_waiting(&s, &l, &stopped));
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
    fn a_session_taken_back_stays_taken_back_until_the_user_says_otherwise() {
        let dir = std::env::temp_dir().join(format!("dispatch-handed-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "019A0000-1111-7222-8333-444455556666";
        let b = "019b0000-1111-7222-8333-444455556666";
        assert!(!handed_back(a));
        set_handed_back(a, true).unwrap();
        set_handed_back(a, true).unwrap();
        set_handed_back(b, true).unwrap();
        assert!(handed_back(&a.to_lowercase()) && handed_back(a) && handed_back(b));
        assert_eq!(handed_back_list().len(), 2, "once each, whatever the case");
        // it is kept in the settings file: a restart of the app does not undo it
        assert!(std::fs::read_to_string(crate::settings::path()).unwrap().contains("wake_handed_back"));
        set_handed_back(a, false).unwrap();
        assert!(!handed_back(a) && handed_back(b));
        assert!(set_handed_back("  ", true).is_err());
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What the main Mac found: two messages queued, the app opens the thread in the background
    /// (which notes `opened` on both), the user takes the thread back while the first is being
    /// worked on, then lets Dispatch run it again without any new message. The second message
    /// was opened for once and never taken: it must be opened for again, not skipped for ever.
    #[test]
    fn a_message_opened_for_and_then_handed_back_is_opened_for_again() {
        let dir = std::env::temp_dir().join(format!("dispatch-resume-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let s = Store::open();
        let thread = uuid::Uuid::new_v4().to_string();
        let letter = |to: &str, status: &str| {
            let mut l = Letter {
                id: uuid::Uuid::new_v4().to_string(),
                dispatch_id: uuid::Uuid::new_v4().to_string(),
                from: "aaaaaaaa-1111-4222-8333-444455556666".into(),
                from_name: "A".into(),
                to: to.to_string(),
                to_name: "B".into(),
                urgency: "普通".into(),
                wake: true,
                status: status.into(),
                created: store::now_text(),
                deadline: None,
                to_harness: Some("codex".into()),
                from_host: None,
                from_host_name: None,
                to_host: None,
                to_host_name: None,
                from_harness: Some("claude".into()),
                signature: None,
                review: None,
                body: "hello".into(),
                path: String::new(),
            };
            s.write_letter(&mut l).unwrap();
            l
        };
        let (first, second) = (letter(&thread, store::DELIVERED_CODEX), letter(&thread, store::DELIVERED_CODEX));
        // one that is over, and one for another thread: neither is touched
        let done = letter(&thread, store::REPLIED);
        let other = letter(&uuid::Uuid::new_v4().to_string(), store::DELIVERED_CODEX);
        for l in [&first, &second, &done, &other] {
            s.append_run(&l.id, "codex", "queued", "").unwrap();
            s.append_run(&l.id, "wake", "opened", "auto, background").unwrap();
        }
        assert!(woken(&s.runs(&second.id)), "the background run was started for it");

        // the user takes the thread back: in upper case, as an id may come
        assert_eq!(hand_back(&s, &thread.to_uppercase(), true).unwrap(), 2, "both copies still pending for this thread");
        assert!(handed_back(&thread));
        assert!(!woken(&s.runs(&second.id)) && woken_at(&s.runs(&second.id)).is_none());
        assert!(woken(&s.runs(&done.id)) && woken(&s.runs(&other.id)), "a copy that is over, or for another thread, is left as it was");

        // and lets Dispatch run it again, with no new message: the old one is not skipped
        assert_eq!(hand_back(&s, &thread, false).unwrap(), 2);
        assert!(!handed_back(&thread), "not kept off any more");
        assert!(!woken(&s.runs(&second.id)), "and not counted as opened for: the two things the watcher looks at");
        let log: Vec<String> = s.runs(&second.id).iter().filter(|r| r.action == "wake").map(|r| format!("{} {}", r.result, r.detail)).collect();
        assert_eq!(log, ["opened auto, background", "released handed back by the user", "released the user lets Dispatch open it again"]);

        // opened for again: that counts from here on, as before
        s.append_run(&second.id, "wake", "opened", "auto, background").unwrap();
        assert!(woken(&s.runs(&second.id)) && woken_at(&s.runs(&second.id)).is_some());
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Against a real store (`DISPATCH_HOME`) and a real Codex thread (`DISPATCH_PROBE_THREAD`)
    /// with a message in its queue that the app already opened the thread for: taking the
    /// thread back and letting the app have it again leaves that message neither kept off nor
    /// counted as opened for, which is what the app's watcher asks before it runs a thread.
    #[test]
    #[ignore]
    fn probe_hand_back_and_resume() {
        let id = std::env::var("DISPATCH_PROBE_THREAD").unwrap();
        let s = Store::open();
        let of_thread = |s: &Store| -> Vec<(String, bool, bool)> {
            waiting(s).into_iter().filter(|w| w["session"]["id"].as_str().map(|x| x.eq_ignore_ascii_case(&id)).unwrap_or(false)).map(|w| (short_id(w["letter_id"].as_str().unwrap_or("")), w["handed"] == true, w["woken"] == true)).collect()
        };
        let before = of_thread(&s);
        eprintln!("before (letter, handed, woken): {before:?}");
        assert!(before.iter().any(|(_, _, woken)| *woken), "a waiting copy the thread was already opened for");
        eprintln!("handed back, copies noted: {}", hand_back(&s, &id, true).unwrap());
        let held = of_thread(&s);
        eprintln!("handed back: {held:?}");
        assert!(held.iter().all(|(_, handed, _)| *handed));
        eprintln!("let go again, copies noted: {}", hand_back(&s, &id, false).unwrap());
        let after = of_thread(&s);
        eprintln!("after: {after:?}");
        assert_eq!(after.len(), before.len());
        assert!(after.iter().all(|(_, handed, woken)| !*handed && !*woken), "the watcher runs the thread for these again");
    }

    /// Told to stop before it began, a background run does not start a Codex at all.
    #[test]
    fn a_run_told_to_stop_before_it_starts_never_touches_the_thread() {
        let stop = std::sync::atomic::AtomicBool::new(true);
        let out = crate::codex::run_unopened_until("019c0000-1111-7222-8333-444455556666", Duration::from_secs(60), &stop).unwrap();
        assert!(out.handed_back && !out.interrupted && out.turns == 0);
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
