use serde::{Deserialize, Serialize};

/// The tool a session runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Grok,
    Human,
}

impl Harness {
    pub fn label(self) -> &'static str {
        match self {
            Harness::Claude => "Claude",
            Harness::Codex => "Codex",
            Harness::Grok => "Grok Bot",
            Harness::Human => "人",
        }
    }
}

/// A session Dispatch knows about: live (a process is running it) or recent (a transcript).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub harness: Harness,
    pub id: String,
    pub short_id: String,
    /// The name the user gave it in the Claude app (`name` in the live record).
    pub name: Option<String>,
    pub cwd: Option<String>,
    /// The first user message, trimmed: a cheap "what is this session about".
    pub title: Option<String>,
    /// Unix milliseconds.
    pub last_active: i64,
    pub running: bool,
    /// Started by the Claude app (Code tab) rather than a terminal.
    pub desktop: bool,
    /// The Claude app's own id for the chat (`local_…`), for `claude://code/continue?session=`.
    pub host_session_id: Option<String>,
    /// `idle` / `running` / … as the live record says.
    pub status: Option<String>,
    /// The transcript file, when one was found.
    pub transcript: Option<String>,
}

impl Session {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| self.short_id.clone())
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The home folder; `DISPATCH_TEST_HOME` replaces it in tests.
pub fn home_dir() -> std::path::PathBuf {
    if let Ok(h) = std::env::var("DISPATCH_TEST_HOME") {
        return h.into();
    }
    dirs::home_dir().unwrap_or_else(|| "/".into())
}

/// The `dispatch` command as another session can run it: this binary's absolute path, quoted.
pub fn self_cmd() -> String {
    std::env::current_exe().map(|p| format!("\"{}\"", p.display())).unwrap_or_else(|_| "dispatch".into())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `~/vaults/…/项目/评论插件`: where it starts and its last two folders, the middle elided.
pub fn short_cwd(cwd: &str) -> String {
    let home = home_dir().to_string_lossy().to_string();
    let p = if cwd.starts_with(&home) { format!("~{}", &cwd[home.len()..]) } else { cwd.to_string() };
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    let head_n = if p.starts_with('~') { 2 } else { 1 };
    if parts.len() <= head_n + 3 {
        return p;
    }
    let lead = if p.starts_with('/') { "/" } else { "" };
    format!("{}{}/…/{}", lead, parts[..head_n].join("/"), parts[parts.len() - 2..].join("/"))
}
