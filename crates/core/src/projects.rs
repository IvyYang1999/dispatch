//! Projects: how the user files sessions.
//!
//! A session starts out in the project of its folder: every folder (on each machine) is a
//! project of its own, named after the folder, with nothing written down. The user can then
//! put things together by hand: move one session into another project, or merge two projects
//! (two folders on two machines that are the same work). Only what the user did is stored, in
//! `projects.json` in the store folder:
//!
//! - `projects`: the ones the user made or changed, each with a name and the folders filed
//!   under it (`<machine>|<path>`, the machine empty for this one);
//! - `moved`: sessions put somewhere by hand (`<machine>|<session id>` to a project id);
//! - `dismissed`: suggestions the user turned down;
//! - `order`: the projects in the order the user dragged them into;
//! - `updated`: when the user last changed any of it.
//!
//! Paired machines keep one book between them: they compare `updated` and the newer book wins
//! on both (see `peer::sync_projects`). On the wire every machine goes by its id; on disk the
//! machine the file is on is the empty id, so `to_wire` / `from_wire` translate.
//!
//! Dispatch only ever suggests (two folders with the same name on two machines); it never
//! merges on its own.

use crate::model::{Harness, Session};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Book {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub moved: BTreeMap<String, String>,
    #[serde(default)]
    pub dismissed: Vec<String>,
    /// Group ids in the order the user put them; a group not listed is new and goes on top.
    #[serde(default)]
    pub order: Vec<String>,
    /// When the user last changed the book (ms since 1970): which copy is newer.
    #[serde(default)]
    pub updated: i64,
    /// A setting, not part of the book: Grok bots are always filed as in the Grok Bot app,
    /// whatever was moved by hand (`grok::sections_mode` is `follow`).
    #[serde(skip)]
    pub follow_grok: bool,
}

/// Where a session is filed. `auto` is a folder nobody has touched: it exists only as long as a
/// session is in it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub auto: bool,
}

const FOLDER: &str = "folder:";
const GROK: &str = "harness:grok";
/// A section of the Grok Bot app's sidebar, by its name.
const GROK_SECTION: &str = "grok:";
const NONE: &str = "none";

fn path() -> PathBuf {
    crate::store::Store::open().root().join("projects.json")
}

pub fn load() -> Book {
    let mut book = load_at(&path());
    book.follow_grok = crate::grok::sections_mode() == "follow";
    book
}

pub fn load_at(path: &std::path::Path) -> Book {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_at(path: &std::path::Path, book: &Book) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(book)? + "\n")?;
    Ok(())
}

/// `<machine>|<rest>`: the machine's id, empty for this one.
fn key(host: Option<&str>, rest: &str) -> String {
    format!("{}|{}", host.unwrap_or(""), rest)
}

pub fn session_key(s: &Session) -> String {
    key(s.host.as_ref().map(|h| h.id.as_str()), &s.id.to_lowercase())
}

fn folder_key(s: &Session) -> Option<String> {
    let cwd = s.cwd.as_deref().map(|c| c.trim_end_matches(['/', '\\'])).filter(|c| !c.is_empty())?;
    Some(key(s.host.as_ref().map(|h| h.id.as_str()), cwd))
}

/// The last part of a folder key's path: what an untouched folder is called.
fn folder_name(folder: &str) -> String {
    let path = folder.split_once('|').map(|(_, p)| p).unwrap_or(folder);
    path.rsplit(['/', '\\']).find(|p| !p.is_empty()).unwrap_or(path).to_string()
}

impl Book {
    fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Where this session is filed: where the user put it, else the project its folder is
    /// filed under, else its folder; a bot has no folder and stands with the other bots.
    pub fn group_of(&self, s: &Session) -> Group {
        if self.follow_grok && s.harness == Harness::Grok && s.cwd.is_none() {
            return self.home_of(s);
        }
        if let Some(p) = self.moved.get(&session_key(s)).and_then(|id| self.project(id)) {
            return Group { id: p.id.clone(), name: p.name.clone(), auto: false };
        }
        self.home_of(s)
    }

    /// Where it would be without having been moved by hand.
    fn home_of(&self, s: &Session) -> Group {
        match folder_key(s) {
            Some(f) => match self.projects.iter().find(|p| p.folders.contains(&f)) {
                Some(p) => Group { id: p.id.clone(), name: p.name.clone(), auto: false },
                None => Group { name: folder_name(&f), id: format!("{FOLDER}{f}"), auto: true },
            },
            // a bot has no folder: it sits where the Grok Bot app's sidebar has it, else with the other bots
            None if s.harness == Harness::Grok => match s.project.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(section) => Group { id: format!("{GROK_SECTION}{section}"), name: section.to_string(), auto: true },
                None => Group { id: GROK.into(), name: "Grok Bot".into(), auto: true },
            },
            None => Group { id: NONE.into(), name: String::new(), auto: true },
        }
    }

    /// The project behind a group id, made real when it was only a folder. A folder becomes a
    /// project named after it, holding that folder.
    fn settle(&mut self, id: &str) -> Result<String> {
        if self.project(id).is_some() {
            return Ok(id.to_string());
        }
        // the bots' group and "not filed" are not places to file things: only folders settle
        let folder = id.strip_prefix(FOLDER).ok_or_else(|| anyhow!("no such project: {id}"))?;
        if let Some(p) = self.projects.iter().find(|p| p.folders.iter().any(|f| f == folder)) {
            return Ok(p.id.clone());
        }
        let new = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        self.projects.push(Project { id: new.clone(), name: folder_name(folder), folders: vec![folder.to_string()] });
        // it keeps the place the folder had
        for o in &mut self.order {
            if o == id {
                *o = new.clone();
            }
        }
        Ok(new)
    }

    /// A project with no folder and no session left in it is gone.
    fn sweep(&mut self) {
        let used: std::collections::HashSet<&String> = self.moved.values().collect();
        self.projects.retain(|p| !p.folders.is_empty() || used.contains(&p.id));
    }

    /// Put one session into a project (`target`: a project id, a folder group's id, or `new`
    /// with a name). Putting it back where its folder is filed just forgets the move.
    pub fn move_session(&mut self, s: &Session, target: &str, new_name: Option<&str>) -> Result<String> {
        let k = session_key(s);
        if target == self.home_of(s).id {
            self.moved.remove(&k);
            self.sweep();
            return Ok(target.to_string());
        }
        let id = if target == "new" {
            let name = new_name.map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| anyhow!("a new project needs a name"))?;
            let new = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
            self.projects.push(Project { id: new.clone(), name: name.to_string(), folders: vec![] });
            new
        } else {
            self.settle(target)?
        };
        self.moved.insert(k, id.clone());
        self.sweep();
        Ok(id)
    }

    /// File a whole folder under a project: every session in it, now and later.
    pub fn move_folder(&mut self, folder: &str, target: &str) -> Result<String> {
        let id = self.settle(target)?;
        for p in &mut self.projects {
            p.folders.retain(|f| f != folder);
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.folders.push(folder.to_string());
        }
        self.sweep();
        Ok(id)
    }

    /// Take a folder out of the project it was filed under: it stands on its own again.
    pub fn release_folder(&mut self, folder: &str) {
        for p in &mut self.projects {
            p.folders.retain(|f| f != folder);
        }
        self.sweep();
    }

    /// Everything in `from` goes to `into`; `from` is gone.
    pub fn merge(&mut self, from: &str, into: &str) -> Result<String> {
        if from == into {
            return Ok(into.to_string());
        }
        let into = self.settle(into)?;
        let folders: Vec<String> = match from.strip_prefix(FOLDER) {
            Some(f) => vec![f.to_string()],
            None => self.project(from).map(|p| p.folders.clone()).unwrap_or_default(),
        };
        self.projects.retain(|p| p.id != from);
        for v in self.moved.values_mut() {
            if v == from {
                *v = into.clone();
            }
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == into) {
            for f in folders {
                if !p.folders.contains(&f) {
                    p.folders.push(f);
                }
            }
        }
        self.sweep();
        Ok(into)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(anyhow!("a project needs a name"));
        }
        let id = self.settle(id)?;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.name = name.to_string();
        }
        Ok(id)
    }

    /// Undo a project: its folders stand on their own again and sessions go back to theirs.
    pub fn dissolve(&mut self, id: &str) {
        self.projects.retain(|p| p.id != id);
        self.moved.retain(|_, v| v != id);
    }

    /// Folders that look like the same work filed apart: an untouched folder whose name another
    /// group already has (the same repository on another machine, usually). Offered, never done.
    pub fn suggestions(&self, sessions: &[Session]) -> Vec<Value> {
        let mut groups: Vec<Group> = vec![];
        for s in sessions {
            let g = self.group_of(s);
            if !g.name.is_empty() && g.id != GROK && !g.id.starts_with(GROK_SECTION) && !groups.iter().any(|x| x.id == g.id) {
                groups.push(g);
            }
        }
        let mut out = vec![];
        for (i, a) in groups.iter().enumerate() {
            for b in groups.iter().skip(i + 1) {
                if !(a.auto || b.auto) || a.name.to_lowercase() != b.name.to_lowercase() {
                    continue;
                }
                // the one the user already made keeps its name and takes the other in
                let (from, into) = if a.auto { (a, b) } else { (b, a) };
                let mut pair = [from.id.clone(), into.id.clone()];
                pair.sort();
                let key = pair.join(" + ");
                if !self.dismissed.contains(&key) {
                    out.push(json!({ "key": key, "name": into.name, "from": from.id, "into": into.id }));
                }
            }
        }
        out
    }
}

/// Change the book on disk, and note when: the newer book is the one paired machines keep.
pub fn change<T>(f: impl FnOnce(&mut Book) -> Result<T>) -> Result<T> {
    let p = path();
    let mut book = load_at(&p);
    let out = f(&mut book)?;
    book.updated = crate::model::now_ms().max(book.updated + 1);
    save_at(&p, &book)?;
    Ok(out)
}

/// Take another machine's book as this machine's, as it is (its `updated` included).
pub fn adopt(book: &Book) -> Result<()> {
    save_at(&path(), book)
}

/// `<machine>|<rest>` with the machine renamed.
fn rehost(key: &str, f: &impl Fn(&str) -> String) -> String {
    match key.split_once('|') {
        Some((host, rest)) => format!("{}|{rest}", f(host)),
        None => key.to_string(),
    }
}

/// A group id with its machine renamed: only a folder's id names one.
fn rehost_id(id: &str, f: &impl Fn(&str) -> String) -> String {
    match id.strip_prefix(FOLDER) {
        Some(folder) => format!("{FOLDER}{}", rehost(folder, f)),
        None => id.to_string(),
    }
}

fn rehosted(book: &Book, f: impl Fn(&str) -> String) -> Book {
    Book {
        projects: book.projects.iter().map(|p| Project { id: p.id.clone(), name: p.name.clone(), folders: p.folders.iter().map(|x| rehost(x, &f)).collect() }).collect(),
        moved: book.moved.iter().map(|(k, v)| (rehost(k, &f), v.clone())).collect(),
        dismissed: book
            .dismissed
            .iter()
            .map(|k| {
                let mut pair: Vec<String> = k.split(" + ").map(|id| rehost_id(id, &f)).collect();
                pair.sort();
                pair.join(" + ")
            })
            .collect(),
        order: book.order.iter().map(|id| rehost_id(id, &f)).collect(),
        updated: book.updated,
        follow_grok: book.follow_grok,
    }
}

/// The book as another machine reads it: every machine by its id, this one (`me`) included.
pub fn to_wire(book: &Book, me: &str) -> Book {
    rehosted(book, |h| if h.is_empty() { me.to_string() } else { h.to_string() })
}

/// A book from the wire as this machine (`me`) keeps it: this machine is the empty id.
pub fn from_wire(book: &Book, me: &str) -> Book {
    rehosted(book, |h| if h == me { String::new() } else { h.to_string() })
}

/// The folder a session's own folder key names: what "file its whole folder" acts on.
pub fn folder_of(s: &Session) -> Option<String> {
    folder_key(s)
}

/// What the app shows for a group: where its folders are, for the header's tooltip.
pub fn describe(book: &Book, id: &str) -> Vec<String> {
    match id.strip_prefix(FOLDER) {
        Some(f) => vec![f.to_string()],
        None => book.project(id).map(|p| p.folders.clone()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Host;

    fn session(id: &str, cwd: Option<&str>, host: Option<&str>, harness: Harness) -> Session {
        Session {
            harness,
            id: id.into(),
            short_id: id.chars().take(8).collect(),
            name: None,
            cwd: cwd.map(str::to_string),
            title: None,
            last_active: 0,
            running: false,
            desktop: false,
            host_session_id: None,
            status: None,
            transcript: None,
            host: host.map(|h| Host { id: h.into(), name: h.into() }),
            project: None,
            app_name: None,
        }
    }

    #[test]
    fn a_session_starts_in_its_folder() {
        let book = Book::default();
        let a = session("a", Some("/code/shop/"), None, Harness::Claude);
        let b = session("b", Some("/code/shop"), None, Harness::Codex);
        let c = session("c", Some("/code/shop"), Some("mini"), Harness::Codex);
        let ga = book.group_of(&a);
        assert_eq!((ga.name.as_str(), ga.auto), ("shop", true));
        assert_eq!(ga.id, book.group_of(&b).id, "the same folder is one project, whatever the agent");
        assert_ne!(ga.id, book.group_of(&c).id, "the same path on another machine is another folder");
        assert_eq!(book.group_of(&session("g", None, None, Harness::Grok)).id, GROK);
        assert_eq!(book.group_of(&session("n", None, None, Harness::Claude)).id, NONE);
    }

    #[test]
    fn folders_on_two_machines_merge_into_one_project_and_stay_merged() {
        let mut book = Book::default();
        let here = session("a", Some("/code/shop"), None, Harness::Claude);
        let there = session("c", Some("/work/shop-app"), Some("mini"), Harness::Codex);
        let (from, into) = (book.group_of(&there).id, book.group_of(&here).id);
        let id = book.merge(&from, &into).unwrap();
        assert_eq!(book.group_of(&here).id, id);
        assert_eq!(book.group_of(&there).id, id);
        assert_eq!(book.group_of(&here).name, "shop", "the one it was merged into keeps its name");
        // a session started later in either folder lands in the project
        assert_eq!(book.group_of(&session("later", Some("/work/shop-app"), Some("mini"), Harness::Claude)).id, id);
        // taken out again, the folder stands alone and the project keeps the rest
        book.release_folder("mini|/work/shop-app");
        assert!(book.group_of(&there).auto);
        assert_eq!(book.group_of(&here).id, id);
        book.merge(&book.group_of(&there).id, &id).unwrap();
        let id2 = book.rename(&id, "Storefront").unwrap();
        assert_eq!((id2.as_str(), book.group_of(&there).name.as_str()), (id.as_str(), "Storefront"));
        book.dissolve(&id);
        assert!(book.group_of(&here).auto && book.group_of(&there).auto, "undone: each folder on its own again");
        assert!(book.projects.is_empty());
    }

    #[test]
    fn one_session_can_be_moved_and_put_back() {
        let mut book = Book::default();
        let a = session("a", Some("/code/shop"), None, Harness::Claude);
        let b = session("b", Some("/code/shop"), None, Harness::Claude);
        let other = session("o", Some("/code/docs"), None, Harness::Claude);
        let home = book.group_of(&a).id;
        let docs = book.group_of(&other).id;
        let id = book.move_session(&a, &docs, None).unwrap();
        assert_eq!(book.group_of(&a).id, id);
        assert_eq!(book.group_of(&other).id, id, "the folder it was dropped on became that project");
        assert_eq!(book.group_of(&b).id, home, "only the one session moved");
        book.move_session(&a, &home, None).unwrap();
        assert_eq!(book.group_of(&a).id, home);
        assert!(book.moved.is_empty(), "back home: nothing left to remember");
        // a project of its own, by name; an empty name is refused
        assert!(book.move_session(&a, "new", Some("  ")).is_err());
        let solo = book.move_session(&a, "new", Some("Launch")).unwrap();
        assert_eq!(book.group_of(&a).name, "Launch");
        book.move_session(&a, &home, None).unwrap();
        assert!(book.project(&solo).is_none(), "a project with nothing in it is gone");
        // a bot has no folder: it can still be filed by hand
        let bot = session("g", None, None, Harness::Grok);
        let id = book.move_session(&bot, &docs, None).unwrap();
        assert_eq!(book.group_of(&bot).id, id);
    }

    #[test]
    fn a_bot_starts_where_the_grok_app_has_it_and_can_be_made_to_stay_there() {
        let mut book = Book::default();
        let mut bot = session("g", None, None, Harness::Grok);
        bot.project = Some("运营".into());
        let home = book.group_of(&bot);
        assert_eq!((home.id.as_str(), home.name.as_str(), home.auto), ("grok:运营", "运营", true));
        // moved by hand, it stays moved
        let docs = book.group_of(&session("o", Some("/code/docs"), None, Harness::Claude)).id;
        let id = book.move_session(&bot, &docs, None).unwrap();
        assert_eq!(book.group_of(&bot).id, id);
        // unless the user asked for bots to always be filed as in Grok Bot
        book.follow_grok = true;
        assert_eq!(book.group_of(&bot).id, "grok:运营");
        // a section is not something to file other sessions under, and is never suggested for merging
        assert!(book.move_session(&session("x", Some("/code/运营"), None, Harness::Claude), "grok:运营", None).is_err());
        assert!(book.suggestions(&[bot.clone(), session("x", Some("/code/运营"), None, Harness::Claude)]).is_empty());
    }

    #[test]
    fn a_whole_folder_can_follow_a_session() {
        let mut book = Book::default();
        let a = session("a", Some("/code/shop"), None, Harness::Claude);
        let other = session("o", Some("/code/docs"), None, Harness::Claude);
        let docs = book.group_of(&other).id;
        let id = book.move_folder(&folder_of(&a).unwrap(), &docs).unwrap();
        assert_eq!(book.group_of(&a).id, id);
        assert_eq!(book.group_of(&session("b", Some("/code/shop"), None, Harness::Codex)).id, id);
    }

    #[test]
    fn same_named_folders_are_suggested_once_and_never_merged_unasked() {
        let mut book = Book::default();
        let all = vec![
            session("a", Some("/code/dispatch"), None, Harness::Claude),
            session("b", Some("/Users/x/Developer/Dispatch"), Some("mini"), Harness::Codex),
            session("c", Some("/code/other"), None, Harness::Claude),
        ];
        let s = book.suggestions(&all);
        assert_eq!(s.len(), 1);
        assert_ne!(book.group_of(&all[0]).id, book.group_of(&all[1]).id, "suggested, not done");
        book.dismissed.push(s[0]["key"].as_str().unwrap().to_string());
        assert!(book.suggestions(&all).is_empty(), "turned down: not offered again");
        book.dismissed.clear();
        book.merge(s[0]["from"].as_str().unwrap(), s[0]["into"].as_str().unwrap()).unwrap();
        assert!(book.suggestions(&all).is_empty(), "merged: nothing left to suggest");
        assert_eq!(book.group_of(&all[0]).id, book.group_of(&all[1]).id);
    }

    /// The same book on two machines: what is "this machine" on one is the other machine on the
    /// other, in folders, moved sessions, the order and turned-down suggestions alike.
    #[test]
    fn a_book_reads_the_same_from_both_machines() {
        let mut here = Book::default();
        let mine = session("a", Some("/code/shop"), None, Harness::Claude);
        let theirs = session("c", Some("/work/shop"), Some("mini"), Harness::Codex);
        let id = here.merge(&here.group_of(&theirs).id, &here.group_of(&mine).id).unwrap();
        here.move_session(&session("solo", Some("/code/notes"), None, Harness::Claude), &id, None).unwrap();
        here.order = vec![id.clone(), "folder:|/code/notes".into(), "folder:mini|/work/other".into()];
        here.dismissed = vec!["folder:mini|/work/x + folder:|/code/x".into()];
        here.updated = 42;

        // as the Mac mini keeps it: the book travels with every machine named, and the mini is "here" there
        let there = from_wire(&to_wire(&here, "laptop"), "mini");
        let as_mini_sees_mine = session("a", Some("/code/shop"), Some("laptop"), Harness::Claude);
        let as_mini_sees_its_own = session("c", Some("/work/shop"), None, Harness::Codex);
        assert_eq!(there.group_of(&as_mini_sees_mine).id, id);
        assert_eq!(there.group_of(&as_mini_sees_its_own).id, id);
        assert_eq!(there.group_of(&session("solo", Some("/code/notes"), Some("laptop"), Harness::Claude)).id, id);
        assert_eq!(there.order, vec![id.clone(), "folder:laptop|/code/notes".to_string(), "folder:|/work/other".to_string()]);
        assert_eq!(there.dismissed, vec!["folder:laptop|/code/x + folder:|/work/x".to_string()], "the pair is sorted again under its new names");
        assert_eq!(there.updated, 42);
        // and back again it is the book it was
        assert_eq!(from_wire(&to_wire(&there, "mini"), "laptop"), here);
    }

    #[test]
    fn a_folder_made_a_project_keeps_its_place_in_the_order() {
        let mut book = Book::default();
        let a = session("a", Some("/code/shop"), None, Harness::Claude);
        let folder = book.group_of(&a).id;
        book.order = vec!["folder:|/code/docs".into(), folder.clone()];
        let id = book.rename(&folder, "Storefront").unwrap();
        assert_eq!(book.order, vec!["folder:|/code/docs".to_string(), id]);
    }

    #[test]
    fn the_book_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("dispatch-projects-{}", uuid::Uuid::new_v4()));
        let file = dir.join("projects.json");
        assert_eq!(load_at(&file), Book::default(), "no file yet: nothing filed by hand");
        let mut book = Book::default();
        let a = session("a", Some("/code/shop"), None, Harness::Claude);
        book.move_session(&a, "new", Some("Launch")).unwrap();
        save_at(&file, &book).unwrap();
        assert_eq!(load_at(&file), book);
        let _ = std::fs::remove_dir_all(dir);
    }
}
