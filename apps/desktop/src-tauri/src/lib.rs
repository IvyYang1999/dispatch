//! The desktop shell: thin Tauri commands over dispatch-core.

use dispatch_core::{hook, model, ops, store::Store};
use serde_json::{json, Value};

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn sessions(running_only: bool) -> Value {
    ops::sessions(running_only)
}

#[tauri::command]
fn find(query: String) -> Value {
    ops::find(&query)
}

#[tauri::command]
fn feed(limit: Option<usize>) -> Result<Value, String> {
    ops::feed(&Store::open(), limit.unwrap_or(200)).map_err(err)
}

#[tauri::command]
fn dispatch_status(id: String) -> Result<Value, String> {
    ops::status(&Store::open(), &id).map_err(err)
}

#[tauri::command]
fn send(to: Vec<String>, text: String, urgency: Option<String>) -> Result<Value, String> {
    let urgency = urgency.unwrap_or_else(|| "普通".into());
    ops::send_many(&Store::open(), &to, &text, &urgency, Some(("app".into(), "我".into()))).map_err(err)
}

#[tauri::command]
fn reply(id: String, text: String) -> Result<Value, String> {
    ops::reply(&Store::open(), &id, &text).map_err(err)
}

/// Open the session: a claude:// or codex:// link goes to the system; a terminal session
/// comes back as a command for the user to run.
#[tauri::command]
fn open_session(id: String) -> Result<Value, String> {
    let v = ops::open_session(&id).map_err(err)?;
    if v["kind"] == "url" {
        let url = v["url"].as_str().unwrap_or("").to_string();
        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("open").arg(&url).status();
        #[cfg(target_os = "linux")]
        let status = std::process::Command::new("xdg-open").arg(&url).status();
        #[cfg(target_os = "windows")]
        let status = std::process::Command::new("cmd").args(["/C", "start", "", &url]).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(format!("打不开 {url}（退出码 {s}）")),
            Err(e) => return Err(format!("打不开 {url}：{e}")),
        }
    }
    Ok(v)
}

fn which(cmd: &str) -> Option<String> {
    let out = std::process::Command::new("sh").args(["-lc", &format!("command -v {cmd}")]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[tauri::command]
fn setup_status() -> Value {
    let home = model::home_dir();
    let bin = home.join(".dispatch").join("bin").join("dispatch");
    let claude_plugin = home.join(".claude").join("plugins");
    let plugin_installed = std::fs::read_dir(&claude_plugin)
        .map(|rd| rd.flatten().any(|e| e.path().join("dispatch").exists() || e.file_name().to_string_lossy().contains("dispatch")))
        .unwrap_or(false)
        || std::fs::read_to_string(home.join(".claude").join("plugins").join("installed_plugins.json")).map(|t| t.contains("dispatch@")).unwrap_or(false);
    json!({
        "hook_installed": hook::installed(),
        "binary": bin.is_file().then(|| bin.to_string_lossy().to_string()),
        "codex": which("codex"),
        "store_root": Store::open().root().to_string_lossy(),
        "claude_plugin": plugin_installed,
    })
}

/// Install the Claude Code hook, pointing at the command-line binary when it is installed,
/// else at this app's own executable (which also answers `hook run` through the CLI crate? no:
/// the app is not the CLI, so we insist on the CLI being there).
#[tauri::command]
fn install_hook() -> Result<String, String> {
    let bin = model::home_dir().join(".dispatch").join("bin").join("dispatch");
    if !bin.is_file() {
        return Err("先在终端运行一次 `dispatch install`，把命令行装到 ~/.dispatch/bin，hook 要靠它。".into());
    }
    hook::install(&bin.to_string_lossy()).map_err(err)?;
    Ok(format!("已装：UserPromptSubmit / Stop / SessionStart → {} hook run", bin.display()))
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![sessions, find, feed, dispatch_status, send, reply, open_session, setup_status, install_hook])
        .run(tauri::generate_context!())
        .expect("error while running Dispatch");
}
