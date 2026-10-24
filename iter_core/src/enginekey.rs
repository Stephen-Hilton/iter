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
//!
//! Join codes (decided 2026-10-23): the one thing a person carries from
//! iter_data to a new engine machine. An admin makes one in the Engine keys
//! screen (Add engine); it holds iter_data's URL, the fingerprints of the
//! certificates iter_data serves itself, the engine name chosen (or none) and
//! a one-time secret, and expires within hours. `iter_engine key join <code>`
//! (the installers' `--join`) trusts iter_data from it, makes the engine's
//! key and registers that key with the secret (`POST /auth/engine/join`), so
//! nothing is pasted back. The secret only ever registers one engine key,
//! once; the engine still signs in only with its own key.
//!
//! A TLS front (a proxy or load balancer with its own certificate before
//! iter_data): the engine sees the front's certificate, not one of
//! iter_data's. Both sides accept it only when a public certificate authority
//! vouches for it under one of iter_data's own names (`public_chain_valid`),
//! the same bar a browser sets; anything else in between is still refused.

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

/// What the engine signs to sign in: its key id, its clock, a one-time
/// nonce and the fingerprint of the TLS certificate it sees ("" over
/// loopback http), so iter_data can refuse a sign-in relayed through
/// anything presenting another certificate.
pub fn login_message(key_id: &str, ts: i64, nonce: &str, tls_fp: &str) -> String {
    format!("iter-engine-login\n{key_id}\n{ts}\n{nonce}\n{tls_fp}")
}

/// What the engine signs, with its current key, to replace that key with
/// `new_public` (`iter_engine key rotate`, no admin step).
pub fn rotate_message(key_id: &str, new_public: &str, ts: i64, nonce: &str) -> String {
    format!("iter-engine-rotate\n{key_id}\n{new_public}\n{ts}\n{nonce}")
}

/// A TLS certificate's fingerprint: the SHA-256 of its DER, 64 lowercase
/// hex. Engines pin it (iter_engine client.rs); iter_data announces it.
pub fn fingerprint_der(der: &[u8]) -> String {
    hex(&Sha256::digest(der))
}

/// A fingerprint the way people type or paste it (`sha256:`, colons, spaces,
/// any case) to 64 lowercase hex; None when it is not one.
pub fn normalize_fingerprint(s: &str) -> Option<String> {
    let t = s.trim();
    let t = t.strip_prefix("sha256:").or_else(|| t.strip_prefix("SHA256:")).unwrap_or(t);
    let h: String = t.chars().filter(|c| !matches!(c, ':' | ' ' | '-')).collect::<String>().to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// What a join code carries (short keys: the code is pasted whole).
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Default)]
pub struct JoinCode {
    /// iter_data's URL, as engines reach it
    #[serde(rename = "u")]
    pub url: String,
    /// the one-time secret, 64 hex; iter_data keeps only its `join_id`
    #[serde(rename = "s")]
    pub secret: String,
    /// the certificates iter_data serves itself (fingerprints); [] without TLS of its own
    #[serde(rename = "p", default)]
    pub pins: Vec<String>,
    /// the engine name the admin chose ("" = the machine's own name)
    #[serde(rename = "n", default)]
    pub engine: String,
    /// when the code stops working (unix seconds)
    #[serde(rename = "x")]
    pub expires: i64,
}

pub const JOIN_PREFIX: &str = "iterjoin1_";

/// The code as it is shown and pasted: `iterjoin1_` + base64url of its JSON.
pub fn encode_join(code: &JoinCode) -> String {
    use base64::Engine as _;
    let json = serde_json::to_vec(code).unwrap_or_default();
    format!("{JOIN_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// A pasted join code (surrounding space and quotes ignored); None when it is not one.
pub fn decode_join(s: &str) -> Option<JoinCode> {
    use base64::Engine as _;
    let b = s.trim().trim_matches(|c| c == '"' || c == '\'').strip_prefix(JOIN_PREFIX)?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b.trim_end_matches('=')).ok()?;
    let c: JoinCode = serde_json::from_slice(&json).ok()?;
    (c.secret.len() == 64 && unhex(&c.secret).is_some() && !c.url.is_empty()).then_some(c)
}

/// A join secret's id, the only form iter_data keeps: the first 24 hex of its SHA-256.
pub fn join_id(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))[..24].to_string()
}

/// A fresh join secret: 32 random bytes as 64 hex.
pub fn new_join_secret() -> String {
    hex(&uuid::Uuid::new_v4().into_bytes().into_iter().chain(uuid::Uuid::new_v4().into_bytes()).collect::<Vec<u8>>())
}

/// Does a public certificate authority (the Mozilla roots browsers ship, plus
/// `extra_roots` for tests) vouch for `chain` (DER: the server's certificate,
/// then its intermediates) as a TLS server certificate for `name`, now?
pub fn public_chain_valid(chain: &[Vec<u8>], name: &str, extra_roots: &[Vec<u8>]) -> Result<(), String> {
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    let (leaf, rest) = chain.split_first().ok_or("no certificate")?;
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    for r in extra_roots {
        roots.add(CertificateDer::from(r.clone())).map_err(|e| e.to_string())?;
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let v = rustls::client::WebPkiServerVerifier::builder_with_provider(std::sync::Arc::new(roots), provider).build().map_err(|e| e.to_string())?;
    let sn = ServerName::try_from(name.trim_start_matches('[').trim_end_matches(']').to_string()).map_err(|e| format!("{name}: {e}"))?;
    let inter: Vec<CertificateDer> = rest.iter().map(|d| CertificateDer::from(d.clone())).collect();
    v.verify_server_cert(&CertificateDer::from(leaf.clone()), &inter, &sn, &[], UnixTime::now()).map(|_| ()).map_err(|e| e.to_string())
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
        assert_eq!(login_message("abc", 5, "n", "ff"), "iter-engine-login\nabc\n5\nn\nff");
        assert_eq!(rotate_message("abc", "iter_00", 5, "n"), "iter-engine-rotate\nabc\niter_00\n5\nn");
        assert_ne!(rotate_message("abc", "iter_00", 5, "n"), login_message("abc", 5, "n", "iter_00"), "a login can never pass as a rotation");
        assert_eq!(unhex("0aff"), Some(vec![10, 255]));
        let h = "ab".repeat(32);
        assert_eq!(normalize_fingerprint(&h).as_deref(), Some(h.as_str()));
        let colons = h.as_bytes().chunks(2).map(|c| std::str::from_utf8(c).unwrap().to_uppercase()).collect::<Vec<_>>().join(":");
        assert_eq!(normalize_fingerprint(&format!("SHA256:{colons}")).as_deref(), Some(h.as_str()));
        assert_eq!(normalize_fingerprint("sha256:abc"), None);
        assert_eq!(normalize_fingerprint(&"zz".repeat(32)), None);
        assert_eq!(fingerprint_der(b"x").len(), 64);
        assert_eq!(unhex("0g"), None);
    }

    #[test]
    fn join_codes_round_trip_and_keep_only_an_id_of_the_secret() {
        let c = JoinCode { url: "https://iter.example.com".into(), secret: new_join_secret(), pins: vec!["ab".repeat(32)], engine: "lap1".into(), expires: 1_900_000_000 };
        let s = encode_join(&c);
        assert!(s.starts_with(JOIN_PREFIX) && !s.contains(['+', '/', '=', ' ']), "{s}");
        assert_eq!(decode_join(&format!("  '{s}'\n")), Some(c.clone()));
        assert_eq!(decode_join(&s[..s.len() - 4]), None);
        assert_eq!(decode_join("iter_abc"), None);
        assert_eq!(join_id(&c.secret).len(), 24);
        assert_ne!(new_join_secret(), new_join_secret());
        // no pins and no name: still a code (iter_data behind a front, --tls off)
        let bare = JoinCode { pins: vec![], engine: String::new(), ..c };
        assert_eq!(decode_join(&encode_join(&bare)), Some(bare));
    }

    #[test]
    fn a_chain_no_public_authority_signed_is_not_valid() {
        assert!(public_chain_valid(&[], "iter.example.com", &[]).is_err());
        assert!(public_chain_valid(&[b"not a certificate".to_vec()], "iter.example.com", &[]).is_err());
    }
}
