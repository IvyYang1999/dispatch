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

/// One recipient copy with its stage: the store's status plus, after a direct delivery, the transcript's receipt.
/// `kind` is the coarse state the UI colors by: queued, delivered, working, replied, failed.
fn copy_json(store: &Store, c: &store::Letter, with_runs: bool) -> Value {
    let runs = store.runs(&c.id);
    let delivered_inbox = runs.iter().any(|r| r.action == "inbox" && r.result == "written");
    let receipt = if delivered_inbox { claude::receipt(&c.to, None, &c.id).ok() } else { None };
    let (stage, kind) = match (&receipt, c.status.as_str()) {
        (_, store::REPLIED) => ("已回复", "replied"),
        (Some(claude::Receipt::Completed { .. }), _) => ("本轮结束", "replied"),
        (Some(claude::Receipt::Started), _) => ("处理中", "working"),
        (Some(claude::Receipt::Queued), _) => ("已送达（排在本轮之后）", "delivered"),
        (_, store::READ_BY_HOOK) => ("已送达（hook 递的）", "delivered"),
        (_, store::DELIVERED_INBOX) => ("已送达", "delivered"),
        _ => ("排队中", "queued"),
    };
    let mut v = json!({ "letter_id": c.id, "short_id": short_id(&c.id), "from": c.from, "from_name": c.from_name, "to": c.to, "to_name": c.to_name, "status": c.status, "stage": stage, "kind": kind, "created": c.created, "path": c.path, "body": c.body });
    if with_runs {
        v["runs"] = json!(runs);
    }
    v
}

/// Where a message stands: its copies, each with a stage read from the store and the transcript.
pub fn status(store: &Store, id: &str) -> Result<Value> {
    let l = store.find(id)?.ok_or_else(|| anyhow!("no message whose id starts with \"{id}\""))?;
    let copies = store.copies(&l.dispatch_id)?;
    let rows: Vec<Value> = copies.iter().map(|c| copy_json(store, c, true)).collect();
    Ok(json!({ "dispatch_id": l.dispatch_id, "copies": rows }))
}

/// The feed: every dispatch (one message, its recipients), newest first.
pub fn feed(store: &Store, limit: usize) -> Result<Value> {
    let letters = store.list(None, false)?;
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
                        v["stage"] = json!("已回复");
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
            Ok(json!({ "kind": "command", "command": format!("claude --resume {}", s.id), "cwd": s.cwd, "exact": false, "note": "终端会话：在它的目录里运行这条命令就回到它" }))
        }
        crate::model::Harness::Codex => Ok(json!({ "kind": "url", "url": format!("codex://threads/{}", s.id), "exact": true })),
        _ => Err(anyhow!("这种会话还不知道怎么打开")),
    }
}
