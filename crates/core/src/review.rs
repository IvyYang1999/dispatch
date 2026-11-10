//! Review before sending: a message a session sends is held on this machine until the user
//! says send in Dispatch's review window. The `review` setting decides when: `new` (the first
//! time a session writes to a recipient; the default), `all` (every message) or `off`.
//! Replies, and what the user sends from the app, are never held.
//!
//! Every step goes into the copy's run log under `review`, so the sender follows it with
//! `dispatch_status`: `asked` (held), `shown` (the window is up), then `approved`, `trusted`
//! (approved, and this sender may write to this recipient without asking again), `declined`,
//! `withdrawn` (the sender cancelled it) or `expired` (its deadline passed first).
//!
//! The app finds held copies through `review/`, one empty file per copy named by its letter
//! id, so it does not read every letter each second; and it touches `review/.app` while it
//! runs, so a sender can tell whether the window can come up at all.

use crate::i18n::t;
use crate::model::{short_id, Harness, Session};
use crate::send;
use crate::store::{self, Run, Store};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

pub const OFF: &str = "off";
pub const NEW: &str = "new";
pub const ALL: &str = "all";

/// The `review` setting: `new` unless set.
pub fn mode() -> &'static str {
    match crate::settings::get("review").and_then(|v| v.as_str().map(str::to_string)).as_deref() {
        Some("off") => OFF,
        Some("all") => ALL,
        _ => NEW,
    }
}

pub fn set_mode(m: &str) -> Result<&'static str> {
    let m = match m {
        "off" => OFF,
        "all" => ALL,
        "new" => NEW,
        other => return Err(anyhow!("review is off, new or all, not \"{other}\"")),
    };
    crate::settings::set("review", json!(m))?;
    Ok(m)
}

fn dir(store: &Store) -> PathBuf {
    store.root().join("review")
}

fn alive_file(store: &Store) -> PathBuf {
    dir(store).join(".app")
}

/// Put a held copy where the app looks.
pub fn enqueue(store: &Store, letter_id: &str) -> Result<()> {
    fs::create_dir_all(dir(store))?;
    fs::write(dir(store).join(letter_id), "")?;
    Ok(())
}

/// Take it out again (decided, withdrawn or expired).
pub fn dequeue(store: &Store, letter_id: &str) {
    let _ = fs::remove_file(dir(store).join(letter_id));
}

/// The held copies' letter ids, oldest first.
pub fn queued(store: &Store) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir(store)) else { return vec![] };
    let mut ids: Vec<(std::time::SystemTime, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            Some((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), name))
        })
        .collect();
    ids.sort();
    ids.into_iter().map(|(_, id)| id).collect()
}

/// The app says it is up (it does this every second or so while it runs).
pub fn touch_app(store: &Store) {
    let _ = fs::create_dir_all(dir(store));
    let _ = fs::write(alive_file(store), store::now_text());
}

/// Whether the app touched its file in the last few seconds: the review window can come up.
pub fn app_alive(store: &Store) -> bool {
    fs::metadata(alive_file(store)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|age| age.as_secs() < 8).unwrap_or(false)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    trusted: Vec<Pair>,
}

/// A sender the user let write to a recipient without asking again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pair {
    pub from: String,
    pub to: String,
    /// The recipient's machine, when it is another one.
    #[serde(default)]
    pub to_host: Option<String>,
    #[serde(default)]
    pub at: String,
}

fn book_path(store: &Store) -> PathBuf {
    store.root().join("review.json")
}

fn book(store: &Store) -> Book {
    fs::read_to_string(book_path(store)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_book(store: &Store, b: &Book) -> Result<()> {
    fs::create_dir_all(store.root())?;
    let tmp = book_path(store).with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(b)? + "\n")?;
    fs::rename(tmp, book_path(store))?;
    Ok(())
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

pub fn trusted(store: &Store, from: &str, to: &str, to_host: Option<&str>) -> bool {
    book(store).trusted.iter().any(|p| same(&p.from, from) && same(&p.to, to) && p.to_host.as_deref() == to_host)
}

fn trust(store: &Store, from: &str, to: &str, to_host: Option<&str>) -> Result<()> {
    if trusted(store, from, to, to_host) {
        return Ok(());
    }
    let mut b = book(store);
    b.trusted.push(Pair { from: from.to_lowercase(), to: to.to_lowercase(), to_host: to_host.map(str::to_string), at: store::now_text() });
    save_book(store, &b)
}

/// How many pairs may write without asking.
pub fn trusted_count(store: &Store) -> usize {
    book(store).trusted.len()
}

/// Forget every pair: each asks again the next time.
pub fn forget_all(store: &Store) -> Result<usize> {
    let n = trusted_count(store);
    save_book(store, &Book::default())?;
    Ok(n)
}

/// Whether the app has ever run with this store: it leaves its file behind when it quits.
/// A machine with only the command line has no window to review in.
pub fn app_installed(store: &Store) -> bool {
    alive_file(store).exists()
}

/// Whether a message from `from` to `to` waits for the user. Not when the user sent it from
/// the app, not when it goes to a person (the feed), not when the app never ran here (no one
/// could review it), not when review is off; under `new`, not when the user already let this
/// sender write to this recipient.
pub fn needed(store: &Store, from: &str, to: &Session) -> bool {
    if from == "app" || to.harness == Harness::Human || send::is_person(&to.id) || !app_installed(store) {
        return false;
    }
    match mode() {
        OFF => false,
        ALL => true,
        _ => !trusted(store, from, &to.id, to.host.as_ref().map(|h| h.id.as_str())),
    }
}

/// The review steps of a copy, from its run log: (step, when) in order. Empty when it was
/// never held.
fn steps(runs: &[Run]) -> Vec<(String, String)> {
    runs.iter().filter(|r| r.action == "review").map(|r| (r.result.clone(), r.at.clone())).collect()
}

/// Where the review of a copy stands: `asked`, `shown`, `approved`, `trusted`, `declined`,
/// `withdrawn` or `expired`, with when; None when it was never held.
pub fn state(runs: &[Run]) -> Option<(String, String)> {
    steps(runs).into_iter().last()
}

/// The review as the sender sees it: where it stands, whether the window can come up, and
/// every step with its time.
pub fn summary(store: &Store, runs: &[Run]) -> Option<Value> {
    let all = steps(runs);
    let (state, at) = all.last().cloned()?;
    let waiting = matches!(state.as_str(), "asked" | "shown");
    Some(json!({
        "state": state,
        "at": at,
        "app_running": if waiting { Some(app_alive(store)) } else { None },
        "steps": all.iter().map(|(s, at)| json!({ "step": s, "at": at })).collect::<Vec<_>>(),
    }))
}

/// The stage key of a copy still held for review.
pub fn waiting_stage(store: &Store, runs: &[Run]) -> &'static str {
    match state(runs).map(|(s, _)| s).as_deref() {
        Some("shown") => "stage.review_shown",
        _ if !app_alive(store) => "stage.review_no_app",
        _ => "stage.review_asked",
    }
}

/// The window put this copy in front of the user (logged once).
pub fn mark_shown(store: &Store, letter_id: &str) -> Result<()> {
    let runs = store.runs(letter_id);
    if state(&runs).map(|(s, _)| s == "asked").unwrap_or(false) {
        store.append_run(letter_id, "review", "shown", "")?;
    }
    Ok(())
}

/// What the user decided in the review window: `send`, `trust` (send, and don't ask again for
/// this sender and recipient) or `decline`. Only the app calls this.
pub fn decide(store: &Store, letter_id: &str, decision: &str) -> Result<Value> {
    let mut l = store.find(letter_id)?.filter(|l| l.id == letter_id).ok_or_else(|| anyhow!("no message {letter_id}"))?;
    if l.status != store::REVIEW {
        dequeue(store, letter_id);
        return Err(anyhow!("{}", t("review.not_waiting").replace("{status}", crate::i18n::status_label(&l.status))));
    }
    dequeue(store, letter_id);
    match decision {
        "decline" => {
            store.set_status(&mut l, store::CANCELLED)?;
            store.append_run(&l.id, "review", "declined", "by the user")?;
            Ok(json!({ "letter_id": l.id, "decision": "declined", "text": t("review.declined_text") }))
        }
        "send" | "trust" => {
            if decision == "trust" {
                trust(store, &l.from, &l.to, l.to_host.as_deref())?;
            }
            store.set_status(&mut l, store::UNREAD)?;
            store.append_run(&l.id, "review", if decision == "trust" { "trusted" } else { "approved" }, "by the user")?;
            // the wait for delivery starts now, not when the agent asked
            let fresh = store::deadline_text(&store::now_text(), store::default_wait(&l.urgency));
            store.set_deadline(&mut l, &fresh)?;
            let to = send::target_of(&l);
            let rep = send::deliver(store, &to, &mut l, true, false)?;
            Ok(json!({ "letter_id": l.id, "decision": if decision == "trust" { "trusted" } else { "approved" }, "route": rep.route, "detail": rep.detail, "text": rep.detail }))
        }
        other => Err(anyhow!("a review is decided with send, trust or decline, not \"{other}\"")),
    }
}

/// A held copy past its deadline is not sent: it leaves the queue, marked expired.
pub fn expire(store: &Store, l: &mut store::Letter, now: chrono::NaiveDateTime) -> Result<bool> {
    if l.status != store::REVIEW || now <= l.deadline_at() {
        return Ok(false);
    }
    dequeue(store, &l.id);
    store.set_status(l, store::CANCELLED)?;
    store.append_run(&l.id, "review", "expired", &t("send.deadline_passed").replace("{t}", &store::time_text(l.deadline_at())))?;
    Ok(true)
}

/// What the review window shows for one held copy: the message, its sender, the recipient as
/// it stands now (running or not, mode, client, how it would get it) and its last few rounds.
/// The last thing the person typed to a session (not a message Dispatch handed over), else the
/// last message it got at all: the line that tells which session this is.
fn last_query(turns: &[crate::transcript::Turn]) -> Option<crate::transcript::Turn> {
    // Claude Code opens a compacted conversation with a summary in the person's place: not a question
    let asked = |t: &&crate::transcript::Turn| t.role == "user" && !t.text.trim().is_empty() && !t.text.starts_with("This session is being continued from a previous conversation");
    let mut q = turns.iter().rev().filter(asked).find(|t| !t.via_dispatch).or_else(|| turns.iter().rev().find(asked)).cloned()?;
    if q.via_dispatch {
        q.text = envelope_body(&q.text);
    }
    Some(q)
}

/// The message inside a Dispatch envelope: after the header line, before the closing note.
fn envelope_body(text: &str) -> String {
    let body = text.split_once("\n\n").map(|(_, b)| b).unwrap_or(text);
    let end = ["\n\n（这是其他 Agent 发来的消息", "\n\n(This is a message from another agent"].iter().filter_map(|m| body.rfind(m)).min().unwrap_or(body.len());
    body[..end].trim().to_string()
}

pub fn info(store: &Store, letter_id: &str) -> Result<Value> {
    let l = store.find(letter_id)?.filter(|l| l.id == letter_id).ok_or_else(|| anyhow!("no message {letter_id}"))?;
    let address = match &l.to_host {
        Some(h) => format!("{h}/{}", l.to),
        None => l.to.clone(),
    };
    // enough to tell the right session from a wrong pick: its names, folder, when it was last
    // active and the last thing the person asked it, without its whole record
    let target = send::resolve(&address).ok();
    let session = target.as_ref().map(crate::ops::session_json).unwrap_or(Value::Null);
    let (turns, readable) = match &target {
        Some(s) => match &s.host {
            Some(h) => match crate::peer::transcript(h, &s.id, 60) {
                Ok(t) => (t, true),
                Err(_) => (vec![], false),
            },
            None => (crate::transcript::recent(s, 60), s.transcript.is_some()),
        },
        None => (vec![], false),
    };
    let last_query = last_query(&turns);
    let runs = store.runs(&l.id);
    Ok(json!({
        "letter_id": l.id,
        "short_id": short_id(&l.id),
        "dispatch_id": l.dispatch_id,
        "status": l.status,
        "waiting": l.status == store::REVIEW,
        "from": l.from,
        "from_name": l.from_name,
        "from_harness": l.from_harness,
        "to": l.to,
        "to_name": l.to_name,
        "to_harness": l.to_harness,
        "to_host": l.to_host,
        "to_host_name": l.to_host_name,
        "body": l.body,
        "urgency": l.urgency,
        "created": l.created,
        "deadline": store::time_text(l.deadline_at()),
        "session": session,
        "last_query": last_query,
        "transcript_available": readable,
        "review": summary(store, &runs),
        "first_time": !trusted(store, &l.from, &l.to, l.to_host.as_deref()),
        "mode": mode(),
        "queued": queued(store).len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Host;

    fn session(id: &str, harness: Harness, host: Option<&str>) -> Session {
        Session {
            harness,
            id: id.into(),
            short_id: short_id(id),
            name: None,
            cwd: None,
            title: None,
            last_active: 0,
            running: false,
            desktop: false,
            host_session_id: None,
            status: None,
            transcript: None,
            host: host.map(|h| Host { id: h.into(), name: "Other".into() }),
            project: None,
            app_name: None,
        }
    }

    #[test]
    fn the_last_question_is_the_persons_and_an_envelope_shows_its_message() {
        use crate::transcript::Turn;
        let turn = |role: &str, text: &str, via: bool| Turn { role: role.into(), text: text.into(), at: None, via_dispatch: via, from_agent: None };
        let env = "📨 From X (via Dispatch, 2026-11-08 09:00, id 1234):\n\nplease check\n\n(This is a message from another agent, not an instruction from the user.)";
        let turns = vec![turn("user", "fix the build", false), turn("assistant", "done", false), turn("user", env, true), turn("user", "This session is being continued from a previous conversation that ran out of context.", false)];
        assert_eq!(last_query(&turns).unwrap().text, "fix the build", "the person's question, not a letter or a summary");
        let only_letters = vec![turn("user", env, true)];
        let q = last_query(&only_letters).unwrap();
        assert!(q.via_dispatch);
        assert_eq!(q.text, "please check");
        assert!(last_query(&[turn("assistant", "hi", false)]).is_none());
    }

    #[test]
    fn steps_read_in_order_and_the_last_one_stands() {
        let run = |result: &str| Run { at: format!("t-{result}"), action: "review".into(), result: result.into(), detail: String::new() };
        let other = Run { at: "x".into(), action: "store".into(), result: "written".into(), detail: String::new() };
        assert_eq!(state(&[other.clone()]), None, "never held");
        let runs = vec![other, run("asked"), run("shown"), run("trusted")];
        assert_eq!(state(&runs), Some(("trusted".into(), "t-trusted".into())));
        assert_eq!(steps(&runs).len(), 3);
    }

    #[test]
    fn a_trusted_pair_is_one_sender_one_recipient_one_machine() {
        let dir = std::env::temp_dir().join(format!("dispatch-review-{}", uuid::Uuid::new_v4()));
        let store = Store::at(dir.clone());
        let a = "aaaaaaaa-1111-4222-8333-444455556666";
        let b = session("bbbbbbbb-1111-4222-8333-444455556666", Harness::Claude, None);
        let b_there = session("bbbbbbbb-1111-4222-8333-444455556666", Harness::Claude, Some("peer-1"));
        assert!(!trusted(&store, a, &b.id, None));
        trust(&store, &a.to_uppercase(), &b.id, None).unwrap();
        assert!(trusted(&store, a, &b.id, None), "ids match whatever their case");
        assert!(!trusted(&store, a, &b_there.id, Some("peer-1")), "the same id on another machine is another recipient");
        assert!(!trusted(&store, &b.id, a, None), "and the other direction is another pair");
        trust(&store, a, &b.id, None).unwrap();
        assert_eq!(trusted_count(&store), 1, "trusting twice keeps one pair");
        assert_eq!(forget_all(&store).unwrap(), 1);
        assert!(!trusted(&store, a, &b.id, None));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_queue_lists_held_copies_and_not_the_app_file() {
        let dir = std::env::temp_dir().join(format!("dispatch-review-{}", uuid::Uuid::new_v4()));
        let store = Store::at(dir.clone());
        assert!(queued(&store).is_empty());
        assert!(!app_alive(&store), "no app file: the window cannot come up");
        enqueue(&store, "one").unwrap();
        touch_app(&store);
        assert_eq!(queued(&store), vec!["one".to_string()]);
        assert!(app_alive(&store));
        dequeue(&store, "one");
        assert!(queued(&store).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    /// The whole round on one machine: held, shown, trusted, then the same pair goes straight
    /// through; another is withdrawn, another runs out and asks again, another is declined.
    #[test]
    fn a_held_message_goes_out_only_when_the_user_says_so() {
        let dir = std::env::temp_dir().join(format!("dispatch-review-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let store = Store::open();
        let a = "aaaaaaaa-1111-4222-8333-444455556666";
        let to = |id: &str| session(id, Harness::Claude, None);
        let send_to = |id: &str| {
            send::send(&store, send::Request { to: to(id), from: a.into(), from_name: "A".into(), text: "hello".into(), urgency: "普通".into(), dispatch_id: None, wait: false, allow_resume: false, deadline: None, review: true }).unwrap()
        };
        let stage = |id: &str| {
            let v = crate::ops::status(&store, id).unwrap();
            (v["copies"][0]["kind"].as_str().unwrap().to_string(), v["copies"][0]["stage"].as_str().unwrap().to_string(), v["copies"][0]["review"]["state"].as_str().map(str::to_string))
        };
        let b = "bbbbbbbb-1111-4222-8333-444455556666";

        let e = send_to("eeeeeeee-1111-4222-8333-444455556666");
        assert_ne!(e.route, "review", "the app never ran here: no one could review it, so it goes out");
        touch_app(&store);
        let quit = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        fs::File::options().write(true).open(alive_file(&store)).unwrap().set_modified(quit).unwrap();
        assert!(app_installed(&store) && !app_alive(&store), "the app ran here once and is closed now");

        let rep = send_to(b);
        assert_eq!(rep.route, "review");
        assert!(rep.detail.contains(t("send.review_no_app")), "app closed: the sender is told the window cannot come up");
        assert_eq!(store.find(&rep.letter_id).unwrap().unwrap().status, store::REVIEW);
        assert_eq!(queued(&store), vec![rep.letter_id.clone()]);
        assert_eq!(stage(&rep.letter_id), ("review".into(), t("stage.review_no_app").into(), Some("asked".into())));
        touch_app(&store);
        assert_eq!(stage(&rep.letter_id).1, t("stage.review_asked"));
        mark_shown(&store, &rep.letter_id).unwrap();
        mark_shown(&store, &rep.letter_id).unwrap();
        assert_eq!(stage(&rep.letter_id), ("review".into(), t("stage.review_shown").into(), Some("shown".into())));
        assert_eq!(store.runs(&rep.letter_id).iter().filter(|r| r.result == "shown").count(), 1, "shown is logged once");
        assert!(crate::ops::retry(&store, &rep.letter_id).is_err(), "retrying a held copy does not approve it");

        let out = decide(&store, &rep.letter_id, "trust").unwrap();
        assert_eq!(out["decision"], "trusted");
        assert_ne!(store.find(&rep.letter_id).unwrap().unwrap().status, store::REVIEW);
        assert!(queued(&store).is_empty());
        assert_eq!(stage(&rep.letter_id).2.as_deref(), Some("trusted"));
        assert!(decide(&store, &rep.letter_id, "send").is_err(), "decided once");
        assert_ne!(send_to(b).route, "review", "the same pair goes straight through now");

        let c = send_to("cccccccc-1111-4222-8333-444455556666");
        assert_eq!(c.route, "review", "another recipient is asked about");
        crate::ops::cancel(&store, &c.letter_id, "A").unwrap();
        assert!(queued(&store).is_empty(), "withdrawn: out of the window");
        assert_eq!(stage(&c.letter_id).2.as_deref(), Some("withdrawn"));

        let d = send_to("dddddddd-1111-4222-8333-444455556666");
        let mut l = store.find(&d.letter_id).unwrap().unwrap();
        store.set_deadline(&mut l, "2020-01-01 10:00").unwrap();
        let (kind, text, state) = stage(&d.letter_id);
        assert_eq!((kind.as_str(), text.as_str(), state.as_deref()), ("failed", t("stage.review_expired"), Some("expired")));
        assert!(queued(&store).is_empty());
        crate::ops::retry(&store, &d.letter_id).unwrap();
        assert_eq!(store.find(&d.letter_id).unwrap().unwrap().status, store::REVIEW, "retrying an expired one asks again");
        assert_eq!(queued(&store), vec![d.letter_id.clone()]);
        decide(&store, &d.letter_id, "decline").unwrap();
        assert_eq!(stage(&d.letter_id), ("cancelled".into(), t("stage.review_declined").into(), Some("declined".into())));
        assert!(!trusted(&store, a, "dddddddd-1111-4222-8333-444455556666", None), "declining trusts nothing");

        crate::store::set_thread_root(None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn people_and_the_app_are_never_held() {
        let dir = std::env::temp_dir().join(format!("dispatch-review-{}", uuid::Uuid::new_v4()));
        let store = Store::at(dir.clone());
        let agent = "aaaaaaaa-1111-4222-8333-444455556666";
        let to = session("bbbbbbbb-1111-4222-8333-444455556666", Harness::Codex, None);
        let person = session("app", Harness::Human, None);
        assert!(!needed(&store, "app", &to), "the user wrote it in the app");
        assert!(!needed(&store, agent, &person), "a reply to the person lands in the feed");
        let _ = fs::remove_dir_all(dir);
    }
}
