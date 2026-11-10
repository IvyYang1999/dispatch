//! The delivery ladder: store the message, then get it in the fastest way that works.
//!
//! 1. Write the recipient's copy to the store (so nothing is ever lost), with a deadline.
//! 2. If the recipient is a running Claude Code session with an inbox that would
//!    take it, write it to the socket and wait for the transcript to show it queued.
//!    Codex threads loaded on the daemon get `codex queue`; Grok bots get a direct message.
//! 3. Otherwise leave it queued: the hook hands it over the next time the session speaks,
//!    or the sweep retries it when its route opens.
//! 4. When nobody could ever deliver it as things stand (no hook and the session is closed,
//!    no codex command, no Grok Bot app), it is marked not delivered right away, with the fix.
//!    Past its deadline a queued copy is marked not delivered too (`ops::reconcile`).

use crate::claude::{self, Gate, SendOutcome};
use crate::discover;
use crate::hook;
use crate::i18n::t;
use crate::model::{is_uuid, Harness, Host, Session};
use crate::peer::{self, PeerError};
use crate::store::{self, Letter, Store, DELIVERED_FEED, DELIVERED_INBOX, FAILED, UNREAD};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Request {
    pub to: Session,
    pub from: String,
    pub from_name: String,
    pub text: String,
    pub urgency: String,
    pub dispatch_id: Option<String>,
    /// Wait for the transcript to confirm the queue (up to 10 s).
    pub wait: bool,
    /// Codex only: let `codex queue --resume` load a thread the daemon has not loaded. Off by
    /// default, because it may start a second instance of a thread that is open elsewhere.
    pub allow_resume: bool,
    /// How long the copy may wait before it counts as not delivered; the urgency's default when None.
    pub deadline: Option<chrono::Duration>,
    /// A new message (not a reply): it waits for the user's review when `review::needed` says so.
    pub review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub letter_id: String,
    pub dispatch_id: String,
    pub path: String,
    /// `inbox` (delivered), `stored` (queued, waiting), `review` (held here until the user says
    /// send), `failed` (marked not delivered, with the fix in `detail`).
    pub route: String,
    pub detail: String,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// How a recipient was found: by its id, by an exact name, or by a fragment of a name,
/// title, folder or description (the kind of match worth remembering as a provisional name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Id,
    Exact,
    Fragment,
}

/// Resolve a recipient: a full id, a short id prefix, a desktop name or a title fragment,
/// with a paired machine's name in front (`主力/release`) to look only there.
pub fn resolve(query: &str) -> Result<Session> {
    resolve_how(query).map(|(s, _)| s)
}

pub fn resolve_how(query: &str) -> Result<(Session, Match)> {
    let (host, q) = match peer::split_address(query.trim())? {
        Some((p, rest)) => (Some(p), rest),
        None => (None, query.trim().to_string()),
    };
    let q = q.as_str();
    // the person at the app or the command line: replies to them land in the feed
    if is_person(q) {
        return Ok((
            match &host {
                Some(p) => peer::remote_target(&p.id, &p.name, q, Some(q.into()), Harness::Human),
                None => bare_session(Harness::Human, q, Some(q.into())),
            },
            Match::Exact,
        ));
    }
    let on_host = |s: &Session| match &host {
        Some(p) => s.host.as_ref().map(|h| h.id == p.id).unwrap_or(false),
        None => true,
    };
    let recent: Vec<Session> = discover::sessions().into_iter().filter(|s| on_host(s)).collect();
    if let Some(hit) = pick(q, &recent)? {
        return Ok(hit);
    }
    // not among the recent ones: every session here whatever its age, and what the machines find
    let wide: Vec<Session> = discover::anywhere(q, host.as_ref()).into_iter().filter(|s| on_host(s)).collect();
    if let Some(hit) = pick(q, &wide)? {
        return Ok(hit);
    }
    if is_uuid(q) {
        return Ok((
            match &host {
                Some(p) => peer::remote_target(&p.id, &p.name, q, None, Harness::Claude),
                // its files say what it runs on, whatever its age; else a Claude session by default
                None => discover::lookup_files(q).unwrap_or_else(|| bare_session(Harness::Claude, q, None)),
            },
            Match::Id,
        ));
    }
    Err(anyhow!("{}", t("send.no_match").replace("{q}", q)))
}

/// The one session among these that `q` names: Ok(None) when none does, an error listing them
/// when several do.
fn pick(q: &str, all: &[Session]) -> Result<Option<(Session, Match)>> {
    if is_uuid(q) {
        return Ok(all.iter().find(|s| s.id.eq_ignore_ascii_case(q)).map(|s| (s.clone(), Match::Id)));
    }
    // an exact name (a contact, provisional or not, or the name the client gave it) wins over any fragment
    let book = crate::contacts::list();
    let named = |s: &Session| {
        let host_id = s.host.as_ref().map(|h| h.id.as_str());
        s.name.as_deref().map(|n| n.eq_ignore_ascii_case(q)).unwrap_or(false)
            || book.iter().any(|c| c.name.eq_ignore_ascii_case(q) && c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id)
    };
    let exact: Vec<&Session> = all.iter().filter(|s| named(s)).collect();
    if exact.len() == 1 {
        return Ok(Some((exact[0].clone(), Match::Exact)));
    }
    let ql = q.to_lowercase();
    let about = |s: &Session| -> Option<String> {
        let host_id = s.host.as_ref().map(|h| h.id.as_str());
        book.iter().find(|c| c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id).and_then(|c| c.description.clone())
    };
    let mut hits: Vec<&Session> = all.iter().filter(|s| s.id.to_lowercase().starts_with(&ql)).collect();
    let how = if hits.is_empty() { Match::Fragment } else { Match::Id };
    if hits.is_empty() {
        hits = all.iter().filter(|s| s.name.as_deref().map(|n| n.to_lowercase().contains(&ql)).unwrap_or(false)).collect();
    }
    if hits.is_empty() {
        hits = all.iter().filter(|s| about(s).map(|d| d.to_lowercase().contains(&ql)).unwrap_or(false)).collect();
    }
    if hits.is_empty() {
        hits = all.iter().filter(|s| s.title.as_deref().map(|n| n.to_lowercase().contains(&ql)).unwrap_or(false)).collect();
    }
    match hits.len() {
        0 => Ok(None),
        1 => Ok(Some((hits[0].clone(), how))),
        _ => {
            let list: Vec<String> = hits.iter().take(8).map(|s| format!("  {}  {}  {}", s.short_address(), s.display_name(), s.cwd.as_deref().map(crate::model::short_cwd).unwrap_or_default())).collect();
            Err(anyhow!("{}", t("send.many_match").replace("{q}", q).replace("{n}", &hits.len().to_string()).replace("{list}", &list.join("\n"))))
        }
    }
}

/// `app`, `cli`, `mcp`: a person (or a process with no session) wrote this; replies to it are
/// shown in the feed rather than handed to a session.
pub fn is_person(id: &str) -> bool {
    matches!(id, "app" | "cli" | "mcp")
}

/// What a sender runs on: a person at the app or the command line, or whatever discovery
/// knows the id or name as. None when nothing says.
pub fn sender_harness(from: &str) -> Option<&'static str> {
    if is_person(from) {
        return Some("human");
    }
    discover::sessions()
        .into_iter()
        .find(|s| s.id.eq_ignore_ascii_case(from) || s.name.as_deref().map(|n| n.eq_ignore_ascii_case(from)).unwrap_or(false))
        .or_else(|| crate::model::is_uuid(from).then(|| discover::lookup(from)).flatten())
        .map(|s| harness_code(s.harness))
}

/// A session discovery did not list, known only by id (and name, from a stored copy).
fn bare_session(harness: Harness, id: &str, name: Option<String>) -> Session {
    Session {
        harness,
        id: id.to_lowercase(),
        short_id: crate::model::short_id(id),
        name,
        cwd: None,
        title: None,
        last_active: 0,
        running: harness == Harness::Claude && claude::is_running(id),
        desktop: false,
        host_session_id: None,
        status: None,
        transcript: None,
        host: None,
        project: None,
        app_name: None,
    }
}

fn harness_code(h: Harness) -> &'static str {
    match h {
        Harness::Claude => "claude",
        Harness::Codex => "codex",
        Harness::Grok => "grok",
        Harness::Human => "human",
    }
}

pub(crate) fn harness_of(code: Option<&str>) -> Harness {
    match code {
        Some("codex") => Harness::Codex,
        Some("grok") => Harness::Grok,
        Some("human") => Harness::Human,
        _ => Harness::Claude,
    }
}

/// The recipient of a stored copy: the live session when discovery lists it, else what the
/// copy says. A copy for another machine names a session there; that machine resolves it.
pub fn target_of(letter: &Letter) -> Session {
    if let Some(h) = &letter.to_host {
        let name = letter.to_host_name.clone().unwrap_or_else(|| crate::model::short_id(h));
        return peer::remote_target(h, &name, &letter.to, Some(letter.to_name.clone()), harness_of(letter.to_harness.as_deref()));
    }
    if let Some(s) = discover::local().into_iter().find(|s| s.id.eq_ignore_ascii_case(&letter.to)) {
        return s;
    }
    bare_session(harness_of(letter.to_harness.as_deref()), &letter.to, Some(letter.to_name.clone()))
}

/// Who sent it, with the machine when it is another one: `Name @ Machine`.
pub fn sender_text(l: &Letter) -> String {
    match (&l.from_host_name, &l.signature) {
        (Some(h), Some(_)) => t("send.from_at_verified").replace("{from}", &l.from_name).replace("{host}", h),
        (Some(h), None) => t("send.from_at").replace("{from}", &l.from_name).replace("{host}", h),
        _ => l.from_name.clone(),
    }
}

/// The text the session sees: who sent it, when, the body, and how to answer.
pub fn envelope(l: &Letter) -> String {
    t("send.envelope")
        .replace("{from}", &sender_text(l))
        .replace("{created}", &l.created)
        .replace("{id}", &crate::model::short_id(&l.id))
        .replace("{body}", l.body.trim())
        .replace("{cmd}", &crate::model::self_cmd())
}

type Reporter<'a> = &'a dyn Fn(&str, String) -> Report;

/// Mark the copy not delivered: the attempt is logged, then a `reach failed` line with the reason.
fn fail(store: &Store, letter: &mut Letter, action: &str, why_log: &str, text: String, report: Reporter) -> Result<Report> {
    store.append_run(&letter.id, action, "skipped", why_log)?;
    store.set_status(letter, FAILED)?;
    store.append_run(&letter.id, "reach", "failed", why_log)?;
    Ok(report("failed", text))
}

/// The Codex branch: only a thread the shared daemon has loaded gets `codex queue`; anything
/// else waits as 待接通 unless the caller allowed `--resume`. No codex command at all: not delivered.
fn deliver_codex(store: &Store, to: &Session, letter: &mut Letter, allow_resume: bool, report: Reporter) -> Result<Report> {
    let st = crate::codex::state();
    if !st.cli {
        return fail(store, letter, "codex", "codex not installed", t("send.failed_codex_nocli").into(), report);
    }
    let loaded = st.daemon && st.loaded.contains(&to.id);
    if !loaded && !(allow_resume && st.daemon) {
        // Codex's own queue: whichever Codex process has the thread open (the desktop app
        // included) takes it when the thread is idle; one nobody has open waits to be opened.
        let text = envelope(letter);
        return match crate::codex::queue_add(&to.id, &text, &letter.id) {
            Ok(qid) => {
                store.set_status(letter, store::DELIVERED_CODEX)?;
                store.append_run(&letter.id, "codex", "queued", &format!("thread/queue/add {qid}"))?;
                Ok(report("inbox", t("send.codex_queued_open").into()))
            }
            Err(e) => {
                store.append_run(&letter.id, "codex", "failed", &format!("{e:#}"))?;
                Ok(report("stored", t("send.codex_failed").replace("{err}", &format!("{e:#}"))))
            }
        };
    }
    let text = envelope(letter);
    match crate::codex::queue(&to.id, &text, !loaded) {
        Ok(out) => {
            store.set_status(letter, store::DELIVERED_CODEX)?;
            store.append_run(&letter.id, "codex", "queued", &format!("{}{}", if out.resumed { "resumed; " } else { "" }, out.detail))?;
            Ok(report("inbox", if loaded { t("send.codex_queued").into() } else if out.resumed { t("send.codex_queued_resumed").into() } else { t("send.codex_queued_unconfirmed").replace("{detail}", &out.detail) }))
        }
        Err(e) => {
            store.append_run(&letter.id, "codex", "failed", &format!("{e:#}"))?;
            Ok(report("stored", t("send.codex_failed").replace("{err}", &format!("{e:#}"))))
        }
    }
}

/// The Grok Bot branch: a direct message through the app's gateway, driven by `gbot`. There is
/// no hook to fall back on: missing app, sign-in or gbot means not delivered; a gateway error
/// keeps it queued for the sweep to retry.
fn deliver_grok(store: &Store, to: &Session, letter: &mut Letter, report: Reporter) -> Result<Report> {
    let st = crate::grok::state();
    let blocked = if !st.app {
        Some(("no Grok Bot app", t("reach.grok_noapp")))
    } else if !st.signed_in {
        Some(("Grok Bot not signed in", t("reach.grok_nologin")))
    } else if st.cli.is_none() {
        Some(("gbot not installed", t("reach.grok_nocli")))
    } else {
        None
    };
    if let Some((why, text)) = blocked {
        return fail(store, letter, "grok", why, t("send.failed_grok").replace("{why}", text), report);
    }
    let target = st.bot(&to.id).map(|b| b.name.clone()).unwrap_or_else(|| to.display_name());
    let text = envelope(letter);
    match crate::grok::send(&target, &text) {
        Ok(out) => {
            store.set_status(letter, store::DELIVERED_GROK)?;
            store.append_run(&letter.id, "grok", "sent", out.message_id.as_deref().unwrap_or(&out.detail))?;
            Ok(report("inbox", t("send.grok_sent").replace("{id}", out.message_id.as_deref().unwrap_or("?"))))
        }
        Err(e) => {
            store.append_run(&letter.id, "grok", "failed", &format!("{e:#}"))?;
            Ok(report("stored", t("send.grok_failed").replace("{err}", &format!("{e:#}"))))
        }
    }
}

/// The Claude Code branch: the socket when the session takes it; the store and the hook when it
/// does not; not delivered when there is no hook to fall back on.
fn deliver_claude(store: &Store, to: &Session, letter: &mut Letter, wait: bool, report: Reporter) -> Result<Report> {
    let hook_on = hook::installed();
    let Some(inbox) = claude::find_live(&to.id).filter(|_| claude::inbox_supported()) else {
        let (why_log, why) = if !claude::inbox_supported() && to.running {
            ("no inbox sockets on this system", t("send.why_nosocket"))
        } else if to.running {
            ("running without an inbox", t("send.why_noinbox"))
        } else {
            ("not running", t("send.why_off"))
        };
        if !hook_on {
            return fail(store, letter, "inbox", why_log, t("send.failed_nohook").replace("{why}", why), report);
        }
        store.append_run(&letter.id, "inbox", "skipped", why_log)?;
        return Ok(report("stored", t("send.stored_hook").replace("{why}", why)));
    };
    let (gate, why) = claude::gate_reason(&to.id, inbox.cwd.as_deref());
    match gate {
        Gate::Accept => {}
        Gate::Hold => {
            let unknown = why == "mode unknown";
            let text_why = if unknown { t("send.why_unknown") } else { t("send.why_hold") };
            if !hook_on {
                return fail(store, letter, "inbox", &format!("{why}: would be held"), t("send.failed_nohook").replace("{why}", text_why), report);
            }
            store.append_run(&letter.id, "inbox", "skipped", &format!("{why}: would be held"))?;
            return Ok(report("stored", t(if unknown { "send.hold_unknown" } else { "send.hold" }).into()));
        }
        Gate::Refuse => {
            if !hook_on {
                return fail(store, letter, "inbox", "crossSessionInbound=refuse", t("send.failed_nohook").replace("{why}", t("send.why_refuse")), report);
            }
            store.append_run(&letter.id, "inbox", "skipped", "crossSessionInbound=refuse")?;
            return Ok(report("stored", t("send.refuse").into()));
        }
    }
    let text = envelope(letter);
    match claude::send(&inbox, &text, &letter.id, Duration::from_secs(5)) {
        SendOutcome::Unreachable => {
            store.append_run(&letter.id, "inbox", "unreachable", &inbox.socket.to_string_lossy())?;
            Ok(report("stored", t("send.unreachable").into()))
        }
        SendOutcome::Sent => {
            store.append_run(&letter.id, "inbox", "written", &inbox.socket.to_string_lossy())?;
            if !wait {
                return Ok(report("inbox", t("send.written_nowait").into()));
            }
            if claude::queued(&to.id, inbox.cwd.as_deref(), &letter.id, &text, Duration::from_secs(10)) {
                store.set_status(letter, DELIVERED_INBOX)?;
                store.append_run(&letter.id, "inbox", "queued", "transcript shows it")?;
                Ok(report("inbox", t("send.queued").into()))
            } else {
                store.append_run(&letter.id, "inbox", "unconfirmed", "no trace in transcript within 10s")?;
                Ok(report("stored", t("send.unconfirmed").into()))
            }
        }
    }
}

/// The branch for another machine: hand the copy over; that machine stores it and runs its own
/// ladder, and its answer becomes this copy's status. A machine that does not answer leaves the
/// copy queued for the sweep; one that refuses (not paired any more) means not delivered.
fn deliver_remote(store: &Store, host: &Host, letter: &mut Letter, report: Reporter) -> Result<Report> {
    match peer::deliver(host, letter) {
        Ok((status, route, detail)) => {
            if letter.status != status {
                store.set_status(letter, &status)?;
            }
            store.append_run(&letter.id, "peer", &route, &format!("{}: {}", host.name, detail))?;
            Ok(report(&route, t("send.peer_route").replace("{host}", &host.name).replace("{detail}", &detail)))
        }
        Err(PeerError::Offline(why)) => {
            store.append_run(&letter.id, "peer", "unreachable", &format!("{}: {}", host.name, why))?;
            Ok(report("stored", t("send.peer_offline").replace("{host}", &host.name)))
        }
        Err(PeerError::Refused(why)) => fail(store, letter, "peer", &format!("{}: {}", host.name, why), t("send.peer_refused").replace("{host}", &host.name).replace("{why}", &why), report),
        Err(PeerError::Protocol(why)) => {
            store.append_run(&letter.id, "peer", "error", &format!("{}: {}", host.name, why))?;
            Ok(report("stored", t("send.peer_error").replace("{host}", &host.name).replace("{why}", &why)))
        }
    }
}

/// Try to get a stored copy into its session now. The copy's status and run log record the outcome.
pub fn deliver(store: &Store, to: &Session, letter: &mut Letter, wait: bool, allow_resume: bool) -> Result<Report> {
    let (letter_id, dispatch_id, path) = (letter.id.clone(), letter.dispatch_id.clone(), letter.path.clone());
    let report = move |route: &str, detail: String| Report { letter_id: letter_id.clone(), dispatch_id: dispatch_id.clone(), path: path.clone(), route: route.into(), detail };
    if let Some(h) = &to.host {
        return deliver_remote(store, h, letter, &report);
    }
    match to.harness {
        Harness::Codex => deliver_codex(store, to, letter, allow_resume, &report),
        Harness::Grok => deliver_grok(store, to, letter, &report),
        Harness::Human => {
            store.set_status(letter, DELIVERED_FEED)?;
            store.append_run(&letter.id, "feed", "shown", "")?;
            Ok(report("inbox", t("send.feed_shown").into()))
        }
        Harness::Claude => deliver_claude(store, to, letter, wait, &report),
    }
}

/// Store the copy, then deliver it; or, when the user reviews this one first, hold it here.
pub fn send(store: &Store, req: Request) -> Result<Report> {
    let created = store::now_text();
    let wait_for = req.deadline.unwrap_or_else(|| store::default_wait(&req.urgency));
    let held = req.review && crate::review::needed(store, &req.from, &req.to);
    let mut letter = Letter {
        id: uuid::Uuid::new_v4().to_string(),
        dispatch_id: req.dispatch_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        from: req.from.clone(),
        from_name: req.from_name.clone(),
        to: req.to.id.clone(),
        to_name: req.to.display_name(),
        urgency: req.urgency.clone(),
        wake: true,
        // held from the first byte: nothing hands over a copy that waits for review
        status: if held { store::REVIEW.into() } else { UNREAD.into() },
        deadline: Some(store::deadline_text(&created, wait_for)),
        to_harness: Some(harness_code(req.to.harness).into()),
        from_host: None,
        from_host_name: None,
        to_host: req.to.host.as_ref().map(|h| h.id.clone()),
        to_host_name: req.to.host.as_ref().map(|h| h.name.clone()),
        from_harness: sender_harness(&req.from).map(str::to_string),
        signature: None,
        created,
        body: req.text.clone(),
        path: String::new(),
    };
    let path = store.write_letter(&mut letter)?;
    store.append_run(&letter.id, "store", "written", &path.to_string_lossy())?;
    if held {
        store.append_run(&letter.id, "review", "asked", crate::review::mode())?;
        crate::review::enqueue(store, &letter.id)?;
        let detail = if crate::review::app_alive(store) { t("send.review").to_string() } else { format!("{} {}", t("send.review"), t("send.review_no_app")) };
        return Ok(Report { letter_id: letter.id.clone(), dispatch_id: letter.dispatch_id.clone(), path: letter.path.clone(), route: "review".into(), detail });
    }
    deliver(store, &req.to, &mut letter, req.wait, req.allow_resume)
}

/// Deliver a stored, still pending copy again (a retry, or the sweep when its route opened).
/// A copy marked not delivered goes back to queued first, so the outcome speaks for itself.
pub fn redeliver(store: &Store, letter: &mut Letter) -> Result<Report> {
    if !store::pending(&letter.status) {
        return Err(anyhow!("{}", t("ops.not_pending").replace("{status}", crate::i18n::status_label(&letter.status))));
    }
    if letter.status == FAILED {
        store.set_status(letter, UNREAD)?;
    }
    store.append_run(&letter.id, "retry", "started", "")?;
    let to = target_of(letter);
    deliver(store, &to, letter, true, false)
}
