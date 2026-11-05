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
}

pub fn decide(s: &Session, hook_installed: bool) -> Reach {
    match s.harness {
        Harness::Claude => {
            if !s.running {
                return if hook_installed {
                    Reach { level: Level::Later, text: t("reach.off_later").into(), fix: None }
                } else {
                    Reach { level: Level::None, text: t("reach.off_none").into(), fix: Some("hook".into()) }
                };
            }
            let Some(inbox) = claude::find_live(&s.id) else {
                return if hook_installed {
                    Reach { level: Level::Later, text: t("reach.noinbox_later").into(), fix: None }
                } else {
                    Reach { level: Level::None, text: t("reach.noinbox_none").into(), fix: Some("hook".into()) }
                };
            };
            match claude::gate(&s.id, inbox.cwd.as_deref()) {
                Gate::Accept => Reach {
                    level: Level::Now,
                    text: if inbox.desktop { t("reach.now_desktop").into() } else { t("reach.now_terminal").into() },
                    fix: None,
                },
                Gate::Hold | Gate::Refuse => {
                    if hook_installed {
                        Reach { level: Level::Later, text: t("reach.bypass_later").into(), fix: None }
                    } else {
                        Reach { level: Level::None, text: t("reach.bypass_none").into(), fix: Some("hook".into()) }
                    }
                }
            }
        }
        Harness::Codex => {
            let st = crate::codex::state();
            if !st.cli {
                Reach { level: Level::None, text: t("reach.codex_nocli").into(), fix: Some("login".into()) }
            } else if !st.daemon {
                Reach { level: Level::None, text: t("reach.codex_nodaemon").into(), fix: Some("daemon".into()) }
            } else if st.loaded.contains(&s.id) {
                Reach { level: Level::Now, text: t("reach.codex_now").into(), fix: None }
            } else {
                Reach { level: Level::None, text: t("reach.codex_notloaded").into(), fix: Some("resume".into()) }
            }
        }
        Harness::Grok => {
            let st = crate::grok::state();
            if !st.app {
                Reach { level: Level::None, text: t("reach.grok_noapp").into(), fix: Some("install".into()) }
            } else if !st.signed_in {
                Reach { level: Level::None, text: t("reach.grok_nologin").into(), fix: Some("login".into()) }
            } else if st.cli.is_none() {
                Reach { level: Level::None, text: t("reach.grok_nocli").into(), fix: Some("gbot".into()) }
            } else if let Some(e) = &st.error {
                Reach { level: Level::None, text: t("reach.grok_offline").replace("{e}", e), fix: Some("online".into()) }
            } else {
                Reach { level: Level::Now, text: t("reach.grok_now").into(), fix: None }
            }
        }
        Harness::Human => Reach { level: Level::None, text: t("reach.human").into(), fix: None },
    }
}
