//! How a message reaches a session right now: the one thing to know before sending.
//!
//! - `Now`: it gets the message straight away (a running Claude Code session that takes it).
//! - `Later`: it sees the message the next time it speaks (the store plus the hook).
//! - `None`: nobody reads the message until the user fixes something.

use crate::claude::{self, Gate};
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
                    Reach { level: Level::Later, text: "没开着，下次打开它时会看到".into(), fix: None }
                } else {
                    Reach { level: Level::None, text: "收不到：没开着，又没装 hook".into(), fix: Some("hook".into()) }
                };
            }
            let Some(inbox) = claude::find_live(&s.id) else {
                return if hook_installed {
                    Reach { level: Level::Later, text: "开着，但它的 Claude Code 没开收信口，要等它下次说话".into(), fix: None }
                } else {
                    Reach { level: Level::None, text: "收不到：开着但没开收信口，又没装 hook".into(), fix: Some("hook".into()) }
                };
            };
            match claude::gate(&s.id, inbox.cwd.as_deref()) {
                Gate::Accept => Reach {
                    level: Level::Now,
                    text: if inbox.desktop { "立刻送到：Claude 客户端开着".into() } else { "立刻送到：终端里开着".into() },
                    fix: None,
                },
                Gate::Hold | Gate::Refuse => {
                    if hook_installed {
                        Reach { level: Level::Later, text: "它跳过了权限确认，要等它下次说话".into(), fix: None }
                    } else {
                        Reach { level: Level::None, text: "收不到：它跳过了权限确认，又没装 hook".into(), fix: Some("hook".into()) }
                    }
                }
            }
        }
        Harness::Codex => {
            let st = crate::codex::state();
            if !st.cli {
                Reach { level: Level::None, text: "收不到：本机没找到 codex 命令".into(), fix: Some("login".into()) }
            } else if !st.daemon {
                Reach { level: Level::None, text: "待接通：Codex 的 app-server daemon 没开".into(), fix: Some("daemon".into()) }
            } else if st.loaded.contains(&s.id) {
                Reach { level: Level::Now, text: "立刻送到：排在它当前这一轮之后".into(), fix: None }
            } else {
                Reach { level: Level::None, text: "待接通：会话没挂在 daemon 上（桌面里开着的会话不在 daemon 里）".into(), fix: Some("resume".into()) }
            }
        }
        Harness::Grok => Reach { level: Level::None, text: "Grok Bot 投递还没接上（M2）".into(), fix: None },
        Harness::Human => Reach { level: Level::None, text: "人不收消息".into(), fix: None },
    }
}
