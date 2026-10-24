//! How the engine trusts iter_data over TLS (2026-10-17).
//!
//! iter_data serves HTTPS with a certificate of its own (iter_data tls.rs).
//! The engine does not ask a certificate authority about it: it pins the
//! certificate's fingerprint (the SHA-256 of its DER) in
//! `<iter dir>/iter_engine/config/server_trust.json`, one entry per server origin, owner-only.
//!
//! - **First contact** (no entry yet): the only calls allowed are the
//!   unauthenticated health check and the signed sign-in. The sign-in signs
//!   the fingerprint the engine sees (iter_core::enginekey::login_message),
//!   so iter_data refuses a sign-in relayed through anything presenting
//!   another certificate, and its answer names the fingerprints it serves;
//!   the engine pins the one it saw only when the answer confirms it. To
//!   rule out an impostor on that first contact, give the fingerprint up
//!   front (`iter_engine key trust --fingerprint …`, shown in the webui's
//!   Engine keys screen).
//! - **After that** every connection must present a pinned certificate.
//!   Each sign-in answer re-announces the fingerprints (the current
//!   certificate and, during a rotation, the next one), and the pins follow
//!   them, so a certificate change reaches the engine before the switch.
//! - **mode `ca`** (`key trust --ca`): ordinary certificate-authority checks,
//!   for iter_data behind a proxy with a real certificate. Chosen by itself
//!   whenever the certificate the engine first meets is one a public
//!   authority issued for the URL's host (a TLS front, or iter_data with a
//!   certificate of your own): it then keeps working when that certificate
//!   is renewed. iter_data accepts such a front only under its own names
//!   (iter_data keys.rs `check_seen`).
//! - **A join code** (`key join`, iter_core::enginekey::JoinCode) says which
//!   certificates iter_data serves itself: the engine pins them when it meets
//!   one, takes mode ca when it meets a public certificate for the host, and
//!   trusts nothing else.
//! - **plain `http://`** only to this machine (127.0.0.1, ::1, localhost).

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub const TRUST_FILE: &str = "server_trust.json";

/// `scheme://host:port`, lowercased, the key of a trust entry.
pub fn origin(url: &str) -> String {
    let u = url.trim();
    let (scheme, rest) = u.split_once("://").unwrap_or(("http", u));
    let scheme = scheme.to_ascii_lowercase();
    let hp = rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
    // a port is the digits after the last ':' outside IPv6 brackets
    let has_port = match hp.rfind(']') {
        Some(b) => hp[b..].contains(':'),
        None => hp.contains(':'),
    };
    if has_port {
        format!("{scheme}://{hp}")
    } else {
        format!("{scheme}://{hp}:{}", if scheme == "https" { 443 } else { 80 })
    }
}

/// The host part of a URL (IPv6 brackets removed).
pub fn host(url: &str) -> String {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or(url.trim());
    let hp = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(v6) = hp.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("").to_string();
    }
    hp.rsplit_once(':').map(|(h, _)| h).unwrap_or(hp).to_ascii_lowercase()
}

pub fn is_loopback_host(h: &str) -> bool {
    h == "localhost" || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Why a URL may not be used, if it may not: plain http to another machine.
pub fn refuse_reason(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return None;
    }
    let h = host(url);
    if lower.starts_with("http://") && is_loopback_host(&h) {
        return None;
    }
    Some(format!(
        "refusing {url}: iter_data must be reached over https:// (plain http:// only to this machine), so session tokens and work never cross the network unencrypted"
    ))
}

fn trust_file() -> PathBuf {
    crate::key::home_file("config", TRUST_FILE)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    /// "pin" (the default) or "ca"
    pub mode: String,
    pub pins: Vec<String>,
}

fn read_all() -> Value {
    std::fs::read_to_string(trust_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"servers": {}}))
}

/// The trust entry for a server, if one was made.
pub fn load(url: &str) -> Option<Entry> {
    let all = read_all();
    let e = all["servers"].get(origin(url))?;
    Some(Entry {
        mode: e["mode"].as_str().unwrap_or("pin").to_string(),
        pins: e["pins"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect()).unwrap_or_default(),
    })
}

/// Record (or replace) a server's trust entry, owner-only.
pub fn save(url: &str, entry: &Entry) -> Result<(), String> {
    let mut all = read_all();
    if !all["servers"].is_object() {
        all["servers"] = json!({});
    }
    all["servers"][origin(url)] = json!({"mode": entry.mode, "pins": entry.pins, "updated": iter_core::now_utc()});
    write_private(&trust_file(), serde_json::to_string_pretty(&all).unwrap_or_default().as_bytes())
}

/// Forget a server (the next sign-in trusts on first use again).
pub fn forget(url: &str) -> Result<bool, String> {
    let mut all = read_all();
    let removed = all["servers"].as_object_mut().map(|m| m.remove(&origin(url)).is_some()).unwrap_or(false);
    if removed {
        write_private(&trust_file(), serde_json::to_string_pretty(&all).unwrap_or_default().as_bytes())?;
    }
    Ok(removed)
}

/// Write a file only its owner can read (0600 on unix), replacing it whole.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp-write");
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What one client's connections are checked against, and what they saw.
#[derive(Debug, Default)]
pub struct PinState {
    pub pins: RwLock<Vec<String>>,
    /// the fingerprint of the last certificate a handshake presented
    pub seen: Mutex<String>,
    /// that handshake's certificates (DER, the server's first)
    pub chain: Mutex<Vec<Vec<u8>>>,
}

impl PinState {
    /// Is the server pinned (bearer calls allowed)?
    pub fn pinned(&self) -> bool {
        self.pins.read().map(|p| !p.is_empty()).unwrap_or(false)
    }
    pub fn seen(&self) -> String {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn chain(&self) -> Vec<Vec<u8>> {
        self.chain.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

/// Does a public certificate authority vouch for `chain` as `url`'s host?
pub fn publicly_valid(url: &str, chain: &[Vec<u8>]) -> bool {
    iter_core::enginekey::public_chain_valid(chain, &host(url), &[]).is_ok()
}

#[derive(Debug)]
struct PinVerifier {
    state: Arc<PinState>,
    provider: Arc<rustls::crypto::CryptoProvider>,
    /// mode ca: ordinary certificate-authority checks decide (the
    /// fingerprint is still recorded, for the signed sign-in)
    ca: Option<Arc<rustls::client::WebPkiServerVerifier>>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let fp = iter_core::enginekey::fingerprint_der(end_entity.as_ref());
        if let Ok(mut s) = self.state.seen.lock() {
            *s = fp.clone();
        }
        if let Ok(mut c) = self.state.chain.lock() {
            *c = std::iter::once(end_entity).chain(intermediates).map(|d| d.as_ref().to_vec()).collect();
        }
        if let Some(ca) = &self.ca {
            return ca.verify_server_cert(end_entity, intermediates, name, ocsp, now);
        }
        let pins = self.state.pins.read().map(|p| p.clone()).unwrap_or_default();
        // unpinned: the handshake goes ahead so the signed sign-in can check
        // it (client.rs refuses every bearer call until the pin is made)
        if pins.is_empty() || pins.contains(&fp) {
            return Ok(ServerCertVerified::assertion());
        }
        Err(rustls::Error::General(format!(
            "iter_data presented certificate sha256:{fp}, which this engine does not trust (pinned: {}). If the server's certificate really changed, run `iter_engine key trust` again",
            pins.iter().map(|p| format!("sha256:{p}")).collect::<Vec<_>>().join(", ")
        )))
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// What a client was set up to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// plain http to this machine
    Loopback,
    /// pinned fingerprints (pinned or not yet)
    Pin,
    /// certificate authorities (mode ca)
    Ca,
}

/// An HTTP client for iter_data at `base`: pinned TLS (or CA checks in mode
/// ca), plain only to this machine. The PinState records what each
/// handshake presented (None for loopback http).
pub fn client(base: &str, timeout: Duration) -> Result<(reqwest::blocking::Client, Option<Arc<PinState>>, Check), String> {
    if let Some(why) = refuse_reason(base) {
        return Err(why);
    }
    let b = reqwest::blocking::Client::builder().timeout(timeout);
    if !base.trim().to_ascii_lowercase().starts_with("https://") {
        return Ok((b.build().map_err(|e| e.to_string())?, None, Check::Loopback));
    }
    let entry = load(base).unwrap_or_default();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let (ca, check) = if entry.mode == "ca" {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let v = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(|e| e.to_string())?;
        (Some(v), Check::Ca)
    } else {
        (None, Check::Pin)
    };
    let state = Arc::new(PinState { pins: RwLock::new(if check == Check::Pin { entry.pins } else { Vec::new() }), ..Default::default() });
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { state: state.clone(), provider, ca }))
        .with_no_client_auth();
    Ok((b.use_preconfigured_tls(cfg).build().map_err(|e| e.to_string())?, Some(state), check))
}

/// What `base` presents now, met with nothing pinned and nothing saved: its
/// certificate's fingerprint and the chain (for `key trust` and `key join`,
/// which decide what to trust from it).
pub fn probe(base: &str) -> Result<(String, Vec<Vec<u8>>), String> {
    if let Some(why) = refuse_reason(base) {
        return Err(why);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let state = Arc::new(PinState::default());
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { state: state.clone(), provider, ca: None }))
        .with_no_client_auth();
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).use_preconfigured_tls(cfg).build().map_err(|e| e.to_string())?;
    let resp = http.get(format!("{}/health", base.trim_end_matches('/'))).send().map_err(|e| format!("could not reach {base}: {e}"))?;
    let _ = resp.text();
    Ok((state.seen(), state.chain()))
}

/// After a signed sign-in over `state`'s connection, on first contact or
/// later: a certificate a public authority issued for the URL's host means
/// mode ca from now on (a TLS front, or a certificate of your own; it may be
/// renewed); otherwise the fingerprint the engine saw must be one iter_data
/// says it serves, and the pins become what it announced (or the one seen,
/// when it announced none: iter_data runs without TLS behind a front). Saved
/// when they changed.
pub fn adopt_announced(base: &str, state: &PinState, announced: &[String]) -> Result<(), String> {
    let seen = state.seen();
    if seen.is_empty() {
        return Ok(());
    }
    if !state.pinned() && publicly_valid(base, &state.chain()) {
        save(base, &Entry { mode: "ca".into(), pins: Vec::new() })?;
        if let Ok(mut p) = state.pins.write() {
            *p = vec![seen.clone()];
        }
        eprintln!("[engine] {base} presents a certificate a public authority issued for {}: checked against certificate authorities from now on", host(base));
        return Ok(());
    }
    if !announced.is_empty() && !announced.contains(&seen) {
        return Err(format!(
            "the certificate this engine sees (sha256:{seen}) is not one iter_data serves ({}): something between them presents its own",
            announced.iter().map(|p| format!("sha256:{p}")).collect::<Vec<_>>().join(", ")
        ));
    }
    let want: Vec<String> = if announced.is_empty() { vec![seen.clone()] } else { announced.to_vec() };
    let first = !state.pinned();
    let changed = state.pins.read().map(|p| *p != want).unwrap_or(true);
    if changed {
        if let Ok(mut p) = state.pins.write() {
            *p = want.clone();
        }
        save(base, &Entry { mode: "pin".into(), pins: want.clone() })?;
        if first {
            // stderr: stdout carries what a command prints for scripts (key session's token)
            eprintln!("[engine] now trusting iter_data's certificate sha256:{seen} (compare it with the one in the webui's Engine keys screen)");
        } else {
            eprintln!("[engine] iter_data's certificates are now {}", want.iter().map(|p| format!("sha256:{p}")).collect::<Vec<_>>().join(", "));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_hosts_and_the_http_rule() {
        assert_eq!(origin("https://Iter.Example.com:8400/api/x"), "https://iter.example.com:8400");
        assert_eq!(origin("https://iter.example.com"), "https://iter.example.com:443");
        assert_eq!(origin("http://127.0.0.1:8400"), "http://127.0.0.1:8400");
        assert_eq!(origin("https://[::1]/x"), "https://[::1]:443");
        assert_eq!(origin("https://[::1]:8400"), "https://[::1]:8400");
        assert_eq!(host("https://[::1]:8400/x"), "::1");
        assert_eq!(host("http://localhost:8400"), "localhost");
        assert!(refuse_reason("https://10.0.0.5:8400").is_none());
        assert!(refuse_reason("http://127.0.0.1:8400").is_none());
        assert!(refuse_reason("http://localhost:8400").is_none());
        assert!(refuse_reason("http://[::1]:8400").is_none());
        assert!(refuse_reason("http://10.0.0.5:8400").is_some());
        assert!(refuse_reason("http://iter.example.com").is_some());
        assert!(refuse_reason("ftp://x").is_some());
    }

    #[test]
    fn announced_fingerprints_confirm_and_move_the_pins() {
        let st = PinState::default();
        *st.seen.lock().unwrap() = "aa".into();
        // the server says it serves something else: refused, nothing pinned
        assert!(adopt_announced("https://pin-test.invalid:1", &st, &["bb".into()]).is_err());
        assert!(!st.pinned());
    }
}
