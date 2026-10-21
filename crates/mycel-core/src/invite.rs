//! Invite codes.
//!
//! An invite is everything a new member needs to reach a server: the host's
//! `PeerId` and a list of addresses to try (direct LAN/WAN addresses and
//! relayed `/p2p-circuit` addresses). It is encoded as Base58 so it can be
//! pasted into a chat, read aloud or rendered as a QR code without ambiguous
//! characters.

use std::fmt;
use std::str::FromStr;

use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};

/// Prefix that makes invite codes recognisable at a glance.
pub const INVITE_PREFIX: &str = "mycel1";

/// Current invite format version.
pub const INVITE_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    /// Format version, so old apps can reject codes they do not understand.
    pub v: u8,
    /// Human-readable server name, shown before joining.
    pub server: String,
    /// The host's identity. Connections to anyone else are rejected.
    pub host: PeerId,
    /// Addresses to try, best first.
    pub addrs: Vec<Multiaddr>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InviteError {
    #[error("this is not a Mycel invite (it should start with `{INVITE_PREFIX}`)")]
    MissingPrefix,
    #[error("the invite is damaged: {0}")]
    Malformed(String),
    #[error("this invite was made by a newer Mycel (format v{0}); please update the app")]
    UnsupportedVersion(u8),
    #[error("the invite contains no addresses to connect to")]
    NoAddresses,
}

impl Invite {
    pub fn new(server: impl Into<String>, host: PeerId, addrs: Vec<Multiaddr>) -> Self {
        Self {
            v: INVITE_VERSION,
            server: server.into(),
            host,
            addrs,
        }
    }

    /// Encodes the invite as a single shareable string.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("invite is always serialisable");
        format!("{INVITE_PREFIX}{}", bs58::encode(json).into_string())
    }

    /// Parses an invite string, tolerating surrounding whitespace.
    pub fn decode(code: &str) -> Result<Self, InviteError> {
        let body = code
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or(InviteError::MissingPrefix)?;
        let json = bs58::decode(body)
            .into_vec()
            .map_err(|e| InviteError::Malformed(e.to_string()))?;
        let invite: Invite =
            serde_json::from_slice(&json).map_err(|e| InviteError::Malformed(e.to_string()))?;
        if invite.v > INVITE_VERSION {
            return Err(InviteError::UnsupportedVersion(invite.v));
        }
        if invite.addrs.is_empty() {
            return Err(InviteError::NoAddresses);
        }
        Ok(invite)
    }
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl FromStr for Invite {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Invite::decode(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    fn sample() -> Invite {
        let host = Keypair::generate_ed25519().public().to_peer_id();
        Invite::new(
            "Friday Night",
            host,
            vec![
                "/ip4/192.168.1.10/udp/4001/quic-v1".parse().unwrap(),
                "/ip4/203.0.113.10/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit"
                    .parse()
                    .unwrap(),
            ],
        )
    }

    #[test]
    fn round_trip() {
        let invite = sample();
        let code = invite.encode();
        assert!(code.starts_with(INVITE_PREFIX));
        assert_eq!(Invite::decode(&code).unwrap(), invite);
    }

    #[test]
    fn tolerates_whitespace_from_copy_paste() {
        let invite = sample();
        let code = format!("  {}\n", invite.encode());
        assert_eq!(Invite::decode(&code).unwrap(), invite);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Invite::decode("hello"), Err(InviteError::MissingPrefix));
        assert!(matches!(
            Invite::decode("mycel1!!!0OIl"),
            Err(InviteError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_truncated_code() {
        let code = sample().encode();
        let truncated = &code[..code.len() - 10];
        assert!(matches!(
            Invite::decode(truncated),
            Err(InviteError::Malformed(_))
        ));
    }

    #[test]
    fn rejects_future_versions() {
        let mut invite = sample();
        invite.v = INVITE_VERSION + 1;
        assert_eq!(
            Invite::decode(&invite.encode()),
            Err(InviteError::UnsupportedVersion(INVITE_VERSION + 1))
        );
    }

    #[test]
    fn rejects_empty_address_list() {
        let mut invite = sample();
        invite.addrs.clear();
        assert_eq!(
            Invite::decode(&invite.encode()),
            Err(InviteError::NoAddresses)
        );
    }
}
