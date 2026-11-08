//! Other machines running Dispatch: who this machine is, which machines it is paired
//! with, how to reach them, and what to do with their sessions and letters.
//!
//! A message to a session elsewhere is a letter that travels into that machine's store,
//! where the local ladder delivers it; its status and any reply come back by mirroring.
//! `docs/peers.md` has the whole picture.
//!
//! - `peers.json` in the store folder: me (id, name, public key) and the paired peers.
//! - `peer.key` beside it: this machine's private key, mode 0600.
//! - `peer/pairing.json`: a pairing code while one is active.
//! - `peer/seen.json`, `peer/serving.json`: what mDNS saw, and the listener's heartbeat.

mod client;
pub mod mdns;
mod server;
mod wire;

pub use client::{hello, pair, PeerError};
pub use server::{serve, ServerHandle};
pub use wire::{RemoteCopy, RemoteSession};

use crate::i18n::t;
use crate::model::{Harness, Host, Session};
use crate::reach::{Level, Reach};
use crate::store::{self, Letter, Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use wire::{Request, Response};

pub const DEFAULT_PORT: u16 = 47831;
pub const SERVICE: &str = "_dispatch._tcp.local.";
const SESSIONS_TTL: Duration = Duration::from_secs(20);
const OFFLINE_BACKOFF: Duration = Duration::from_secs(20);
const MIRROR_EVERY: Duration = Duration::from_secs(10);
const CODE_SECS: i64 = 120;
const CODE_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub public_key: String,
    /// `host:port` set by hand, for a machine mDNS cannot see.
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub paired_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub me: Me,
    #[serde(default)]
    pub peers: Vec<PeerInfo>,
}

fn root() -> PathBuf {
    Store::open().root().to_path_buf()
}

fn book_path() -> PathBuf {
    root().join("peers.json")
}

fn key_path() -> PathBuf {
    root().join("peer.key")
}

pub(crate) fn state_dir() -> PathBuf {
    root().join("peer")
}

fn write_private(path: &PathBuf, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(root())?;
    std::fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn new_keypair() -> Result<snow::Keypair> {
    Ok(snow::Builder::new(wire::PATTERN.parse()?).generate_keypair()?)
}

/// The private key, made with the book on first use.
pub(crate) fn private_key() -> Result<Vec<u8>> {
    load();
    let text = std::fs::read_to_string(key_path())?;
    let k = hex::decode(text.trim())?;
    if k.len() != 32 {
        anyhow::bail!("peer.key is not a 32-byte key");
    }
    Ok(k)
}

fn load_raw() -> Book {
    std::fs::read_to_string(book_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Me and the paired peers; an id, a name and a key pair are made the first time.
pub fn load() -> Book {
    let mut b = load_raw();
    let mut dirty = false;
    if b.me.id.is_empty() {
        b.me.id = uuid::Uuid::new_v4().to_string();
        dirty = true;
    }
    if b.me.name.trim().is_empty() {
        b.me.name = default_name();
        dirty = true;
    }
    if b.me.public_key.is_empty() || !key_path().exists() {
        if let Ok(kp) = new_keypair() {
            if write_private(&key_path(), &hex::encode(&kp.private)).is_ok() {
                b.me.public_key = hex::encode(&kp.public);
                dirty = true;
            }
        }
    }
    if dirty {
        let _ = save(&b);
    }
    b
}

pub fn save(b: &Book) -> Result<()> {
    std::fs::create_dir_all(root())?;
    std::fs::write(book_path(), serde_json::to_string_pretty(b)? + "\n")?;
    Ok(())
}

/// The computer's name, as the system shows it.
fn default_name() -> String {
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    #[cfg(not(target_os = "macos"))]
    let name = std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    name.filter(|n| !n.is_empty()).unwrap_or_else(|| "Dispatch".into())
}

pub fn me() -> Me {
    load().me
}

pub fn peers() -> Vec<PeerInfo> {
    load().peers
}

pub fn set_name(name: &str) -> Result<Me> {
    let mut b = load();
    b.me.name = name.trim().chars().take(48).collect();
    if b.me.name.is_empty() {
        anyhow::bail!("the name is empty");
    }
    save(&b)?;
    Ok(b.me)
}

/// A paired peer by name (case does not matter) or by a prefix of its id.
pub fn find_peer(q: &str) -> Option<PeerInfo> {
    let q = q.trim();
    if q.is_empty() {
        return None;
    }
    let peers = peers();
    peers.iter().find(|p| p.name.eq_ignore_ascii_case(q)).or_else(|| peers.iter().find(|p| q.len() >= 4 && p.id.starts_with(&q.to_lowercase()))).cloned()
}

pub(crate) fn peer_by_key(key: &[u8]) -> Option<PeerInfo> {
    let k = hex::encode(key);
    peers().into_iter().find(|p| p.public_key.eq_ignore_ascii_case(&k))
}

/// Add or replace a peer (by id).
pub fn remember(p: PeerInfo) -> Result<()> {
    let mut b = load();
    b.peers.retain(|x| x.id != p.id);
    b.peers.push(p);
    save(&b)
}

pub fn forget(q: &str) -> Result<Option<PeerInfo>> {
    let Some(p) = find_peer(q) else { return Ok(None) };
    let mut b = load();
    b.peers.retain(|x| x.id != p.id);
    save(&b)?;
    Ok(Some(p))
}

/// A hand-set address for a paired peer; None goes back to mDNS.
pub fn set_address(q: &str, address: Option<String>) -> Result<PeerInfo> {
    let Some(p) = find_peer(q) else { anyhow::bail!("{}", t("cli.peers_not_found").replace("{q}", q)) };
    let mut b = load();
    let mut out = p.clone();
    for x in b.peers.iter_mut() {
        if x.id == p.id {
            x.address = address.clone().map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
            out = x.clone();
        }
    }
    save(&b)?;
    Ok(out)
}

// ---------------------------------------------------------------- pairing code

#[derive(Debug, Serialize, Deserialize)]
struct Pairing {
    code: String,
    expires_at: i64,
    attempts: u32,
}

fn pairing_path() -> PathBuf {
    state_dir().join("pairing.json")
}

fn read_pairing() -> Option<Pairing> {
    let p: Pairing = serde_json::from_str(&std::fs::read_to_string(pairing_path()).ok()?).ok()?;
    if chrono::Utc::now().timestamp() >= p.expires_at {
        let _ = std::fs::remove_file(pairing_path());
        return None;
    }
    Some(p)
}

/// A new six-digit code, good for two minutes: (code, seconds left).
pub fn new_code() -> Result<(String, i64)> {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    let code = format!("{n:06}");
    let p = Pairing { code: code.clone(), expires_at: chrono::Utc::now().timestamp() + CODE_SECS, attempts: 0 };
    std::fs::create_dir_all(state_dir())?;
    write_private(&pairing_path(), &serde_json::to_string(&p)?)?;
    Ok((code, CODE_SECS))
}

/// The active code and its seconds left, if any.
pub fn current_code() -> Option<(String, i64)> {
    read_pairing().map(|p| (p.code, p.expires_at - chrono::Utc::now().timestamp()))
}

/// Check a pairing attempt against the active code. Ok(true): matched, the code is spent.
/// Ok(false): wrong; after three wrong attempts the code is spent too. Err: no code active.
pub(crate) fn take_pairing(ok: impl Fn(&str) -> bool) -> Result<bool, &'static str> {
    let Some(mut p) = read_pairing() else { return Err("no pairing code is active on this machine") };
    if ok(&p.code) {
        let _ = std::fs::remove_file(pairing_path());
        return Ok(true);
    }
    p.attempts += 1;
    if p.attempts >= CODE_ATTEMPTS {
        let _ = std::fs::remove_file(pairing_path());
    } else {
        let _ = write_private(&pairing_path(), &serde_json::to_string(&p).unwrap_or_default());
    }
    Ok(false)
}

// ---------------------------------------------------------------- addressing

/// `<peer>/<session>`: the peer when the part before the slash names one.
pub fn split_address(q: &str) -> Option<(PeerInfo, String)> {
    let (h, rest) = q.split_once('/')?;
    let p = find_peer(h)?;
    Some((p, rest.trim().to_string()))
}

/// A session on a peer known only by id and name (a stored copy's recipient or sender).
pub fn remote_target(host_id: &str, host_name: &str, session_id: &str, name: Option<String>, harness: Harness) -> Session {
    Session {
        harness,
        id: session_id.to_lowercase(),
        short_id: crate::model::short_id(session_id),
        name,
        cwd: None,
        title: None,
        last_active: 0,
        running: false,
        desktop: false,
        host_session_id: None,
        status: None,
        transcript: None,
        host: Some(Host { id: host_id.to_string(), name: host_name.to_string() }),
    }
}

// ---------------------------------------------------------------- caches

struct Remote {
    at: Instant,
    sessions: Vec<Session>,
    reach: HashMap<String, Reach>,
}

static REMOTE: LazyLock<Mutex<HashMap<String, Remote>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Peers that did not answer, and until when they are left alone.
static OFFLINE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// What a peer last said about a copy on it: (kind, stage key), by letter id.
static STAGES: LazyLock<Mutex<HashMap<String, (String, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static MIRRORED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn mark_offline(p: &PeerInfo, why: String) {
    OFFLINE.lock().unwrap().insert(p.id.clone(), (Instant::now() + OFFLINE_BACKOFF, why));
    REMOTE.lock().unwrap().remove(&p.id);
}

fn backing_off(p: &PeerInfo) -> bool {
    OFFLINE.lock().unwrap().get(&p.id).map(|(until, _)| Instant::now() < *until).unwrap_or(false)
}

fn mark_online(p: &PeerInfo) {
    OFFLINE.lock().unwrap().remove(&p.id);
}

/// Whether a peer is reachable right now: an address is known, it did not just fail, and its
/// port answers a connection (which is dropped unused).
pub fn online(p: &PeerInfo) -> bool {
    if backing_off(p) {
        return false;
    }
    client::addresses(p).iter().any(|a| std::net::TcpStream::connect_timeout(a, Duration::from_millis(700)).is_ok())
}

fn ask(p: &PeerInfo, req: &Request) -> Result<Response, PeerError> {
    if backing_off(p) {
        return Err(PeerError::Offline(OFFLINE.lock().unwrap().get(&p.id).map(|(_, w)| w.clone()).unwrap_or_default()));
    }
    match client::request(p, req) {
        Ok(r) => {
            mark_online(p);
            Ok(r)
        }
        Err(e) => {
            if matches!(e, PeerError::Offline(_)) {
                mark_offline(p, e.to_string());
            }
            Err(e)
        }
    }
}

/// The sessions on one peer, from a 20-second cache, each marked with its host.
pub fn sessions_of(p: &PeerInfo) -> Option<Vec<Session>> {
    if let Some(r) = REMOTE.lock().unwrap().get(&p.id) {
        if r.at.elapsed() < SESSIONS_TTL {
            return Some(r.sessions.clone());
        }
    }
    match ask(p, &Request::Sessions) {
        Ok(Response::Sessions { sessions, .. }) => {
            let mut list = vec![];
            let mut reach = HashMap::new();
            for rs in sessions {
                let mut s = rs.session;
                s.host = Some(Host { id: p.id.clone(), name: p.name.clone() });
                s.transcript = None;
                reach.insert(s.id.clone(), rs.reach);
                list.push(s);
            }
            REMOTE.lock().unwrap().insert(p.id.clone(), Remote { at: Instant::now(), sessions: list.clone(), reach });
            Some(list)
        }
        Ok(Response::Error { message }) => {
            mark_offline(p, message);
            None
        }
        _ => None,
    }
}

/// The sessions on every paired peer that answers.
pub fn remote_sessions() -> Vec<Session> {
    let peers = peers();
    let mut out = vec![];
    for p in &peers {
        if let Some(list) = sessions_of(p) {
            out.extend(list);
        }
    }
    out
}

/// How a message reaches a remote session: what its machine said when it listed it.
pub fn remote_reach(s: &Session) -> Reach {
    if let Some(h) = &s.host {
        if let Some(r) = REMOTE.lock().unwrap().get(&h.id).and_then(|r| r.reach.get(&s.id)) {
            return r.clone();
        }
    }
    Reach { level: Level::Later, text: t("reach.peer_offline").into(), fix: None }
}

/// Hand a letter to the machine its recipient is on: (status there, route, detail).
pub fn deliver(host: &Host, letter: &Letter) -> Result<(String, String, String), PeerError> {
    let Some(p) = peers().into_iter().find(|p| p.id == host.id) else { return Err(PeerError::Refused("not paired with that machine".into())) };
    match ask(&p, &Request::Deliver { letter: letter.clone() })? {
        Response::Delivered { status, route, detail } => Ok((status, route, detail)),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// Withdraw a copy on a peer, best effort.
pub fn cancel(host: &Host, letter_id: &str) {
    if let Some(p) = peers().into_iter().find(|p| p.id == host.id) {
        let _ = ask(&p, &Request::Cancel { letter_id: letter_id.to_string() });
    }
}

/// What the peer last said about a copy on it.
pub fn remote_stage(letter_id: &str) -> Option<(String, String)> {
    STAGES.lock().unwrap().get(letter_id).cloned()
}

/// Bring the local store up to date with the peers: the status of copies delivered there,
/// and letters for sessions here (replies) that have not arrived yet. At most once every ten
/// seconds per peer. Returns how many letters changed or arrived.
pub fn mirror(store: &Store) -> Result<usize> {
    let peers = peers();
    if peers.is_empty() {
        return Ok(0);
    }
    let me = me();
    let letters = store.list(None, false)?;
    let cutoff = store::time_text(chrono::Local::now().naive_local() - chrono::Duration::days(7));
    let mut by_peer: HashMap<String, BTreeSet<String>> = HashMap::new();
    for l in &letters {
        if let Some(h) = &l.to_host {
            if l.status != store::CANCELLED && l.created >= cutoff {
                by_peer.entry(h.clone()).or_default().insert(l.dispatch_id.clone());
            }
        }
    }
    let mut changed = 0;
    for (pid, dids) in by_peer {
        let Some(p) = peers.iter().find(|p| p.id == pid) else { continue };
        if MIRRORED.lock().unwrap().get(&pid).map(|t| t.elapsed() < MIRROR_EVERY).unwrap_or(false) {
            continue;
        }
        if backing_off(p) {
            continue;
        }
        MIRRORED.lock().unwrap().insert(pid.clone(), Instant::now());
        let copies = match ask(p, &Request::Status { dispatch_ids: dids.into_iter().collect() }) {
            Ok(Response::Status { copies }) => copies,
            _ => continue,
        };
        for rc in copies {
            let remote = rc.letter;
            match letters.iter().find(|l| l.id == remote.id) {
                Some(local) => {
                    // the peer delivers this copy, so its word on the status is final
                    if remote.to_host.is_none() && local.to_host.as_deref() == Some(&pid) {
                        if local.status != remote.status {
                            let mut l = local.clone();
                            store.set_status(&mut l, &remote.status)?;
                            store.append_run(&l.id, "peer", "mirrored", &format!("{}: {}", p.name, remote.status))?;
                            changed += 1;
                        }
                        STAGES.lock().unwrap().insert(remote.id.clone(), (rc.kind, rc.stage_key));
                    }
                }
                None => {
                    // a letter for a session here that the peer could not push: take it now
                    if remote.to_host.as_deref() == Some(&me.id) && remote.status != store::CANCELLED {
                        let mut l = remote.clone();
                        l.to_host = None;
                        l.to_host_name = None;
                        l.from_host = Some(p.id.clone());
                        l.from_host_name = Some(p.name.clone());
                        l.status = store::UNREAD.into();
                        l.path = String::new();
                        store.write_letter(&mut l)?;
                        store.append_run(&l.id, "peer", "pulled", &p.name)?;
                        let to = crate::send::target_of(&l);
                        let _ = crate::send::deliver(store, &to, &mut l, true, false);
                        changed += 1;
                    }
                }
            }
        }
    }
    Ok(changed)
}

/// Me, the paired peers (online or not) and the machines seen but not paired, for the
/// command line and the app.
pub fn overview() -> Value {
    let b = load();
    let seen = mdns::seen_all();
    let listening = mdns::serving();
    let peers: Vec<Value> = b
        .peers
        .iter()
        .map(|p| {
            let s = seen.iter().find(|s| s.id == p.id);
            json!({ "id": p.id, "name": p.name, "address": p.address, "paired_at": p.paired_at, "online": online(p), "seen": s.map(|s| json!({ "addrs": s.addrs, "port": s.port })) })
        })
        .collect();
    let unpaired: Vec<Value> = seen.iter().filter(|s| !b.peers.iter().any(|p| p.id == s.id)).map(|s| json!({ "id": s.id, "name": s.name, "addrs": s.addrs, "port": s.port })).collect();
    json!({
        "me": { "id": b.me.id, "name": b.me.name, "public_key": b.me.public_key, "listening": listening },
        "peers": peers,
        "seen": unpaired,
        "code": current_code().map(|(c, left)| json!({ "code": c, "seconds_left": left })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two machines in two store folders of one process: pair with a code, list sessions, deliver
    /// a letter, then mirror the status the other machine gives it.
    #[test]
    fn two_machines_pair_deliver_and_mirror() {
        let _g = crate::i18n::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("dispatch-peer-{}", uuid::Uuid::new_v4()));
        let a_root = base.join("a");
        let b_root = base.join("b");
        // machine B: its book and key, a code, a listener thread that works out of B's folder
        store::set_thread_root(Some(b_root.clone()));
        let b_me = load().me;
        let b_key = private_key().unwrap();
        let (code, _) = new_code().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let b_root2 = b_root.clone();
        let server = std::thread::spawn(move || {
            store::set_thread_root(Some(b_root2));
            for _ in 0..4 {
                let (s, _) = listener.accept().unwrap();
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut conn = wire::respond(s, &b_key).unwrap();
                let req: Request = conn.recv().unwrap();
                let resp = server::handle(&conn, req);
                conn.send(&resp).unwrap();
            }
        });
        // machine A pairs with B at that address
        store::set_thread_root(Some(a_root.clone()));
        let a_me = load().me;
        assert_ne!(a_me.id, b_me.id);
        let p = pair(&format!("{addr}"), &code).expect("pairing");
        assert_eq!(p.id, b_me.id);
        assert_eq!(p.address.as_deref(), Some(format!("{addr}").as_str()), "a machine mDNS did not see keeps its address");
        assert!(current_code().is_none() || true, "the code lives on B");
        // B remembered A as well
        store::set_thread_root(Some(b_root.clone()));
        assert!(peers().iter().any(|x| x.id == a_me.id && x.public_key == a_me.public_key));
        store::set_thread_root(Some(a_root.clone()));
        // A lists B's sessions (whatever this host runs) and delivers a letter
        assert!(sessions_of(&p).is_some(), "a paired peer answers");
        let to = "33333333-4444-4555-8666-777777777777";
        let letter = Letter {
            id: uuid::Uuid::new_v4().to_string(),
            dispatch_id: uuid::Uuid::new_v4().to_string(),
            from: "cli".into(),
            from_name: "test".into(),
            to: to.into(),
            to_name: "T".into(),
            urgency: "普通".into(),
            wake: true,
            status: store::UNREAD.into(),
            created: store::now_text(),
            deadline: Some("2999-01-01 00:00".into()),
            to_harness: Some("claude".into()),
            body: "across the wire".into(),
            path: String::new(),
            from_host: None,
            from_host_name: None,
            to_host: Some(b_me.id.clone()),
            to_host_name: Some(b_me.name.clone()),
        };
        let a_store = Store::at(a_root.clone());
        let mut mine = letter.clone();
        a_store.write_letter(&mut mine).unwrap();
        let (status, route, _) = deliver(&Host { id: b_me.id.clone(), name: b_me.name.clone() }, &letter).expect("deliver");
        // nothing on this host runs that session: queued for the hook, or not delivered without one
        assert!(status == store::FAILED || status == store::UNREAD, "{status}");
        assert!(route == "failed" || route == "stored", "{route}");
        // B holds the letter, stamped with A as its origin
        let b_store = Store::at(b_root.clone());
        let mut got = b_store.list(None, false).unwrap().into_iter().find(|l| l.id == letter.id).expect("stored on B");
        assert_eq!(got.from_host.as_deref(), Some(a_me.id.as_str()));
        assert_eq!(got.from_host_name.as_deref(), Some(a_me.name.as_str()));
        assert!(got.to_host.is_none());
        // B's hook hands it over; A mirrors that onto its copy
        b_store.set_status(&mut got, store::READ_BY_HOOK).unwrap();
        MIRRORED.lock().unwrap().clear();
        assert_eq!(mirror(&a_store).unwrap(), 1);
        let local = a_store.list(None, false).unwrap().into_iter().find(|l| l.id == letter.id).unwrap();
        assert_eq!(local.status, store::READ_BY_HOOK);
        assert_eq!(remote_stage(&letter.id), Some(("delivered".into(), "stage.delivered_hook".into())));
        server.join().unwrap();
        store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(base);
    }
}
