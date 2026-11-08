use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use mira_script::skua::{self, Kind, Setting, Settings};

use crate::view::{Shown, State};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use tui_input::Input as TextInput;
use tui_input::backend::crossterm::EventHandler;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    CoreBots,
    Script,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::CoreBots, Tab::Script];

    pub fn label(self) -> &'static str {
        match self {
            Tab::CoreBots => "CoreBots",
            Tab::Script => "Script",
        }
    }

    fn at(self) -> usize {
        self as usize
    }
}

#[derive(Debug)]
pub enum Pane {
    Loading,
    Failed(String),
    Ready(Settings),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Save {
    pub tab: Tab,
    pub changes: Vec<(String, String)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Save(Save),
    Nothing,
}

type Result = (Tab, bool, std::result::Result<Settings, String>);

pub struct Skua {
    pub account: usize,
    pub user: String,
    pub script: PathBuf,
    pub tab: Tab,
    pub panes: [Pane; 2],
    pub cursor: [usize; 2],
    pub editing: Option<TextInput>,
    pub saving: Option<Tab>,
    pub error: Option<String>,
    sender: mpsc::Sender<Result>,
    results: mpsc::Receiver<Result>,
}

impl Skua {
    pub fn new(account: usize, user: String, script: PathBuf) -> Skua {
        let (sender, results) = mpsc::channel();
        Skua {
            account,
            user,
            script,
            tab: Tab::CoreBots,
            panes: [Pane::Loading, Pane::Loading],
            cursor: [0, 0],
            editing: None,
            saving: None,
            error: None,
            sender,
            results,
        }
    }

    pub fn open(account: usize, user: String, script: PathBuf) -> Skua {
        let skua = Skua::new(account, user, script);
        for tab in Tab::ALL {
            skua.spawn(tab, false, Vec::new());
        }
        skua
    }

    pub fn save(&mut self, save: Save) {
        self.saving = Some(save.tab);
        self.spawn(save.tab, true, save.changes);
    }

    fn spawn(&self, tab: Tab, saving: bool, changes: Vec<(String, String)>) {
        let sender = self.sender.clone();
        let user = self.user.clone();
        let script = self.script.clone();
        thread::spawn(move || {
            let result = crate::config::skua_home().and_then(|home| {
                match tab {
                    Tab::CoreBots => skua::core_bots_options(&user, &home, &changes),
                    Tab::Script => skua::script_options(&script, &home, &changes),
                }
                .map_err(|e| e.to_string())
            });
            let _ = sender.send((tab, saving, result));
        });
    }

    pub fn poll(&mut self) {
        while let Ok((tab, saving, result)) = self.results.try_recv() {
            self.loaded(tab, saving, result);
        }
    }

    pub fn loaded(
        &mut self,
        tab: Tab,
        saving: bool,
        result: std::result::Result<Settings, String>,
    ) {
        if saving {
            self.saving = None;
        }
        match result {
            Ok(settings) => {
                let last = settings.options.len().saturating_sub(1);
                self.cursor[tab.at()] = self.cursor[tab.at()].min(last);
                self.panes[tab.at()] = Pane::Ready(settings);
            }
            Err(why) if saving => self.error = Some(why),
            Err(why) => self.panes[tab.at()] = Pane::Failed(why),
        }
    }

    pub fn pane(&self) -> &Pane {
        &self.panes[self.tab.at()]
    }

    pub fn cursor(&self) -> usize {
        self.cursor[self.tab.at()]
    }

    pub fn selected(&self) -> Option<&Setting> {
        match self.pane() {
            Pane::Ready(settings) => settings.options.get(self.cursor()),
            _ => None,
        }
    }

    pub fn key(&mut self, key: KeyEvent, editable: bool) -> Outcome {
        if let Some(input) = &mut self.editing {
            match key.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter if !editable => self.error = Some(BUSY.into()),
                KeyCode::Enter => {
                    let value = input.value().to_owned();
                    self.editing = None;
                    return self.change(value);
                }
                _ => {
                    input.handle_event(&Event::Key(key));
                }
            }
            return Outcome::Nothing;
        }
        let len = match self.pane() {
            Pane::Ready(settings) => settings.options.len(),
            _ => 0,
        };
        let at = self.tab.at();
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Tab | KeyCode::BackTab => {
                self.tab = match self.tab {
                    Tab::CoreBots => Tab::Script,
                    Tab::Script => Tab::CoreBots,
                };
                self.error = None;
            }
            KeyCode::Up => self.cursor[at] = self.cursor[at].saturating_sub(1),
            KeyCode::Down => self.cursor[at] = (self.cursor[at] + 1).min(len.saturating_sub(1)),
            KeyCode::PageUp => self.cursor[at] = self.cursor[at].saturating_sub(PAGE),
            KeyCode::PageDown => {
                self.cursor[at] = (self.cursor[at] + PAGE).min(len.saturating_sub(1));
            }
            KeyCode::Enter
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Backspace
            | KeyCode::Delete => {
                let Some(setting) = self.selected() else {
                    return Outcome::Nothing;
                };
                if !editable {
                    self.error = Some(BUSY.into());
                    return Outcome::Nothing;
                }
                if self.saving.is_some() {
                    return Outcome::Nothing;
                }
                let value = setting.value.clone().unwrap_or_default();
                let next = match (key.code, setting.kind) {
                    (KeyCode::Backspace | KeyCode::Delete, _) if self.tab == Tab::CoreBots => {
                        if setting.value.is_none() {
                            return Outcome::Nothing;
                        }
                        String::new()
                    }
                    (KeyCode::Backspace | KeyCode::Delete, _) => return Outcome::Nothing,
                    (_, Kind::Bool) => if value == "True" { "False" } else { "True" }.to_owned(),
                    (_, Kind::Choice) => {
                        let step = if key.code == KeyCode::Left { -1 } else { 1 };
                        cycle(&setting.choices, &value, step)
                    }
                    (KeyCode::Enter, Kind::Number | Kind::Text) => {
                        self.editing = Some(TextInput::new(value));
                        return Outcome::Nothing;
                    }
                    _ => return Outcome::Nothing,
                };
                return self.change(next);
            }
            _ => {}
        }
        Outcome::Nothing
    }

    fn change(&mut self, value: String) -> Outcome {
        let Some(setting) = self.selected() else {
            return Outcome::Nothing;
        };
        if setting.value.as_deref() == Some(value.as_str()) {
            return Outcome::Nothing;
        }
        let key = match self.tab {
            Tab::Script if setting.group != "Options" => {
                format!("{}:{}", setting.group, setting.name)
            }
            _ => setting.name.clone(),
        };
        self.error = None;
        Outcome::Save(Save {
            tab: self.tab,
            changes: vec![(key, value)],
        })
    }
}

const PAGE: usize = 10;

pub const BUSY: &str = "Options can change while the account is offline or idle.";

pub fn editable(account: &Shown) -> bool {
    matches!(account.state, State::Offline { .. } | State::Online)
}

fn cycle(choices: &[String], current: &str, step: isize) -> String {
    let Some(len) = isize::try_from(choices.len()).ok().filter(|&n| n > 0) else {
        return current.to_owned();
    };
    let next = match choices.iter().position(|c| c == current) {
        Some(at) => (at as isize + step).rem_euclid(len),
        None if step < 0 => len - 1,
        None => 0,
    };
    choices[next as usize].clone()
}
