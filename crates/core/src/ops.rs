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

fn session_json(s: &Session, hook_on: bool) -> Value {
    let r = reach::decide(s, hook_on);
    json!({
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
        "reach": { "level": r.level, "text": r.text, "fix": r.fix },
    })
}

/// Who is calling: the Claude Code session this process was spawned by, if any.
pub fn whoami() -> Value {
    match claude::caller_session() {
        Some(i) => json!({ "session_id": i.session_id, "short_id": short_id(&i.session_id), "name": i.name.clone().unwrap_or_else(|| short_id(&i.session_id)), "harness": "Claude", "cwd": i.cwd, "desktop": i.desktop }),
        None => json!({ "session_id": null, "name": "unknown", "harness": null }),
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
    let list: Vec<Value> = discover::sessions().iter().filter(|s| !running_only || s.running).map(|s| session_json(s, hook_on)).collect();
    json!({ "hook_installed": hook_on, "sessions": list })
}

/// Every session that matches, each with why it matched.
pub fn find(query: &str) -> Value {
    let q = query.trim().to_lowercase();
    let hook_on = hook::installed();
    let mut out = vec![];
    for s in discover::sessions() {
        let reason = if !q.is_empty() && s.id.starts_with(&q) {
            Some("id")
        } else if s.name.as_deref().map(|n| n.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("name")
        } else if s.cwd.as_deref().map(|c| c.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("cwd")
        } else if s.title.as_deref().map(|t| t.to_lowercase().contains(&q)).unwrap_or(false) {
            Some("title")
        } else {
            None
        };
        if let Some(r) = reason {
            let mut v = session_json(&s, hook_on);
            v["reason"] = json!(r);
            out.push(v);
        }
    }
    json!({ "query": query, "count": out.len(), "candidates": out, "note": "Send only when exactly one candidate is the right one; otherwise ask the user or refine the query." })
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
        let target = send::resolve(who)?;
        let rep = send::send(store, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: text.to_string(), urgency: urgency.to_string(), dispatch_id: Some(dispatch_id.clone()), wait: true, allow_resume, deadline })?;
        reports.push(json!({ "to": { "id": target.id, "short_id": target.short_id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail, "path": rep.path }));
    }
    Ok(json!({ "dispatch_id": dispatch_id, "from": { "id": from_id, "name": from_name }, "recipients": reports }))
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
    runs.iter().any(|r| matches!((r.action.as_str(), r.result.as_str()), ("inbox", "written") | ("codex", "queued") | ("grok", "sent") | ("hook", "handed")))
}

fn last_run_age_secs(runs: &[store::Run]) -> i64 {
    runs.last().and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.at).ok()).map(|at| (chrono::Local::now().fixed_offset() - at).num_seconds()).unwrap_or(i64::MAX)
}

/// The postman's round: expire overdue copies, and deliver queued copies whose route is open now
/// (the session came back, the daemon loaded the thread, the gateway answers). Copies marked not
/// delivered are left for a person to retry; a copy is not tried again within five minutes.
pub fn sweep(store: &Store) -> Result<Value> {
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
        .map(|l| json!({ "id": l.id, "short_id": short_id(&l.id), "dispatch_id": l.dispatch_id, "from": l.from, "from_name": l.from_name, "status": l.status, "created": l.created, "urgency": l.urgency, "body": l.body, "path": l.path, "can_reply": is_uuid(&l.from) || known.iter().any(|k| k.eq_ignore_ascii_case(&l.from)) }))
        .collect();
    Ok(json!({ "session_id": sid, "unread_only": !all, "count": rows.len(), "messages": rows }))
}

pub fn reply(store: &Store, id: &str, text: &str) -> Result<Value> {
    let mut orig = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    // A session id (Claude, Codex) or a bot id (Grok) can be answered; "cli", "app" and names cannot.
    let target = match send::resolve(&orig.from) {
        Ok(s) if is_uuid(&orig.from) || s.harness == crate::model::Harness::Grok => s,
        _ => return Err(anyhow!("the sender \"{}\" is not a session, so it cannot be replied to", orig.from)),
    };
    let rep = send::send(store, send::Request { to: target.clone(), from: orig.to.clone(), from_name: orig.to_name.clone(), text: text.to_string(), urgency: orig.urgency.clone(), dispatch_id: Some(orig.dispatch_id.clone()), wait: true, allow_resume: false, deadline: None })?;
    store.set_status(&mut orig, store::REPLIED)?;
    store.append_run(&orig.id, "reply", "sent", &rep.letter_id)?;
    Ok(json!({ "replied_to": orig.id, "to": { "id": target.id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail }))
}

/// One recipient copy with its stage: the store's status plus, after a direct delivery, the transcript's receipt.
/// `kind` is the coarse state the UI colors by: queued, delivered, working, replied, failed.
fn copy_json(store: &Store, c: &store::Letter, with_runs: bool) -> Value {
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
    let (stage, kind) = match (&receipt, c.status.as_str()) {
        (_, store::REPLIED) => (t("stage.replied"), "replied"),
        (Some(claude::Receipt::Completed { .. }), _) => (t("stage.finished"), "replied"),
        (Some(claude::Receipt::Started), _) => (t("stage.working"), "working"),
        (Some(claude::Receipt::Queued), _) => (t("stage.queued_after_turn"), "delivered"),
        (_, store::READ_BY_HOOK) => (t("stage.delivered_hook"), "delivered"),
        (_, store::DELIVERED_INBOX) => (t("stage.delivered"), "delivered"),
        (_, store::DELIVERED_CODEX) => (t("stage.queued_codex"), "delivered"),
        (_, store::DELIVERED_GROK) => (t("stage.delivered"), "delivered"),
        (_, store::FAILED) => (t("stage.failed"), "failed"),
        (_, store::CANCELLED) => (t("stage.cancelled"), "cancelled"),
        _ => (t("stage.queued"), "queued"),
    };
    let deadline = store::time_text(c.deadline_at());
    let stage = if kind == "queued" { t("stage.queued_until").replace("{t}", short_time(&deadline)) } else { stage.to_string() };
    let mut v = json!({ "letter_id": c.id, "short_id": short_id(&c.id), "from": c.from, "from_name": c.from_name, "to": c.to, "to_name": c.to_name, "status": c.status, "stage": stage, "kind": kind, "created": c.created, "deadline": deadline, "path": c.path, "body": c.body });
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
    let l = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    let mut copies = store.copies(&l.dispatch_id)?;
    reconcile(store, &mut copies)?;
    let rows: Vec<Value> = copies.iter().map(|c| copy_json(store, c, true)).collect();
    Ok(json!({ "dispatch_id": l.dispatch_id, "copies": rows }))
}

/// The feed: every dispatch (one message, its recipients), newest first.
pub fn feed(store: &Store, limit: usize) -> Result<Value> {
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
        // (a reply shares the dispatch id and goes back to the sender).
        let first = copies[0];
        let sender = first.from.clone();
        let (sent, replies): (Vec<&store::Letter>, Vec<&store::Letter>) = copies.iter().partition(|c| c.from == sender);
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
        out.push(json!({ "dispatch_id": did, "from": sender, "from_name": first.from_name, "created": first.created, "urgency": first.urgency, "text": first.body, "recipients": recipients }));
    }
    Ok(json!({ "count": out.len(), "dispatches": out }))
}

/// How to open a session on this machine: a URL the system opens, or a command to copy.
pub fn open_session(session_id: &str) -> Result<Value> {
    let s = send::resolve(session_id)?;
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
            body: "hi".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        l
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
