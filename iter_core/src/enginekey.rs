//! Engine keys (decided 2026-10-11): an engine machine proves who it is with
//! an Ed25519 key pair it makes for itself (`iter_engine key new`). The
//! private key never leaves the machine; an admin pastes only the public key
//! into iter_data's Engine keys screen, so nothing secret is ever copied.
//!
//! Signing in: the engine signs `login_message` (its key id, the time and a
//! one-time nonce) and posts it to `POST /auth/engine`; iter_data checks the
//! signature against the registered public key and answers with a session
//! token (`SESSION_PREFIX` + a signed JWT that names the key). Every request
//! made with a session re-checks that its key is still active, so revoking a
//! key stops that engine at once; the session's expiry only bounds a session
//! token copied off the wire while nobody revoked anything.
//!
//! Shapes: a public key is `iter_` + 64 lowercase hex (the 32 key bytes); its
//! id is the first 12 hex of the key's SHA-256. The `iter_` prefix on a
//! session token lets a proxy in front of iter_data (an ALB rule) tell an
//! engine's requests from a browser's.

use sha2::{Digest, Sha256};

pub const PREFIX: &str = "iter_";
/// A session token is this prefix + a JWT (which always holds dots).
pub const SESSION_PREFIX: &str = "iter_";
/// How far a login's timestamp may be from the server's clock.
pub const LOGIN_SKEW_SECS: i64 = 300;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hex to bytes; None for anything else.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// The public key as it is shown and pasted: `iter_<64 hex>`.
pub fn format_public(key: &[u8; 32]) -> String {
    format!("{PREFIX}{}", hex(key))
}

/// The 32 key bytes of a pasted public key (surrounding space ignored).
pub fn parse_public(s: &str) -> Option<[u8; 32]> {
    let h = s.trim().strip_prefix(PREFIX)?;
    unhex(h)?.try_into().ok()
}

/// A key's id: the first 12 hex of its SHA-256 (safe to show and log).
pub fn key_id(key: &[u8; 32]) -> String {
    hex(&Sha256::digest(key))[..12].to_string()
}

/// What the engine signs to sign in.
pub fn login_message(key_id: &str, ts: i64, nonce: &str) -> String {
    format!("iter-engine-login\n{key_id}\n{ts}\n{nonce}")
}

/// The JWT inside an engine session token, if `bearer` is one.
pub fn session_jwt(bearer: &str) -> Option<&str> {
    bearer.strip_prefix(SESSION_PREFIX).filter(|j| j.contains('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_keys_round_trip_and_have_short_ids() {
        let k = [7u8; 32];
        let s = format_public(&k);
        assert_eq!(s.len(), PREFIX.len() + 64);
        assert_eq!(parse_public(&format!("  {s}\n")), Some(k));
        assert_eq!(key_id(&k).len(), 12);
        assert_ne!(key_id(&k), key_id(&[8u8; 32]));
        assert_eq!(parse_public(&s.to_uppercase()), None);
        assert_eq!(parse_public(&s[..s.len() - 2]), None);
        assert_eq!(parse_public("key_00"), None);
    }

    #[test]
    fn session_tokens_are_told_from_keys_and_plain_jwts() {
        assert_eq!(session_jwt("iter_eyJ.a.b"), Some("eyJ.a.b"));
        assert_eq!(session_jwt(&format_public(&[1u8; 32])), None);
        assert_eq!(session_jwt("eyJ.a.b"), None);
        assert_eq!(login_message("abc", 5, "n"), "iter-engine-login\nabc\n5\nn");
        assert_eq!(unhex("0aff"), Some(vec![10, 255]));
        assert_eq!(unhex("0g"), None);
    }
}
