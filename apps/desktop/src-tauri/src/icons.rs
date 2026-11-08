//! The clients' own icons for the harness marks, taken from the apps installed on this machine
//! at run time (Dispatch ships none of them), converted once to small PNGs in the store folder
//! and handed to the window as data URLs. A client that is not installed gets a text tag.

use serde_json::{json, Value};
use std::path::Path;

#[cfg(target_os = "macos")]
const APPS: [(&str, &str); 3] = [("claude", "Claude"), ("codex", "Codex"), ("grok", "Grok Bot")];

pub fn harness_icons(store_root: &Path) -> Value {
    let mut v = json!({});
    #[cfg(target_os = "macos")]
    {
        let dir = store_root.join("icons");
        let _ = std::fs::create_dir_all(&dir);
        for (key, app) in APPS {
            let Some(bundle) = app_bundle(app) else { continue };
            let out = dir.join(format!("{key}.png"));
            if !fresh(&out, &bundle) {
                let _ = std::fs::remove_file(&out);
                render(&bundle, &out);
            }
            if let Ok(bytes) = std::fs::read(&out) {
                v[key] = json!(format!("data:image/png;base64,{}", base64(&bytes)));
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = store_root;
    }
    v
}

/// `/Applications/<name>.app`, `~/Applications/<name>.app`, else wherever Spotlight has it.
#[cfg(target_os = "macos")]
fn app_bundle(name: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let home = dispatch_core::model::home_dir();
    for dir in [PathBuf::from("/Applications"), home.join("Applications")] {
        let p = dir.join(format!("{name}.app"));
        if p.join("Contents/Info.plist").is_file() {
            return Some(p);
        }
    }
    let out = std::process::Command::new("mdfind").args(["-onlyin", "/", &format!("kMDItemKind == 'Application' && kMDItemDisplayName == '{name}'")]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).find(|p| p.extension().map(|e| e == "app").unwrap_or(false) && p.join("Contents/Info.plist").is_file())
}

/// Good for a week, and only while the app has not changed since.
#[cfg(target_os = "macos")]
fn fresh(out: &Path, bundle: &Path) -> bool {
    let Ok(md) = std::fs::metadata(out) else { return false };
    let made = md.modified().ok();
    let app = std::fs::metadata(bundle.join("Contents/Info.plist")).ok().and_then(|m| m.modified().ok());
    match (made, app) {
        (Some(m), Some(a)) => m > a && m.elapsed().map(|e| e.as_secs() < 7 * 86_400).unwrap_or(false),
        _ => false,
    }
}

/// The icns the bundle names (or any icns in its resources), scaled to 64 px with `sips`.
#[cfg(target_os = "macos")]
fn render(bundle: &Path, out: &Path) {
    let resources = bundle.join("Contents/Resources");
    let plist = bundle.join("Contents/Info.plist");
    let named = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleIconFile", &plist.to_string_lossy()])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|n| resources.join(if n.ends_with(".icns") { n } else { format!("{n}.icns") }))
        .filter(|p| p.is_file());
    let any = || std::fs::read_dir(&resources).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().map(|e| e == "icns").unwrap_or(false));
    let Some(icns) = named.or_else(any) else { return };
    let _ = std::process::Command::new("sips").args(["-s", "format", "png", "-Z", "64"]).arg(&icns).arg("--out").arg(out).output();
}

#[cfg(target_os = "macos")]
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}
