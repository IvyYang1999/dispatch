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

fn session_json_with(s: &Session, hook_on: bool, contacts: &[crate::contacts::Contact]) -> Value {
    let r = reach::decide(s, hook_on);
    let host_id = s.host.as_ref().map(|h| h.id.as_str());
    let contact = contacts.iter().find(|c| c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id);
    json!({
        "contact": contact.map(|c| c.name.clone()),
        "about": contact.and_then(|c| c.description.clone()),
        "provisional": contact.map(|c| c.provisional).unwrap_or(false),
        "id": s.id,
        "short_id": s.short_id,
        "name": s.display_name(),
        "harness": s.harness.label(),
        "cwd": s.cwd,
        "cwd_short": s.cwd.as_deref().map(short_cwd),
        "running": s.running,
        "status": s.status,
        "desktop": s.desktop,
        "host_session_id": s.host_session_id,
        "last_active": s.last_active,
        "host": s.host,
        "address": s.address(),
        "reach": { "level": r.level, "text": r.text, "fix": r.fix },
    })
}

/// Who is calling: the Claude Code session this process was spawned by, if any.
pub fn whoami() -> Value {
    let me = crate::peer::me();
    let machine = json!({ "id": me.id, "name": me.name });
    match claude::caller_session() {
        Some(i) => {
            let contact = crate::contacts::for_session(&i.session_id, None).map(|c| c.name);
            let handle = contact.clone().unwrap_or_else(|| short_id(&i.session_id));
            json!({ "session_id": i.session_id, "short_id": short_id(&i.session_id), "name": i.name.clone().unwrap_or_else(|| short_id(&i.session_id)), "contact": contact, "harness": "Claude", "cwd": i.cwd, "desktop": i.desktop, "machine": machine, "address_elsewhere": format!("{}/{}", me.name, handle) })
        }
        None => json!({ "session_id": null, "name": "unknown", "harness": null, "machine": machine }),
    }
}

/// The caller's identity for `from`: (id, name).
pub fn caller_identity() -> (String, String) {
    match claude::caller_session() {
        Some(i) => (i.session_id.clone(), i.name.unwrap_or_else(|| short_id(&i.session_id))),
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
            let i = claude::caller_session().ok_or_else(|| anyhow!("no session given and the caller is not a Claude Code session; pass the session's id or name"))?;
            discover::lookup(&i.session_id).unwrap_or_else(|| {
                let mut s = crate::peer::remote_target("", "", &i.session_id, i.name.clone(), crate::model::Harness::Claude);
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

/// Every session that matches, each with why it matched.
pub fn find(query: &str) -> Value {
    let out = matches(query, discover::sessions());
    json!({ "query": query, "count": out.len(), "candidates": out, "note": "Send only when exactly one candidate is the right one; otherwise ask the user or refine the query." })
}

/// Every session this machine has records of (any age) or a paired machine lists, that matches.
pub fn search(query: &str) -> Value {
    let mut all = discover::all_local();
    all.extend(crate::peer::remote_sessions());
    let out = matches(query, all);
    json!({ "query": query, "count": out.len(), "sessions": out })
}

fn matches(query: &str, sessions: Vec<Session>) -> Vec<Value> {
    let (host, q) = match crate::peer::split_address(query.trim()) {
        Some((p, rest)) => (Some(p), rest.to_lowercase()),
        None => (None, query.trim().to_lowercase()),
    };
    let hook_on = hook::installed();
    let book = crate::contacts::list();
    let mut out = vec![];
    for s in sessions {
        if let Some(p) = &host {
            if s.host.as_ref().map(|h| h.id != p.id).unwrap_or(true) {
                continue;
            }
        }
        let host_id = s.host.as_ref().map(|h| h.id.as_str());
        let contact = book.iter().find(|c| c.session_id.eq_ignore_ascii_case(&s.id) && c.host.as_ref().map(|h| h.id.as_str()) == host_id);
        let reason = if q.is_empty() {
            Some("all")
        } else if s.id.starts_with(&q) {
            Some("id")
        } else if s.name.as_deref().map(|n| n.to_lowercase().contains(&q)).unwrap_or(false) || contact.map(|c| c.name.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("name")
        } else if contact.and_then(|c| c.description.as_deref()).map(|d| d.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("about")
        } else if s.cwd.as_deref().map(|c| c.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("cwd")
        } else if s.title.as_deref().map(|t| t.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("title")
        } else if s.host.is_some() && s.address().to_lowercase().contains(&q) {
            Some("host")
        } else {
            None
        };
        if let Some(r) = reason {
            let mut v = session_json_with(&s, hook_on, &book);
            v["reason"] = json!(r);
            out.push(v);
        }
    }
    out
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
        let rep = send::send(store, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: text.to_string(), urgency: urgency.to_string(), dispatch_id: Some(dispatch_id.clone()), wait: true, allow_resume, deadline })?;
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
    let (_, q) = crate::peer::split_address(query.trim()).map(|(p, rest)| (Some(p), rest)).unwrap_or((None, query.trim().to_string()));
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
    for mut l in copies.iter().filter(|l| store::pending(&l.status)).cloned() {
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
    for mut l in copies.iter().filter(|l| store::pending(&l.status)).cloned() {
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
            if was_handed(&runs) || last_run_age_secs(&runs) < 300 {
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
        None => claude::caller_session().map(|i| i.session_id).ok_or_else(|| anyhow!("no session given and the caller is not a Claude Code session"))?,
    };
    let letters = store.list(Some(&sid), !all)?;
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
            let harness = if send::is_person(&orig.from) { crate::model::Harness::Human } else { crate::model::Harness::Claude };
            crate::peer::remote_target(h, orig.from_host_name.as_deref().unwrap_or(""), &orig.from, Some(orig.from_name.clone()), harness)
        }
        None => match send::resolve(&orig.from) {
            Ok(s) if is_uuid(&orig.from) || matches!(s.harness, crate::model::Harness::Grok | crate::model::Harness::Human) => s,
            _ => return Err(anyhow!("the sender \"{}\" is not a session, so it cannot be replied to", orig.from)),
        },
    };
    let rep = send::send(store, send::Request { to: target.clone(), from: orig.to.clone(), from_name: orig.to_name.clone(), text: text.to_string(), urgency: orig.urgency.clone(), dispatch_id: Some(orig.dispatch_id.clone()), wait: true, allow_resume: false, deadline: None })?;
    store.set_status(&mut orig, store::REPLIED)?;
    store.append_run(&orig.id, "reply", "sent", &rep.letter_id)?;
    Ok(json!({ "replied_to": orig.id, "to": { "id": target.id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail }))
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
        _ => ("stage.queued", "queued"),
    }
}

const STAGE_KEYS: [&str; 16] = ["stage.replied", "stage.finished", "stage.working", "stage.queued_after_turn", "stage.delivered_hook", "stage.delivered", "stage.queued_codex", "stage.failed", "stage.cancelled", "stage.queued", "stage.queued_until", "stage.wait_open", "stage.wait_turn_end", "stage.wait_speak", "stage.wait_codex", "stage.wait_peer"];

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
    if c.to_host.is_some() {
        if let Some(remote) = crate::peer::remote_stage(&c.id) {
            return remote;
        }
        let (key, kind) = stage_by_status(&c.status);
        let key = if kind == "queued" { waiting_key(c) } else { key };
        return (kind.into(), key.into());
    }
    let runs = store.runs(&c.id);
    let delivered_inbox = runs.iter().any(|r| r.action == "inbox" && r.result == "written");
    let queued_codex = runs.iter().any(|r| r.action == "codex" && r.result == "queued");
    let sent_grok = runs.iter().find(|r| r.action == "grok" && r.result == "sent");
    let receipt = if delivered_inbox {
        claude::receipt(&c.to, None, &c.id).ok()
    } else if queued_codex {
        Some(match crate::codex::receipt(&c.to, &short_id(&c.id)) {
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
    let (key, kind) = match (&receipt, c.status.as_str()) {
        (_, store::REPLIED) => ("stage.replied", "replied"),
        (Some(claude::Receipt::Completed { .. }), _) => ("stage.finished", "replied"),
        (Some(claude::Receipt::Started), _) => ("stage.working", "working"),
        (Some(claude::Receipt::Queued), _) => ("stage.queued_after_turn", "delivered"),
        _ => stage_by_status(&c.status),
    };
    let key = if kind == "queued" { waiting_key(c) } else { key };
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
    let mut v = json!({ "letter_id": c.id, "short_id": short_id(&c.id), "from": c.from, "from_name": c.from_name, "from_harness": sender_label(c), "from_host_name": c.from_host_name, "to": c.to, "to_name": c.to_name, "to_harness": harness_label(c.to_harness.as_deref()), "to_host_name": c.to_host_name, "status": c.status, "stage": stage, "kind": kind, "created": c.created, "deadline": deadline, "path": c.path, "body": c.body });
    if with_runs {
        v["runs"] = json!(store.runs(&c.id));
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
        Some(h) => (crate::peer::transcript(h, &s.id, 24).unwrap_or_default(), true),
        None => (crate::transcript::recent(&s, 24), s.transcript.is_some()),
    };
    Ok(json!({ "session": session, "dispatches": dispatches, "turns": turns, "transcript_available": readable }))
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
            body: "hi".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        l
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
