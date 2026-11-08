//! The settings file: `settings.json` in the store folder. The app writes it; the
//! command line and the hook read it, so one switch covers all three.
//!
//! Keys: `lang` (`en` / `zh`), `notify` (system notifications for replies and failed
//! deliveries; default on).

use serde_json::{Map, Value};
use std::path::PathBuf;

pub fn path() -> PathBuf {
    crate::store::Store::open().root().join("settings.json")
}

pub fn read() -> Map<String, Value> {
    read_at(&path())
}

pub fn read_at(path: &std::path::Path) -> Map<String, Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

pub fn get(key: &str) -> Option<Value> {
    read().get(key).cloned()
}

/// Write one key, keeping the others.
pub fn set(key: &str, value: Value) -> std::io::Result<()> {
    set_at(&path(), key, value)
}

pub fn set_at(path: &std::path::Path, key: &str, value: Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut m = read_at(path);
    m.insert(key.into(), value);
    std::fs::write(path, serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default() + "\n")
}

/// Whether the app posts system notifications (a reply came in, a delivery failed).
pub fn notify_enabled() -> bool {
    get("notify").and_then(|v| v.as_bool()).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("dispatch-settings-{}", uuid::Uuid::new_v4()));
        let file = dir.join("settings.json");
        set_at(&file, "lang", Value::String("zh".into())).unwrap();
        set_at(&file, "notify", Value::Bool(false)).unwrap();
        let m = read_at(&file);
        assert_eq!(m["lang"], "zh");
        assert_eq!(m["notify"], false);
        assert_eq!(read_at(&dir.join("missing.json")).len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
