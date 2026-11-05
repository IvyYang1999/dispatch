//! The delivery ladder: store the message, then get it in the fastest way that works.
//!
//! 1. Write the recipient's copy to the store (so nothing is ever lost).
//! 2. If the recipient is a running Claude Code session with an inbox that would
//!    take it, write it to the socket and wait for the transcript to show it queued.
//! 3. Otherwise leave it unread: the hook hands it over the next time the session speaks.

use crate::claude::{self, Gate, SendOutcome};
use crate::discover;
use crate::i18n::t;
use crate::model::{is_uuid, Session};
use crate::store::{self, Letter, Store, DELIVERED_INBOX, UNREAD};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub letter_id: String,
    pub dispatch_id: String,
    pub path: String,
    /// `inbox` (written to the socket and queued), `stored` (waiting for the hook).
    pub route: String,
    pub detail: String,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Resolve a recipient: a full id, a short id prefix, a desktop name or a title fragment.
pub fn resolve(query: &str) -> Result<Session> {
    let q = query.trim();
    let all = discover::sessions();
    if is_uuid(q) {
        if let Some(s) = all.iter().find(|s| s.id.eq_ignore_ascii_case(q)) {
            return Ok(s.clone());
        }
        return Ok(Session {
            harness: crate::model::Harness::Claude,
            id: q.to_lowercase(),
            short_id: crate::model::short_id(q),
            name: None,
            cwd: None,
            title: None,
            last_active: 0,
            running: claude::is_running(q),
            desktop: false,
            host_session_id: None,
            status: None,
            transcript: None,
        });
    }
    let ql = q.to_lowercase();
    let mut hits: Vec<&Session> = all.iter().filter(|s| s.id.starts_with(&ql)).collect();
    if hits.is_empty() {
        hits = all.iter().filter(|s| s.name.as_deref().map(|n| n.to_lowercase().contains(&ql)).unwrap_or(false)).collect();
    }
    if hits.is_empty() {
        hits = all.iter().filter(|s| s.title.as_deref().map(|n| n.to_lowercase().contains(&ql)).unwrap_or(false)).collect();
    }
    match hits.len() {
        0 => Err(anyhow!("{}", t("send.no_match").replace("{q}", q))),
        1 => Ok(hits[0].clone()),
        _ => {
            let list: Vec<String> = hits.iter().take(8).map(|s| format!("  {}  {}  {}", s.short_id, s.display_name(), s.cwd.as_deref().map(crate::model::short_cwd).unwrap_or_default())).collect();
            Err(anyhow!("{}", t("send.many_match").replace("{q}", q).replace("{n}", &hits.len().to_string()).replace("{list}", &list.join("\n"))))
        }
    }
}

/// The text the session sees: who sent it, when, the body, and how to answer.
pub fn envelope(l: &Letter) -> String {
    t("send.envelope")
        .replace("{from}", &l.from_name)
        .replace("{created}", &l.created)
        .replace("{id}", &crate::model::short_id(&l.id))
        .replace("{body}", l.body.trim())
        .replace("{cmd}", &crate::model::self_cmd())
}

/// The Codex branch: only a thread the shared daemon has loaded gets `codex queue`; anything
/// else waits as 待接通 unless the caller allowed `--resume`.
fn send_codex(store: &Store, req: &Request, letter: &mut Letter, report: impl Fn(&str, String) -> Report) -> Result<Report> {
    let st = crate::codex::state();
    if !st.cli {
        store.append_run(&letter.id, "codex", "skipped", "codex not installed")?;
        return Ok(report("stored", t("send.codex_nocli").into()));
    }
    if !st.daemon {
        store.append_run(&letter.id, "codex", "skipped", "no app-server daemon")?;
        return Ok(report("stored", t("send.codex_nodaemon").into()));
    }
    let loaded = st.loaded.contains(&req.to.id);
    if !loaded && !req.allow_resume {
        store.append_run(&letter.id, "codex", "skipped", "thread not loaded on the daemon; resume not allowed")?;
        return Ok(report("stored", t("send.codex_notloaded").into()));
    }
    let text = envelope(letter);
    match crate::codex::queue(&req.to.id, &text, !loaded) {
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

pub fn send(store: &Store, req: Request) -> Result<Report> {
    let mut letter = Letter {
        id: uuid::Uuid::new_v4().to_string(),
        dispatch_id: req.dispatch_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        from: req.from.clone(),
        from_name: req.from_name.clone(),
        to: req.to.id.clone(),
        to_name: req.to.display_name(),
        urgency: req.urgency.clone(),
        wake: true,
        status: UNREAD.into(),
        created: store::now_text(),
        body: req.text.clone(),
        path: String::new(),
    };
    let path = store.write_letter(&mut letter)?;
    store.append_run(&letter.id, "store", "written", &path.to_string_lossy())?;
    let (letter_id, dispatch_id, path_s) = (letter.id.clone(), letter.dispatch_id.clone(), path.to_string_lossy().to_string());
    let report = move |route: &str, detail: String| Report { letter_id: letter_id.clone(), dispatch_id: dispatch_id.clone(), path: path_s.clone(), route: route.into(), detail };

    if req.to.harness == crate::model::Harness::Codex {
        return send_codex(store, &req, &mut letter, report);
    }

    let Some(inbox) = claude::find_live(&req.to.id) else {
        let (why_log, why) = if req.to.running { ("running without an inbox", t("send.why_noinbox")) } else { ("not running", t("send.why_off")) };
        store.append_run(&letter.id, "inbox", "skipped", why_log)?;
        return Ok(report("stored", t("send.stored_hook").replace("{why}", why)));
    };
    match claude::gate(&req.to.id, inbox.cwd.as_deref()) {
        Gate::Accept => {}
        Gate::Hold => {
            store.append_run(&letter.id, "inbox", "skipped", "bypassPermissions would hold it")?;
            return Ok(report("stored", t("send.hold").into()));
        }
        Gate::Refuse => {
            store.append_run(&letter.id, "inbox", "skipped", "crossSessionInbound=refuse")?;
            return Ok(report("stored", t("send.refuse").into()));
        }
    }
    let text = envelope(&letter);
    match claude::send(&inbox, &text, &letter.id, Duration::from_secs(5)) {
        SendOutcome::Unreachable => {
            store.append_run(&letter.id, "inbox", "unreachable", &inbox.socket.to_string_lossy())?;
            Ok(report("stored", t("send.unreachable").into()))
        }
        SendOutcome::Sent => {
            store.append_run(&letter.id, "inbox", "written", &inbox.socket.to_string_lossy())?;
            if !req.wait {
                return Ok(report("inbox", t("send.written_nowait").into()));
            }
            if claude::queued(&req.to.id, inbox.cwd.as_deref(), &letter.id, &text, Duration::from_secs(10)) {
                store.set_status(&mut letter, DELIVERED_INBOX)?;
                store.append_run(&letter.id, "inbox", "queued", "transcript shows it")?;
                Ok(report("inbox", t("send.queued").into()))
            } else {
                store.append_run(&letter.id, "inbox", "unconfirmed", "no trace in transcript within 10s")?;
                Ok(report("stored", t("send.unconfirmed").into()))
            }
        }
    }
}
