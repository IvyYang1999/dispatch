//! The operations both the CLI and the MCP server expose, as JSON-friendly values.

use crate::claude;
use crate::discover;
use crate::hook;
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
    if to.is_empty() {
        return Err(anyhow!("`to` must name at least one session"));
    }
    let (from_id, from_name) = from.unwrap_or_else(caller_identity);
    let dispatch_id = send::new_id();
    let mut reports = vec![];
    for t in to {
        let target = send::resolve(t)?;
        let rep = send::send(store, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: text.to_string(), urgency: urgency.to_string(), dispatch_id: Some(dispatch_id.clone()), wait: true })?;
        reports.push(json!({ "to": { "id": target.id, "short_id": target.short_id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail, "path": rep.path }));
    }
    Ok(json!({ "dispatch_id": dispatch_id, "from": { "id": from_id, "name": from_name }, "recipients": reports }))
}

pub fn inbox(store: &Store, session: Option<&str>, all: bool) -> Result<Value> {
    let sid = match session {
        Some(s) => send::resolve(s)?.id,
        None => claude::caller_session().map(|i| i.session_id).ok_or_else(|| anyhow!("no session given and the caller is not a Claude Code session"))?,
    };
    let letters = store.list(Some(&sid), !all)?;
    let rows: Vec<Value> = letters
        .iter()
        .map(|l| json!({ "id": l.id, "short_id": short_id(&l.id), "dispatch_id": l.dispatch_id, "from": l.from, "from_name": l.from_name, "status": l.status, "created": l.created, "urgency": l.urgency, "body": l.body, "path": l.path, "can_reply": is_uuid(&l.from) }))
        .collect();
    Ok(json!({ "session_id": sid, "unread_only": !all, "count": rows.len(), "messages": rows }))
}

pub fn reply(store: &Store, id: &str, text: &str) -> Result<Value> {
    let mut orig = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    if !is_uuid(&orig.from) {
        return Err(anyhow!("the sender \"{}\" is not a session, so it cannot be replied to", orig.from));
    }
    let target = send::resolve(&orig.from)?;
    let rep = send::send(store, send::Request { to: target.clone(), from: orig.to.clone(), from_name: orig.to_name.clone(), text: text.to_string(), urgency: orig.urgency.clone(), dispatch_id: Some(orig.dispatch_id.clone()), wait: true })?;
    store.set_status(&mut orig, store::REPLIED)?;
    store.append_run(&orig.id, "reply", "sent", &rep.letter_id)?;
    Ok(json!({ "replied_to": orig.id, "to": { "id": target.id, "name": target.display_name() }, "letter_id": rep.letter_id, "route": rep.route, "detail": rep.detail }))
}

/// Where a message stands: its copies, each with a stage read from the store and the transcript.
pub fn status(store: &Store, id: &str) -> Result<Value> {
    let l = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    let copies = store.copies(&l.dispatch_id)?;
    let mut rows = vec![];
    for c in &copies {
        let runs = store.runs(&c.id);
        let delivered_inbox = runs.iter().any(|r| r.action == "inbox" && r.result == "written");
        let receipt = if delivered_inbox { claude::receipt(&c.to, None, &c.id).ok() } else { None };
        let stage = match (&receipt, c.status.as_str()) {
            (_, store::REPLIED) => "已回复",
            (Some(claude::Receipt::Completed { .. }), _) => "本轮结束",
            (Some(claude::Receipt::Started), _) => "处理中",
            (Some(claude::Receipt::Queued), _) => "已送达（排在本轮之后）",
            (_, store::READ_BY_HOOK) => "已送达（hook 递的）",
            (_, store::DELIVERED_INBOX) => "已送达",
            _ => "排队中",
        };
        rows.push(json!({ "letter_id": c.id, "from_name": c.from_name, "to": c.to, "to_name": c.to_name, "status": c.status, "stage": stage, "created": c.created, "path": c.path, "runs": runs }));
    }
    Ok(json!({ "dispatch_id": l.dispatch_id, "copies": rows }))
}
