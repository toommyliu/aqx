use std::path::{Path, PathBuf};

use serde::Deserialize;

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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    pub name: String,
    pub user: String,
    pub password: String,
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
    let config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
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

        [[bot]]
        name = "main"
        user = "hero"
        password = "x"

        [[bot]]
        name = "healer"
        user = "medic"
        password = "y"
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
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[[bot]]\nname = \"main\"\nuser = \"hero\"\npasword = \"x\"").is_err());
    }
}
