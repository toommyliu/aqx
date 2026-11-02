use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, rename = "bot")]
    pub bots: Vec<BotConfig>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub server: Option<String>,
    pub api: Option<String>,
    pub room: Option<u32>,
    #[serde(default, deserialize_with = "duration")]
    pub stall_after: Option<Duration>,
    #[serde(default)]
    pub on_stall: OnStall,
    #[serde(default)]
    pub restart: Restart,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OnStall {
    #[default]
    Warn,
    Restart,
    Relogin,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    #[default]
    Never,
    OnError,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    pub name: String,
    pub user: String,
    pub password: String,
    pub server: Option<String>,
    pub script: Option<PathBuf>,
    #[serde(default)]
    pub args: toml::Table,
}

pub fn load(explicit: Option<&Path>) -> Result<(Config, PathBuf), String> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => [PathBuf::from("aqx.toml"), user_config()]
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| format!("no config found at ./aqx.toml or {}", user_config().display()))?,
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: Config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut names: Vec<&str> = config.bots.iter().map(|b| b.name.as_str()).collect();
    names.sort_unstable();
    if let Some(twice) = names.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("{}: two bots are named {}", path.display(), twice[0]));
    }
    warn_if_others_can_read(&path);
    Ok((config, path))
}

impl Config {
    pub fn bot(&self, name: Option<&str>) -> Result<&BotConfig, String> {
        match (name, &self.bots[..]) {
            (Some(name), bots) => bots.iter().find(|b| b.name == name).ok_or_else(|| format!("no [[bot]] named {name}")),
            (None, [only]) => Ok(only),
            (None, []) => Err("the config has no [[bot]]".into()),
            (None, _) => Err("the config has several bots; pick one with -a NAME".into()),
        }
    }
}

impl BotConfig {
    pub fn script_path(&self, config: &Path) -> Option<PathBuf> {
        let script = self.script.as_ref()?;
        Some(if script.is_absolute() { script.clone() } else { config.parent().unwrap_or(Path::new(".")).join(script) })
    }

    pub fn script_args(&self) -> Result<Vec<(String, String)>, String> {
        self.args
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Integer(n) => n.to_string(),
                    toml::Value::Float(f) => f.to_string(),
                    toml::Value::Boolean(b) => b.to_string(),
                    other => return Err(format!("bot {}: args.{key} is a {}; use a string, number or boolean", self.name, other.type_str())),
                };
                Ok((key.clone(), text))
            })
            .collect()
    }
}

fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    let text = String::deserialize(d)?;
    parse_duration(&text).map(Some).ok_or_else(|| serde::de::Error::custom(format!("{text:?} isn't a duration like \"90s\", \"5m\" or \"1h\"")))
}

pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let n: u64 = text[..split].parse().ok()?;
    let unit = match text[split..].trim() {
        "ms" => Duration::from_millis(1),
        "s" | "" => Duration::from_secs(1),
        "m" => Duration::from_secs(60),
        "h" => Duration::from_secs(3600),
        _ => return None,
    };
    unit.checked_mul(u32::try_from(n).ok()?)
}

fn user_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default()
        .join("aqx/aqx.toml")
}

fn warn_if_others_can_read(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        eprintln!("warning: {} holds passwords and other users can read it; run chmod 600 on it", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_BOTS: &str = r#"
        [defaults]
        server = "Twilly"
        api = "http://127.0.0.1:8787"
        room = 12345
        stall_after = "90s"
        on_stall = "relogin"
        restart = "on-error"

        [[bot]]
        name = "main"
        user = "hero"
        password = "x"
        script = "scripts/void-aura.ts"
        args = { target = 7500, batch = 5, fast = true, label = "a=b" }

        [[bot]]
        name = "healer"
        user = "medic"
        password = "y"
        server = "Yorumi"
    "#;

    #[test]
    fn picks_a_bot_by_name_or_the_only_one() {
        let config: Config = toml::from_str(TWO_BOTS).unwrap();
        assert_eq!(config.bot(Some("healer")).unwrap().user, "medic");
        assert!(config.bot(None).is_err());
        assert!(config.bot(Some("nobody")).is_err());
        let one: Config = toml::from_str("[[bot]]\nname = \"main\"\nuser = \"hero\"\npassword = \"x\"").unwrap();
        assert_eq!(one.bot(None).unwrap().user, "hero");
        assert_eq!(config.defaults.api.as_deref(), Some("http://127.0.0.1:8787"));
    }

    #[test]
    fn reads_the_defaults_and_each_bots_script_and_args() {
        let config: Config = toml::from_str(TWO_BOTS).unwrap();
        let d = &config.defaults;
        assert_eq!((d.room, d.stall_after, d.on_stall, d.restart), (Some(12345), Some(Duration::from_secs(90)), OnStall::Relogin, Restart::OnError));
        let main = config.bot(Some("main")).unwrap();
        assert_eq!(main.script_path(Path::new("/srv/aqx/aqx.toml")), Some(PathBuf::from("/srv/aqx/scripts/void-aura.ts")));
        let mut args = main.script_args().unwrap();
        args.sort();
        let pairs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(pairs, [("batch", "5"), ("fast", "true"), ("label", "a=b"), ("target", "7500")]);
        assert_eq!(config.bot(Some("healer")).unwrap().server.as_deref(), Some("Yorumi"));
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        assert!(toml::from_str::<Config>("[[bot]]\nname = \"main\"\nuser = \"hero\"\npasword = \"x\"").is_err());
        assert!(toml::from_str::<Config>("[defaults]\nstall_after = \"soon\"").is_err());
        assert!(toml::from_str::<Config>("[defaults]\non_stall = \"panic\"").is_err());
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    }
}
