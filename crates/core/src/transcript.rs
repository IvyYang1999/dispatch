//! The last turns of a session's own record, for the session page: what the person and the
//! agent said, with the messages Dispatch handed over marked. Claude Code transcripts and
//! Codex rollouts are read from their tails; nothing is written.

use crate::model::{Harness, Session};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
    /// RFC 3339, when the record has it.
    pub at: Option<String>,
    /// Handed over by Dispatch (an envelope, or the hook's digest), from another agent.
    pub via_dispatch: bool,
    pub from_agent: Option<String>,
}

/// One exchange: what the person (or Dispatch, for the person) said, and everything the agent
/// said back before the next prompt. Tool calls are not part of a transcript turn here, so the
/// replies are the agent's words only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub user: Option<Turn>,
    pub replies: Vec<Turn>,
}

/// Group turns into rounds: a user turn opens a round, assistant turns join the open one
/// (a reply with no prompt before it opens one of its own). Newest `limit` rounds.
pub fn rounds(turns: &[Turn], limit: usize) -> Vec<Round> {
    let mut out: Vec<Round> = vec![];
    for t in turns {
        // images attached to a prompt arrive as a message of their own: they belong to it
        if t.role == "user" && IMAGES_RE.is_match(&t.text) {
            if let Some(Round { user: Some(u), replies }) = out.last_mut() {
                if replies.is_empty() {
                    u.text = format!("{}\n{}", u.text, t.text.trim());
                    continue;
                }
            }
        }
        if t.role == "user" {
            out.push(Round { user: Some(t.clone()), replies: vec![] });
        } else {
            match out.last_mut() {
                Some(r) => r.replies.push(t.clone()),
                None => out.push(Round { user: None, replies: vec![t.clone()] }),
            }
        }
    }
    if out.len() > limit {
        out.drain(..out.len() - limit);
    }
    out
}

const TAIL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CHARS: usize = 600;
/// Harness boilerplate that says nothing about the conversation.
const TAGS: [&str; 11] = ["system-reminder", "environment_context", "user_instructions", "permissions_instructions", "local-command-stdout", "local-command-caveat", "command-name", "command-message", "command-args", "skills_instructions", "INSTRUCTIONS"];

static TAG_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| TAGS.iter().map(|t| Regex::new(&format!(r"(?s)<{t}[^>]*>.*?</{t}>")).unwrap()).collect());
static PASTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?pasted_content[^>]*>").unwrap());
static IMAGES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*\[Image: source: [^\]]+\]\s*)+$").unwrap());
static FROM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:📨 来自 |📨 From |（来自 |\(from )([^（(，,]+?)(?:（|\s\(|，|,)").unwrap());

/// The last `limit` turns of the session's record, oldest first.
pub fn recent(s: &Session, limit: usize) -> Vec<Turn> {
    let Some(path) = s.transcript.as_deref() else { return vec![] };
    let Ok(tail) = crate::claude::read_tail(std::path::Path::new(path), TAIL_BYTES) else { return vec![] };
    parse(&tail, s.harness, limit)
}

pub fn parse(text: &str, harness: Harness, limit: usize) -> Vec<Turn> {
    let mut turns: Vec<Turn> = vec![];
    for v in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let Some(t) = (match harness {
            Harness::Codex => codex_turn(&v),
            _ => claude_turn(&v),
        }) else {
            continue;
        };
        // a rollout can carry the same message as an event and as an item
        if turns.last().map(|p| p.role == t.role && p.text == t.text).unwrap_or(false) {
            continue;
        }
        turns.push(t);
    }
    if turns.len() > limit {
        turns.drain(..turns.len() - limit);
    }
    turns
}

fn claude_turn(v: &Value) -> Option<Turn> {
    let role = v["type"].as_str()?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    finish(role, &text, v["timestamp"].as_str())
}

fn codex_turn(v: &Value) -> Option<Turn> {
    let p = &v["payload"];
    let (role, text) = if v["type"] == "response_item" && p["type"] == "message" {
        let role = p["role"].as_str()?;
        if role != "user" && role != "assistant" {
            return None;
        }
        (role, p["content"].as_array()?.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
    } else if v["type"] == "event_msg" && (p["type"] == "user_message" || p["type"] == "agent_message") {
        (if p["type"] == "user_message" { "user" } else { "assistant" }, p["message"].as_str()?.to_string())
    } else {
        return None;
    };
    finish(role, &text, v["timestamp"].as_str())
}

fn finish(role: &str, raw: &str, at: Option<&str>) -> Option<Turn> {
    let text = clean(raw);
    if text.is_empty() {
        return None;
    }
    let via_dispatch = text.starts_with("📨");
    let from_agent = if via_dispatch { FROM_RE.captures(&text).map(|c| c[1].trim().to_string()) } else { None };
    Some(Turn { role: role.into(), text: clip(&text), at: at.map(str::to_string), via_dispatch, from_agent })
}

/// Reminder and context blocks out; a message that was nothing but a tag block is dropped.
fn clean(raw: &str) -> String {
    let mut t = raw.to_string();
    for re in TAG_RES.iter() {
        t = re.replace_all(&t, "").to_string();
    }
    // a paste's markers go, its text stays
    t = PASTE_RE.replace_all(&t, "").to_string();
    let t = t.trim();
    if t.starts_with('<') && t.ends_with('>') && !t.contains("\n\n") {
        return String::new();
    }
    // Codex hands the project's AGENTS.md to the model as a user message
    if t.starts_with("# AGENTS.md instructions") {
        return String::new();
    }
    t.to_string()
}

fn clip(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= MAX_CHARS {
        t.to_string()
    } else {
        format!("{}…", chars[..MAX_CHARS].iter().collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_turns_keep_text_and_mark_dispatch_envelopes() {
        let lines = [
            r#"{"type":"user","timestamp":"2026-11-08T05:00:00Z","message":{"role":"user","content":"<system-reminder>noise</system-reminder>hello there"}}"#,
            r#"{"type":"assistant","timestamp":"2026-11-08T05:00:05Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","timestamp":"2026-11-08T05:01:00Z","message":{"role":"user","content":"📨 来自 🪞 Transparent（通过 Dispatch，2026-11-08 13:00，id abcd1234）：\n\nping\n\n（这是其他 Agent 发来的消息）"}}"#,
            r#"{"type":"user","timestamp":"2026-11-08T05:02:00Z","message":{"role":"user","content":"📨 Dispatch: 1 message(s) from other agents for you.\n\n━━ Message 1 (from Codex release, 2026-11-08 13:01, id ef012345) ━━\nbuilt\n"}}"#,
            r#"{"type":"system","content":"ignored"}"#,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>just output</local-command-stdout>"}}"#,
        ];
        let turns = parse(&lines.join("\n"), Harness::Claude, 10);
        assert_eq!(turns.len(), 4, "{turns:?}");
        assert_eq!(turns[0].text, "hello there");
        assert_eq!(turns[0].at.as_deref(), Some("2026-11-08T05:00:00Z"));
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "hi");
        assert!(turns[2].via_dispatch);
        assert_eq!(turns[2].from_agent.as_deref(), Some("🪞 Transparent"));
        assert!(turns[3].via_dispatch);
        assert_eq!(turns[3].from_agent.as_deref(), Some("Codex release"));
        // the limit keeps the newest
        let last = parse(&lines.join("\n"), Harness::Claude, 1);
        assert_eq!(last.len(), 1);
        assert!(last[0].text.starts_with("📨 Dispatch"));
    }

    #[test]
    fn rounds_group_a_prompt_with_everything_said_back() {
        let mk = |role: &str, text: &str| Turn { role: role.into(), text: text.into(), at: None, via_dispatch: text.starts_with("📨"), from_agent: None };
        let turns = vec![mk("assistant", "orphan"), mk("user", "q1"), mk("user", "[Image: source: /tmp/a.png]"), mk("assistant", "a1"), mk("assistant", "a1b"), mk("user", "📨 from x"), mk("assistant", "a2"), mk("user", "q3")];
        let all = rounds(&turns, 10);
        assert_eq!(all.len(), 4);
        assert!(all[0].user.is_none() && all[0].replies.len() == 1);
        assert_eq!(all[1].user.as_ref().unwrap().text, "q1\n[Image: source: /tmp/a.png]", "image paths join the prompt they belong to");
        assert_eq!(all[1].replies.len(), 2);
        assert!(all[2].user.as_ref().unwrap().via_dispatch);
        assert!(all[3].replies.is_empty(), "a prompt still being answered has no replies yet");
        assert_eq!(rounds(&turns, 2).len(), 2);
        assert_eq!(rounds(&turns, 2)[0].user.as_ref().unwrap().text, "📨 from x");
    }

    #[test]
    fn codex_turns_skip_developer_and_context_and_dedupe_events() {
        let lines = [
            r#"{"timestamp":"2026-10-28T12:14:15Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"You are root"}]}}"#,
            r#"{"timestamp":"2026-10-28T12:14:15Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context><cwd>/x</cwd></environment_context>"}]}}"#,
            r#"{"timestamp":"2026-10-28T12:14:16Z","type":"event_msg","payload":{"type":"user_message","message":"say ok"}}"#,
            r#"{"timestamp":"2026-10-28T12:14:16Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"say ok"}]}}"#,
            r#"{"timestamp":"2026-10-28T12:14:20Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
        ];
        let mut lines = lines.to_vec();
        lines.insert(2, r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /x\n\n<INSTRUCTIONS>\nbe nice\n</INSTRUCTIONS>"}]}}"##);
        let turns = parse(&lines.join("\n"), Harness::Codex, 10);
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "say ok");
        assert_eq!(turns[1].text, "ok");
    }
}
