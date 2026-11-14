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

/// Every reach text, so one written on another machine (in that machine's language) can be
/// read here in this one's.
const KEYS: &[&str] = &[
    "reach.off_later", "reach.off_none", "reach.noinbox_later", "reach.noinbox_none", "reach.now_desktop", "reach.now_terminal",
    "reach.bypass_busy", "reach.bypass_idle", "reach.unknown_later", "reach.refuse_later", "reach.bypass_none", "reach.codex_nocli",
    "reach.codex_nodaemon", "reach.codex_queue", "reach.codex_now", "reach.codex_notloaded", "reach.grok_noapp", "reach.grok_nologin",
    "reach.grok_nocli", "reach.grok_now", "reach.nosocket_later", "reach.nosocket_none", "reach.human", "reach.peer_offline", "reach.checking",
];

/// A reach another machine worked out, its text in this machine's language when it is one of
/// ours (a text with a detail filled in, like the Grok gateway's error, stays as it came).
pub fn localized(mut r: Reach) -> Reach {
    if let Some(key) = KEYS.iter().find(|k| {
        let (zh, en) = crate::i18n::pair(k);
        r.text == zh || r.text == en
    }) {
        r.text = t(key).into();
    }
    r
}

/// What sending to a session does, in a person's words: `kind` is `now` (it gets it at once),
/// `auto` (Dispatch opens it), `you` (the person has to open it), `turn` (when its turn ends),
/// `speak` (at its next turn), `machine` (its machine is offline) or `off` (it cannot get it);
/// `title` says that in a few words, `detail` in a sentence, `todo` what the person can do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plain {
    pub kind: String,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
}

/// The reach text's key, in whichever language it was written (here or on another machine).
fn key_of(r: &Reach) -> Option<&'static str> {
    KEYS.iter().copied().find(|k| {
        let (zh, en) = crate::i18n::pair(k);
        r.text == zh || r.text == en
    })
}

fn plain_of(kind: &str, title: &str, detail: String, todo: Option<String>) -> Plain {
    Plain { kind: kind.into(), title: t(title).into(), detail, todo }
}

/// What sending does now, said plainly. Whether Dispatch opens a closed session itself follows
/// the wake setting of the machine the session is on; this machine's setting stands in for it.
pub fn plain(s: &Session, r: &Reach) -> Plain {
    let auto = crate::wake::mode() == "auto";
    let key = key_of(r);
    match key {
        Some("reach.now_desktop" | "reach.now_terminal" | "reach.codex_now" | "reach.grok_now") => plain_of("now", "plain.now_title", t("plain.now").into(), None),
        Some("reach.human") => plain_of("now", "plain.now_title", r.text.clone(), None),
        Some("reach.codex_queue" | "reach.codex_nodaemon" | "reach.codex_notloaded") => {
            if auto {
                plain_of("auto", "plain.auto_title", t("plain.codex").into(), None)
            } else {
                plain_of("you", "plain.you_title", t("plain.codex_you").into(), None)
            }
        }
        Some("reach.off_later") => {
            // a terminal session Dispatch cannot start again: its folder is gone, or there is no `claude` here
            let gone = s.host.is_none() && !s.desktop && (s.cwd.as_deref().map(|c| !std::path::Path::new(c).is_dir()).unwrap_or(false) || crate::claude::executable().is_none());
            if auto && !gone {
                plain_of("auto", "plain.auto_title", t(if s.desktop { "plain.claude_desktop" } else { "plain.claude_terminal" }).into(), None)
            } else {
                plain_of("you", "plain.you_title", t("plain.claude_you").into(), None)
            }
        }
        Some("reach.bypass_busy") => plain_of("turn", "plain.turn_title", t("plain.turn").into(), None),
        Some("reach.bypass_idle" | "reach.unknown_later") => plain_of("speak", "plain.speak_title", t("plain.speak").into(), Some(t("plain.speak_todo").into())),
        Some("reach.refuse_later") => plain_of("speak", "plain.speak_title", t("plain.speak").into(), Some(t("plain.refuse_todo").into())),
        Some("reach.noinbox_later" | "reach.nosocket_later") => plain_of("speak", "plain.speak_title", t("plain.speak").into(), None),
        Some("reach.peer_offline") => plain_of("machine", "plain.machine_title", t("plain.machine").into(), None),
        _ => match r.level {
            Level::Now => plain_of("now", "plain.now_title", t("plain.now").into(), None),
            Level::Later => plain_of("speak", "plain.speak_title", r.text.clone(), None),
            Level::None => plain_of("off", "plain.off_title", r.text.clone(), r.fix.as_deref().map(|f| t(match f { "hook" => "plain.fix_hook", "inbound" => "plain.speak_todo", _ => "plain.fix_setup" }).to_string())),
        },
    }
}

/// What the person reads once the message went (`route` as the send report gives it).
pub fn after_send(p: &Plain, route: &str, detail: &str) -> String {
    if route == "failed" {
        return t("plain.sent_failed").replace("{why}", detail);
    }
    let key = match p.kind.as_str() {
        "now" => "plain.sent_now",
        "auto" if p.detail == t("plain.codex") => "plain.sent_codex",
        "auto" => "plain.sent_auto",
        "you" => "plain.sent_you",
        "turn" => "plain.sent_turn",
        "speak" => "plain.sent_speak",
        "machine" => "plain.sent_machine",
        _ => "plain.sent_off",
    };
    t(key).replace("{why}", &p.detail)
}

pub fn decide(s: &Session, hook_installed: bool) -> Reach {
    decide_with(s, hook_installed, None)
}

pub(crate) fn decide_with(s: &Session, hook_installed: bool, clients: Option<&crate::discover::ClientStates>) -> Reach {
    if s.host.is_some() {
        // the machine it runs on worked all of this out, mode and client included
        return crate::peer::remote_reach(s);
    }
    let mut r = decide_here(s, hook_installed, clients);
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

fn checking() -> Reach {
    Reach { level: Level::Later, text: t("reach.checking").into(), fix: None, mode: None, client: None }
}

fn decide_here(s: &Session, hook_installed: bool, clients: Option<&crate::discover::ClientStates>) -> Reach {
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
            let Some(st) = clients.map(|c| c.codex.clone()).unwrap_or_else(|| Some(crate::codex::state())) else { return checking() };
            if !st.cli {
                Reach { level: Level::None, text: t("reach.codex_nocli").into(), fix: Some("login".into()), mode: None, client: None }
            } else if st.daemon && st.loaded.contains(&s.id) {
                Reach { level: Level::Now, text: t("reach.codex_now").into(), fix: None, mode: None, client: None }
            } else {
                Reach { level: Level::Later, text: t("reach.codex_queue").into(), fix: None, mode: None, client: None }
            }
        }
        Harness::Grok => {
            let Some(st) = clients.map(|c| c.grok.clone()).unwrap_or_else(|| Some(crate::grok::state())) else { return checking() };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reach_from_another_machine_reads_in_this_ones_language() {
        let (zh, en) = crate::i18n::pair("reach.now_desktop");
        for text in [zh, en] {
            let r = localized(Reach { level: Level::Now, text: text.into(), fix: None, mode: None, client: None });
            assert_eq!(r.text, t("reach.now_desktop"));
        }
        let odd = localized(Reach { level: Level::None, text: "something else".into(), fix: None, mode: None, client: None });
        assert_eq!(odd.text, "something else");
    }
}
