//! How a message reaches a session right now: the one thing to know before sending.
//!
//! - `Now`: it gets the message straight away (a running Claude Code session that takes it).
//! - `Later`: it sees the message the next time it speaks (the store plus the hook).
//! - `None`: nobody reads the message until the user fixes something.

use crate::claude::{self, Gate};
use crate::i18n::t;
use crate::model::{Harness, Session};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Now,
    Later,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reach {
    pub level: Level,
    /// Short, in the user's words.
    pub text: String,
    /// One thing that would turn `None` into something better.
    pub fix: Option<String>,
    /// How it runs, as its own app records it: Claude Code's permission mode (`bypassPermissions`,
    /// `auto`, `default`, `acceptEdits`, `plan`) or Codex's approval policy (`never`,
    /// `on-request`, `untrusted`, `on-failure`). Only for a running session; read where it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Which of its app's front ends has it open: `desktop`, `terminal`, `exec` (Codex run
    /// without a person), or what Codex calls its originator. Only for a running session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

pub fn decide(s: &Session, hook_installed: bool) -> Reach {
    if s.host.is_some() {
        // the machine it runs on worked all of this out, mode and client included
        return crate::peer::remote_reach(s);
    }
    let mut r = decide_here(s, hook_installed);
    if s.running {
        match s.harness {
            Harness::Claude => {
                r.mode = claude::mode(&s.id, s.cwd.as_deref());
                r.client = Some(if s.desktop { "desktop" } else { "terminal" }.into());
            }
            Harness::Codex => {
                if let Some(t) = s.transcript.as_deref().map(|p| crate::codex::traits(std::path::Path::new(p))) {
                    r.mode = t.approval;
                    r.client = t.client;
                }
            }
            _ => {}
        }
    }
    r
}

fn decide_here(s: &Session, hook_installed: bool) -> Reach {
    match s.harness {
        Harness::Claude => {
            if !s.running {
                return if hook_installed {
                    Reach { level: Level::Later, text: t("reach.off_later").into(), fix: None, mode: None, client: None }
                } else {
                    Reach { level: Level::None, text: t("reach.off_none").into(), fix: Some("hook".into()), mode: None, client: None }
                };
            }
            let Some(inbox) = claude::find_live(&s.id) else {
                return if hook_installed {
                    Reach { level: Level::Later, text: t("reach.noinbox_later").into(), fix: None, mode: None, client: None }
                } else {
                    Reach { level: Level::None, text: t("reach.noinbox_none").into(), fix: Some("hook".into()), mode: None, client: None }
                };
            };
            if !claude::inbox_supported() {
                return if hook_installed {
                    Reach { level: Level::Later, text: t("reach.nosocket_later").into(), fix: None, mode: None, client: None }
                } else {
                    Reach { level: Level::None, text: t("reach.nosocket_none").into(), fix: Some("hook".into()), mode: None, client: None }
                };
            }
            let (gate, why) = claude::gate_reason(&s.id, inbox.cwd.as_deref());
            match gate {
                Gate::Accept => Reach {
                    level: Level::Now,
                    text: if inbox.desktop { t("reach.now_desktop").into() } else { t("reach.now_terminal").into() },
                    fix: None,
                    mode: None,
                    client: None,
                },
                gate => {
                    if hook_installed {
                        let busy = matches!(s.status.as_deref(), Some("busy") | Some("running"));
                        let key = if gate == Gate::Refuse {
                            "reach.refuse_later"
                        } else if why == "mode unknown" {
                            "reach.unknown_later"
                        } else if busy {
                            "reach.bypass_busy"
                        } else {
                            "reach.bypass_idle"
                        };
                        Reach { level: Level::Later, text: t(key).into(), fix: Some("inbound".into()), mode: None, client: None }
                    } else {
                        Reach { level: Level::None, text: t("reach.bypass_none").into(), fix: Some("hook".into()), mode: None, client: None }
                    }
                }
            }
        }
        Harness::Codex => {
            let st = crate::codex::state();
            if !st.cli {
                Reach { level: Level::None, text: t("reach.codex_nocli").into(), fix: Some("login".into()), mode: None, client: None }
            } else if st.daemon && st.loaded.contains(&s.id) {
                Reach { level: Level::Now, text: t("reach.codex_now").into(), fix: None, mode: None, client: None }
            } else {
                Reach { level: Level::Later, text: t("reach.codex_queue").into(), fix: None, mode: None, client: None }
            }
        }
        Harness::Grok => {
            let st = crate::grok::state();
            if !st.app {
                Reach { level: Level::None, text: t("reach.grok_noapp").into(), fix: Some("install".into()), mode: None, client: None }
            } else if !st.signed_in {
                Reach { level: Level::None, text: t("reach.grok_nologin").into(), fix: Some("login".into()), mode: None, client: None }
            } else if st.cli.is_none() {
                Reach { level: Level::None, text: t("reach.grok_nocli").into(), fix: Some("gbot".into()), mode: None, client: None }
            } else if let Some(e) = &st.error {
                Reach { level: Level::None, text: t("reach.grok_offline").replace("{e}", e), fix: Some("online".into()), mode: None, client: None }
            } else {
                Reach { level: Level::Now, text: t("reach.grok_now").into(), fix: None, mode: None, client: None }
            }
        }
        Harness::Human => Reach { level: Level::Now, text: t("reach.human").into(), fix: None, mode: None, client: None },
    }
}
