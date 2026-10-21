//! Persistent device identity.
//!
//! Every Mycel device owns an Ed25519 keypair. Its public half becomes the
//! device's `PeerId`, which is how other peers recognise it. There are no
//! accounts, passwords or phone numbers: the key file *is* the identity.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use libp2p::identity::Keypair;

/// File name of the identity key inside a data directory.
pub const IDENTITY_FILE: &str = "identity.key";

/// Loads the keypair stored in `data_dir`, or creates and stores a new one.
///
/// Returns the keypair and whether it was freshly created.
pub fn load_or_create(data_dir: &Path) -> Result<(Keypair, bool)> {
    let path = data_dir.join(IDENTITY_FILE);

    if path.exists() {
        let bytes =
            fs::read(&path).with_context(|| format!("reading identity file {}", path.display()))?;
        let keypair = Keypair::from_protobuf_encoding(&bytes)
            .with_context(|| format!("identity file {} is corrupted", path.display()))?;
        return Ok((keypair, false));
    }

    fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    let keypair = Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        .context("encoding new identity key")?;
    match write_private(&path, &bytes) {
        Ok(()) => Ok((keypair, true)),
        // Another process created it a moment ago: use theirs, so both agree.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            load_or_create(data_dir)
        }
        Err(e) => Err(e).with_context(|| format!("creating identity file {}", path.display())),
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (first, created) = load_or_create(dir.path()).unwrap();
        assert!(created);
        let (second, created_again) = load_or_create(dir.path()).unwrap();
        assert!(!created_again);
        assert_eq!(first.public().to_peer_id(), second.public().to_peer_id());
    }

    #[test]
    fn separate_directories_get_separate_identities() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (ka, _) = load_or_create(a.path()).unwrap();
        let (kb, _) = load_or_create(b.path()).unwrap();
        assert_ne!(ka.public().to_peer_id(), kb.public().to_peer_id());
    }

    #[test]
    fn concurrent_first_starts_agree_on_one_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let ids: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || load_or_create(&path).unwrap().0.public().to_peer_id())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn corrupted_identity_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(IDENTITY_FILE), b"not a key").unwrap();
        let err = load_or_create(dir.path()).unwrap_err();
        assert!(err.to_string().contains("corrupted"));
    }
}
