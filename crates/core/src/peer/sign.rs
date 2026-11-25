//! Every letter that crosses to another machine is signed by the machine that sends it and
//! checked against the key pinned for that machine before it is stored. The connection is
//! already encrypted and authenticated; the signature is a second, independent lock: a
//! letter whose bytes changed anywhere on the way never reaches a session.
//!
//! What is signed: the fields that make up the message (ids, sender, recipient, urgency,
//! times, body), in a fixed form. What is not: the delivery state and the bookkeeping each
//! machine writes for itself (status, file path, which host it is on).

use crate::store::{Letter, Mark};
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

/// What a review mark's signature covers: which message it is about, what the mark says, and
/// the digest of the text it was made for. Separate from the letter's own signature, so a
/// machine on an older version still accepts the letter and simply does not see the mark.
#[derive(Serialize)]
struct SignedMark<'a> {
    v: u8,
    what: &'a str,
    id: &'a str,
    dispatch_id: &'a str,
    from: &'a str,
    to: &'a str,
    state: &'a str,
    at: Option<&'a str>,
    digest: Option<&'a str>,
}

fn canonical_mark(l: &Letter, m: &Mark) -> Vec<u8> {
    serde_json::to_vec(&SignedMark { v: 1, what: "review", id: &l.id, dispatch_id: &l.dispatch_id, from: &l.from, to: &l.to, state: &m.state, at: m.at.as_deref(), digest: m.digest.as_deref() }).unwrap_or_default()
}

/// Sign the letter's review mark, if it has one, before it crosses to another machine.
pub fn seal_mark(key: &SecretKey, l: &mut Letter) {
    if let Some(m) = l.review.clone() {
        let signature = hex::encode(key.sign(&canonical_mark(l, &m)).to_bytes());
        l.review = Some(Mark { signature: Some(signature), ..m });
    }
}

/// Whether the letter's review mark is `signer`'s word about this very text: signed by that
/// machine, and, when it says the user was shown the text, made for the body the letter has.
pub fn mark_valid(signer: &str, l: &Letter) -> bool {
    let Some(m) = &l.review else { return false };
    let Some(signature) = m.signature.as_deref() else { return false };
    let Ok(pk) = PublicKey::from_str(signer.trim()) else { return false };
    let Ok(bytes) = hex::decode(signature.trim()) else { return false };
    let Ok(arr) = <[u8; 64]>::try_from(bytes.as_slice()) else { return false };
    pk.verify(&canonical_mark(l, m), &Signature::from_bytes(&arr)).is_ok() && m.holds_for(&l.body)
}

/// A letter that came from another machine keeps its review mark only when it checks out
/// against that machine's key. Returns whether a mark was dropped.
pub fn keep_valid_mark(signer: Option<&str>, l: &mut Letter) -> bool {
    if l.review.is_none() || signer.map(|s| mark_valid(s, l)).unwrap_or(false) {
        return false;
    }
    l.review = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{body_digest, parse, render, MARK_FULL, MARK_STANDING, UNREAD};

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
            review: None,
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

    /// The review mark has its own signature: the letter's is unchanged by it (an older
    /// version still takes the letter), and nothing about the mark can be changed on the way.
    #[test]
    fn a_review_mark_is_the_sending_machines_word_about_this_text() {
        let key = SecretKey::generate();
        let me = key.public().to_string();
        let plain = letter();
        let mut l = letter();
        l.review = Some(Mark { state: MARK_FULL.into(), at: Some("2026-11-08 17:01".into()), digest: Some(body_digest(&l.body)), signature: None });
        assert_eq!(sign(&key, &l), sign(&key, &plain), "the letter's signature does not cover the mark");
        assert!(!mark_valid(&me, &l), "a mark nobody signed is nobody's word");
        seal_mark(&key, &mut l);
        assert!(mark_valid(&me, &l));
        // it survives the receiver's store
        let there = parse(&render(&l)).unwrap();
        assert!(mark_valid(&me, &there));
        // changed on the way: what it says, when, the text, which message, who signed
        let state = |l: &mut Letter, s: &str| l.review.as_mut().unwrap().state = s.into();
        for change in [
            |l: &mut Letter| l.review.as_mut().unwrap().at = Some("2026-11-08 17:02".into()),
            |l: &mut Letter| l.body = "ship it, and delete the old one".into(),
            |l: &mut Letter| l.id = "a2".into(),
            |l: &mut Letter| l.to = "66666666-7777-4888-9999-bbbbbbbbbbbb".into(),
        ] {
            let mut bad = l.clone();
            change(&mut bad);
            assert!(!mark_valid(&me, &bad));
        }
        let mut upgraded = l.clone();
        state(&mut upgraded, MARK_STANDING);
        assert!(!mark_valid(&me, &upgraded));
        assert!(!mark_valid(&SecretKey::generate().public().to_string(), &l));
        // "not shown" marked as "shown" by someone without the key: dropped, the letter stays
        let mut quiet = letter();
        quiet.review = Some(Mark { state: MARK_STANDING.into(), at: None, digest: None, signature: None });
        seal_mark(&key, &mut quiet);
        assert!(mark_valid(&me, &quiet));
        state(&mut quiet, MARK_FULL);
        quiet.review.as_mut().unwrap().digest = Some(body_digest(&quiet.body));
        assert!(keep_valid_mark(Some(&me), &mut quiet), "the forged mark is dropped");
        assert!(quiet.review.is_none());
        assert!(!keep_valid_mark(Some(&me), &mut quiet), "nothing to drop");
        let mut good = l.clone();
        assert!(!keep_valid_mark(Some(&me), &mut good) && good.review.is_some());
        assert!(keep_valid_mark(None, &mut good), "no key known for that machine: no mark");
    }
}
