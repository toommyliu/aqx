use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::OnceLock;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tiny_http::{Header, Method};

const BODY_LIMIT: u64 = 1 << 20;
const GZIP_FROM: usize = 512;

static ASSETS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

pub struct Request {
    pub method: Method,
    pub path: String,
    query: HashMap<String, String>,
    body: Vec<u8>,
    pub gzip: bool,
    pub last_event: Option<String>,
    inner: tiny_http::Request,
}

impl Request {
    pub fn read(mut inner: tiny_http::Request) -> Result<Request, Refused> {
        let header = |name: &'static str| {
            inner
                .headers()
                .iter()
                .find(|h| h.field.equiv(name))
                .map(|h| h.value.as_str().to_owned())
        };
        if !allowed_host(&header("Host").unwrap_or_default()) {
            return Err(Refused(Box::new(inner), 421, "this host isn't served"));
        }
        let method = inner.method().clone();
        if method == Method::Post
            && !header("Content-Type").is_some_and(|t| t.starts_with("application/json"))
        {
            return Err(Refused(Box::new(inner), 415, "send JSON"));
        }
        let gzip = header("Accept-Encoding").is_some_and(|e| e.contains("gzip"));
        let last_event = header("Last-Event-ID");
        let (path, query) = match inner.url().split_once('?') {
            Some((path, query)) => (path.to_owned(), parse_query(query)),
            None => (inner.url().to_owned(), HashMap::new()),
        };
        let mut body = Vec::new();
        if method == Method::Post
            && inner
                .as_reader()
                .take(BODY_LIMIT)
                .read_to_end(&mut body)
                .is_err()
        {
            return Err(Refused(Box::new(inner), 400, "couldn't read the request"));
        }
        Ok(Request {
            method,
            path: percent_decode(&path),
            query,
            body,
            gzip,
            last_event,
            inner,
        })
    }

    pub fn query(&self, key: &str) -> Option<String> {
        self.query.get(key).cloned()
    }

    pub fn indices(&self, key: &str) -> Vec<usize> {
        self.query
            .get(key)
            .map(|list| list.split(',').filter_map(|i| i.parse().ok()).collect())
            .unwrap_or_default()
    }

    pub fn body<T: DeserializeOwned>(&self) -> Result<T, Failure> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Failure::bad(format!("couldn't read the request: {e}")))
    }

    pub fn respond(self, reply: Reply) {
        let gzip = self.gzip && reply.body.len() >= GZIP_FROM && reply.compressible;
        let body = match (gzip, reply.gzipped) {
            (false, _) => reply.body,
            (true, Some(gzipped)) => gzipped.to_vec(),
            (true, None) => compress(&reply.body),
        };
        let mut response = tiny_http::Response::from_data(body)
            .with_status_code(reply.status)
            .with_header(header("Content-Type", reply.kind))
            .with_header(header("Cache-Control", reply.cache))
            .with_header(header("X-Content-Type-Options", "nosniff"));
        if gzip {
            response = response
                .with_header(header("Content-Encoding", "gzip"))
                .with_header(header("Vary", "Accept-Encoding"));
        }
        let _ = self.inner.respond(response);
    }

    pub fn into_stream(self) -> Option<Box<dyn Write + Send>> {
        let mut raw = self.inner.into_writer();
        let encoding = if self.gzip {
            "Content-Encoding: gzip\r\n"
        } else {
            ""
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nX-Accel-Buffering: no\r\n{encoding}Connection: close\r\n\r\n"
        );
        raw.write_all(head.as_bytes()).ok()?;
        raw.flush().ok()?;
        if self.gzip {
            Some(Box::new(GzEncoder::new(raw, Compression::fast())))
        } else {
            Some(raw)
        }
    }
}

pub struct Refused(Box<tiny_http::Request>, u16, &'static str);

impl Refused {
    pub fn send(self) {
        let Refused(request, status, why) = self;
        let _ = request.respond(tiny_http::Response::from_string(why).with_status_code(status));
    }
}

pub struct Reply {
    status: u16,
    kind: &'static str,
    cache: &'static str,
    compressible: bool,
    gzipped: Option<&'static [u8]>,
    body: Vec<u8>,
}

impl Reply {
    pub fn json(value: &impl Serialize) -> Reply {
        Reply {
            status: 200,
            kind: "application/json",
            cache: "no-store",
            compressible: true,
            gzipped: None,
            body: serde_json::to_vec(value).expect("replies serialize"),
        }
    }

    pub fn status(mut self, status: u16) -> Reply {
        self.status = status;
        self
    }
}

pub struct Failure {
    status: u16,
    why: String,
}

impl Failure {
    pub fn bad(why: impl ToString) -> Failure {
        Failure {
            status: 400,
            why: why.to_string(),
        }
    }

    pub fn conflict(why: impl ToString) -> Failure {
        Failure {
            status: 409,
            why: why.to_string(),
        }
    }

    pub fn internal(why: impl ToString) -> Failure {
        Failure {
            status: 500,
            why: why.to_string(),
        }
    }

    pub fn not_found() -> Failure {
        Failure {
            status: 404,
            why: "not found".into(),
        }
    }

    pub fn into_reply(self) -> Reply {
        Reply::json(&serde_json::json!({ "error": self.why })).status(self.status)
    }
}

pub fn built() -> bool {
    ASSETS.iter().any(|(path, _)| *path == "/index.html")
}

pub fn asset(path: &str) -> Reply {
    let at = |wanted: &str| ASSETS.iter().position(|(served, _)| *served == wanted);
    let at = match at(path) {
        Some(at) => at,
        None if path.starts_with("/assets/") => return Failure::not_found().into_reply(),
        None => match at("/index.html") {
            Some(at) => at,
            None => {
                return Reply {
                    status: 503,
                    kind: "text/plain; charset=utf-8",
                    cache: "no-store",
                    compressible: false,
                    gzipped: None,
                    body: b"The web UI isn't built into this aqx. Install Node 24 or newer, then build aqx again.".to_vec(),
                };
            }
        },
    };
    let (path, bytes) = ASSETS[at];
    let kind = content_type(path);
    let compressible = !matches!(kind, "image/png" | "font/woff2");
    Reply {
        status: 200,
        kind,
        cache: if path.starts_with("/assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        },
        compressible,
        gzipped: compressible.then(|| gzipped(at)),
        body: bytes.to_vec(),
    }
}

fn gzipped(at: usize) -> &'static [u8] {
    static GZIPPED: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    &GZIPPED.get_or_init(|| ASSETS.iter().map(|(_, bytes)| compress(bytes)).collect())[at]
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" | "webmanifest" => "application/manifest+json",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("writing to memory");
    encoder.finish().expect("writing to memory")
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("headers are ASCII")
}

fn allowed_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1") || name.ends_with(".ts.net")
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (!key.is_empty()).then(|| (percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let hex = |byte: u8| (byte as char).to_digit(16);
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = match bytes.get(i + 1..i + 3) {
            Some(&[high, low]) if bytes[i] == b'%' => hex(high).zip(hex(low)),
            _ => None,
        };
        match (bytes[i], escaped) {
            (_, Some((high, low))) => {
                out.push((high * 16 + low) as u8);
                i += 2;
            }
            (b'+', None) => out.push(b' '),
            (byte, None) => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_and_tailnet_hosts_are_served() {
        for host in [
            "127.0.0.1:4970",
            "localhost:4970",
            "[::1]:4970",
            "box.tail1234.ts.net",
        ] {
            assert!(allowed_host(host), "{host}");
        }
        for host in [
            "",
            "evil.example",
            "127.0.0.1.evil.example",
            "ts.net.evil.example",
        ] {
            assert!(!allowed_host(host), "{host}");
        }
    }

    #[test]
    fn queries_decode_percent_escapes_and_plus() {
        let query = parse_query("dir=~%2Faqx%20scripts&q=void+aura&empty=");
        assert_eq!(query["dir"], "~/aqx scripts");
        assert_eq!(query["q"], "void aura");
        assert_eq!(query["empty"], "");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
