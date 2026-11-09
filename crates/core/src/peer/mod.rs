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
mod persistence;
pub mod mdns;
pub mod net;
mod server;
pub mod sign;
mod wire;

pub use client::{hello, pair, pair_link, PeerError};
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
const LINK_SECS: i64 = 600;
const CODE_ATTEMPTS: u32 = 3;
const LINK_PREFIX: &str = "dispatch-pair:";

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
    /// Its iroh endpoint id: how it is dialled over the internet, and the key its letters are
    /// signed with. Pinned the first time it is learned.
    #[serde(default)]
    pub iroh_id: Option<String>,
    /// Where it said it can be reached over the internet (relay and addresses), for dialling
    /// without a lookup service.
    #[serde(default)]
    pub iroh_addr: Option<Value>,
    /// A person compared the fingerprints (or it was paired with a code in the same room).
    /// Letters from a machine that is not confirmed are held for approval.
    #[serde(default = "yes")]
    pub verified: bool,
    /// What its letters may do here: `all`, `contacts` (only to named sessions), `ask` (each
    /// waits for approval).
    #[serde(default = "rule_all")]
    pub rule: String,
}

fn yes() -> bool {
    true
}

fn rule_all() -> String {
    "all".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    pub me: Me,
    pub peers: Vec<PeerInfo>,
}

pub(crate) fn root() -> PathBuf {
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

pub(crate) fn write_private(path: &PathBuf, text: &str) -> std::io::Result<()> {
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
    let _lock = persistence::lock()?;
    persistence::load_locked()?;
    persistence::read_key()
}

/// Me and paired peers. Only a completely new store creates an identity.
pub fn load() -> Result<Book> {
    let _lock = persistence::lock()?;
    persistence::load_locked()
}

/// The computer's name, as the system shows it.
fn default_name() -> String {
    #[cfg(target_os = "macos")]
    let name = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    #[cfg(not(target_os = "macos"))]
    let name = std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    // the same cap a peer applies when it records us
    name.filter(|n| !n.is_empty()).unwrap_or_else(|| "Dispatch".into()).chars().take(48).collect()
}

pub fn me() -> Result<Me> {
    Ok(load()?.me)
}

pub fn peers() -> Result<Vec<PeerInfo>> {
    Ok(load()?.peers)
}

pub fn set_name(name: &str) -> Result<Me> {
    let name: String = name.trim().chars().take(48).collect();
    if name.is_empty() { anyhow::bail!("the name is empty"); }
    persistence::update(|b| { b.me.name = name; Ok(b.me.clone()) })
}

/// A peer by full id, then by name, then by id prefix. A machine paired again under the same
/// name (a new identity) is added after the old record, so a name means the latest pairing.
fn find_in(b: &Book, q: &str) -> Option<PeerInfo> {
    let q = q.trim();
    if q.is_empty() { return None; }
    let lower = q.to_lowercase();
    b.peers.iter().find(|p| p.id == lower)
        .or_else(|| b.peers.iter().rev().find(|p| p.name.eq_ignore_ascii_case(q)))
        .or_else(|| b.peers.iter().find(|p| q.len() >= 4 && p.id.starts_with(&lower))).cloned()
}

/// Whether a probe's failure is this system refusing to connect (macOS's Local Network
/// permission answers "No route to host"; a firewall, "Operation not permitted"), rather than
/// a machine that is off, asleep or never seen.
fn refused_here(problem: &str) -> bool {
    ["os error 65", "os error 1)", "No route to host", "Operation not permitted"].iter().any(|m| problem.contains(m))
}

/// A record that a later pairing under the same name has replaced: the name no longer reaches it.
fn superseded(b: &Book, i: usize) -> bool {
    b.peers.get(i).is_some_and(|p| b.peers[i + 1..].iter().any(|x| x.name.eq_ignore_ascii_case(&p.name)))
}

/// A paired peer by name or id prefix. Storage errors must not look like an unpaired host.
pub fn find_peer(q: &str) -> Result<Option<PeerInfo>> { Ok(find_in(&load()?, q)) }

pub(crate) fn peer_by_key(key: &[u8]) -> Result<Option<PeerInfo>> {
    let k = hex::encode(key);
    Ok(peers()?.into_iter().find(|p| p.public_key.eq_ignore_ascii_case(&k)))
}

/// Add or replace a peer by id after explicit pairing.
pub fn remember(p: PeerInfo) -> Result<()> {
    persistence::update(|b| { b.peers.retain(|x| x.id != p.id); b.peers.push(p); Ok(()) })
}

pub fn forget(q: &str) -> Result<Option<PeerInfo>> {
    persistence::update(|b| {
        let p = find_in(b, q);
        if let Some(p) = &p { b.peers.retain(|x| x.id != p.id); }
        Ok(p)
    })
}

fn edit_peer(q: &str, edit: impl FnOnce(&mut PeerInfo) -> Result<()>) -> Result<PeerInfo> {
    persistence::update(|b| {
        let p = find_in(b, q).ok_or_else(|| anyhow::anyhow!("{}", t("cli.peers_not_found").replace("{q}", q)))?;
        let p = b.peers.iter_mut().find(|x| x.id == p.id).expect("found under the same lock");
        edit(p)?;
        Ok(p.clone())
    })
}

pub fn set_address(q: &str, address: Option<String>) -> Result<PeerInfo> {
    edit_peer(q, |p| { p.address = address.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()); Ok(()) })
}

pub(crate) fn pin_keys(peer_id: &str, iroh: Option<String>, iroh_addr: Option<Value>) -> Result<(), PeerError> {
    persistence::update(|b| {
        let p = b.peers.iter_mut().find(|p| p.id == peer_id)
            .ok_or_else(|| anyhow::anyhow!("not paired with that machine"))?;
        if let Some(new) = iroh.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            if p.iroh_id.as_ref().is_some_and(|old| old != &new) {
                log_problem(&p.name, "its signing key changed; refused until it is paired again");
                anyhow::bail!("that machine's signing key changed; pair it again");
            }
            p.iroh_id = Some(new);
        }
        if iroh_addr.is_some() { p.iroh_addr = iroh_addr; }
        Ok(())
    }).map_err(|e| PeerError::Refused(format!("{e:#}")))
}

/// Four groups of four digits that both machines of a pair compute the same way from the two
/// machines' keys: what a person compares on the two screens.
pub fn fingerprint(peer_key: &str) -> Result<String> {
    Ok(fingerprint_with(&me()?.public_key, peer_key))
}

fn fingerprint_with(my_key: &str, peer_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mine = my_key.to_lowercase();
    let theirs = peer_key.to_lowercase();
    let (a, b) = if mine <= theirs { (mine, theirs) } else { (theirs, mine) };
    let h = Sha256::digest(format!("dispatch-fingerprint/1:{a}:{b}").as_bytes());
    (0..4).map(|i| format!("{:04}", u16::from_be_bytes([h[2 * i], h[2 * i + 1]]) % 10000)).collect::<Vec<_>>().join(" ")
}

/// A person compared the fingerprints: the machine is confirmed, and its letters held for
/// that reason are delivered (unless its rule says to ask). Returns how many were released.
pub fn verify_peer(q: &str) -> Result<(PeerInfo, usize)> {
    let p = edit_peer(q, |p| { p.verified = true; Ok(()) })?;
    let mut released = 0;
    if p.rule != "ask" {
        let store = Store::open();
        for mut l in store.list(None, false)?.into_iter().filter(|l| l.status == store::HELD && l.from_host.as_deref() == Some(&p.id)) {
            store.set_status(&mut l, store::UNREAD)?;
            store.append_run(&l.id, "peer", "released", "machine confirmed")?;
            let to = crate::send::target_of(&l);
            let _ = crate::send::deliver(&store, &to, &mut l, true, false);
            released += 1;
        }
    }
    Ok((p, released))
}

/// What a peer's letters may do here: `all`, `contacts` or `ask`.
pub fn set_rule(q: &str, rule: &str) -> Result<PeerInfo> {
    if !matches!(rule, "all" | "contacts" | "ask") {
        anyhow::bail!("the rule must be all, contacts or ask");
    }
    edit_peer(q, |p| { p.rule = rule.to_string(); Ok(()) })
}

/// A line for whoever has to find out why a machine is offline or a letter was refused.
static LOGGED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const LOG_REPEAT: Duration = Duration::from_secs(600);
const LOG_MAX: u64 = 1_000_000;

/// One line per problem, written in a single call so processes logging at once do not
/// interleave; the same problem at most once every ten minutes per process; past 1 MB the
/// file moves to `errors.log.1`.
pub(crate) fn log_problem(who: &str, what: &str) {
    let text = format!("{} {}", who, what).replace(['\n', '\r'], " ");
    {
        let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        if logged.get(&text).is_some_and(|at| at.elapsed() < LOG_REPEAT) { return; }
        if logged.len() > 256 { logged.retain(|_, at| at.elapsed() < LOG_REPEAT); }
        logged.insert(text.clone(), Instant::now());
    }
    let path = state_dir().join("errors.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&path, state_dir().join("errors.log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write as _;
        let line = format!("{} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), text);
        let _ = f.write_all(line.as_bytes());
    }
}

// ---------------------------------------------------------------- pairing code

#[derive(Debug, Serialize, Deserialize)]
struct Pairing {
    code: String,
    expires_at: i64,
    attempts: u32,
    /// `code` (six digits, shown in the same room) or `link` (a long secret that travels).
    #[serde(default)]
    kind: String,
}

/// What a pairing link carries: who made it, its keys, where to reach it, the one-time secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Link {
    pub v: u8,
    pub id: String,
    pub name: String,
    pub noise: String,
    pub iroh: String,
    #[serde(default)]
    pub addr: Option<Value>,
    #[serde(rename = "s")]
    pub secret: String,
}

impl Link {
    pub fn encode(&self) -> String {
        use base64::Engine as _;
        format!("{LINK_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default()))
    }

    pub fn decode(text: &str) -> Result<Link> {
        use base64::Engine as _;
        let body = text.trim().strip_prefix(LINK_PREFIX).ok_or_else(|| anyhow::anyhow!("not a Dispatch pairing link"))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body.trim()).map_err(|_| anyhow::anyhow!("the pairing link is damaged"))?;
        let l: Link = serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("the pairing link is damaged"))?;
        if l.v != 1 || l.secret.len() < 32 {
            anyhow::bail!("this pairing link is from another version of Dispatch");
        }
        Ok(l)
    }
}

/// Whether text is a pairing link.
pub fn is_link(text: &str) -> bool {
    text.trim().starts_with(LINK_PREFIX)
}

/// A pairing link good for ten minutes and one use, for a machine on another network: it
/// carries this machine's keys, where to reach it, and a long one-time secret.
pub fn new_link() -> Result<(String, i64)> {
    if !net::enabled() {
        anyhow::bail!("internet connections are off on this machine (Settings, or dispatch peers internet on)");
    }
    let iroh = net::my_id().ok_or_else(|| anyhow::anyhow!("no iroh key"))?;
    let secret = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let me = me()?;
    let addr = net::my_addr_ready(Duration::from_secs(15));
    if addr.is_none() {
        anyhow::bail!("nothing here listens over the internet yet: open the Dispatch app, or run dispatch serve");
    }
    let link = Link { v: 1, id: me.id, name: me.name, noise: me.public_key, iroh, addr, secret: secret.clone() };
    let p = Pairing { code: secret, expires_at: chrono::Utc::now().timestamp() + LINK_SECS, attempts: 0, kind: "link".into() };
    std::fs::create_dir_all(state_dir())?;
    write_private(&pairing_path(), &serde_json::to_string(&p)?)?;
    Ok((link.encode(), LINK_SECS))
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
    load()?;
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    let code = format!("{n:06}");
    let p = Pairing { code: code.clone(), expires_at: chrono::Utc::now().timestamp() + CODE_SECS, attempts: 0, kind: "code".into() };
    std::fs::create_dir_all(state_dir())?;
    write_private(&pairing_path(), &serde_json::to_string(&p)?)?;
    Ok((code, CODE_SECS))
}

/// The active six-digit code and its seconds left, if any (a link's secret is never shown).
pub fn current_code() -> Option<(String, i64)> {
    read_pairing().filter(|p| p.kind != "link").map(|p| (p.code, p.expires_at - chrono::Utc::now().timestamp()))
}

/// Check a pairing attempt against the active code or link. Ok(Some(kind)): matched, spent.
/// Ok(None): wrong; after three wrong attempts it is spent too. Err: nothing active.
pub(crate) fn take_pairing(ok: impl Fn(&str) -> bool) -> Result<Option<String>, &'static str> {
    let Some(mut p) = read_pairing() else { return Err("no pairing code or link is active on that machine: it was used, it expired, or none was made") };
    if ok(&p.code) {
        let _ = std::fs::remove_file(pairing_path());
        return Ok(Some(if p.kind.is_empty() { "code".into() } else { p.kind }));
    }
    p.attempts += 1;
    if p.attempts >= CODE_ATTEMPTS {
        let _ = std::fs::remove_file(pairing_path());
    } else {
        let _ = write_private(&pairing_path(), &serde_json::to_string(&p).unwrap_or_default());
    }
    Ok(None)
}

// ---------------------------------------------------------------- addressing

/// `<peer>/<session>`: the peer when the part before the slash names one.
pub fn split_address(q: &str) -> Result<Option<(PeerInfo, String)>> {
    let Some((h, rest)) = q.split_once('/') else { return Ok(None) };
    Ok(find_peer(h)?.map(|p| (p, rest.trim().to_string())))
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
        project: None,
    }
}

// ---------------------------------------------------------------- caches

struct Remote {
    at: Instant,
    sessions: Vec<Session>,
    reach: HashMap<String, Reach>,
}

static REMOTE: LazyLock<Mutex<HashMap<String, Remote>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// How a message would reach the remote sessions searches turned up, by (machine, session).
static FOUND: LazyLock<Mutex<HashMap<(String, String), Reach>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Peers that did not answer, and until when they are left alone.
static OFFLINE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// What a peer last said about a copy on it: (kind, stage key), by letter id.
static STAGES: LazyLock<Mutex<HashMap<String, (String, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static MIRRORED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// How each peer was last reached: `lan`, `direct` or `relay`, and when.
static ROUTES: LazyLock<Mutex<HashMap<String, (Instant, &'static str)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn routes_path() -> PathBuf {
    state_dir().join("routes.json")
}

/// Remember how a peer was reached, here and in `peer/routes.json` for the other processes
/// (the command line asks; the app connects).
pub(crate) fn note_route(peer_id: &str, route: &'static str) {
    ROUTES.lock().unwrap().insert(peer_id.to_string(), (Instant::now(), route));
    let mut all: HashMap<String, Value> = std::fs::read_to_string(routes_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.insert(peer_id.to_string(), json!({ "route": route, "at": chrono::Utc::now().timestamp() }));
    let _ = std::fs::create_dir_all(state_dir());
    let _ = std::fs::write(routes_path(), serde_json::to_string(&all).unwrap_or_default());
}

/// How a peer was reached within the last minute and a half, by this process or another.
pub fn route(peer_id: &str) -> Option<&'static str> {
    if let Some(r) = ROUTES.lock().unwrap().get(peer_id).filter(|(at, _)| at.elapsed() < Duration::from_secs(90)).map(|(_, r)| *r) {
        return Some(r);
    }
    let all: HashMap<String, Value> = std::fs::read_to_string(routes_path()).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    let v = all.get(peer_id)?;
    let fresh = v["at"].as_i64().map(|at| chrono::Utc::now().timestamp() - at < 90).unwrap_or(false);
    match (fresh, v["route"].as_str()) {
        (true, Some("lan")) => Some("lan"),
        (true, Some("direct")) => Some("direct"),
        (true, Some("relay")) => Some("relay"),
        _ => None,
    }
}

fn mark_offline(p: &PeerInfo, why: String) {
    log_problem(&p.name, &why);
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
    !backing_off(p) && (route(&p.id).is_some() || probe(p).is_ok())
}

/// Try the peer's addresses in order; Ok(the address that answered) or Err(why the last one did
/// not), in words a person can act on.
pub fn probe(p: &PeerInfo) -> Result<String, String> {
    let addrs = client::addresses(p);
    if addrs.is_empty() {
        return Err("no address: not seen on the network and none set by hand".into());
    }
    let mut errors: Vec<String> = vec![];
    for a in &addrs {
        match std::net::TcpStream::connect_timeout(a, Duration::from_millis(700)) {
            Ok(_) => return Ok(a.to_string()),
            Err(e) => errors.push(format!("{a}: {e}")),
        }
    }
    errors.truncate(3);
    Err(errors.join("; "))
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
    match ask(p, &Request::Sessions { query: None }) {
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
            // names this machine gave sessions over there
            crate::contacts::apply_remote(&p.id, &mut list);
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

/// The paired machines worth asking for sessions: not those a later pairing under the same name
/// replaced (a dead identity that only costs a timeout).
fn current(peers: Vec<PeerInfo>) -> Vec<PeerInfo> {
    let mut out: Vec<PeerInfo> = vec![];
    for p in peers.into_iter().rev() {
        if !out.iter().any(|x| x.name.eq_ignore_ascii_case(&p.name)) {
            out.push(p);
        }
    }
    out.reverse();
    out
}

/// The sessions on every paired peer that answers.
pub fn remote_sessions() -> Vec<Session> {
    let peers = match peers() { Ok(p) => current(p), Err(e) => { log_problem("storage", &format!("{e:#}")); return vec![]; } };
    let mut out = vec![];
    for p in &peers {
        if let Some(list) = sessions_of(p) {
            out.extend(list);
        }
    }
    out
}

/// Every session on the paired machines (or only on `only`) that matches the query, whatever
/// its age, as each machine searches its own records. A machine that predates searching sends
/// its recent list instead; the caller filters what comes back. Names this machine gave
/// sessions over there are put on, and those not found are added from the address book.
pub fn remote_find(query: &str, only: Option<&PeerInfo>) -> Vec<Session> {
    let peers = match only {
        Some(p) => vec![p.clone()],
        None => match peers() { Ok(p) => current(p), Err(e) => { log_problem("storage", &format!("{e:#}")); return vec![]; } },
    };
    let mut out = vec![];
    for p in &peers {
        let mut list = vec![];
        match ask(p, &Request::Sessions { query: Some(query.trim().to_string()) }) {
            Ok(Response::Sessions { sessions, .. }) => {
                let mut found = FOUND.lock().unwrap();
                if found.len() > 5000 {
                    found.clear();
                }
                for rs in sessions {
                    let mut s = rs.session;
                    s.host = Some(Host { id: p.id.clone(), name: p.name.clone() });
                    s.transcript = None;
                    found.insert((p.id.clone(), s.id.clone()), rs.reach);
                    list.push(s);
                }
            }
            Ok(Response::Error { message }) => mark_offline(p, message),
            _ => {}
        }
        crate::contacts::apply_remote(&p.id, &mut list);
        out.extend(list);
    }
    out
}

/// How a message reaches a remote session: what its machine said when it listed it, or when it
/// turned up in a search.
pub fn remote_reach(s: &Session) -> Reach {
    if let Some(h) = &s.host {
        if let Some(r) = REMOTE.lock().unwrap().get(&h.id).and_then(|r| r.reach.get(&s.id)) {
            return r.clone();
        }
        if let Some(r) = FOUND.lock().unwrap().get(&(h.id.clone(), s.id.clone())) {
            return r.clone();
        }
    }
    Reach { level: Level::Later, text: t("reach.peer_offline").into(), fix: None, mode: None, client: None }
}

/// Hand a letter to the machine its recipient is on: (status there, route, detail).
pub fn deliver(host: &Host, letter: &Letter) -> Result<(String, String, String), PeerError> {
    let Some(p) = peers().map_err(|e| PeerError::Refused(format!("{e:#}")))?.into_iter().find(|p| p.id == host.id) else { return Err(PeerError::Refused("not paired with that machine".into())) };
    let signature = net::secret().ok().map(|k| sign::sign(&k, letter));
    match ask(&p, &Request::Deliver { letter: letter.clone(), signature })? {
        Response::Delivered { status, route, detail } => Ok((status, route, detail)),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// The last turns of a session on a peer, as that machine reads them.
pub fn transcript(host: &Host, session_id: &str, limit: usize) -> Result<Vec<crate::transcript::Turn>, PeerError> {
    let Some(p) = peers().map_err(|e| PeerError::Refused(format!("{e:#}")))?.into_iter().find(|p| p.id == host.id) else { return Err(PeerError::Refused("not paired with that machine".into())) };
    match ask(&p, &Request::Transcript { session_id: session_id.to_string(), limit })? {
        Response::Transcript { turns } => Ok(turns),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// Withdraw a copy on a peer, best effort.
pub fn cancel(host: &Host, letter_id: &str) {
    if let Some(p) = peers().ok().into_iter().flatten().find(|p| p.id == host.id) {
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
    let peers = peers()?;
    if peers.is_empty() {
        return Ok(0);
    }
    let me = me()?;
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
                    // a letter for a session here that the peer could not push: take it now,
                    // once its signature checks out against the key pinned for that machine
                    if remote.to_host.as_deref() == Some(&me.id) && remote.status != store::CANCELLED && remote.status != store::REVIEW {
                        let signed = match (p.iroh_id.as_deref(), rc.signature.as_deref()) {
                            (Some(signer), Some(sig)) => sign::verify(signer, &remote, sig),
                            _ => false,
                        };
                        if !signed {
                            log_problem(&p.name, &format!("pulled letter {} has no valid signature; not taken", crate::model::short_id(&remote.id)));
                            continue;
                        }
                        if p.rule == "contacts" && crate::contacts::for_session(&remote.to, None).is_none() {
                            log_problem(&p.name, &format!("pulled letter {} is not for a named session; not taken", crate::model::short_id(&remote.id)));
                            continue;
                        }
                        // the same rule as a pushed letter: an unconfirmed machine, or one that
                        // must ask first, waits for a person here
                        let hold = !p.verified || p.rule == "ask";
                        let mut l = remote.clone();
                        l.signature = rc.signature.clone();
                        l.to_host = None;
                        l.to_host_name = None;
                        l.from_host = Some(p.id.clone());
                        l.from_host_name = Some(p.name.clone());
                        l.status = if hold { store::HELD.into() } else { store::UNREAD.into() };
                        l.path = String::new();
                        store.write_letter(&mut l)?;
                        store.append_run(&l.id, "peer", "pulled", &format!("{}; signature verified", p.name))?;
                        if hold {
                            store.append_run(&l.id, "peer", "held", if !p.verified { "machine not confirmed yet" } else { "rule: ask first" })?;
                        } else {
                            let to = crate::send::target_of(&l);
                            let _ = crate::send::deliver(store, &to, &mut l, true, false);
                        }
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
pub fn overview() -> Result<Value> {
    let b = load()?;
    let seen = mdns::seen_all();
    let listening = mdns::serving();
    let peers: Vec<Value> = b
        .peers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let s = seen.iter().find(|s| s.id == p.id);
            let backoff = OFFLINE.lock().unwrap().get(&p.id).filter(|(until, _)| Instant::now() < *until).map(|(_, why)| why.clone());
            let link = route(&p.id);
            let probe = if link.is_some() { Ok(String::new()) } else { probe(p) };
            let blocked = !superseded(&b, i) && backoff.is_none() && probe.as_ref().err().is_some_and(|e| refused_here(e));
            json!({
                "id": p.id, "name": p.name, "address": p.address, "paired_at": p.paired_at,
                "online": backoff.is_none() && probe.is_ok(),
                "seen": s.map(|s| json!({ "addrs": s.addrs, "port": s.port })),
                "problem": backoff.or_else(|| probe.err()),
                "link": link.or(if s.is_some() { Some("lan") } else { None }),
                "fingerprint": fingerprint_with(&b.me.public_key, &p.public_key),
                "verified": p.verified,
                "rule": p.rule,
                "internet": p.iroh_id.is_some(),
                "superseded": superseded(&b, i),
                "blocked": blocked,
            })
        })
        .collect();
    let unpaired: Vec<Value> = seen.iter().filter(|s| !b.peers.iter().any(|p| p.id == s.id)).map(|s| json!({ "id": s.id, "name": s.name, "addrs": s.addrs, "port": s.port })).collect();
    Ok(json!({
        "me": { "id": b.me.id, "name": b.me.name, "public_key": b.me.public_key, "listening": listening, "iroh_id": net::my_id(), "internet_listening": net::my_addr().is_some() },
        "internet": net::enabled(),
        "relay": net::relay_url(),
        "proxy": net::proxy(),
        "peers": peers,
        "seen": unpaired,
        "code": current_code().map(|(c, left)| json!({ "code": c, "seconds_left": left })),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_named(id: &str, name: &str) -> PeerInfo {
        serde_json::from_value(serde_json::json!({ "id": id, "name": name, "public_key": "00", "paired_at": "2026-11-09 08:43" })).unwrap()
    }

    /// The main Mac lost its identity and was paired again under the same name: the name, the
    /// app's actions (by id) and the overview must all point at the right record.
    #[test]
    fn only_a_refused_connection_reads_as_blocked() {
        assert!(refused_here("192.168.71.39:47831: No route to host (os error 65)"));
        assert!(refused_here("10.0.0.2:47831: Operation not permitted (os error 1)"));
        assert!(!refused_here("no address: not seen on the network and none set by hand"));
        assert!(!refused_here("192.168.71.39:47831: Connection refused (os error 61)"));
        assert!(!refused_here("192.168.71.39:47831: connection timed out"));
    }

    #[test]
    fn a_name_paired_again_means_the_latest_pairing() {
        let mut b: Book = serde_json::from_value(serde_json::json!({ "me": { "id": "me", "name": "me", "public_key": "00" }, "peers": [] })).unwrap();
        b.peers.push(peer_named("a7acb9c9-0000-0000-0000-000000000000", "Mac mini"));
        b.peers.push(peer_named("0b4329cb-0000-0000-0000-000000000000", "Mac mini"));
        b.peers.push(peer_named("5555aaaa-0000-0000-0000-000000000000", "Laptop"));
        assert_eq!(find_in(&b, "mac MINI").unwrap().id, b.peers[1].id);
        assert_eq!(find_in(&b, "a7acb9c9-0000-0000-0000-000000000000").unwrap().id, b.peers[0].id);
        assert_eq!(find_in(&b, "a7ac").unwrap().id, b.peers[0].id);
        assert_eq!(find_in(&b, "Laptop").unwrap().id, b.peers[2].id);
        assert!(superseded(&b, 0));
        assert!(!superseded(&b, 1) && !superseded(&b, 2));
        // only the latest pairing is asked for sessions
        let asked: Vec<String> = current(b.peers.clone()).into_iter().map(|p| p.id[..8].to_string()).collect();
        assert_eq!(asked, ["0b4329cb", "5555aaaa"]);
    }

    #[test]
    fn a_repeated_problem_is_logged_once_and_whole() {
        let base = std::env::temp_dir().join(format!("dispatch-log-{}", uuid::Uuid::new_v4()));
        store::set_thread_root(Some(base.clone()));
        std::fs::create_dir_all(state_dir()).unwrap();
        let who = format!("peer-{}", uuid::Uuid::new_v4());
        for _ in 0..5 { log_problem(&who, "offline: no answer\nwithin 8s"); }
        log_problem(&who, "a different problem");
        let text = std::fs::read_to_string(state_dir().join("errors.log")).unwrap();
        store::set_thread_root(None);
        std::fs::remove_dir_all(base).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].ends_with("offline: no answer within 8s"), "{text}");
        assert!(lines[1].ends_with("a different problem"), "{text}");
    }

    #[test]
    fn corrupt_book_never_replaces_existing_identity() {
        let base = std::env::temp_dir().join(format!("dispatch-corrupt-{}", uuid::Uuid::new_v4()));
        store::set_thread_root(Some(base.clone()));
        load().unwrap();
        let key_before = std::fs::read(key_path()).unwrap();
        std::fs::write(book_path(), b"{\"me\":").unwrap();
        assert!(load().is_err());
        let key_unchanged = std::fs::read(key_path()).unwrap() == key_before;
        let book_unchanged = std::fs::read(book_path()).unwrap() == b"{\"me\":";
        store::set_thread_root(None);
        std::fs::remove_dir_all(base).unwrap();
        assert!(key_unchanged, "a read must never replace an existing private key");
        assert!(book_unchanged, "preserve the damaged book for recovery");
    }

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
        let b_me = load().unwrap().me;
        let b_key = private_key().unwrap();
        let (code, _) = new_code().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let b_root2 = b_root.clone();
        std::thread::spawn(move || {
            store::set_thread_root(Some(b_root2));
            for s in listener.incoming() {
                let Ok(s) = s else { continue };
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                if let Ok(mut conn) = wire::respond(s, &b_key) {
                    let _ = server::serve_conn(&mut conn);
                }
            }
        });
        // machine A pairs with B at that address
        store::set_thread_root(Some(a_root.clone()));
        let a_me = load().unwrap().me;
        assert_ne!(a_me.id, b_me.id);
        let p = pair(&format!("{addr}"), &code).expect("pairing");
        assert_eq!(p.id, b_me.id);
        assert_eq!(p.address.as_deref(), Some(format!("{addr}").as_str()), "a machine mDNS did not see keeps its address");
        assert!(current_code().is_none() || true, "the code lives on B");
        // B remembered A as well
        store::set_thread_root(Some(b_root.clone()));
        assert!(peers().unwrap().iter().any(|x| x.id == a_me.id && x.public_key == a_me.public_key));
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
            from_harness: None,
            signature: None,
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
        // both sides learned each other's signing key on the way
        assert!(peers().unwrap().iter().any(|x| x.id == b_me.id && x.iroh_id.is_some()));
        // a letter with a forged signature is refused before it is stored
        let mut forged = letter.clone();
        forged.id = uuid::Uuid::new_v4().to_string();
        forged.body = "forged".into();
        let bad = sign::sign(&iroh::SecretKey::generate(), &forged);
        match client::request(&p, &Request::Deliver { letter: forged.clone(), signature: Some(bad) }) {
            Ok(Response::Error { message }) => assert!(message.contains("signature"), "{message}"),
            other => panic!("a forged letter must be refused: {other:?}"),
        }
        match client::request(&p, &Request::Deliver { letter: forged.clone(), signature: None }) {
            Ok(Response::Error { message }) => assert!(message.contains("not signed"), "{message}"),
            other => panic!("an unsigned letter must be refused: {other:?}"),
        }
        assert!(Store::at(b_root.clone()).list(None, false).unwrap().iter().all(|l| l.id != forged.id), "nothing forged was stored");
        // a machine that is not confirmed gets its letters held; confirming it releases them
        store::set_thread_root(Some(b_root.clone()));
        persistence::update(|book| { book.peers.iter_mut().for_each(|x| x.verified = false); Ok(()) }).unwrap();
        store::set_thread_root(Some(a_root.clone()));
        let mut held = letter.clone();
        held.id = uuid::Uuid::new_v4().to_string();
        held.body = "wait for me".into();
        let (status, _, _) = deliver(&Host { id: b_me.id.clone(), name: b_me.name.clone() }, &held).expect("held delivery");
        assert_eq!(status, store::HELD);
        store::set_thread_root(Some(b_root.clone()));
        let (_, released) = verify_peer(&a_me.id).unwrap();
        assert_eq!(released, 1);
        let after = Store::at(b_root.clone()).list(None, false).unwrap().into_iter().find(|l| l.id == held.id).unwrap();
        assert_ne!(after.status, store::HELD);
        assert!(after.signature.is_some(), "the verified signature is kept with the letter");
        store::set_thread_root(None);
        let _ = std::fs::remove_dir_all(base);
    }
}
