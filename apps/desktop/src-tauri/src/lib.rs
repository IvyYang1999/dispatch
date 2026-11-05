//! The desktop shell: thin Tauri commands over dispatch-core.
//!
//! Every command is async and does its work on the blocking pool: the core reads
//! files and runs `ps`, and a sync command would do that on the main thread,
//! where it freezes clicks, drags and even the Dock's activate.

use dispatch_core::{hook, model, ops, store::Store};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

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

#[tauri::command]
async fn feed(limit: Option<usize>) -> Result<Value, String> {
    blocking(move || ops::feed(&Store::open(), limit.unwrap_or(200)).map_err(err)).await
}

#[tauri::command]
async fn dispatch_status(id: String) -> Result<Value, String> {
    blocking(move || ops::status(&Store::open(), &id).map_err(err)).await
}

#[tauri::command]
async fn send(to: Vec<String>, text: String, urgency: Option<String>, allow_resume: Option<bool>) -> Result<Value, String> {
    let urgency = urgency.unwrap_or_else(|| "普通".into());
    blocking(move || ops::send_many_opts(&Store::open(), &to, &text, &urgency, Some(("app".into(), "我".into())), allow_resume.unwrap_or(false)).map_err(err)).await
}

#[tauri::command]
async fn reply(id: String, text: String) -> Result<Value, String> {
    blocking(move || ops::reply(&Store::open(), &id, &text).map_err(err)).await
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
        Ok(s) => Err(format!("打不开 {url}（退出码 {s}）")),
        Err(e) => Err(format!("打不开 {url}：{e}")),
    }
}

/// Open the session: a claude:// or codex:// link goes to the system; a terminal session
/// comes back as a command for the user to run.
#[tauri::command]
async fn open_session(id: String) -> Result<Value, String> {
    blocking(move || {
        let v = ops::open_session(&id).map_err(err)?;
        if v["kind"] == "url" {
            open_url(v["url"].as_str().unwrap_or(""))?;
        }
        Ok(v)
    })
    .await
}

fn which(cmd: &str) -> Option<String> {
    let out = std::process::Command::new("sh").args(["-lc", &format!("command -v {cmd}")]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
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
        Ok(json!({
            "hook_installed": hook::installed(),
            "binary": bin.is_file().then(|| bin.to_string_lossy().to_string()),
            "bundled_cli": cli_path().map(|p| p.to_string_lossy().to_string()),
            "codex": which("codex"),
            "codex_daemon": codex.daemon,
            "codex_loaded": codex.loaded.len(),
            "store_root": Store::open().root().to_string_lossy(),
            "claude_plugin": plugin_installed,
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
        let cli = cli_path().ok_or("这个 App 包里没有 dispatch 命令行（开发模式：先 cargo build --release -p dispatch，再把它放到 src-tauri/binaries/）")?;
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
            return Err("命令行还没装，先点「一键安装」。".into());
        }
        hook::install(&bin.to_string_lossy()).map_err(err)?;
        Ok(format!("已装：UserPromptSubmit / Stop / SessionStart → {} hook run", bin.display()))
    })
    .await
}

fn updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
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
        Err(e) => Err(format!("检查更新失败：{e}")),
    }
}

#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    let Some(u) = updater(&app)?.check().await.map_err(|e| e.to_string())? else {
        return Err("没有可用的更新".into());
    };
    u.download_and_install(|_, _| {}, || {}).await.map_err(|e| format!("更新失败：{e}"))?;
    app.restart();
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![sessions, find, feed, dispatch_status, send, reply, open_session, setup_status, install_cli, install_hook, update_check, update_install])
        .build(tauri::generate_context!())
        .expect("error while building Dispatch");
    app.run(|app, event| {
        // The Dock icon: bring the window back, whether it was hidden, minimized or just behind.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, event);
        }
    });
}
