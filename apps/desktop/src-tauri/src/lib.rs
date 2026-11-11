//! The desktop shell: thin Tauri commands over dispatch-core, a menu bar item and
//! system notifications.
//!
//! Every command is async and does its work on the blocking pool: the core reads
//! files and runs `ps`, and a sync command would do that on the main thread,
//! where it freezes clicks, drags and even the Dock's activate.
//!
//! A watcher thread reads the feed every few seconds. It keeps the menu bar item
//! current (how many dispatches are queued or failed, the most recent few), posts a
//! notification when a reply comes in or a delivery fails, and tells the window to
//! refresh. Closing the window hides it; the app stays in the menu bar until Quit.
//!
//! A second watcher looks every second for messages held for the user's review and brings
//! up the review window: on top of everything, but without taking the keyboard.

use dispatch_core::i18n::{self, t};
use dispatch_core::{hook, model, ops, peer, review, settings, store::Store, watch};
use std::collections::HashSet;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Theme};
use tauri_plugin_notification::NotificationExt;

mod cli_sync;
mod icons;

const TRAY_ID: &str = "main";
const RECENT_IN_TRAY: usize = 6;

/// The watcher's latest feed, so the window's polls do not read everything again.
#[derive(Default)]
struct Cache {
    feed: Mutex<Option<(Instant, Value)>>,
}

/// Held copies the user put away by closing the review window: it comes back for a new one,
/// or when the user opens it from the feed.
#[derive(Default)]
struct Reviews {
    dismissed: Mutex<HashSet<String>>,
}

const REVIEW_WINDOW: &str = "review";

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn sessions(running_only: bool) -> Result<Value, String> {
    blocking(move || Ok(ops::sessions(running_only))).await
}

#[tauri::command]
async fn find(query: String) -> Result<Value, String> {
    blocking(move || Ok(ops::find(&query))).await
}

/// The feed: the watcher's copy when it is fresh, else read now.
/// The conversations, one per pair who wrote to each other (see `ops::threads`).
#[tauri::command]
async fn threads(limit: Option<usize>) -> Result<Value, String> {
    let limit = limit.unwrap_or(200);
    blocking(move || ops::threads(&Store::open(), limit).map_err(err)).await
}

#[tauri::command]
async fn feed(app: AppHandle, limit: Option<usize>) -> Result<Value, String> {
    let limit = limit.unwrap_or(200);
    if let Some((at, v)) = app.state::<Cache>().feed.lock().ok().and_then(|c| c.clone()) {
        if at.elapsed() < Duration::from_secs(6) {
            if let Some(list) = v["dispatches"].as_array() {
                if list.len() <= limit {
                    return Ok(v);
                }
                return Ok(json!({ "count": limit, "dispatches": list[..limit] }));
            }
        }
    }
    blocking(move || ops::feed(&Store::open(), limit).map_err(err)).await
}

#[tauri::command]
async fn dispatch_status(id: String) -> Result<Value, String> {
    blocking(move || ops::status(&Store::open(), &id).map_err(err)).await
}

#[tauri::command]
async fn send(app: AppHandle, to: Vec<String>, text: String, urgency: Option<String>, allow_resume: Option<bool>, deadline: Option<String>) -> Result<Value, String> {
    let urgency = urgency.unwrap_or_else(|| "normal".into());
    let r = blocking(move || ops::send_many_opts(&Store::open(), &to, &text, &urgency, Some(("app".into(), t("me").into())), allow_resume.unwrap_or(false), deadline.as_deref()).map_err(err)).await;
    forget_feed(&app);
    r
}

/// The language the app and every later `dispatch` process use: `en` or `zh`.
#[tauri::command]
async fn get_lang() -> Result<String, String> {
    blocking(|| Ok(i18n::lang().code().to_string())).await
}

#[tauri::command]
async fn set_lang(app: AppHandle, lang: String) -> Result<String, String> {
    let r = blocking(move || {
        let l = i18n::parse(&lang).ok_or_else(|| format!("unknown language {lang}"))?;
        i18n::save(l).map_err(|e| e.to_string())?;
        Ok(l.code().to_string())
    })
    .await;
    // Stage texts in the cached feed are in the old language; the tray follows on its next tick.
    forget_feed(&app);
    r
}

/// `system`, `light` or `dark`, from the settings file.
fn theme_setting() -> String {
    settings::get("theme").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "system".into())
}

/// Put the saved appearance on the window; the page's dark styles follow `prefers-color-scheme`.
fn theme_now() -> Option<Theme> {
    match theme_setting().as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}

fn apply_theme(app: &AppHandle) {
    for label in ["main", REVIEW_WINDOW] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_theme(theme_now());
        }
    }
}

#[tauri::command]
async fn set_theme(app: AppHandle, theme: String) -> Result<String, String> {
    if !matches!(theme.as_str(), "system" | "light" | "dark") {
        return Err(format!("unknown theme {theme}"));
    }
    settings::set("theme", Value::String(theme.clone())).map_err(|e| e.to_string())?;
    apply_theme(&app);
    Ok(theme)
}

/// Claude Code's `crossSessionInbound`: `accept` lets sessions that bypass permission checks
/// take Dispatch's messages at once; off removes the key (Claude Code's own rule applies).
#[tauri::command]
async fn set_inbound(on: bool) -> Result<Value, String> {
    blocking(move || {
        dispatch_core::claude::set_inbound(if on { Some("accept") } else { None }).map_err(err)?;
        Ok(json!({ "inbound": dispatch_core::claude::inbound_setting() }))
    })
    .await
}

/// System notifications on or off (replies and failed deliveries).
#[tauri::command]
async fn set_notify(on: bool) -> Result<bool, String> {
    blocking(move || {
        settings::set("notify", Value::Bool(on)).map_err(|e| e.to_string())?;
        Ok(on)
    })
    .await
}

// ---- other machines ----

/// Me, the paired machines (with who answers right now) and the machines seen but not paired.
#[tauri::command]
async fn peers_overview() -> Result<Value, String> {
    blocking(|| peer::overview().map_err(err)).await
}

/// Browse the network for two seconds, then the overview.
#[tauri::command]
async fn peer_scan() -> Result<Value, String> {
    blocking(|| {
        peer::mdns::scan(Duration::from_secs(2));
        peer::overview().map_err(err)
    })
    .await
}

#[tauri::command]
async fn peer_set_name(name: String) -> Result<Value, String> {
    blocking(move || peer::set_name(&name).map(|m| json!({ "id": m.id, "name": m.name })).map_err(err)).await
}

/// A pairing code for two minutes; the other machine enters it.
#[tauri::command]
async fn peer_code() -> Result<Value, String> {
    blocking(|| peer::new_code().map(|(code, secs)| json!({ "code": code, "seconds_left": secs })).map_err(err)).await
}

/// Pair with the machine at `target` (a name seen on the network, or host:port) using its code,
/// or with a pairing link (then the code is not needed).
#[tauri::command]
async fn peer_pair(target: String, code: Option<String>) -> Result<Value, String> {
    blocking(move || {
        let p = if peer::is_link(&target) { peer::pair_link(&target) } else { peer::pair(&target, code.as_deref().unwrap_or("")) };
        let p = p.map_err(|e| e.to_string())?;
        let fingerprint = peer::fingerprint(&p.public_key).map_err(err)?;
        Ok(json!({ "id": p.id, "name": p.name, "verified": p.verified, "fingerprint": fingerprint }))
    })
    .await
}

/// A pairing link for a machine on another network: ten minutes, one use.
#[tauri::command]
async fn peer_link() -> Result<Value, String> {
    blocking(|| peer::new_link().map(|(link, secs)| json!({ "link": link, "seconds_left": secs })).map_err(err)).await
}

/// A person compared the fingerprints: confirm the machine and release its held letters.
#[tauri::command]
async fn peer_verify(name: String) -> Result<Value, String> {
    blocking(move || peer::verify_peer(&name).map(|(p, n)| json!({ "name": p.name, "released": n })).map_err(err)).await
}

#[tauri::command]
async fn peer_rule(name: String, rule: String) -> Result<Value, String> {
    blocking(move || peer::set_rule(&name, &rule).map(|p| json!({ "name": p.name, "rule": p.rule })).map_err(err)).await
}

/// The internet path on or off; it takes effect when the app restarts.
#[tauri::command]
async fn set_peer_internet(on: bool) -> Result<bool, String> {
    blocking(move || settings::set("peer_internet", Value::Bool(on)).map(|_| on).map_err(|e| e.to_string())).await
}

/// The user's own relay (an iroh-relay URL), or none for the default; takes effect on restart.
#[tauri::command]
async fn set_peer_relay(url: Option<String>) -> Result<Value, String> {
    blocking(move || {
        let v = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        if let Some(u) = &v {
            if !(u.starts_with("https://") || u.starts_with("http://")) {
                return Err("the relay address must start with https://".into());
            }
        }
        settings::set("peer_relay", v.clone().map(Value::String).unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
        Ok(json!({ "relay": v }))
    })
    .await
}

#[tauri::command]
async fn peer_remove(name: String) -> Result<bool, String> {
    blocking(move || peer::forget(&name).map(|p| p.is_some()).map_err(err)).await
}

#[tauri::command]
async fn peer_set_address(name: String, address: Option<String>) -> Result<Value, String> {
    blocking(move || peer::set_address(&name, address).map(|p| json!({ "id": p.id, "name": p.name, "address": p.address })).map_err(err)).await
}

/// macOS: the Privacy & Security → Local Network pane, where the user allows Dispatch to reach
/// other machines; elsewhere nothing to open.
#[tauri::command]
async fn open_local_network_settings() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")?;
        return Ok(true);
    }
    #[allow(unreachable_code)]
    Ok(false)
}

/// Listen for other machines: the port from settings (`peer_port`), else the default.
fn start_listener() {
    std::thread::Builder::new()
        .name("dispatch-peer-start".into())
        .spawn(|| {
            let port = settings::get("peer_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            match peer::serve(port) {
                Ok(h) => eprintln!("dispatch: listening for other machines on port {}{}", h.port, if h.mdns { "" } else { " (mDNS off)" }),
                Err(e) => eprintln!("dispatch: not listening for other machines: {e:#}"),
            }
        })
        .expect("spawn the listener");
}

/// Name a session in the address book (`session`: id or `machine/id`), with a line on its role.
#[tauri::command]
async fn contact_set(name: String, session: String, about: Option<String>, avatar: Option<dispatch_core::contacts::Look>) -> Result<Value, String> {
    blocking(move || {
        let v = ops::register(&name, Some(&session), "app", about.as_deref()).map_err(err)?;
        if let Some(look) = avatar {
            dispatch_core::contacts::set_avatar(v["name"].as_str().unwrap_or(&name), look).map_err(err)?;
        }
        Ok(v)
    })
    .await
}

/// One session's page: identity, the dispatches it took part in, its last turns.
#[tauri::command]
async fn session_page(id: String) -> Result<Value, String> {
    blocking(move || ops::session_page(&Store::open(), &id).map_err(err)).await
}

/// Every session with records here (any age) or on a paired machine that matches.
#[tauri::command]
async fn search_sessions(query: String) -> Result<Value, String> {
    blocking(move || Ok(ops::search(&query))).await
}

/// The installed clients' icons as data URLs, by harness key (claude, codex, grok).
#[tauri::command]
async fn harness_icons() -> Result<Value, String> {
    blocking(|| Ok(icons::harness_icons(Store::open().root()))).await
}

#[tauri::command]
async fn contact_remove(name: String) -> Result<Value, String> {
    blocking(move || ops::unregister(&name).map_err(err)).await
}

/// Try a queued or failed copy again now.
/// The copies held for review, oldest first.
#[tauri::command]
async fn review_list() -> Result<Vec<String>, String> {
    blocking(|| Ok(review::queued(&Store::open()))).await
}

/// What the review window shows for one held copy; asking for it means it is on screen.
#[tauri::command]
async fn review_info(id: String) -> Result<Value, String> {
    blocking(move || {
        let store = Store::open();
        review::mark_shown(&store, &id).map_err(err)?;
        review::info(&store, &id).map_err(err)
    })
    .await
}

/// The user's answer: send, trust (send, and let this sender write to this recipient from now
/// on) or decline.
#[tauri::command]
async fn review_decide(app: AppHandle, id: String, decision: String) -> Result<Value, String> {
    let r = blocking(move || review::decide(&Store::open(), &id, &decision).map_err(err)).await;
    forget_feed(&app);
    let _ = app.emit("dispatch:changed", ());
    r
}

/// Put the review window away. What is still held stays held, and the window does not come
/// back for it until something new is held or the user opens it from the feed.
#[tauri::command]
async fn review_close(app: AppHandle) -> Result<(), String> {
    let held = blocking(|| Ok(review::queued(&Store::open()))).await?;
    put_away(&app, held);
    if let Some(w) = app.get_webview_window(REVIEW_WINDOW) {
        let _ = w.hide();
    }
    Ok(())
}

/// What the review window held when it was put away: still held, not brought back on its own,
/// its senders told the window was closed, and the feed offers Review to bring it back.
fn put_away(app: &AppHandle, held: Vec<String>) {
    let store = Store::open();
    for id in &held {
        let _ = review::mark_closed(&store, id);
    }
    if let Ok(mut d) = app.state::<Reviews>().dismissed.lock() {
        d.extend(held);
    }
    let _ = app.emit("review:changed", ());
    let _ = app.emit("dispatch:changed", ());
}

/// Open the review window now, in front and with the keyboard (the user asked for it).
#[tauri::command]
async fn review_open(app: AppHandle) -> Result<(), String> {
    if let Ok(mut d) = app.state::<Reviews>().dismissed.lock() {
        d.clear();
    }
    show_review(&app, true);
    let _ = app.emit("review:changed", ());
    Ok(())
}

/// The review setting (`off`, `new`, `all`) and how many sender-recipient pairs go straight through.
#[tauri::command]
async fn review_settings() -> Result<Value, String> {
    blocking(|| Ok(json!({ "mode": review::mode(), "trusted": review::trusted_count(&Store::open()) }))).await
}

#[tauri::command]
async fn set_review(mode: String) -> Result<Value, String> {
    blocking(move || {
        let m = review::set_mode(&mode).map_err(err)?;
        Ok(json!({ "mode": m, "trusted": review::trusted_count(&Store::open()) }))
    })
    .await
}

/// Forget every trusted pair: each sender asks again the next time.
#[tauri::command]
async fn review_forget() -> Result<Value, String> {
    blocking(|| {
        let n = review::forget_all(&Store::open()).map_err(err)?;
        Ok(json!({ "mode": review::mode(), "trusted": 0, "forgot": n }))
    })
    .await
}

#[tauri::command]
async fn retry(app: AppHandle, id: String) -> Result<Value, String> {
    let r = blocking(move || ops::retry(&Store::open(), &id).map_err(err)).await;
    forget_feed(&app);
    r
}

/// Withdraw a queued or failed copy.
#[tauri::command]
async fn cancel(app: AppHandle, id: String) -> Result<Value, String> {
    let r = blocking(move || ops::cancel(&Store::open(), &id, "app").map_err(err)).await;
    forget_feed(&app);
    r
}

#[tauri::command]
async fn reply(app: AppHandle, id: String, text: String) -> Result<Value, String> {
    let r = blocking(move || ops::reply(&Store::open(), &id, &text).map_err(err)).await;
    forget_feed(&app);
    r
}

fn forget_feed(app: &AppHandle) {
    if let Ok(mut c) = app.state::<Cache>().feed.lock() {
        *c = None;
    }
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("cmd").args(["/C", "start", "", url]).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(t("app.open_failed_status").replace("{url}", url).replace("{s}", &s.to_string())),
        Err(e) => Err(t("app.open_failed").replace("{url}", url).replace("{e}", &e.to_string())),
    }
}

fn open_app(name: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").args(["-a", name]).status();
    #[cfg(not(target_os = "macos"))]
    let status: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::other("no app launcher"));
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(t("app.open_failed_status").replace("{url}", name).replace("{s}", &s.to_string())),
        Err(e) => Err(t("app.open_failed").replace("{url}", name).replace("{e}", &e.to_string())),
    }
}

/// A closed terminal session: open Terminal in its folder and resume it there (macOS).
#[cfg(target_os = "macos")]
fn open_in_terminal(command: &str, cwd: Option<&str>) -> Result<(), String> {
    let sh_quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let shell = match cwd {
        Some(d) if !d.is_empty() => format!("cd {} && {}", sh_quote(d), command),
        _ => command.to_string(),
    };
    let as_quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell", as_quote(&shell));
    let status = std::process::Command::new("osascript").args(["-e", &script]).status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(t("app.open_failed_status").replace("{url}", "Terminal").replace("{s}", &status.to_string()))
    }
}

/// Linux: the first terminal emulator on PATH runs the command in a login shell and stays open.
#[cfg(target_os = "linux")]
fn open_in_terminal(command: &str, cwd: Option<&str>) -> Result<(), String> {
    let sh_quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let shell = match cwd {
        Some(d) if !d.is_empty() => format!("cd {} && {}; exec bash", sh_quote(d), command),
        _ => format!("{command}; exec bash"),
    };
    let on_path = |name: &str| std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file())).unwrap_or(false);
    let (term, args): (&str, Vec<&str>) = if on_path("gnome-terminal") {
        ("gnome-terminal", vec!["--", "bash", "-lc", &shell])
    } else if on_path("konsole") {
        ("konsole", vec!["-e", "bash", "-lc", &shell])
    } else if on_path("x-terminal-emulator") {
        ("x-terminal-emulator", vec!["-e", "bash", "-lc", &shell])
    } else if on_path("xterm") {
        ("xterm", vec!["-e", "bash", "-lc", &shell])
    } else {
        return Err("no terminal emulator found".into());
    };
    std::process::Command::new(term).args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_in_terminal(_command: &str, _cwd: Option<&str>) -> Result<(), String> {
    Err("no terminal launcher on this system".into())
}

/// Open the session: a claude:// or codex:// link goes to the system; a Grok bot brings its app
/// forward; a closed terminal session is resumed in a new Terminal window; a running terminal
/// session comes back as a note (Dispatch cannot tell which window it is in).
#[tauri::command]
async fn open_session(id: String) -> Result<Value, String> {
    blocking(move || {
        let mut v = ops::open_session(&id).map_err(err)?;
        if v["kind"] == "url" {
            open_url(v["url"].as_str().unwrap_or(""))?;
        } else if v["kind"] == "app" {
            open_app(v["app"].as_str().unwrap_or(""))?;
        } else if v["kind"] == "command" {
            if v["running"] == true {
                v["kind"] = json!("note");
                v["note"] = json!(t("open.terminal_running"));
            } else if open_in_terminal(v["command"].as_str().unwrap_or(""), v["cwd"].as_str()).is_ok() {
                v["kind"] = json!("note");
                v["note"] = json!(t("open.terminal_opened"));
            }
        }
        Ok(v)
    })
    .await
}

/// Where `dispatch install` puts the command line.
fn installed_cli() -> PathBuf {
    model::home_dir().join(".dispatch").join("bin").join(if cfg!(windows) { "dispatch.exe" } else { "dispatch" })
}

/// Only the sidecar; never use the installed copy as the source of an update.
fn bundled_cli() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let ext = if cfg!(windows) { ".exe" } else { "" };
            for name in ["dispatch", "dispatch-aarch64-apple-darwin", "dispatch-x86_64-apple-darwin", "dispatch-x86_64-unknown-linux-gnu", "dispatch-aarch64-unknown-linux-gnu", "dispatch-x86_64-pc-windows-msvc", "dispatch-aarch64-pc-windows-msvc"] {
                let p = dir.join(format!("{name}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn cli_path() -> Option<PathBuf> {
    bundled_cli().or_else(|| {
        let installed = installed_cli();
        installed.is_file().then_some(installed)
    })
}

#[tauri::command]
async fn setup_status(app: AppHandle) -> Result<Value, String> {
    blocking(move || {
        let sync_error = app.state::<cli_sync::State>().ensure(bundled_cli().as_deref(), &installed_cli());
        let home = model::home_dir();
        let bin = installed_cli();
        let plugins = home.join(".claude").join("plugins");
        let plugin_installed = std::fs::read_to_string(plugins.join("installed_plugins.json")).map(|t| t.contains("dispatch@")).unwrap_or(false)
            || std::fs::read_dir(&plugins).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().contains("dispatch"))).unwrap_or(false);
        let codex = dispatch_core::codex::state();
        let grok = dispatch_core::grok::state();
        Ok(json!({
            "hook_installed": hook::installed(),
            "binary": bin.is_file().then(|| bin.to_string_lossy().to_string()),
            "cli_sync_error": sync_error.map(|e| t("app.cli_sync_failed").replace("{e}", &e)),
            "bundled_cli": cli_path().map(|p| p.to_string_lossy().to_string()),
            "codex": dispatch_core::codex::executable().map(|p| p.to_string_lossy().to_string()),
            "codex_daemon": codex.daemon,
            "codex_loaded": codex.loaded.len(),
            "grok_app": grok.app,
            "grok_signed_in": grok.signed_in,
            "gbot": grok.cli.map(|p| p.to_string_lossy().to_string()),
            "grok_bots": grok.bots.len(),
            "grok_error": grok.error,
            "store_root": Store::open().root().to_string_lossy(),
            "claude_plugin": plugin_installed,
            "notify": settings::notify_enabled(),
            "theme": theme_setting(),
            "inbound": dispatch_core::claude::inbound_setting(),
            "review": review::mode(),
            "review_trusted": review::trusted_count(&Store::open()),
            "version": env!("CARGO_PKG_VERSION"),
        }))
    })
    .await
}

/// One click: put the bundled CLI at ~/.dispatch/bin, install the Claude Code hook from there,
/// register the MCP server with Codex. Runs the CLI's own `install`, so the two never drift.
#[tauri::command]
async fn install_cli(app: AppHandle) -> Result<String, String> {
    blocking(move || {
        app.state::<cli_sync::State>().ensure(bundled_cli().as_deref(), &installed_cli());
        let cli = cli_path().ok_or_else(|| t("app.no_bundled_cli").to_string())?;
        let out = std::process::Command::new(&cli).arg("install").output().map_err(|e| e.to_string())?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() {
            app.state::<cli_sync::State>().installed();
            Ok(text)
        } else {
            Err(text)
        }
    })
    .await
}

/// Install the Claude Code hook alone, pointing at the installed command line.
#[tauri::command]
async fn install_hook() -> Result<String, String> {
    blocking(|| {
        let bin = installed_cli();
        if !bin.is_file() {
            return Err(t("app.cli_missing").into());
        }
        hook::install(&bin.to_string_lossy()).map_err(err)?;
        Ok(t("cli.hook_done").replace("{bin}", &bin.display().to_string()))
    })
    .await
}

fn updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    use tauri_plugin_updater::UpdaterExt;
    let mut b = app.updater_builder();
    // DISPATCH_UPDATE_URL points a build at a local latest.json while testing the flow.
    if let Ok(u) = std::env::var("DISPATCH_UPDATE_URL") {
        let url = u.parse().map_err(|e| format!("DISPATCH_UPDATE_URL：{e}"))?;
        b = b.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    b.build().map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_check(app: tauri::AppHandle) -> Result<Value, String> {
    let current = app.package_info().version.to_string();
    match updater(&app)?.check().await {
        Ok(Some(u)) => Ok(json!({ "available": true, "current": current, "version": u.version, "notes": u.body, "date": u.date.map(|d| d.to_string()) })),
        Ok(None) => Ok(json!({ "available": false, "current": current })),
        Err(e) => Err(t("app.update_check_failed").replace("{e}", &e.to_string())),
    }
}

#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    let Some(u) = updater(&app)?.check().await.map_err(|e| e.to_string())? else {
        return Err(t("app.no_update").into());
    };
    u.download_and_install(|_, _| {}, || {}).await.map_err(|e| t("app.update_failed").replace("{e}", &e.to_string()))?;
    app.restart();
}

// ---- the window, the menu bar item, the watcher ----

/// Bring the main window back, whether it was hidden, minimized or just behind.
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Bring up the review window: above every other window and on every desktop, and, unless
/// the user asked for it, without taking the keyboard from what they are typing in.
fn show_review(app: &AppHandle, focus: bool) {
    if let Some(w) = app.get_webview_window(REVIEW_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        if focus {
            let _ = w.set_focus();
        }
        return;
    }
    let builder = tauri::WebviewWindowBuilder::new(app, REVIEW_WINDOW, tauri::WebviewUrl::App("index.html#review".into()))
        .title(t("review.window_title"))
        .inner_size(480.0, 600.0)
        .min_inner_size(400.0, 440.0)
        .center()
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .focused(focus)
        .theme(theme_now());
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    let _ = builder.build();
}

/// Every second: tell senders the app is up, and bring the review window up when a held copy
/// is waiting that the user has not put away.
fn start_review_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("dispatch-review".into())
        .spawn(move || {
            let mut last: Vec<String> = vec![];
            loop {
                let store = Store::open();
                review::touch_app(&store);
                let held = review::queued(&store);
                if held != last {
                    let _ = app.emit("review:changed", ());
                    // forget what was put away once it is decided, withdrawn or expired
                    if let Ok(mut d) = app.state::<Reviews>().dismissed.lock() {
                        d.retain(|id| held.contains(id));
                    }
                }
                let fresh = app.state::<Reviews>().dismissed.lock().map(|d| held.iter().any(|id| !d.contains(id))).unwrap_or(true);
                let visible = app.get_webview_window(REVIEW_WINDOW).map(|w| w.is_visible().unwrap_or(false)).unwrap_or(false);
                if fresh && !visible {
                    show_review(&app, false);
                }
                last = held;
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn the review watcher");
}

/// The menu bar menu: the counts, the recent dispatches (each opens the app on that one),
/// Open, Send, Quit. Built on the main thread, where the system wants its menus made.
fn tray_menu(app: &AppHandle, sum: &watch::Summary) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "counts", watch::headline(sum), false, None::<&str>)?)?;
    if !sum.recent.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for r in &sum.recent {
            menu.append(&MenuItem::with_id(app, format!("d:{}", r.dispatch_id), &r.line, true, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "open", t("tray.open"), true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "compose", t("tray.compose"), true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&PredefinedMenuItem::quit(app, Some(t("tray.quit")))?)?;
    Ok(menu)
}

fn on_tray_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "compose" => {
            show_main(app);
            let _ = app.emit("tray:compose", ());
        }
        other => {
            if let Some(did) = other.strip_prefix("d:") {
                show_main(app);
                let _ = app.emit("tray:select", did.to_string());
            }
        }
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // macOS: the bare mark as a template image (the system tints it). Elsewhere the app icon,
    // since a black glyph would vanish on a dark taskbar.
    #[cfg(target_os = "macos")]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let menu = tray_menu(app, &watch::Summary::default())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Dispatch")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| on_tray_menu(app, ev.id().as_ref()))
        .build(app)?;
    Ok(())
}

/// Put the counts beside the icon and the recent list in the menu.
fn refresh_tray(app: &AppHandle, sum: watch::Summary) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = app2.tray_by_id(TRAY_ID) {
            if let Ok(menu) = tray_menu(&app2, &sum) {
                let _ = tray.set_menu(Some(menu));
            }
            let _ = tray.set_title(watch::title(&sum));
        }
    });
}

fn notify(app: &AppHandle, e: &watch::Event) {
    let (title, body) = watch::notification(e);
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Read the feed every few seconds (less often while the window is hidden); keep the tray,
/// the notifications and the window's cached feed current.
fn start_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("dispatch-watch".into())
        .spawn(move || {
            let mut prev: Option<watch::Snapshot> = None;
            let mut shown: Option<(&'static str, watch::Summary)> = None;
            let mut tick: u32 = 0;
            loop {
                // the postman's round every half minute: expire overdue copies, retry queued ones whose route opened
                if tick % 6 == 0 {
                    let _ = ops::sweep(&Store::open());
                }
                tick = tick.wrapping_add(1);
                if let Ok(feed) = ops::feed(&Store::open(), 200) {
                    let (snap, events) = watch::events(prev.as_ref(), &feed);
                    let changed = prev.as_ref() != Some(&snap);
                    prev = Some(snap);
                    if let Ok(mut c) = app.state::<Cache>().feed.lock() {
                        *c = Some((Instant::now(), feed.clone()));
                    }
                    if changed {
                        let _ = app.emit("dispatch:changed", ());
                    }
                    let sum = watch::summary(&feed, RECENT_IN_TRAY);
                    let lang = i18n::lang().code();
                    if shown.as_ref().map(|(l, s)| *l != lang || *s != sum).unwrap_or(true) {
                        refresh_tray(&app, sum.clone());
                        shown = Some((lang, sum));
                    }
                    if !events.is_empty() && settings::notify_enabled() {
                        for e in &events {
                            notify(&app, e);
                        }
                    }
                }
                let visible = app.get_webview_window("main").map(|w| w.is_visible().unwrap_or(false)).unwrap_or(false);
                std::thread::sleep(Duration::from_secs(if visible { 5 } else { 15 }));
            }
        })
        .expect("spawn the watcher");
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Cache::default())
        .manage(Reviews::default())
        .manage(cli_sync::State::default())
        .invoke_handler(tauri::generate_handler![
            sessions, find, feed, threads, dispatch_status, send, reply, open_session, setup_status, install_cli, install_hook, update_check, update_install, get_lang, set_lang, set_notify, set_theme, retry, cancel,
            peers_overview, peer_scan, peer_set_name, peer_code, peer_pair, peer_remove, peer_set_address, contact_set, contact_remove, search_sessions, harness_icons, session_page, set_inbound, open_local_network_settings,
            peer_link, peer_verify, peer_rule, set_peer_internet, set_peer_relay, review_list, review_info, review_decide, review_close, review_open, review_settings, set_review, review_forget
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                handle.state::<cli_sync::State>().ensure(bundled_cli().as_deref(), &installed_cli());
            });
            apply_theme(app.handle());
            build_tray(app.handle())?;
            start_listener();
            start_watcher(app.handle().clone());
            start_review_watcher(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the menu bar; Quit is in the tray menu and ⌘Q.
            // Closing the review window puts away what it holds until something new comes.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == REVIEW_WINDOW {
                    put_away(window.app_handle(), review::queued(&Store::open()));
                }
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Dispatch");
    app.run(|app, event| {
        // The Dock icon: bring the window back, whether it was hidden, minimized or just behind.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, event);
        }
    });
}
