use std::io::{IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::prompt;

const TYPES: &str = include_str!("../../../api/aqx.d.ts");
const TSCONFIG: &str = include_str!("../../../api/tsconfig.json");

const SAMPLE: &str = r#"import { combat, map, quests } from "aqx";

map.join("farm", "End", "Right");
quests.accept(11);
combat.farm("Treeant Branch", 1);
quests.complete(11);
"#;

pub struct Account {
    pub user: String,
    pub password: String,
}

pub fn init(dir: &Path, global: &Path, account: impl FnOnce() -> Result<Option<Account>, String>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write(dir, "aqx.d.ts", TYPES, true)?;
    write(dir, "tsconfig.json", TSCONFIG, false)?;
    write(dir, "farm.ts", SAMPLE, false)?;
    let config = dir.join("aqx.toml");
    if config.exists() {
        println!("kept {}", config.display());
    } else if global.exists() {
        println!("skipped aqx.toml; aqx uses {}", global.display());
        return Ok(());
    } else if let Some(account) = account()? {
        create_config(&config, &account)?;
        println!("wrote {}", config.display());
    } else {
        return Ok(());
    }
    write(dir, "aqx.schema.json", &crate::config::schema(), true)
}

pub fn ask() -> Result<Option<Account>, String> {
    if !std::io::stdin().is_terminal() {
        println!("skipped aqx.toml; run aqx init in a terminal to add an account");
        return Ok(None);
    }
    if !prompt::confirm("Add an account to a new aqx.toml?")? {
        return Ok(None);
    }
    let user = prompt::read("Username: ")?.trim().to_string();
    let password = prompt::read_hidden("Password: ")?;
    if user.is_empty() || password.is_empty() {
        return Err("the username and password can't be empty".into());
    }
    Ok(Some(Account { user, password }))
}

fn create_config(path: &Path, account: &Account) -> Result<(), String> {
    let text = format!(
        "#:schema ./aqx.schema.json\n\n[[bot]]\nuser = {}\npassword = {}\nscript = \"farm.ts\"\n",
        toml::Value::from(account.user.as_str()),
        toml::Value::from(account.password.as_str()),
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn write(dir: &Path, name: &str, contents: &str, replace: bool) -> Result<(), String> {
    let path = dir.join(name);
    let existed = path.exists();
    if existed && !replace {
        println!("kept {}", path.display());
        return Ok(());
    }
    std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{} {}", if existed { "updated" } else { "wrote" }, path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::{Account, TYPES, init};

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aqx-init-tests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn not_asked() -> Result<Option<Account>, String> {
        panic!("asked for an account")
    }

    #[test]
    fn a_new_folder_gets_a_config_that_runs_the_sample() {
        let dir = fresh("new");
        let account = || Ok(Some(Account { user: "Hero".into(), password: r#"p"a\ss"#.into() }));
        init(&dir, &dir.join("no-global.toml"), account).unwrap();
        let path = dir.join("aqx.toml");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let (config, path) = crate::config::load(Some(&path)).unwrap();
        let bot = config.bot(None).unwrap();
        assert_eq!((bot.user.as_str(), bot.password.as_str()), ("Hero", r#"p"a\ss"#));
        let script = bot.script(&path).unwrap().unwrap().path;
        assert_eq!(aqx_script::check(&script), Ok(()));
        assert_eq!(std::fs::read_to_string(dir.join("aqx.schema.json")).unwrap(), crate::config::schema());
    }

    #[test]
    fn rerunning_updates_what_aqx_owns_and_keeps_the_users_files() {
        let dir = fresh("rerun");
        let no_global = dir.join("no-global.toml");
        init(&dir, &no_global, || Ok(Some(Account { user: "Hero".into(), password: "x".into() }))).unwrap();
        for name in ["aqx.d.ts", "aqx.schema.json"] {
            std::fs::write(dir.join(name), "old").unwrap();
        }
        for name in ["farm.ts", "aqx.toml", "tsconfig.json"] {
            std::fs::write(dir.join(name), "mine").unwrap();
        }
        init(&dir, &no_global, not_asked).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("aqx.d.ts")).unwrap(), TYPES);
        assert_eq!(std::fs::read_to_string(dir.join("aqx.schema.json")).unwrap(), crate::config::schema());
        for name in ["farm.ts", "aqx.toml", "tsconfig.json"] {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "mine", "{name}");
        }
    }

    #[test]
    fn a_global_config_is_never_overridden() {
        let dir = fresh("global");
        let global = dir.join("global.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&global, "").unwrap();
        init(&dir, &global, not_asked).unwrap();
        assert!(!dir.join("aqx.toml").exists());
        assert!(dir.join("farm.ts").exists());
    }

    #[test]
    fn declining_writes_no_config() {
        let dir = fresh("declined");
        init(&dir, &dir.join("no-global.toml"), || Ok(None)).unwrap();
        assert!(!dir.join("aqx.toml").exists());
        assert!(!dir.join("aqx.schema.json").exists());
    }
}
