use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mira_client::DEFAULT_RECONNECT_ATTEMPTS;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "mira.toml";
const SERVER: &str = "Server to join, as mira servers lists it. Required here or on each account.";
const CLIENT_VERSION: &str =
    "Log in with exactly this client version instead of the server's minimum.";
const AUTOLOGIN: &str =
    "Log the account in when a session starts without naming accounts. Defaults to true.";
const RECONNECT: &str = "After a lost connection or a kick, log back in after this many seconds, doubling the wait up to 5 minutes. Off unless set.";
const RECONNECT_ATTEMPTS: &str =
    "How many times in a row to try reconnecting before the account goes offline. Defaults to 5.";
const RESTART_ON_ERROR: &str = "Run the script again when it throws, after 10 seconds, doubling up to 5 minutes. Defaults to false.";
const ROOM: &str = "Room number to join; scripts read it as settings.room.";
const API: &str = "Host that takes the login. Defaults to https://game.aq.com.";
const STALL_AFTER: &str = "How long a running script can go without progress before the account counts as stalled, like \"90s\", \"5m\" or \"1h\". Defaults to 5m.";
const ON_STALL: &str = "What happens when the account stalls. Defaults to warn.";
const STALL_AFTER_DEFAULT: Duration = Duration::from_secs(300);

#[derive(Deserialize, JsonSchema, Debug, Default)]
#[serde(deny_unknown_fields)]
#[schemars(title = "mira.toml")]
pub struct Config {
    #[serde(default)]
    #[schemars(description = "Settings for every account. An [[account]] can override each one.")]
    pub defaults: Settings,
    #[serde(default, rename = "account")]
    #[schemars(description = "One account to log in.")]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Deserialize, JsonSchema, Debug, Default, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[schemars(description = SERVER)]
    pub server: Option<String>,
    #[schemars(description = CLIENT_VERSION)]
    pub client_version: Option<String>,
    #[schemars(description = AUTOLOGIN)]
    pub autologin: Option<bool>,
    #[schemars(description = RECONNECT)]
    pub reconnect: Option<u64>,
    #[schemars(description = RECONNECT_ATTEMPTS)]
    pub reconnect_attempts: Option<u32>,
    #[schemars(description = RESTART_ON_ERROR)]
    pub restart_on_error: Option<bool>,
    #[schemars(description = ROOM)]
    pub room: Option<u32>,
    #[schemars(description = API)]
    pub api: Option<String>,
    #[serde(default, deserialize_with = "duration")]
    #[schemars(description = STALL_AFTER, schema_with = "duration_schema")]
    pub stall_after: Option<Duration>,
    #[schemars(description = ON_STALL)]
    pub on_stall: Option<OnStall>,
}

#[derive(Deserialize, Serialize, JsonSchema, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OnStall {
    #[default]
    #[schemars(description = "Log a warning and keep going.")]
    Warn,
    #[schemars(description = "Restart the script.")]
    Restart,
    #[schemars(description = "Log out, log back in and start the script again.")]
    Relogin,
}

impl Settings {
    fn or(self, defaults: &Settings) -> Settings {
        Settings {
            server: self.server.or_else(|| defaults.server.clone()),
            client_version: self
                .client_version
                .or_else(|| defaults.client_version.clone()),
            autologin: self.autologin.or(defaults.autologin),
            reconnect: self.reconnect.or(defaults.reconnect),
            reconnect_attempts: self.reconnect_attempts.or(defaults.reconnect_attempts),
            restart_on_error: self.restart_on_error.or(defaults.restart_on_error),
            room: self.room.or(defaults.room),
            api: self.api.or_else(|| defaults.api.clone()),
            stall_after: self.stall_after.or(defaults.stall_after),
            on_stall: self.on_stall.or(defaults.on_stall),
        }
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    #[schemars(description = "What mira calls the account. Defaults to user.")]
    name: Option<String>,
    #[schemars(description = "Account username.")]
    pub user: String,
    #[schemars(description = "Account password.")]
    #[serde(deserialize_with = "password")]
    pub password: String,
    #[schemars(
        description = "Script to run each time the account logs in, relative to this file."
    )]
    script: Option<PathBuf>,
    #[serde(default)]
    #[schemars(
        description = "Arguments for the script. Values are strings, numbers or booleans.",
        schema_with = "args_schema"
    )]
    args: toml::Table,
    #[schemars(description = SERVER)]
    server: Option<String>,
    #[schemars(description = CLIENT_VERSION)]
    client_version: Option<String>,
    #[schemars(description = AUTOLOGIN)]
    autologin: Option<bool>,
    #[schemars(description = RECONNECT)]
    reconnect: Option<u64>,
    #[schemars(description = RECONNECT_ATTEMPTS)]
    reconnect_attempts: Option<u32>,
    #[schemars(description = RESTART_ON_ERROR)]
    restart_on_error: Option<bool>,
    #[schemars(description = ROOM)]
    room: Option<u32>,
    #[schemars(description = API)]
    api: Option<String>,
    #[serde(default, deserialize_with = "duration")]
    #[schemars(description = STALL_AFTER, schema_with = "duration_schema")]
    stall_after: Option<Duration>,
    #[schemars(description = ON_STALL)]
    on_stall: Option<OnStall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconnect {
    pub delay: Duration,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub user: String,
    pub password: String,
    pub server: String,
    pub client_version: Option<String>,
    pub autologin: bool,
    pub room: Option<u32>,
    pub api: Option<String>,
    pub stall_after: Duration,
    pub on_stall: OnStall,
    pub reconnect: Option<Reconnect>,
    pub restart_on_error: bool,
    pub script: Option<Script>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub path: PathBuf,
    pub args: Vec<(String, String)>,
}

pub fn find(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let candidates = [
        Some(PathBuf::from(FILE_NAME)),
        default_home().map(|h| h.join(FILE_NAME)),
    ];
    candidates
        .iter()
        .flatten()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            let looked: Vec<String> = candidates
                .iter()
                .flatten()
                .map(|p| p.display().to_string())
                .collect();
            format!("no config at {}", looked.join(" or "))
        })
}

pub fn default_home() -> Option<PathBuf> {
    home(std::env::var_os("MIRA_HOME"), std::env::var_os("HOME"))
}

fn home(mira_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    match mira_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => home.map(|home| PathBuf::from(home).join("mira")),
    }
}

pub fn load(path: &Path) -> Result<Vec<Account>, String> {
    let accounts = load_quietly(path)?;
    warn_if_others_can_read(path);
    Ok(accounts)
}

pub fn load_quietly(path: &Path) -> Result<Vec<Account>, String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let path = std::path::absolute(path).map_err(|e| at(&e))?;
    let text = std::fs::read_to_string(&path).map_err(|e| at(&e))?;
    let config = parse(&text).map_err(|e| at(&e))?;
    config
        .accounts(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| at(&e))
}

fn password<'de, D: serde::Deserializer<'de>>(given: D) -> Result<String, D::Error> {
    String::deserialize(given)
        .map_err(|_| serde::de::Error::custom("password must be a quoted string"))
}

fn parse(text: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|e: toml::de::Error| {
        let Some(span) = e.span() else {
            return e.message().to_owned();
        };
        let before = &text[..span.start];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("line {line}, column {column}: {}", e.message())
    })
}

impl Config {
    pub fn accounts(&self, dir: &Path) -> Result<Vec<Account>, String> {
        if self.accounts.is_empty() {
            return Err("there is no [[account]]".into());
        }
        let accounts: Vec<Account> = self
            .accounts
            .iter()
            .map(|a| a.resolve(&self.defaults, dir))
            .collect::<Result<_, _>>()?;
        let mut names: Vec<String> = accounts.iter().map(|a| a.name.to_lowercase()).collect();
        names.sort_unstable();
        if let Some(twice) = names.windows(2).find(|w| w[0] == w[1]) {
            return Err(format!(
                "two accounts are named {}; give one a `name`",
                twice[0]
            ));
        }
        Ok(accounts)
    }
}

impl AccountConfig {
    fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.user)
    }

    fn resolve(&self, defaults: &Settings, dir: &Path) -> Result<Account, String> {
        let name = self.name().to_owned();
        let settings = Settings {
            server: self.server.clone(),
            client_version: self.client_version.clone(),
            autologin: self.autologin,
            reconnect: self.reconnect,
            reconnect_attempts: self.reconnect_attempts,
            restart_on_error: self.restart_on_error,
            room: self.room,
            api: self.api.clone(),
            stall_after: self.stall_after,
            on_stall: self.on_stall,
        }
        .or(defaults);
        let server = settings.server.ok_or_else(|| {
            format!("account {name} has no server; set `server` on it or in [defaults]")
        })?;
        let reconnect = match (settings.reconnect, settings.reconnect_attempts) {
            (Some(secs), attempts) => Some(Reconnect {
                delay: Duration::from_secs(secs),
                attempts: attempts.unwrap_or(DEFAULT_RECONNECT_ATTEMPTS),
            }),
            (None, None) => None,
            (None, Some(_)) => {
                return Err(format!(
                    "account {name} sets reconnect_attempts but not reconnect"
                ));
            }
        };
        let script = match &self.script {
            Some(path) => Some(Script {
                path: dir.join(path),
                args: self.script_args()?,
            }),
            None if self.args.is_empty() => None,
            None => return Err(format!("account {name} has args but no script")),
        };
        Ok(Account {
            user: self.user.clone(),
            password: self.password.clone(),
            server,
            client_version: settings.client_version,
            autologin: settings.autologin.unwrap_or(true),
            room: settings.room,
            api: settings.api,
            stall_after: settings.stall_after.unwrap_or(STALL_AFTER_DEFAULT),
            on_stall: settings.on_stall.unwrap_or_default(),
            reconnect,
            restart_on_error: settings.restart_on_error.unwrap_or(false),
            script,
            name,
        })
    }

    fn script_args(&self) -> Result<Vec<(String, String)>, String> {
        self.args
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Integer(n) => n.to_string(),
                    toml::Value::Float(f) => f.to_string(),
                    toml::Value::Boolean(b) => b.to_string(),
                    other => {
                        return Err(format!(
                            "account {}: args.{key} is a {}; use a string, number or boolean",
                            self.name(),
                            other.type_str()
                        ));
                    }
                };
                Ok((key.clone(), text))
            })
            .collect()
    }
}

pub fn schema() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(Config)).expect("a schema serializes")
}

fn duration_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "pattern": r"^\s*\d+\s*(ms|s|m|h)?\s*$" })
}

fn args_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "object", "additionalProperties": { "type": ["string", "number", "boolean"] } })
}

fn duration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    let text = String::deserialize(d)?;
    parse_duration(&text).map(Some).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "{text:?} isn't a duration like \"90s\", \"5m\" or \"1h\""
        ))
    })
}

fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let n: u32 = text[..split].parse().ok()?;
    let unit = match text[split..].trim() {
        "ms" => Duration::from_millis(1),
        "s" | "" => Duration::from_secs(1),
        "m" => Duration::from_secs(60),
        "h" => Duration::from_secs(3600),
        _ => return None,
    };
    unit.checked_mul(n)
}

fn warn_if_others_can_read(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        eprintln!(
            "warning: {} holds passwords and other users can read it; run chmod 600 on it",
            path.display()
        );
    }
}

#[cfg(test)]
impl Account {
    pub fn for_tests(name: &str) -> Account {
        Account {
            name: name.into(),
            user: name.into(),
            password: "x".into(),
            server: "Twilly".into(),
            client_version: None,
            autologin: false,
            room: None,
            api: None,
            stall_after: STALL_AFTER_DEFAULT,
            on_stall: OnStall::Warn,
            reconnect: None,
            restart_on_error: false,
            script: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"
        [defaults]
        server = "Twilly"
        client_version = "5.32"
        reconnect = 30
        restart_on_error = true
        room = 1234
        stall_after = "90s"
        on_stall = "relogin"

        [[account]]
        user = "player1"
        password = "x"
        script = "scripts/farm.ts"
        args = { target = 7500, fast = true, label = "a=b" }
        autologin = false
        api = "http://127.0.0.1:8787"

        [[account]]
        name = "healer"
        user = "player2"
        password = "y"
        server = "Yorumi"
        reconnect_attempts = 2
        restart_on_error = false
    "#;

    fn accounts(text: &str) -> Result<Vec<Account>, String> {
        parse(text)?.accounts(Path::new("/srv/mira"))
    }

    #[test]
    fn an_account_takes_the_defaults_it_does_not_override() {
        let [first, healer] = accounts(TWO).unwrap().try_into().unwrap();
        assert_eq!(first.name, "player1", "an account is named after its user");
        assert_eq!(first.server, "Twilly");
        assert_eq!(first.client_version.as_deref(), Some("5.32"));
        assert!(!first.autologin);
        assert_eq!(first.api.as_deref(), Some("http://127.0.0.1:8787"));
        assert_eq!(
            (first.room, first.stall_after, first.on_stall),
            (Some(1234), Duration::from_secs(90), OnStall::Relogin)
        );
        assert_eq!(
            first.reconnect,
            Some(Reconnect {
                delay: Duration::from_secs(30),
                attempts: DEFAULT_RECONNECT_ATTEMPTS
            })
        );
        assert!(first.restart_on_error);
        let script = first.script.unwrap();
        assert_eq!(script.path, PathBuf::from("/srv/mira/scripts/farm.ts"));
        let mut args = script.args;
        args.sort();
        let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            args,
            [("fast", "true"), ("label", "a=b"), ("target", "7500")]
        );

        assert_eq!(
            (healer.name.as_str(), healer.user.as_str()),
            ("healer", "player2")
        );
        assert_eq!(healer.server, "Yorumi");
        assert!(
            healer.autologin,
            "accounts log in when a session starts unless autologin is off"
        );
        assert_eq!(healer.api, None);
        assert_eq!(healer.script, None);
        assert_eq!(
            healer.reconnect,
            Some(Reconnect {
                delay: Duration::from_secs(30),
                attempts: 2
            })
        );
        assert!(!healer.restart_on_error, "an account overrides a default");
    }

    #[test]
    fn rejects_typos_missing_servers_stray_args_and_twins() {
        let one =
            |extra: &str| format!("[[account]]\nuser = \"player1\"\npassword = \"x\"\n{extra}");
        assert!(accounts(&one("pasword = \"x\"\nserver = \"Twilly\"")).is_err());
        assert!(accounts(&one("")).unwrap_err().contains("has no server"));
        let broken =
            accounts("[[account]]\nuser = \"player1\"\npassword = \"hunter2\n").unwrap_err();
        assert!(
            broken.starts_with("line 3, column ") && !broken.contains("hunter2"),
            "a parse error names the place, not the text: {broken}"
        );
        let unquoted =
            accounts("[[account]]\nuser = \"player1\"\npassword = 123456\n").unwrap_err();
        assert!(
            unquoted.contains("password") && !unquoted.contains("123456"),
            "a mistyped password stays out of the error: {unquoted}"
        );
        assert!(
            accounts(&one("server = \"Twilly\"\nstall_after = \"soon\""))
                .unwrap_err()
                .contains("isn't a duration")
        );
        assert!(accounts(&one("server = \"Twilly\"\non_stall = \"panic\"")).is_err());
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("45"), Some(Duration::from_secs(45)));
        assert!(
            accounts(&one("server = \"Twilly\"\nreconnect_attempts = 3"))
                .unwrap_err()
                .contains("not reconnect")
        );
        assert!(
            accounts(&one("server = \"Twilly\"\nargs = { n = 1 }"))
                .unwrap_err()
                .contains("args but no script")
        );
        assert!(
            accounts(&one(
                "server = \"Twilly\"\nscript = \"a.ts\"\nargs = { n = [1] }"
            ))
            .unwrap_err()
            .contains("args.n is a array")
        );
        assert!(
            accounts("[defaults]\nserver = \"Twilly\"")
                .unwrap_err()
                .contains("no [[account]]")
        );
        let twins = format!(
            "[defaults]\nserver = \"Twilly\"\n{}\n[[account]]\nname = \"Player1\"\nuser = \"player2\"\npassword = \"y\"",
            one("")
        );
        assert!(
            accounts(&twins)
                .unwrap_err()
                .contains("two accounts are named player1")
        );
    }

    #[test]
    fn the_fallback_home_is_mira_home_or_a_mira_folder_in_the_home_directory() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            home(None, os("/Users/player1")),
            Some(PathBuf::from("/Users/player1/mira"))
        );
        assert_eq!(
            home(os("/srv/bots"), os("/Users/player1")),
            Some(PathBuf::from("/srv/bots"))
        );
        assert_eq!(
            home(os(""), os("/Users/player1")),
            Some(PathBuf::from("/Users/player1/mira")),
            "an empty MIRA_HOME is unset"
        );
        assert_eq!(home(None, None), None);
    }

    #[test]
    fn the_tui_guide_example_loads() {
        let page = include_str!("../../../docs/tui.md");
        let example = page
            .split_once("```toml\n")
            .and_then(|(_, rest)| rest.split_once("```"))
            .expect("a toml example")
            .0;
        let names: Vec<String> = accounts(example)
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["player1", "healer"]);
        let schema: serde_json::Value = serde_json::from_str(&schema()).unwrap();
        let defs = schema["$defs"].as_object().unwrap().values();
        let keys = defs.flat_map(|def| {
            let fields = def["properties"]
                .as_object()
                .into_iter()
                .flat_map(|p| p.keys().cloned());
            let values = def["oneOf"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["const"].as_str().map(String::from));
            fields.chain(values).collect::<Vec<_>>()
        });
        let missing: Vec<String> = keys
            .filter(|key| !page.contains(&format!("`{key}`")))
            .collect();
        assert!(
            missing.is_empty(),
            "docs/tui.md doesn't mention {missing:?}"
        );
    }

    #[test]
    fn the_schema_describes_every_key() {
        let schema: serde_json::Value = serde_json::from_str(&schema()).unwrap();
        let settings = schema["$defs"]["Settings"]["properties"]
            .as_object()
            .unwrap();
        let account = schema["$defs"]["AccountConfig"]["properties"]
            .as_object()
            .unwrap();
        for (key, field) in settings.iter().chain(account.iter()) {
            assert!(
                field.get("description").is_some(),
                "{key} has no description"
            );
        }
        assert!(account.contains_key("password") && settings.contains_key("on_stall"));
    }
}
