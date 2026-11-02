use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::bot::Script;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub defaults: Settings,
    #[serde(default, rename = "bot")]
    pub bots: Vec<BotConfig>,
}

#[derive(Deserialize, Clone, Default, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub server: Option<String>,
    pub api: Option<String>,
    pub room: Option<u32>,
    #[serde(default, deserialize_with = "duration")]
    pub stall_after: Option<Duration>,
    pub on_stall: Option<OnStall>,
    pub restart: Option<Restart>,
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
    name: Option<String>,
    pub user: String,
    pub password: String,
    script: Option<PathBuf>,
    #[serde(default)]
    args: toml::Table,
    server: Option<String>,
    api: Option<String>,
    room: Option<u32>,
    #[serde(default, deserialize_with = "duration")]
    stall_after: Option<Duration>,
    on_stall: Option<OnStall>,
    restart: Option<Restart>,
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
    let mut names: Vec<&str> = config.bots.iter().map(BotConfig::name).collect();
    names.sort_unstable();
    if let Some(twice) = names.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("{}: two bots are named {}; give one a `name`", path.display(), twice[0]));
    }
    warn_if_others_can_read(&path);
    Ok((config, path))
}

impl Config {
    pub fn bot(&self, name: Option<&str>) -> Result<&BotConfig, String> {
        match (name, &self.bots[..]) {
            (Some(name), bots) => bots
                .iter()
                .find(|b| b.name() == name)
                .or_else(|| bots.iter().find(|b| b.user.eq_ignore_ascii_case(name)))
                .ok_or_else(|| format!("no [[bot]] named {name}")),
            (None, [only]) => Ok(only),
            (None, []) => Err("the config has no [[bot]]".into()),
            (None, _) => Err("the config has several bots; pick one with -a NAME".into()),
        }
    }
}

impl BotConfig {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.user)
    }

    pub fn settings(&self, defaults: &Settings) -> Settings {
        Settings {
            server: self.server.clone().or_else(|| defaults.server.clone()),
            api: self.api.clone().or_else(|| defaults.api.clone()),
            room: self.room.or(defaults.room),
            stall_after: self.stall_after.or(defaults.stall_after),
            on_stall: self.on_stall.or(defaults.on_stall),
            restart: self.restart.or(defaults.restart),
        }
    }

    pub fn script(&self, config: &Path) -> Result<Option<Script>, String> {
        let Some(script) = &self.script else {
            return if self.args.is_empty() { Ok(None) } else { Err(format!("bot {} has args but no script", self.name())) };
        };
        let path = if script.is_absolute() { script.clone() } else { config.parent().unwrap_or(Path::new(".")).join(script) };
        Ok(Some(Script { path: std::path::absolute(&path).unwrap_or(path), args: self.script_args()? }))
    }

    pub fn run_script(&self, config: &Path, given: Option<PathBuf>, args: Vec<(String, String)>, idle: bool) -> Result<Option<Script>, String> {
        if let Some(path) = given {
            return Ok(Some(Script { path: std::path::absolute(&path).unwrap_or(path), args }));
        }
        if idle {
            return Ok(None);
        }
        let Some(mut script) = self.script(config)? else {
            return if args.is_empty() { Ok(None) } else { Err(format!("--arg needs a script, and bot {} has none", self.name())) };
        };
        for (key, value) in args {
            match script.args.iter_mut().find(|(k, _)| *k == key) {
                Some(kept) => kept.1 = value,
                None => script.args.push((key, value)),
            }
        }
        Ok(Some(script))
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
                    other => return Err(format!("bot {}: args.{key} is a {}; use a string, number or boolean", self.name(), other.type_str())),
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
        user = "Hero"
        password = "x"
        script = "scripts/void-aura.ts"
        args = { target = 7500, batch = 5, fast = true, label = "a=b" }

        [[bot]]
        name = "healer"
        user = "medic"
        password = "y"
        server = "Yorumi"
        stall_after = "20m"
        on_stall = "warn"
    "#;

    fn pairs(script: &Script) -> Vec<(&str, &str)> {
        let mut pairs: Vec<(&str, &str)> = script.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        pairs.sort();
        pairs
    }

    #[test]
    fn a_bot_is_named_after_its_user_unless_it_has_a_name() {
        let config: Config = toml::from_str(TWO_BOTS).unwrap();
        assert_eq!(config.bot(Some("Hero")).unwrap().name(), "Hero");
        assert_eq!(config.bot(Some("hero")).unwrap().name(), "Hero", "users match case-insensitively");
        assert_eq!(config.bot(Some("healer")).unwrap().user, "medic");
        assert_eq!(config.bot(Some("medic")).unwrap().name(), "healer", "-a also takes the user");
        assert!(config.bot(None).is_err());
        assert!(config.bot(Some("nobody")).is_err());
        let one: Config = toml::from_str("[[bot]]\nuser = \"hero\"\npassword = \"x\"").unwrap();
        assert_eq!(one.bot(None).unwrap().name(), "hero");
    }

    #[test]
    fn a_bot_overrides_any_default() {
        let config: Config = toml::from_str(TWO_BOTS).unwrap();
        let d = &config.defaults;
        let hero = config.bot(Some("Hero")).unwrap().settings(d);
        assert_eq!(hero, *d);
        let healer = config.bot(Some("healer")).unwrap().settings(d);
        assert_eq!(healer.server.as_deref(), Some("Yorumi"));
        assert_eq!((healer.stall_after, healer.on_stall), (Some(Duration::from_secs(1200)), Some(OnStall::Warn)));
        assert_eq!((healer.room, healer.restart, healer.api.as_deref()), (Some(12345), Some(Restart::OnError), Some("http://127.0.0.1:8787")));
    }

    #[test]
    fn aqx_run_takes_the_bots_script_unless_given_another() {
        let config: Config = toml::from_str(TWO_BOTS).unwrap();
        let (hero, healer) = (config.bot(Some("Hero")).unwrap(), config.bot(Some("healer")).unwrap());
        let file = Path::new("/srv/aqx/aqx.toml");
        let cli = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();

        let own = hero.run_script(file, None, cli(&[("target", "100")]), false).unwrap().unwrap();
        assert_eq!(own.path, PathBuf::from("/srv/aqx/scripts/void-aura.ts"));
        assert_eq!(pairs(&own), [("batch", "5"), ("fast", "true"), ("label", "a=b"), ("target", "100")]);

        let other = hero.run_script(file, Some(PathBuf::from("/tmp/other.ts")), cli(&[("n", "1")]), false).unwrap().unwrap();
        assert_eq!((other.path.as_path(), pairs(&other)), (Path::new("/tmp/other.ts"), vec![("n", "1")]));

        assert!(hero.run_script(file, None, Vec::new(), true).unwrap().is_none(), "--idle skips the script");
        assert!(healer.run_script(file, None, Vec::new(), false).unwrap().is_none());
        assert!(healer.run_script(file, None, cli(&[("n", "1")]), false).unwrap_err().contains("bot healer has none"));
    }

    #[test]
    fn rejects_unknown_keys_bad_values_and_twins() {
        assert!(toml::from_str::<Config>("[[bot]]\nuser = \"hero\"\npasword = \"x\"").is_err());
        assert!(toml::from_str::<Config>("[defaults]\nstall_after = \"soon\"").is_err());
        assert!(toml::from_str::<Config>("[[bot]]\nuser = \"hero\"\npassword = \"x\"\non_stall = \"panic\"").is_err());
        assert!(toml::from_str::<Config>("[defaults]\nscript = \"a.ts\"").is_err(), "a script belongs to a bot");
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        let dir = std::env::temp_dir().join(format!("aqx-config-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let twins = dir.join("twins.toml");
        std::fs::write(&twins, "[[bot]]\nuser = \"hero\"\npassword = \"x\"\n\n[[bot]]\nname = \"hero\"\nuser = \"other\"\npassword = \"y\"\n").unwrap();
        std::fs::set_permissions(&twins, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        assert!(load(Some(&twins)).err().unwrap().contains("two bots are named hero"));
    }
}
