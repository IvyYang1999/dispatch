//! The operations both the CLI and the MCP server expose, as JSON-friendly values.

use crate::claude;
use crate::discover;
use crate::hook;
use crate::i18n::t;
use crate::model::{is_uuid, short_cwd, short_id, Session};
use crate::reach;
use crate::send;
use crate::store::{self, Store};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// One session as the app shows it: names, where it runs, and how a message reaches it now.
pub fn session_json(s: &Session) -> Value {
    session_json_with(s, hook::installed(), &crate::contacts::list())
}

fn session_json_with(s: &Session, hook_on: bool, contacts: &[crate::contacts::Contact]) -> Value {
    let r = reach::decide(s, hook_on);
    let host_id = s.host.as_ref().map(|h| h.id.as_str());
    let contact = contacts.iter().find(|c| c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id);
    json!({
        "contact": contact.map(|c| c.name.clone()),
        "avatar": contact.and_then(|c| c.avatar.clone()),
        "about": contact.and_then(|c| c.description.clone()),
        "provisional": contact.map(|c| c.provisional).unwrap_or(false),
        "id": s.id,
        "short_id": s.short_id,
        "name": s.display_name(),
        // a name the address book gave is not the app's: then only what the app was seen to call it
        "app_name": if contact.is_some() { s.app_name.clone() } else { s.app_name.clone().or_else(|| s.name.clone()) },
        "harness": s.harness.label(),
        "title": s.title,
        "project": s.project,
        "cwd": s.cwd,
        "cwd_short": s.cwd.as_deref().map(short_cwd),
        "running": s.running,
        "status": s.status,
        "desktop": s.desktop,
        "host_session_id": s.host_session_id,
        "last_active": s.last_active,
        "host": s.host,
        "address": s.address(),
        "reach": { "level": r.level, "text": r.text, "fix": r.fix, "mode": r.mode, "client": r.client, "plain": reach::plain(s, &r) },
        "share": crate::share::level(&s.id, host_id),
    })
}

/// A Codex thread id handed over with the current MCP call (`_meta.threadId`, which Codex puts
/// on every tool call). Set by the MCP server around each call.
static MCP_THREAD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_mcp_thread(id: Option<String>) {
    if let Ok(mut t) = MCP_THREAD.lock() {
        *t = id.filter(|i| is_uuid(i)).map(|i| i.to_lowercase());
    }
}

/// The session making this call.
#[derive(Debug, Clone)]
pub struct Caller {
    pub id: String,
    pub name: String,
    pub harness: crate::model::Harness,
    pub cwd: Option<String>,
    pub desktop: bool,
}

/// Who is calling: the Codex thread named on this MCP call, else the Claude Code session this
/// process was started by, else the Codex thread whose command started it (Codex sets
/// `CODEX_THREAD_ID` on the commands it runs). None for a person at a terminal.
pub fn caller() -> Option<Caller> {
    let mcp = MCP_THREAD.lock().ok().and_then(|t| t.clone());
    if let Some(id) = mcp {
        return Some(codex_caller(&id));
    }
    if let Some(i) = claude::caller_session() {
        let name = i.name.clone().unwrap_or_else(|| short_id(&i.session_id));
        return Some(Caller { id: i.session_id.to_lowercase(), name, harness: crate::model::Harness::Claude, cwd: i.cwd.clone(), desktop: i.desktop });
    }
    std::env::var("CODEX_THREAD_ID").ok().filter(|i| is_uuid(i)).map(|id| codex_caller(&id.to_lowercase()))
}

/// A Codex thread as a sender: its address-book name, else the title Codex shows, else its id.
fn codex_caller(id: &str) -> Caller {
    let s = discover::lookup(id);
    let name = crate::contacts::for_session(id, None).map(|c| c.name).or_else(|| s.as_ref().and_then(|s| s.app_name.clone().or_else(|| s.name.clone()).or_else(|| s.title.clone()))).unwrap_or_else(|| short_id(id));
    Caller { id: id.to_string(), name, harness: crate::model::Harness::Codex, cwd: s.as_ref().and_then(|s| s.cwd.clone()), desktop: false }
}

/// Who is calling, as `dispatch_whoami` answers.
pub fn whoami() -> Result<Value> {
    let me = crate::peer::me()?;
    let machine = json!({ "id": me.id, "name": me.name });
    Ok(match caller() {
        Some(c) => {
            let contact = crate::contacts::for_session(&c.id, None).map(|c| c.name);
            let handle = contact.clone().unwrap_or_else(|| short_id(&c.id));
            json!({ "session_id": c.id, "short_id": short_id(&c.id), "name": c.name, "contact": contact, "harness": c.harness.label(), "cwd": c.cwd, "desktop": c.desktop, "machine": machine, "address_elsewhere": format!("{}/{}", me.name, handle) })
        }
        None => json!({ "session_id": null, "name": "unknown", "harness": null, "machine": machine }),
    })
}

/// The caller's identity for `from`: (id, name).
pub fn caller_identity() -> (String, String) {
    match caller() {
        Some(c) => (c.id, c.name),
        None => ("mcp".into(), "mcp".into()),
    }
}

pub fn sessions(running_only: bool) -> Value {
    let hook_on = hook::installed();
    let contacts = crate::contacts::list();
    // a named session is listed whether or not it is running: that is what the name is for
    let list: Vec<Value> = discover::sessions().iter().filter(|s| !running_only || s.running || contacts.iter().any(|c| c.session_id.eq_ignore_ascii_case(&s.id))).map(|s| session_json_with(s, hook_on, &contacts)).collect();
    json!({ "hook_installed": hook_on, "sessions": list })
}

/// The address book, each name with the session it points at.
pub fn contacts() -> Value {
    let all = discover::sessions();
    let rows: Vec<Value> = crate::contacts::list()
        .into_iter()
        .map(|c| {
            let live = all.iter().find(|s| s.id.eq_ignore_ascii_case(&c.session_id) && s.host.as_ref().map(|h| &h.id) == c.host.as_ref().map(|h| &h.id));
            json!({ "name": c.name, "session_id": c.session_id, "short_id": short_id(&c.session_id), "harness": c.harness.label(), "host": c.host, "cwd": c.cwd, "set_by": c.set_by, "updated": c.updated, "about": c.description, "provisional": c.provisional, "running": live.map(|s| s.running).unwrap_or(false), "address": match &c.host { Some(h) => format!("{}/{}", h.name, c.name), None => c.name.clone() } })
        })
        .collect();
    json!({ "count": rows.len(), "contacts": rows })
}

/// Give a session a stable name (and a line on what it is for): the one `session` names, else
/// the calling session.
pub fn register(name: &str, session: Option<&str>, by: &str, description: Option<&str>) -> Result<Value> {
    let target = match session.map(str::trim).filter(|s| !s.is_empty()) {
        Some(q) => send::resolve(q)?,
        None => {
            let i = caller().ok_or_else(|| anyhow!("no session given and the caller is not a Claude Code or Codex session; pass the session's id or name"))?;
            discover::lookup(&i.id).unwrap_or_else(|| {
                let mut s = crate::peer::remote_target("", "", &i.id, Some(i.name.clone()), i.harness);
                s.host = None;
                s.cwd = i.cwd.clone();
                s
            })
        }
    };
    let c = crate::contacts::set(name, &target, by, description, false)?;
    Ok(json!({ "name": c.name, "about": c.description, "session_id": c.session_id, "short_id": short_id(&c.session_id), "harness": c.harness.label(), "host": c.host, "address": match &c.host { Some(h) => format!("{}/{}", h.name, c.name), None => c.name.clone() }, "text": t("ops.registered").replace("{name}", &c.name).replace("{who}", &target.display_name()) }))
}

pub fn unregister(name: &str) -> Result<Value> {
    match crate::contacts::remove(name)? {
        Some(c) => Ok(json!({ "removed": c.name })),
        None => Err(anyhow!("{}", t("ops.no_contact").replace("{name}", name))),
    }
}

/// At most this many candidates come back from a find.
const FIND_MAX: usize = 20;

/// Every session, of any age, here or on a paired machine, whose fields hold all the words of
/// the query, each with why it matched: one named exactly by the query first, then running
/// ones, then the most recently active. `<machine>/<words>` looks on that machine only.
pub fn find_sessions(query: &str) -> Vec<(Session, &'static str)> {
    let (host, q) = match crate::peer::split_address(query.trim()).ok().flatten() {
        Some((p, rest)) => (Some(p), rest),
        None => (None, query.trim().to_string()),
    };
    // nothing to look for: the recent ones, as the session list shows them
    let pool = if q.trim().is_empty() { discover::sessions() } else { discover::anywhere(&q, host.as_ref()) };
    let pool: Vec<Session> = pool.into_iter().filter(|s| host.as_ref().map(|p| s.host.as_ref().map(|h| h.id == p.id).unwrap_or(false)).unwrap_or(true)).collect();
    let book = crate::contacts::list();
    let ql = q.trim().to_lowercase();
    let exact = |s: &Session| {
        let host_id = s.host.as_ref().map(|h| h.id.as_str());
        !ql.is_empty()
            && (s.name.as_deref().map(|n| n.to_lowercase() == ql).unwrap_or(false)
                || book.iter().any(|c| c.name.to_lowercase() == ql && c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id))
    };
    let mut out = discover::matching(&q, pool);
    out.sort_by(|(a, _), (b, _)| exact(b).cmp(&exact(a)).then(b.running.cmp(&a.running)).then(b.last_active.cmp(&a.last_active)));
    out
}

/// What `find_sessions` finds, as candidates for an agent: at most `FIND_MAX`, with how many matched.
pub fn find(query: &str) -> Value {
    let hook_on = hook::installed();
    let book = crate::contacts::list();
    let all = find_sessions(query);
    let count = all.len();
    let out: Vec<Value> = all.iter().take(FIND_MAX).map(|(s, r)| with_reason(s, r, hook_on, &book)).collect();
    let mut note = "Send only when exactly one candidate is the right one; otherwise ask the user or refine the query. Sessions of any age are searched; every word of the query must match.".to_string();
    if count > out.len() {
        note = format!("{count} sessions matched; these are the {} most relevant. Add a word to narrow it down. {note}", out.len());
    }
    json!({ "query": query, "count": count, "candidates": out, "note": note })
}

/// Every session this machine has records of or a paired machine finds (any age), that matches.
pub fn search(query: &str) -> Value {
    let hook_on = hook::installed();
    let book = crate::contacts::list();
    let out: Vec<Value> = find_sessions(query).iter().map(|(s, r)| with_reason(s, r, hook_on, &book)).collect();
    json!({ "query": query, "count": out.len(), "sessions": out })
}

fn with_reason(s: &Session, reason: &str, hook_on: bool, book: &[crate::contacts::Contact]) -> Value {
    let mut v = session_json_with(s, hook_on, book);
    v["reason"] = json!(reason);
    v
}

pub fn send_many(store: &Store, to: &[String], text: &str, urgency: &str, from: Option<(String, String)>) -> Result<Value> {
    send_many_opts(store, to, text, urgency, from, false, None)
}

/// `30m`, `2h`, `1d` → a wait; None → the urgency's default.
pub fn parse_deadline(s: Option<&str>) -> Result<Option<chrono::Duration>> {
    match s.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => store::parse_duration(s).map(Some).ok_or_else(|| anyhow!("{}", t("ops.bad_deadline").replace("{s}", s))),
    }
}

pub fn send_many_opts(store: &Store, to: &[String], text: &str, urgency: &str, from: Option<(String, String)>, allow_resume: bool, deadline: Option<&str>) -> Result<Value> {
    if to.is_empty() {
        return Err(anyhow!("`to` must name at least one session"));
    }
    let deadline = parse_deadline(deadline)?;
    let (from_id, from_name) = from.unwrap_or_else(caller_identity);
    let dispatch_id = send::new_id();
    let urgency = store::urgency_stored(urgency);
    let mut reports = vec![];
    for who in to {
        let (target, how) = send::resolve_how(who)?;
        let rep = send::send(store, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: text.to_string(), urgency: urgency.to_string(), dispatch_id: Some(dispatch_id.clone()), wait: true, allow_resume, deadline, review: true })?;
        let named = remember_match(who, &target, how);
        reports.push(json!({ "to": { "id": target.id, "short_id": target.short_id, "name": target.display_name(), "host": target.host, "address": target.address() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail, "path": rep.path, "named": named }));
    }
    Ok(json!({ "dispatch_id": dispatch_id, "from": { "id": from_id, "name": from_name }, "recipients": reports }))
}

/// A recipient found by a fragment and not yet in the address book gets the words it was found
/// by as a provisional name, so the same words hit it directly next time. Returns the name.
pub fn remember_match(query: &str, target: &Session, how: send::Match) -> Option<Value> {
    if how != send::Match::Fragment || send::is_person(&target.id) {
        return None;
    }
    let (_, q) = crate::peer::split_address(query.trim()).ok().flatten().map(|(p, rest)| (Some(p), rest)).unwrap_or((None, query.trim().to_string()));
    if q.is_empty() || crate::contacts::for_session(&target.id, target.host.as_ref().map(|h| h.id.as_str())).is_some() {
        return None;
    }
    crate::contacts::set(&q, target, "auto", None, true).ok().map(|c| json!({ "name": c.name, "provisional": true, "note": t("ops.provisional_note").replace("{name}", &c.name) }))
}

/// Mark every queued copy past its deadline as not delivered. Returns how many changed.
pub fn reconcile(store: &Store, letters: &mut [store::Letter]) -> Result<usize> {
    let now = chrono::Local::now().naive_local();
    let mut n = 0;
    for l in letters.iter_mut() {
        if crate::review::expire(store, l, now)? {
            n += 1;
            continue;
        }
        if l.overdue(now) {
            store.set_status(l, store::FAILED)?;
            store.append_run(&l.id, "deadline", "failed", &t("send.deadline_passed").replace("{t}", &store::time_text(l.deadline_at())))?;
            n += 1;
        }
    }
    Ok(n)
}

/// The copies `id` names: one copy (by id or prefix), or every copy of a dispatch (by its id).
fn copies_named(store: &Store, id: &str) -> Result<Vec<store::Letter>> {
    let all = store.list(None, false)?;
    if let Some(l) = all.iter().find(|l| l.id.starts_with(id)) {
        return Ok(vec![l.clone()]);
    }
    let copies: Vec<store::Letter> = all.into_iter().filter(|l| l.dispatch_id.starts_with(id)).collect();
    if copies.is_empty() {
        return Err(anyhow!("no message whose id starts with \"{id}\""));
    }
    Ok(copies)
}

/// Withdraw the still pending copies `id` names: nothing delivers them any more.
pub fn cancel(store: &Store, id: &str, by: &str) -> Result<Value> {
    let copies = copies_named(store, id)?;
    let mut done = vec![];
    for mut l in copies.iter().filter(|l| store::pending(&l.status) || l.status == store::HELD || l.status == store::REVIEW).cloned() {
        if l.status == store::REVIEW {
            crate::review::dequeue(store, &l.id);
            store.append_run(&l.id, "review", "withdrawn", by)?;
        }
        store.set_status(&mut l, store::CANCELLED)?;
        store.append_run(&l.id, "cancel", "by", by)?;
        if let Some(h) = &l.to_host {
            crate::peer::cancel(&crate::model::Host { id: h.clone(), name: l.to_host_name.clone().unwrap_or_default() }, &l.id);
        }
        done.push(json!({ "letter_id": l.id, "to": l.to, "to_name": l.to_name }));
    }
    if done.is_empty() {
        let status = copies.first().map(|l| crate::i18n::status_label(&l.status)).unwrap_or("");
        return Err(anyhow!("{}", t("ops.not_pending").replace("{status}", status)));
    }
    Ok(json!({ "cancelled": done.len(), "copies": done, "text": t("ops.cancelled").replace("{n}", &done.len().to_string()) }))
}

/// Try the still pending copies `id` names again now, each with a fresh deadline.
pub fn retry(store: &Store, id: &str) -> Result<Value> {
    let copies = copies_named(store, id)?;
    let mut reports = vec![];
    // a copy whose review ran out asks the user again; retrying never skips the review
    for mut l in copies.iter().filter(|l| l.status == store::CANCELLED && crate::review::state(&store.runs(&l.id)).map(|(s, _)| s == "expired").unwrap_or(false)).cloned() {
        let fresh = store::deadline_text(&store::now_text(), store::default_wait(&l.urgency));
        store.set_deadline(&mut l, &fresh)?;
        store.set_status(&mut l, store::REVIEW)?;
        store.append_run(&l.id, "review", "asked", "again")?;
        crate::review::enqueue(store, &l.id)?;
        reports.push(json!({ "letter_id": l.id, "to": l.to, "to_name": l.to_name, "route": "review", "detail": t("send.review"), "status": l.status }));
    }
    for mut l in copies.iter().filter(|l| store::pending(&l.status) || (l.status == store::HELD && l.from_host.is_some())).cloned() {
        // a letter held for approval: retrying it is the approval
        if l.status == store::HELD {
            store.set_status(&mut l, store::UNREAD)?;
            store.append_run(&l.id, "approve", "by", "user")?;
        }
        let fresh = store::deadline_text(&store::now_text(), store::default_wait(&l.urgency));
        store.set_deadline(&mut l, &fresh)?;
        let rep = send::redeliver(store, &mut l)?;
        reports.push(json!({ "letter_id": l.id, "to": l.to, "to_name": l.to_name, "route": rep.route, "detail": rep.detail, "status": l.status }));
    }
    if reports.is_empty() {
        let status = copies.first().map(|l| crate::i18n::status_label(&l.status)).unwrap_or("");
        return Err(anyhow!("{}", t("ops.not_pending").replace("{status}", status)));
    }
    Ok(json!({ "retried": reports.len(), "copies": reports, "text": t("ops.retried").replace("{n}", &reports.len().to_string()) }))
}

/// A delivery attempt that reached the session (so a retry could duplicate it).
fn was_handed(runs: &[store::Run]) -> bool {
    runs.iter().any(|r| matches!((r.action.as_str(), r.result.as_str()), ("inbox", "written") | ("codex", "queued") | ("grok", "sent") | ("hook", "handed") | ("peer", "inbox")))
}

fn last_run_age_secs(runs: &[store::Run]) -> i64 {
    runs.last().and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.at).ok()).map(|at| (chrono::Local::now().fixed_offset() - at).num_seconds()).unwrap_or(i64::MAX)
}

/// The postman's round: expire overdue copies, and deliver queued copies whose route is open now
/// (the session came back, the daemon loaded the thread, the gateway answers). Copies marked not
/// delivered are left for a person to retry; a copy is not tried again within five minutes.
pub fn sweep(store: &Store) -> Result<Value> {
    let _ = crate::peer::mirror(store);
    let mut letters = store.list(None, false)?;
    let failed = reconcile(store, &mut letters)?;
    let queued: Vec<&store::Letter> = letters.iter().filter(|l| l.status == store::UNREAD).collect();
    let mut retried = vec![];
    if !queued.is_empty() {
        let all = discover::sessions();
        let hook_on = hook::installed();
        for l in queued {
            let Some(s) = all.iter().find(|s| s.id.eq_ignore_ascii_case(&l.to)) else { continue };
            if reach::decide(s, hook_on).level != reach::Level::Now {
                continue;
            }
            let runs = store.runs(&l.id);
            // not again within five minutes, unless the session was just opened for this message
            let just_opened = runs.last().map(|r| r.action == "wake").unwrap_or(false);
            if was_handed(&runs) || (last_run_age_secs(&runs) < 300 && !just_opened) {
                continue;
            }
            let mut l = l.clone();
            store.append_run(&l.id, "retry", "started", "sweep")?;
            let rep = send::deliver(store, s, &mut l, true, false)?;
            retried.push(json!({ "letter_id": l.id, "to": l.to, "to_name": l.to_name, "route": rep.route, "detail": rep.detail }));
        }
    }
    Ok(json!({ "failed": failed, "retried": retried }))
}

pub fn inbox(store: &Store, session: Option<&str>, all: bool) -> Result<Value> {
    let sid = match session {
        Some(s) => send::resolve(s)?.id,
        None => caller().map(|c| c.id).ok_or_else(|| anyhow!("no session given and the caller is not a Claude Code or Codex session"))?,
    };
    // a message still held for the user's review has not been sent: its recipient does not see it
    let letters: Vec<store::Letter> = store.list(Some(&sid), !all)?.into_iter().filter(|l| l.status != store::REVIEW).collect();
    let known: Vec<String> = discover::sessions().into_iter().filter(|s| s.harness == crate::model::Harness::Grok).map(|s| s.id).collect();
    let rows: Vec<Value> = letters
        .iter()
        .map(|l| json!({ "id": l.id, "short_id": short_id(&l.id), "dispatch_id": l.dispatch_id, "from": l.from, "from_name": l.from_name, "from_host_name": l.from_host_name, "status": l.status, "created": l.created, "urgency": l.urgency, "body": l.body, "path": l.path, "can_reply": is_uuid(&l.from) || l.from_host.is_some() || send::is_person(&l.from) || known.iter().any(|k| k.eq_ignore_ascii_case(&l.from)) }))
        .collect();
    Ok(json!({ "session_id": sid, "unread_only": !all, "count": rows.len(), "messages": rows }))
}

pub fn reply(store: &Store, id: &str, text: &str) -> Result<Value> {
    let mut orig = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    // A session id (Claude, Codex) or a bot id (Grok) can be answered; "cli", "app" and names cannot.
    // A sender on another machine is answered there: that machine resolves the id.
    // A person who wrote from the app or the command line gets the reply in the feed.
    let target = match &orig.from_host {
        Some(h) => {
            let harness = if send::is_person(&orig.from) { crate::model::Harness::Human } else { send::harness_of(orig.from_harness.as_deref()) };
            crate::peer::remote_target(h, orig.from_host_name.as_deref().unwrap_or(""), &orig.from, Some(orig.from_name.clone()), harness)
        }
        None => match send::resolve(&orig.from) {
            Ok(s) if is_uuid(&orig.from) || matches!(s.harness, crate::model::Harness::Grok | crate::model::Harness::Human) => s,
            _ => return Err(anyhow!("the sender \"{}\" is not a session, so it cannot be replied to", orig.from)),
        },
    };
    let rep = send::send(store, send::Request { to: target.clone(), from: orig.to.clone(), from_name: orig.to_name.clone(), text: text.to_string(), urgency: orig.urgency.clone(), dispatch_id: Some(orig.dispatch_id.clone()), wait: true, allow_resume: false, deadline: None, review: false })?;
    store.set_status(&mut orig, store::REPLIED)?;
    store.append_run(&orig.id, "reply", "sent", &rep.letter_id)?;
    Ok(json!({ "replied_to": orig.id, "to": { "id": target.id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail }))
}

/// A reply from the command line or MCP: as `reply`, except that a message written to the
/// person is answered only in the app. A reply goes out as the one the message was for, so
/// answering a message to the person from outside the app would speak as the person.
pub fn reply_as_agent(store: &Store, id: &str, text: &str) -> Result<Value> {
    let orig = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    if orig.to.eq_ignore_ascii_case("app") {
        return Err(anyhow!("{}", t("reply.person_only_in_app")));
    }
    reply(store, id, text)
}

/// `claude` / `codex` / `grok` / `human` as the label a person reads; None for anything else.
fn harness_label(code: Option<&str>) -> Option<&'static str> {
    use crate::model::Harness;
    Some(
        match code? {
            "claude" => Harness::Claude,
            "codex" => Harness::Codex,
            "grok" => Harness::Grok,
            "human" => Harness::Human,
            _ => return None,
        }
        .label(),
    )
}

/// The sender's harness: what the copy recorded, else what discovery says about the id now.
fn sender_label(c: &store::Letter) -> Option<&'static str> {
    harness_label(c.from_harness.as_deref()).or_else(|| harness_label(send::sender_harness(&c.from)))
}

/// The stage of a copy from its status alone: (i18n key, kind).
fn stage_by_status(status: &str) -> (&'static str, &'static str) {
    match status {
        store::REPLIED => ("stage.replied", "replied"),
        store::READ_BY_HOOK => ("stage.delivered_hook", "delivered"),
        store::DELIVERED_INBOX | store::DELIVERED_GROK | store::DELIVERED_FEED => ("stage.delivered", "delivered"),
        store::DELIVERED_CODEX => ("stage.queued_codex", "delivered"),
        store::FAILED => ("stage.failed", "failed"),
        store::CANCELLED => ("stage.cancelled", "cancelled"),
        store::HELD => ("stage.held", "held"),
        store::REVIEW => ("stage.review_asked", "review"),
        _ => ("stage.queued", "queued"),
    }
}

const STAGE_KEYS: [&str; 26] = ["stage.wait_open_codex", "stage.woken", "stage.review_asked", "stage.review_no_app", "stage.review_shown", "stage.review_closed", "stage.review_declined", "stage.review_expired", "stage.held", "stage.held_there", "stage.replied", "stage.finished", "stage.working", "stage.queued_after_turn", "stage.delivered_hook", "stage.delivered", "stage.queued_codex", "stage.failed", "stage.cancelled", "stage.queued", "stage.queued_until", "stage.wait_open", "stage.wait_turn_end", "stage.wait_speak", "stage.wait_codex", "stage.wait_peer"];

/// Why a pending copy waits, from the recipient's state right now: the session is closed, busy
/// (the hook hands it over when the turn ends), idle (its next turn), a Codex thread not on the
/// daemon, or a machine that is off.
fn waiting_key(c: &store::Letter) -> &'static str {
    if c.to_host.is_some() {
        return "stage.wait_peer";
    }
    let Some(s) = discover::local().into_iter().find(|s| s.id.eq_ignore_ascii_case(&c.to)) else { return "stage.wait_open" };
    match s.harness {
        crate::model::Harness::Codex => {
            if s.running && s.status.as_deref() == Some("loaded") {
                "stage.queued_until"
            } else {
                "stage.wait_codex"
            }
        }
        crate::model::Harness::Claude => {
            if !s.running {
                "stage.wait_open"
            } else if matches!(s.status.as_deref(), Some("busy") | Some("running")) {
                "stage.wait_turn_end"
            } else {
                "stage.wait_speak"
            }
        }
        _ => "stage.queued_until",
    }
}

/// The coarse state of a copy (`kind`: queued, delivered, working, replied, failed, cancelled) and
/// the i18n key of its stage text, from the store's status and, after a direct delivery here, the
/// transcript's receipt. A copy on another machine shows what that machine last said about it.
pub fn stage_of(store: &Store, c: &store::Letter) -> (String, String) {
    // held here for the user's review, wherever it is going: nothing has left this machine
    if c.status == store::REVIEW {
        return ("review".into(), crate::review::waiting_stage(store, &store.runs(&c.id)).into());
    }
    if c.status == store::CANCELLED {
        match crate::review::state(&store.runs(&c.id)).map(|(s, _)| s).as_deref() {
            Some("declined") => return ("cancelled".into(), "stage.review_declined".into()),
            Some("expired") => return ("failed".into(), "stage.review_expired".into()),
            _ => {}
        }
    }
    if c.to_host.is_some() {
        // held over there: waiting for a person on that machine, nothing to approve here
        if c.status == store::HELD {
            return ("queued".into(), "stage.held_there".into());
        }
        if let Some(remote) = crate::peer::remote_stage(&c.id) {
            if remote.0 == "held" {
                return ("queued".into(), "stage.held_there".into());
            }
            return remote;
        }
        let (key, kind) = stage_by_status(&c.status);
        let key = if kind == "queued" { waiting_key(c) } else { key };
        return (kind.into(), key.into());
    }
    // answered: nothing in a transcript can change that
    if c.status == store::REPLIED {
        return ("replied".into(), "stage.replied".into());
    }
    let runs = store.runs(&c.id);
    let delivered_inbox = runs.iter().any(|r| r.action == "inbox" && r.result == "written");
    let queued_codex = runs.iter().any(|r| r.action == "codex" && r.result == "queued");
    let sent_grok = runs.iter().find(|r| r.action == "grok" && r.result == "sent");
    // a turn seen finished stays finished: it is written down once and never looked up again
    let finished = runs.iter().any(|r| r.action == "receipt" && r.result == "completed");
    let receipt = if finished {
        Some(claude::Receipt::Completed { turn_id: String::new() })
    } else if delivered_inbox {
        claude::receipt(&c.to, None, &c.id).ok()
    } else if queued_codex {
        Some(match crate::wake::codex_receipt(store, c, &runs) {
            crate::codex::Receipt::Completed => claude::Receipt::Completed { turn_id: String::new() },
            crate::codex::Receipt::Started => claude::Receipt::Started,
            crate::codex::Receipt::Queued => claude::Receipt::Queued,
            crate::codex::Receipt::NotFound => claude::Receipt::NotFound,
        })
    } else if let Some(run) = sent_grok {
        let message_id = Some(run.detail.as_str()).filter(|d| !d.is_empty() && !d.starts_with('{'));
        // A direct message to a bot is not queued behind a turn: delivered, then answered or not.
        Some(match crate::grok::receipt(&c.to, message_id, &short_id(&c.id)) {
            crate::grok::Receipt::Answered => claude::Receipt::Completed { turn_id: String::new() },
            crate::grok::Receipt::Delivered | crate::grok::Receipt::NotFound => claude::Receipt::NotFound,
        })
    } else {
        None
    };
    if !finished && matches!(receipt, Some(claude::Receipt::Completed { .. })) {
        let _ = store.append_run(&c.id, "receipt", "completed", "");
    }
    let (key, kind) = match (&receipt, c.status.as_str()) {
        (_, store::REPLIED) => ("stage.replied", "replied"),
        (Some(claude::Receipt::Completed { .. }), _) => ("stage.finished", "replied"),
        (Some(claude::Receipt::Started), _) => ("stage.working", "working"),
        (Some(claude::Receipt::Queued), _) => ("stage.queued_after_turn", "delivered"),
        _ => stage_by_status(&c.status),
    };
    // in Codex's queue and untaken: nobody has the thread open (said once it is clear, after a while)
    let woken = crate::wake::woken(&runs);
    let key = if queued_codex && matches!(receipt, Some(claude::Receipt::NotFound)) && crate::wake::codex_left_alone(c, &runs) {
        if woken { "stage.woken" } else { "stage.wait_open_codex" }
    } else {
        key
    };
    let key = if kind == "queued" {
        match waiting_key(c) {
            "stage.wait_open" if woken => "stage.woken",
            k => k,
        }
    } else {
        key
    };
    (kind.into(), key.into())
}

/// One recipient copy with its stage (see `stage_of`), the machines involved, and its run log.
fn copy_json(store: &Store, c: &store::Letter, with_runs: bool) -> Value {
    let (kind, key) = stage_of(store, c);
    let deadline = store::time_text(c.deadline_at());
    let stage = if kind == "queued" {
        t(if STAGE_KEYS.contains(&key.as_str()) { &key } else { "stage.queued_until" }).replace("{t}", short_time(&deadline))
    } else if STAGE_KEYS.contains(&key.as_str()) {
        t(&key).to_string()
    } else {
        crate::i18n::status_label(&c.status).to_string()
    };
    let mut v = json!({ "letter_id": c.id, "short_id": short_id(&c.id), "from": c.from, "from_name": c.from_name, "from_harness": sender_label(c), "from_host_name": c.from_host_name, "to": c.to, "to_name": c.to_name, "to_harness": harness_label(c.to_harness.as_deref()), "to_host": c.to_host, "to_host_name": c.to_host_name, "status": c.status, "stage": stage, "kind": kind, "created": c.created, "deadline": deadline, "path": c.path, "body": c.body });
    let runs = store.runs(&c.id);
    if let Some(r) = crate::review::summary(store, &runs) {
        v["review"] = r;
    }
    if with_runs {
        v["runs"] = json!(runs);
    }
    v
}

/// `2026-11-08 16:30` → `16:30` today, `11-08 16:30` otherwise.
fn short_time(t: &str) -> &str {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if t.starts_with(&today) {
        t.get(11..).unwrap_or(t)
    } else {
        t.get(5..).unwrap_or(t)
    }
}

/// Where a message stands: its copies, each with a stage read from the store and the transcript.
pub fn status(store: &Store, id: &str) -> Result<Value> {
    let _ = crate::peer::mirror(store);
    let l = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    let mut copies = store.copies(&l.dispatch_id)?;
    reconcile(store, &mut copies)?;
    let rows: Vec<Value> = copies.iter().map(|c| copy_json(store, c, true)).collect();
    Ok(json!({ "dispatch_id": l.dispatch_id, "copies": rows }))
}

/// `status`, waiting up to `wait_secs` (at most two minutes) for any copy to move on: its kind,
/// stage or review step. Returns at once when nothing waits (all delivered, replied, failed,
/// cancelled) or when `wait_secs` is 0. `changed` says whether something moved.
pub fn status_wait(store: &Store, id: &str, wait_secs: u64) -> Result<Value> {
    let first = status(store, id)?;
    let key = |v: &Value| -> Vec<String> {
        v["copies"].as_array().into_iter().flatten().map(|c| format!("{}|{}|{}", c["kind"], c["stage"], c["review"]["state"])).collect()
    };
    let settled = |v: &Value| v["copies"].as_array().into_iter().flatten().all(|c| matches!(c["kind"].as_str(), Some("replied" | "failed" | "cancelled")));
    let wait = std::time::Duration::from_secs(wait_secs.min(120));
    if wait.is_zero() || settled(&first) {
        let mut v = first;
        v["changed"] = json!(false);
        return Ok(v);
    }
    let start = std::time::Instant::now();
    let before = key(&first);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let now = status(store, id)?;
        if key(&now) != before || start.elapsed() >= wait {
            let mut v = now;
            v["changed"] = json!(key(&v) != before);
            v["waited_secs"] = json!(start.elapsed().as_secs());
            return Ok(v);
        }
    }
}

/// The feed: every dispatch (one message, its recipients), newest first.
pub fn feed(store: &Store, limit: usize) -> Result<Value> {
    let _ = crate::peer::mirror(store);
    let mut letters = store.list(None, false)?;
    reconcile(store, &mut letters)?;
    let mut order: Vec<String> = vec![];
    let mut groups: std::collections::HashMap<String, Vec<&store::Letter>> = std::collections::HashMap::new();
    for l in &letters {
        let g = groups.entry(l.dispatch_id.clone()).or_default();
        if g.is_empty() {
            order.push(l.dispatch_id.clone());
        }
        g.push(l);
    }
    let mut out = vec![];
    for did in order.into_iter().take(limit) {
        let mut copies: Vec<&store::Letter> = groups[&did].clone();
        copies.sort_by(|a, b| a.created.cmp(&b.created).then(a.path.cmp(&b.path)));
        // The oldest copy is the sender's; the copies it sent are the recipients, the rest are replies
        // (a reply shares the dispatch id and goes back to the sender). A session that writes to
        // itself has one recipient copy with from == to; any later copy like it is its reply.
        let first = copies[0];
        let sender = first.from.clone();
        let mut sent: Vec<&store::Letter> = vec![];
        let mut replies: Vec<&store::Letter> = vec![];
        for c in &copies {
            if c.from == sender && (c.to != sender || !sent.iter().any(|s| s.to == c.to)) {
                sent.push(c);
            } else {
                replies.push(c);
            }
        }
        let recipients: Vec<Value> = sent
            .iter()
            .map(|c| {
                let mut v = copy_json(store, c, false);
                if let Some(r) = replies.iter().find(|r| r.from == c.to) {
                    v["reply"] = json!({ "letter_id": r.id, "body": r.body, "created": r.created, "status": r.status });
                    if v["kind"] != "replied" {
                        v["kind"] = json!("replied");
                        v["stage"] = json!(t("stage.replied"));
                    }
                }
                v
            })
            .collect();
        out.push(json!({ "dispatch_id": did, "from": sender, "from_name": first.from_name, "from_harness": sender_label(first), "from_host_name": first.from_host_name, "created": first.created, "urgency": first.urgency, "text": first.body, "recipients": recipients }));
    }
    Ok(json!({ "count": out.len(), "dispatches": out }))
}

/// One side of a conversation: a session (here or on a paired machine), the person at the app or
/// the command line (on any machine: one person), or a process that did not say who it is.
fn party_of(id: &str, stored_name: &str, harness: Option<&str>, host: Option<(&str, &str)>, index: &std::collections::HashMap<String, Session>, book: &[crate::contacts::Contact]) -> (String, Value) {
    if id == "app" {
        let me = crate::contacts::me();
        return ("you".into(), json!({ "key": "you", "id": id, "name": me.name.unwrap_or_else(|| t("party.you").to_string()), "app_name": null, "harness": crate::model::Harness::Human.label(), "host_id": null, "host_name": null, "person": true, "address": null, "running": false, "status": null, "cwd_short": null, "avatar": me.avatar, "contact": null, "about": null }));
    }
    // the command line or an MCP caller that no session was recognized behind: a person typing
    // or an agent, Dispatch cannot tell, so it is never shown as the person
    if id == "cli" || id == "mcp" {
        let key = format!("unknown|{id}");
        let name = t(if id == "cli" { "party.cli" } else { "party.mcp" });
        return (key.clone(), json!({ "key": key, "id": id, "name": name, "app_name": null, "harness": null, "host_id": null, "host_name": null, "person": false, "unknown": true, "address": null, "running": false, "status": null, "cwd_short": null, "avatar": null, "contact": null, "about": null }));
    }
    let host_id = host.map(|h| h.0).unwrap_or("");
    let key = format!("{host_id}|{}", id.to_lowercase());
    let s = index.get(&key);
    // a name a command line gave itself, with no session of that id behind it: shown as the claim it is
    if s.is_none() && !crate::send::recognized_sender(id) {
        let name = t("party.claimed").replace("{name}", stored_name);
        return (key.clone(), json!({ "key": key, "id": id, "name": name, "app_name": null, "harness": null, "host_id": host.map(|h| h.0), "host_name": host.map(|h| h.1), "person": false, "unknown": true, "address": null, "running": false, "status": null, "cwd_short": null, "avatar": null, "contact": null, "about": null }));
    }
    let name = s.map(|s| s.display_name()).unwrap_or_else(|| stored_name.to_string());
    let harness = s.map(|s| s.harness.label()).or_else(|| harness_label(harness));
    let address = if id == "mcp" { None } else { Some(match host { Some((h, _)) => format!("{h}/{id}"), None => id.to_string() }) };
    let contact = book.iter().find(|c| c.session_id.eq_ignore_ascii_case(id) && c.host.as_ref().map(|h| h.id.as_str()) == host.map(|h| h.0));
    let v = json!({
        "key": key,
        "id": id,
        "name": name,
        "app_name": s.and_then(|s| s.app_name.clone().or_else(|| s.name.clone())),
        "harness": harness,
        "host_id": host.map(|h| h.0),
        "host_name": host.map(|h| h.1),
        "person": false,
        "address": address,
        "running": s.map(|s| s.running).unwrap_or(false),
        "status": s.and_then(|s| s.status.clone()),
        "cwd_short": s.and_then(|s| s.cwd.as_deref().map(short_cwd)),
        "avatar": contact.and_then(|c| c.avatar.clone()),
        "contact": contact.filter(|c| !c.provisional).map(|c| c.name.clone()),
        "about": contact.and_then(|c| c.description.clone()),
    });
    (key, v)
}

/// When a copy came to be on this machine's clock: its first run (written here, or received),
/// else its `created` read as local time.
fn happened_at(store: &Store, l: &store::Letter) -> chrono::DateTime<chrono::Local> {
    store
        .runs(&l.id)
        .first()
        .and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.at).ok())
        .map(|t| t.with_timezone(&chrono::Local))
        .or_else(|| store::parse_time(&l.created).and_then(|t| t.and_local_timezone(chrono::Local).earliest()))
        .unwrap_or_else(chrono::Local::now)
}

/// How much a copy needs the user: review first, then not delivered, held, waiting.
fn attention_rank(kind: &str) -> Option<u8> {
    match kind {
        "review" => Some(0),
        "failed" => Some(1),
        "held" => Some(2),
        "queued" => Some(3),
        _ => None,
    }
}

/// The conversations: one per pair of parties who wrote to each other, whatever the dispatch,
/// newest activity first. Each has both parties as equals, every message between them in order
/// (with who else a message went to), and the most pressing state that needs the user, if any.
pub fn threads(store: &Store, limit: usize) -> Result<Value> {
    let _ = crate::peer::mirror(store);
    let mut letters = store.list(None, false)?;
    reconcile(store, &mut letters)?;
    let mut index: std::collections::HashMap<String, Session> = std::collections::HashMap::new();
    for s in discover::sessions() {
        let host = s.host.as_ref().map(|h| h.id.clone()).unwrap_or_default();
        index.insert(format!("{host}|{}", s.id.to_lowercase()), s);
    }
    let book = crate::contacts::list();
    // a machine paired again under the same name is one machine: its old records join the new one
    let peers = crate::peer::peers().unwrap_or_default();
    let mut hosts: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for (i, p) in peers.iter().enumerate() {
        let latest = peers[i..].iter().rev().find(|x| x.name.eq_ignore_ascii_case(&p.name)).unwrap_or(p);
        hosts.insert(p.id.clone(), (latest.id.clone(), latest.name.clone()));
    }
    let host_of = |id: Option<&str>, name: Option<&str>| -> Option<(String, String)> {
        let id = id?;
        Some(hosts.get(id).cloned().unwrap_or_else(|| (id.to_string(), name.unwrap_or("").to_string())))
    };
    // A copy's `created` is the writing machine's wall clock, with no zone; two machines in
    // different zones would interleave wrongly. Its first run here (written, or received from
    // the other machine) carries the zone: that is when it happened on this machine's clock.
    let when: std::collections::HashMap<String, chrono::DateTime<chrono::Local>> = letters.iter().map(|l| (l.id.clone(), happened_at(store, l))).collect();
    letters.sort_by(|a, b| when[&a.id].cmp(&when[&b.id]).then(a.path.cmp(&b.path)));
    // the copies of each dispatch, to tell who else a message went to
    let mut by_dispatch: std::collections::HashMap<&str, Vec<&store::Letter>> = std::collections::HashMap::new();
    for l in &letters {
        by_dispatch.entry(l.dispatch_id.as_str()).or_default().push(l);
    }
    struct Thread {
        a: Value,
        b: Value,
        a_key: String,
        messages: Vec<Value>,
        last_at: String,
    }
    let mut order: Vec<String> = vec![];
    let mut threads: std::collections::HashMap<String, Thread> = std::collections::HashMap::new();
    for l in &letters {
        let from_host = host_of(l.from_host.as_deref(), l.from_host_name.as_deref());
        let to_host = host_of(l.to_host.as_deref(), l.to_host_name.as_deref());
        let (fk, fv) = party_of(&l.from, &l.from_name, l.from_harness.as_deref(), from_host.as_ref().map(|(i, n)| (i.as_str(), n.as_str())), &index, &book);
        let (tk, tv) = party_of(&l.to, &l.to_name, l.to_harness.as_deref(), to_host.as_ref().map(|(i, n)| (i.as_str(), n.as_str())), &index, &book);
        let key = if fk <= tk { format!("{fk}\u{1f}{tk}") } else { format!("{tk}\u{1f}{fk}") };
        let th = threads.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Thread { a: fv.clone(), b: tv.clone(), a_key: fk.clone(), messages: vec![], last_at: String::new() }
        });
        let mut m = copy_json(store, l, false);
        m["dispatch_id"] = json!(l.dispatch_id);
        m["side"] = json!(if fk == th.a_key { "a" } else { "b" });
        // the same message sent to others as well
        let others: Vec<Value> = by_dispatch
            .get(l.dispatch_id.as_str())
            .into_iter()
            .flatten()
            .filter(|o| o.dispatch_id == l.dispatch_id && o.from == l.from && o.id != l.id && !(o.to == l.to && o.to_host == l.to_host) && o.body == l.body)
            .map(|o| json!(match &o.to_host_name { Some(h) => format!("{} @ {h}", o.to_name), None => o.to_name.clone() }))
            .collect();
        if !others.is_empty() {
            m["also_to"] = json!(others);
        }
        let at = when[&l.id].format("%Y-%m-%d %H:%M").to_string();
        m["at"] = json!(at);
        th.last_at = at;
        th.messages.push(m);
    }
    let mut out: Vec<Value> = order
        .into_iter()
        .filter_map(|k| threads.remove(&k).map(|t| (k, t)))
        .map(|(key, t)| {
            // what needs the user now: anything held for review, and whatever has not gone through
            // since the last message that did (an old failure the talk has moved past is history)
            let went = t.messages.iter().rposition(|m| matches!(m["kind"].as_str(), Some("delivered" | "working" | "replied"))).map(|i| i + 1).unwrap_or(0);
            let attention = t.messages.iter().enumerate().filter(|(i, m)| *i >= went || m["kind"] == "review").filter_map(|(_, m)| m["kind"].as_str().and_then(|k| attention_rank(k).map(|r| (r, k.to_string())))).min().map(|(_, k)| k);
            let last = t.messages.last().cloned().unwrap_or(Value::Null);
            let count = t.messages.len();
            let keep = count.saturating_sub(60);
            json!({ "key": key, "a": t.a, "b": t.b, "last_at": t.last_at, "count": count, "attention": attention, "last": { "side": last["side"], "body": last["body"], "kind": last["kind"], "stage": last["stage"] }, "messages": t.messages[keep..].to_vec() })
        })
        .collect();
    out.sort_by(|x, y| y["last_at"].as_str().cmp(&x["last_at"].as_str()));
    out.truncate(limit);
    Ok(json!({ "count": out.len(), "threads": out }))
}

/// One session's page: who it is, the dispatches it took part in (each marked as received or
/// sent from its point of view), and the last turns of its own record, with the messages
/// Dispatch handed over marked. A session on a peer is asked for over the wire.
pub fn session_page(store: &Store, query: &str) -> Result<Value> {
    let s = send::resolve(query)?;
    let hook_on = hook::installed();
    let book = crate::contacts::list();
    let session = session_json_with(&s, hook_on, &book);
    let all = feed(store, 500)?;
    let empty = vec![];
    let mut dispatches = vec![];
    for d in all["dispatches"].as_array().unwrap_or(&empty) {
        let from_it = d["from"].as_str().map(|f| f.eq_ignore_ascii_case(&s.id)).unwrap_or(false);
        let to_it = d["recipients"].as_array().map(|rs| rs.iter().any(|r| r["to"].as_str().map(|t| t.eq_ignore_ascii_case(&s.id)).unwrap_or(false))).unwrap_or(false);
        if from_it || to_it {
            let mut v = d.clone();
            v["direction"] = json!(if from_it { "out" } else { "in" });
            dispatches.push(v);
        }
    }
    let (turns, readable) = match &s.host {
        Some(h) => (crate::peer::transcript(h, &s.id, 80).unwrap_or_default(), true),
        None => (crate::transcript::recent(&s, 80), s.transcript.is_some()),
    };
    let rounds = crate::transcript::rounds(&turns, 16);
    Ok(json!({ "session": session, "dispatches": dispatches, "rounds": rounds, "transcript_available": readable }))
}

/// How to open a session on this machine: a URL the system opens, or a command to copy.
pub fn open_session(session_id: &str) -> Result<Value> {
    let s = send::resolve(session_id)?;
    if let Some(h) = &s.host {
        return Ok(json!({ "kind": "note", "exact": false, "note": t("open.remote_note").replace("{host}", &h.name) }));
    }
    match s.harness {
        crate::model::Harness::Claude => {
            if let Some(host) = s.host_session_id.as_deref().filter(|_| s.desktop) {
                return Ok(json!({ "kind": "url", "url": format!("claude://code/continue?session={host}"), "exact": true }));
            }
            Ok(json!({ "kind": "command", "command": format!("claude --resume {}", s.id), "cwd": s.cwd, "running": s.running, "exact": false, "note": t("open.terminal_note") }))
        }
        crate::model::Harness::Codex => Ok(json!({ "kind": "url", "url": format!("codex://threads/{}", s.id), "exact": true })),
        crate::model::Harness::Grok => Ok(json!({ "kind": "app", "app": "Grok Bot", "exact": false, "note": t("open.grok_note").replace("{name}", &s.display_name()) })),
        _ => Err(anyhow!("{}", t("open.unknown"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Letter, Store, CANCELLED, FAILED, UNREAD};

    fn letter(s: &Store, created: &str, deadline: Option<&str>, status: &str) -> Letter {
        let mut l = Letter {
            id: uuid::Uuid::new_v4().to_string(),
            dispatch_id: "d-test".into(),
            from: "app".into(),
            from_name: "me".into(),
            to: "11111111-2222-3333-4444-555555555555".into(),
            to_name: "T".into(),
            urgency: "普通".into(),
            wake: true,
            status: status.into(),
            created: created.into(),
            deadline: deadline.map(str::to_string),
            to_harness: Some("claude".into()),
            from_host: None,
            from_host_name: None,
            to_host: None,
            to_host_name: None,
            from_harness: None,
            signature: None,
            body: "hi".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        l
    }

    #[test]
    fn conversations_are_pairs_whatever_the_dispatch() {
        use crate::store::{DELIVERED_INBOX, REPLIED};
        let dir = std::env::temp_dir().join(format!("dispatch-ops-threads-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let s = Store::at(dir.clone());
        let (a, b, c) = ("aaaaaaaa-1111-4222-8333-444455556666", "bbbbbbbb-1111-4222-8333-444455556666", "cccccccc-1111-4222-8333-444455556666");
        let mk = |did: &str, from: &str, from_name: &str, to: &str, to_name: &str, body: &str, created: &str, status: &str| -> String {
            let mut l = Letter {
                id: uuid::Uuid::new_v4().to_string(),
                dispatch_id: did.into(),
                from: from.into(),
                from_name: from_name.into(),
                to: to.into(),
                to_name: to_name.into(),
                urgency: "普通".into(),
                wake: true,
                status: status.into(),
                created: created.into(),
                deadline: Some("2999-01-01 00:00".into()),
                to_harness: Some("claude".into()),
                from_host: None,
                from_host_name: None,
                to_host: None,
                to_host_name: None,
                from_harness: Some("claude".into()),
                signature: None,
                body: body.into(),
                path: String::new(),
            };
            s.write_letter(&mut l).unwrap();
            l.id
        };
        mk("d0", "app", "me", a, "A", "from you", "2026-11-08 08:00", DELIVERED_INBOX);
        mk("d1", a, "A", b, "B", "hello", "2026-11-08 09:00", REPLIED);
        mk("d1", b, "B", a, "A", "hi back", "2026-11-08 09:01", DELIVERED_INBOX);
        mk("d2", a, "A", b, "B", "second", "2026-11-08 10:00", FAILED);
        mk("d2", a, "A", c, "C", "second", "2026-11-08 10:00", UNREAD);
        mk("d3", b, "B", a, "A", "third", "2026-11-08 11:00", DELIVERED_INBOX);
        let v = threads(&s, 10).unwrap();
        let list = v["threads"].as_array().unwrap();
        assert_eq!(list.len(), 3, "A-B, A-C and you-A: {v:#}");
        let ab = &list[0];
        assert_eq!(ab["count"], 4, "both directions and both dispatches in one conversation");
        let bodies: Vec<&str> = ab["messages"].as_array().unwrap().iter().map(|m| m["body"].as_str().unwrap()).collect();
        assert_eq!(bodies, ["hello", "hi back", "second", "third"]);
        let sides: Vec<&str> = ab["messages"].as_array().unwrap().iter().map(|m| m["side"].as_str().unwrap()).collect();
        assert_eq!(sides, ["a", "b", "a", "b"], "a is whoever wrote first");
        assert_eq!(ab["messages"][2]["also_to"], json!(["C"]));
        assert_eq!(ab["attention"], Value::Null, "the failure was followed by a message that went through");
        let ac = list.iter().find(|t| t["b"]["name"] == "C").unwrap();
        assert_eq!(ac["attention"], "queued");
        let you = list.iter().find(|t| t["a"]["person"] == true).unwrap();
        assert_eq!(you["a"]["name"], t("party.you"));
        assert_eq!(you["a"]["address"], Value::Null, "the person is not a session to write to");

        // the command line with no session behind it is not the person
        mk("d4", "cli", "cli", a, "A", "from a terminal", "2026-11-08 12:00", DELIVERED_INBOX);
        let to_you = mk("d5", a, "A", "app", "me", "to you", "2026-11-08 12:30", DELIVERED_INBOX);
        let v = threads(&s, 10).unwrap();
        let list = v["threads"].as_array().unwrap();
        assert_eq!(list.len(), 4, "the unrecognized command line is a party of its own: {v:#}");
        let unknown = list.iter().find(|t| t["a"]["id"] == "cli").unwrap();
        assert_eq!(unknown["a"]["name"], t("party.cli"));
        assert_eq!(unknown["a"]["person"], false);
        assert_eq!(list.iter().filter(|t| t["a"]["person"] == true || t["b"]["person"] == true).count(), 1, "you and A, both ways, one conversation");
        crate::contacts::set_me(Some("yyt".into()), crate::contacts::Look { emoji: Some("🦊".into()), color: None }).unwrap();
        let v = threads(&s, 10).unwrap();
        let you = v["threads"].as_array().unwrap().iter().find(|t| t["a"]["person"] == true || t["b"]["person"] == true).unwrap().clone();
        let me = if you["a"]["person"] == true { &you["a"] } else { &you["b"] };
        assert_eq!((me["name"].as_str(), me["avatar"]["emoji"].as_str()), (Some("yyt"), Some("🦊")), "the person's own name and picture");

        // a message to the person is answered in the app only, never as the person from outside
        let e = reply_as_agent(&s, &to_you, "as you").unwrap_err().to_string();
        assert_eq!(e, t("reply.person_only_in_app"));

        // a name a command line gave itself is shown as the claim it is
        mk("d6", "Test Bot", "Test Bot", a, "A", "a claim", "2026-11-08 13:00", DELIVERED_INBOX);
        let v = threads(&s, 10).unwrap();
        let claim = v["threads"].as_array().unwrap().iter().find(|t| t["a"]["id"] == "Test Bot" || t["b"]["id"] == "Test Bot").unwrap().clone();
        let side = if claim["a"]["id"] == "Test Bot" { &claim["a"] } else { &claim["b"] };
        assert_eq!(side["name"], t("party.claimed").replace("{name}", "Test Bot"));
        assert_eq!(side["unknown"], true);
        assert_eq!(side["address"], Value::Null);
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A turn seen finished is written into the copy's log, and from then on its stage needs no
    /// transcript: here the session does not even exist.
    #[test]
    fn a_finished_turn_is_remembered_without_its_transcript() {
        use crate::store::{Letter, DELIVERED_INBOX, REPLIED};
        let dir = std::env::temp_dir().join(format!("dispatch-finished-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let mk = |status: &str| {
            let mut l = Letter {
                id: uuid::Uuid::new_v4().to_string(),
                dispatch_id: uuid::Uuid::new_v4().to_string(),
                from: "aaaaaaaa-1111-4222-8333-444455556666".into(),
                from_name: "A".into(),
                to: "bbbbbbbb-1111-4222-8333-444455556666".into(),
                to_name: "B".into(),
                urgency: "普通".into(),
                wake: true,
                status: status.into(),
                created: crate::store::now_text(),
                deadline: None,
                to_harness: Some("claude".into()),
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
            s.append_run(&l.id, "inbox", "written", "").unwrap();
            l
        };
        let delivered = mk(DELIVERED_INBOX);
        assert_eq!(stage_of(&s, &delivered), ("delivered".to_string(), "stage.delivered".to_string()), "no transcript to read: it stands as delivered");
        s.append_run(&delivered.id, "receipt", "completed", "").unwrap();
        assert_eq!(stage_of(&s, &delivered), ("replied".to_string(), "stage.finished".to_string()));
        assert_eq!(s.runs(&delivered.id).iter().filter(|r| r.action == "receipt").count(), 1, "written down once");
        let replied = mk(REPLIED);
        assert_eq!(stage_of(&s, &replied), ("replied".to_string(), "stage.replied".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_codex_thread_named_on_the_call_is_the_sender() {
        let id = "019a0000-1111-7222-8333-444455556666";
        set_mcp_thread(Some(id.to_uppercase()));
        let c = caller().unwrap();
        assert_eq!(c.id, id);
        assert_eq!(c.harness, crate::model::Harness::Codex);
        assert_eq!(caller_identity().0, id);
        set_mcp_thread(Some("not-an-id".into()));
        assert_ne!(caller().map(|c| c.id).as_deref(), Some("not-an-id"));
        set_mcp_thread(None);
    }

    #[test]
    fn a_dispatch_to_oneself_shows_one_recipient_and_its_reply() {
        let dir = std::env::temp_dir().join(format!("dispatch-ops-self-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let me = "22222222-3333-4444-8555-666666666666";
        let mk = |body: &str, created: &str| {
            let mut l = Letter {
                id: uuid::Uuid::new_v4().to_string(),
                dispatch_id: "d-self".into(),
                from: me.into(),
                from_name: "Me".into(),
                to: me.into(),
                to_name: "Me".into(),
                urgency: "普通".into(),
                wake: true,
                status: UNREAD.into(),
                created: created.into(),
                deadline: Some("2999-01-01 00:00".into()),
                to_harness: Some("claude".into()),
                from_host: None,
                from_host_name: None,
                to_host: None,
                to_host_name: None,
                from_harness: Some("claude".into()),
                signature: None,
                body: body.into(),
                path: String::new(),
            };
            s.write_letter(&mut l).unwrap();
            l
        };
        mk("note to self", "2026-11-08 09:00");
        mk("done", "2026-11-08 09:05");
        let v = feed(&s, 10).unwrap();
        let d = &v["dispatches"][0];
        assert_eq!(d["recipients"].as_array().unwrap().len(), 1, "one recipient, not two");
        assert_eq!(d["recipients"][0]["reply"]["body"], "done");
        assert_eq!(d["recipients"][0]["kind"], "replied");
        assert_eq!(d["from_harness"], "Claude");
        assert_eq!(d["recipients"][0]["to_harness"], "Claude");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn overdue_copies_fail_and_can_be_cancelled_but_not_retried_once_cancelled() {
        let dir = std::env::temp_dir().join(format!("dispatch-ops-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let old = letter(&s, "2020-01-01 10:00", None, UNREAD);
        let fresh = letter(&s, "2020-01-01 10:00", Some("2999-01-01 10:00"), UNREAD);
        let mut all = s.list(None, false).unwrap();
        assert_eq!(reconcile(&s, &mut all).unwrap(), 1);
        let got = s.list(None, false).unwrap();
        assert_eq!(got.iter().find(|l| l.id == old.id).unwrap().status, FAILED);
        assert_eq!(got.iter().find(|l| l.id == fresh.id).unwrap().status, UNREAD);
        assert!(s.runs(&old.id).iter().any(|r| r.action == "deadline" && r.result == "failed"));
        // a failed copy is still pending: the hook would hand it over, a cancel withdraws it
        assert_eq!(s.list(Some(&old.to), true).unwrap().len(), 2);
        let v = cancel(&s, &old.id[..8], "test").unwrap();
        assert_eq!(v["cancelled"], 1);
        assert_eq!(s.find(&old.id).unwrap().unwrap().status, CANCELLED);
        assert!(cancel(&s, &old.id, "test").is_err());
        assert!(retry(&s, &old.id).is_err());
        // by dispatch id: only the pending copy is cancelled
        let v = cancel(&s, "d-test", "test").unwrap();
        assert_eq!(v["cancelled"], 1);
        assert_eq!(s.find(&fresh.id).unwrap().unwrap().status, CANCELLED);
        let _ = std::fs::remove_dir_all(dir);
    }
}
