use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};

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
pub enum LoginError {
    Http(String),
    Rejected(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Http(e) => write!(f, "login request failed: {e}"),
            LoginError::Rejected(msg) => write!(f, "login rejected: {msg}"),
        }
    }
}

pub fn login(api: &str, user: &str, pass: &str) -> Result<Account, LoginError> {
    let url = format!("{}/game/api/login/now?ran={}", api.trim_end_matches('/'), cache_buster());
    let body = ureq::post(&url)
        .header("x-requested-with", "ShockwaveFlash/32.0.0.371")
        .header("artixmode", "launcher")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .send_form([("user", user), ("pass", pass), ("option", "1")])
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| LoginError::Http(e.to_string()))?;
    parse_login(&body)
}

fn cache_buster() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    format!("0.{nanos:09}")
}

fn parse_login(body: &str) -> Result<Account, LoginError> {
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
    let reply: Reply = serde_json::from_str(body).map_err(|e| LoginError::Http(format!("unreadable reply: {e}")))?;
    match reply.login {
        Some(Login { ok: true, token: Some(token), member, .. }) => Ok(Account { token, member, servers: reply.servers }),
        login => Err(LoginError::Rejected(
            login.and_then(|l| l.msg).or(reply.msg).unwrap_or_else(|| "no token in the reply".into()),
        )),
    }
}

fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Int(i64),
        Text(String),
    }
    Ok(match Flag::deserialize(d)? {
        Flag::Bool(b) => b,
        Flag::Int(n) => n != 0,
        Flag::Text(s) => !matches!(s.as_str(), "" | "0" | "false"),
    })
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
    }

    #[test]
    fn reports_the_servers_message_on_failure() {
        let err = parse_login(r#"{"login":{"bSuccess":0,"sMsg":"Invalid password"}}"#).unwrap_err();
        assert!(matches!(err, LoginError::Rejected(m) if m == "Invalid password"));
    }
}
