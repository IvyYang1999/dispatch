//! The listener: accepts peers, answers their requests, and runs the mDNS side.

use super::wire::{self, Conn, RemoteCopy, RemoteSession, Request, Response};
use super::{mdns, me, peer_by_key, private_key, remember, take_pairing, PeerInfo, DEFAULT_PORT};
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
    Ok(ServerHandle { port, mdns: mdns_on })
}

fn handle_stream(s: TcpStream, key: &[u8]) -> anyhow::Result<()> {
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    let _ = s.set_nodelay(true);
    let mut conn = wire::respond(s, key)?;
    let req: Request = conn.recv()?;
    let resp = handle(&conn, req);
    conn.send(&resp)
}

/// Answer one request from an authenticated connection.
pub(crate) fn handle(conn: &Conn, req: Request) -> Response {
    let me = me();
    match req {
        Request::Hello => Response::Welcome { id: me.id, name: me.name, version: env!("CARGO_PKG_VERSION").into() },
        Request::Pair { proof, id, name } => {
            if id.trim().is_empty() || name.trim().is_empty() {
                return Response::Error { message: "pairing needs an id and a name".into() };
            }
            match take_pairing(|code| wire::proof_ok(code, &conn.handshake_hash, &proof)) {
                Ok(true) => {
                    let p = PeerInfo { id: id.trim().to_string(), name: name.trim().chars().take(48).collect(), public_key: hex::encode(&conn.remote_key), address: None, paired_at: store::now_text() };
                    match remember(p) {
                        Ok(()) => Response::Paired { id: me.id, name: me.name },
                        Err(e) => Response::Error { message: format!("{e:#}") },
                    }
                }
                Ok(false) => Response::Error { message: "the pairing code did not match".into() },
                Err(e) => Response::Error { message: e.into() },
            }
        }
        other => {
            let Some(peer) = peer_by_key(&conn.remote_key) else { return Response::Error { message: "not paired with this machine".into() } };
            let store = Store::open();
            match other {
                Request::Sessions => {
                    let hook_on = hook::installed();
                    let sessions = discover::local().into_iter().map(|s| RemoteSession { reach: reach::decide(&s, hook_on), session: s }).collect();
                    Response::Sessions { hook_installed: hook_on, sessions }
                }
                Request::Deliver { letter } => deliver_in(&store, &peer, letter),
                Request::Status { dispatch_ids } => {
                    let mut copies = vec![];
                    for did in dispatch_ids.iter().take(200) {
                        for l in store.copies(did).unwrap_or_default() {
                            // only what concerns the asking machine
                            if l.from_host.as_deref() != Some(&peer.id) && l.to_host.as_deref() != Some(&peer.id) {
                                continue;
                            }
                            let (kind, stage_key) = ops::stage_of(&store, &l);
                            copies.push(RemoteCopy { letter: l, kind, stage_key });
                        }
                    }
                    Response::Status { copies }
                }
                Request::Cancel { letter_id } => {
                    let found = store.list(None, false).ok().and_then(|all| all.into_iter().find(|l| l.id == letter_id));
                    if let Some(mut l) = found {
                        if l.from_host.as_deref() == Some(&peer.id) && store::pending(&l.status) {
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

/// A letter from a peer for a session here: store it with the peer as its origin, run the
/// ladder. The same letter again (a retry) is delivered again only while it is still waiting.
fn deliver_in(store: &Store, peer: &PeerInfo, mut letter: Letter) -> Response {
    if !ok_id(&letter.id) || !ok_id(&letter.dispatch_id) || letter.to.is_empty() || letter.to.len() > 128 {
        return Response::Error { message: "bad letter ids".into() };
    }
    if letter.body.len() > MAX_BODY {
        return Response::Error { message: "body too large".into() };
    }
    if let Ok(all) = store.list(None, false) {
        if let Some(mut existing) = all.into_iter().find(|l| l.id == letter.id) {
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
    letter.from_host = Some(peer.id.clone());
    letter.from_host_name = Some(peer.name.clone());
    letter.to_host = None;
    letter.to_host_name = None;
    letter.status = store::UNREAD.into();
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
    let _ = store.append_run(&letter.id, "peer", "received", &peer.name);
    let to = send::target_of(&letter);
    match send::deliver(store, &to, &mut letter, true, false) {
        Ok(rep) => Response::Delivered { status: letter.status.clone(), route: rep.route, detail: rep.detail },
        Err(e) => Response::Error { message: format!("{e:#}") },
    }
}
