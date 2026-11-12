//! What other sessions may see of a session. The person at the app sees everything; this is
//! what agents get from `dispatch_sessions`, `dispatch_find`, the command line's lists and
//! `dispatch_context`:
//!
//! - `none`: its name in the address book (else its short id), so it can be written to.
//! - `basic` (the default): its name, the line about it, its project and whether it is open.
//! - `asks`: also its title and folder, and the questions it was asked lately.
//! - `rounds`: also the latest rounds, question and answer.
//!
//! A session's title is usually its first prompt and its folder can say whom it works for, so
//! neither goes out unless the user chose so.

use crate::model::{short_cwd, short_id, Session};
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

pub const LEVELS: [&str; 4] = ["none", "basic", "asks", "rounds"];
const ASKS: usize = 10;
const ROUNDS: usize = 4;
const CLIP: usize = 600;

fn key(id: &str, host: Option<&str>) -> String {
    format!("{}|{}", host.unwrap_or(""), id.to_lowercase())
}

fn book() -> Map<String, Value> {
    crate::settings::get("share").and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// What this session shares with other sessions.
pub fn level(id: &str, host: Option<&str>) -> &'static str {
    let b = book();
    let v = b.get(&key(id, host)).and_then(|v| v.as_str()).unwrap_or("basic");
    LEVELS.iter().copied().find(|l| *l == v).unwrap_or("basic")
}

pub fn set_level(id: &str, host: Option<&str>, level: &str) -> Result<&'static str> {
    let l = LEVELS.iter().copied().find(|l| *l == level).ok_or_else(|| anyhow!("share is none, basic, asks or rounds, not \"{level}\""))?;
    let mut b = book();
    if l == "basic" {
        b.remove(&key(id, host));
    } else {
        b.insert(key(id, host), json!(l));
    }
    crate::settings::set("share", Value::Object(b))?;
    Ok(l)
}

fn last_folder(cwd: &str) -> Option<String> {
    cwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
}

fn clip(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= CLIP {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(CLIP).collect::<String>())
    }
}

// Display names can fall back to the first prompt. Shared identities must use only an
// explicit name (the address book takes precedence), or the short id when none was given.
fn shared_name(id: &str, lvl: &str, explicit: Option<&str>, contact: Option<&str>) -> String {
    contact.or_else(|| if lvl == "none" { None } else { explicit })
        .map(str::to_string).unwrap_or_else(|| short_id(id))
}

// Both the local and paired-machine resolvers accept ids; a routing address never needs a
// display name or a private prompt. A peer id also avoids ambiguous same-name computers.
fn shared_address(id: &str, host: Option<&str>) -> String {
    host.map(|h| format!("{h}/{id}")).unwrap_or_else(|| id.to_string())
}

/// A session as `ops::session_json` describes it, cut down to what it shares with agents.
pub fn for_agents(mut v: Value) -> Value {
    let id = v["id"].as_str().unwrap_or("").to_string();
    let host = v["host"]["id"].as_str().map(str::to_string);
    let lvl = level(&id, host.as_deref());
    let contact = v["contact"].as_str().map(str::to_string)
        .or_else(|| crate::contacts::for_session(&id, host.as_deref()).map(|c| c.name));
    // ops::session_json's `name` has already fallen back to the title; its `app_name`
    // preserves the explicit name. A raw serialized Session (CLI) has no `contact` field
    // and its `name` is still explicit, with no title fallback.
    let explicit = if v.get("contact").is_some() { v["app_name"].as_str() } else { v["name"].as_str() };
    let name = shared_name(&id, lvl, explicit, contact.as_deref());
    let address = shared_address(&id, host.as_deref());
    let project = v["project"].as_str().map(str::to_string).or_else(|| v["cwd"].as_str().and_then(last_folder));
    let Some(o) = v.as_object_mut() else { return v };
    o.insert("address".into(), json!(address));
    // how it runs and where its record lives are nobody else's business
    o.remove("host_session_id");
    o.remove("transcript");
    if let Some(r) = o.get_mut("reach").and_then(|r| r.as_object_mut()) {
        r.remove("mode");
        r.remove("client");
    }
    match lvl {
        "none" => {
            o.insert("name".into(), json!(name));
            for k in ["about", "avatar", "app_name", "title", "project", "cwd", "cwd_short"] {
                o.remove(k);
            }
        }
        "basic" => {
            o.insert("name".into(), json!(name));
            for k in ["app_name", "title", "cwd", "cwd_short"] {
                o.remove(k);
            }
            o.insert("project".into(), json!(project));
        }
        "asks" => {
            o.remove("cwd");
        }
        _ => {}
    }
    o.insert("share".into(), json!(lvl));
    v
}

/// Every session in a list of session JSONs (under `key`), cut down for agents.
pub fn list_for_agents(mut v: Value, key: &str) -> Value {
    if let Some(a) = v.get_mut(key).and_then(|a| a.as_array_mut()) {
        for s in a.iter_mut() {
            *s = for_agents(s.take());
        }
    }
    v
}

/// What an agent may know of a session before writing to it, as its share setting allows.
pub fn context(s: &Session) -> Value {
    let host = s.host.as_ref().map(|h| h.id.as_str());
    let lvl = level(&s.id, host);
    let contact = crate::contacts::for_session(&s.id, host);
    let name = match lvl {
        "none" | "basic" => shared_name(&s.id, lvl, s.name.as_deref(), contact.as_ref().map(|c| c.name.as_str())),
        _ => s.display_name(),
    };
    let mut v = json!({ "address": shared_address(&s.id, host), "name": name, "harness": s.harness.label(), "share": lvl });
    if lvl == "none" {
        v["note"] = json!(crate::i18n::t("share.none_note"));
        return v;
    }
    v["about"] = json!(contact.as_ref().and_then(|c| c.description.clone()));
    v["project"] = json!(s.project.clone().or_else(|| s.cwd.as_deref().and_then(last_folder)));
    v["running"] = json!(s.running);
    if lvl == "basic" {
        return v;
    }
    v["title"] = json!(s.title);
    v["folder"] = json!(s.cwd.as_deref().map(short_cwd));
    if s.host.is_some() {
        // the machine it is on decides what it shares; asking it is for a later version
        v["note"] = json!(crate::i18n::t("share.remote_note"));
        return v;
    }
    let turns = crate::transcript::recent(s, 200);
    let users: Vec<&crate::transcript::Turn> = turns.iter().filter(|t| t.role == "user").collect();
    let asked: Vec<Value> = users[users.len().saturating_sub(ASKS)..].iter().map(|t| json!({ "at": t.at, "text": clip(&t.text), "via_dispatch": t.via_dispatch, "from": t.from_agent })).collect();
    v["questions"] = json!(asked);
    if lvl == "rounds" {
        let rounds: Vec<Value> = crate::transcript::rounds(&turns, ROUNDS)
            .iter()
            .map(|r| json!({ "question": r.user.as_ref().map(|u| clip(&u.text)), "answer": clip(&r.replies.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join("\n\n")) }))
            .collect();
        v["rounds"] = json!(rounds);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unnamed_session() -> Session {
        Session {
            harness: crate::model::Harness::Claude,
            id: "aaaaaaaa-1111-4222-8333-444455556666".into(), short_id: "aaaaaaaa".into(),
            name: None, title: Some("PRIVATE_PROMPT_SENTINEL".into()),
            cwd: Some("/synthetic/project".into()), project: Some("PublicProject".into()),
            last_active: 0, running: false, desktop: false, host_session_id: None,
            status: None, transcript: None, host: None, app_name: None,
        }
    }

    #[test]
    fn restricted_identity_never_falls_back_to_a_private_prompt() {
        let dir = std::env::temp_dir().join(format!("dispatch-share-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let mut s = unnamed_session();
        for host in [None, Some(crate::model::Host { id: "bbbbbbbb-1111-4222-8333-444455556666".into(), name: "Other computer".into() })] {
            s.host = host;
            let host_id = s.host.as_ref().map(|h| h.id.as_str());
            let expected_address = host_id.map(|h| format!("{h}/{}", s.id)).unwrap_or_else(|| s.id.clone());
            for lvl in ["basic", "none"] {
                set_level(&s.id, host_id, lvl).unwrap();
                // Exercise the actual App/MCP projection and the CLI's raw Session shape.
                let values = [for_agents(crate::ops::session_json(&s)), for_agents(serde_json::to_value(&s).unwrap()), context(&s)];
                for v in values {
                    assert!(!v.to_string().contains("PRIVATE_PROMPT_SENTINEL"), "{lvl} leaked: {v}");
                    assert_eq!(v["name"], "aaaaaaaa");
                    assert_eq!(v["address"], expected_address);
                    assert!(v.get("title").is_none());
                    if lvl == "none" {
                        assert!(v.get("about").is_none() && v.get("project").is_none());
                    }
                }
            }
        }
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shared_identity_preserves_explicit_names_and_contact_priority() {
        let dir = std::env::temp_dir().join(format!("dispatch-share-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let mut s = unnamed_session();
        s.name = Some("Public nickname".into());
        for v in [for_agents(crate::ops::session_json(&s)), for_agents(serde_json::to_value(&s).unwrap()), context(&s)] {
            assert_eq!(v["name"], "Public nickname");
            assert_eq!(v["address"], s.id);
            assert!(!v.to_string().contains("PRIVATE_PROMPT_SENTINEL"));
        }
        set_level(&s.id, None, "none").unwrap();
        assert_eq!(context(&s)["name"], s.short_id);
        crate::contacts::set("Directory name", &s, "app", Some("Public description"), false).unwrap();
        for lvl in ["none", "basic"] {
            set_level(&s.id, None, lvl).unwrap();
            for v in [for_agents(crate::ops::session_json(&s)), for_agents(serde_json::to_value(&s).unwrap()), context(&s)] {
                assert_eq!(v["name"], "Directory name");
                assert_eq!(v["address"], s.id);
                assert!(!v.to_string().contains("PRIVATE_PROMPT_SENTINEL"));
            }
        }
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn session_json(id: &str) -> Value {
        json!({ "id": id, "name": "目前用下来，我感觉有几个问题", "contact": "日历", "about": "日历小插件", "app_name": "目前用下来", "title": "目前用下来，我感觉有几个问题：周视图…", "project": null, "cwd": "/Users/me/clients/acme/日历小插件", "cwd_short": "~/…/acme/日历小插件", "host_session_id": "local_x", "reach": { "level": "now", "text": "x", "mode": "bypassPermissions", "client": "desktop" } })
    }

    #[test]
    fn agents_get_name_and_about_unless_the_user_shares_more() {
        let dir = std::env::temp_dir().join(format!("dispatch-share-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let id = "aaaaaaaa-1111-4222-8333-444455556666";

        let v = for_agents(session_json(id));
        assert_eq!(v["share"], "basic");
        assert_eq!(v["about"], "日历小插件");
        assert_eq!(v["project"], "日历小插件", "the folder's name stands for the project");
        for k in ["title", "cwd", "cwd_short", "app_name", "host_session_id"] {
            assert!(v.get(k).is_none(), "{k} is not shared by default");
        }
        assert!(v["reach"].get("mode").is_none(), "how it runs is not shared");

        set_level(id, None, "none").unwrap();
        let v = for_agents(session_json(id));
        assert_eq!(v["name"], "日历", "its address-book name only");
        assert!(v.get("about").is_none() && v.get("project").is_none());

        set_level(id, None, "asks").unwrap();
        let v = for_agents(session_json(id));
        assert!(v.get("title").is_some() && v.get("cwd_short").is_some() && v.get("cwd").is_none());

        set_level(id, None, "basic").unwrap();
        assert_eq!(level(id, None), "basic");
        assert!(set_level(id, None, "everything").is_err());
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
