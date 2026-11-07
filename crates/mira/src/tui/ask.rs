use std::collections::HashMap;
use std::path::PathBuf;

use mira_script::{Arg, ArgKind, Scalar};
use tui_input::Input as TextInput;

use crate::config::Script;

pub type Remembered = HashMap<(PathBuf, String, Option<String>), String>;

pub struct Ask {
    pub script: Script,
    asked: Vec<Arg>,
    accounts: Vec<(usize, String)>,
    pub shared: bool,
    answers: Vec<String>,
    pub input: TextInput,
    pub recalled: bool,
    pub error: Option<String>,
}

pub fn missing(script: &Script) -> Vec<Arg> {
    let skua = script
        .path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cs"));
    if skua {
        return Vec::new();
    }
    let declared = mira_script::script_args(&script.path).unwrap_or_default();
    declared
        .into_iter()
        .filter(|arg| !script.args.iter().any(|(key, _)| *key == arg.name))
        .collect()
}

impl Ask {
    pub fn new(
        script: Script,
        asked: Vec<Arg>,
        accounts: Vec<(usize, String)>,
        shared: bool,
        remembered: &Remembered,
    ) -> Ask {
        let mut ask = Ask {
            script,
            asked,
            accounts,
            shared,
            answers: Vec::new(),
            input: TextInput::default(),
            recalled: false,
            error: None,
        };
        ask.recall(remembered);
        ask
    }

    pub fn can_share(&self) -> bool {
        self.accounts.len() > 1 && self.answers.is_empty()
    }

    fn per_account(&self) -> bool {
        self.accounts.len() > 1 && !self.shared
    }

    pub fn asking(&self) -> (&Arg, Option<&str>) {
        let n = self.answers.len();
        let account = self
            .per_account()
            .then(|| self.accounts[n / self.asked.len()].1.as_str());
        (&self.asked[n % self.asked.len()], account)
    }

    pub fn left(&self) -> usize {
        let rounds = if self.per_account() {
            self.accounts.len()
        } else {
            1
        };
        rounds * self.asked.len() - self.answers.len()
    }

    fn slot(&self) -> (PathBuf, String, Option<String>) {
        let (arg, account) = self.asking();
        (
            self.script.path.clone(),
            arg.name.clone(),
            account.map(str::to_owned),
        )
    }

    fn recall(&mut self, remembered: &Remembered) {
        let (path, name, account) = self.slot();
        let value = account
            .and_then(|a| remembered.get(&(path.clone(), name.clone(), Some(a))))
            .or_else(|| remembered.get(&(path, name, None)));
        self.input = value.cloned().map(TextInput::new).unwrap_or_default();
        self.recalled = !self.input.value().is_empty();
    }

    pub fn edit(&mut self) {
        self.error = None;
        if std::mem::take(&mut self.recalled) {
            self.input.reset();
        }
    }

    pub fn toggle_share(&mut self, remembered: &Remembered) {
        if !self.can_share() {
            return;
        }
        self.shared = !self.shared;
        if self.recalled || self.input.value().is_empty() {
            self.recall(remembered);
        }
    }

    pub fn answer(&mut self, remembered: &mut Remembered) -> Option<Vec<(usize, Script)>> {
        let value = self.input.value().trim().to_owned();
        let (arg, account) = self.asking();
        let skipped = value.is_empty() && arg.optional;
        if let Err(e) = arg.parse(&value)
            && !skipped
        {
            self.error = Some(e);
            return None;
        }
        let (path, name, _) = self.slot();
        remembered.insert((path, name, account.map(str::to_owned)), value.clone());
        self.answers.push(value);
        if self.left() > 0 {
            self.recall(remembered);
            return None;
        }
        Some(self.runs())
    }

    fn runs(&self) -> Vec<(usize, Script)> {
        let per_account = self.per_account();
        self.accounts
            .iter()
            .enumerate()
            .map(|(round, &(i, _))| {
                let from = if per_account {
                    round * self.asked.len()
                } else {
                    0
                };
                let answered = self
                    .asked
                    .iter()
                    .zip(&self.answers[from..])
                    .filter(|(arg, value)| !(arg.optional && value.is_empty()))
                    .map(|(arg, value)| (arg.name.clone(), value.clone()));
                let mut script = self.script.clone();
                script.args.extend(answered);
                (i, script)
            })
            .collect()
    }
}

pub fn kind(arg: &Arg) -> &'static str {
    match arg.kind {
        ArgKind::Scalar(Scalar::String) => "text",
        ArgKind::Scalar(Scalar::Number) => "a number",
        ArgKind::Scalar(Scalar::Boolean) => "true or false",
        ArgKind::List(_) => "a JSON list",
        ArgKind::Map(_) => "a JSON object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(name: &str, kind: ArgKind) -> Arg {
        Arg {
            name: name.into(),
            kind,
            optional: false,
            default: None,
        }
    }

    fn script() -> Script {
        Script {
            path: PathBuf::from("/srv/farm.ts"),
            args: vec![("fast".into(), "true".into())],
        }
    }

    fn type_in(ask: &mut Ask, text: &str) {
        ask.edit();
        ask.input = TextInput::new(text.into());
    }

    #[test]
    fn one_answer_covers_every_account_unless_tab_asks_each() {
        let asked = vec![
            arg("monster", ArgKind::Scalar(Scalar::String)),
            arg("count", ArgKind::Scalar(Scalar::Number)),
        ];
        let accounts = vec![(0, "player1".to_owned()), (2, "player3".to_owned())];
        let mut remembered = Remembered::new();

        let mut ask = Ask::new(script(), asked.clone(), accounts.clone(), true, &remembered);
        assert_eq!(
            (ask.asking().0.name.as_str(), ask.asking().1, ask.left()),
            ("monster", None, 2)
        );
        type_in(&mut ask, "Frogzard");
        assert!(ask.answer(&mut remembered).is_none());
        type_in(&mut ask, "lots");
        assert!(ask.answer(&mut remembered).is_none());
        assert_eq!(
            ask.error.as_deref(),
            Some("count must be number, not lots"),
            "a bad answer stays asked"
        );
        type_in(&mut ask, "5");
        let runs = ask.answer(&mut remembered).unwrap();
        let args = |s: &Script| {
            s.args
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(runs.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 2]);
        assert!(
            runs.iter()
                .all(|(_, s)| args(s) == ["fast=true", "monster=Frogzard", "count=5"])
        );

        let mut each = Ask::new(script(), asked, accounts, true, &remembered);
        assert!(
            each.recalled && each.input.value() == "Frogzard",
            "the last answer comes back"
        );
        each.toggle_share(&remembered);
        assert_eq!((each.asking().1, each.left()), (Some("player1"), 4));
        each.answer(&mut remembered);
        each.answer(&mut remembered);
        assert!(
            !each.can_share(),
            "the choice is fixed once something is answered"
        );
        assert_eq!(
            each.asking(),
            (
                &arg("monster", ArgKind::Scalar(Scalar::String)),
                Some("player3")
            )
        );
        type_in(&mut each, "Slime");
        each.answer(&mut remembered);
        let runs = each.answer(&mut remembered).unwrap();
        assert_eq!(
            args(&runs[0].1),
            ["fast=true", "monster=Frogzard", "count=5"]
        );
        assert_eq!(args(&runs[1].1), ["fast=true", "monster=Slime", "count=5"]);
    }

    #[test]
    fn enter_skips_an_optional_arg_and_leaves_it_out() {
        let mut class = arg("class", ArgKind::Scalar(Scalar::String));
        class.optional = true;
        let mut count = arg("count", ArgKind::Scalar(Scalar::Number));
        count.optional = true;
        let accounts = vec![(0, "player1".to_owned()), (1, "player2".to_owned())];
        let mut remembered = Remembered::new();
        let mut ask = Ask::new(script(), vec![class, count], accounts, false, &remembered);
        type_in(&mut ask, "Necromancer");
        ask.answer(&mut remembered);
        type_in(&mut ask, "");
        ask.answer(&mut remembered);
        assert_eq!(
            ask.error, None,
            "an empty optional number is skipped, not a bad number"
        );
        type_in(&mut ask, "");
        ask.answer(&mut remembered);
        type_in(&mut ask, "3");
        let runs = ask.answer(&mut remembered).unwrap();
        let args = |s: &Script| {
            s.args
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(args(&runs[0].1), ["fast=true", "class=Necromancer"]);
        assert_eq!(args(&runs[1].1), ["fast=true", "count=3"]);
    }

    #[test]
    fn every_declared_arg_not_typed_is_asked() {
        let dir = std::env::temp_dir().join(format!("mira-ask-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("follow.ts");
        std::fs::write(
            &path,
            "import { script } from \"mira\";\nexport default function* () { script.args<{ leader: string; class?: string }>({ class: \"\" }); }\n",
        )
        .unwrap();
        let typed = Script {
            path: path.clone(),
            args: vec![("leader".into(), "player2".into())],
        };
        let names: Vec<String> = missing(&typed).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["class"]);
        let bare = Script {
            path,
            args: Vec::new(),
        };
        let names: Vec<String> = missing(&bare).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["leader", "class"]);
    }
}
