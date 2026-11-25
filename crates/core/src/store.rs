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
pub const DELIVERED_GROK: &str = "已送达（Grok 直投）";
/// A reply to a person (who sent from the app or the command line): shown in the feed, nothing to hand over.
pub const DELIVERED_FEED: &str = "已送达（派发页）";
pub const READ_BY_HOOK: &str = "已读（hook代标）";
pub const REPLIED: &str = "已回复";
/// Nothing could deliver it, or its deadline passed. Still pending: a late hook pass or a retry delivers it.
pub const FAILED: &str = "没送到";
/// Withdrawn by the sender or the user; nothing delivers it any more.
pub const CANCELLED: &str = "已取消";
/// From another machine, held until a person here approves it (the machine is not confirmed
/// yet, or its rule says to ask). Not pending: no hook or sweep hands it over.
pub const HELD: &str = "待批准";
/// Sent by a session here, held on this machine until the user says send in the review window
/// (`review`). Not pending: no hook, sweep or peer hands it over before that.
pub const REVIEW: &str = "待你审核";

/// Still waiting to be delivered (the hook hands these over, the sweep retries them).
pub fn pending(status: &str) -> bool {
    status == UNREAD || status == FAILED
}

/// `30m`, `2h`, `1d`, or plain minutes.
pub fn parse_duration(s: &str) -> Option<chrono::Duration> {
    let s = s.trim().to_ascii_lowercase();
    let (num, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s.as_str(), "m"),
    };
    let n: i64 = num.parse().ok()?;
    match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" | "分" | "分钟" => Some(chrono::Duration::minutes(n)),
        "h" | "hr" | "hour" | "hours" | "小时" => Some(chrono::Duration::hours(n)),
        "d" | "day" | "days" | "天" => Some(chrono::Duration::days(n)),
        _ => None,
    }
}

/// How long a copy may wait before it counts as not delivered: an hour when urgent, a day otherwise.
pub fn default_wait(urgency: &str) -> chrono::Duration {
    if urgency_stored(urgency) == URGENCY_URGENT {
        chrono::Duration::hours(1)
    } else {
        chrono::Duration::hours(24)
    }
}

pub fn parse_time(s: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M").ok()
}

pub fn time_text(t: chrono::NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M").to_string()
}

/// The deadline text for a copy created at `created`, after `wait`.
pub fn deadline_text(created: &str, wait: chrono::Duration) -> String {
    let base = parse_time(created).unwrap_or_else(|| chrono::Local::now().naive_local());
    time_text(base + wait)
}
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
    /// `YYYY-MM-DD HH:MM`: past it, an undelivered copy counts as not delivered. Absent in old files.
    #[serde(default)]
    pub deadline: Option<String>,
    /// `claude` / `codex` / `grok`: what the recipient runs on, so a retry needs no discovery.
    #[serde(default)]
    pub to_harness: Option<String>,
    /// The machine the sender is on, when it is another one (its peer id and name).
    #[serde(default)]
    pub from_host: Option<String>,
    #[serde(default)]
    pub from_host_name: Option<String>,
    /// The machine the recipient is on, when it is another one.
    #[serde(default)]
    pub to_host: Option<String>,
    #[serde(default)]
    pub to_host_name: Option<String>,
    /// `claude` / `codex` / `grok` / `human`: what the sender runs on, when it was known at send time.
    #[serde(default)]
    pub from_harness: Option<String>,
    /// The sending machine's signature over the message, kept on letters that crossed over.
    #[serde(default)]
    pub signature: Option<String>,
    /// What the sending machine says about the user's review of this message (`Mark`). Absent
    /// on replies, on what the user wrote in the app, and on messages from older versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Mark>,
    pub body: String,
    #[serde(default)]
    pub path: String,
}

/// The user saw the whole text in the review window and pressed Send.
pub const MARK_FULL: &str = "full";
/// The user pressed Send on a long message that was folded: they saw its first lines.
pub const MARK_PART: &str = "part";
/// Not shown: the user let this sender write to this recipient without asking each time.
pub const MARK_STANDING: &str = "standing";
/// Not shown: the sending machine does not review before sending.
pub const MARK_NONE: &str = "none";

/// A fact about how a new message left its machine: whether the user was shown this text in
/// Dispatch's review window before it went. It says what the user saw, never that they ask
/// the recipient to do what the message says.
///
/// `digest` is the body the user pressed Send on: a mark whose digest is not the body's is
/// void. `signature` is the sending machine's signature over the mark (`peer::sign`), set
/// when the message crosses to another machine and checked there before it is stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mark {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl Mark {
    /// The user was shown the text (all of it, or its first lines) and pressed Send.
    pub fn shown(&self) -> bool {
        self.state == MARK_FULL || self.state == MARK_PART
    }

    /// Whether the mark still speaks for this body: a shown mark only for the very text the
    /// user pressed Send on.
    pub fn holds_for(&self, body: &str) -> bool {
        match self.state.as_str() {
            MARK_FULL | MARK_PART => self.digest.as_deref() == Some(body_digest(body).as_str()),
            MARK_STANDING | MARK_NONE => true,
            _ => false,
        }
    }

    /// One front-matter line: state, digest, signature, time (`-` for a part it has not).
    fn line(&self) -> String {
        let part = |v: &Option<String>| v.as_deref().filter(|s| !s.is_empty()).unwrap_or("-").to_string();
        format!("{} {} {} {}", self.state, part(&self.digest), part(&self.signature), part(&self.at))
    }

    fn from_line(v: &str) -> Option<Mark> {
        let mut it = v.splitn(4, ' ');
        let state = it.next().filter(|s| !s.is_empty())?.to_string();
        let mut part = || it.next().map(str::trim).filter(|s| !s.is_empty() && *s != "-").map(str::to_string);
        let (digest, signature, at) = (part(), part(), part());
        Some(Mark { state, at, digest, signature })
    }
}

/// The digest a review mark carries: SHA-256 of the body as the store keeps it.
pub fn body_digest(body: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(body.trim().as_bytes()))
}

impl Letter {
    /// The deadline: the stored one, else the default wait after creation.
    pub fn deadline_at(&self) -> chrono::NaiveDateTime {
        self.deadline.as_deref().and_then(parse_time).unwrap_or_else(|| parse_time(&self.created).unwrap_or_else(|| chrono::Local::now().naive_local()) + default_wait(&self.urgency))
    }

    /// Still undelivered and past its deadline.
    pub fn overdue(&self, now: chrono::NaiveDateTime) -> bool {
        self.status == UNREAD && now > self.deadline_at()
    }
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

thread_local! {
    /// A store folder for this thread alone, ahead of `DISPATCH_HOME`: how a test runs two
    /// machines in one process.
    static THREAD_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Make `Store::open()` on this thread use `root` (None: back to the environment).
pub fn set_thread_root(root: Option<PathBuf>) {
    THREAD_ROOT.with(|r| *r.borrow_mut() = root);
}

impl Store {
    /// This thread's root when one is set, else `DISPATCH_HOME`, else the platform data folder
    /// (`~/Library/Application Support/Dispatch` on macOS).
    pub fn open() -> Store {
        if let Some(r) = THREAD_ROOT.with(|r| r.borrow().clone()) {
            return Store { root: r };
        }
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
                if unread_only && !pending(&l.status) {
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
        self.set_field(&l.path, "status", status)?;
        l.status = status.to_string();
        Ok(())
    }

    /// Rewrite the `deadline:` line (adding it when the file has none).
    pub fn set_deadline(&self, l: &mut Letter, deadline: &str) -> Result<()> {
        self.set_field(&l.path, "deadline", deadline)?;
        l.deadline = Some(deadline.to_string());
        Ok(())
    }

    /// Write the review mark into the file (adding the line when it has none).
    pub fn set_mark(&self, l: &mut Letter, mark: Mark) -> Result<()> {
        self.set_field(&l.path, "review", &mark.line())?;
        l.review = Some(mark);
        Ok(())
    }

    /// Rewrite one front-matter line in place, or add it before the closing `---`.
    fn set_field(&self, path: &str, key: &str, value: &str) -> Result<()> {
        let text = fs::read_to_string(path).with_context(|| format!("read {path}"))?;
        let mut out: Vec<String> = vec![];
        let mut done = false;
        let mut in_front = false;
        for (i, line) in text.lines().enumerate() {
            if i == 0 && line == "---" {
                in_front = true;
                out.push(line.to_string());
                continue;
            }
            if in_front && line == "---" {
                if !done {
                    out.push(format!("{key}: {value}"));
                    done = true;
                }
                in_front = false;
            } else if in_front && !done && line.starts_with(&format!("{key}:")) {
                out.push(format!("{key}: {value}"));
                done = true;
                continue;
            }
            out.push(line.to_string());
        }
        if !done {
            return Err(anyhow!("no front matter in {path}"));
        }
        let tmp = format!("{path}.tmp");
        fs::write(&tmp, out.join("\n") + "\n")?;
        fs::rename(&tmp, path)?;
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
    let mut extra = String::new();
    if let Some(h) = &l.to_harness {
        extra.push_str(&format!("to_harness: {h}\n"));
    }
    if let Some(d) = &l.deadline {
        extra.push_str(&format!("deadline: {d}\n"));
    }
    for (k, v) in [("from_harness", &l.from_harness), ("from_host", &l.from_host), ("from_host_name", &l.from_host_name), ("to_host", &l.to_host), ("to_host_name", &l.to_host_name), ("signature", &l.signature)] {
        if let Some(v) = v {
            extra.push_str(&format!("{k}: {v}\n"));
        }
    }
    if let Some(m) = &l.review {
        extra.push_str(&format!("review: {}\n", m.line()));
    }
    format!(
        "---\nid: {}\ndispatch_id: {}\nfrom: {}\nfrom_name: {}\nto: {}\nto_name: {}\nurgency: {}\nwake: {}\nstatus: {}\ncreated: {}\n{}---\n{}\n",
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
        extra,
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
        deadline: None,
        to_harness: None,
        from_host: None,
        from_host_name: None,
        to_host: None,
        to_host_name: None,
        from_harness: None,
        signature: None,
        review: None,
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
            "deadline" => l.deadline = Some(v).filter(|s| !s.is_empty()),
            "to_harness" => l.to_harness = Some(v).filter(|s| !s.is_empty()),
            "from_host" => l.from_host = Some(v).filter(|s| !s.is_empty()),
            "from_host_name" => l.from_host_name = Some(v).filter(|s| !s.is_empty()),
            "to_host" => l.to_host = Some(v).filter(|s| !s.is_empty()),
            "to_host_name" => l.to_host_name = Some(v).filter(|s| !s.is_empty()),
            "from_harness" => l.from_harness = Some(v).filter(|s| !s.is_empty()),
            "signature" => l.signature = Some(v).filter(|s| !s.is_empty()),
            "review" => l.review = Mark::from_line(&v),
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
            deadline: Some("2026-11-06 14:00".into()),
            to_harness: Some("codex".into()),
            from_host: None,
            from_host_name: None,
            to_host: None,
            to_host_name: None,
            from_harness: None,
            signature: None,
            review: None,
            body: "hello\nworld".into(),
            path: String::new(),
        };
        let p = parse(&render(&l)).unwrap();
        assert_eq!(p.body, "hello\nworld");
        assert_eq!(p.to_name, "Y");
        assert!(p.wake);
        assert_eq!(p.deadline.as_deref(), Some("2026-11-06 14:00"));
        assert_eq!(p.to_harness.as_deref(), Some("codex"));
        assert_eq!(time_text(p.deadline_at()), "2026-11-06 14:00");
        let mut away = l.clone();
        away.from_harness = Some("grok".into());
        away.to_host = Some("m-1".into());
        away.to_host_name = Some("主力".into());
        let q = parse(&render(&away)).unwrap();
        assert_eq!(q.to_host.as_deref(), Some("m-1"));
        assert_eq!(q.to_host_name.as_deref(), Some("主力"));
        assert_eq!(q.from_harness.as_deref(), Some("grok"));
        assert!(q.from_host.is_none());
        assert!(p.overdue(parse_time("2026-11-06 14:01").unwrap()));
        assert!(!p.overdue(parse_time("2026-11-06 13:59").unwrap()));
        assert!(p.review.is_none() && !render(&l).contains("review:"), "no mark, no line");
    }

    /// A review mark goes through the file whole, and speaks only for the text it was made for.
    #[test]
    fn a_review_mark_round_trips_and_is_bound_to_the_body() {
        let mut l = parse("---\nid: a\ndispatch_id: d\nfrom: x\nto: y\nurgency: 普通\nstatus: 未读\ncreated: 2026-11-05 14:00\n---\nplease check the build\n").unwrap();
        let seen = Mark { state: MARK_FULL.into(), at: Some("2026-11-24 15:12".into()), digest: Some(body_digest(&l.body)), signature: Some("abcd".into()) };
        l.review = Some(seen.clone());
        let back = parse(&render(&l)).unwrap();
        assert_eq!(back.review, Some(seen.clone()), "state, time, digest and signature all come back");
        assert!(seen.shown() && seen.holds_for(&back.body));
        assert!(seen.holds_for("please check the build\n\n"), "trailing whitespace is not part of the text");
        assert!(!seen.holds_for("please check the build and then delete it"), "another text: the mark is void");
        let none = Mark { state: MARK_STANDING.into(), at: None, digest: None, signature: None };
        l.review = Some(none.clone());
        let back = parse(&render(&l)).unwrap();
        assert_eq!(back.review, Some(none.clone()));
        assert!(!none.shown() && none.holds_for("anything"), "saying it was not shown needs no text");
        let odd = Mark { state: "approved-by-user".into(), at: None, digest: None, signature: None };
        assert!(!odd.holds_for(&l.body), "a state this version does not know says nothing");
        // a mark with no digest never counts as shown text
        assert!(!Mark { state: MARK_FULL.into(), at: None, digest: None, signature: None }.holds_for(&l.body));
    }

    #[test]
    fn deadlines_default_by_urgency() {
        assert_eq!(deadline_text("2026-11-05 14:00", default_wait("紧急")), "2026-11-05 15:00");
        assert_eq!(deadline_text("2026-11-05 14:00", default_wait("普通")), "2026-11-06 14:00");
        assert_eq!(parse_duration("30m"), Some(chrono::Duration::minutes(30)));
        assert_eq!(parse_duration("2h"), Some(chrono::Duration::hours(2)));
        assert_eq!(parse_duration("1d"), Some(chrono::Duration::days(1)));
        assert_eq!(parse_duration("45"), Some(chrono::Duration::minutes(45)));
        assert_eq!(parse_duration("soon"), None);
        let old = parse("---\nid: a\ndispatch_id: d\nfrom: x\nto: y\nurgency: 普通\nstatus: 未读\ncreated: 2026-11-05 14:00\n---\nhi\n").unwrap();
        assert_eq!(time_text(old.deadline_at()), "2026-11-06 14:00");
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
            deadline: None,
            to_harness: None,
            from_host: None,
            from_host_name: None,
            to_host: None,
            to_host_name: None,
            from_harness: None,
            signature: None,
            review: None,
            body: "hi".into(),
            path: String::new(),
        };
        s.write_letter(&mut l).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 1);
        s.set_deadline(&mut l, "2026-12-01 09:00").unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap()[0].deadline.as_deref(), Some("2026-12-01 09:00"));
        s.set_status(&mut l, FAILED).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 1, "failed copies are still pending");
        s.set_status(&mut l, CANCELLED).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 0);
        s.set_status(&mut l, READ_BY_HOOK).unwrap();
        assert_eq!(s.list(Some(&l.to), true).unwrap().len(), 0);
        s.append_run(&l.id, "store", "ok", "").unwrap();
        assert_eq!(s.runs(&l.id).len(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}
