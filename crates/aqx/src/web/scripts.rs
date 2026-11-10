use std::path::{Path, PathBuf};
use std::{fs, io};

use aqx_script::skua::library::Entry;

use crate::config::Script;

const SCRIPT_EXTENSIONS: [&str; 4] = [".ts", ".js", ".mjs", ".cs"];

const RECENT_KEPT: usize = 100;

#[derive(Default)]
pub struct Recent {
    runs: Vec<Script>,
}

impl Recent {
    pub fn add(&mut self, script: Script) {
        self.runs.retain(|kept| kept.path != script.path);
        self.runs.push(script);
        let excess = self.runs.len().saturating_sub(RECENT_KEPT);
        self.runs.drain(..excess);
    }

    pub fn forget(&mut self, forgotten: impl Fn(&Path) -> bool) {
        self.runs.retain(|run| !forgotten(&run.path));
    }

    #[cfg(test)]
    pub fn scripts(&self) -> &[Script] {
        &self.runs
    }

    pub fn typed(&self, home: &Path) -> Vec<String> {
        self.runs.iter().map(|script| typed(home, script)).collect()
    }
}

#[derive(Default)]
pub struct History(Option<PathBuf>);

impl History {
    pub fn in_data_dir() -> History {
        History(dirs::data_dir().map(|dir| dir.join("aqx").join("history")))
    }

    pub fn load(&self, recent: &mut Recent) {
        let Some(path) = &self.0 else {
            return;
        };
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for script in text
            .lines()
            .filter_map(|line| parse(line, Path::new("/")).ok())
        {
            recent.add(script);
        }
    }

    pub fn save(&self, recent: &Recent) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(path) = &self.0 else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let fresh = path.with_extension("new");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&fresh)?;
        for script in &recent.runs {
            writeln!(file, "{}", typed(Path::new(""), script))?;
        }
        file.sync_all()?;
        std::fs::rename(&fresh, path)
    }
}

pub fn parse(typed: &str, home: &Path) -> Result<Script, String> {
    let typed = typed.trim();
    let (word, rest) = typed.split_at(typed.find(char::is_whitespace).unwrap_or(typed.len()));
    let path = home.join(expand_home(Path::new(word)));
    if !path.is_file() {
        return Err(format!("{} isn't a file", path.display()));
    }
    Ok(Script {
        path,
        args: typed_args(rest.split_whitespace())?,
    })
}

pub fn shown_path(home: &Path, script: &Path) -> String {
    script
        .strip_prefix(home)
        .unwrap_or(script)
        .display()
        .to_string()
}

pub fn typed(home: &Path, script: &Script) -> String {
    let mut typed = shown_path(home, &script.path);
    for (key, value) in &script.args {
        typed.push_str(&format!(" {key}={value}"));
    }
    typed
}

pub fn expand_home(typed: &Path) -> PathBuf {
    match (typed.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => typed.to_path_buf(),
    }
}

fn typed_args<'a>(words: impl Iterator<Item = &'a str>) -> Result<Vec<(String, String)>, String> {
    let mut args: Vec<(String, String)> = Vec::new();
    for word in words {
        match (word.split_once('='), args.last_mut()) {
            (Some((key, value)), _) if !key.is_empty() => {
                args.push((key.to_owned(), value.to_owned()));
            }
            (_, Some((_, value))) => {
                value.push(' ');
                value.push_str(word);
            }
            (_, None) => return Err(format!("{word} isn't KEY=VALUE")),
        }
    }
    Ok(args)
}

pub fn script_file(name: &str) -> bool {
    SCRIPT_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) && !name.ends_with(".d.ts")
}

pub fn list(folder: &Path) -> Result<Vec<Entry>, String> {
    let found = match fs::read_dir(folder) {
        Ok(found) => found,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("couldn't list {}: {e}", folder.display())),
    };
    let mut entries: Vec<Entry> = found
        .flatten()
        .filter_map(|found| {
            let name = found.file_name().into_string().ok()?;
            if name.starts_with('.') || name == "node_modules" {
                return None;
            }
            let path = found.path();
            let name = if path.is_dir() {
                format!("{name}/")
            } else if script_file(&name) && path.is_file() {
                name
            } else {
                return None;
            };
            Some(Entry {
                name: name.clone(),
                description: String::new(),
                tags: Vec::new(),
                path: name,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        let file = |e: &Entry| !e.path.ends_with('/');
        (file(a), &a.path).cmp(&(file(b), &b.path))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_run_names_a_script_and_its_args() {
        let home = std::env::temp_dir().join(format!("aqx-prompt-tests-{}", std::process::id()));
        std::fs::create_dir_all(home.join("farm")).unwrap();
        for file in [
            "farm/void.ts",
            "farm/vault.cs",
            "farm/types.d.ts",
            "farm/notes.txt",
        ] {
            std::fs::write(home.join(file), "").unwrap();
        }
        let listed: Vec<String> = list(&home.join("farm"))
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(listed, ["vault.cs", "void.ts"]);

        let script = parse("farm/void.ts target=7500 label=two words", &home).unwrap();
        assert_eq!(script.path, home.join("farm/void.ts"));
        assert_eq!(
            script.args,
            [
                ("target".into(), "7500".into()),
                ("label".into(), "two words".into())
            ]
        );
        assert_eq!(
            typed(&home, &script),
            "farm/void.ts target=7500 label=two words"
        );
        assert!(
            parse("farm/void.ts stray", &home)
                .unwrap_err()
                .contains("isn't KEY=VALUE")
        );
        assert!(
            parse("farm/gone.ts", &home)
                .unwrap_err()
                .contains("isn't a file")
        );
    }

    #[test]
    fn the_history_keeps_one_run_per_script_with_its_latest_args() {
        let dir = std::env::temp_dir().join(format!("aqx-history-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("farm")).unwrap();
        for file in ["farm/void.ts", "farm/vault.ts"] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let history = History(Some(dir.join("state/history")));
        let mut loaded = Recent::default();
        history.load(&mut loaded);
        assert!(
            loaded.scripts().is_empty(),
            "a missing file is an empty history"
        );

        let mut recent = Recent::default();
        for typed in [
            "farm/void.ts target=7500",
            "farm/vault.ts",
            "farm/void.ts target=100",
        ] {
            recent.add(parse(typed, &dir).unwrap());
            history.save(&recent).unwrap();
        }
        assert_eq!(
            recent.typed(&dir),
            ["farm/vault.ts", "farm/void.ts target=100"]
        );
        let text = std::fs::read_to_string(dir.join("state/history")).unwrap();
        let void = dir.join("farm/void.ts");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                dir.join("farm/vault.ts").display().to_string(),
                format!("{} target=100", void.display())
            ],
            "one line per script typed here, newest last"
        );
        let mut loaded = Recent::default();
        history.load(&mut loaded);
        assert_eq!(loaded.scripts(), recent.scripts());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(dir.join("state/history"))
                .unwrap()
                .permissions(),
        );
        assert_eq!(
            mode & 0o777,
            0o600,
            "args can be private, so only the owner reads the history"
        );
    }

    #[test]
    fn recent_runs_stay_unique_and_capped() {
        let mut recent = Recent::default();
        for i in 0..RECENT_KEPT + 5 {
            recent.add(Script {
                path: PathBuf::from(format!("/srv/{i}.ts")),
                args: Vec::new(),
            });
        }
        recent.add(Script {
            path: PathBuf::from("/srv/50.ts"),
            args: vec![("n".into(), "1".into())],
        });
        let paths: Vec<&Path> = recent.scripts().iter().map(|s| s.path.as_path()).collect();
        assert_eq!(paths.len(), RECENT_KEPT);
        assert_eq!(paths[0], Path::new("/srv/5.ts"), "the oldest runs drop off");
        assert_eq!(
            paths
                .iter()
                .filter(|p| **p == Path::new("/srv/50.ts"))
                .count(),
            1
        );
        assert_eq!(
            *paths.last().unwrap(),
            Path::new("/srv/50.ts"),
            "running a script again moves it to the end"
        );
        recent.forget(|path| path == Path::new("/srv/50.ts"));
        assert_eq!(recent.scripts().len(), RECENT_KEPT - 1);
        assert!(
            recent
                .scripts()
                .iter()
                .all(|s| s.path != Path::new("/srv/50.ts")),
            "a forgotten script leaves the history"
        );
    }
}
