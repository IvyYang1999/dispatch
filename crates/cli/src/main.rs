use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dispatch_core::i18n::{status_label, t};
use dispatch_core::{claude, discover, hook, model, peer, reach, send, store};
use std::io::{Read, Write};
use std::time::Duration;

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
        /// normal or urgent
        #[arg(long, default_value = "normal")]
        urgency: String,
        /// Do not wait for the transcript to confirm the queue
        #[arg(long)]
        no_wait: bool,
        /// Codex only: let `codex queue --resume` load a thread the daemon has not loaded
        #[arg(long)]
        allow_resume: bool,
        /// How long it may wait before it counts as not delivered: 30m, 2h, 1d (default: 1h urgent, 24h normal)
        #[arg(long)]
        deadline: Option<String>,
        #[arg(long)]
        json: bool,
        text: Vec<String>,
    },
    /// Try a queued or failed message again now (by letter id or dispatch id)
    Retry {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a queued or failed message: nothing delivers it any more
    Cancel {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// The postman's round: expire overdue messages, retry queued ones whose route is open now
    Sweep {
        #[arg(long)]
        json: bool,
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
    /// Serve MCP over stdio (what the plugins start)
    Mcp,
    /// Who am I, as the session that started this process sees it
    Whoami,
    /// What this machine's Codex can do right now: CLI, daemon, loaded threads
    Codex,
    /// What this machine's Grok Bot can do right now: app, sign-in, gbot, the bots
    Grok,
    /// Put the binary at ~/.dispatch/bin/dispatch, install the hook from there, register with Codex
    Install,
    /// Other machines running Dispatch: list them, name this one, pair, scan the network
    Peers {
        #[command(subcommand)]
        cmd: Option<PeersCmd>,
        #[arg(long)]
        json: bool,
    },
    /// The address book: stable names for sessions (`add <name> [session]`, `remove <name>`)
    Contacts {
        #[command(subcommand)]
        cmd: Option<ContactsCmd>,
        #[arg(long)]
        json: bool,
    },
    /// Listen for other machines in the foreground (the app does this while it runs)
    Serve {
        /// Port to listen on (default 47831)
        #[arg(long)]
        port: Option<u16>,
    },
    /// Print the store folder
    Where,
}

#[derive(Subcommand)]
enum ContactsCmd {
    /// Every name and the session it points at
    List,
    /// Name a session (id, prefix, name or title fragment); without one, the session this command runs in
    Add {
        name: String,
        session: Option<String>,
        /// One line on what the session is for (other agents match it when looking for a role)
        #[arg(long)]
        about: Option<String>,
    },
    /// Forget a name
    Remove { name: String },
}

#[derive(Subcommand)]
enum PeersCmd {
    /// This machine and the paired ones, with who is online
    List,
    /// Rename this machine (the name other machines see)
    Name { name: String },
    /// Browse the network for other machines running Dispatch
    Scan,
    /// Show a pairing code for two minutes; the other machine pairs with it
    Code,
    /// Pair with a machine: its name as seen on the network (or host:port), and the code it shows
    Pair { target: String, code: String },
    /// Give a paired machine a fixed address (host:port) for when the network cannot see it
    Add { name: String, address: String },
    /// Forget a paired machine
    Remove { name: String },
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
    // `dispatch sessions | head` closes the pipe early: end quietly instead of panicking in println.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
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
        Cmd::Mcp => {
            dispatch_core::mcp::serve(&st);
        }
        Cmd::Whoami => {
            println!("{}", serde_json::to_string_pretty(&dispatch_core::ops::whoami())?);
        }
        Cmd::Codex => {
            let s = dispatch_core::codex::state_now();
            println!("{}", t("cli.codex_cmd").replace("{v}", if s.cli { t("cli.yes") } else { t("cli.no") }));
            println!("{}", t("cli.daemon").replace("{v}", if s.daemon { t("cli.on") } else { t("cli.off") }).replace("{p}", &dispatch_core::codex::daemon_socket().display().to_string()));
            println!("{}", t("cli.loaded").replace("{v}", &if s.loaded.is_empty() { t("cli.none").to_string() } else { s.loaded.iter().map(|x| model::short_id(x)).collect::<Vec<_>>().join(" ") }));
            if s.cli {
                match dispatch_core::codex::Server::connect() {
                    Ok(mut srv) => {
                        println!("{}", t("cli.appserver").replace("{v}", if srv.shared { t("cli.shared") } else { t("cli.private") }));
                        match srv.list_threads(5) {
                            Ok(list) => {
                                println!("{}", t("cli.thread_list").replace("{n}", &list.len().to_string()));
                                for th in list {
                                    println!("  {} {} {}", th.get("id").and_then(|v| v.as_str()).map(model::short_id).unwrap_or_default(), th.pointer("/status/type").and_then(|v| v.as_str()).unwrap_or("?"), th.get("name").or(th.get("title")).and_then(|v| v.as_str()).unwrap_or(""));
                                }
                            }
                            Err(e) => println!("{}", t("cli.thread_list_failed").replace("{e}", &format!("{e:#}"))),
                        }
                    }
                    Err(e) => println!("{}", t("cli.appserver_failed").replace("{e}", &format!("{e:#}"))),
                }
            }
        }
        Cmd::Grok => {
            let s = dispatch_core::grok::state_now();
            println!("{}", t("cli.grok_app").replace("{v}", if s.app { t("cli.yes") } else { t("cli.no") }));
            println!("{}", t("cli.grok_login").replace("{v}", if s.signed_in { t("cli.yes") } else { t("cli.no") }));
            println!("{}", t("cli.grok_cli").replace("{v}", &s.cli.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| t("cli.no").to_string())));
            if let Some(e) = &s.error {
                println!("{}", t("cli.grok_error").replace("{e}", e));
            }
            println!("{}", t("cli.grok_bots").replace("{n}", &s.bots.len().to_string()));
            for b in &s.bots {
                println!("  {}  {}{}  {}", model::short_id(&b.id), b.name, if b.is_group { " (group)" } else { "" }, truncate(&b.description, 48));
            }
        }
        Cmd::Install => {
            let home = model::home_dir();
            let bin_dir = home.join(".dispatch").join("bin");
            std::fs::create_dir_all(&bin_dir)?;
            let target = bin_dir.join(if cfg!(windows) { "dispatch.exe" } else { "dispatch" });
            let me = std::env::current_exe()?;
            if me != target {
                let tmp = bin_dir.join(".dispatch.tmp");
                std::fs::copy(&me, &tmp)?;
                std::fs::rename(&tmp, &target)?;
            }
            println!("{}", t("cli.binary").replace("{p}", &target.display().to_string()));
            hook::install(&target.to_string_lossy())?;
            println!("{}", t("cli.hook_installed"));
            if let Ok(out) = dispatch_core::codex::command().args(["mcp", "add", "dispatch", "--", &target.to_string_lossy(), "mcp"]).output() {
                if out.status.success() {
                    println!("{}", t("cli.codex_registered"));
                } else {
                    println!("{}", t("cli.codex_register_failed").replace("{e}", String::from_utf8_lossy(&out.stderr).trim()));
                }
            } else {
                println!("{}", t("cli.codex_missing"));
            }
            println!("{}", t("cli.claude_plugin").replace("{cmd}", "claude plugin marketplace add IvyYang1999/dispatch && claude plugin install dispatch@dispatch"));
        }
        Cmd::Peers { cmd, json } => match cmd.unwrap_or(PeersCmd::List) {
            PeersCmd::List => {
                let v = peer::overview();
                if json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print_peers(&v);
                }
            }
            PeersCmd::Name { name } => {
                let me = peer::set_name(&name)?;
                println!("{}", t("cli.peers_named").replace("{name}", &me.name));
            }
            PeersCmd::Scan => {
                peer::mdns::scan(Duration::from_secs(2));
                let v = peer::overview();
                if json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print_peers(&v);
                }
            }
            PeersCmd::Code => {
                if peer::mdns::serving().is_none() {
                    println!("{}", t("cli.peers_code_needs_listener"));
                }
                let (code, secs) = peer::new_code()?;
                let me = peer::me();
                println!("{}", t("cli.peers_code").replace("{code}", &code).replace("{secs}", &secs.to_string()).replace("{name}", &me.name));
            }
            PeersCmd::Pair { target, code } => {
                let p = peer::pair(&target, &code)?;
                println!("{}", t("cli.peers_paired").replace("{name}", &p.name).replace("{id}", &model::short_id(&p.id)));
            }
            PeersCmd::Add { name, address } => {
                let p = peer::set_address(&name, Some(address))?;
                println!("{}", t("cli.peers_address").replace("{name}", &p.name).replace("{addr}", p.address.as_deref().unwrap_or("")));
            }
            PeersCmd::Remove { name } => match peer::forget(&name)? {
                Some(p) => println!("{}", t("cli.peers_removed").replace("{name}", &p.name)),
                None => return Err(anyhow!("{}", t("cli.peers_not_found").replace("{q}", &name))),
            },
        },
        Cmd::Contacts { cmd, json } => match cmd.unwrap_or(ContactsCmd::List) {
            ContactsCmd::List => {
                let v = dispatch_core::ops::contacts();
                if json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    let rows = v["contacts"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                    if rows.is_empty() {
                        println!("{}", t("cli.contacts_none"));
                    }
                    for c in rows {
                        let mark = if c["provisional"].as_bool().unwrap_or(false) { "≈ " } else { "" };
                        println!("{:<24} {:<8} {:<9} {:<4} {:<32} {}", truncate(&format!("{mark}{}", c["address"].as_str().unwrap_or("")), 24), c["harness"].as_str().unwrap_or(""), c["short_id"].as_str().unwrap_or(""), if c["running"].as_bool().unwrap_or(false) { t("cli.running_mark") } else { "  " }, truncate(&c["cwd"].as_str().map(model::short_cwd).unwrap_or_default(), 32), c["about"].as_str().unwrap_or(""));
                    }
                }
            }
            ContactsCmd::Add { name, session, about } => {
                let v = dispatch_core::ops::register(&name, session.as_deref(), "cli", about.as_deref())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!("{}", v["text"].as_str().unwrap_or(""));
                }
            }
            ContactsCmd::Remove { name } => {
                dispatch_core::ops::unregister(&name)?;
                println!("{}", t("cli.contacts_removed").replace("{name}", &name));
            }
        },
        Cmd::Serve { port } => {
            if let Some(p) = peer::mdns::serving() {
                return Err(anyhow!("{}", t("cli.serve_busy").replace("{port}", &p.to_string())));
            }
            let h = peer::serve(port.unwrap_or(0))?;
            println!("{}", t("cli.serve").replace("{port}", &h.port.to_string()).replace("{mdns}", if h.mdns { t("cli.on") } else { t("cli.off") }));
            // the postman's round, as the app's watcher would do it
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let _ = dispatch_core::ops::sweep(&st);
            }
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
                    println!("{}", t("cli.no_running"));
                }
                for s in &list {
                    let r = reach::decide(s, hook_on);
                    println!(
                        "{:<18} {:<7} {:<4} {:<26} {:<32} {}",
                        truncate(&s.short_address(), 18),
                        s.harness.label(),
                        if s.running { t("cli.running_mark") } else { "  " },
                        truncate(&s.display_name(), 26),
                        truncate(&s.cwd.as_deref().map(model::short_cwd).unwrap_or_default(), 32),
                        r.text
                    );
                }
            }
        }
        Cmd::Send { to, from, urgency, no_wait, allow_resume, deadline, json, text } => {
            if to.is_empty() {
                return Err(anyhow!("{}", t("cli.to_required")));
            }
            let body = read_text(text)?;
            let deadline = dispatch_core::ops::parse_deadline(deadline.as_deref())?;
            let (from_id, from_name) = sender_identity(from);
            let dispatch_id = uuid_v4();
            let urgency = store::urgency_stored(&urgency).to_string();
            let mut reports = vec![];
            for who in &to {
                let (target, how) = send::resolve_how(who)?;
                let rep = send::send(&st, send::Request { to: target.clone(), from: from_id.clone(), from_name: from_name.clone(), text: body.clone(), urgency: urgency.clone(), dispatch_id: Some(dispatch_id.clone()), wait: !no_wait, allow_resume, deadline })?;
                if !json {
                    println!("→ {} ({}): {} [{}]", target.address(), target.short_id, rep.detail, model::short_id(&rep.letter_id));
                    if let Some(n) = dispatch_core::ops::remember_match(who, &target, how) {
                        println!("  {}", n["note"].as_str().unwrap_or(""));
                    }
                } else {
                    dispatch_core::ops::remember_match(who, &target, how);
                }
                reports.push(rep);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            }
        }
        Cmd::Reply { id, text, json } => {
            let body = read_text(text)?;
            let v = dispatch_core::ops::reply(&st, &id, &body)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("↩ {} ({}): {}", v["to"]["name"].as_str().unwrap_or(""), model::short_id(v["to"]["id"].as_str().unwrap_or("")), v["detail"].as_str().unwrap_or(""));
            }
        }
        Cmd::Inbox { session, all, json } => {
            let target = send::resolve(&session)?;
            let letters = st.list(Some(&target.id), !all)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&letters)?);
            } else if letters.is_empty() {
                println!("{}", t("cli.inbox_empty").replace("{who}", &target.display_name()).replace("{unread}", if all { "" } else { t("cli.unread") }));
            } else {
                for l in &letters {
                    println!("{}\n    {}", t("cli.inbox_line").replace("{id}", &model::short_id(&l.id)).replace("{status}", status_label(&l.status)).replace("{from}", &send::sender_text(l)).replace("{created}", &l.created), first_line(&l.body));
                }
            }
        }
        Cmd::Retry { id, json } => {
            let v = dispatch_core::ops::retry(&st, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for c in v["copies"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    println!("↻ {} ({}): {}", c["to_name"].as_str().unwrap_or(""), model::short_id(c["to"].as_str().unwrap_or("")), c["detail"].as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Cancel { id, json } => {
            let v = dispatch_core::ops::cancel(&st, &id, "cli")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("{}", v["text"].as_str().unwrap_or(""));
            }
        }
        Cmd::Sweep { json } => {
            let v = dispatch_core::ops::sweep(&st)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                let retried = v["retried"].as_array().map(|a| a.len()).unwrap_or(0);
                println!("{}", t("cli.sweep").replace("{failed}", &v["failed"].to_string()).replace("{retried}", &retried.to_string()));
                for c in v["retried"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    println!("{}", t("cli.sweep_retry").replace("{to}", c["to_name"].as_str().unwrap_or("")).replace("{detail}", c["detail"].as_str().unwrap_or("")));
                }
            }
        }
        Cmd::Status { id, json } => {
            let v = dispatch_core::ops::status(&st, &id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for c in v["copies"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    println!("{} → {} ({}): {}\n    {}", c["from_name"].as_str().unwrap_or(""), c["to_name"].as_str().unwrap_or(""), model::short_id(c["to"].as_str().unwrap_or("")), c["stage"].as_str().unwrap_or(""), t("cli.file").replace("{p}", c["path"].as_str().unwrap_or("")));
                    for r in c["runs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                        let at = r["at"].as_str().unwrap_or("");
                        println!("    {} {} {} {}", at.get(11..19).unwrap_or(at), r["action"].as_str().unwrap_or(""), r["result"].as_str().unwrap_or(""), r["detail"].as_str().unwrap_or(""));
                    }
                }
            }
        }
        Cmd::Hook { cmd } => match cmd {
            HookCmd::Install => {
                let bin = std::env::current_exe()?.to_string_lossy().to_string();
                hook::install(&bin)?;
                println!("{}", t("cli.hook_done").replace("{bin}", &bin));
            }
            HookCmd::Uninstall => {
                hook::uninstall()?;
                println!("{}", t("cli.hook_removed"));
            }
            HookCmd::Status => {
                println!("{}", if hook::installed() { t("cli.installed") } else { t("cli.not_installed") });
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

fn print_peers(v: &serde_json::Value) {
    let me = &v["me"];
    println!("{}", t("cli.peers_me").replace("{name}", me["name"].as_str().unwrap_or("")).replace("{id}", &model::short_id(me["id"].as_str().unwrap_or(""))));
    let listening = me["listening"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| t("cli.peers_not_listening").to_string());
    println!("{}", t("cli.peers_listening").replace("{v}", &listening));
    let peers = v["peers"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    if peers.is_empty() {
        println!("{}", t("cli.peers_none"));
    }
    for p in peers {
        let state = if p["online"].as_bool().unwrap_or(false) { t("cli.peers_online") } else { t("cli.peers_offline") };
        println!("  {:<20} {:<9} {:<6} {}", truncate(p["name"].as_str().unwrap_or(""), 20), model::short_id(p["id"].as_str().unwrap_or("")), state, p["address"].as_str().unwrap_or(""));
    }
    let seen = v["seen"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    if !seen.is_empty() {
        println!("{}", t("cli.peers_seen"));
        for s in seen {
            // IPv4 first: that is the address a person would type
            let addrs = s["addrs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            let addr = addrs.iter().filter_map(|a| a.as_str()).find(|a| !a.contains(':')).or_else(|| addrs.first().and_then(|a| a.as_str())).unwrap_or("?");
            println!("  {:<20} {:<9} {}:{}", truncate(s["name"].as_str().unwrap_or(""), 20), model::short_id(s["id"].as_str().unwrap_or("")), addr, s["port"]);
        }
    }
}

fn read_text(words: Vec<String>) -> Result<String> {
    let joined = words.join(" ");
    if !joined.trim().is_empty() {
        return Ok(joined);
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    if s.trim().is_empty() {
        return Err(anyhow!("{}", t("cli.empty_text")));
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
