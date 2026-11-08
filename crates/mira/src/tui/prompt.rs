use std::path::{Path, PathBuf};

use tui_input::Input as TextInput;

use crate::config::Script;

const SCRIPT_EXTENSIONS: [&str; 4] = [".ts", ".js", ".mjs", ".cs"];

pub struct Prompt {
    pub input: TextInput,
    draft: TextInput,
    pick: Option<usize>,
    pub matches: Vec<String>,
}

impl Prompt {
    pub fn new(home: &Path) -> Self {
        let mut prompt = Prompt {
            input: TextInput::default(),
            draft: TextInput::default(),
            pick: None,
            matches: Vec::new(),
        };
        prompt.find_matches(home);
        prompt
    }

    pub fn find_matches(&mut self, home: &Path) {
        self.matches = scripts_matching(self.input.value(), home);
    }

    pub fn edited(&mut self) {
        self.pick = None;
    }

    pub fn complete(&mut self) {
        self.pick = None;
        match self.matches.as_slice() {
            [] => {}
            [only] if !only.ends_with('/') => self.input = TextInput::new(format!("{only} ")),
            many => self.input = TextInput::new(shared_prefix(many).to_owned()),
        }
    }

    pub fn recall(&mut self, recent: &[String], older: bool) {
        if recent.is_empty() {
            return;
        }
        let pick = match (self.pick, older) {
            (None, false) => return,
            (None, true) => {
                self.draft = std::mem::take(&mut self.input);
                0
            }
            (Some(i), true) => (i + 1).min(recent.len() - 1),
            (Some(0), false) => {
                self.pick = None;
                self.input = std::mem::take(&mut self.draft);
                return;
            }
            (Some(i), false) => i - 1,
        };
        self.pick = Some(pick);
        self.input = TextInput::new(recent[recent.len() - 1 - pick].clone());
    }
}

const RECENT_KEPT: usize = 100;

#[derive(Default)]
pub struct Recent {
    runs: Vec<Script>,
    offered: Vec<Script>,
}

impl Recent {
    pub fn add(&mut self, script: Script) {
        self.runs.retain(|kept| kept.path != script.path);
        self.runs.push(script);
        let excess = self.runs.len().saturating_sub(RECENT_KEPT);
        self.runs.drain(..excess);
    }

    pub fn offer(&mut self, script: Script) {
        let known = self.offered.iter().any(|kept| kept.path == script.path);
        if self.offered.len() < RECENT_KEPT && !known {
            self.offered.push(script);
        }
    }

    #[cfg(test)]
    pub fn scripts(&self) -> &[Script] {
        &self.runs
    }

    pub fn typed(&self, home: &Path) -> Vec<String> {
        let ran = |offered: &&Script| self.runs.iter().any(|run| run.path == offered.path);
        let offered = self.offered.iter().filter(|offered| !ran(offered));
        offered
            .chain(&self.runs)
            .map(|script| typed(home, script))
            .collect()
    }
}

#[derive(Default)]
pub struct History(Option<PathBuf>);

impl History {
    pub fn in_data_dir() -> History {
        History(dirs::data_dir().map(|dir| dir.join("mira").join("history")))
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

fn scripts_matching(input: &str, home: &Path) -> Vec<String> {
    let input = input.trim_start();
    if input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (dir, prefix) = input.split_at(input.rfind('/').map_or(0, |i| i + 1));
    let Ok(entries) = std::fs::read_dir(home.join(expand_home(Path::new(dir)))) else {
        return Vec::new();
    };
    let mut matches: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let hidden = name.starts_with('.') && !prefix.starts_with('.');
            if !name.starts_with(prefix) || hidden || name.contains(char::is_whitespace) {
                return None;
            }
            let kind = entry.file_type().ok()?;
            if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                Some(format!("{dir}{name}/"))
            } else if script_file(&name) {
                Some(format!("{dir}{name}"))
            } else {
                None
            }
        })
        .collect();
    if prefix == "." || prefix == ".." {
        matches.push(format!("{dir}../"));
    }
    matches.sort();
    matches
}

pub fn script_file(name: &str) -> bool {
    SCRIPT_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) && !name.ends_with(".d.ts")
}

pub fn shared_prefix(sorted: &[String]) -> &str {
    let (Some(first), Some(last)) = (sorted.first(), sorted.last()) else {
        return "";
    };
    let end = first
        .char_indices()
        .zip(last.chars())
        .find(|((_, a), b)| a != b)
        .map_or(first.len(), |((i, _), _)| i);
    &first[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_run_names_a_script_and_its_args() {
        let home = std::env::temp_dir().join(format!("mira-prompt-tests-{}", std::process::id()));
        std::fs::create_dir_all(home.join("farm")).unwrap();
        for file in [
            "farm/void.ts",
            "farm/vault.cs",
            "farm/types.d.ts",
            "farm/notes.txt",
        ] {
            std::fs::write(home.join(file), "").unwrap();
        }
        assert_eq!(scripts_matching("fa", &home), ["farm/"]);
        assert_eq!(
            scripts_matching("farm/", &home),
            ["farm/vault.cs", "farm/void.ts"]
        );
        assert_eq!(scripts_matching("farm/vo", &home), ["farm/void.ts"]);
        assert!(scripts_matching("farm/void.ts n=1", &home).is_empty());

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
        let dir = std::env::temp_dir().join(format!("mira-history-tests-{}", std::process::id()));
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
        recent.offer(Script {
            path: dir.join("farm/config.ts"),
            args: vec![("token".into(), "x".into())],
        });
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
            [
                "farm/config.ts token=x",
                "farm/vault.ts",
                "farm/void.ts target=100"
            ]
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
    }

    #[test]
    fn up_walks_back_through_recent_runs_and_down_returns_to_the_draft() {
        let recent = vec!["old.ts".to_owned(), "new.ts".to_owned()];
        let mut prompt = Prompt::new(Path::new("/nonexistent"));
        prompt.input = TextInput::new("dra".into());
        prompt.recall(&recent, true);
        assert_eq!(prompt.input.value(), "new.ts");
        prompt.recall(&recent, true);
        prompt.recall(&recent, true);
        assert_eq!(prompt.input.value(), "old.ts");
        prompt.recall(&recent, false);
        prompt.recall(&recent, false);
        assert_eq!(prompt.input.value(), "dra");
    }
}
