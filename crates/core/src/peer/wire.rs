//! The wire between two machines: length-framed TCP, a Noise XX handshake, then JSON
//! requests and responses inside the encrypted channel. One request per connection.
//!
//! The handshake leaves both sides holding the other's static public key; `peers.json`
//! says whether that key belongs to a paired machine. Pairing proves a six-digit code
//! with an HMAC over the handshake hash, so the proof is good for this connection only.

use crate::model::Session;
use crate::reach::Reach;
use crate::store::Letter;
use anyhow::{anyhow, Context, Result};
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::{Read, Write};
use std::net::TcpStream;

pub const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const PROLOGUE: &[u8] = b"dispatch/1";
/// A message larger than this is refused before it is read.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
/// Plaintext per encrypted frame (a Noise message holds at most 65535 bytes with its tag).
const CHUNK: usize = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Who are you? Open to anyone.
    Hello,
    /// Pair with me: the proof of the code (or link secret) you show, and who I am, with my
    /// iroh endpoint id (also my signing key) and where to reach me over the internet.
    Pair {
        proof: String,
        id: String,
        name: String,
        #[serde(default)]
        iroh: Option<String>,
        #[serde(default)]
        iroh_addr: Option<serde_json::Value>,
    },
    /// First on every connection between paired machines: my signing key and internet
    /// address; you answer with yours. Each side pins the other's key the first time and
    /// refuses a different one afterwards.
    Keys {
        #[serde(default)]
        iroh: Option<String>,
        #[serde(default)]
        iroh_addr: Option<serde_json::Value>,
    },
    /// The sessions on your machine, each with how a message would reach it: the recent ones,
    /// or with a query, every one of any age whose fields hold all of its words. A machine that
    /// predates the query ignores it and sends its recent list.
    Sessions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
    },
    /// A letter for a session on your machine, with my signature over it; store it and run
    /// your ladder.
    Deliver {
        letter: Letter,
        #[serde(default)]
        signature: Option<String>,
    },
    /// Your copies of these dispatches that concern me, with their stage.
    Status { dispatch_ids: Vec<String> },
    /// Withdraw a copy I sent you, if it is still waiting.
    Cancel { letter_id: String },
    /// The last turns of one of your sessions' records, for its page.
    Transcript { session_id: String, limit: usize },
    /// My project book (how the user files sessions), with every machine by its id. Keep
    /// whichever of ours is newer and answer with what you have now. A machine that predates
    /// this closes the connection, and is not asked again for a while.
    Projects { book: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Welcome {
        id: String,
        name: String,
        version: String,
        #[serde(default)]
        iroh: Option<String>,
    },
    Paired {
        id: String,
        name: String,
        #[serde(default)]
        iroh: Option<String>,
        #[serde(default)]
        iroh_addr: Option<serde_json::Value>,
    },
    Keys {
        #[serde(default)]
        iroh: Option<String>,
        #[serde(default)]
        iroh_addr: Option<serde_json::Value>,
    },
    Sessions { hook_installed: bool, sessions: Vec<RemoteSession> },
    Delivered { status: String, route: String, detail: String },
    Status { copies: Vec<RemoteCopy> },
    Transcript { turns: Vec<crate::transcript::Turn> },
    Projects { book: serde_json::Value },
    Ok,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSession {
    pub session: Session,
    pub reach: Reach,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCopy {
    pub letter: Letter,
    pub kind: String,
    pub stage_key: String,
    /// The answering machine's signature over a letter that started there (a reply).
    #[serde(default)]
    pub signature: Option<String>,
}

/// An encrypted connection after the handshake.
pub struct Conn {
    stream: TcpStream,
    noise: snow::TransportState,
    /// The other side's static public key.
    pub remote_key: Vec<u8>,
    /// Binds a pairing proof to this very connection.
    pub handshake_hash: Vec<u8>,
}

fn builder(key: &[u8]) -> Result<snow::Builder<'_>> {
    let params: snow::params::NoiseParams = PATTERN.parse().context("noise pattern")?;
    Ok(snow::Builder::new(params).local_private_key(key)?.prologue(PROLOGUE)?)
}

/// The client side of the handshake.
pub fn initiate(mut stream: TcpStream, key: &[u8]) -> Result<Conn> {
    let mut hs = builder(key)?.build_initiator()?;
    let mut buf = vec![0u8; 65535];
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut stream, &buf[..n])?;
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut stream, &buf[..n])?;
    finish(stream, hs)
}

/// The listener side of the handshake.
pub fn respond(mut stream: TcpStream, key: &[u8]) -> Result<Conn> {
    let mut hs = builder(key)?.build_responder()?;
    let mut buf = vec![0u8; 65535];
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    write_frame(&mut stream, &buf[..n])?;
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf)?;
    finish(stream, hs)
}

fn finish(stream: TcpStream, hs: snow::HandshakeState) -> Result<Conn> {
    let remote_key = hs.get_remote_static().ok_or_else(|| anyhow!("the other side sent no static key"))?.to_vec();
    let handshake_hash = hs.get_handshake_hash().to_vec();
    let noise = hs.into_transport_mode()?;
    Ok(Conn { stream, noise, remote_key, handshake_hash })
}

impl Conn {
    /// One JSON message: its length, then the bytes, every piece encrypted.
    pub fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let bytes = serde_json::to_vec(msg)?;
        if bytes.len() > MAX_MESSAGE {
            return Err(anyhow!("message too large ({} bytes)", bytes.len()));
        }
        self.write_encrypted(&(bytes.len() as u32).to_be_bytes())?;
        for chunk in bytes.chunks(CHUNK) {
            self.write_encrypted(chunk)?;
        }
        Ok(())
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        let head = self.read_encrypted()?;
        if head.len() != 4 {
            return Err(anyhow!("bad message header"));
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if len > MAX_MESSAGE {
            return Err(anyhow!("message too large ({len} bytes)"));
        }
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let part = self.read_encrypted()?;
            if part.is_empty() {
                return Err(anyhow!("short message"));
            }
            out.extend_from_slice(&part);
        }
        if out.len() != len {
            return Err(anyhow!("message length mismatch"));
        }
        Ok(serde_json::from_slice(&out)?)
    }

    fn write_encrypted(&mut self, plain: &[u8]) -> Result<()> {
        let mut buf = vec![0u8; plain.len() + 16];
        let n = self.noise.write_message(plain, &mut buf)?;
        write_frame(&mut self.stream, &buf[..n])
    }

    fn read_encrypted(&mut self) -> Result<Vec<u8>> {
        let m = read_frame(&mut self.stream)?;
        let mut buf = vec![0u8; m.len()];
        let n = self.noise.read_message(&m, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

fn write_frame(s: &mut TcpStream, b: &[u8]) -> Result<()> {
    if b.len() > u16::MAX as usize {
        return Err(anyhow!("frame too large"));
    }
    s.write_all(&(b.len() as u16).to_be_bytes())?;
    s.write_all(b)?;
    Ok(())
}

fn read_frame(s: &mut TcpStream) -> Result<Vec<u8>> {
    let mut h = [0u8; 2];
    s.read_exact(&mut h)?;
    let n = u16::from_be_bytes(h) as usize;
    let mut v = vec![0u8; n];
    s.read_exact(&mut v)?;
    Ok(v)
}

type HmacSha256 = Hmac<Sha256>;

/// The pairing proof: HMAC-SHA256 keyed with the code, over the handshake hash.
pub fn proof(code: &str, handshake_hash: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(code.as_bytes()).expect("hmac takes any key length");
    mac.update(handshake_hash);
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time check of a proof against the code this side shows.
pub fn proof_ok(code: &str, handshake_hash: &[u8], proof_hex: &str) -> bool {
    let Ok(given) = hex::decode(proof_hex.trim()) else { return false };
    let mut mac = HmacSha256::new_from_slice(code.as_bytes()).expect("hmac takes any key length");
    mac.update(handshake_hash);
    mac.verify_slice(&given).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn keypair() -> snow::Keypair {
        snow::Builder::new(PATTERN.parse().unwrap()).generate_keypair().unwrap()
    }

    #[test]
    fn a_search_reads_as_the_recent_list_on_an_older_machine() {
        // the request as 0.1.9 and earlier know it
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Before {
            Hello,
            Sessions,
        }
        let asked = serde_json::to_string(&Request::Sessions { query: Some("opentrends codex".into()) }).unwrap();
        assert!(matches!(serde_json::from_str::<Before>(&asked).unwrap(), Before::Sessions));
        assert!(matches!(serde_json::from_str::<Before>(r#"{"type":"hello"}"#).unwrap(), Before::Hello));
        // and an older machine's plain request asks for the recent list here
        assert!(matches!(serde_json::from_str::<Request>(r#"{"type":"sessions"}"#).unwrap(), Request::Sessions { query: None }));
        assert_eq!(serde_json::to_string(&Request::Sessions { query: None }).unwrap(), r#"{"type":"sessions"}"#);
    }

    #[test]
    fn handshake_carries_keys_and_messages_both_ways() {
        let a = keypair();
        let b = keypair();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let b_priv = b.private.clone();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut c = respond(s, &b_priv).unwrap();
            let req: Request = c.recv().unwrap();
            assert!(matches!(req, Request::Sessions { query: None }));
            // a response bigger than one frame, to exercise chunking
            let big = Response::Error { message: "x".repeat(150_000) };
            c.send(&big).unwrap();
            (c.remote_key.clone(), c.handshake_hash.clone())
        });
        let mut c = initiate(TcpStream::connect(addr).unwrap(), &a.private).unwrap();
        assert_eq!(c.remote_key, b.public);
        c.send(&Request::Sessions { query: None }).unwrap();
        let resp: Response = c.recv().unwrap();
        assert!(matches!(resp, Response::Error { message } if message.len() == 150_000));
        let (seen_key, server_hash) = server.join().unwrap();
        assert_eq!(seen_key, a.public, "the server learned the client's static key");
        assert_eq!(server_hash, c.handshake_hash, "both sides agree on the handshake hash");
        let p = proof("123456", &c.handshake_hash);
        assert!(proof_ok("123456", &server_hash, &p));
        assert!(!proof_ok("123457", &server_hash, &p));
        assert!(!proof_ok("123456", &[0u8; 32], &p), "a proof is bound to its handshake");
    }
}
