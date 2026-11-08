use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use mira_script::skua::library::{self, Entry, Library, Updated};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use tui_input::Input as TextInput;
use tui_input::backend::crossterm::EventHandler;

const PAGE: usize = 10;

#[derive(Debug)]
pub enum Catalog {
    Missing,
    Failed(String),
    Ready(Library),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Run(PathBuf),
    Update,
    Nothing,
}

pub struct Picker {
    pub scripts: PathBuf,
    pub catalog: Catalog,
    pub query: TextInput,
    pub searching: bool,
    pub cursor: usize,
    pub failed: Option<String>,
}

impl Picker {
    pub fn open() -> Picker {
        let (scripts, found) = match crate::config::skua_home() {
            Ok(home) => (library::scripts(&home), library::open(&home)),
            Err(why) => (PathBuf::new(), Err(why)),
        };
        Picker::new(scripts, found)
    }

    pub fn new(scripts: PathBuf, found: Result<Option<Library>, String>) -> Picker {
        let mut picker = Picker {
            scripts,
            catalog: Catalog::Missing,
            query: TextInput::default(),
            searching: false,
            cursor: 0,
            failed: None,
        };
        picker.load(found);
        picker
    }

    pub fn reload(&mut self) {
        let found = crate::config::skua_home().and_then(|home| library::open(&home));
        self.load(found);
    }

    fn load(&mut self, found: Result<Option<Library>, String>) {
        self.catalog = match found {
            Ok(Some(library)) => Catalog::Ready(library),
            Ok(None) => Catalog::Missing,
            Err(why) => Catalog::Failed(why),
        };
        self.cursor = self.cursor.min(self.shown().len().saturating_sub(1));
    }

    pub fn shown(&self) -> Vec<&Entry> {
        match &self.catalog {
            Catalog::Ready(library) => library.search(self.query.value()),
            _ => Vec::new(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.shown().len().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down => self.cursor = (self.cursor + 1).min(last),
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(PAGE),
            KeyCode::PageDown => self.cursor = (self.cursor + PAGE).min(last),
            KeyCode::Enter => {
                return match self.shown().get(self.cursor) {
                    Some(entry) => Outcome::Run(self.scripts.join(&entry.path)),
                    None => {
                        self.searching = false;
                        Outcome::Nothing
                    }
                };
            }
            KeyCode::Esc if self.searching || !self.query.value().is_empty() => {
                self.query.reset();
                self.searching = false;
                self.cursor = 0;
            }
            KeyCode::Esc => return Outcome::Close,
            _ if self.searching => {
                if self
                    .query
                    .handle_event(&Event::Key(key))
                    .is_some_and(|changed| changed.value)
                {
                    self.cursor = 0;
                }
            }
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('u') => return Outcome::Update,
            _ => {}
        }
        Outcome::Nothing
    }
}

enum Progress {
    Stage(&'static str),
    Done(Result<String, String>),
}

pub struct Update {
    pub stage: &'static str,
    results: mpsc::Receiver<Progress>,
}

impl Update {
    pub fn start() -> Update {
        Update::spawn(|stage| {
            let home = crate::config::skua_home()?;
            stage("downloading the Skua scripts");
            let updated = library::update(&home)?;
            mira_script::skua::worker(&mut || stage("building the Skua worker"))
                .map_err(|e| e.to_string())?;
            Ok(match updated {
                Updated::Current => "the Skua scripts are up to date".to_owned(),
                Updated::To(commit) => {
                    format!("updated the Skua scripts to {}", library::short(&commit))
                }
            })
        })
    }

    pub fn spawn(
        job: impl FnOnce(&dyn Fn(&'static str)) -> Result<String, String> + Send + 'static,
    ) -> Update {
        let (sender, results) = mpsc::channel();
        thread::spawn(move || {
            let stage = |stage| {
                let _ = sender.send(Progress::Stage(stage));
            };
            let done = job(&stage);
            let _ = sender.send(Progress::Done(done));
        });
        Update {
            stage: "updating the Skua scripts",
            results,
        }
    }

    pub fn poll(&mut self) -> Option<Result<String, String>> {
        loop {
            match self.results.try_recv() {
                Ok(Progress::Stage(stage)) => self.stage = stage,
                Ok(Progress::Done(done)) => return Some(done),
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Some(Err("the Skua update stopped unexpectedly".into()));
                }
            }
        }
    }
}
