use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fmt, fs, io, process};

use mira_protocol::flex;
use serde::Deserialize;
use serde_json::Value;
use ureq::config::AutoHeaderValue;

const API_BASE: &str = "https://game.aq.com/game/api";
const TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "Ruffle/0.7.1 (https://ruffle.rs)";
const DEVICE_ID_NOTE: &str = "\
# AQW's Ruffle player sends a device fingerprint as x-device-id.
# mira sends this random id instead. Delete this file to make a new one.";

#[derive(Debug, Clone, Deserialize)]
pub struct Server {
    #[serde(rename = "sName")]
    pub name: String,
    #[serde(rename = "sIP")]
    pub host: String,
    #[serde(rename = "iPort", deserialize_with = "flex::int")]
    pub port: u16,
    #[serde(rename = "iCount", deserialize_with = "flex::int", default)]
    pub players: u32,
    #[serde(rename = "iMax", deserialize_with = "flex::int", default)]
    pub capacity: u32,
    #[serde(rename = "bOnline", deserialize_with = "flex::bool", default)]
    pub online: bool,
    #[serde(rename = "bUpg", deserialize_with = "flex::bool", default)]
    pub members_only: bool,
}

impl Server {
    pub fn full(&self) -> bool {
        self.capacity > 0 && self.players >= self.capacity
    }
}

#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_tests(token: &str) -> Token {
        Token(token.into())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub struct Account {
    pub username: String,
    pub token: Token,
    pub member: bool,
    pub servers: Vec<Server>,
    pub device: DeviceId,
    pub(crate) api: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Http(String),
    #[error("login rejected: {0}")]
    Rejected(String),
    #[error("{0}")]
    NoServer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn load() -> io::Result<DeviceId> {
        let dir = dirs::data_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no data directory"))?;
        DeviceId::load_or_create(&dir.join("mira").join("device-id"))
    }

    fn load_or_create(path: &Path) -> io::Result<DeviceId> {
        match fs::read_to_string(path) {
            Ok(text) => {
                return DeviceId::parse(&text).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{} is not a device id; delete it to make a new one",
                            path.display()
                        ),
                    )
                });
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            Err(_) => {}
        }
        let id = DeviceId::random()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let staged = path.with_extension(format!("{}.tmp", process::id()));
        fs::write(&staged, format!("{DEVICE_ID_NOTE}\n{}\n", id.0))?;
        let placed = fs::hard_link(&staged, path);
        fs::remove_file(&staged)?;
        match placed {
            Ok(()) => Ok(id),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => DeviceId::load_or_create(path),
            Err(e) => Err(e),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> DeviceId {
        DeviceId("0123456789abcdef0123456789abcdef".into())
    }

    fn random() -> io::Result<DeviceId> {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(DeviceId(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    fn parse(text: &str) -> Option<DeviceId> {
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'));
        let (Some(id), None) = (lines.next(), lines.next()) else {
            return None;
        };
        let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
        (id.len() == 32 && id.chars().all(hex)).then(|| DeviceId(id.to_owned()))
    }
}

fn request<B>(request: ureq::RequestBuilder<B>, device: &DeviceId) -> ureq::RequestBuilder<B> {
    request
        .header("x-device-id", &device.0)
        .config()
        .user_agent(USER_AGENT)
        .accept("*/*")
        .accept_encoding(AutoHeaderValue::None)
        .timeout_global(Some(TIMEOUT))
        .build()
}

pub fn servers(device: &DeviceId) -> Result<Vec<Server>, ApiError> {
    let body = request(ureq::get(format!("{API_BASE}/data/servers")), device)
        .header("content-type", "")
        .call()
        .and_then(|mut reply| reply.body_mut().read_to_string())
        .map_err(|e| ApiError::Http(format!("server list request failed: {e}")))?;
    serde_json::from_str(&body).map_err(|e| ApiError::Http(format!("unreadable server list: {e}")))
}

pub fn login(device: &DeviceId, username: &str, password: &str) -> Result<Account, ApiError> {
    let url = format!("{API_BASE}/login/now?ran={}", cache_buster());
    let body = request(ureq::post(url), device)
        .send_form([("user", username), ("pass", password), ("option", "1")])
        .and_then(|mut reply| reply.body_mut().read_to_string())
        .map_err(|e| ApiError::Http(format!("login request failed: {e}")))?;
    parse_login(device, username, &body)
}

fn cache_buster() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("0.{nanos:09}")
}

pub(crate) fn bank(account: &Account, char_id: i64) -> Result<Vec<Value>, ApiError> {
    let url = format!("{}/char/bank?v={}", account.api, cache_buster());
    let body = request(ureq::post(url), &account.device)
        .header("ccid", char_id.to_string())
        .header("token", account.token.as_str())
        .send_form([("layout", r#"{"cat":"all"}"#)])
        .and_then(|mut reply| reply.body_mut().read_to_string())
        .map_err(|e| ApiError::Http(format!("bank request failed: {e}")))?;
    serde_json::from_str(&body).map_err(|e| ApiError::Http(format!("unreadable bank: {e}")))
}

fn parse_login(device: &DeviceId, username: &str, body: &str) -> Result<Account, ApiError> {
    #[derive(Deserialize)]
    struct Reply {
        login: Option<Grant>,
        #[serde(default)]
        servers: Vec<Server>,
        #[serde(rename = "sMsg")]
        message: Option<String>,
    }
    #[derive(Deserialize)]
    struct Grant {
        #[serde(rename = "bSuccess", deserialize_with = "flex::bool", default)]
        ok: bool,
        #[serde(rename = "sToken", default)]
        token: String,
        #[serde(rename = "iUpg", deserialize_with = "flex::int", default)]
        upgrade: i64,
        #[serde(rename = "sMsg")]
        message: Option<String>,
    }
    let reply: Reply = serde_json::from_str(body)
        .map_err(|e| ApiError::Http(format!("unreadable login reply: {e}")))?;
    match reply.login {
        Some(grant) if grant.ok && !grant.token.is_empty() => Ok(Account {
            username: username.to_owned(),
            token: Token(grant.token),
            member: grant.upgrade > 0,
            servers: reply.servers,
            device: device.clone(),
            api: API_BASE.to_owned(),
        }),
        grant => Err(ApiError::Rejected(
            grant
                .and_then(|g| g.message)
                .or(reply.message)
                .unwrap_or_else(|| "no token in the reply".into()),
        )),
    }
}

impl Account {
    pub fn pick_server(&self, name: &str) -> Result<&Server, ApiError> {
        let Some(server) = self
            .servers
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
        else {
            let names: Vec<&str> = self.servers.iter().map(|s| s.name.as_str()).collect();
            return Err(ApiError::NoServer(format!(
                "no server named {name}; the servers are {}",
                names.join(", ")
            )));
        };
        let refusal = if server.members_only && !self.member {
            "is for members only"
        } else if !server.online {
            "is offline"
        } else if server.full() {
            "is full"
        } else {
            return Ok(server);
        };
        Err(ApiError::NoServer(format!("{} {refusal}", server.name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"login":{"bSuccess":1,"sToken":"tok","iUpg":0},
        "servers":[{"sName":"Twilly","sIP":"10.0.0.1","iPort":5588,"iCount":900,"iMax":1000,"bOnline":1,"bUpg":0},
                   {"sName":"Gravelyn","sIP":"10.0.0.2","iPort":"5589","iCount":1000,"iMax":1000,"bOnline":1,"bUpg":0},
                   {"sName":"Sir Ver","sIP":"10.0.0.3","iPort":5590,"iCount":10,"iMax":1000,"bOnline":1,"bUpg":1}]}"#;

    fn test_device() -> DeviceId {
        DeviceId("0123456789abcdef0123456789abcdef".into())
    }

    #[test]
    fn login_reply_gives_the_token_and_servers() {
        let account = parse_login(&test_device(), "player1", REPLY).unwrap();
        assert_eq!(account.token.as_str(), "tok");
        assert!(!account.member);
        let twilly = &account.servers[0];
        assert_eq!((twilly.host.as_str(), twilly.port), ("10.0.0.1", 5588));
        assert_eq!(account.servers[1].port, 5589);
    }

    #[test]
    fn rejected_login_reports_the_servers_reason() {
        let err = parse_login(
            &test_device(),
            "player1",
            r#"{"login":{"bSuccess":0,"sMsg":"Invalid password"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::Rejected(m) if m == "Invalid password"));
        let err = parse_login(
            &test_device(),
            "player1",
            r#"{"login":{"bSuccess":1,"sToken":""}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::Rejected(_)));
    }

    #[test]
    fn pick_server_refuses_full_and_member_servers() {
        let account = parse_login(&test_device(), "player1", REPLY).unwrap();
        assert_eq!(account.pick_server("twilly").unwrap().name, "Twilly");
        let refusal = |name| account.pick_server(name).unwrap_err().to_string();
        assert_eq!(refusal("Gravelyn"), "Gravelyn is full");
        assert_eq!(refusal("sir ver"), "Sir Ver is for members only");
        assert_eq!(
            refusal("Yokai"),
            "no server named Yokai; the servers are Twilly, Gravelyn, Sir Ver"
        );
    }

    #[test]
    fn requests_send_the_ruffle_player_headers() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut lines = Vec::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                lines.push(line.trim().to_ascii_lowercase());
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
            lines
        });
        let device = DeviceId("0123456789abcdef0123456789abcdef".into());
        request(ureq::get(format!("http://{addr}/")), &device)
            .call()
            .unwrap();
        let mut headers = peer.join().unwrap();
        headers.remove(0);
        headers.sort();
        assert_eq!(
            headers,
            [
                "accept: */*".to_owned(),
                format!("host: {addr}"),
                "user-agent: ruffle/0.7.1 (https://ruffle.rs)".to_owned(),
                "x-device-id: 0123456789abcdef0123456789abcdef".to_owned(),
            ]
        );
    }

    #[test]
    fn device_id_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("mira-device-id-{}", process::id()));
        let path = dir.join("nested").join("device-id");
        let first = DeviceId::load_or_create(&path).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with("# "));
        assert!(saved.contains(&first.0));
        assert_eq!(DeviceId::load_or_create(&path).unwrap(), first);
        assert_ne!(DeviceId::random().unwrap(), first);
        fs::write(&path, "0123456789abcdef0123456789abcdef\n").unwrap();
        assert_eq!(
            DeviceId::load_or_create(&path).unwrap().0,
            "0123456789abcdef0123456789abcdef"
        );
        fs::write(&path, "not an id").unwrap();
        let err = DeviceId::load_or_create(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn token_debug_hides_the_value() {
        assert!(!format!("{:?}", Token("secret".into())).contains("secret"));
    }
}
