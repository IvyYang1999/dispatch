//! What a session is for, so a sender can pick a recipient by role instead of guessing among
//! threads named after their first task.
//!
//! The line about a session comes from, in this order:
//! - the address book: what the user wrote there, or the session when it was named;
//! - the session itself: one line it wrote about itself (`about.json`), for a session that
//!   nobody named. It does not put the session in the address book, so a session that
//!   described itself is not listed for ever;
//! - for a session on another machine, what that machine said when it listed it.
//!
//! A role (`roles.json`) is a name for something that has no session of its own: a scheduled
//! task, a bot that only runs now and then. It points at the session that answers for it. A
//! search for the role finds that session, and a message to the role goes to that session.

use crate::contacts::Contact;
use crate::model::{Harness, Host, Session};
use crate::store::Store;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The longest line about a session or a role.
const MAX: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Own {
    text: String,
    #[serde(default)]
    at: String,
}

/// Something a session answers for that has no session of its own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    pub session_id: String,
    pub harness: Harness,
    /// The machine the session is on, when it is another one.
    #[serde(default)]
    pub host: Option<Host>,
    /// One line on what it is: what a sender matches when looking for it.
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub set_by: String,
    #[serde(default)]
    pub updated: String,
}

/// A role as a sender sees it on the session that answers for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleLine {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RoleBook {
    #[serde(default)]
    roles: Vec<Role>,
}

fn own_path() -> PathBuf {
    Store::open().root().join("about.json")
}

fn roles_path() -> PathBuf {
    Store::open().root().join("roles.json")
}

fn own_book() -> HashMap<String, Own> {
    std::fs::read_to_string(own_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write(path: PathBuf, text: String) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// One line, trimmed and cut to length; None when nothing is left.
fn line(text: &str) -> Option<String> {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one.chars().take(MAX).collect::<String>()).filter(|t| !t.is_empty())
}

/// What a session on this machine wrote about itself.
pub fn own(session_id: &str) -> Option<String> {
    own_book().remove(&session_id.to_lowercase()).map(|o| o.text)
}

/// A session on this machine says what it is for; an empty line takes it back.
pub fn set_own(session_id: &str, text: &str) -> Result<Option<String>> {
    let mut book = own_book();
    let text = line(text);
    match &text {
        Some(t) => {
            book.insert(session_id.to_lowercase(), Own { text: t.clone(), at: crate::store::now_text() });
        }
        None => {
            book.remove(&session_id.to_lowercase());
        }
    }
    write(own_path(), serde_json::to_string_pretty(&book)?)?;
    Ok(text)
}

pub fn roles() -> Vec<Role> {
    std::fs::read_to_string(roles_path()).ok().and_then(|t| serde_json::from_str::<RoleBook>(&t).ok()).map(|b| b.roles).unwrap_or_default()
}

/// A role by name; case does not matter.
pub fn role(name: &str) -> Option<Role> {
    let q = name.trim();
    roles().into_iter().find(|r| r.name.eq_ignore_ascii_case(q))
}

/// Name something `s` answers for. A role of that name is replaced; a session's own name in
/// the address book is not a role and is left alone.
pub fn set_role(name: &str, s: &Session, by: &str, about: Option<&str>) -> Result<Role> {
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
    if let Some(c) = crate::contacts::find(name) {
        bail!("\"{}\" is already the name of a session in the address book; a role needs a name of its own", c.name);
    }
    let mut all = roles();
    let previous = all.iter().find(|r| r.name.eq_ignore_ascii_case(name)).and_then(|r| r.about.clone());
    let r = Role { name: name.to_string(), session_id: s.id.to_lowercase(), harness: s.harness, host: s.host.clone(), about: about.and_then(line).or(previous), set_by: by.to_string(), updated: crate::store::now_text() };
    all.retain(|x| !x.name.eq_ignore_ascii_case(name));
    all.push(r.clone());
    write(roles_path(), serde_json::to_string_pretty(&RoleBook { roles: all })?)?;
    Ok(r)
}

pub fn remove_role(name: &str) -> Result<Option<Role>> {
    let mut all = roles();
    let Some(i) = all.iter().position(|r| r.name.eq_ignore_ascii_case(name.trim())) else { return Ok(None) };
    let r = all.remove(i);
    write(roles_path(), serde_json::to_string_pretty(&RoleBook { roles: all })?)?;
    Ok(Some(r))
}

/// What is known about one session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub about: Option<String>,
    /// Where the line is from: `book` (the address book), `self` (the session wrote it) or
    /// `machine` (its own machine said so).
    pub by: Option<&'static str>,
    pub roles: Vec<RoleLine>,
}

impl Known {
    /// The line and every role's name and line, lowercased: what a search looks in.
    pub fn text(&self) -> (String, String) {
        let names = self.roles.iter().map(|r| r.name.to_lowercase()).collect::<Vec<_>>().join("\n");
        let mut about = self.about.clone().unwrap_or_default().to_lowercase();
        for r in &self.roles {
            if let Some(a) = &r.about {
                about.push('\n');
                about.push_str(&a.to_lowercase());
            }
        }
        (names, about)
    }
}

/// The files read once, for asking about many sessions.
pub struct View {
    contacts: Vec<Contact>,
    own: HashMap<String, Own>,
    roles: Vec<Role>,
}

impl View {
    pub fn load() -> View {
        View::with(crate::contacts::list())
    }

    pub fn with(contacts: Vec<Contact>) -> View {
        View { contacts, own: own_book(), roles: roles() }
    }

    pub fn contacts(&self) -> &[Contact] {
        &self.contacts
    }

    pub fn of(&self, s: &Session) -> Known {
        let host = s.host.as_ref().map(|h| h.id.as_str());
        let same = |id: &str, h: Option<&Host>| id.eq_ignore_ascii_case(&s.id) && h.map(|h| h.id.as_str()) == host;
        let told = host.and_then(|h| crate::peer::told(h, &s.id));
        let book = self.contacts.iter().find(|c| same(&c.session_id, c.host.as_ref())).and_then(|c| c.description.clone()).filter(|d| !d.trim().is_empty());
        let (about, by) = match book {
            Some(d) => (Some(d), Some("book")),
            None if host.is_none() => match self.own.get(&s.id.to_lowercase()) {
                Some(o) => (Some(o.text.clone()), Some("self")),
                None => (None, None),
            },
            None => match told.as_ref().and_then(|t| t.about.clone()) {
                Some(a) => (Some(a), Some("machine")),
                None => (None, None),
            },
        };
        let mut roles: Vec<RoleLine> = self.roles.iter().filter(|r| same(&r.session_id, r.host.as_ref())).map(|r| RoleLine { name: r.name.clone(), about: r.about.clone() }).collect();
        for r in told.map(|t| t.roles).unwrap_or_default() {
            if !roles.iter().any(|x| x.name.eq_ignore_ascii_case(&r.name)) {
                roles.push(r);
            }
        }
        Known { about, by, roles }
    }
}

/// What is known about one session (reads the files; use a `View` for many).
pub fn of(s: &Session) -> Known {
    View::load().of(s)
}

/// How long ago a session was last active, as a sender reads it among candidates.
pub fn active_text(running: bool, last_active_ms: i64, now_ms: i64) -> String {
    if running {
        return "running".into();
    }
    if last_active_ms <= 0 {
        return "unknown".into();
    }
    let days = (now_ms - last_active_ms).max(0) / 86_400_000;
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=13 => format!("{days} days ago"),
        14..=59 => format!("{} weeks ago", days / 7),
        60..=729 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::short_id;

    fn session(id: &str) -> Session {
        Session { harness: Harness::Codex, id: id.into(), short_id: short_id(id), name: None, cwd: Some("/w/handoff".into()), title: Some("修复某某".into()), last_active: 5, running: false, desktop: false, host_session_id: None, status: None, transcript: None, host: None, project: None, app_name: None }
    }

    #[test]
    fn a_session_says_what_it_is_for_and_the_address_book_has_the_last_word() {
        let dir = std::env::temp_dir().join(format!("dispatch-about-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "11111111-aaaa-4bbb-8ccc-dddddddddddd";
        assert_eq!(of(&session(a)), Known::default(), "nothing says what it is for");
        // its own line: one line, cut to length, and it is not in the address book for that
        set_own(a, "  负责 Handoff 的代码；\n不要找我发版  ").unwrap();
        let k = of(&session(a));
        assert_eq!(k.about.as_deref(), Some("负责 Handoff 的代码； 不要找我发版"));
        assert_eq!(k.by, Some("self"));
        assert!(crate::contacts::list().is_empty());
        assert_eq!(set_own(a, &"长".repeat(500)).unwrap().unwrap().chars().count(), 200);
        // what the user wrote in the address book is what others see
        crate::contacts::set("Handoff dev", &session(a), "app", Some("用户写的那句"), false).unwrap();
        let k = of(&session(a));
        assert_eq!((k.about.as_deref(), k.by), (Some("用户写的那句"), Some("book")));
        // a name without a line leaves the session's own line showing
        crate::contacts::remove("Handoff dev").unwrap();
        crate::contacts::set("Handoff dev", &session(a), "app", None, false).unwrap();
        assert_eq!(of(&session(a)).by, Some("self"));
        // an empty line takes it back
        assert_eq!(set_own(a, "  ").unwrap(), None);
        assert_eq!(of(&session(a)).about, None);
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_role_points_at_the_session_that_answers_for_it() {
        let dir = std::env::temp_dir().join(format!("dispatch-roles-{}", uuid::Uuid::new_v4()));
        crate::store::set_thread_root(Some(dir.clone()));
        let a = "22222222-aaaa-4bbb-8ccc-dddddddddddd";
        let b = "33333333-aaaa-4bbb-8ccc-dddddddddddd";
        crate::contacts::set("Handoff dev", &session(a), "app", None, false).unwrap();
        assert!(set_role("Handoff dev", &session(b), "self", None).is_err(), "a session's name is not free for a role");
        assert!(set_role("a/b", &session(a), "self", None).is_err());
        set_role("handoff助理", &session(a), "self", Some("每 30 分钟跑一次的例行任务")).unwrap();
        set_role("周报", &session(a), "self", None).unwrap();
        // the session keeps its own name, and carries both roles
        assert_eq!(crate::contacts::for_session(a, None).unwrap().name, "Handoff dev");
        let k = of(&session(a));
        assert_eq!(k.roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["handoff助理", "周报"]);
        let (names, about) = k.text();
        assert!(names.contains("handoff助理") && about.contains("例行任务"));
        assert!(of(&session(b)).roles.is_empty());
        // set again: it moves to the other session and keeps its line
        set_role("HANDOFF助理", &session(b), "self", None).unwrap();
        assert_eq!(roles().len(), 2);
        assert_eq!(role("handoff助理").unwrap().session_id, b);
        assert_eq!(role("handoff助理").unwrap().about.as_deref(), Some("每 30 分钟跑一次的例行任务"));
        assert!(remove_role("周报").unwrap().is_some());
        assert!(remove_role("周报").unwrap().is_none());
        crate::store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn how_long_ago_reads_at_a_glance() {
        let day = 86_400_000;
        let now = 1_000 * day;
        assert_eq!(active_text(true, 0, now), "running");
        assert_eq!(active_text(false, 0, now), "unknown");
        assert_eq!(active_text(false, now - 1000, now), "today");
        assert_eq!(active_text(false, now - day, now), "yesterday");
        assert_eq!(active_text(false, now - 9 * day, now), "9 days ago");
        assert_eq!(active_text(false, now - 30 * day, now), "4 weeks ago");
        assert_eq!(active_text(false, now - 65 * day, now), "2 months ago");
        assert_eq!(active_text(false, now - 800 * day, now), "2 years ago");
    }
}
