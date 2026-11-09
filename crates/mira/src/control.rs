use std::sync::mpsc::Receiver;
use std::time::Instant;

use mira_client::ItemId;
use serde::{Deserialize, Serialize};

use crate::config::Script;
use crate::fleet::{Fleet, Line};
use crate::view::Shown;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Want {
    pub selected: usize,
    pub since: u64,
    pub full: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub accounts: Vec<Shown>,
    pub log: Vec<Line>,
    #[serde(default)]
    pub ending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Login,
    Logout,
    Stop,
    Restart,
    Pause,
    Resume,
    Relogin,
    Switch(String),
    Run(Script),
    PickUp(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerChoice {
    pub name: String,
    pub players: u32,
    pub capacity: u32,
    pub members: bool,
    pub refusal: Option<String>,
}

pub trait Source {
    fn frame(&self, want: &Want) -> Result<Frame, String>;
    fn act(&self, account: usize, action: Action) -> Result<(), String>;
    fn servers(&self, accounts: &[usize]) -> Receiver<Result<Vec<ServerChoice>, String>>;
    fn configured(&self) -> Vec<Script>;
    fn end(&self) -> Result<(), String>;
}

impl Source for Fleet {
    fn frame(&self, want: &Want) -> Result<Frame, String> {
        let accounts = self.accounts();
        let now = Instant::now();
        let shown = accounts
            .iter()
            .enumerate()
            .map(|(i, account)| Shown::of(account, want.full && i == want.selected, now))
            .collect();
        let log = accounts
            .get(want.selected)
            .map(|a| a.board().log.since(want.since).cloned().collect())
            .unwrap_or_default();
        Ok(Frame {
            accounts: shown,
            log,
            ending: false,
        })
    }

    fn act(&self, account: usize, action: Action) -> Result<(), String> {
        let accounts = self.accounts();
        let account = accounts.get(account).ok_or("no such account")?;
        match action {
            Action::Login => account.login(),
            Action::Logout => account.logout(),
            Action::Stop => account.stop(),
            Action::Restart => account.restart(),
            Action::Pause => account.pause(),
            Action::Resume => account.resume(),
            Action::Relogin => {
                account.relogin();
                Ok(())
            }
            Action::Switch(server) => account.switch(server),
            Action::Run(script) => {
                account.run(script);
                Ok(())
            }
            Action::PickUp(ids) => account.pick_up(ids.into_iter().map(ItemId).collect()),
        }
        .map_err(str::to_owned)
    }

    fn servers(&self, accounts: &[usize]) -> Receiver<Result<Vec<ServerChoice>, String>> {
        self.server_choices(accounts)
    }

    fn end(&self) -> Result<(), String> {
        self.logout_all();
        Ok(())
    }

    fn configured(&self) -> Vec<Script> {
        self.accounts()
            .iter()
            .filter_map(|a| a.script().cloned())
            .collect()
    }
}
