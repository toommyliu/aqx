use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::config::FILE_NAME;

const TYPES: &str = include_str!("../../../api/mira.d.ts");
const TSCONFIG: &str = include_str!("../../../examples/tsconfig.json");
const SAMPLE: &str = include_str!("../../../examples/basic.ts");
const SAMPLE_NAME: &str = "farm.ts";
const SCHEMA_NAME: &str = "mira.schema.json";

pub struct Login {
    pub user: String,
    pub password: String,
    pub server: String,
}

pub fn init(
    dir: &Path,
    login: impl FnOnce() -> Result<Option<Login>, String>,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write(dir, "mira.d.ts", TYPES, true)?;
    write(dir, "tsconfig.json", &tsconfig(), false)?;
    write(dir, ".gitignore", &format!("{FILE_NAME}\n"), false)?;
    write(dir, SAMPLE_NAME, SAMPLE, false)?;
    write(dir, SCHEMA_NAME, &crate::config::schema(), true)?;
    write(
        dir,
        crate::tui::KEYS_SCHEMA_NAME,
        &crate::tui::keys_schema(),
        true,
    )?;
    let config = dir.join(FILE_NAME);
    if config.exists() {
        println!("kept {}", config.display());
    } else if let Some(login) = login()? {
        create_config(&config, &login)?;
        println!("wrote {}", config.display());
    }
    Ok(())
}

fn tsconfig() -> String {
    let pointed = TSCONFIG.replace("\"../api/mira.d.ts\"", "\"./mira.d.ts\"");
    assert_ne!(
        pointed, TSCONFIG,
        "the example tsconfig points at api/mira.d.ts"
    );
    pointed
}

pub fn ask() -> Result<Option<Login>, String> {
    if !std::io::stdin().is_terminal() {
        println!("skipped {FILE_NAME}; run mira init in a terminal to add an account");
        return Ok(None);
    }
    let yes = read(&format!("Add an account to a new {FILE_NAME}? [y/N] "))?;
    if !yes.trim().eq_ignore_ascii_case("y") {
        return Ok(None);
    }
    let user = read("Username: ")?.trim().to_owned();
    let password = read_hidden("Password: ")?;
    let server = read("Server, as mira servers lists it: ")?
        .trim()
        .to_owned();
    if user.is_empty() || password.is_empty() || server.is_empty() {
        return Err("the username, password and server can't be empty".into());
    }
    Ok(Some(Login {
        user,
        password,
        server,
    }))
}

fn create_config(path: &Path, login: &Login) -> Result<(), String> {
    let quoted = |text: &str| toml::Value::from(text).to_string();
    let text = format!(
        "#:schema ./{SCHEMA_NAME}\n\n[[account]]\nuser = {}\npassword = {}\nserver = {}\nscript = {}\n",
        quoted(&login.user),
        quoted(&login.password),
        quoted(&login.server),
        quoted(SAMPLE_NAME),
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
    println!(
        "{} {}",
        if existed { "updated" } else { "wrote" },
        path.display()
    );
    Ok(())
}

fn read(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn read_hidden(prompt: &str) -> Result<String, String> {
    let fd = libc::STDIN_FILENO;
    let mut shown = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut shown) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    ctrlc::set_handler(move || {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &shown) };
        eprintln!();
        std::process::exit(130);
    })
    .map_err(|e| e.to_string())?;
    let mut hidden = shown;
    hidden.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) };
    let line = read(prompt);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &shown) };
    eprintln!();
    line
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    fn fresh(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mira-init-tests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn not_asked() -> Result<Option<Login>, String> {
        panic!("asked for an account")
    }

    #[test]
    fn a_new_folder_gets_a_config_that_runs_the_sample() {
        let dir = fresh("new");
        let login = || {
            Ok(Some(Login {
                user: "player1".into(),
                password: r#"p"a\ss"#.into(),
                server: "Twilly".into(),
            }))
        };
        init(&dir, login).unwrap();
        let path = dir.join(FILE_NAME);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let [account] = crate::config::load(&path).unwrap().try_into().unwrap();
        assert_eq!(
            (account.user.as_str(), account.password.as_str()),
            ("player1", r#"p"a\ss"#)
        );
        assert_eq!(account.server, "Twilly");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#:schema ./mira.schema.json\n"), "{text}");
        assert_eq!(
            std::fs::read_to_string(dir.join(SCHEMA_NAME)).unwrap(),
            crate::config::schema()
        );
        assert_eq!(account.script.unwrap().path, dir.join(SAMPLE_NAME));
        assert_eq!(
            std::fs::read_to_string(dir.join("mira.d.ts")).unwrap(),
            TYPES
        );
        let tsconfig = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
        assert!(
            tsconfig.contains(r#""paths": { "mira": ["./mira.d.ts"] }"#),
            "{tsconfig}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "mira.toml\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(SAMPLE_NAME)).unwrap(),
            SAMPLE
        );
    }

    #[test]
    fn rerunning_updates_the_types_and_keeps_the_users_files() {
        let dir = fresh("rerun");
        init(&dir, || Ok(None)).unwrap();
        std::fs::write(dir.join("mira.d.ts"), "old").unwrap();
        for name in [SAMPLE_NAME, "tsconfig.json", ".gitignore", FILE_NAME] {
            std::fs::write(dir.join(name), "mine").unwrap();
        }
        init(&dir, not_asked).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("mira.d.ts")).unwrap(),
            TYPES
        );
        for name in [SAMPLE_NAME, "tsconfig.json", ".gitignore", FILE_NAME] {
            assert_eq!(
                std::fs::read_to_string(dir.join(name)).unwrap(),
                "mine",
                "{name}"
            );
        }
    }

    #[test]
    fn declining_writes_no_config() {
        let dir = fresh("declined");
        init(&dir, || Ok(None)).unwrap();
        assert!(dir.join(SAMPLE_NAME).is_file());
        assert!(!dir.join(FILE_NAME).exists());
        assert!(dir.join(SCHEMA_NAME).is_file());
    }
}
