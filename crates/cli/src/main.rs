use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dispatch_core::{claude, discover, hook, model, reach, send, store};
use std::io::{Read, Write};

#[derive(Parser)]
#[command(name = "dispatch", version, about = "Find an agent session on this machine and deliver a message into it")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the sessions on this machine (running first)
    Sessions {
        #[arg(long)]
        json: bool,
        /// Include sessions that are not running
        #[arg(long)]
        all: bool,
    },
    /// Send a message to a session (id, id prefix, desktop name or title fragment)
    Send {
        #[arg(long)]
        to: Vec<String>,
        /// Who it is from: a session id or a name (defaults to $CLAUDE_SESSION_ID or "cli")
        #[arg(long)]
        from: Option<String>,
        #[arg(long, default_value = "普通")]
        urgency: String,
        /// Do not wait for the transcript to confirm the queue
        #[arg(long)]
        no_wait: bool,
        #[arg(long)]
        json: bool,
        text: Vec<String>,
    },
    /// Reply to a message (by its id) back to its sender
    Reply {
        id: String,
        text: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// The messages waiting for a session (what the hook hands over)
    Inbox {
        #[arg(long)]
        session: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Where a message stands: its copies, their delivery runs and receipts
    Status {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// The Claude Code hook
    Hook {
        #[command(subcommand)]
        cmd: HookCmd,
    },
    /// Print the store folder
    Where,
}

#[derive(Subcommand)]
enum HookCmd {
    /// Add the hook to ~/.claude/settings.json (idempotent)
    Install,
    /// Remove it
    Uninstall,
    /// Is it installed?
    Status,
    /// Handle one hook event (Claude Code pipes the event JSON to stdin)
    Run,
}

fn main() {
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dispatch: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn sender_identity(from: Option<String>) -> (String, String) {
    if let Some(f) = from {
        if model::is_uuid(&f) {
            let name = claude::find_live(&f).and_then(|i| i.name).unwrap_or_else(|| model::short_id(&f));
            return (f.to_lowercase(), name);
        }
        return (f.clone(), f);
    }
    if let Ok(sid) = std::env::var("CLAUDE_SESSION_ID") {
        if model::is_uuid(&sid) {
            let name = claude::find_live(&sid).and_then(|i| i.name).unwrap_or_else(|| model::short_id(&sid));
            return (sid.to_lowercase(), name);
        }
    }
    ("cli".into(), "cli".into())
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let st = store::Store::open();
    match cli.cmd {
        Cmd::Where => {
            println!("{}", st.root().display());
        }
        Cmd::Sessions { json, all } => {
            let hook_on = hook::installed();
            let list: Vec<model::Session> = discover::sessions().into_iter().filter(|s| all || s.running).collect();
            if json {
                let rows: Vec<serde_json::Value> = list
                    .iter()
                    .map(|s| {
                        let r = reach::decide(s, hook_on);
                        serde_json::json!({ "session": s, "reach": r })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                if list.is_empty() {
                    println!("没有正在运行的会话（加 --all 看最近 48 小时的）。");
                }
                for s in &list {
                    let r = reach::decide(s, hook_on);
                    println!(
                        "{:<8} {:<7} {:<4} {:<26} {:<32} {}",
                        s.short_id,
                        s.harness.label(),
                        if s.running { "在" } else { "  " },
                        truncate(&s.display_name(), 26),
                        truncate(&s.cwd.as_deref().map(model::short_cwd).unwrap_or_default(), 32),
                        r.text
                    );
                }
            }
        }
        Cmd::Send { to, from, urgency, no_wait, json, text } => {
            if to.is_empty() {
                return Err(anyhow!("--to 至少一个"));
            }
            let body = read_text(text)?;
            let (from_id, from_name) = sender_identity(from);
            let dispatch_id = uuid_v4();
            let mut reports = vec![];
            for t in &to {
                let target = send::resolve(t)?;
                let rep = send::send(&st, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: body.clone(), urgency: urgency.clone(), dispatch_id: Some(dispatch_id.clone()), wait: !no_wait })?;
                if !json {
                    println!("→ {} ({}): {} [{}]", target.display_name(), target.short_id, rep.detail, model::short_id(&rep.letter_id));
                }
                reports.push(rep);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            }
        }
        Cmd::Reply { id, text, json } => {
            let body = read_text(text)?;
            let mut orig = st.find(&id)?.ok_or_else(|| anyhow!("没有 id 以「{id}」开头的消息"))?;
            if !model::is_uuid(&orig.from) {
                return Err(anyhow!("这条消息的发件方「{}」不是一个会话，没法回它", orig.from));
            }
            let target = send::resolve(&orig.from)?;
            let rep = send::send(&st, send::Request { to: target.clone(), from: orig.to.clone(), from_name: orig.to_name.clone(), text: body, urgency: orig.urgency.clone(), dispatch_id: Some(orig.dispatch_id.clone()), wait: true })?;
            st.set_status(&mut orig, store::REPLIED)?;
            st.append_run(&orig.id, "reply", "sent", &rep.letter_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rep)?);
            } else {
                println!("↩ {} ({}): {}", target.display_name(), target.short_id, rep.detail);
            }
        }
        Cmd::Inbox { session, all, json } => {
            let target = send::resolve(&session)?;
            let letters = st.list(Some(&target.id), !all)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&letters)?);
            } else if letters.is_empty() {
                println!("{} 没有{}消息。", target.display_name(), if all { "" } else { "未读" });
            } else {
                for l in &letters {
                    println!("[{}] {} 来自 {} · {}\n    {}", model::short_id(&l.id), l.status, l.from_name, l.created, first_line(&l.body));
                }
            }
        }
        Cmd::Status { id, json } => {
            let l = st.find(&id)?.ok_or_else(|| anyhow!("没有 id 以「{id}」开头的消息"))?;
            let copies = st.copies(&l.dispatch_id)?;
            let mut rows = vec![];
            for c in &copies {
                let runs = st.runs(&c.id);
                let delivered_inbox = runs.iter().any(|r| r.action == "inbox" && r.result == "written");
                let receipt = if delivered_inbox { claude::receipt(&c.to, None, &c.id).ok() } else { None };
                let stage = match (&receipt, c.status.as_str()) {
                    (_, store::REPLIED) => "已回复".to_string(),
                    (Some(claude::Receipt::Completed { .. }), _) => "本轮结束".into(),
                    (Some(claude::Receipt::Started), _) => "处理中".into(),
                    (Some(claude::Receipt::Queued), _) => "已送达（排在本轮之后）".into(),
                    (_, store::READ_BY_HOOK) => "已送达（hook 递的）".into(),
                    (_, store::DELIVERED_INBOX) => "已送达".into(),
                    _ => "排队中".into(),
                };
                if !json {
                    println!("{} → {} ({}): {}\n    文件 {}", c.from_name, c.to_name, model::short_id(&c.to), stage, c.path);
                    for r in &runs {
                        println!("    {} {} {} {}", &r.at[11..19.min(r.at.len())], r.action, r.result, r.detail);
                    }
                }
                rows.push(serde_json::json!({ "letter": c, "stage": stage, "runs": runs }));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            }
        }
        Cmd::Hook { cmd } => match cmd {
            HookCmd::Install => {
                let bin = std::env::current_exe()?.to_string_lossy().to_string();
                hook::install(&bin)?;
                println!("已装：UserPromptSubmit / Stop / SessionStart → {bin} hook run");
            }
            HookCmd::Uninstall => {
                hook::uninstall()?;
                println!("已卸。");
            }
            HookCmd::Status => {
                println!("{}", if hook::installed() { "已装" } else { "没装" });
            }
            HookCmd::Run => {
                let mut input = String::new();
                std::io::stdin().read_to_string(&mut input)?;
                let out = hook::run(&st, &input)?;
                if out.to_stderr {
                    eprint!("{}", out.text);
                } else {
                    print!("{}", out.text);
                }
                let _ = std::io::stdout().flush();
                return Ok(out.exit_code);
            }
        },
    }
    Ok(0)
}

fn read_text(words: Vec<String>) -> Result<String> {
    let joined = words.join(" ");
    if !joined.trim().is_empty() {
        return Ok(joined);
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    if s.trim().is_empty() {
        return Err(anyhow!("消息内容是空的（写在参数里，或从 stdin 传）"));
    }
    Ok(s)
}

fn uuid_v4() -> String {
    // the core crate owns the uuid dependency; a v4 from there keeps versions aligned
    dispatch_core::send::new_id()
}

fn truncate(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        format!("{}…", chars[..n - 1].iter().collect::<String>())
    }
}

fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 60)
}
