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

use dispatch_core::i18n::{self, t};
use dispatch_core::{hook, model, ops, settings, store::Store, watch};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

const TRAY_ID: &str = "main";
const RECENT_IN_TRAY: usize = 6;

/// The watcher's latest feed, so the window's polls do not read everything again.
#[derive(Default)]
struct Cache {
    feed: Mutex<Option<(Instant, Value)>>,
}

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
async fn send(app: AppHandle, to: Vec<String>, text: String, urgency: Option<String>, allow_resume: Option<bool>) -> Result<Value, String> {
    let urgency = urgency.unwrap_or_else(|| "normal".into());
    let r = blocking(move || ops::send_many_opts(&Store::open(), &to, &text, &urgency, Some(("app".into(), t("me").into())), allow_resume.unwrap_or(false)).map_err(err)).await;
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

/// System notifications on or off (replies and failed deliveries).
#[tauri::command]
async fn set_notify(on: bool) -> Result<bool, String> {
    blocking(move || {
        settings::set("notify", Value::Bool(on)).map_err(|e| e.to_string())?;
        Ok(on)
    })
    .await
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

/// Open the session: a claude:// or codex:// link goes to the system; a Grok bot brings its
/// app forward; a terminal session comes back as a command for the user to run.
#[tauri::command]
async fn open_session(id: String) -> Result<Value, String> {
    blocking(move || {
        let v = ops::open_session(&id).map_err(err)?;
        if v["kind"] == "url" {
            open_url(v["url"].as_str().unwrap_or(""))?;
        } else if v["kind"] == "app" {
            open_app(v["app"].as_str().unwrap_or(""))?;
        }
        Ok(v)
    })
    .await
}

/// The `dispatch` command line: the sidecar inside this app bundle, else an installed copy.
fn cli_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["dispatch", "dispatch-aarch64-apple-darwin", "dispatch-x86_64-apple-darwin"] {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let installed = model::home_dir().join(".dispatch").join("bin").join("dispatch");
    installed.is_file().then_some(installed)
}

#[tauri::command]
async fn setup_status() -> Result<Value, String> {
    blocking(|| {
        let home = model::home_dir();
        let bin = home.join(".dispatch").join("bin").join("dispatch");
        let plugins = home.join(".claude").join("plugins");
        let plugin_installed = std::fs::read_to_string(plugins.join("installed_plugins.json")).map(|t| t.contains("dispatch@")).unwrap_or(false)
            || std::fs::read_dir(&plugins).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().contains("dispatch"))).unwrap_or(false);
        let codex = dispatch_core::codex::state();
        let grok = dispatch_core::grok::state();
        Ok(json!({
            "hook_installed": hook::installed(),
            "binary": bin.is_file().then(|| bin.to_string_lossy().to_string()),
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
            "version": env!("CARGO_PKG_VERSION"),
        }))
    })
    .await
}

/// One click: put the bundled CLI at ~/.dispatch/bin, install the Claude Code hook from there,
/// register the MCP server with Codex. Runs the CLI's own `install`, so the two never drift.
#[tauri::command]
async fn install_cli() -> Result<String, String> {
    blocking(|| {
        let cli = cli_path().ok_or_else(|| t("app.no_bundled_cli").to_string())?;
        let out = std::process::Command::new(&cli).arg("install").output().map_err(|e| e.to_string())?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() {
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
        let bin = model::home_dir().join(".dispatch").join("bin").join("dispatch");
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
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    let menu = tray_menu(app, &watch::Summary::default())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
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
            loop {
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
        .invoke_handler(tauri::generate_handler![sessions, find, feed, dispatch_status, send, reply, open_session, setup_status, install_cli, install_hook, update_check, update_install, get_lang, set_lang, set_notify])
        .setup(|app| {
            build_tray(app.handle())?;
            start_watcher(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the menu bar; Quit is in the tray menu and ⌘Q.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
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
