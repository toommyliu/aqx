use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::{fs, io, thread};

use aqx_script::skua::library::{self, Entry, Library, Updated};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use tui_input::Input as TextInput;
use tui_input::backend::crossterm::EventHandler;

const PAGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Files,
    Skua,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Files, Tab::Skua];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Files => "files",
            Tab::Skua => "Skua",
        }
    }

    fn other(self) -> Tab {
        match self {
            Tab::Files => Tab::Skua,
            Tab::Skua => Tab::Files,
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
    Options(PathBuf),
    Update,
    Nothing,
}

pub struct Picker {
    pub tab: Tab,
    pub home: PathBuf,
    pub here: PathBuf,
    pub files: Result<Vec<Entry>, String>,
    pub scripts: PathBuf,
    pub catalog: Catalog,
    pub query: TextInput,
    pub searching: bool,
    pub cursor: usize,
    pub failed: Option<String>,
    typed: Option<(PathBuf, Vec<Entry>)>,
}

impl Picker {
    pub fn open(home: &Path, tab: Tab) -> Picker {
        let (scripts, found) = match crate::config::skua_home() {
            Ok(home) => (library::scripts(&home), library::open(&home)),
            Err(why) => (PathBuf::new(), Err(why)),
        };
        Picker::new(tab, scripts, found).with_files(home.join("scripts"))
    }

    pub fn new(tab: Tab, scripts: PathBuf, found: Result<Option<Library>, String>) -> Picker {
        let mut picker = Picker {
            tab,
            home: PathBuf::new(),
            here: PathBuf::new(),
            files: Ok(Vec::new()),
            scripts,
            catalog: Catalog::Missing,
            query: TextInput::default(),
            searching: false,
            cursor: 0,
            failed: None,
            typed: None,
        };
        picker.load(found);
        picker
    }

    pub fn with_files(mut self, home: PathBuf) -> Picker {
        self.browse(home.clone());
        self.home = home;
        self
    }

    fn browse(&mut self, folder: PathBuf) {
        self.files = list(&folder);
        self.here = folder;
        self.query.reset();
        self.searching = false;
        self.edited();
    }

    pub fn edited(&mut self) {
        self.cursor = 0;
        self.typed = typed_path(self.query.value(), &self.here).map(|(folder, start)| {
            let start = start.to_lowercase();
            let mut entries = list(&folder).unwrap_or_default();
            entries.retain(|e| e.name.to_lowercase().starts_with(&start));
            (folder, entries)
        });
        if self.typed.is_some() {
            self.tab = Tab::Files;
        }
    }

    fn up(&mut self) {
        let Some(parent) = self.here.parent().map(Path::to_path_buf) else {
            return;
        };
        let left = self
            .here
            .file_name()
            .map(|name| format!("{}/", name.to_string_lossy()));
        self.browse(parent);
        self.cursor = self
            .shown()
            .iter()
            .position(|e| Some(&e.path) == left.as_ref())
            .unwrap_or(0);
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
            Tab::Files => self.typed.as_ref().map_or(&self.here, |(folder, _)| folder),
            Tab::Skua => &self.scripts,
        }
    }

    pub fn shown(&self) -> Vec<&Entry> {
        let query = self.query.value();
        match (self.tab, &self.files, &self.catalog) {
            (Tab::Files, _, _) if let Some((_, entries)) = &self.typed => entries.iter().collect(),
            (Tab::Files, Ok(entries), _) => library::search(entries, query),
            (Tab::Skua, _, Catalog::Ready(found)) => library::search(&found.entries, query),
            _ => Vec::new(),
        }
    }

    pub fn select(&mut self, script: &Path) {
        if !self.scripts.as_os_str().is_empty() && script.starts_with(&self.scripts) {
            self.tab = Tab::Skua;
            self.cursor = 0;
        } else if let Some(folder) = script.parent() {
            self.tab = Tab::Files;
            self.browse(folder.to_path_buf());
        }
        if let Some(at) = self
            .shown()
            .iter()
            .position(|e| self.folder().join(&e.path) == script)
        {
            self.cursor = at;
        }
    }

    fn choose(&mut self, options: bool) -> Outcome {
        let Some(entry) = self.shown().get(self.cursor).map(|e| e.path.clone()) else {
            self.searching = false;
            return Outcome::Nothing;
        };
        let path = self.folder().join(&entry);
        if entry.ends_with('/') {
            self.browse(path);
            Outcome::Nothing
        } else if !options {
            Outcome::Run(path)
        } else if super::skua_script(Some(&entry)).is_some() {
            Outcome::Options(path)
        } else {
            Outcome::Nothing
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
            KeyCode::Enter => return self.choose(false),
            KeyCode::Esc if self.searching || !self.query.value().is_empty() => {
                self.query.reset();
                self.searching = false;
                self.edited();
            }
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Right if self.query.cursor() == self.query.value().chars().count() => {
                return self.choose(true);
            }
            _ if self.searching => {
                if self
                    .query
                    .handle_event(&Event::Key(key))
                    .is_some_and(|changed| changed.value)
                {
                    self.edited();
                }
            }
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('u') if self.tab == Tab::Skua => return Outcome::Update,
            KeyCode::Left if self.tab == Tab::Files => self.up(),
            _ => {}
        }
        Outcome::Nothing
    }
}

fn typed_path(query: &str, here: &Path) -> Option<(PathBuf, String)> {
    let mut chars = query.trim().trim_matches(['\'', '"']).chars();
    let mut query = String::new();
    while let Some(c) = chars.next() {
        query.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    if !["/", "~", "./", "../"]
        .iter()
        .any(|start| query.starts_with(start))
    {
        return None;
    }
    let (folder, start) = query.split_at(query.rfind('/').map_or(query.len(), |i| i + 1));
    let mut resolved = PathBuf::new();
    for part in here
        .join(super::prompt::expand_home(Path::new(folder)))
        .components()
    {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            part => resolved.push(part),
        }
    }
    Some((resolved, start.to_owned()))
}

pub(crate) fn list(folder: &Path) -> Result<Vec<Entry>, String> {
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
            } else if super::prompt::script_file(&name) && path.is_file() {
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
            aqx_script::skua::worker(&mut || stage("building the Skua worker"))
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
