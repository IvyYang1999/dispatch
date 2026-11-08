//! Every letter that crosses to another machine is signed by the machine that sends it and
//! checked against the key pinned for that machine before it is stored. The connection is
//! already encrypted and authenticated; the signature is a second, independent lock: a
//! letter whose bytes changed anywhere on the way never reaches a session.
//!
//! What is signed: the fields that make up the message (ids, sender, recipient, urgency,
//! times, body), in a fixed form. What is not: the delivery state and the bookkeeping each
//! machine writes for itself (status, file path, which host it is on).

use crate::store::Letter;
use iroh::{PublicKey, SecretKey, Signature};
use serde::Serialize;
use std::str::FromStr;

#[derive(Serialize)]
struct Signed<'a> {
    v: u8,
    id: &'a str,
    dispatch_id: &'a str,
    from: &'a str,
    from_name: &'a str,
    from_harness: Option<&'a str>,
    to: &'a str,
    to_name: &'a str,
    to_harness: Option<&'a str>,
    urgency: &'a str,
    created: &'a str,
    deadline: Option<&'a str>,
    body: &'a str,
}

/// The bytes a signature covers.
pub fn canonical(l: &Letter) -> Vec<u8> {
    serde_json::to_vec(&Signed {
        v: 1,
        id: &l.id,
        dispatch_id: &l.dispatch_id,
        from: &l.from,
        from_name: &l.from_name,
        from_harness: l.from_harness.as_deref(),
        to: &l.to,
        to_name: &l.to_name,
        to_harness: l.to_harness.as_deref(),
        urgency: crate::store::urgency_stored(&l.urgency),
        created: &l.created,
        deadline: l.deadline.as_deref(),
        // the store keeps bodies without trailing whitespace
        body: l.body.trim_end(),
    })
    .unwrap_or_default()
}

pub fn sign(key: &SecretKey, l: &Letter) -> String {
    hex::encode(key.sign(&canonical(l)).to_bytes())
}

/// Whether `signature` is `signer`'s signature over this letter.
pub fn verify(signer: &str, l: &Letter, signature: &str) -> bool {
    let Ok(pk) = PublicKey::from_str(signer.trim()) else { return false };
    let Ok(bytes) = hex::decode(signature.trim()) else { return false };
    let Ok(arr) = <[u8; 64]>::try_from(bytes.as_slice()) else { return false };
    pk.verify(&canonical(l), &Signature::from_bytes(&arr)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{parse, render, UNREAD};

    fn letter() -> Letter {
        Letter {
            id: "a1".into(),
            dispatch_id: "d1".into(),
            from: "11111111-2222-4333-8444-555555555555".into(),
            from_name: "Release".into(),
            to: "66666666-7777-4888-9999-aaaaaaaaaaaa".into(),
            to_name: "Dispatch dev".into(),
            urgency: "普通".into(),
            wake: true,
            status: UNREAD.into(),
            created: "2026-11-08 17:00".into(),
            deadline: Some("2026-11-09 17:00".into()),
            to_harness: Some("claude".into()),
            from_host: None,
            from_host_name: None,
            to_host: Some("m-1".into()),
            to_host_name: Some("Mac".into()),
            from_harness: Some("codex".into()),
            signature: None,
            body: "ship it\n".into(),
            path: String::new(),
        }
    }

    #[test]
    fn a_signature_holds_through_the_store_and_breaks_on_any_change() {
        let key = SecretKey::generate();
        let me = key.public().to_string();
        let l = letter();
        let sig = sign(&key, &l);
        assert!(verify(&me, &l, &sig));
        // the receiver rewrites its own bookkeeping and stores it: still valid
        let mut there = parse(&render(&l)).unwrap();
        there.to_host = None;
        there.from_host = Some("m-2".into());
        there.status = "已读（hook代标）".into();
        assert!(verify(&me, &there, &sig));
        // any change to the message is caught
        for change in [
            |l: &mut Letter| l.body = "ship it now".into(),
            |l: &mut Letter| l.to = "66666666-7777-4888-9999-bbbbbbbbbbbb".into(),
            |l: &mut Letter| l.from_name = "Someone else".into(),
            |l: &mut Letter| l.urgency = "紧急".into(),
        ] {
            let mut bad = l.clone();
            change(&mut bad);
            assert!(!verify(&me, &bad, &sig));
        }
        // another machine's key, a mangled signature
        assert!(!verify(&SecretKey::generate().public().to_string(), &l, &sig));
        assert!(!verify(&me, &l, &sig.replace('a', "b")));
        assert!(!verify(&me, &l, "zz"));
    }
}
