use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::{fs, io, thread};

use mira_script::skua::library::{self, Entry, Library, Updated};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use tui_input::Input as TextInput;
use tui_input::backend::crossterm::EventHandler;

const PAGE: usize = 10;
const MIRA_EXTENSIONS: [&str; 3] = [".ts", ".js", ".mjs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Mira,
    Skua,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Mira, Tab::Skua];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Mira => "mira",
            Tab::Skua => "Skua",
        }
    }

    fn other(self) -> Tab {
        match self {
            Tab::Mira => Tab::Skua,
            Tab::Skua => Tab::Mira,
        }
    }
}

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
    pub tab: Tab,
    pub mira_scripts: PathBuf,
    pub mira: Result<Vec<Entry>, String>,
    pub scripts: PathBuf,
    pub catalog: Catalog,
    pub query: TextInput,
    pub searching: bool,
    pub cursor: usize,
    pub failed: Option<String>,
}

impl Picker {
    pub fn open(home: &Path, tab: Tab) -> Picker {
        let (scripts, found) = match crate::config::skua_home() {
            Ok(home) => (library::scripts(&home), library::open(&home)),
            Err(why) => (PathBuf::new(), Err(why)),
        };
        Picker::new(tab, scripts, found).with_mira(home.join("scripts"))
    }

    pub fn new(tab: Tab, scripts: PathBuf, found: Result<Option<Library>, String>) -> Picker {
        let mut picker = Picker {
            tab,
            mira_scripts: PathBuf::new(),
            mira: Ok(Vec::new()),
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

    pub fn with_mira(mut self, folder: PathBuf) -> Picker {
        self.mira = mira_scripts(&folder);
        self.mira_scripts = folder;
        self.cursor = self.cursor.min(self.shown().len().saturating_sub(1));
        self
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

    pub fn folder(&self) -> &Path {
        match self.tab {
            Tab::Mira => &self.mira_scripts,
            Tab::Skua => &self.scripts,
        }
    }

    pub fn shown(&self) -> Vec<&Entry> {
        let query = self.query.value();
        match (self.tab, &self.mira, &self.catalog) {
            (Tab::Mira, Ok(entries), _) => library::search(entries, query),
            (Tab::Skua, _, Catalog::Ready(found)) => library::search(&found.entries, query),
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
            KeyCode::Tab | KeyCode::BackTab => {
                self.tab = self.tab.other();
                self.cursor = 0;
            }
            KeyCode::Enter => {
                return match self.shown().get(self.cursor) {
                    Some(entry) => Outcome::Run(self.folder().join(&entry.path)),
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
            KeyCode::Char('u') if self.tab == Tab::Skua => return Outcome::Update,
            _ => {}
        }
        Outcome::Nothing
    }
}

fn mira_scripts(folder: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    match walk(folder, "", &mut entries) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(format!("couldn't list {}: {e}", folder.display()));
        }
        _ => {}
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, dir: &str, entries: &mut Vec<Entry>) -> io::Result<()> {
    for found in fs::read_dir(root.join(dir))? {
        let found = found?;
        let Ok(name) = found.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = format!("{dir}{name}");
        if found.file_type()?.is_dir() {
            walk(root, &format!("{path}/"), entries)?;
        } else if MIRA_EXTENSIONS.iter().any(|ext| name.ends_with(ext))
            && !name.ends_with(".d.ts")
            && found.path().is_file()
        {
            let stem = Path::new(&name).file_stem().unwrap_or_default();
            entries.push(Entry {
                name: stem.to_string_lossy().into_owned(),
                description: String::new(),
                tags: Vec::new(),
                path,
            });
        }
    }
    Ok(())
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
