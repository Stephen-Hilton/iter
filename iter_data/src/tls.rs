//! HTTPS for iter_data (decided 2026-10-17): every engine ⇄ iter_data byte is
//! encrypted. `--tls auto` (the default) makes a self-signed certificate on
//! first start and keeps it beside the secret file; `--tls-cert/--tls-key`
//! serve a certificate you bring (a real one for a domain); `--tls off` is
//! plain HTTP for a loopback dev loop or behind a TLS proxy.
//!
//! Engines do not trust a certificate authority for the self-signed case:
//! each engine pins the certificate's fingerprint (the SHA-256 of its DER),
//! learned on its first signed sign-in or given to it up front
//! (`iter_engine key trust`). The sign-in answer carries the fingerprints
//! iter_data serves now and next (`announced`), so a certificate change
//! reaches every engine over the channel it already trusts before the switch:
//! `POST /api/server/tls/next` makes the next certificate, engines pick its
//! fingerprint up at their next sign-in (at most 12 hours), and
//! `POST /api/server/tls/promote` switches to it without a restart.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const CERT: &str = "tls.crt";
pub const KEY: &str = "tls.key";
pub const NEXT_CERT: &str = "tls.next.crt";
pub const NEXT_KEY: &str = "tls.next.key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// plain HTTP
    Off,
    /// a self-signed certificate iter_data makes and rotates
    Auto,
    /// the certificate given with --tls-cert / --tls-key
    Custom,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Auto => "auto",
            Mode::Custom => "custom",
        }
    }
}

/// What iter_data serves, shared with the API (sign-in answers, the admin
/// screen) and the listener (a promote reloads it).
pub struct TlsState {
    pub mode: Mode,
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    names: Vec<String>,
    current: RwLock<String>,
    next: RwLock<String>,
    config: RwLock<Option<axum_server::tls_rustls::RustlsConfig>>,
}

pub use iter_core::enginekey::fingerprint_der;

/// The fingerprint of the first certificate in a PEM file.
pub fn fingerprint_pem_file(path: &Path) -> Result<String, String> {
    let pem = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let der = rustls_pemfile_first_cert(std::io::BufReader::new(&pem[..])).ok_or_else(|| format!("{}: no certificate in it", path.display()))?;
    Ok(fingerprint_der(&der))
}

/// The first `CERTIFICATE` block's DER (base64 between the PEM markers).
fn rustls_pemfile_first_cert(rd: impl std::io::BufRead) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let mut inside = false;
    let mut b64 = String::new();
    for line in rd.lines().map_while(Result::ok) {
        let l = line.trim();
        if l == "-----BEGIN CERTIFICATE-----" {
            inside = true;
        } else if l == "-----END CERTIFICATE-----" && inside {
            return base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok();
        } else if inside {
            b64.push_str(l);
        }
    }
    None
}

/// Write a file only its owner can read (0600 on unix), replacing it whole.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
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
    restrict(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Narrow an existing secret file to its owner (unix; no-op elsewhere).
pub fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(path) {
            if m.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A new self-signed certificate (ECDSA P-256) for `names`: (cert PEM, key PEM).
pub fn self_signed(names: &[String]) -> Result<(String, String), String> {
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(|e| e.to_string())?;
    params.distinguished_name.push(rcgen::DnType::CommonName, "iter_data");
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "iter");
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    // engines pin the fingerprint, so expiry is never what retires it
    params.not_after = rcgen::date_time_ymd(2125, 12, 31);
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// The names a self-signed certificate carries: localhost, the loopback
/// addresses, this host's name, and every --tls-name.
pub fn default_names(extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Ok(h) = std::process::Command::new("hostname").output() {
        let h = String::from_utf8_lossy(&h.stdout).trim().to_string();
        if !h.is_empty() {
            v.push(h);
        }
    }
    for n in extra.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        v.push(n.to_string());
    }
    v.sort();
    v.dedup();
    v
}

impl TlsState {
    /// No TLS (tests and `--tls off`).
    pub fn off() -> TlsState {
        TlsState {
            mode: Mode::Off,
            dir: PathBuf::new(),
            cert: PathBuf::new(),
            key: PathBuf::new(),
            names: Vec::new(),
            current: RwLock::new(String::new()),
            next: RwLock::new(String::new()),
            config: RwLock::new(None),
        }
    }

    /// Set up from the flags: makes the self-signed certificate on first start.
    pub fn setup(mode: &str, cert: &str, key: &str, dir: &Path, extra_names: &[String]) -> Result<TlsState, String> {
        let mut st = TlsState::off();
        st.dir = dir.to_path_buf();
        st.names = default_names(extra_names);
        match (mode.trim(), cert.trim().is_empty(), key.trim().is_empty()) {
            (_, false, false) => {
                st.mode = Mode::Custom;
                st.cert = PathBuf::from(cert.trim());
                st.key = PathBuf::from(key.trim());
            }
            (_, false, true) | (_, true, false) => return Err("--tls-cert and --tls-key go together".into()),
            ("off", true, true) => return Ok(st),
            ("auto" | "", true, true) => {
                st.mode = Mode::Auto;
                st.cert = dir.join(CERT);
                st.key = dir.join(KEY);
                if !st.cert.exists() || !st.key.exists() {
                    let (c, k) = self_signed(&st.names)?;
                    write_private(&st.key, k.as_bytes())?;
                    write_private(&st.cert, c.as_bytes())?;
                    println!("[iter_data] made a self-signed TLS certificate: {} (names: {})", st.cert.display(), st.names.join(", "));
                }
                restrict(&st.key);
                if let Ok(fp) = fingerprint_pem_file(&dir.join(NEXT_CERT)) {
                    *st.next.write().unwrap() = fp;
                }
            }
            (other, true, true) => return Err(format!("--tls is auto or off, not {other:?}")),
        }
        *st.current.write().unwrap() = fingerprint_pem_file(&st.cert)?;
        Ok(st)
    }

    pub fn fingerprint(&self) -> String {
        self.current.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn next_fingerprint(&self) -> String {
        self.next.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// The fingerprints an engine should trust: the current certificate's
    /// and, while a rotation is under way, the next one's. Empty without TLS.
    pub fn announced(&self) -> Vec<String> {
        [self.fingerprint(), self.next_fingerprint()].into_iter().filter(|f| !f.is_empty()).collect()
    }

    /// What the admin screen shows.
    pub fn describe(&self) -> serde_json::Value {
        let next_created = std::fs::metadata(self.dir.join(NEXT_CERT))
            .and_then(|m| m.modified())
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();
        serde_json::json!({
            "mode": self.mode.as_str(),
            "fingerprint": self.fingerprint(),
            "next_fingerprint": self.next_fingerprint(),
            "next_created": if self.next_fingerprint().is_empty() { String::new() } else { next_created },
            "names": if self.mode == Mode::Auto { self.names.clone() } else { Vec::new() },
            "rotatable": self.mode == Mode::Auto,
        })
    }

    /// Make the next self-signed certificate (replacing an earlier next one).
    pub fn make_next(&self) -> Result<String, String> {
        if self.mode != Mode::Auto {
            return Err(format!("iter_data serves {} TLS: only its own self-signed certificate (--tls auto) rotates here", self.mode.as_str()));
        }
        let (c, k) = self_signed(&self.names)?;
        write_private(&self.dir.join(NEXT_KEY), k.as_bytes())?;
        write_private(&self.dir.join(NEXT_CERT), c.as_bytes())?;
        let fp = fingerprint_pem_file(&self.dir.join(NEXT_CERT))?;
        *self.next.write().unwrap() = fp.clone();
        Ok(fp)
    }

    /// Serve the next certificate from now on (no restart).
    pub async fn promote(&self) -> Result<String, String> {
        let next = self.next_fingerprint();
        if self.mode != Mode::Auto || next.is_empty() {
            return Err("there is no next certificate: make one first".into());
        }
        let config = self.config.read().unwrap().clone();
        if let Some(cfg) = &config {
            cfg.reload_from_pem_file(self.dir.join(NEXT_CERT), self.dir.join(NEXT_KEY)).await.map_err(|e| e.to_string())?;
        }
        std::fs::rename(self.dir.join(NEXT_KEY), &self.key).map_err(|e| e.to_string())?;
        std::fs::rename(self.dir.join(NEXT_CERT), &self.cert).map_err(|e| e.to_string())?;
        *self.current.write().unwrap() = next.clone();
        self.next.write().unwrap().clear();
        Ok(next)
    }

    /// The listener's TLS configuration (None when the mode is off), kept so
    /// a promote can reload it in place.
    pub async fn rustls_config(&self) -> Result<Option<axum_server::tls_rustls::RustlsConfig>, String> {
        if self.mode == Mode::Off {
            return Ok(None);
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&self.cert, &self.key)
            .await
            .map_err(|e| format!("TLS certificate {} / key {}: {e}", self.cert.display(), self.key.display()))?;
        *self.config.write().unwrap() = Some(cfg.clone());
        Ok(Some(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_makes_keeps_and_rotates_its_certificate() {
        let dir = std::env::temp_dir().join(format!("iter-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = TlsState::setup("auto", "", "", &dir, &["iter.example.com".into()]).unwrap();
        assert_eq!(st.mode, Mode::Auto);
        let fp = st.fingerprint();
        assert_eq!(fp.len(), 64);
        assert_eq!(st.announced(), vec![fp.clone()]);
        assert!(st.describe()["names"].as_array().unwrap().iter().any(|n| n == "iter.example.com"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(KEY)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // a restart keeps the same certificate
        assert_eq!(TlsState::setup("auto", "", "", &dir, &[]).unwrap().fingerprint(), fp);
        // rotation: the next one is announced beside the current one, then replaces it
        let next = st.make_next().unwrap();
        assert_ne!(next, fp);
        assert_eq!(st.announced(), vec![fp.clone(), next.clone()]);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert_eq!(rt.block_on(st.promote()).unwrap(), next);
        assert_eq!(st.announced(), vec![next.clone()]);
        assert_eq!(TlsState::setup("auto", "", "", &dir, &[]).unwrap().fingerprint(), next, "the promoted certificate is the one kept");
        assert!(rt.block_on(st.promote()).is_err(), "nothing left to promote");
        // off and custom
        assert_eq!(TlsState::setup("off", "", "", &dir, &[]).unwrap().announced(), Vec::<String>::new());
        let custom = TlsState::setup("auto", dir.join(CERT).to_str().unwrap(), dir.join(KEY).to_str().unwrap(), &dir, &[]).unwrap();
        assert_eq!((custom.mode, custom.fingerprint()), (Mode::Custom, next));
        assert!(custom.make_next().is_err());
        assert!(TlsState::setup("auto", "x.crt", "", &dir, &[]).is_err());
        assert!(TlsState::setup("maybe", "", "", &dir, &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
