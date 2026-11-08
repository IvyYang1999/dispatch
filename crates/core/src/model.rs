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
            Harness::Human => crate::i18n::t("harness.human"),
        }
    }
}

/// Another machine running Dispatch, as the home of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
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
    /// The machine it is on when that is not this one (a paired peer).
    #[serde(default)]
    pub host: Option<Host>,
}

impl Session {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| self.short_id.clone())
    }

    /// `主力/release`: the display name, with its machine in front when it is elsewhere.
    pub fn address(&self) -> String {
        match &self.host {
            Some(h) => format!("{}/{}", h.name, self.display_name()),
            None => self.display_name(),
        }
    }

    pub fn short_address(&self) -> String {
        match &self.host {
            Some(h) => format!("{}/{}", h.name, self.short_id),
            None => self.short_id.clone(),
        }
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

/// The `dispatch` command as another session can run it, quoted: the installed command line
/// when there is one, else this binary when it is the command line, else the command line
/// beside this binary (the app ships it as a sidecar), else plain `dispatch`. The app itself
/// delivers messages too, and its own binary must never be the answer: it is not the CLI.
pub fn self_cmd() -> String {
    let installed = home_dir().join(".dispatch").join("bin").join(if cfg!(windows) { "dispatch.exe" } else { "dispatch" });
    cli_command(std::env::current_exe().ok().as_deref(), &installed)
}

fn cli_command(exe: Option<&std::path::Path>, installed: &std::path::Path) -> String {
    let quote = |p: &std::path::Path| format!("\"{}\"", p.display());
    if installed.is_file() {
        return quote(installed);
    }
    if let Some(exe) = exe {
        let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name == "dispatch" || name == "dispatch.exe" {
            return quote(exe);
        }
        if let Some(dir) = exe.parent() {
            for n in ["dispatch", "dispatch.exe", "dispatch-aarch64-apple-darwin", "dispatch-x86_64-apple-darwin", "dispatch-x86_64-unknown-linux-gnu", "dispatch-aarch64-unknown-linux-gnu", "dispatch-x86_64-pc-windows-msvc.exe"] {
                let p = dir.join(n);
                if p.is_file() {
                    return quote(&p);
                }
            }
        }
    }
    "dispatch".into()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_command_is_always_the_command_line_never_the_app() {
        let dir = std::env::temp_dir().join(format!("dispatch-selfcmd-{}", uuid::Uuid::new_v4()));
        let app_bin = dir.join("Dispatch.app/Contents/MacOS");
        std::fs::create_dir_all(&app_bin).unwrap();
        let desktop = app_bin.join("dispatch-desktop");
        std::fs::write(&desktop, b"").unwrap();
        let missing = dir.join("none/dispatch");
        // the app's own binary, no CLI anywhere: the plain command, not the app
        assert_eq!(cli_command(Some(&desktop), &missing), "dispatch");
        // the sidecar beside the app wins over nothing
        let sidecar = app_bin.join("dispatch");
        std::fs::write(&sidecar, b"").unwrap();
        assert_eq!(cli_command(Some(&desktop), &missing), format!("\"{}\"", sidecar.display()));
        // the command line itself, when that is what runs
        let cli = dir.join("target/release/dispatch");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, b"").unwrap();
        assert_eq!(cli_command(Some(&cli), &missing), format!("\"{}\"", cli.display()));
        // the installed command line beats both, with its spaces quoted
        let installed = dir.join("home dir/.dispatch/bin/dispatch");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, b"").unwrap();
        let cmd = cli_command(Some(&desktop), &installed);
        assert_eq!(cmd, format!("\"{}\"", installed.display()));
        assert!(!cmd.contains("dispatch-desktop"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
