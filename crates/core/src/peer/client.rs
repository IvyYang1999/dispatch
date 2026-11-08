//! Talking to a peer: find an address for it, connect, check its key, ask one thing.

use super::wire::{self, Request, Response};
use super::{find_peer, mdns, me, private_key, remember, PeerInfo, DEFAULT_PORT};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub enum PeerError {
    /// No address, or nothing answered: try again later.
    Offline(String),
    /// It answered and said no (not paired, another key, a wrong code).
    Refused(String),
    /// A broken exchange.
    Protocol(String),
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerError::Offline(s) => write!(f, "offline: {s}"),
            PeerError::Refused(s) => write!(f, "refused: {s}"),
            PeerError::Protocol(s) => write!(f, "protocol: {s}"),
        }
    }
}

impl std::error::Error for PeerError {}

fn protocol(e: anyhow::Error) -> PeerError {
    PeerError::Protocol(format!("{e:#}"))
}

/// Where a paired peer can be reached: its hand-set address, else what mDNS saw.
pub(crate) fn addresses(p: &PeerInfo) -> Vec<SocketAddr> {
    if let Some(a) = p.address.as_deref().filter(|a| !a.trim().is_empty()) {
        return resolve(a);
    }
    mdns::seen(&p.id).map(|s| s.socket_addrs()).unwrap_or_default()
}

/// `host:port`, `host` (the default port) or an IP, to socket addresses.
pub(crate) fn resolve(target: &str) -> Vec<SocketAddr> {
    let t = target.trim();
    if let Ok(a) = t.parse::<SocketAddr>() {
        return vec![a];
    }
    let candidate = if t.matches(':').count() == 1 { t.to_string() } else { format!("{t}:{DEFAULT_PORT}") };
    candidate.to_socket_addrs().map(|i| i.collect()).unwrap_or_default()
}

fn connect(addrs: &[SocketAddr], key: &[u8]) -> Result<wire::Conn, PeerError> {
    if addrs.is_empty() {
        return Err(PeerError::Offline("no address".into()));
    }
    let mut last = String::new();
    for a in addrs {
        match TcpStream::connect_timeout(a, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                let _ = s.set_nodelay(true);
                return wire::initiate(s, key).map_err(protocol);
            }
            Err(e) => last = format!("{a}: {e}"),
        }
    }
    Err(PeerError::Offline(last))
}

/// One request to a paired peer, its key pinned to what pairing recorded.
pub(crate) fn request(p: &PeerInfo, req: &Request) -> Result<Response, PeerError> {
    let key = private_key().map_err(protocol)?;
    let mut conn = connect(&addresses(p), &key)?;
    if hex::encode(&conn.remote_key) != p.public_key.to_lowercase() {
        return Err(PeerError::Refused("the machine at that address has a different key".into()));
    }
    conn.send(req).map_err(protocol)?;
    conn.recv().map_err(protocol)
}

/// A name mDNS saw, a paired peer's name, or `host:port`.
fn target_addrs(target: &str) -> Vec<SocketAddr> {
    if let Some(s) = mdns::seen_by_name(target) {
        return s.socket_addrs();
    }
    if let Some(p) = find_peer(target) {
        let a = addresses(&p);
        if !a.is_empty() {
            return a;
        }
    }
    resolve(target)
}

/// Who is at `target`: id, name, version and static key, no pairing needed.
pub fn hello(target: &str) -> Result<(String, String, String, String), PeerError> {
    let key = private_key().map_err(protocol)?;
    let mut conn = connect(&target_addrs(target), &key)?;
    conn.send(&Request::Hello).map_err(protocol)?;
    match conn.recv().map_err(protocol)? {
        Response::Welcome { id, name, version } => Ok((id, name, version, hex::encode(&conn.remote_key))),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// Pair with the machine at `target` using the code it shows. Both sides remember each other.
pub fn pair(target: &str, code: &str) -> Result<PeerInfo, PeerError> {
    let code = code.trim();
    let key = private_key().map_err(protocol)?;
    let mut conn = connect(&target_addrs(target), &key)?;
    let me = me();
    let proof = wire::proof(code, &conn.handshake_hash);
    conn.send(&Request::Pair { proof, id: me.id, name: me.name }).map_err(protocol)?;
    match conn.recv().map_err(protocol)? {
        Response::Paired { id, name } => {
            // a machine mDNS does not see keeps the address it was reached at
            let manual = mdns::seen_by_name(target).is_none() && mdns::seen(&id).is_none();
            let p = PeerInfo {
                id,
                name,
                public_key: hex::encode(&conn.remote_key),
                address: if manual { Some(target.trim().to_string()) } else { None },
                paired_at: crate::store::now_text(),
            };
            remember(p.clone()).map_err(protocol)?;
            Ok(p)
        }
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}
