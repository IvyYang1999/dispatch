//! The address book: stable names for the sessions that matter.
//!
//! Discovery finds sessions by their traces and stops listing them after two days. A contact
//! is a name a person (or the session itself) gave a session, kept in `contacts.json` in the
//! store folder and good for as long as it is there. Contacts are part of discovery: a listed
//! session takes its contact's name, and a contact whose session the list lacks is looked up
//! by id (its transcript, whatever its age) and listed anyway. So `dispatch send --to <name>`
//! hits it without a search, and a peer's session list carries the names too.

use crate::model::{short_id, Harness, Host, Session};
use crate::store::Store;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub session_id: String,
    pub harness: Harness,
    /// The machine the session is on, when it is another one.
    #[serde(default)]
    pub host: Option<Host>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// `app`, `cli`, `self` (the session registered itself) or `mcp`.
    #[serde(default)]
    pub set_by: String,
    #[serde(default)]
    pub updated: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    contacts: Vec<Contact>,
}

fn path() -> PathBuf {
    Store::open().root().join("contacts.json")
}

pub fn list() -> Vec<Contact> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str::<Book>(&t).ok()).map(|b| b.contacts).unwrap_or_default()
}

fn save(contacts: &[Contact]) -> Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&Book { contacts: contacts.to_vec() })? + "\n")?;
    Ok(())
}

/// A contact by name; case does not matter.
pub fn find(name: &str) -> Option<Contact> {
    let q = name.trim();
    list().into_iter().find(|c| c.name.eq_ignore_ascii_case(q))
}

/// The contact for a session, on this machine (`host` None) or on a peer.
pub fn for_session(session_id: &str, host: Option<&str>) -> Option<Contact> {
    list().into_iter().find(|c| c.session_id.eq_ignore_ascii_case(session_id) && c.host.as_ref().map(|h| h.id.as_str()) == host)
}

/// Name a session. A contact of that name is replaced, and so is an older name of the same session.
pub fn set(name: &str, s: &Session, by: &str) -> Result<Contact> {
    let name = name.trim();
    if name.is_empty() {
        bail!("the name is empty");
    }
    if name.contains('/') {
        bail!("a name cannot contain '/', which separates a machine from a session");
    }
    if name.chars().count() > 48 {
        bail!("the name is longer than 48 characters");
    }
    let c = Contact { name: name.to_string(), session_id: s.id.to_lowercase(), harness: s.harness, host: s.host.clone(), cwd: s.cwd.clone(), set_by: by.to_string(), updated: crate::store::now_text() };
    let mut all = list();
    all.retain(|x| !x.name.eq_ignore_ascii_case(name) && !(x.session_id.eq_ignore_ascii_case(&c.session_id) && x.host == c.host));
    all.push(c.clone());
    save(&all)?;
    Ok(c)
}

pub fn remove(name: &str) -> Result<Option<Contact>> {
    let mut all = list();
    let Some(i) = all.iter().position(|c| c.name.eq_ignore_ascii_case(name.trim())) else { return Ok(None) };
    let c = all.remove(i);
    save(&all)?;
    Ok(Some(c))
}

/// The contact's name onto a session; the name it had before stays as its title.
fn rename(s: &mut Session, name: &str) {
    if let Some(old) = s.name.take() {
        if old != name && s.title.is_none() {
            s.title = Some(old);
        }
    }
    s.name = Some(name.to_string());
}

/// A session known from its contact alone.
fn bare(c: &Contact) -> Session {
    Session {
        harness: c.harness,
        id: c.session_id.clone(),
        short_id: short_id(&c.session_id),
        name: Some(c.name.clone()),
        cwd: c.cwd.clone(),
        title: None,
        last_active: 0,
        running: false,
        desktop: false,
        host_session_id: None,
        status: None,
        transcript: None,
        host: c.host.clone(),
    }
}

/// Put this machine's contacts onto its discovery list: names on listed sessions, and an entry
/// for each contact whose session the list lacks (looked up by id, else from the contact alone).
pub fn apply_local(list: &mut Vec<Session>) {
    for c in self::list().into_iter().filter(|c| c.host.is_none()) {
        match list.iter_mut().find(|s| s.id.eq_ignore_ascii_case(&c.session_id)) {
            Some(s) => rename(s, &c.name),
            None => {
                let mut s = crate::discover::lookup_files(&c.session_id).unwrap_or_else(|| bare(&c));
                rename(&mut s, &c.name);
                list.push(s);
            }
        }
    }
}

/// Names for a peer's list: this machine's contacts that point at that machine. A contact the
/// peer did not list is added from the contact alone, so it stays addressable.
pub fn apply_remote(host_id: &str, list: &mut Vec<Session>) {
    for c in self::list().into_iter().filter(|c| c.host.as_ref().map(|h| h.id == host_id).unwrap_or(false)) {
        match list.iter_mut().find(|s| s.id.eq_ignore_ascii_case(&c.session_id)) {
            Some(s) => rename(s, &c.name),
            None => list.push(bare(&c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: Option<&str>) -> Session {
        Session { harness: Harness::Codex, id: id.into(), short_id: short_id(id), name: name.map(str::to_string), cwd: Some("/w/dispatch".into()), title: None, last_active: 5, running: true, desktop: false, host_session_id: None, status: None, transcript: None, host: None }
    }

    #[test]
    fn contacts_name_sessions_and_outlive_the_list() {
        let dir = std::env::temp_dir().join(format!("dispatch-contacts-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "11111111-aaaa-4bbb-8ccc-dddddddddddd";
        let b = "22222222-aaaa-4bbb-8ccc-dddddddddddd";
        assert!(set("Dispatch/dev", &session(a, None), "test").is_err(), "a slash means a machine");
        set("Dispatch dev", &session(a, Some("thread 1")), "test").unwrap();
        set("Release", &session(b, None), "test").unwrap();
        // renaming the same session replaces its old name; a reused name moves to the new session
        set("Dispatch 开发", &session(a, None), "test").unwrap();
        assert_eq!(list().len(), 2);
        assert!(find("dispatch dev").is_none());
        assert_eq!(find("DISPATCH 开发").unwrap().session_id, a);
        assert_eq!(for_session(b, None).unwrap().name, "Release");
        // the list only knows a: it is renamed, its client name kept as the title; b is added from the contact
        let mut l = vec![session(a, Some("thread 1"))];
        apply_local(&mut l);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name.as_deref(), Some("Dispatch 开发"));
        assert_eq!(l[0].title.as_deref(), Some("thread 1"));
        assert_eq!(l[1].id, b);
        assert_eq!(l[1].harness, Harness::Codex);
        assert!(!l[1].running);
        assert!(remove("release").unwrap().is_some());
        assert!(remove("release").unwrap().is_none());
        assert_eq!(list().len(), 1);
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
