use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aqx_protocol::de::flag;
use serde::Deserialize;

const MAP_FILE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct Account {
    pub token: String,
    pub member: bool,
    pub servers: Vec<Server>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Server {
    #[serde(rename = "sName")]
    pub name: String,
    #[serde(rename = "sIP")]
    pub host: String,
    #[serde(rename = "iPort")]
    pub port: u16,
    #[serde(rename = "bOnline", deserialize_with = "flag", default)]
    pub online: bool,
    #[serde(rename = "bUpg", deserialize_with = "flag", default)]
    pub members_only: bool,
    #[serde(rename = "iCount", default)]
    pub players: u32,
    #[serde(rename = "iMax", default)]
    pub capacity: u32,
}

impl Server {
    pub fn full(&self) -> bool {
        self.capacity > 0 && self.players >= self.capacity
    }
}

#[derive(Debug)]
pub enum ApiError {
    Http(String),
    Rejected(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Http(e) | ApiError::Rejected(e) => f.write_str(e),
        }
    }
}

pub fn login(api: &str, user: &str, pass: &str) -> Result<Account, ApiError> {
    let url = format!("{}/game/api/login/now?ran={}", api.trim_end_matches('/'), cache_buster());
    let body = ureq::post(&url)
        .header("x-requested-with", "ShockwaveFlash/32.0.0.371")
        .header("artixmode", "launcher")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .send_form([("user", user), ("pass", pass), ("option", "1")])
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| ApiError::Http(format!("login request failed: {e}")))?;
    parse_login(&body)
}

fn cache_buster() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    format!("0.{nanos:09}")
}

pub fn map_file(api: &str, file: &str) -> Result<Vec<u8>, ApiError> {
    let url = format!("{}/game/gamefiles/maps/{}", api.trim_end_matches('/'), file.trim_start_matches('/'));
    ureq::get(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .call()
        .and_then(|mut r| r.body_mut().with_config().limit(MAP_FILE_LIMIT).read_to_vec())
        .map_err(|e| ApiError::Http(format!("map file request failed: {e}")))
}

pub fn servers(api: &str) -> Result<Vec<Server>, ApiError> {
    let url = format!("{}/game/api/data/servers", api.trim_end_matches('/'));
    let body = ureq::get(&url)
        .header("artixmode", "launcher")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| ApiError::Http(format!("server list request failed: {e}")))?;
    serde_json::from_str(&body).map_err(|e| ApiError::Http(format!("unreadable server list: {e}")))
}

#[derive(Debug, PartialEq, Eq)]
pub struct ServerError {
    pub message: String,
    pub lasting: bool,
}

pub fn pick_server<'a>(servers: &'a [Server], name: Option<&str>, member: bool) -> Result<&'a Server, ServerError> {
    let fail = |message: String, lasting| Err(ServerError { message, lasting });
    let Some(name) = name else {
        return match servers.iter().find(|s| s.online && !s.full() && (member || !s.members_only)) {
            Some(server) => Ok(server),
            None => fail("no server is open to this account".into(), false),
        };
    };
    let Some(server) = servers.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else {
        let names: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
        return fail(format!("no server named {name}; the servers are {}", names.join(", ")), true);
    };
    if server.members_only && !member {
        fail(format!("{} is for members only", server.name), true)
    } else if !server.online {
        fail(format!("{} is offline", server.name), false)
    } else if server.full() {
        fail(format!("{} is full", server.name), false)
    } else {
        Ok(server)
    }
}

pub fn load_bank(api: &str, char_id: u32, token: &str) -> Result<String, ApiError> {
    let url = format!("{}/game/api/char/bank?v={}", api.trim_end_matches('/'), cache_buster());
    ureq::post(&url)
        .header("ccid", char_id.to_string())
        .header("token", token)
        .header("artixmode", "launcher")
        .header("x-requested-with", "ShockwaveFlash/32.0.0.371")
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .send_form([("layout", r#"{"cat":"all"}"#)])
        .and_then(|mut r| r.body_mut().with_config().limit(64 << 20).read_to_string())
        .map_err(|e| ApiError::Http(format!("bank request failed: {e}")))
}

fn parse_login(body: &str) -> Result<Account, ApiError> {
    #[derive(Deserialize)]
    struct Reply {
        login: Option<Login>,
        #[serde(default)]
        servers: Vec<Server>,
        #[serde(rename = "sMsg")]
        msg: Option<String>,
    }
    #[derive(Deserialize)]
    struct Login {
        #[serde(rename = "bSuccess", deserialize_with = "flag", default)]
        ok: bool,
        #[serde(rename = "sToken")]
        token: Option<String>,
        #[serde(rename = "iUpg", deserialize_with = "flag", default)]
        member: bool,
        #[serde(rename = "sMsg")]
        msg: Option<String>,
    }
    let reply: Reply = serde_json::from_str(body).map_err(|e| ApiError::Http(format!("unreadable login reply: {e}")))?;
    match reply.login {
        Some(Login { ok: true, token: Some(token), member, .. }) => Ok(Account { token, member, servers: reply.servers }),
        login => Err(ApiError::Rejected(format!(
            "login rejected: {}",
            login.and_then(|l| l.msg).or(reply.msg).unwrap_or_else(|| "no token in the reply".into())
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_token_membership_and_servers() {
        let body = r#"{"login":{"bSuccess":1,"sToken":"tok","iUpg":0},
            "servers":[{"sName":"Twilly","sIP":"10.0.0.1","iPort":5588,"iCount":900,"iMax":1000,"bOnline":1,"bUpg":0},
                       {"sName":"Gravelyn","sIP":"10.0.0.2","iPort":5588,"iCount":1000,"iMax":1000,"bOnline":1,"bUpg":1}]}"#;
        let account = parse_login(body).unwrap();
        assert_eq!((account.token.as_str(), account.member), ("tok", false));
        let [twilly, gravelyn] = &account.servers[..] else { panic!("two servers") };
        assert_eq!((twilly.name.as_str(), twilly.host.as_str(), twilly.port), ("Twilly", "10.0.0.1", 5588));
        assert!(twilly.online && !twilly.members_only && !twilly.full());
        assert!(gravelyn.members_only && gravelyn.full());
        assert_eq!(pick_server(&account.servers, None, true).unwrap().name, "Twilly");
        assert_eq!(pick_server(&account.servers, Some("gravelyn"), true).unwrap_err(), ServerError { message: "Gravelyn is full".into(), lasting: false });
        let unknown = pick_server(&account.servers, Some("Yokai"), false).unwrap_err();
        assert_eq!((unknown.message.as_str(), unknown.lasting), ("no server named Yokai; the servers are Twilly, Gravelyn", true));
    }

    #[test]
    fn reports_the_servers_message_on_failure() {
        let err = parse_login(r#"{"login":{"bSuccess":0,"sMsg":"Invalid password"}}"#).unwrap_err();
        assert!(matches!(err, ApiError::Rejected(m) if m == "login rejected: Invalid password"));
    }
}
