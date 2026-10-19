//! Blocking HTTP client for the iter_data API.
//!
//! How the engine proves who it is (2026-10-11): with its own Ed25519 key
//! (`iter_engine key new`, key.rs). The client signs in (`POST /auth/engine`,
//! iter_core::enginekey) for a session token and sends that as its bearer,
//! signing in again well before the session ends and at once on a 401.
//! When `key new` left a next key beside the key file (`<key file>.next`),
//! `adopt_next` tries it; once iter_data accepts it (an admin rotated the
//! key in the Engine keys screen) every clone of this client signs in with
//! it and the next key becomes the key file. The engine calls `adopt_next`
//! every tick (rate-limited), and a refused sign-in calls it at once.
//!
//! An agent's `iter` shim runs on the session token it was started with
//! (`Api::new`). Every connection follows trust.rs: iter_data's certificate
//! pinned (or CA-checked), plain http only to this machine, and no call that
//! carries a token before the certificate is trusted: the signed sign-in,
//! which signs the fingerprint it sees, makes that trust on first contact.

use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Sign in again when the session has less than this left: an agent started
/// now gets a session that outlives it.
const RENEW_BEFORE_SECS: i64 = 12 * 3600;
/// How often a next key iter_data refused is tried again: on the tick, and
/// after a refused sign-in (a revoked key must not wait a minute).
const NEXT_RETRY: Duration = Duration::from_secs(60);
const NEXT_RETRY_ON_401: Duration = Duration::from_secs(5);

/// Signing in with the machine's key: where it is and the session it holds.
struct KeyAuth {
    key_file: PathBuf,
    /// the session's expiry (unix seconds); 0 = none yet
    expires_at: Mutex<i64>,
    /// one sign-in at a time
    signing_in: Mutex<()>,
    /// when a next key was last tried
    next_tried: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct Api {
    pub base: String,
    token: Arc<RwLock<String>>,
    key: Option<Arc<KeyAuth>>,
    http: reqwest::blocking::Client,
    /// what the TLS handshakes are checked against (None: loopback http)
    pin: Option<Arc<crate::trust::PinState>>,
    check: crate::trust::Check,
    /// why this base URL may not be used at all (plain http to another machine)
    refused: Option<String>,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.body)
    }
}

/// `<key file>.next`: the key `iter_engine key new` made while one existed.
pub fn next_key_file(key_file: &Path) -> PathBuf {
    let mut s = key_file.as_os_str().to_owned();
    s.push(".next");
    PathBuf::from(s)
}

/// The signing key in a PEM file (PKCS#8).
pub fn load_key(path: &Path) -> Result<SigningKey, String> {
    let pem = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    SigningKey::from_pkcs8_pem(&pem).map_err(|e| format!("{}: not an engine key ({e})", path.display()))
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Api {
    /// A client with a fixed bearer token.
    pub fn new(base: &str, token: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let (http, pin, check, refused) = match crate::trust::client(&base, Duration::from_secs(30)) {
            Ok((h, p, c)) => (h, p, c, None),
            Err(why) => (reqwest::blocking::Client::new(), None, crate::trust::Check::Loopback, Some(why)),
        };
        Self { base, token: Arc::new(RwLock::new(token.to_string())), key: None, http, pin, check, refused }
    }

    /// May a call that carries a token go out now?
    pub fn trusted(&self) -> bool {
        self.untrusted().is_none()
    }

    /// The underlying HTTP client (shares this client's live pins).
    pub fn http(&self) -> reqwest::blocking::Client {
        self.http.clone()
    }

    /// Why this client cannot be used, when it cannot (plain http to another machine).
    pub fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// An HTTP client to iter_data with a longer timeout, under the same trust.
    pub fn long_client(&self, timeout: Duration) -> Result<reqwest::blocking::Client, String> {
        crate::trust::client(&self.base, timeout).map(|(h, _, _)| h)
    }

    /// A call that carries a token is refused until iter_data's certificate
    /// is trusted (pinned by a signed sign-in, or `key trust`).
    fn untrusted(&self) -> Option<ApiError> {
        if let Some(why) = &self.refused {
            return Some(ApiError { status: 0, body: why.clone() });
        }
        match (&self.pin, self.check) {
            (Some(p), crate::trust::Check::Pin) if !p.pinned() => Some(ApiError {
                status: 0,
                body: "iter_data's certificate is not trusted yet: the engine trusts it at its first signed sign-in (or run `iter_engine key trust`)".into(),
            }),
            _ => None,
        }
    }

    /// A client that signs in with the machine's key (on first use).
    pub fn with_key(base: &str, key_file: &Path) -> Self {
        let mut api = Self::new(base, "");
        api.key = Some(Arc::new(KeyAuth {
            key_file: key_file.to_path_buf(),
            expires_at: Mutex::new(0),
            signing_in: Mutex::new(()),
            next_tried: Mutex::new(None),
        }));
        api
    }

    /// The bearer token in use right now (signing in first when the session
    /// is missing or runs out within RENEW_BEFORE_SECS).
    pub fn token(&self) -> String {
        if let Some(k) = &self.key {
            if *k.expires_at.lock().unwrap_or_else(|p| p.into_inner()) - now_secs() < RENEW_BEFORE_SECS {
                if let Err(e) = self.sign_in(false) {
                    eprintln!("[engine] sign-in with the engine key failed: {e}");
                }
            }
        }
        self.token.read().map(|t| t.clone()).unwrap_or_default()
    }

    /// The fingerprint of the certificate iter_data presents (a health check
    /// makes the handshake); "" over loopback http.
    pub fn seen_fingerprint(&self) -> Result<String, ApiError> {
        if let Some(why) = &self.refused {
            return Err(ApiError { status: 0, body: why.clone() });
        }
        let Some(pin) = &self.pin else { return Ok(String::new()) };
        let resp = self.http.get(format!("{}/health", self.base)).send().map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        let _ = resp.text();
        Ok(pin.seen())
    }

    /// GET a public file (no token) from iter_data, only over a connection
    /// whose certificate this engine already trusts (pinned, or mode ca), so
    /// what comes back is from the iter_data this engine trusts.
    pub fn get_trusted_text(&self, path: &str) -> Result<String, ApiError> {
        if let Some(why) = &self.refused {
            return Err(ApiError { status: 0, body: why.clone() });
        }
        let ok = match (self.check, &self.pin) {
            (crate::trust::Check::Pin, Some(p)) => p.pinned(),
            (crate::trust::Check::Ca, _) => true,
            _ => false,
        };
        if !ok {
            let why = if self.check == crate::trust::Check::Loopback {
                "plain http to this machine: there is no certificate".to_string()
            } else {
                "iter_data's certificate is not trusted yet: run `iter_engine key trust` (or sign in once) first".to_string()
            };
            return Err(ApiError { status: 0, body: why });
        }
        let resp = self.http.get(format!("{}{path}", self.base)).send().map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        let status = resp.status().as_u16();
        let body = resp.text().map_err(|e| ApiError { status, body: e.to_string() })?;
        if status >= 400 {
            return Err(ApiError { status, body });
        }
        Ok(body)
    }

    /// One signed login with `key`: (session token, its expiry). It signs the
    /// certificate fingerprint it sees, so iter_data refuses it when relayed
    /// through anything else, and it pins (or re-pins) what iter_data
    /// confirms it serves.
    pub fn login_with(&self, key: &SigningKey) -> Result<(String, i64), ApiError> {
        let kid = iter_core::enginekey::key_id(&key.verifying_key().to_bytes());
        let tls_fp = self.seen_fingerprint()?;
        let (ts, nonce) = (now_secs(), uuid::Uuid::new_v4().simple().to_string());
        let sig: String = key.sign(iter_core::enginekey::login_message(&kid, ts, &nonce, &tls_fp).as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let resp = self
            .http
            .post(format!("{}/auth/engine", self.base))
            .json(&json!({"key_id": kid, "ts": ts, "nonce": nonce, "tls_fp": tls_fp, "signature": sig}))
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        let v = Self::handle(resp)?;
        self.adopt_tls(&v)?;
        let token = v["token"].as_str().unwrap_or_default().to_string();
        Ok((token, v["expires_at"].as_i64().unwrap_or(0)))
    }

    /// Pin what a sign-in answer says iter_data serves (pin mode only).
    fn adopt_tls(&self, answer: &Value) -> Result<(), ApiError> {
        let (Some(pin), crate::trust::Check::Pin) = (&self.pin, self.check) else { return Ok(()) };
        let announced: Vec<String> = answer["tls"]["fingerprints"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect()).unwrap_or_default();
        crate::trust::adopt_announced(&self.base, pin, &announced).map_err(|e| ApiError { status: 0, body: e })
    }

    /// Replace this machine's key with `new`, signed by the current `key`
    /// (`iter_engine key rotate`): (new key id, when the old key stops).
    pub fn rotate_key(&self, key: &SigningKey, new: &SigningKey) -> Result<(String, String), ApiError> {
        let kid = iter_core::enginekey::key_id(&key.verifying_key().to_bytes());
        let new_pub = iter_core::enginekey::format_public(&new.verifying_key().to_bytes());
        if let Some(e) = self.untrusted() {
            return Err(e);
        }
        let (ts, nonce) = (now_secs(), uuid::Uuid::new_v4().simple().to_string());
        let sig: String = key.sign(iter_core::enginekey::rotate_message(&kid, &new_pub, ts, &nonce).as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let resp = self
            .http
            .post(format!("{}/auth/engine/rotate", self.base))
            .json(&json!({"key_id": kid, "new_public_key": new_pub, "ts": ts, "nonce": nonce, "signature": sig}))
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        let v = Self::handle(resp)?;
        self.adopt_tls(&v)?;
        Ok((v["key_id"].as_str().unwrap_or_default().to_string(), v["old_key_stops"].as_str().unwrap_or_default().to_string()))
    }

    /// Sign in with the key file (`force`: even with a fresh session).
    fn sign_in(&self, force: bool) -> Result<(), String> {
        let Some(k) = &self.key else { return Ok(()) };
        let _one = k.signing_in.lock().unwrap_or_else(|p| p.into_inner());
        if !force && *k.expires_at.lock().unwrap_or_else(|p| p.into_inner()) - now_secs() >= RENEW_BEFORE_SECS {
            return Ok(()); // another thread just did
        }
        let key = load_key(&k.key_file)?;
        let (token, exp) = self.login_with(&key).map_err(|e| e.to_string())?;
        self.set_session(token, exp);
        Ok(())
    }

    fn set_session(&self, token: String, exp: i64) {
        if let Ok(mut t) = self.token.write() {
            *t = token;
        }
        if let Some(k) = &self.key {
            *k.expires_at.lock().unwrap_or_else(|p| p.into_inner()) = exp;
        }
    }

    fn handle(resp: reqwest::blocking::Response) -> Result<Value, ApiError> {
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        if (200..300).contains(&status) {
            Ok(serde_json::from_str(&body).unwrap_or(Value::Null))
        } else {
            Err(ApiError { status, body })
        }
    }

    fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>, token: &str) -> Result<Value, ApiError> {
        if let Some(e) = self.untrusted() {
            return Err(e);
        }
        let mut req = self.http.request(method, format!("{}{}", self.base, path)).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        Self::handle(resp)
    }

    /// One call; a 401 signs in again (with the next key when the current
    /// one is refused) and, when that worked, sends the call again.
    fn call(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        let token = self.token();
        match self.send(method.clone(), path, body, &token) {
            Err(e) if e.status == 401 && self.key.is_some() && (self.sign_in(true).is_ok() || self.adopt_next(true)) => {
                self.send(method, path, body, &self.token())
            }
            r => r,
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.call(reqwest::Method::GET, path, None)
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(reqwest::Method::PUT, path, Some(body))
    }

    pub fn delete(&self, path: &str) -> Result<Value, ApiError> {
        self.call(reqwest::Method::DELETE, path, None)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(reqwest::Method::POST, path, Some(body))
    }

    /// Sign in with the next key once iter_data accepts it, and make it the
    /// key file. Tried at most every NEXT_RETRY (NEXT_RETRY_ON_401 when
    /// `now`: the current key was just refused); true = this call switched.
    pub fn adopt_next(&self, now: bool) -> bool {
        let Some(k) = &self.key else { return false };
        let next = next_key_file(&k.key_file);
        if !next.exists() {
            return false;
        }
        let Ok(mut tried) = k.next_tried.try_lock() else { return false };
        let every = if now { NEXT_RETRY_ON_401 } else { NEXT_RETRY };
        if tried.is_some_and(|t| t.elapsed() < every) {
            return false;
        }
        *tried = Some(Instant::now());
        let key = match load_key(&next) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("[engine] next engine key unreadable: {e}");
                return false;
            }
        };
        let Ok((token, exp)) = self.login_with(&key) else { return false }; // not registered yet, or the server is away
        let _one = k.signing_in.lock().unwrap_or_else(|p| p.into_inner());
        self.set_session(token, exp);
        let id = iter_core::enginekey::key_id(&key.verifying_key().to_bytes());
        match std::fs::rename(&next, &k.key_file) {
            Ok(()) => println!("[engine] iter_data accepted the next engine key ({id}): signed in with it, and it is now {}", k.key_file.display()),
            Err(e) => eprintln!("[engine] signed in with the next engine key ({id}) but could not move it to {}: {e} — move it by hand before a restart", k.key_file.display()),
        }
        true
    }
}

/// A throwaway iter_data stand-in for engine unit tests: a real HTTP server
/// on 127.0.0.1 whose every request goes to `handler(method, path, body)`.
/// `None` drops the connection without an answer — the transport failure
/// (`ApiError.status == 0`) a network outage produces.
#[cfg(test)]
pub mod fake {
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    pub type BearerHandler = dyn Fn(&str, &str, &str, &Value) -> Option<(u16, Value)> + Send + Sync;

    pub struct FakeServer {
        pub base: String,
        /// every request seen: (method, path, body)
        pub calls: Arc<Mutex<Vec<(String, String, Value)>>>,
    }

    impl FakeServer {
        pub fn api(&self) -> super::Api {
            super::Api::new(&self.base, "test-token")
        }
        pub fn calls_to(&self, method: &str, path_part: &str) -> Vec<Value> {
            self.calls.lock().unwrap().iter().filter(|(m, p, _)| m == method && p.contains(path_part)).map(|(_, _, b)| b.clone()).collect()
        }
    }

    pub fn serve(handler: impl Fn(&str, &str, &Value) -> Option<(u16, Value)> + Send + Sync + 'static) -> FakeServer {
        serve_with_bearer(move |_bearer, m, p, b| handler(m, p, b))
    }

    /// `serve`, with the request's bearer token as the handler's first argument.
    pub fn serve_with_bearer(handler: impl Fn(&str, &str, &str, &Value) -> Option<(u16, Value)> + Send + Sync + 'static) -> FakeServer {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls: Arc<Mutex<Vec<(String, String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<BearerHandler> = Arc::new(handler);
        let seen = calls.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                let seen = seen.clone();
                std::thread::spawn(move || {
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let header_end = loop {
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                        .unwrap_or(0);
                    while buf.len() < header_end + len {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let mut first = head.lines().next().unwrap_or("").split_whitespace();
                    let method = first.next().unwrap_or("").to_string();
                    let path = first.next().unwrap_or("").to_string();
                    let body: Value = serde_json::from_slice(&buf[header_end..(header_end + len).min(buf.len())]).unwrap_or(Value::Null);
                    let bearer = head
                        .lines()
                        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().trim_start_matches("Bearer ").to_string()))
                        .unwrap_or_default();
                    seen.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
                    let Some((code, out)) = handler(&bearer, &method, &path, &body) else {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return;
                    };
                    let text = out.to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                });
            }
        });
        FakeServer { base, calls }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePrivateKey;

    fn write_key(path: &Path) -> String {
        let k = SigningKey::generate(&mut rand::rngs::OsRng);
        std::fs::write(path, k.to_pkcs8_pem(Default::default()).unwrap().as_bytes()).unwrap();
        iter_core::enginekey::key_id(&k.verifying_key().to_bytes())
    }

    #[test]
    fn signs_in_with_its_key_and_moves_to_the_next_once_accepted() {
        // the fake iter_data: a key in `ok` signs in; a session is good while its key is
        let ok: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let ok2 = ok.clone();
        let srv = fake::serve_with_bearer(move |bearer, m, p, b| {
            let ok = ok2.lock().unwrap();
            if m == "POST" && p == "/auth/engine" {
                let kid = b["key_id"].as_str().unwrap_or("");
                return Some(if ok.iter().any(|k| k == kid) {
                    (200, json!({"token": format!("iter_s.{kid}"), "expires_at": now_secs() + 86400}))
                } else {
                    (401, json!({"error": "unknown engine key"}))
                });
            }
            let kid = bearer.strip_prefix("iter_s.").unwrap_or("");
            Some(if ok.iter().any(|k| k == kid) { (200, json!({"ok": true})) } else { (401, json!({"error": "engine key revoked"})) })
        });
        let dir = std::env::temp_dir().join(format!("iter-client-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("engine.key");
        let k1 = write_key(&file);
        let k2 = write_key(&next_key_file(&file));
        ok.lock().unwrap().push(k1.clone());

        let api = Api::with_key(&srv.base, &file);
        let clone = api.clone();
        assert!(api.get("/api/server").is_ok());
        assert_eq!(api.token(), format!("iter_s.{k1}"));
        // the next key is not accepted yet: nothing changes, and it is not retried within the minute
        assert!(!api.adopt_next(false));
        ok.lock().unwrap().push(k2.clone());
        assert!(!api.adopt_next(false), "rate-limited");
        // the old key is revoked: the next call is refused, signs in again, falls
        // back on the next key, and goes through; every clone follows
        ok.lock().unwrap().retain(|k| k != &k1);
        *api.key.as_ref().unwrap().next_tried.lock().unwrap() = None;
        assert!(clone.get("/api/server").is_ok());
        assert_eq!((api.token(), clone.token()), (format!("iter_s.{k2}"), format!("iter_s.{k2}")));
        // the next key is now the key file
        assert!(!next_key_file(&file).exists());
        assert_eq!(iter_core::enginekey::key_id(&load_key(&file).unwrap().verifying_key().to_bytes()), k2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
