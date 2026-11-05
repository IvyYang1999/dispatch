//! The message store: one Markdown file per recipient copy, a run log per message.
//!
//! The file format is the one Agent Comments' mailbox hook already reads
//! (`from`, `to`, `to_name`, `urgency`, `wake`, `status`, `created`), plus `id`
//! (this copy), `dispatch_id` (the dispatch it belongs to) and `from_name`.
//! A dispatch to three recipients is three files sharing one `dispatch_id`.
//!
//! Read state (`status` in the file) and delivery attempts (`runs/<id>.jsonl`)
//! are two separate records on purpose: a retry never rewrites history, and a
//! person can read or hand-edit the message without touching the log.

use crate::model::{home_dir, short_id};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const UNREAD: &str = "未读";
pub const DELIVERED_INBOX: &str = "已送达（直投）";
pub const DELIVERED_CODEX: &str = "已送达（Codex 已入队）";
pub const READ_BY_HOOK: &str = "已读（hook代标）";
pub const REPLIED: &str = "已回复";
pub const URGENCY_NORMAL: &str = "普通";
pub const URGENCY_URGENT: &str = "紧急";

/// The urgency as the file stores it: `urgent` / `紧急` → `紧急`, anything else → `普通`.
pub fn urgency_stored(s: &str) -> &'static str {
    match s.trim().to_ascii_lowercase().as_str() {
        "urgent" | "紧急" | "high" => URGENCY_URGENT,
        _ => URGENCY_NORMAL,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Letter {
    pub id: String,
    pub dispatch_id: String,
    pub from: String,
    pub from_name: String,
    pub to: String,
    pub to_name: String,
    pub urgency: String,
    pub wake: bool,
    pub status: String,
    pub created: String,
    pub body: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub at: String,
    pub action: String,
    pub result: String,
    pub detail: String,
}

pub struct Store {
    root: PathBuf,
}

pub fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M").to_string()
}

fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r') { '_' } else { c }).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "未命名".into()
    } else {
        t.chars().take(24).collect()
    }
}

impl Store {
    /// `DISPATCH_HOME`, else the platform data folder (`~/Library/Application Support/Dispatch` on macOS).
    pub fn open() -> Store {
        let root = match std::env::var("DISPATCH_HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => dirs::data_dir().unwrap_or_else(|| home_dir().join(".local/share")).join("Dispatch"),
        };
        Store { root }
    }

    pub fn at(root: PathBuf) -> Store {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn messages_dir(&self) -> PathBuf {
        self.root.join("messages")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    pub fn box_dir(&self, session_id: &str) -> PathBuf {
        self.messages_dir().join(short_id(session_id))
    }

    pub fn write_letter(&self, l: &mut Letter) -> Result<PathBuf> {
        let dir = self.box_dir(&l.to);
        fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{}-来自{}-{}-{}.md", stamp(), safe_name(&l.from_name), l.urgency, short_id(&l.id)));
        let text = render(l);
        // Publish whole: write beside, then rename.
        let tmp = dir.join(format!(".{}.tmp", short_id(&l.id)));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &file)?;
        l.path = file.to_string_lossy().to_string();
        Ok(file)
    }

    /// Every letter, newest first; `to` narrows to one recipient, `unread_only` to `未读`.
    pub fn list(&self, to: Option<&str>, unread_only: bool) -> Result<Vec<Letter>> {
        let mut out = vec![];
        let dirs: Vec<PathBuf> = match to {
            Some(t) => vec![self.box_dir(t)],
            None => fs::read_dir(self.messages_dir()).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default(),
        };
        for d in dirs {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("md") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&p) else { continue };
                let Some(mut l) = parse(&text) else { continue };
                if let Some(t) = to {
                    if !l.to.eq_ignore_ascii_case(t) {
                        continue;
                    }
                }
                if unread_only && l.status != UNREAD {
                    continue;
                }
                l.path = p.to_string_lossy().to_string();
                out.push(l);
            }
        }
        out.sort_by(|a, b| b.created.cmp(&a.created).then(b.path.cmp(&a.path)));
        Ok(out)
    }

    /// A letter by its id, a prefix of it, or its dispatch id (first copy).
    pub fn find(&self, id: &str) -> Result<Option<Letter>> {
        let all = self.list(None, false)?;
        Ok(all.iter().find(|l| l.id.starts_with(id)).or_else(|| all.iter().find(|l| l.dispatch_id.starts_with(id))).cloned())
    }

    pub fn copies(&self, dispatch_id: &str) -> Result<Vec<Letter>> {
        Ok(self.list(None, false)?.into_iter().filter(|l| l.dispatch_id == dispatch_id).collect())
    }

    /// Rewrite the `status:` line in place.
    pub fn set_status(&self, l: &mut Letter, status: &str) -> Result<()> {
        let text = fs::read_to_string(&l.path).with_context(|| format!("read {}", l.path))?;
        let mut done = false;
        let out: Vec<String> = text
            .lines()
            .map(|line| {
                if !done && line.starts_with("status:") {
                    done = true;
                    format!("status: {status}")
                } else {
                    line.to_string()
                }
            })
            .collect();
        if !done {
            return Err(anyhow!("no status line in {}", l.path));
        }
        let tmp = format!("{}.tmp", l.path);
        fs::write(&tmp, out.join("\n") + "\n")?;
        fs::rename(&tmp, &l.path)?;
        l.status = status.to_string();
        Ok(())
    }

    pub fn append_run(&self, letter_id: &str, action: &str, result: &str, detail: &str) -> Result<()> {
        fs::create_dir_all(self.runs_dir())?;
        let run = Run { at: chrono::Local::now().to_rfc3339(), action: action.into(), result: result.into(), detail: detail.into() };
        let mut f = fs::OpenOptions::new().create(true).append(true).open(self.runs_dir().join(format!("{letter_id}.jsonl")))?;
        writeln!(f, "{}", serde_json::to_string(&run)?)?;
        Ok(())
    }

    pub fn runs(&self, letter_id: &str) -> Vec<Run> {
        fs::read_to_string(self.runs_dir().join(format!("{letter_id}.jsonl")))
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default()
    }
}

pub fn render(l: &Letter) -> String {
    format!(
        "---\nid: {}\ndispatch_id: {}\nfrom: {}\nfrom_name: {}\nto: {}\nto_name: {}\nurgency: {}\nwake: {}\nstatus: {}\ncreated: {}\n---\n{}\n",
        l.id,
        l.dispatch_id,
        l.from,
        l.from_name,
        l.to,
        l.to_name,
        l.urgency,
        l.wake,
        l.status,
        l.created,
        l.body.trim_end()
    )
}

pub fn parse(text: &str) -> Option<Letter> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let (fm, body) = (&rest[..end], &rest[end + 5..]);
    let mut l = Letter {
        id: String::new(),
        dispatch_id: String::new(),
        from: String::new(),
        from_name: String::new(),
        to: String::new(),
        to_name: String::new(),
        urgency: "普通".into(),
        wake: true,
        status: UNREAD.into(),
        created: String::new(),
        body: body.trim_end().to_string(),
        path: String::new(),
    };
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "id" => l.id = v,
            "dispatch_id" => l.dispatch_id = v,
            "from" => l.from = v,
            "from_name" => l.from_name = v,
            "to" => l.to = v,
            "to_name" => l.to_name = v,
            "urgency" => l.urgency = v,
            "wake" => l.wake = v != "false",
            "status" => l.status = v,
            "created" => l.created = v,
            _ => {}
        }
    }
    if l.to.is_empty() {
        return None;
    }
    if l.id.is_empty() {
        l.id = format!("legacy-{}", short_id(&l.created.replace([' ', ':', '-'], "")));
    }
    if l.from_name.is_empty() {
        l.from_name = l.from.clone();
    }
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let l = Letter {
            id: "a".into(),
            dispatch_id: "d".into(),
            from: "x".into(),
            from_name: "X".into(),
            to: "y".into(),
            to_name: "Y".into(),
            urgency: "普通".into(),
            wake: true,
            status: UNREAD.into(),
            created: "2026-11-05 14:00".into(),
            body: "hello\nworld".into(),
            path: String::new(),
        };
        let p = parse(&render(&l)).unwrap();
        assert_eq!(p.body, "hello\nworld");
        assert_eq!(p.to_name, "Y");
        assert!(p.wake);
    }

    #[test]
    fn store_writes_and_lists() {
        let dir = std::env::temp_dir().join(format!("dispatch-test-{}", uuid::Uuid::new_v4()));
        let s = Store::at(dir.clone());
        let mut l = Letter {
            id: uuid::Uuid::new_v4().to_string(),
            dispatch_id: "d1".into(),
            from: "me".into(),
            from_name: "me".into(),
            to: "11111111-2222-3333-4444-555555555555".into(),
            to_name: "T".into(),
            urgency: "普通".into(),
            wake: true,
            status: UNREAD.into(),
            created: now_text(),
            body: "hi".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 1);
        s.set_status(&mut l, READ_BY_HOOK).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 0);
        s.append_run(&l.id, "store", "ok", "").unwrap();
        assert_eq!(s.runs(&l.id).len(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}
