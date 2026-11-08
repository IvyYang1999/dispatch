//! The listener: accepts peers, answers their requests, and runs the mDNS side.

use super::wire::{self, Conn, RemoteCopy, RemoteSession, Request, Response};
use super::{mdns, me, net, peer_by_key, private_key, remember, sign, take_pairing, PeerInfo, DEFAULT_PORT};
use crate::i18n::t;
use crate::store::{self, Letter, Store};
use crate::{discover, hook, ops, reach, send};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 256 * 1024;

pub struct ServerHandle {
    pub port: u16,
    pub mdns: bool,
    /// This machine's iroh endpoint id when the internet path listens too.
    pub internet: Option<String>,
}

fn bind(pref: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", pref)).or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
}

/// Start listening and advertising in background threads. `pref` 0 means the default port;
/// a taken port falls back to a free one.
pub fn serve(pref: u16) -> anyhow::Result<ServerHandle> {
    let pref = if pref == 0 { DEFAULT_PORT } else { pref };
    let listener = bind(pref)?;
    let port = listener.local_addr()?.port();
    let key = private_key()?;
    std::thread::Builder::new().name("dispatch-peer-accept".into()).spawn(move || {
        for s in listener.incoming() {
            let Ok(s) = s else { continue };
            let key = key.clone();
            let _ = std::thread::Builder::new().name("dispatch-peer-conn".into()).spawn(move || {
                let _ = handle_stream(s, &key);
            });
        }
    })?;
    let mdns_on = match mdns::start(port) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("dispatch: mDNS is off: {e:#}");
            mdns::note_serving(port, false);
            false
        }
    };
    let internet = if net::enabled() {
        match net::serve(port) {
            Ok(id) => Some(id),
            Err(e) => {
                eprintln!("dispatch: the internet path is off: {e:#}");
                None
            }
        }
    } else {
        None
    };
    Ok(ServerHandle { port, mdns: mdns_on, internet })
}

fn handle_stream(s: TcpStream, key: &[u8]) -> anyhow::Result<()> {
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    let _ = s.set_nodelay(true);
    let mut conn = wire::respond(s, key)?;
    serve_conn(&mut conn)
}

/// Answer what one connection asks: the key exchange first when it comes, then one request.
pub(crate) fn serve_conn(conn: &mut Conn) -> anyhow::Result<()> {
    for _ in 0..2 {
        let req: Request = conn.recv()?;
        let keys = matches!(req, Request::Keys { .. });
        let resp = handle(conn, req);
        let refused = matches!(resp, Response::Error { .. });
        conn.send(&resp)?;
        if !keys || refused {
            break;
        }
    }
    Ok(())
}

/// Answer one request from an authenticated connection.
pub(crate) fn handle(conn: &Conn, req: Request) -> Response {
    let me = match me() { Ok(me) => me, Err(e) => return Response::Error { message: format!("{e:#}") } };
    match req {
        Request::Hello => Response::Welcome { id: me.id, name: me.name, version: env!("CARGO_PKG_VERSION").into(), iroh: net::my_id() },
        Request::Pair { proof, id, name, iroh, iroh_addr } => {
            if id.trim().is_empty() || name.trim().is_empty() {
                return Response::Error { message: "pairing needs an id and a name".into() };
            }
            match take_pairing(|code| wire::proof_ok(code, &conn.handshake_hash, &proof)) {
                Ok(Some(kind)) => {
                    let p = PeerInfo {
                        id: id.trim().to_string(),
                        name: name.trim().chars().take(48).collect(),
                        public_key: hex::encode(&conn.remote_key),
                        address: None,
                        paired_at: store::now_text(),
                        iroh_id: iroh,
                        iroh_addr,
                        // a code is shown and typed in the same room; a link travels, so its
                        // pairing waits for the fingerprints to be compared
                        verified: kind != "link",
                        rule: "all".into(),
                    };
                    match remember(p) {
                        Ok(()) => Response::Paired { id: me.id, name: me.name, iroh: net::my_id(), iroh_addr: net::my_addr() },
                        Err(e) => Response::Error { message: format!("{e:#}") },
                    }
                }
                Ok(None) => Response::Error { message: "the pairing code did not match".into() },
                Err(e) => Response::Error { message: e.into() },
            }
        }
        other => {
            let peer = match peer_by_key(&conn.remote_key) {
                Ok(Some(peer)) => peer,
                Ok(None) => return Response::Error { message: "not paired with this machine".into() },
                Err(e) => return Response::Error { message: format!("{e:#}") },
            };
            let store = Store::open();
            match other {
                Request::Sessions => {
                    let hook_on = hook::installed();
                    let sessions = discover::local().into_iter().map(|s| RemoteSession { reach: reach::decide(&s, hook_on), session: s }).collect();
                    Response::Sessions { hook_installed: hook_on, sessions }
                }
                Request::Keys { iroh, iroh_addr } => match super::pin_keys(&peer.id, iroh, iroh_addr) {
                    Ok(()) => Response::Keys { iroh: net::my_id(), iroh_addr: net::my_addr() },
                    Err(e) => Response::Error { message: e.to_string() },
                },
                Request::Deliver { letter, signature } => deliver_in(&store, &peer, letter, signature),
                Request::Status { dispatch_ids } => {
                    let key = net::secret().ok();
                    let mut copies = vec![];
                    for did in dispatch_ids.iter().take(200) {
                        for l in store.copies(did).unwrap_or_default() {
                            // only what concerns the asking machine
                            if l.from_host.as_deref() != Some(&peer.id) && l.to_host.as_deref() != Some(&peer.id) {
                                continue;
                            }
                            let (kind, stage_key) = ops::stage_of(&store, &l);
                            // a letter that started here (a reply) carries this machine's signature
                            let signature = if l.from_host.is_none() { key.as_ref().map(|k| sign::sign(k, &l)) } else { None };
                            copies.push(RemoteCopy { letter: l, kind, stage_key, signature });
                        }
                    }
                    Response::Status { copies }
                }
                Request::Cancel { letter_id } => {
                    let found = store.list(None, false).ok().and_then(|all| all.into_iter().find(|l| l.id == letter_id));
                    if let Some(mut l) = found {
                        if l.from_host.as_deref() == Some(&peer.id) && (store::pending(&l.status) || l.status == store::HELD) {
                            let _ = store.set_status(&mut l, store::CANCELLED);
                            let _ = store.append_run(&l.id, "cancel", "by", &format!("peer {}", peer.name));
                        }
                    }
                    Response::Ok
                }
                Request::Transcript { session_id, limit } => {
                    let turns = discover::lookup(&session_id).map(|s| crate::transcript::recent(&s, limit.min(100))).unwrap_or_default();
                    Response::Transcript { turns }
                }
                Request::Hello | Request::Pair { .. } => Response::Error { message: "unexpected".into() },
            }
        }
    }
}

fn ok_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// At most this many letters from one machine within `RATE_WINDOW`.
const RATE_MAX: usize = 30;
const RATE_WINDOW: Duration = Duration::from_secs(300);
static RATE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>> = std::sync::LazyLock::new(Default::default);

fn rate_ok(peer: &str) -> bool {
    let now = std::time::Instant::now();
    let mut all = RATE.lock().unwrap();
    let q = all.entry(peer.to_string()).or_default();
    while q.front().map(|t| now.duration_since(*t) > RATE_WINDOW).unwrap_or(false) {
        q.pop_front();
    }
    if q.len() >= RATE_MAX {
        return false;
    }
    q.push_back(now);
    true
}

fn refuse(peer: &PeerInfo, letter: &Letter, why: &str) -> Response {
    super::log_problem(&peer.name, &format!("refused letter {}: {why}", crate::model::short_id(&letter.id)));
    Response::Error { message: why.to_string() }
}

/// A letter from a peer for a session here: check its signature against the key pinned for
/// that machine, apply the machine's receiving rule, store it with the peer as its origin, and
/// run the ladder (or hold it for a person to approve). The same letter again (a retry) is
/// delivered again only while it is still waiting.
fn deliver_in(store: &Store, peer: &PeerInfo, mut letter: Letter, signature: Option<String>) -> Response {
    if !ok_id(&letter.id) || !ok_id(&letter.dispatch_id) || letter.to.is_empty() || letter.to.len() > 128 {
        return Response::Error { message: "bad letter ids".into() };
    }
    if letter.body.len() > MAX_BODY {
        return Response::Error { message: "body too large".into() };
    }
    let Some(signer) = peer.iroh_id.as_deref() else { return refuse(peer, &letter, "no signing key is known for that machine yet; update Dispatch on both machines") };
    let Some(signature) = signature else { return refuse(peer, &letter, "the letter is not signed; refused") };
    if !sign::verify(signer, &letter, &signature) {
        return refuse(peer, &letter, "the letter's signature does not match the sending machine's key; refused");
    }
    if !rate_ok(&peer.id) {
        return refuse(peer, &letter, "too many messages from that machine in the last few minutes; refused");
    }
    if let Ok(all) = store.list(None, false) {
        if let Some(mut existing) = all.into_iter().find(|l| l.id == letter.id) {
            if existing.status == store::HELD {
                return Response::Delivered { status: existing.status.clone(), route: "stored".into(), detail: t("send.peer_held").into() };
            }
            if store::pending(&existing.status) {
                // this machine's own sweep may have just tried: no second attempt within five minutes
                let recent = store.runs(&existing.id).last().and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.at).ok()).map(|at| (chrono::Local::now().fixed_offset() - at).num_seconds() < 300).unwrap_or(false);
                if recent {
                    return Response::Delivered { status: existing.status.clone(), route: "stored".into(), detail: t("send.peer_recent").into() };
                }
                return match send::redeliver(store, &mut existing) {
                    Ok(rep) => Response::Delivered { status: existing.status, route: rep.route, detail: rep.detail },
                    Err(e) => Response::Error { message: format!("{e:#}") },
                };
            }
            return Response::Delivered { status: existing.status.clone(), route: "inbox".into(), detail: t("send.peer_already").into() };
        }
    }
    if peer.rule == "contacts" && crate::contacts::for_session(&letter.to, None).is_none() {
        return refuse(peer, &letter, &t("send.peer_contacts_only").replace("{host}", &match me() { Ok(m) => m.name, Err(e) => return Response::Error { message: format!("{e:#}") } }));
    }
    let hold = !peer.verified || peer.rule == "ask";
    letter.from_host = Some(peer.id.clone());
    letter.from_host_name = Some(peer.name.clone());
    letter.to_host = None;
    letter.to_host_name = None;
    letter.signature = Some(signature);
    letter.status = if hold { store::HELD.into() } else { store::UNREAD.into() };
    letter.path = String::new();
    letter.wake = true;
    letter.urgency = store::urgency_stored(&letter.urgency).into();
    if letter.created.is_empty() {
        letter.created = store::now_text();
    }
    if letter.deadline.is_none() {
        letter.deadline = Some(store::deadline_text(&letter.created, store::default_wait(&letter.urgency)));
    }
    if let Err(e) = store.write_letter(&mut letter) {
        return Response::Error { message: format!("{e:#}") };
    }
    let _ = store.append_run(&letter.id, "peer", "received", &format!("{}; signature verified", peer.name));
    if hold {
        let why = if !peer.verified { "machine not confirmed yet" } else { "rule: ask first" };
        let _ = store.append_run(&letter.id, "peer", "held", why);
        return Response::Delivered { status: store::HELD.into(), route: "stored".into(), detail: t("send.peer_held").into() };
    }
    let to = send::target_of(&letter);
    match send::deliver(store, &to, &mut letter, true, false) {
        Ok(rep) => Response::Delivered { status: letter.status.clone(), route: rep.route, detail: rep.detail },
        Err(e) => Response::Error { message: format!("{e:#}") },
    }
}
