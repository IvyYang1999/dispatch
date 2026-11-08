//! What changed in the feed since the last look, for the menu bar item and the system
//! notifications. Pure functions over the feed JSON (`ops::feed`), so the app's watcher
//! stays a loop and this stays testable.
//!
//! Only two things are worth a notification (the design's "don't be noisy"): a reply
//! came in, or a delivery failed. The menu bar item shows how many dispatches are
//! queued and how many failed, and lists the most recent few.

use crate::i18n::t;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The copies' coarse states and the replies seen, keyed by letter id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snapshot {
    kinds: HashMap<String, String>,
    replies: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `from` answered the message `to` sent. `to_is_me`: the sender was the person (app or CLI).
    Reply { dispatch_id: String, from: String, to: String, to_is_me: bool, body: String },
    /// The copy to `to` is now marked failed.
    Failed { dispatch_id: String, to: String, text: String, stage: String },
}

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

fn is_me(sender_id: &str) -> bool {
    matches!(sender_id, "app" | "cli" | "mcp")
}

pub fn snapshot(feed: &Value) -> Snapshot {
    let mut snap = Snapshot::default();
    for d in feed["dispatches"].as_array().into_iter().flatten() {
        for r in d["recipients"].as_array().into_iter().flatten() {
            snap.kinds.insert(s(r, "letter_id"), s(r, "kind"));
            if let Some(id) = r["reply"]["letter_id"].as_str() {
                snap.replies.insert(id.to_string());
            }
        }
    }
    snap
}

/// The new snapshot and what happened between `prev` and it. With no `prev` (first look),
/// nothing happened: what was already there is not news.
pub fn events(prev: Option<&Snapshot>, feed: &Value) -> (Snapshot, Vec<Event>) {
    let next = snapshot(feed);
    let Some(prev) = prev else { return (next, vec![]) };
    let mut out = vec![];
    for d in feed["dispatches"].as_array().into_iter().flatten() {
        let dispatch_id = s(d, "dispatch_id");
        for r in d["recipients"].as_array().into_iter().flatten() {
            let letter_id = s(r, "letter_id");
            if let Some(id) = r["reply"]["letter_id"].as_str() {
                if !prev.replies.contains(id) {
                    out.push(Event::Reply { dispatch_id: dispatch_id.clone(), from: s(r, "to_name"), to: s(d, "from_name"), to_is_me: is_me(&s(d, "from")), body: s(&r["reply"], "body") });
                }
            }
            if r["kind"] == "failed" && prev.kinds.get(&letter_id).map(|k| k != "failed").unwrap_or(false) {
                out.push(Event::Failed { dispatch_id: dispatch_id.clone(), to: s(r, "to_name"), text: s(d, "text"), stage: s(r, "stage") });
            }
        }
    }
    (next, out)
}

/// One line of the menu bar list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recent {
    pub dispatch_id: String,
    pub line: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Dispatches with a copy still queued.
    pub queued: usize,
    /// Dispatches with a failed copy.
    pub failed: usize,
    pub total: usize,
    pub recent: Vec<Recent>,
}

/// The first line of `text`, at most `max` characters, with an ellipsis when cut.
pub fn clip(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let n = line.chars().count();
    if n <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

fn recipients_text(d: &Value) -> String {
    let names: Vec<String> = d["recipients"].as_array().into_iter().flatten().map(|r| s(r, "to_name")).collect();
    match names.len() {
        0 => String::new(),
        1 | 2 => names.join(", "),
        n => format!("{} +{}", names[..2].join(", "), n - 2),
    }
}

pub fn summary(feed: &Value, recent: usize) -> Summary {
    let mut out = Summary::default();
    let has = |d: &Value, kind: &str| d["recipients"].as_array().into_iter().flatten().any(|r| r["kind"] == kind);
    for d in feed["dispatches"].as_array().into_iter().flatten() {
        out.total += 1;
        if has(d, "queued") {
            out.queued += 1;
        }
        if has(d, "failed") {
            out.failed += 1;
        }
        if out.recent.len() < recent {
            let mark = if has(d, "failed") {
                format!("  · {}", t("stage.failed"))
            } else if has(d, "queued") {
                format!("  · {}", t("stage.queued"))
            } else {
                String::new()
            };
            let line = format!("{} → {}: {}{}", s(d, "from_name"), recipients_text(d), clip(&s(d, "text"), 36), mark);
            out.recent.push(Recent { dispatch_id: s(d, "dispatch_id"), line });
        }
    }
    out
}

/// The text beside the menu bar icon: `排队 2 · 没送到 1`; nothing when both are zero.
pub fn title(sum: &Summary) -> Option<String> {
    let mut parts = vec![];
    if sum.queued > 0 {
        parts.push(t("tray.queued").replace("{n}", &sum.queued.to_string()));
    }
    if sum.failed > 0 {
        parts.push(t("tray.failed").replace("{n}", &sum.failed.to_string()));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The first, disabled line of the menu: the counts in words.
pub fn headline(sum: &Summary) -> String {
    if sum.total == 0 {
        return t("tray.empty").to_string();
    }
    title(sum).unwrap_or_else(|| t("tray.clear").to_string())
}

/// Title and body of the notification for `e`.
pub fn notification(e: &Event) -> (String, String) {
    match e {
        Event::Reply { from, to, to_is_me, body, .. } => {
            let title = if *to_is_me { t("notify.reply_me").replace("{from}", from) } else { t("notify.reply").replace("{from}", from).replace("{to}", to) };
            (title, clip(body, 120))
        }
        Event::Failed { to, text, stage, .. } => (t("notify.failed").replace("{to}", to), format!("{} · {}", stage, clip(text, 100))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(reply: bool, failed: bool) -> Value {
        let mut r1 = json!({ "letter_id": "l1", "to": "s1", "to_name": "Researcher", "kind": if failed { "failed" } else { "queued" }, "stage": if failed { "没送到" } else { "排队中" } });
        if reply {
            r1["kind"] = json!("replied");
            r1["reply"] = json!({ "letter_id": "r1", "body": "Done, see the PR.\nMore lines." });
        }
        json!({ "dispatches": [
            { "dispatch_id": "d1", "from": "app", "from_name": "我", "text": "Please check the e2e run and tell me if it is green", "recipients": [r1] },
            { "dispatch_id": "d2", "from": "11111111-2222-3333-4444-555555555555", "from_name": "Transparent", "text": "ping", "recipients": [
                { "letter_id": "l2", "to": "s2", "to_name": "KeyKeeper", "kind": "delivered", "stage": "已送达" },
                { "letter_id": "l3", "to": "s3", "to_name": "Shyposter", "kind": "replied", "stage": "已回复", "reply": { "letter_id": "r3", "body": "pong" } },
                { "letter_id": "l4", "to": "s4", "to_name": "Writer", "kind": "delivered", "stage": "已送达" }
            ] }
        ] })
    }

    #[test]
    fn first_look_is_quiet_then_replies_and_failures_are_news() {
        let _g = crate::i18n::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::i18n::set(crate::i18n::Lang::Zh);
        let (snap, ev) = events(None, &feed(false, false));
        assert!(ev.is_empty());
        let (snap2, ev) = events(Some(&snap), &feed(true, false));
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            Event::Reply { from, to_is_me, body, .. } => {
                assert_eq!(from, "Researcher");
                assert!(to_is_me);
                assert_eq!(notification(&ev[0]).0, "Researcher 回复了你");
                assert_eq!(clip(body, 120), "Done, see the PR.");
            }
            _ => panic!("expected a reply"),
        }
        // the same feed again: nothing new
        let (_, ev) = events(Some(&snap2), &feed(true, false));
        assert!(ev.is_empty());
        let (_, ev) = events(Some(&snap), &feed(false, true));
        assert!(matches!(&ev[0], Event::Failed { to, .. } if to == "Researcher"));
        assert_eq!(notification(&ev[0]).0, "没送到 Researcher");
        crate::i18n::set(crate::i18n::Lang::En);
    }

    #[test]
    fn summary_counts_and_lists() {
        let _g = crate::i18n::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::i18n::set(crate::i18n::Lang::Zh);
        let sum = summary(&feed(false, false), 5);
        assert_eq!((sum.queued, sum.failed, sum.total), (1, 0, 2));
        assert_eq!(title(&sum).as_deref(), Some("排队 1"));
        assert_eq!(sum.recent[0].line, "我 → Researcher: Please check the e2e run and tell m…  · 排队中");
        assert_eq!(sum.recent[1].line, "Transparent → KeyKeeper, Shyposter +1: ping");
        let sum = summary(&feed(true, false), 5);
        assert_eq!(title(&sum), None);
        assert_eq!(headline(&sum), "没有在排队或没送到的");
        assert_eq!(headline(&summary(&json!({ "dispatches": [] }), 5)), "还没有派发");
        crate::i18n::set(crate::i18n::Lang::En);
        assert_eq!(title(&summary(&feed(false, true), 5)).as_deref(), Some("1 failed"));
    }
}
