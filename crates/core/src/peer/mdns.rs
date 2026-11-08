//! Finding peers on the local network: advertise `_dispatch._tcp`, browse for others, and
//! keep what was seen in `peer/seen.json` so a short-lived command can read it without a
//! scan of its own. `peer/serving.json` is the listener's heartbeat: while it is fresh, the
//! seen list is live; otherwise a stale list is refreshed by a short browse.

use super::{me, state_dir, SERVICE};
use crate::model::short_id;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Seen {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    /// Unix seconds.
    pub at: i64,
}

impl Seen {
    /// IPv4 first: a link-local IPv6 address needs a scope this side may not have.
    pub fn socket_addrs(&self) -> Vec<SocketAddr> {
        let mut v: Vec<&IpAddr> = self.addrs.iter().collect();
        v.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
        v.into_iter().map(|a| SocketAddr::new(*a, self.port)).collect()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SeenFile {
    /// When the list was last refreshed by a browse (unix seconds).
    #[serde(default)]
    scanned_at: i64,
    #[serde(default)]
    peers: HashMap<String, Seen>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Serving {
    port: u16,
    pid: u32,
    mdns: bool,
    at: i64,
}

fn seen_path() -> PathBuf {
    state_dir().join("seen.json")
}

fn serving_path() -> PathBuf {
    state_dir().join("serving.json")
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn read_seen() -> SeenFile {
    std::fs::read_to_string(seen_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_seen(f: &SeenFile) {
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(seen_path(), serde_json::to_string_pretty(f).unwrap_or_default());
}

pub(crate) fn note_serving(port: u16, mdns: bool) {
    let _ = std::fs::create_dir_all(state_dir());
    let s = Serving { port, pid: std::process::id(), mdns, at: now_secs() };
    let _ = std::fs::write(serving_path(), serde_json::to_string(&s).unwrap_or_default());
}

/// The port a listener in some process on this machine is on, when its heartbeat is fresh.
pub fn serving() -> Option<u16> {
    let s: Serving = serde_json::from_str(&std::fs::read_to_string(serving_path()).ok()?).ok()?;
    (now_secs() - s.at < 90).then_some(s.port)
}

fn upsert(r: &mdns_sd::ResolvedService, mine: &str) {
    let id = r.txt_properties.get_property_val_str("id").map(str::to_string).unwrap_or_else(|| r.fullname.split('.').next().unwrap_or("").to_string());
    if id.is_empty() || id == mine {
        return;
    }
    let name = r.txt_properties.get_property_val_str("name").unwrap_or("").to_string();
    let addrs: Vec<IpAddr> = r.addresses.iter().map(|a| a.to_ip_addr()).filter(|a| !a.is_unspecified()).collect();
    if addrs.is_empty() {
        return;
    }
    let mut f = read_seen();
    f.scanned_at = now_secs();
    f.peers.insert(id.clone(), Seen { id, name, port: r.port, addrs, at: now_secs() });
    write_seen(&f);
}

fn remove(fullname: &str) {
    let id = fullname.split('.').next().unwrap_or("");
    let mut f = read_seen();
    if f.peers.remove(id).is_some() {
        f.scanned_at = now_secs();
        write_seen(&f);
    }
}

/// Advertise this machine's listener and keep watching for others, in background threads.
pub(crate) fn start(port: u16) -> anyhow::Result<()> {
    let me = me()?;
    let daemon = ServiceDaemon::new()?;
    let mut props = HashMap::new();
    props.insert("id".to_string(), me.id.clone());
    props.insert("name".to_string(), me.name.clone());
    props.insert("v".to_string(), "1".to_string());
    let host = format!("dispatch-{}.local.", short_id(&me.id));
    let info = ServiceInfo::new(SERVICE, &me.id, &host, "", port, props)?.enable_addr_auto();
    daemon.register(info)?;
    let rx = daemon.browse(SERVICE)?;
    let mine = me.id.clone();
    note_serving(port, true);
    std::thread::Builder::new().name("dispatch-mdns".into()).spawn(move || {
        // the daemon lives as long as this thread
        let _daemon = daemon;
        while let Ok(ev) = rx.recv() {
            match ev {
                ServiceEvent::ServiceResolved(r) => upsert(&r, &mine),
                ServiceEvent::ServiceRemoved(_, fullname) => remove(&fullname),
                _ => {}
            }
        }
    })?;
    std::thread::Builder::new().name("dispatch-peer-heartbeat".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        note_serving(port, true);
    })?;
    Ok(())
}

/// Browse for `wait`, merging what shows up into the seen list.
pub fn scan(wait: Duration) -> Vec<Seen> {
    let mine = match me() { Ok(m) => m.id, Err(e) => { super::log_problem("storage", &format!("{e:#}")); return vec![]; } };
    if let Ok(daemon) = ServiceDaemon::new() {
        if let Ok(rx) = daemon.browse(SERVICE) {
            let end = Instant::now() + wait;
            loop {
                let now = Instant::now();
                if now >= end {
                    break;
                }
                match rx.recv_timeout(end - now) {
                    Ok(ServiceEvent::ServiceResolved(r)) => upsert(&r, &mine),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            let _ = daemon.shutdown();
        }
    }
    let mut f = read_seen();
    f.scanned_at = now_secs();
    write_seen(&f);
    f.peers.into_values().collect()
}

/// Everything seen on the network. Live while a listener runs here; otherwise a list older
/// than a minute is refreshed by a short browse first.
pub fn seen_all() -> Vec<Seen> {
    let f = read_seen();
    let fresh = serving().is_some() || now_secs() - f.scanned_at < 60;
    if !fresh {
        return scan(Duration::from_millis(1500));
    }
    f.peers.into_values().collect()
}

pub fn seen(id: &str) -> Option<Seen> {
    seen_all().into_iter().find(|s| s.id == id)
}

pub fn seen_by_name(name: &str) -> Option<Seen> {
    let q = name.trim();
    seen_all().into_iter().find(|s| s.name.eq_ignore_ascii_case(q))
}
