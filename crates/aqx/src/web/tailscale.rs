use std::collections::HashMap;
use std::process::{Command, Stdio};

use serde::Deserialize;

pub fn announce(port: u16, serve: bool) {
    let status = match read_status() {
        Ok(status) => status,
        Err(e) => {
            if serve {
                eprintln!("aqx: couldn't read tailscale serve status: {e}");
            }
            return;
        }
    };
    if !connected() {
        if serve {
            eprintln!("aqx: tailscale isn't connected, so aqx web isn't on your tailnet yet");
        }
        return;
    }
    let urls = status.urls(port);
    for url in &urls {
        println!("and on your tailnet at {url}");
    }
    if !serve || !urls.is_empty() {
        return;
    }
    if status.taken(port) {
        eprintln!(
            "aqx: port {port} on your tailnet already serves something else; start aqx web with another --port"
        );
        return;
    }
    let target = format!("http://127.0.0.1:{port}");
    match Command::new("tailscale")
        .args(["serve", "--bg", &format!("--https={port}"), &target])
        .stdin(Stdio::null())
        .status()
    {
        Ok(done) if done.success() => {}
        Ok(done) => eprintln!("aqx: tailscale serve failed ({done})"),
        Err(e) => eprintln!("aqx: couldn't run tailscale: {e}"),
    }
}

fn read_status() -> anyhow::Result<Status> {
    let output = Command::new("tailscale")
        .args(["serve", "status", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    anyhow::ensure!(output.status.success(), "{}", output.status);
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn connected() -> bool {
    Command::new("tailscale")
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| running(&output.stdout))
}

fn running(status: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Backend {
        #[serde(rename = "BackendState")]
        state: String,
    }
    serde_json::from_slice::<Backend>(status).is_ok_and(|backend| backend.state == "Running")
}

#[derive(Deserialize)]
struct Status {
    #[serde(rename = "TCP", default)]
    tcp: HashMap<u16, Listener>,
    #[serde(rename = "Web", default)]
    web: HashMap<String, Site>,
}

#[derive(Deserialize)]
struct Listener {
    #[serde(rename = "HTTP", default)]
    http: bool,
}

#[derive(Deserialize)]
struct Site {
    #[serde(rename = "Handlers", default)]
    handlers: HashMap<String, Handler>,
}

#[derive(Deserialize)]
struct Handler {
    #[serde(rename = "Proxy")]
    proxy: Option<String>,
}

impl Status {
    fn sites(&self) -> impl Iterator<Item = (&str, u16, &Site)> {
        self.web.iter().filter_map(|(key, site)| {
            let (host, port) = key.rsplit_once(':')?;
            Some((host, port.parse().ok()?, site))
        })
    }

    fn urls(&self, port: u16) -> Vec<String> {
        let targets = [
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ];
        let mut urls: Vec<String> = self
            .sites()
            .filter(|(_, _, site)| {
                site.handlers
                    .get("/")
                    .and_then(|h| h.proxy.as_deref())
                    .is_some_and(|proxy| targets.iter().any(|t| t == proxy.trim_end_matches('/')))
            })
            .map(|(host, served, _)| {
                let http = self.tcp.get(&served).is_some_and(|l| l.http);
                match (http, served) {
                    (true, 80) => format!("http://{host}"),
                    (true, _) => format!("http://{host}:{served}"),
                    (false, 443) => format!("https://{host}"),
                    (false, _) => format!("https://{host}:{served}"),
                }
            })
            .collect();
        urls.sort();
        urls
    }

    fn taken(&self, port: u16) -> bool {
        self.tcp.contains_key(&port) || self.sites().any(|(_, served, _)| served == port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(json: &str) -> Status {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn finds_every_root_route_to_the_web_port() {
        let status = status(
            r#"{
                "TCP": {"80": {"HTTP": true}, "443": {"HTTPS": true}, "4970": {"HTTPS": true}, "8443": {"HTTPS": true}, "8080": {"HTTP": true}, "9000": {"TCPForward": "127.0.0.1:9001"}},
                "Web": {
                    "box.tail1234.ts.net:80": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4970"}}},
                    "box.tail1234.ts.net:443": {"Handlers": {"/": {"Proxy": "http://localhost:4970"}}},
                    "box.tail1234.ts.net:8080": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4970"}}},
                    "box.tail1234.ts.net:4970": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4970"}}},
                    "box.tail1234.ts.net:8443": {"Handlers": {"/aqx": {"Proxy": "http://127.0.0.1:4970"}}},
                    "box.tail1234.ts.net:8444": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4971"}}}
                }
            }"#,
        );
        assert_eq!(
            status.urls(4970),
            [
                "http://box.tail1234.ts.net",
                "http://box.tail1234.ts.net:8080",
                "https://box.tail1234.ts.net",
                "https://box.tail1234.ts.net:4970"
            ]
        );
        assert_eq!(status.urls(4971), ["https://box.tail1234.ts.net:8444"]);
        assert!(status.urls(4972).is_empty());
    }

    #[test]
    fn a_port_is_taken_by_any_route_on_it() {
        let status = status(
            r#"{
                "TCP": {"9000": {"TCPForward": "127.0.0.1:9001"}},
                "Web": {"box.tail1234.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4790"}}}}
            }"#,
        );
        assert!(status.taken(9000));
        assert!(status.taken(8443));
        assert!(!status.taken(4970));
        assert!(!self::status("{}").taken(4970));
    }

    #[test]
    fn only_a_running_tailscale_counts_as_connected() {
        assert!(running(br#"{"BackendState": "Running", "Self": {}}"#));
        assert!(!running(br#"{"BackendState": "Stopped"}"#));
        assert!(!running(br#"{"BackendState": "NeedsLogin"}"#));
        assert!(!running(b""));
    }
}
