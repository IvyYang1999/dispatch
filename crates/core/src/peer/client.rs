//! Talking to a peer: find an address for it, connect, check its key, ask one thing.

use super::wire::{self, Request, Response};
use super::{find_peer, mdns, me, net, private_key, remember, PeerInfo, DEFAULT_PORT};
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
    let mut errors: Vec<String> = vec![];
    for a in addrs {
        match TcpStream::connect_timeout(a, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                let _ = s.set_nodelay(true);
                return wire::initiate(s, key).map_err(protocol);
            }
            Err(e) => errors.push(format!("{a}: {e}")),
        }
    }
    errors.truncate(3);
    Err(PeerError::Offline(errors.join("; ")))
}

/// A connection to a paired peer: over the local network when it answers there, else over the
/// internet path by its iroh id. Returns the route: `lan`, `direct` or `relay`.
fn connect_peer(p: &PeerInfo, key: &[u8]) -> Result<(wire::Conn, &'static str), PeerError> {
    let mut lan: Option<PeerError> = None;
    if std::env::var_os("DISPATCH_PEER_FORCE_INTERNET").is_none() {
        match connect(&addresses(p), key) {
            Ok(c) => return Ok((c, "lan")),
            Err(e @ PeerError::Offline(_)) => lan = Some(e),
            Err(e) => return Err(e),
        }
    }
    if let (true, Some(id)) = (net::enabled(), p.iroh_id.as_deref()) {
        return match net::peer_addr(id, p.iroh_addr.as_ref()).and_then(net::dial) {
            Ok((s, route)) => {
                let _ = s.set_read_timeout(Some(IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                wire::initiate(s, key).map(|c| (c, route.code())).map_err(protocol)
            }
            Err(e) => Err(PeerError::Offline(match lan {
                Some(l) => format!("{l}; internet: {e:#}"),
                None => format!("internet: {e:#}"),
            })),
        };
    }
    Err(lan.unwrap_or_else(|| PeerError::Offline("no address".into())))
}

/// Exchange signing keys and internet addresses with a paired peer (pinned on first sight).
fn exchange_keys(conn: &mut wire::Conn, p: &PeerInfo) -> Result<(), PeerError> {
    conn.send(&Request::Keys { iroh: net::my_id(), iroh_addr: net::my_addr() }).map_err(protocol)?;
    match conn.recv().map_err(protocol)? {
        Response::Keys { iroh, iroh_addr } => super::pin_keys(&p.id, iroh, iroh_addr),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// One request to a paired peer, its key pinned to what pairing recorded.
pub(crate) fn request(p: &PeerInfo, req: &Request) -> Result<Response, PeerError> {
    let key = private_key().map_err(protocol)?;
    let pinned = |conn: &wire::Conn| -> Result<(), PeerError> {
        if hex::encode(&conn.remote_key) != p.public_key.to_lowercase() {
            return Err(PeerError::Refused("the machine at that address has a different key".into()));
        }
        Ok(())
    };
    let (mut conn, mut route) = connect_peer(p, &key)?;
    pinned(&conn)?;
    match exchange_keys(&mut conn, p) {
        Ok(()) => {}
        // an older Dispatch closes on a request it does not know: ask again without the key
        // exchange. Nothing is weakened: letters from it still need a valid signature here,
        // and replies pulled from it are not taken without one.
        Err(PeerError::Protocol(_)) => {
            let (c, r) = connect_peer(p, &key)?;
            pinned(&c)?;
            conn = c;
            route = r;
        }
        Err(e) => return Err(e),
    }
    conn.send(req).map_err(protocol)?;
    let r = conn.recv().map_err(protocol)?;
    super::note_route(&p.id, route);
    Ok(r)
}

/// A name mDNS saw, a paired peer's name, or `host:port`.
fn target_addrs(target: &str) -> Result<Vec<SocketAddr>, PeerError> {
    if let Some(s) = mdns::seen_by_name(target) {
        return Ok(s.socket_addrs());
    }
    if let Some(p) = find_peer(target).map_err(protocol)? {
        let a = addresses(&p);
        if !a.is_empty() {
            return Ok(a);
        }
    }
    Ok(resolve(target))
}

/// Who is at `target`: id, name, version and static key, no pairing needed.
pub fn hello(target: &str) -> Result<(String, String, String, String), PeerError> {
    let key = private_key().map_err(protocol)?;
    let mut conn = connect(&target_addrs(target)?, &key)?;
    conn.send(&Request::Hello).map_err(protocol)?;
    match conn.recv().map_err(protocol)? {
        Response::Welcome { id, name, version, .. } => Ok((id, name, version, hex::encode(&conn.remote_key))),
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// Pair with the machine at `target` using the code it shows. Both sides remember each other.
pub fn pair(target: &str, code: &str) -> Result<PeerInfo, PeerError> {
    let code = code.trim();
    let key = private_key().map_err(protocol)?;
    let mut conn = connect(&target_addrs(target)?, &key)?;
    // a machine mDNS does not see keeps the address it was reached at
    let manual = mdns::seen_by_name(target).is_none();
    finish_pairing(&mut conn, code, true, if manual { Some(target.trim().to_string()) } else { None })
}

/// Send the proof and record the machine that accepted it.
fn finish_pairing(conn: &mut wire::Conn, secret: &str, verified: bool, address: Option<String>) -> Result<PeerInfo, PeerError> {
    let me = me().map_err(protocol)?;
    let proof = wire::proof(secret, &conn.handshake_hash);
    conn.send(&Request::Pair { proof, id: me.id, name: me.name, iroh: net::my_id(), iroh_addr: net::my_addr() }).map_err(protocol)?;
    match conn.recv().map_err(protocol)? {
        Response::Paired { id, name, iroh, iroh_addr } => {
            let address = address.filter(|_| mdns::seen(&id).is_none());
            let p = PeerInfo {
                id,
                name,
                public_key: hex::encode(&conn.remote_key),
                address,
                paired_at: crate::store::now_text(),
                iroh_id: iroh,
                iroh_addr,
                verified,
                rule: "all".into(),
            };
            remember(p.clone()).map_err(protocol)?;
            Ok(p)
        }
        Response::Error { message } => Err(PeerError::Refused(message)),
        _ => Err(PeerError::Protocol("unexpected answer".into())),
    }
}

/// Pair with the machine that made a pairing link, wherever it is: over the local network
/// when it is there, else over the internet path. The machine's key in the link must match
/// the one that answers. A machine paired by link stays unconfirmed until a person compares
/// the fingerprints on both screens.
pub fn pair_link(link: &str) -> Result<PeerInfo, PeerError> {
    let l = super::Link::decode(link).map_err(|e| PeerError::Refused(format!("{e:#}")))?;
    let key = private_key().map_err(protocol)?;
    let lan = if std::env::var_os("DISPATCH_PEER_FORCE_INTERNET").is_some() { vec![] } else { mdns::seen(&l.id).map(|s| s.socket_addrs()).unwrap_or_default() };
    let (mut conn, route) = match connect(&lan, &key) {
        Ok(c) => (c, "lan"),
        Err(_) => {
            let addr = net::peer_addr(&l.iroh, l.addr.as_ref()).map_err(|e| PeerError::Refused(format!("{e:#}")))?;
            let (s, route) = net::dial(addr).map_err(|e| PeerError::Offline(format!("{e:#}")))?;
            let _ = s.set_read_timeout(Some(IO_TIMEOUT));
            let _ = s.set_write_timeout(Some(IO_TIMEOUT));
            (wire::initiate(s, &key).map_err(protocol)?, route.code())
        }
    };
    if hex::encode(&conn.remote_key) != l.noise.to_lowercase() {
        return Err(PeerError::Refused("the machine that answered is not the one that made the link".into()));
    }
    let p = finish_pairing(&mut conn, &l.secret, false, None)?;
    super::note_route(&p.id, route);
    Ok(p)
}
