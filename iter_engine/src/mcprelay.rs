//! The local MCP relay (2026-10-17). An agent's `claude` reaches iter_data's
//! MCP endpoint (`POST /mcp`, work.rs mcp_config) itself, and `claude` would
//! refuse iter_data's self-signed certificate: it knows nothing of the
//! engine's pinned trust (trust.rs). So when iter_data is reached over
//! https, the engine listens on 127.0.0.1 (a port the system picks) and
//! forwards each `/mcp` request over its own pinned TLS connection, headers
//! (the agent's bearer token among them) and body as they came. Nothing
//! leaves the machine unencrypted, and nothing is forwarded until the
//! server's certificate is trusted. Over loopback http (a dev loop) agents
//! call iter_data directly.

use crate::client::Api;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

/// How long one forwarded MCP call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
/// The largest request body accepted (an MCP call is a small JSON-RPC message).
const MAX_BODY: usize = 8 << 20;

static RELAY: OnceLock<Option<String>> = OnceLock::new();

/// The MCP URL an agent's `claude` should use: iter_data's own over loopback
/// http, else this engine's relay (started on first use). None when the
/// relay cannot listen.
pub fn mcp_url(api: &Api) -> Option<String> {
    if !api.base.to_ascii_lowercase().starts_with("https://") {
        return Some(format!("{}/mcp", api.base.trim_end_matches('/')));
    }
    RELAY
        .get_or_init(|| match start(api.clone()) {
            Ok(base) => Some(base),
            Err(e) => {
                eprintln!("[engine] MCP relay could not start ({e}): agents run without the iter MCP tools");
                None
            }
        })
        .as_ref()
        .map(|b| format!("{b}/mcp"))
}

fn start(api: Api) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let api = api.clone();
            std::thread::spawn(move || {
                if let Err(e) = serve_one(stream, &api) {
                    eprintln!("[engine] MCP relay: {e}");
                }
            });
        }
    });
    Ok(format!("http://{addr}"))
}

/// Hop-by-hop headers (and the ones the relay sets itself) are not forwarded.
fn forwardable(name: &str) -> bool {
    !matches!(name, "host" | "connection" | "keep-alive" | "content-length" | "transfer-encoding" | "te" | "upgrade" | "proxy-connection" | "accept-encoding")
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_request(rd: &mut impl BufRead) -> Result<Request, String> {
    let mut line = String::new();
    rd.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || !path.starts_with('/') {
        return Err("not an HTTP request".into());
    }
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if rd.read_line(&mut h).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let get = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        loop {
            let mut size = String::new();
            rd.read_line(&mut size).map_err(|e| e.to_string())?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).map_err(|_| "bad chunk size".to_string())?;
            if n == 0 {
                let mut end = String::new();
                let _ = rd.read_line(&mut end);
                break;
            }
            if body.len() + n > MAX_BODY {
                return Err("request body too large".into());
            }
            let mut chunk = vec![0u8; n];
            rd.read_exact(&mut chunk).map_err(|e| e.to_string())?;
            body.extend_from_slice(&chunk);
            let mut crlf = [0u8; 2];
            rd.read_exact(&mut crlf).map_err(|e| e.to_string())?;
        }
    } else if let Some(n) = get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        if n > MAX_BODY {
            return Err("request body too large".into());
        }
        body = vec![0u8; n];
        rd.read_exact(&mut body).map_err(|e| e.to_string())?;
    }
    Ok(Request { method, path, headers, body })
}

fn respond(out: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) -> Result<(), String> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    out.write_all(head.as_bytes()).and_then(|_| out.write_all(body)).and_then(|_| out.flush()).map_err(|e| e.to_string())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Status",
    }
}

fn serve_one(stream: TcpStream, api: &Api) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let mut out = stream.try_clone().map_err(|e| e.to_string())?;
    let mut rd = BufReader::new(stream);
    let req = read_request(&mut rd)?;
    let json = |msg: &str| serde_json::json!({"error": msg}).to_string().into_bytes();
    let ctype = vec![("Content-Type".to_string(), "application/json".to_string())];
    if req.path != "/mcp" && !req.path.starts_with("/mcp?") {
        return respond(&mut out, 404, &ctype, &json("the engine's relay forwards /mcp only"));
    }
    if !api.trusted() {
        return respond(&mut out, 502, &ctype, &json("the engine does not trust iter_data's certificate yet"));
    }
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut fwd = api.http().request(method, format!("{}{}", api.base.trim_end_matches('/'), req.path)).timeout(CALL_TIMEOUT);
    for (k, v) in req.headers.iter().filter(|(k, _)| forwardable(k)) {
        fwd = fwd.header(k.as_str(), v.as_str());
    }
    if !req.body.is_empty() {
        fwd = fwd.body(req.body);
    }
    match fwd.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let keep: Vec<(String, String)> = resp
                .headers()
                .iter()
                .filter(|(k, _)| forwardable(k.as_str()) && k.as_str() != "content-encoding")
                .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string())))
                .collect();
            let body = resp.bytes().map(|b| b.to_vec()).unwrap_or_default();
            respond(&mut out, status, &keep, &body)
        }
        Err(e) => respond(&mut out, 502, &ctype, &json(&format!("iter_data: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn reads_plain_and_chunked_requests() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t\r\nContent-Length: 5\r\n\r\nhello";
        let r = read_request(&mut BufReader::new(&raw[..])).unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str(), r.body.as_slice()), ("POST", "/mcp", &b"hello"[..]));
        assert!(r.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer t"));
        let chunked = b"POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(read_request(&mut BufReader::new(&chunked[..])).unwrap().body, b"abcde");
        assert!(read_request(&mut BufReader::new(&b"garbage"[..])).is_err());
        assert!(!forwardable("host") && !forwardable("content-length") && forwardable("authorization") && forwardable("mcp-session-id"));
    }

    #[test]
    fn loopback_http_needs_no_relay() {
        let api = Api::new("http://127.0.0.1:9", "t");
        assert_eq!(mcp_url(&api).as_deref(), Some("http://127.0.0.1:9/mcp"));
    }

    #[test]
    fn the_relay_forwards_mcp_calls_with_their_headers() {
        // a loopback-http "iter_data" stands in; the relay is started by hand
        let srv = crate::client::fake::serve_with_bearer(|bearer, m, p, b| Some((200, serde_json::json!({"bearer": bearer, "m": m, "p": p, "b": b}))));
        let base = start(Api::new(&srv.base, "")).unwrap();
        let mut s = TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        write!(s, "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer agent-tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(resp.contains(r#""bearer":"agent-tok""#) && resp.contains(r#""method":"tools/list""#), "{resp}");
        let mut s = TcpStream::connect(base.trim_start_matches("http://")).unwrap();
        write!(s, "GET /api/engines HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 404"), "only /mcp is relayed: {resp}");
    }
}
