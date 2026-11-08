//! The internet path to a peer, through iroh.
//!
//! When the local network cannot reach a paired machine, it is dialled by its iroh endpoint
//! id: iroh punches a direct path through the routers when it can and goes through a relay
//! when it cannot. The connection carries nothing of its own: on the accepting side its
//! bytes are piped to this machine's local listener, on the dialling side to a loopback
//! socket, so the Noise handshake, the key pinning and the pairing proof run unchanged inside
//! it, end to end. A relay only ever forwards QUIC packets that are themselves encrypted, and
//! inside them our own encrypted stream.
//!
//! Two keys: the listener's iroh key is stable (`peer.iroh.key`, 0600); it is also the key
//! letters are signed with. Processes that only dial (the command line) use a fresh key each
//! time and publish nothing, so they never take over the listener's published address.
//!
//! Settings: `peer_internet` (default on) turns this path off entirely; `peer_relay` names a
//! relay of the user's own (an `iroh-relay` server), in which case nothing is published to or
//! looked up from n0's services and peers are dialled by the address they gave at pairing.

use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const ALPN: &[u8] = b"dispatch/peer/1";
const DIAL_TIMEOUT: Duration = Duration::from_secs(8);

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("dispatch-net").enable_all().build().expect("a tokio runtime"))
}

/// Run a future to completion from synchronous code, on a thread of its own so it never
/// meets another runtime's context (the app calls in from tokio's blocking pool).
fn run<F, T>(f: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(move || rt().block_on(f)).join().expect("the network thread")
}

/// Whether the internet path is on (`peer_internet`, default on; `DISPATCH_NO_INTERNET` forces off).
pub fn enabled() -> bool {
    std::env::var_os("DISPATCH_NO_INTERNET").is_none() && crate::settings::get("peer_internet").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// The user's own relay, when one is set.
pub fn relay_url() -> Option<String> {
    crate::settings::get("peer_relay").and_then(|v| v.as_str().map(str::to_string)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The HTTP proxy relay connections go through: `HTTPS_PROXY` / `ALL_PROXY`, else the macOS
/// system proxy (an app opened from Finder has no shell variables). On many networks (with a
/// VPN client in "TUN" mode, behind a company proxy) only proxied connections get out; a
/// relay connection is plain HTTPS, so it goes through the proxy like a browser's would.
pub fn proxy() -> Option<String> {
    for k in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(v) = std::env::var(k) {
            if let Some(u) = http_proxy_url(&v) {
                return Some(u);
            }
        }
    }
    system_proxy()
}

/// `host:port` or `http://host:port` as an http URL; SOCKS and anything else is not used.
fn http_proxy_url(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() || v.starts_with("socks") {
        return None;
    }
    let u = if v.contains("://") { v.to_string() } else { format!("http://{v}") };
    url::Url::parse(&u).ok().filter(|p| p.scheme() == "http" || p.scheme() == "https").map(|_| u)
}

#[cfg(target_os = "macos")]
fn system_proxy() -> Option<String> {
    let out = std::process::Command::new("scutil").arg("--proxy").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(&format!("{k} : ")).map(|s| s.trim().to_string()));
    for (enable, host, port) in [("HTTPSEnable", "HTTPSProxy", "HTTPSPort"), ("HTTPEnable", "HTTPProxy", "HTTPPort")] {
        if field(enable).as_deref() == Some("1") {
            if let (Some(h), Some(p)) = (field(host), field(port)) {
                return http_proxy_url(&format!("{h}:{p}"));
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn system_proxy() -> Option<String> {
    None
}

fn with_proxy(b: iroh::endpoint::Builder) -> iroh::endpoint::Builder {
    match proxy().and_then(|p| url::Url::parse(&p).ok()) {
        Some(u) => b.proxy_url(u),
        None => b,
    }
}

fn key_path() -> PathBuf {
    super::root().join("peer.iroh.key")
}

/// The stable iroh key: the listener's identity and the key letters are signed with.
pub(crate) fn secret() -> Result<SecretKey> {
    let _lock = super::persistence::lock()?;
    super::persistence::load_locked()?;
    let path = key_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bytes = hex::decode(text.trim()).context("peer.iroh.key is not hex; file preserved")?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("peer.iroh.key is not 32 bytes; file preserved"))?;
            Ok(SecretKey::from_bytes(&arr))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            super::persistence::create_private(&path, hex::encode(key.to_bytes()).as_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e).context("cannot read peer.iroh.key; existing key preserved"),
    }
}

/// This machine's iroh endpoint id (also its signing key), as text.
pub fn my_id() -> Option<String> {
    secret().ok().map(|k| k.public().to_string())
}

fn addr_path() -> PathBuf {
    super::state_dir().join("iroh.json")
}

/// `my_addr`, waiting up to `wait` for the listener to have written one that names a way in.
pub fn my_addr_ready(wait: Duration) -> Option<Value> {
    let end = std::time::Instant::now() + wait;
    loop {
        let a = my_addr();
        let usable = a.as_ref().and_then(|v| v["addrs"].as_array()).map(|x| !x.is_empty()).unwrap_or(false);
        if usable || std::time::Instant::now() >= end {
            return a;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Where the listener can be reached over the internet right now, as the listener last wrote
/// it (relay and direct addresses); None when no listener runs here or the path is off.
pub fn my_addr() -> Option<Value> {
    if !enabled() {
        return None;
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(addr_path()).ok()?).ok()?;
    let fresh = v["at"].as_i64().map(|at| chrono::Utc::now().timestamp() - at < 120).unwrap_or(false);
    fresh.then(|| v["addr"].clone())
}

fn relay_mode() -> Result<Option<RelayMode>> {
    Ok(match relay_url() {
        Some(u) => Some(RelayMode::Custom(RelayMap::from(RelayUrl::from_str(&u).map_err(|e| anyhow!("relay address {u}: {e}"))?))),
        None => None,
    })
}

static LISTENER: Mutex<Option<Endpoint>> = Mutex::new(None);
static DIALER: Mutex<Option<Endpoint>> = Mutex::new(None);

/// Accept peers over the internet and pipe each stream to the local listener at `local_port`.
/// Returns this machine's endpoint id.
pub(crate) fn serve(local_port: u16) -> Result<String> {
    if !enabled() {
        bail!("internet connections are off");
    }
    let key = secret()?;
    let mode = relay_mode()?;
    let ep = run(async move {
        let b = match mode {
            // the user's own relay: nothing published to or looked up from n0
            Some(m) => Endpoint::builder(presets::Minimal).relay_mode(m),
            None => Endpoint::builder(presets::N0),
        };
        let b = if std::env::var_os("DISPATCH_IROH_RELAY_ONLY").is_some() { b.clear_ip_transports() } else { b };
        with_proxy(b).secret_key(key).alpns(vec![ALPN.to_vec()]).bind().await
    })
    .map_err(|e| anyhow!("{e}"))?;
    let id = ep.id().to_string();
    *LISTENER.lock().unwrap() = Some(ep.clone());
    let accepting = ep.clone();
    rt().spawn(async move { accept_loop(accepting, local_port).await });
    let publishing = ep;
    rt().spawn(async move {
        // the address is only worth giving out once the home relay is connected
        let _ = tokio::time::timeout(Duration::from_secs(20), publishing.online()).await;
        loop {
            write_addr(&publishing);
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    Ok(id)
}

fn write_addr(ep: &Endpoint) {
    let v = serde_json::json!({ "id": ep.id().to_string(), "addr": ep.addr(), "at": chrono::Utc::now().timestamp() });
    let _ = std::fs::create_dir_all(super::state_dir());
    let _ = std::fs::write(addr_path(), serde_json::to_string(&v).unwrap_or_default());
}

async fn accept_loop(ep: Endpoint, local_port: u16) {
    while let Some(incoming) = ep.accept().await {
        tokio::spawn(async move {
            let Ok(accepting) = incoming.accept() else { return };
            let Ok(conn) = accepting.await else { return };
            // each bidirectional stream carries one exchange of our own wire
            while let Ok((send, recv)) = conn.accept_bi().await {
                let Ok(mut tcp) = tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await else { break };
                let mut quic = tokio::io::join(recv, send);
                let _ = tokio::io::copy_bidirectional(&mut quic, &mut tcp).await;
            }
        });
    }
}

fn dialer() -> Result<Endpoint> {
    if let Some(ep) = LISTENER.lock().unwrap().clone() {
        return Ok(ep);
    }
    if let Some(ep) = DIALER.lock().unwrap().clone() {
        return Ok(ep);
    }
    let mode = relay_mode()?;
    let ep = run(async move {
        let b = Endpoint::builder(presets::Minimal).secret_key(SecretKey::generate());
        let b = match mode {
            Some(m) => b.relay_mode(m),
            // resolve peers through n0, publish nothing about ourselves
            None => b
                .relay_mode(RelayMode::Default)
                .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
                .address_lookup(iroh::address_lookup::DnsAddressLookup::n0_dns()),
        };
        let b = if std::env::var_os("DISPATCH_IROH_RELAY_ONLY").is_some() { b.clear_ip_transports() } else { b };
        with_proxy(b).bind().await
    })
    .map_err(|e| anyhow!("{e}"))?;
    *DIALER.lock().unwrap() = Some(ep.clone());
    Ok(ep)
}

/// How a connection got through: straight between the two machines, or by way of a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Direct,
    Relay,
}

impl Route {
    pub fn code(self) -> &'static str {
        match self {
            Route::Direct => "direct",
            Route::Relay => "relay",
        }
    }
}

/// The address to dial a peer at: its id, with the relay and addresses it last gave.
pub fn peer_addr(id: &str, addr: Option<&Value>) -> Result<EndpointAddr> {
    let id = EndpointId::from_str(id.trim()).map_err(|e| anyhow!("bad endpoint id: {e}"))?;
    if let Some(a) = addr.and_then(|a| serde_json::from_value::<EndpointAddr>(a.clone()).ok()).filter(|a| a.id == id) {
        return Ok(a);
    }
    Ok(EndpointAddr::new(id))
}

/// Open a connection to a peer and hand back a loopback socket that carries it, plus the
/// route it took. The socket is the far end of a pipe into one QUIC stream.
pub(crate) fn dial(addr: EndpointAddr) -> Result<(std::net::TcpStream, Route)> {
    if !enabled() {
        bail!("internet connections are off");
    }
    let ep = dialer()?;
    dial_on(ep, addr)
}

fn dial_on(ep: Endpoint, addr: EndpointAddr) -> Result<(std::net::TcpStream, Route)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    let (tx, rx) = std::sync::mpsc::channel::<Result<Route, String>>();
    rt().spawn(async move {
        let conn = match tokio::time::timeout(DIAL_TIMEOUT, ep.connect(addr, ALPN)).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
            Err(_) => {
                let _ = tx.send(Err(format!("no answer within {}s", DIAL_TIMEOUT.as_secs())));
                return;
            }
        };
        let (send, recv) = match conn.open_bi().await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
        };
        let route = if conn.paths().iter().any(|p| p.is_selected() && p.is_relay()) { Route::Relay } else { Route::Direct };
        let _ = tx.send(Ok(route));
        let Ok(l) = tokio::net::TcpListener::from_std(listener) else { return };
        if let Ok(Ok((mut tcp, _))) = tokio::time::timeout(Duration::from_secs(5), l.accept()).await {
            let mut quic = tokio::io::join(recv, send);
            let _ = tokio::io::copy_bidirectional(&mut quic, &mut tcp).await;
        }
        conn.close(0u32.into(), b"done");
    });
    let route = rx.recv_timeout(DIAL_TIMEOUT + Duration::from_secs(2)).map_err(|_| anyhow!("no answer"))?.map_err(|e| anyhow!("{e}"))?;
    let stream = std::net::TcpStream::connect_timeout(&local, Duration::from_secs(2))?;
    Ok((stream, route))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Live: bind with n0's relays and report the home relay status. Run by hand:
    /// `cargo test -p dispatch-core net_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn net_probe() {
        use iroh::Watcher as _;
        let _ = tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "iroh=info,iroh_relay=debug,iroh::net_report=debug".into())).with_writer(std::io::stderr).try_init();
        let out = run(async {
            let ep = with_proxy(Endpoint::builder(presets::N0)).bind().await.unwrap();
            let online = tokio::time::timeout(Duration::from_secs(25), ep.online()).await.is_ok();
            let mut status = ep.home_relay_status();
            (online, format!("{:?}", status.get()), format!("{:?}", ep.addr()))
        });
        println!("proxy: {:?}\nonline: {}\nhome relay: {}\naddr: {}", proxy(), out.0, out.1, out.2);
    }

    /// Two endpoints on this machine, relays off: a stream written on one side comes out of the
    /// local listener the other side pipes to, and the answer comes back.
    #[test]
    fn a_stream_is_piped_through_to_the_local_listener_and_back() {
        // the "local listener": echoes what it reads, upper-cased
        let local = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = local.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = local.accept().unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).unwrap();
            s.write_all(&buf.to_ascii_uppercase()).unwrap();
        });
        let (server, server_addr) = run(async {
            let ep = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
            let sock = ep.bound_sockets().into_iter().find(|a| a.is_ipv4()).unwrap();
            let addr = EndpointAddr::new(ep.id()).with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], sock.port())));
            (ep, addr)
        });
        let accepting = server.clone();
        rt().spawn(async move { accept_loop(accepting, port).await });
        let client = run(async { Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap() });
        let (mut s, route) = dial_on(client, server_addr).unwrap();
        assert_eq!(route, Route::Direct);
        s.write_all(b"hello").unwrap();
        let mut back = [0u8; 5];
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"HELLO");
    }
}
