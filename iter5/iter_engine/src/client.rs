//! Blocking HTTP client for the iter_data API.

use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct Api {
    pub base: String,
    pub token: String,
    http: reqwest::blocking::Client,
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

impl Api {
    pub fn new(base: &str, token: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("http client"),
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

    pub fn get(&self, path: &str) -> Result<Value, ApiError> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        Self::handle(resp)
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let resp = self
            .http
            .put(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        Self::handle(resp)
    }

    pub fn delete(&self, path: &str) -> Result<Value, ApiError> {
        let resp = self
            .http
            .delete(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        Self::handle(resp)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let resp = self
            .http
            .post(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .map_err(|e| ApiError { status: 0, body: e.to_string() })?;
        Self::handle(resp)
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

    pub type Handler = dyn Fn(&str, &str, &Value) -> Option<(u16, Value)> + Send + Sync;

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
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls: Arc<Mutex<Vec<(String, String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
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
                    seen.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
                    let Some((code, out)) = handler(&method, &path, &body) else {
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
