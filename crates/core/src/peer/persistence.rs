//! One cross-process transaction for identity initialization and every book mutation.
//! Read errors are not an empty installation. Never repair or rotate keys implicitly.

use super::{Book, Me};
use anyhow::{bail, Context, Result};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const RECOVERY: &str = "pairing state unavailable; existing files were preserved. Restore a matching peers.json and peer.key backup before retrying";

pub(super) fn lock() -> Result<File> {
    fs::create_dir_all(super::root()).context(RECOVERY)?;
    let f = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(super::root().join("peers.lock")).context(RECOVERY)?;
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(f),
            Err(TryLockError::WouldBlock) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            Err(TryLockError::WouldBlock) => bail!("pairing state is busy; no files changed; retry later"),
            Err(TryLockError::Error(e)) => return Err(e).context(RECOVERY),
        }
    }
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context(RECOVERY),
    }
}

pub(super) fn create_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path).context("create identity key without replacing an existing file")?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

pub(super) fn read_key() -> Result<Vec<u8>> {
    let text = fs::read_to_string(super::key_path()).context(RECOVERY)?;
    let bytes = hex::decode(text.trim()).context("peer.key is not valid hex; file preserved")?;
    if bytes.len() != 32 { bail!("peer.key is not a 32-byte key; file preserved"); }
    Ok(bytes)
}

/// The caller holds peers.lock, including throughout any subsequent mutation and save.
pub(super) fn load_locked() -> Result<Book> {
    let book = super::book_path();
    if !exists(&book)? {
        if exists(&super::key_path())? || exists(&super::root().join("peer.iroh.key"))? {
            bail!("{RECOVERY}: peers.json is missing beside an existing identity key");
        }
        let kp = super::new_keypair()?;
        let b = Book { me: Me { id: uuid::Uuid::new_v4().to_string(), name: super::default_name(), public_key: hex::encode(kp.public) }, peers: vec![] };
        create_private(&super::key_path(), hex::encode(kp.private).as_bytes())?;
        save_locked(&b)?;
        return Ok(b);
    }
    let text = fs::read_to_string(book).context(RECOVERY)?;
    // Do not include input contents in an error (a damaged file may contain secrets).
    let b: Book = serde_json::from_str(&text).map_err(|_| anyhow::anyhow!("{RECOVERY}: invalid peers.json"))?;
    if b.me.id.is_empty() || b.me.name.trim().is_empty() || b.me.public_key.is_empty() {
        bail!("{RECOVERY}: incomplete identity");
    }
    let key = read_key()?;
    use snow::resolvers::CryptoResolver;
    let mut dh = snow::resolvers::DefaultResolver.resolve_dh(&snow::params::DHChoice::Curve25519)
        .context("Noise identity algorithm unavailable")?;
    dh.set(&key);
    if !hex::encode(dh.pubkey()).eq_ignore_ascii_case(&b.me.public_key) {
        bail!("{RECOVERY}: public and private identity keys do not match");
    }
    Ok(b)
}

fn save_locked(b: &Book) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(b)?;
    let tmp = super::root().join(format!(".peers-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        match fs::metadata(super::book_path()) {
            Ok(m) => f.set_permissions(m.permissions())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        f.write_all(&bytes)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, super::book_path())?;
        #[cfg(unix)]
        File::open(super::root())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result.context("could not commit pairing state")
}

pub(super) fn update<T>(edit: impl FnOnce(&mut Book) -> Result<T>) -> Result<T> {
    let _lock = lock()?;
    let mut b = load_locked()?;
    let before = serde_json::to_vec(&b)?;
    let result = edit(&mut b)?;
    if serde_json::to_vec(&b)? != before { save_locked(&b)?; }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{peer, store};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("dispatch-persistence-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            store::set_thread_root(Some(p.clone()));
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            store::set_thread_root(None);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn invalid_and_incomplete_state_is_preserved() {
        let _fixture = Fixture::new();
        let original = peer::load().unwrap();
        let book = fs::read(peer::book_path()).unwrap();
        let key = fs::read(peer::key_path()).unwrap();
        for damaged in [b"".as_slice(), b"{", b"{}", b"{\"me\":null}"] {
            fs::write(peer::book_path(), damaged).unwrap();
            assert!(peer::load().is_err());
            assert!(peer::set_name("must not persist").is_err());
            assert!(peer::net::secret().is_err());
            assert!(fs::read(peer::book_path()).unwrap() == damaged);
            assert!(fs::read(peer::key_path()).unwrap() == key);
        }
        fs::remove_file(peer::book_path()).unwrap();
        assert!(peer::load().is_err(), "an orphan key is not a new installation");
        assert!(!peer::book_path().exists());
        assert!(fs::read(peer::key_path()).unwrap() == key);
        fs::write(peer::book_path(), &book).unwrap();
        fs::remove_file(peer::key_path()).unwrap();
        assert!(peer::load().is_err(), "a missing key requires recovery");
        assert!(!peer::key_path().exists());
        assert!(fs::read(peer::book_path()).unwrap() == book);
        fs::write(peer::key_path(), &key).unwrap();
        assert!(create_private(&peer::key_path(), b"replacement").is_err());
        assert!(fs::read(peer::key_path()).unwrap() == key);
        let mut mismatch = original;
        mismatch.me.public_key = "00".repeat(32);
        fs::write(peer::book_path(), serde_json::to_vec(&mismatch).unwrap()).unwrap();
        assert!(peer::load().is_err(), "mismatched keys must not be used or replaced");
        assert!(fs::read(peer::key_path()).unwrap() == key);
    }

    #[cfg(unix)]
    #[test]
    fn permission_errors_never_rotate_keys_or_commit_a_partial_book() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        peer::load().unwrap();
        peer::net::secret().unwrap();
        let book = fs::read(peer::book_path()).unwrap();
        let key = fs::read(peer::key_path()).unwrap();
        let iroh = fixture.0.join("peer.iroh.key");
        let signing_key = fs::read(&iroh).unwrap();
        fs::set_permissions(peer::book_path(), fs::Permissions::from_mode(0o000)).unwrap();
        let book_failed = peer::load().is_err();
        fs::set_permissions(peer::book_path(), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(book_failed);
        fs::set_permissions(&iroh, fs::Permissions::from_mode(0o200)).unwrap();
        let key_failed = peer::net::secret().is_err();
        fs::set_permissions(&iroh, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(key_failed, "a writeable but unreadable key must not be replaced");
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o500)).unwrap();
        let commit_failed = peer::set_name("cannot commit").is_err();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(commit_failed);
        assert!(fs::read(peer::book_path()).unwrap() == book);
        assert!(fs::read(peer::key_path()).unwrap() == key);
        assert!(fs::read(&iroh).unwrap() == signing_key);
        peer::set_name("updated").unwrap();
        assert_eq!(fs::metadata(peer::book_path()).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn busy_lock_is_bounded_and_abandoned_temporary_files_are_not_state() {
        let fixture = Fixture::new();
        let before = peer::load().unwrap();
        fs::write(fixture.0.join(".peers-interrupted.tmp"), b"{").unwrap();
        let held = lock().unwrap();
        let start = Instant::now();
        assert!(peer::load().is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
        drop(held);
        assert_eq!(peer::load().unwrap().me.id, before.me.id);
    }

    // Spawn this exact test in separate OS processes; no real store or network is used.
    #[test]
    fn subprocess_writer() {
        let Ok(root) = std::env::var("DISPATCH_PERSISTENCE_TEST_ROOT") else { return };
        let index = std::env::var("DISPATCH_PERSISTENCE_TEST_INDEX").unwrap();
        let root = PathBuf::from(root);
        store::set_thread_root(Some(root.clone()));
        let me = peer::load().unwrap().me;
        let signing = peer::net::secret().unwrap().public().to_string();
        for i in 0..10 {
            let id = format!("peer-{index}-{i}");
            peer::remember(peer::PeerInfo { id: id.clone(), name: id.clone(), public_key: me.public_key.clone(), address: None, paired_at: String::new(), iroh_id: None, iroh_addr: None, verified: false, rule: "ask".into() }).unwrap();
            peer::pin_keys(&id, Some(signing.clone()), Some(serde_json::json!({"fixture":i}))).unwrap();
            peer::set_rule(&id, "contacts").unwrap();
            peer::set_address(&id, Some("fixture.invalid:1".into())).unwrap();
        }
        fs::write(root.join(format!("identity-{index}")), me.id).unwrap();
    }

    struct Workers(Vec<Child>);
    impl Drop for Workers {
        fn drop(&mut self) {
            for child in &mut self.0 { let _ = child.kill(); let _ = child.wait(); }
        }
    }

    #[test]
    fn concurrent_process_initialization_and_updates_keep_one_identity_and_all_peers() {
        let fixture = Fixture::new();
        let mut workers = Workers(vec![]);
        for i in 0..4 {
            workers.0.push(Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "peer::persistence::tests::subprocess_writer", "--nocapture"])
                .env("DISPATCH_PERSISTENCE_TEST_ROOT", &fixture.0)
                .env("DISPATCH_PERSISTENCE_TEST_INDEX", i.to_string())
                .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap());
        }
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            assert!(Instant::now() < deadline, "subprocess writes exceeded their deadline");
            // An unlocked reader must see either no initial file or one complete snapshot.
            match fs::read(peer::book_path()) {
                Ok(bytes) => { assert!(serde_json::from_slice::<Book>(&bytes).is_ok()); }
                Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            }
            let mut done = true;
            for child in &mut workers.0 {
                match child.try_wait().unwrap() {
                    Some(exit) => assert!(exit.success(), "writer failed: {exit}"),
                    None => done = false,
                }
            }
            if done { break; }
            std::thread::sleep(Duration::from_millis(2));
        }
        let b = peer::load().unwrap();
        assert_eq!(b.peers.len(), 40, "no concurrent additions lost");
        for i in 0..4 { assert_eq!(fs::read_to_string(fixture.0.join(format!("identity-{i}"))).unwrap(), b.me.id); }
        for p in &b.peers {
            assert_eq!(p.rule, "contacts");
            assert_eq!(p.address.as_deref(), Some("fixture.invalid:1"));
            assert!(p.iroh_id.is_some() && p.iroh_addr.is_some());
            assert!(!p.verified);
        }
        let before = fs::read(peer::book_path()).unwrap();
        assert!(peer::pin_keys(&b.peers[0].id, Some("different-signing-key".into()), None).is_err());
        assert!(fs::read(peer::book_path()).unwrap() == before, "a refused key change is not committed");
        assert!(peer::forget(&b.peers[0].id).unwrap().is_some());
        assert!(peer::pin_keys(&b.peers[0].id, None, None).is_err(), "a stale writer must not resurrect a forgotten peer");
        assert_eq!(peer::peers().unwrap().len(), 39);
        let mut collision = b.peers[0].clone();
        collision.id = "different-peer".into();
        collision.name = b.peers[1].id.clone();
        collision.iroh_addr = None;
        peer::remember(collision.clone()).unwrap();
        peer::pin_keys(&b.peers[1].id, None, Some(serde_json::json!({"exact": true}))).unwrap();
        let after = peer::load().unwrap();
        assert!(after.peers.iter().find(|p| p.id == collision.id).unwrap().iroh_addr.is_none());
        assert_eq!(after.peers.iter().find(|p| p.id == b.peers[1].id).unwrap().iroh_addr, Some(serde_json::json!({"exact":true})));
    }
}
