//! HTTPS for iter_data (decided 2026-10-17): every engine ⇄ iter_data byte is
//! encrypted. `--tls auto` (the default) makes a self-signed certificate on
//! first start and keeps it beside the secret file; `--tls-cert/--tls-key`
//! serve a certificate you bring (a real one for a domain); `--tls off` is
//! plain HTTP for a loopback dev loop or behind a TLS proxy.
//!
//! Browsers (decided 2026-10-19: the webui must show as secure): `--tls auto`
//! also makes a local certificate authority of its own (state/tls-ca.crt +
//! tls-ca.key) and signs its certificate with it; tls.crt holds the server
//! certificate followed by the CA. The installers add that CA to the trust
//! stores of the machine they install on (iter_data's own, and every engine's,
//! fetched over the engine's pinned connection from `GET /tls/ca.crt`), so a
//! browser there shows the webui as secure. The CA can only vouch for this
//! server: its name constraints permit the certificate's own names, `.local`,
//! `.lan`, `.internal`, `.home.arpa`, loopback and private addresses, nothing
//! on the public internet that is not one of `--tls-name`. A rotation signs the
//! next certificate with the same CA, so browsers keep trusting it. A
//! certificate from before the CA (self-signed) stays in use; a CA-signed next
//! one is made beside it at the next start, and the admin switches to it from
//! the Engine keys screen as with any rotation.
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
pub const CA_CERT: &str = "tls-ca.crt";
pub const CA_KEY: &str = "tls-ca.key";

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

/// A self-signed certificate (ECDSA P-256) for `names`: (cert PEM, key PEM),
/// as `--tls auto` made before the local CA; kept for the tests that start
/// from one.
#[cfg(test)]
fn self_signed(names: &[String]) -> Result<(String, String), String> {
    let params = leaf_params(names)?;
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn leaf_params(names: &[String]) -> Result<rcgen::CertificateParams, String> {
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(|e| e.to_string())?;
    params.distinguished_name.push(rcgen::DnType::CommonName, "iter_data");
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "iter");
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    // engines pin the fingerprint, so expiry is never what retires it
    params.not_after = rcgen::date_time_ymd(2125, 12, 31);
    // macOS and Windows want a TLS server certificate to say so
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    Ok(params)
}

/// Local DNS suffixes the CA may vouch for besides the certificate's own names.
const LOCAL_SUFFIXES: &[&str] = &["local", "lan", "internal", "home.arpa"];
/// Loopback and private address ranges the CA may vouch for (address, prefix).
const LOCAL_NETS: &[(&str, u8)] = &[
    ("127.0.0.0", 8),
    ("10.0.0.0", 8),
    ("172.16.0.0", 12),
    ("192.168.0.0", 16),
    ("100.64.0.0", 10),
    ("169.254.0.0", 16),
    ("::1", 128),
    ("fc00::", 7),
    ("fe80::", 10),
];

/// The CA's name constraints: `names` exactly, plus the local suffixes and
/// address ranges above.
fn ca_constraints(names: &[String]) -> rcgen::NameConstraints {
    use rcgen::{CidrSubnet, GeneralSubtree};
    let mut permitted = Vec::new();
    let mut dns: Vec<String> = LOCAL_SUFFIXES.iter().map(|s| s.to_string()).collect();
    for n in names {
        match n.parse::<std::net::IpAddr>() {
            Ok(ip) => permitted.push(GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(ip, if ip.is_ipv4() { 32 } else { 128 }))),
            Err(_) => dns.push(n.trim_end_matches('.').to_ascii_lowercase()),
        }
    }
    dns.sort();
    dns.dedup();
    permitted.extend(dns.into_iter().map(GeneralSubtree::DnsName));
    for (a, p) in LOCAL_NETS {
        permitted.push(GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(a.parse().unwrap(), *p)));
    }
    rcgen::NameConstraints { permitted_subtrees: permitted, excluded_subtrees: Vec::new() }
}

/// A new local CA for `names`: (cert PEM, key PEM).
pub fn new_ca(names: &[String]) -> Result<(String, String), String> {
    let mut params = rcgen::CertificateParams::default();
    let host = names.iter().find(|n| *n != "localhost" && n.parse::<std::net::IpAddr>().is_err()).cloned().unwrap_or_else(|| "localhost".into());
    let id: String = {
        use rand::Rng;
        rand::thread_rng().sample_iter(&rand::distributions::Alphanumeric).take(6).map(char::from).collect()
    };
    // a name a person recognises in a trust store, unique per install
    params.distinguished_name.push(rcgen::DnType::CommonName, format!("iter_data local CA ({host}, {id})"));
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "iter");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(ca_constraints(names));
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2125, 12, 31);
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// A certificate for `names` signed by the CA in `dir`: (cert PEM followed by
/// the CA's, key PEM).
pub fn ca_signed(dir: &Path, names: &[String]) -> Result<(String, String), String> {
    let ca_pem = std::fs::read_to_string(dir.join(CA_CERT)).map_err(|e| format!("{CA_CERT}: {e}"))?;
    let ca_key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(dir.join(CA_KEY)).map_err(|e| format!("{CA_KEY}: {e}"))?).map_err(|e| format!("{CA_KEY}: {e}"))?;
    // the issuer: the CA's own parameters with its key (same subject, same key id)
    let ca = rcgen::CertificateParams::from_ca_cert_pem(&ca_pem).map_err(|e| format!("{CA_CERT}: {e}"))?.self_signed(&ca_key).map_err(|e| e.to_string())?;
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = leaf_params(names)?.signed_by(&key, &ca, &ca_key).map_err(|e| e.to_string())?;
    Ok((format!("{}{}", cert.pem(), ca_pem), key.serialize_pem()))
}

/// The first non-loopback address this machine reaches the network from, when
/// it is a private one (no packet is sent: a UDP socket only picks a route).
fn lan_address() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if v4.is_private() => Some(v4.to_string()),
        _ => None,
    }
}

/// The names iter_data's own certificate carries: localhost, the loopback
/// addresses, this host's name (and `<name>.local`), its private LAN address,
/// and every --tls-name.
pub fn default_names(extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Ok(h) = std::process::Command::new("hostname").output() {
        let h = String::from_utf8_lossy(&h.stdout).trim().to_string();
        if !h.is_empty() {
            if !h.contains('.') {
                v.push(format!("{h}.local"));
            }
            v.push(h);
        }
    }
    v.extend(lan_address());
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
                let had_cert = st.cert.exists() && st.key.exists();
                let had_ca = dir.join(CA_CERT).exists() && dir.join(CA_KEY).exists();
                if !had_ca {
                    let (c, k) = new_ca(&st.names)?;
                    write_private(&dir.join(CA_KEY), k.as_bytes())?;
                    write_private(&dir.join(CA_CERT), c.as_bytes())?;
                    println!("[iter_data] made a local certificate authority for browsers to trust: {}", dir.join(CA_CERT).display());
                }
                if !had_cert {
                    let (c, k) = ca_signed(dir, &st.names)?;
                    write_private(&st.key, k.as_bytes())?;
                    write_private(&st.cert, c.as_bytes())?;
                    println!("[iter_data] made a TLS certificate signed by that CA: {} (names: {})", st.cert.display(), st.names.join(", "));
                }
                restrict(&st.key);
                restrict(&dir.join(CA_KEY));
                if let Ok(fp) = fingerprint_pem_file(&dir.join(NEXT_CERT)) {
                    *st.next.write().unwrap() = fp;
                }
                // a certificate from before the CA: a CA-signed next one, announced
                // to engines at their next sign-in, for the admin to switch to
                if had_cert && !had_ca && st.next.read().unwrap().is_empty() {
                    let fp = st.make_next()?;
                    println!("[iter_data] the current TLS certificate is self-signed: made a next one signed by the local CA (sha256:{fp}); switch to it in the Engine keys screen once every engine has signed in");
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

    /// The names on iter_data's own certificate (none without TLS of its own).
    pub fn names(&self) -> Vec<String> {
        if self.mode == Mode::Off { Vec::new() } else { self.names.clone() }
    }

    pub fn next_fingerprint(&self) -> String {
        self.next.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// The local CA's certificate (PEM), which browsers are told to trust;
    /// None unless iter_data makes its own certificates (--tls auto).
    pub fn ca_pem(&self) -> Option<String> {
        if self.mode != Mode::Auto {
            return None;
        }
        std::fs::read_to_string(self.dir.join(CA_CERT)).ok()
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
            "ca_fingerprint": self.ca_pem().and_then(|_| fingerprint_pem_file(&self.dir.join(CA_CERT)).ok()).unwrap_or_default(),
        })
    }

    /// Make the next certificate, signed by the local CA (replacing an earlier next one).
    pub fn make_next(&self) -> Result<String, String> {
        if self.mode != Mode::Auto {
            return Err(format!("iter_data serves {} TLS: only its own self-signed certificate (--tls auto) rotates here", self.mode.as_str()));
        }
        let (c, k) = ca_signed(&self.dir, &self.names)?;
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

    fn ders(pem: &str) -> Vec<Vec<u8>> {
        use base64::Engine as _;
        pem.split("-----BEGIN CERTIFICATE-----")
            .skip(1)
            .map(|b| {
                let b64: String = b.split("-----END CERTIFICATE-----").next().unwrap().split_whitespace().collect();
                base64::engine::general_purpose::STANDARD.decode(b64).unwrap()
            })
            .collect()
    }

    /// What a browser does: the server certificate chains to the trusted CA,
    /// is a server certificate, and the CA's name constraints allow `name`.
    fn browser_accepts(chain_pem: &str, ca_pem: &str, name: &str) -> Result<(), String> {
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        let ca = CertificateDer::from(ders(ca_pem).remove(0));
        let anchor = webpki::anchor_from_trusted_cert(&ca).map_err(|e| e.to_string())?;
        let leaf = CertificateDer::from(ders(chain_pem).remove(0));
        let ee = webpki::EndEntityCert::try_from(&leaf).map_err(|e| e.to_string())?;
        ee.verify_for_usage(webpki::ALL_VERIFICATION_ALGS, &[anchor], &[], UnixTime::now(), webpki::KeyUsage::server_auth(), None, None)
            .map_err(|e| e.to_string())?;
        ee.verify_is_valid_for_subject_name(&ServerName::try_from(name.to_string()).unwrap()).map_err(|e| e.to_string())
    }

    #[test]
    fn auto_mode_signs_with_a_name_constrained_local_ca() {
        let dir = std::env::temp_dir().join(format!("iter-tls-ca-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = TlsState::setup("auto", "", "", &dir, &["iter.example.com".into(), "203.0.113.7".into()]).unwrap();
        let ca = st.ca_pem().expect("a CA in auto mode");
        let chain = std::fs::read_to_string(dir.join(CERT)).unwrap();
        assert_eq!(ders(&chain).len(), 2, "tls.crt is the server certificate followed by the CA");
        assert_eq!(ders(&chain)[1], ders(&ca)[0]);
        for name in ["localhost", "127.0.0.1", "iter.example.com", "203.0.113.7"] {
            browser_accepts(&chain, &ca, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert!(browser_accepts(&chain, &ca, "elsewhere.example.com").is_err(), "a name the certificate does not carry");
        // the CA cannot vouch for a public name it was not made for
        let (bad, _) = ca_signed(&dir, &["www.google.com".into()]).unwrap();
        assert!(browser_accepts(&bad, &ca, "www.google.com").is_err(), "name constraints must refuse a public name");
        let (bad_ip, _) = ca_signed(&dir, &["8.8.8.8".into()]).unwrap();
        assert!(browser_accepts(&bad_ip, &ca, "8.8.8.8").is_err(), "name constraints must refuse a public address");
        let (lan, _) = ca_signed(&dir, &["box.local".into(), "192.168.1.20".into()]).unwrap();
        browser_accepts(&lan, &ca, "box.local").unwrap();
        browser_accepts(&lan, &ca, "192.168.1.20").unwrap();
        // a rotation is signed by the same CA, so browsers keep trusting it
        st.make_next().unwrap();
        let next = std::fs::read_to_string(dir.join(NEXT_CERT)).unwrap();
        browser_accepts(&next, &ca, "localhost").unwrap();
        assert!(!st.describe()["ca_fingerprint"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_self_signed_certificate_from_before_the_ca_gets_a_ca_signed_next_one() {
        let dir = std::env::temp_dir().join(format!("iter-tls-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (c, k) = self_signed(&default_names(&[])).unwrap();
        write_private(&dir.join(KEY), k.as_bytes()).unwrap();
        write_private(&dir.join(CERT), c.as_bytes()).unwrap();
        let old = fingerprint_pem_file(&dir.join(CERT)).unwrap();
        let st = TlsState::setup("auto", "", "", &dir, &[]).unwrap();
        assert_eq!(st.fingerprint(), old, "the pinned certificate stays in use");
        assert_eq!(st.announced().len(), 2, "the CA-signed next one is announced to engines");
        let next = std::fs::read_to_string(dir.join(NEXT_CERT)).unwrap();
        browser_accepts(&next, &st.ca_pem().unwrap(), "localhost").unwrap();
        // the next start does not make yet another one
        let again = TlsState::setup("auto", "", "", &dir, &[]).unwrap();
        assert_eq!(again.next_fingerprint(), st.next_fingerprint());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
