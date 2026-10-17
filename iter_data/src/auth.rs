//! Baked-in JWT auth (decided 2026-09-01, chosen over Cognito for
//! portability): argon2id password hashes, HS256 tokens, stateless verify.
//! Revocation: "tokenver" on the user row is stamped into each token; bump
//! the row to invalidate everything outstanding for that principal.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub tokenver: u64,
    pub exp: u64,
    /// an engine session (keys.rs): the id of the engine key it was issued
    /// for, re-checked on every request; "" for a person's token
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kid: String,
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, pwhash: &str) -> bool {
    match PasswordHash::new(pwhash) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

pub fn mint_token(secret: &[u8], sub: &str, role: &str, tokenver: u64, ttl_secs: u64) -> Result<String, String> {
    let exp = chrono::Utc::now().timestamp() as u64 + ttl_secs;
    let claims = Claims { sub: sub.to_string(), role: role.to_string(), tokenver, exp, kid: String::new() };
    mint_claims(secret, &claims)
}

pub fn mint_claims(secret: &[u8], claims: &Claims) -> Result<String, String> {
    encode(&Header::default(), claims, &EncodingKey::from_secret(secret)).map_err(|e| e.to_string())
}

pub fn verify_token(secret: &[u8], token: &str) -> Result<Claims, String> {
    decode::<Claims>(token, &DecodingKey::from_secret(secret), &Validation::default())
        .map(|d| d.claims)
        .map_err(|e| e.to_string())
}

/// Load a signing secret: the `env` variable wins; else read (and narrow to
/// its owner) or create the sidecar file, so restarts keep sessions valid.
/// Two of them: ITER_JWT_SECRET / iter_data.secret signs people's tokens,
/// ITER_ENGINE_SESSION_SECRET / engine_session.secret engine sessions.
pub fn load_secret(env: &str, secret_file: &str) -> Vec<u8> {
    if let Ok(s) = std::env::var(env) {
        if !s.trim().is_empty() {
            return s.trim().as_bytes().to_vec();
        }
    }
    if let Ok(s) = std::fs::read_to_string(secret_file) {
        let t = s.trim().to_string();
        if !t.is_empty() {
            crate::tls::restrict(std::path::Path::new(secret_file));
            return t.into_bytes();
        }
    }
    use rand::Rng;
    let generated: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    if let Err(e) = crate::tls::write_private(std::path::Path::new(secret_file), generated.as_bytes()) {
        eprintln!("[iter_data] could not keep the new secret in {secret_file}: {e} (sessions end at restart)");
    }
    eprintln!("[iter_data] generated a new signing secret -> {secret_file}");
    generated.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("s3cret").unwrap();
        assert!(verify_password("s3cret", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn token_roundtrip() {
        let secret = b"test-secret";
        let t = mint_token(secret, "stephen", "admin", 1, 3600).unwrap();
        let c = verify_token(secret, &t).unwrap();
        assert_eq!(c.sub, "stephen");
        assert_eq!(c.role, "admin");
        assert!(verify_token(b"other", &t).is_err());
    }
}
