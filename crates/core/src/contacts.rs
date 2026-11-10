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
    /// `app`, `cli`, `self` (the session registered itself), `mcp`, or `auto` (named from the
    /// words a sender found it by; provisional until a person confirms it).
    #[serde(default)]
    pub set_by: String,
    #[serde(default)]
    pub updated: String,
    /// One line on what the session is for: what other agents match when they look for it.
    #[serde(default)]
    pub description: Option<String>,
    /// Named automatically from a search; shown apart, never replaces a confirmed name.
    #[serde(default)]
    pub provisional: bool,
    /// How the user wants it pictured: an emoji and a color, instead of an initial on a color
    /// picked from its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Look>,
}

/// A picture for a session: an emoji, a background color (`#rrggbb`), or both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Look {
    /// Trimmed and checked: a short emoji (a few code points), a `#rrggbb` color; None when empty.
    fn clean(self) -> Result<Option<Look>> {
        let emoji = self.emoji.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
        if emoji.as_ref().is_some_and(|e| e.chars().count() > 12) {
            bail!("the picture is longer than one emoji");
        }
        let color = self.color.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty());
        if color.as_ref().is_some_and(|c| c.len() != 7 || !c.starts_with('#') || !c[1..].chars().all(|x| x.is_ascii_hexdigit())) {
            bail!("a color is written #rrggbb");
        }
        Ok((emoji.is_some() || color.is_some()).then_some(Look { emoji, color }))
    }
}

/// Give a named session a picture (or take it away with an empty one).
pub fn set_avatar(name: &str, look: Look) -> Result<Contact> {
    let look = look.clean()?;
    let mut all = list();
    let Some(c) = all.iter_mut().find(|c| c.name.eq_ignore_ascii_case(name.trim())) else { bail!("no contact named {name}") };
    c.avatar = look;
    c.updated = crate::store::now_text();
    let out = c.clone();
    save(&all)?;
    Ok(out)
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

/// Name a session. A contact of that name is replaced, and so is an older name of the same
/// session (its description is kept when none is given). A provisional name never replaces a
/// confirmed one: the confirmed contact comes back unchanged.
pub fn set(name: &str, s: &Session, by: &str, description: Option<&str>, provisional: bool) -> Result<Contact> {
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
    let host_id = s.host.as_ref().map(|h| h.id.clone());
    let same_session = |x: &Contact| x.session_id.eq_ignore_ascii_case(&s.id) && x.host.as_ref().map(|h| h.id.clone()) == host_id;
    let mut all = list();
    if provisional {
        if let Some(c) = all.iter().find(|x| same_session(x) && !x.provisional) {
            return Ok(c.clone());
        }
    }
    let previous = all.iter().find(|x| same_session(x)).cloned();
    let description = description.map(|d| d.trim().chars().take(200).collect::<String>()).filter(|d| !d.is_empty()).or_else(|| previous.as_ref().and_then(|p| p.description.clone()));
    let avatar = previous.as_ref().and_then(|p| p.avatar.clone());
    let c = Contact { name: name.to_string(), session_id: s.id.to_lowercase(), harness: s.harness, host: s.host.clone(), cwd: s.cwd.clone(), set_by: by.to_string(), updated: crate::store::now_text(), description, provisional, avatar };
    all.retain(|x| !x.name.eq_ignore_ascii_case(name) && !same_session(x));
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

/// The contact's name onto a session; the name it had before stays as its title. A provisional
/// name only fills in a session that has no name of its own.
fn rename(s: &mut Session, c: &Contact) {
    if c.provisional && s.name.is_some() {
        return;
    }
    if let Some(old) = s.name.take() {
        if old != c.name {
            // the name its app shows stays known: it is how the user recognizes the session
            if s.app_name.is_none() {
                s.app_name = Some(old.clone());
            }
            if s.title.is_none() {
                s.title = Some(old);
            }
        }
    }
    s.name = Some(c.name.clone());
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
        project: None,
        app_name: None,
    }
}

/// Put this machine's contacts onto its discovery list: names on listed sessions, and an entry
/// for each contact whose session the list lacks (looked up by id, else from the contact alone).
pub fn apply_local(list: &mut Vec<Session>) {
    for c in self::list().into_iter().filter(|c| c.host.is_none()) {
        match list.iter_mut().find(|s| s.id.eq_ignore_ascii_case(&c.session_id)) {
            Some(s) => rename(s, &c),
            None => {
                let mut s = crate::discover::lookup_files(&c.session_id).unwrap_or_else(|| bare(&c));
                rename(&mut s, &c);
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
            Some(s) => rename(s, &c),
            None => list.push(bare(&c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: Option<&str>) -> Session {
        Session { harness: Harness::Codex, id: id.into(), short_id: short_id(id), name: name.map(str::to_string), cwd: Some("/w/dispatch".into()), title: None, last_active: 5, running: true, desktop: false, host_session_id: None, status: None, transcript: None, host: None, project: None, app_name: None }
    }

    #[test]
    fn contacts_name_sessions_and_outlive_the_list() {
        let dir = std::env::temp_dir().join(format!("dispatch-contacts-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "11111111-aaaa-4bbb-8ccc-dddddddddddd";
        let b = "22222222-aaaa-4bbb-8ccc-dddddddddddd";
        assert!(set("Dispatch/dev", &session(a, None), "test", None, false).is_err(), "a slash means a machine");
        set("Dispatch dev", &session(a, Some("thread 1")), "test", Some("builds and ships Dispatch"), false).unwrap();
        set("Release", &session(b, None), "test", None, false).unwrap();
        // renaming the same session replaces its old name and keeps its description
        set("Dispatch 开发", &session(a, None), "test", None, false).unwrap();
        assert_eq!(list().len(), 2);
        assert!(find("dispatch dev").is_none());
        let c = find("DISPATCH 开发").unwrap();
        assert_eq!(c.session_id, a);
        assert_eq!(c.description.as_deref(), Some("builds and ships Dispatch"));
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

    #[test]
    fn provisional_names_fill_gaps_but_never_beat_a_confirmed_name() {
        let dir = std::env::temp_dir().join(format!("dispatch-contacts-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "33333333-aaaa-4bbb-8ccc-dddddddddddd";
        let b = "44444444-aaaa-4bbb-8ccc-dddddddddddd";
        // a sender found a by "release codex": a provisional name
        let p = set("release codex", &session(a, None), "auto", None, true).unwrap();
        assert!(p.provisional);
        // a confirmed name replaces it; a later provisional one is ignored
        set("Release", &session(a, None), "app", None, false).unwrap();
        let again = set("codex thing", &session(a, None), "auto", None, true).unwrap();
        assert_eq!(again.name, "Release");
        assert_eq!(list().len(), 1);
        // a provisional name only fills in a session without a name of its own
        set("transparent", &session(b, Some("🪞 Transparent")), "auto", None, true).unwrap();
        let mut l = vec![session(b, Some("🪞 Transparent"))];
        apply_local(&mut l);
        assert_eq!(l.iter().find(|s| s.id == b).unwrap().name.as_deref(), Some("🪞 Transparent"));
        assert_eq!(l.iter().find(|s| s.id == a).unwrap().name.as_deref(), Some("Release"));
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_picture_is_checked_and_survives_a_rename() {
        let dir = std::env::temp_dir().join(format!("dispatch-contacts-look-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "33333333-aaaa-4bbb-8ccc-dddddddddddd";
        set("Mirror", &session(a, None), "app", None, false).unwrap();
        assert!(set_avatar("Mirror", Look { emoji: None, color: Some("red".into()) }).is_err(), "a color is #rrggbb");
        assert!(set_avatar("Mirror", Look { emoji: Some("🪞🪞🪞🪞🪞🪞🪞🪞🪞🪞🪞🪞🪞".into()), color: None }).is_err(), "one emoji, not a sentence");
        set_avatar("Mirror", Look { emoji: Some(" 🪞 ".into()), color: Some("#0090FF".into()) }).unwrap();
        set("Mirror 2", &session(a, None), "app", None, false).unwrap();
        let c = find("Mirror 2").unwrap();
        assert_eq!(c.avatar, Some(Look { emoji: Some("🪞".into()), color: Some("#0090ff".into()) }), "renaming keeps the picture");
        set_avatar("Mirror 2", Look::default()).unwrap();
        assert_eq!(find("Mirror 2").unwrap().avatar, None, "an empty picture takes it away");
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
