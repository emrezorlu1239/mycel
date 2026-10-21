//! Wire protocol between members and the host.
//!
//! Phase 0 uses a simple host-relayed model: members send to the host, the
//! host fans messages out to every other member. All traffic travels inside
//! libp2p connections, which are always encrypted (Noise for TCP, TLS 1.3 for
//! QUIC).

use libp2p::StreamProtocol;
use serde::{Deserialize, Serialize};

/// Protocol identifier negotiated on every chat stream.
pub const CHAT_PROTOCOL: StreamProtocol = StreamProtocol::new("/mycel/chat/0.1.0");

/// Longest message the host accepts, in bytes.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// Longest nickname the host accepts, in characters.
pub const MAX_NICK_CHARS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Member → host: ask to join with a display name.
    Join { nick: String },
    /// Member → host: post a message to the server.
    Post { text: String },
    /// Host → member: a message someone else posted.
    Deliver { from: String, text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Host → member: you are in.
    Welcome {
        server: String,
        members: Vec<String>,
    },
    /// Generic acknowledgement.
    Ack,
    /// The request was refused.
    Denied { reason: String },
}

/// Normalises a nickname: trims it, removes control characters and caps its
/// length. Returns `None` if nothing usable is left.
pub fn sanitize_nick(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NICK_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_stable_json_shape() {
        let json = serde_json::to_string(&Request::Post { text: "hi".into() }).unwrap();
        assert_eq!(json, r#"{"type":"post","text":"hi"}"#);
    }

    #[test]
    fn nick_is_sanitized() {
        assert_eq!(sanitize_nick("  emre \n"), Some("emre".to_string()));
        assert_eq!(sanitize_nick("a\u{7}b"), Some("ab".to_string()));
        assert_eq!(sanitize_nick("   "), None);
        assert_eq!(
            sanitize_nick(&"x".repeat(100)).unwrap().chars().count(),
            MAX_NICK_CHARS
        );
    }
}
