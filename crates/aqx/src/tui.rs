use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthChar;

use aqx_script::Arg;

use crate::account::{expand_home, script_path};
use crate::hub::{Bar, Cells, Command, Game, Log, LogLine, Snapshot};
use crate::output::{clock, span};
use crate::socket;

const WIDE: u16 = 90;
const NOTE_FOR: Duration = Duration::from_secs(4);
const ACCENT: Color = Color::Blue;
const CARD_WIDTH: u16 = 44;
const CARD_HEIGHT: u16 = 8;
const WHEEL_LINES: usize = 3;
const LIST_MIN: u16 = 10;
const ABOUT_WIDTH: usize = 14;
const LIST_STEP: u16 = 4;
const PANE_MIN: u16 = 40;
const LABEL_MIN: usize = 6;
const FRAME: Duration = Duration::from_millis(125);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

type Field = Vec<Span<'static>>;

const ACCOUNT_KEYS: [(&str, &str); 13] = [
    ("", "Accounts"),
    ("↑ ↓", "pick"),
    ("tab", "next"),
    ("{ }", "online / offline"),
    ("/", "filter by name"),
    ("space", "mark"),
    ("a", "mark all / none"),
    ("esc", "clear marks"),
    ("i", "log in"),
    ("x", "log out"),
    ("q", "detach"),
    ("Q", "log out all and end"),
    ("ctrl+c", "end or detach"),
];

const SCRIPT_KEYS: [(&str, &str); 14] = [
    ("", "Scripts"),
    ("o", "run a script"),
    ("s", "stop"),
    ("r", "restart"),
    ("p", "pause / resume"),
    ("", "View"),
    ("g", "dashboard"),
    ("d", "more details"),
    ("b", "hide sidebar"),
    ("[ ]", "resize sidebar"),
    ("l", "filter the log"),
    ("⇧↑ ⇧↓", "scroll a line"),
    ("PgUp PgDn", "scroll a page"),
    ("End", "newest lines"),
];

pub struct Fleet {
    pub title: String,
    pub cells: Arc<Cells>,
    pub socket: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Filter {
    #[default]
    Main,
    All,
    Script,
    Aqx,
    Server,
    Game,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::Main => Filter::All,
            Filter::All => Filter::Script,
            Filter::Script => Filter::Aqx,
            Filter::Aqx => Filter::Server,
            Filter::Server => Filter::Game,
            Filter::Game => Filter::Main,
        }
    }

    fn shows(self, line: &LogLine) -> bool {
        match self {
            Filter::Main => !line.is_game(),
            Filter::All => true,
            Filter::Script => line.source == "script",
            Filter::Aqx => line.source == "aqx",
            Filter::Server => line.source == "server",
            Filter::Game => line.is_game(),
        }
    }

    fn tagged(self) -> bool {
        matches!(self, Filter::Main | Filter::All)
    }

    fn label(self) -> &'static str {
        match self {
            Filter::Main => "main",
            Filter::All => "all",
            Filter::Script => "script",
            Filter::Aqx => "aqx",
            Filter::Server => "server",
            Filter::Game => "game",
        }
    }
}

struct Prompt {
    input: String,
    draft: String,
    pick: Option<usize>,
    matches: Vec<String>,
}

impl Prompt {
    fn new() -> Self {
        let mut prompt = Prompt { input: String::new(), draft: String::new(), pick: None, matches: Vec::new() };
        prompt.find_matches();
        prompt
    }

    fn find_matches(&mut self) {
        self.matches = scripts_matching(&self.input);
    }

    fn recall(&mut self, recent: &[Recent], older: bool) {
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
        self.input.clone_from(&recent[recent.len() - 1 - pick].typed);
    }
}

struct Recent {
    typed: String,
    script: PathBuf,
}

struct Ask {
    typed: String,
    script: PathBuf,
    given: Vec<(String, String)>,
    asked: Vec<Arg>,
    accounts: Vec<(usize, String)>,
    answers: Vec<String>,
    input: String,
}

impl Ask {
    fn can_share(&self) -> bool {
        self.accounts.len() > 1 && self.answers.is_empty()
    }

    fn per_account(&self, shared: bool) -> bool {
        self.accounts.len() > 1 && !shared
    }

    fn asking(&self, shared: bool) -> (&Arg, Option<&str>) {
        let n = self.answers.len();
        let account = self.per_account(shared).then(|| self.accounts[n / self.asked.len()].1.as_str());
        (&self.asked[n % self.asked.len()], account)
    }

    fn answered(&self, round: &[String]) -> impl Iterator<Item = (String, String)> {
        self.asked.iter().zip(round).filter(|(_, value)| !value.is_empty()).map(|(arg, value)| (arg.key.clone(), value.clone()))
    }

    fn left(&self, shared: bool) -> usize {
        let rounds = if self.per_account(shared) { self.accounts.len() } else { 1 };
        rounds * self.asked.len() - self.answers.len()
    }

    fn args(&self, shared: bool) -> Vec<(usize, Vec<(String, String)>)> {
        let per_account = self.per_account(shared);
        self.accounts
            .iter()
            .enumerate()
            .map(|(round, &(i, _))| {
                let answers = &self.answers[if per_account { round * self.asked.len() } else { 0 }..];
                (i, self.given.iter().cloned().chain(self.answered(answers)).collect())
            })
            .collect()
    }
}

#[derive(Default)]
struct App {
    selected: usize,
    marked: BTreeSet<usize>,
    details: bool,
    grid: bool,
    keys: bool,
    list_hidden: bool,
    list_width: Option<u16>,
    list_area: Cell<Option<Rect>>,
    dragging: bool,
    frame: usize,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    log_width: Cell<usize>,
    unseen_from: Option<u64>,
    jump_area: Cell<Option<Rect>>,
    list_top: Cell<usize>,
    grid_top: Cell<usize>,
    grid_cols: Cell<usize>,
    prompt: Option<Prompt>,
    query: String,
    querying: bool,
    ask: Option<Ask>,
    share_args: bool,
    ending: bool,
    recent: Vec<Recent>,
    note: Option<(String, Instant)>,
    log: Log,
    log_of: Option<usize>,
}

struct Shown {
    name: String,
    snapshot: Option<Snapshot>,
}

pub fn run(fleet: &Fleet, done: impl Fn() -> bool) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        restore(info);
    }));
    let mut app = App::default();
    let started = Instant::now();
    loop {
        app.frame = (started.elapsed().as_millis() / FRAME.as_millis()) as usize;
        let shown = gather(fleet, &mut app);
        let _ = terminal.draw(|f| draw(f, fleet, &app, &shown));
        if done() {
            break;
        }
        if !event::poll(FRAME).unwrap_or(false) {
            continue;
        }
        match event::read() {
            Ok(Input::Key(key)) if key.kind == KeyEventKind::Press => {
                if app.handle(key, fleet, &shown) {
                    break;
                }
            }
            Ok(Input::Mouse(mouse)) => app.mouse(mouse, &shown),
            _ => {}
        }
    }
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

fn gather(fleet: &Fleet, app: &mut App) -> Vec<Shown> {
    if app.log_of != Some(app.selected) {
        app.log.clear();
        app.log_of = Some(app.selected);
    }
    if app.scroll == 0 {
        app.unseen_from = None;
    }
    let shown: Vec<Shown> = fleet
        .cells
        .all()
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let board = cell.board();
            if i == app.selected {
                let from = app.log.last().map_or(0, |l| l.seq + 1);
                let fresh = board.log.iter().rev().take_while(|l| l.seq >= from).count();
                let fresh = board.log.iter().skip(board.log.len() - fresh);
                if app.scroll > 0 {
                    for line in fresh.clone().filter(|l| app.filter.shows(l)) {
                        app.scroll += pieces(line, app.log_width.get(), app.filter.tagged()).1.len();
                        app.unseen_from.get_or_insert(line.seq);
                    }
                }
                for line in fresh {
                    app.log.push(line.clone());
                }
            }
            Shown { name: cell.name.clone(), snapshot: board.snapshot.clone() }
        })
        .collect();
    for typed in shown.iter().filter_map(|s| s.snapshot.as_ref()?.script.clone()) {
        let script = PathBuf::from(&typed);
        if !app.recent.iter().any(|r| r.script == script) {
            app.recent.insert(0, Recent { typed, script });
        }
    }
    shown
}

fn snapshot(shown: &[Shown], i: usize) -> Option<&Snapshot> {
    shown.get(i)?.snapshot.as_ref()
}

fn is_offline(s: &Snapshot) -> bool {
    s.phase == "OFFLINE"
}

fn order(shown: &[Shown], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let (online, offline): (Vec<usize>, Vec<usize>) =
        (0..shown.len()).filter(|&i| shown[i].name.to_lowercase().contains(&query)).partition(|&i| !snapshot(shown, i).is_some_and(is_offline));
    [online, offline].concat()
}

fn complete(query: &str, names: &[&str]) -> String {
    if let [only] = names {
        return only.to_string();
    }
    let mut lower: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    lower.sort();
    let prefix = shared_prefix(&lower);
    if prefix.contains(&query.to_lowercase()) { prefix.to_string() } else { query.to_string() }
}

enum Mark {
    Spin,
    Still(&'static str),
}

struct Status {
    mark: Mark,
    color: Color,
    word: &'static str,
    about: String,
}

impl Status {
    fn of(snapshot: Option<&Snapshot>) -> Self {
        let busy = |word: &'static str| Status { mark: Mark::Spin, color: Color::Yellow, word, about: word.into() };
        let still = |glyph, color, word, about| Status { mark: Mark::Still(glyph), color, word, about };
        let Some(s) = snapshot else { return busy("starting") };
        let script = s.script_label();
        match s.state() {
            "CONNECTING" => busy("connecting"),
            "LOGGING IN" => busy("logging in"),
            "LOADING" => busy("loading"),
            "RECONNECTING" => Status { about: format!("reconnecting {}", s.detail), ..busy("reconnecting") },
            "PAUSED" => still("‖", Color::Yellow, "paused", format!("{script}, paused")),
            "STALLED" => still("!", Color::Yellow, "stalled", format!("{script}, stalled")),
            "READY" if s.run == "running" => Status { mark: Mark::Spin, color: Color::Cyan, word: "running", about: script },
            "READY" => still("●", Color::Green, "online", script),
            "OFFLINE" if s.detail.is_empty() => still("○", Color::DarkGray, "offline", "logged out".into()),
            "OFFLINE" => still("✗", Color::Red, "offline", s.detail.clone()),
            _ => still("✗", Color::Red, "disconnected", if s.detail.is_empty() { "disconnected".into() } else { s.detail.clone() }),
        }
    }

    fn mark(&self, frame: usize) -> Span<'static> {
        let glyph = match self.mark {
            Mark::Spin => SPINNER[frame % SPINNER.len()],
            Mark::Still(glyph) => glyph,
        };
        Span::raw(glyph).fg(self.color)
    }

    fn badge(&self, frame: usize) -> Field {
        vec![self.mark(frame), Span::raw(format!(" {}", self.word)).fg(self.color).bold()]
    }
}

impl App {
    fn targets(&self) -> Vec<usize> {
        if self.marked.is_empty() { vec![self.selected] } else { self.marked.iter().copied().collect() }
    }

    fn all(&self, shown: &[Shown], test: impl Fn(&Snapshot) -> bool) -> bool {
        self.targets().iter().all(|&i| snapshot(shown, i).is_some_and(&test))
    }

    fn who(&self, shown: &[Shown], room: usize) -> String {
        let targets = self.targets();
        let names = targets.iter().filter_map(|&i| shown.get(i)).map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ");
        if names.chars().count() <= room { names } else { format!("{} accounts", targets.len()) }
    }

    fn step(&mut self, shown: &[Shown], by: isize, wrap: bool) {
        let order = order(shown, &self.query);
        let Some(at) = order.iter().position(|&i| i == self.selected) else { return };
        let (at, n) = (at as isize + by, order.len() as isize);
        self.pick(order[if wrap { at.rem_euclid(n) } else { at.clamp(0, n - 1) } as usize]);
    }

    fn jump(&mut self, shown: &[Shown], by: isize) {
        let order = order(shown, &self.query);
        let offline = |i| snapshot(shown, i).is_some_and(is_offline);
        let starts: Vec<usize> = (0..order.len()).filter(|&at| at == 0 || offline(order[at]) != offline(order[at - 1])).collect();
        let Some(at) = order.iter().position(|&i| i == self.selected) else { return };
        let group = starts.iter().rposition(|&start| start <= at).unwrap_or(0) as isize;
        self.pick(order[starts[(group + by).rem_euclid(starts.len() as isize) as usize]]);
    }

    fn refilter(&mut self, shown: &[Shown]) {
        let order = order(shown, &self.query);
        if let Some(&first) = order.first().filter(|_| !order.contains(&self.selected)) {
            self.pick(first);
        }
    }

    fn pick(&mut self, i: usize) {
        if i != self.selected {
            self.selected = i;
            self.scroll = 0;
        }
    }

    fn mouse(&mut self, mouse: MouseEvent, shown: &[Shown]) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(mouse.column, mouse.row, shown),
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => self.list_width = Some(mouse.column + 1),
            MouseEventKind::Up(_) => self.dragging = false,
            _ if self.grid => {}
            MouseEventKind::ScrollUp => self.scroll = (self.scroll + WHEEL_LINES).min(self.max_scroll.get()),
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_sub(WHEEL_LINES),
            _ => {}
        }
    }

    fn click(&mut self, column: u16, row: u16, shown: &[Shown]) {
        if self.jump_area.get().is_some_and(|pill| pill.contains(Position::new(column, row))) {
            self.scroll = 0;
            return;
        }
        let list = self.list_area.get();
        self.dragging = list.is_some_and(|l| column + 1 == l.right() || column == l.right());
        let Some(list) = list.filter(|l| !self.dragging && l.contains(Position::new(column, row))) else { return };
        let at = self.list_top.get() + usize::from(row - list.y);
        if let Some(&i) = list_rows(shown, &self.query).get(at..).and_then(|rows| rows.iter().flatten().next()) {
            self.pick(i);
        }
    }

    fn resize_list(&mut self, by: impl Fn(u16) -> u16) {
        if let Some(list) = self.list_area.get() {
            self.list_width = Some(by(list.width));
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), Instant::now() + NOTE_FOR));
    }

    fn act(&mut self, fleet: &Fleet, command: Command, done: &str) {
        let commands = self.targets().into_iter().map(|i| (i, command.clone())).collect();
        self.act_on(fleet, commands, done);
    }

    fn act_on(&mut self, fleet: &Fleet, commands: Vec<(usize, Command)>, done: &str) {
        let cells = fleet.cells.all();
        let (mut sent, mut same, mut said, mut unchanged) = (Vec::new(), Vec::new(), Vec::new(), "");
        for (i, command) in commands {
            let Some(cell) = cells.get(i) else { continue };
            match socket::ctl(&fleet.socket, &cell.name, &command) {
                Ok(true) => sent.push(cell.name.as_str()),
                Ok(false) => {
                    same.push(cell.name.as_str());
                    unchanged = command.unchanged();
                }
                Err(e) => said.push(e),
            }
        }
        if !same.is_empty() {
            said.insert(0, format!("{}: {unchanged}", same.join(", ")));
        }
        if !sent.is_empty() {
            said.insert(0, format!("{}: {done}", sent.join(", ")));
        }
        self.say(said.join("; "));
    }

    fn handle(&mut self, key: KeyEvent, fleet: &Fleet, shown: &[Shown]) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.ending {
                return true;
            }
            self.ending = true;
            return false;
        }
        if self.keys {
            self.keys = false;
            return false;
        }
        if self.ending {
            match key.code {
                KeyCode::Enter => {
                    self.ending = false;
                    let everyone = (0..shown.len()).map(|i| (i, Command::Logout)).collect();
                    self.act_on(fleet, everyone, "logging out; the session ends once every account is offline");
                }
                KeyCode::Char('q') => return true,
                KeyCode::Esc => self.ending = false,
                _ => {}
            }
            return false;
        }
        if let Some(prompt) = &mut self.prompt {
            match key.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Enter => {
                    let prompt = self.prompt.take().expect("a prompt is open");
                    self.submit(prompt, fleet);
                }
                KeyCode::Up | KeyCode::Down if !self.recent.is_empty() => prompt.recall(&self.recent, key.code == KeyCode::Up),
                KeyCode::Backspace => {
                    prompt.pick = None;
                    prompt.input.pop();
                }
                KeyCode::Tab => {
                    prompt.pick = None;
                    match prompt.matches.as_slice() {
                        [] => {}
                        [only] if !only.ends_with('/') => prompt.input = format!("{only} "),
                        many => prompt.input = shared_prefix(many).to_string(),
                    }
                }
                KeyCode::Char(c) => {
                    prompt.pick = None;
                    prompt.input.push(c);
                }
                _ => {}
            }
            if let Some(prompt) = &mut self.prompt {
                prompt.find_matches();
            }
            return false;
        }
        if self.querying {
            match key.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.querying = false;
                }
                KeyCode::Enter => self.querying = false,
                KeyCode::Up => self.step(shown, -1, false),
                KeyCode::Down => self.step(shown, 1, false),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Tab => {
                    let names: Vec<&str> = order(shown, &self.query).into_iter().map(|i| shown[i].name.as_str()).collect();
                    self.query = complete(&self.query, &names);
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => {}
            }
            self.refilter(shown);
            return false;
        }
        if let Some(ask) = &mut self.ask {
            match key.code {
                KeyCode::Esc => self.ask = None,
                KeyCode::Tab if ask.can_share() => self.share_args = !self.share_args,
                KeyCode::Enter if !ask.input.trim().is_empty() || ask.asking(self.share_args).0.default.is_some() => {
                    let value = std::mem::take(&mut ask.input).trim().to_string();
                    ask.answers.push(value);
                    if ask.left(self.share_args) == 0 {
                        let ask = self.ask.take().expect("an ask is open");
                        let mut typed = ask.typed.clone();
                        if !ask.per_account(self.share_args) {
                            typed.extend(ask.answered(&ask.answers).map(|(key, value)| format!(" {key}={value}")));
                        }
                        self.run(fleet, typed, &ask.script, ask.args(self.share_args));
                    }
                }
                KeyCode::Backspace => {
                    ask.input.pop();
                }
                KeyCode::Char(c) => ask.input.push(c),
                _ => {}
            }
            return false;
        }
        let accounts = shown.len();
        let visible = order(shown, &self.query);
        let cols = self.grid_cols.get() as isize;
        let line = accounts < 2 || key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('Q') => self.ending = true,
            KeyCode::Char('?') => self.keys = true,
            KeyCode::Char('g') => self.grid = !self.grid,
            KeyCode::Char('d' | 'l') | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End if self.grid => {}
            KeyCode::Enter if self.grid => self.grid = false,
            KeyCode::Left if self.grid => self.step(shown, -1, false),
            KeyCode::Right if self.grid => self.step(shown, 1, false),
            KeyCode::Up if self.grid => self.step(shown, -cols, false),
            KeyCode::Down if self.grid => self.step(shown, cols, false),
            KeyCode::Up if line => self.scroll = (self.scroll + 1).min(self.max_scroll.get()),
            KeyCode::Down if line => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Up => self.step(shown, -1, false),
            KeyCode::Down => self.step(shown, 1, false),
            KeyCode::Tab => self.step(shown, 1, true),
            KeyCode::BackTab => self.step(shown, -1, true),
            KeyCode::Char('}') => self.jump(shown, 1),
            KeyCode::Char('{') => self.jump(shown, -1),
            KeyCode::Char(' ') => {
                if !self.marked.remove(&self.selected) {
                    self.marked.insert(self.selected);
                }
            }
            KeyCode::Char('a') if visible.iter().all(|i| self.marked.contains(i)) => self.marked.retain(|i| !visible.contains(i)),
            KeyCode::Char('a') => self.marked.extend(visible),
            KeyCode::Char('/') => self.querying = true,
            KeyCode::Esc => self.marked.clear(),
            KeyCode::PageUp => self.scroll = (self.scroll + 10).min(self.max_scroll.get()),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = self.max_scroll.get(),
            KeyCode::End => self.scroll = 0,
            KeyCode::Char('l') => {
                self.filter = self.filter.next();
                self.scroll = 0;
            }
            KeyCode::Char('d') => self.details = !self.details,
            KeyCode::Char('b') => self.list_hidden = !self.list_hidden,
            KeyCode::Char('[') => self.resize_list(|w| w.saturating_sub(LIST_STEP)),
            KeyCode::Char(']') => self.resize_list(|w| w + LIST_STEP),
            KeyCode::Char('o') => self.prompt = Some(Prompt::new()),
            KeyCode::Char('s') => self.act(fleet, Command::Stop, "stopping the script"),
            KeyCode::Char('r') => self.act(fleet, Command::Restart, "restarting the script"),
            KeyCode::Char('i') => self.act(fleet, Command::Login, "logging in"),
            KeyCode::Char('x') => self.act(fleet, Command::Logout, "logging out"),
            KeyCode::Char('p') if self.all(shown, |s| s.paused) => self.act(fleet, Command::Resume, "resumed"),
            KeyCode::Char('p') => self.act(fleet, Command::Pause, "paused"),
            _ => {}
        }
        false
    }

    fn submit(&mut self, prompt: Prompt, fleet: &Fleet) {
        let input = prompt.input.trim().to_string();
        if input.is_empty() {
            return;
        }
        let mut words = input.split_whitespace();
        let path = match script_path(words.next().unwrap_or_default().as_ref()) {
            Ok(path) => path,
            Err(e) => return self.say(e),
        };
        let mut args = Vec::new();
        for word in words {
            match word.trim_start_matches("--arg=").split_once('=') {
                Some((key, value)) if !key.is_empty() => args.push((key.to_string(), value.to_string())),
                _ => return self.say(format!("{word} isn't KEY=VALUE")),
            }
        }
        let asked: Vec<Arg> = aqx_script::script_args(&path).into_iter().filter(|arg| !args.iter().any(|(given, _)| *given == arg.key)).collect();
        let targets = self.targets();
        if asked.is_empty() {
            let each = targets.into_iter().map(|i| (i, args.clone())).collect();
            return self.run(fleet, input, &path, each);
        }
        let cells = fleet.cells.all();
        let accounts = targets.into_iter().filter_map(|i| Some((i, cells.get(i)?.name.clone()))).collect();
        self.ask = Some(Ask { typed: input, script: path, given: args, asked, accounts, answers: Vec::new(), input: String::new() });
    }

    fn run(&mut self, fleet: &Fleet, typed: String, script: &Path, each: Vec<(usize, Vec<(String, String)>)>) {
        self.recent.retain(|r| r.typed != typed);
        self.recent.push(Recent { typed, script: script.to_path_buf() });
        let commands = each.into_iter().map(|(i, args)| (i, Command::Run { script: script.to_path_buf(), args })).collect();
        self.act_on(fleet, commands, &format!("running {}", script.display()));
    }
}

fn draw(f: &mut Frame, fleet: &Fleet, app: &App, shown: &[Shown]) {
    let matches: Vec<String> = match &app.prompt {
        _ if app.querying => order(shown, &app.query).into_iter().map(|i| shown[i].name.clone()).collect(),
        Some(prompt) => prompt.matches.clone(),
        None => Vec::new(),
    };
    let [header, body, suggestions, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(!matches.is_empty())),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_matches(f, suggestions, &matches);
    let list = !app.grid && !app.list_hidden && body.width >= WIDE && shown.len() > 1;
    app.list_area.set(None);
    app.jump_area.set(None);
    draw_header(f, header, fleet, app, shown, list || app.grid);
    if app.grid {
        draw_grid(f, body, app, shown);
    } else if let Some(account) = shown.get(app.selected) {
        let pane = if list {
            let fitted = (4 + name_width(shown) + 1 + ABOUT_WIDTH + 1) as u16;
            let width = app.list_width.unwrap_or(fitted).clamp(LIST_MIN, body.width - PANE_MIN);
            let [left, _, pane] = Layout::horizontal([Constraint::Length(width), Constraint::Length(1), Constraint::Min(0)]).areas(body);
            app.list_area.set(Some(left));
            draw_list(f, left, app, shown);
            pane
        } else {
            body.inner(Margin::new(1, 0))
        };
        draw_account(f, pane, account, app);
    }
    draw_footer(f, footer, app, shown);
    if app.keys {
        draw_keys(f, body);
    }
    if app.ending {
        draw_ending(f, body, shown.len());
    }
}

fn scripts_matching(input: &str) -> Vec<String> {
    let input = input.trim_start();
    if input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (dir, prefix) = input.split_at(input.rfind('/').map_or(0, |i| i + 1));
    let Ok(entries) = std::fs::read_dir(expand_home(std::path::Path::new(if dir.is_empty() { "." } else { dir }))) else { return Vec::new() };
    let mut matches: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) || name.contains(char::is_whitespace) {
                return None;
            }
            let kind = entry.file_type().ok()?;
            if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                Some(format!("{dir}{name}/"))
            } else if [".ts", ".js", ".mjs"].iter().any(|ext| name.ends_with(ext)) && !name.ends_with(".d.ts") {
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

fn shared_prefix(sorted: &[String]) -> &str {
    let (Some(first), Some(last)) = (sorted.first(), sorted.last()) else { return "" };
    let end = first.char_indices().zip(last.chars()).find(|((_, a), b)| a != b).map_or(first.len(), |((i, _), _)| i);
    &first[..end]
}

fn draw_matches(f: &mut Frame, area: Rect, matches: &[String]) {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (i, path) in matches.iter().enumerate() {
        let name = &path[path.trim_end_matches('/').rfind('/').map_or(0, |at| at + 1)..];
        let span = if name.ends_with('/') { Span::raw(name).blue() } else { Span::raw(name) };
        let gap = if i == 0 { "" } else { "  " };
        let after = matches.len() - i - 1;
        let reserve = if after == 0 { 0 } else { format!("  +{after} more").len() };
        if used + gap.len() + span.width() + reserve > usize::from(area.width) {
            spans.push(Span::raw(format!("{gap}+{} more", matches.len() - i)).dark_gray());
            break;
        }
        used += gap.len() + span.width();
        spans.extend([Span::raw(gap), span]);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_header(f: &mut Frame, area: Rect, fleet: &Fleet, app: &App, shown: &[Shown], overview: bool) {
    let mut left = vec![Span::raw(format!(" {}", fleet.title)).bold()];
    if overview {
        let online = shown.iter().filter(|s| s.snapshot.as_ref().is_some_and(|s| s.phase != "OFFLINE")).count();
        left.push(Span::raw(format!("  {online} of {} online", shown.len())).dark_gray());
    } else if let Some(account) = shown.get(app.selected).filter(|_| shown.len() > 1) {
        let order = order(shown, &app.query);
        let at = order.iter().position(|&i| i == app.selected).unwrap_or(0);
        left.push(Span::raw(format!("  {} {}/{}", account.name, at + 1, order.len())).dark_gray());
    }
    if !app.query.is_empty() {
        left.push(Span::raw(format!(" · filter \"{}\"", app.query)).dark_gray());
    }
    if !app.marked.is_empty() {
        left.push(Span::raw(format!(" · {} marked", app.marked.len())).dark_gray());
    }
    let mut keys = Vec::new();
    if !overview && shown.len() > 1 {
        keys.push(("tab", "next"));
    }
    if shown.len() > 1 {
        keys.extend([("g", "dashboard"), ("/", "filter")]);
    }
    keys.extend([("?", "keys"), ("q", "detach")]);
    f.render_widget(Paragraph::new(spread(left, hints(&keys), area.width.saturating_sub(1) as usize)), area);
}

fn name_width(shown: &[Shown]) -> usize {
    shown.iter().map(|s| s.name.chars().count()).max().unwrap_or(0).clamp(4, 14)
}

fn draw_list(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let inner = usize::from(area.width.saturating_sub(1));
    let name_col = name_width(shown).min(inner.saturating_sub(4));
    let rows = list_rows(shown, &app.query);
    let offline = rows.iter().flatten().filter(|&&i| snapshot(shown, i).is_some_and(is_offline)).count();
    let height = (area.height as usize).max(1);
    let at = rows.iter().position(|&row| row == Some(app.selected)).unwrap_or(0);
    let top = app.list_top.get().min(at).max((at + 1).saturating_sub(height));
    app.list_top.set(top);
    let lines: Vec<Line> = rows
        .iter()
        .skip(top)
        .take(height)
        .map(|&row| match row {
            Some(i) => list_row(&shown[i], i == app.selected, app.marked.contains(&i), app.frame, name_col, inner),
            None => {
                let label = format!("── offline {offline} ");
                let fill = inner.saturating_sub(label.chars().count());
                fit(vec![Span::raw(format!("{label}{}", "─".repeat(fill))).dark_gray()], inner)
            }
        })
        .collect();
    let rule = Block::new().borders(Borders::RIGHT).border_style(Style::new().dark_gray());
    f.render_widget(Paragraph::new(lines).block(rule), area);
}

fn list_rows(shown: &[Shown], query: &str) -> Vec<Option<usize>> {
    let mut rows: Vec<Option<usize>> = order(shown, query).into_iter().map(Some).collect();
    let offline = rows.iter().flatten().filter(|&&i| snapshot(shown, i).is_some_and(is_offline)).count();
    if 0 < offline && offline < rows.len() {
        rows.insert(rows.len() - offline, None);
    }
    rows
}

fn list_row(account: &Shown, selected: bool, marked: bool, frame: usize, name_col: usize, inner: usize) -> Line<'static> {
    let status = Status::of(account.snapshot.as_ref());
    let name = Span::raw(account.name.clone());
    let mut spans = vec![
        Span::raw(if selected { "▌" } else { " " }).fg(ACCENT),
        Span::raw(if marked { "◆" } else { " " }).fg(ACCENT),
        status.mark(frame),
        Span::raw(" "),
    ];
    spans.extend(clip(vec![if selected { name.bold() } else { name }], name_col));
    if inner >= 4 + name_col + 1 + LABEL_MIN {
        spans.push(Span::raw(" ".repeat(4 + name_col + 1 - spans_width(&spans))));
        spans.push(Span::raw(status.about).dark_gray());
    }
    fit(spans, inner)
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let width = area.width as usize;
    let who = app.who(shown, width / 3);
    if app.querying {
        let keys = hints(&[("↑↓", "pick"), ("tab", "complete"), ("enter", "done"), ("esc", "clear")]);
        let typed = vec![Span::raw(" filter accounts: ").bold(), Span::raw(app.query.clone()), Span::raw("█")];
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some(prompt) = &app.prompt {
        let label = format!(" run on {who}: ");
        let keys = hints(&[("tab", "complete"), ("↑↓", "recent"), ("esc", "cancel")]);
        let typed = vec![Span::raw(label).bold(), Span::raw(prompt.input.clone()), Span::raw("█")];
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some(ask) = &app.ask {
        let script = ask.script.file_name().map_or_else(|| ask.script.display().to_string(), |name| name.to_string_lossy().into_owned());
        let (arg, account) = ask.asking(app.share_args);
        let account = account.map(|a| format!(" for {a}")).unwrap_or_default();
        let label = match arg.default {
            None => format!(" {script} needs {}{account}: ", arg.key),
            Some(_) => format!(" {script} {}{account}: ", arg.key),
        };
        let mut keys = vec![("enter", if ask.left(app.share_args) > 1 { "next" } else { "run" })];
        if ask.can_share() {
            keys.push(("tab", if app.share_args { "ask each account" } else { "same for all" }));
        }
        keys.push(("esc", "cancel"));
        let keys = hints(&keys);
        let mut typed = vec![Span::raw(label).bold(), Span::raw(ask.input.clone()), Span::raw("█")];
        if let Some(default) = arg.default.as_deref().filter(|_| ask.input.is_empty()) {
            typed.push(Span::raw(if default.is_empty() { "optional" } else { default }.to_string()).dark_gray());
        }
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some((note, until)) = &app.note
        && Instant::now() < *until
    {
        return f.render_widget(Paragraph::new(format!(" {note}")).yellow(), area);
    }
    let details = if app.details { "less" } else { "details" };
    let offline = app.all(shown, is_offline);
    let mut keys: Vec<(&str, &str)> = if offline {
        vec![("i", "log in"), ("o", "log in and run")]
    } else {
        let pause = if app.all(shown, |s| s.paused) { "resume" } else { "pause" };
        vec![("o", "run"), ("s", "stop"), ("r", "restart"), ("p", pause), ("x", "log out")]
    };
    keys.push(("space", "mark"));
    match (app.grid, offline) {
        (true, _) => keys.push(("enter", "open")),
        (false, true) => keys.push(("d", details)),
        (false, false) => keys.extend([("d", details), ("l", "filter")]),
    }
    let mut line = vec![Span::raw(" → ").fg(ACCENT), Span::raw(who).bold(), Span::raw("   ")];
    let used = spans_width(&line);
    if used + spans_width(&hints(&keys)) <= width {
        line.extend(hints(&keys));
    } else {
        let more = spans_width(&hints(&[("?", "more")])) + 2;
        let fit = (0..keys.len()).rev().find(|&n| used + spans_width(&hints(&keys[..n])) + more <= width).unwrap_or(0);
        line.extend(hints(&keys[..fit]));
        if fit > 0 {
            line.push(Span::raw("  "));
        }
        line.extend(hints(&[("?", "more")]));
    }
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

fn draw_grid(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let cols = usize::from(area.width / CARD_WIDTH).max(1);
    let rows = usize::from(area.height / CARD_HEIGHT).max(1);
    app.grid_cols.set(cols);
    let order = order(shown, &app.query);
    let row = order.iter().position(|&i| i == app.selected).unwrap_or(0) / cols;
    let top = app.grid_top.get().min(row).max((row + 1).saturating_sub(rows));
    app.grid_top.set(top);
    for (at, &i) in order.iter().skip(top * cols).take(rows * cols).enumerate() {
        let account = &shown[i];
        let edge = |col: usize| (usize::from(area.width) * col / cols) as u16;
        let rect = Rect {
            x: area.x + edge(at % cols),
            y: area.y + (at / cols) as u16 * CARD_HEIGHT,
            width: edge(at % cols + 1) - edge(at % cols),
            height: CARD_HEIGHT,
        };
        draw_card(f, rect.intersection(area), account, i == app.selected, app.marked.contains(&i), app.frame);
    }
}

fn draw_card(f: &mut Frame, area: Rect, account: &Shown, selected: bool, marked: bool, frame: usize) {
    let mut badge = vec![Span::raw(" ")];
    badge.extend(Status::of(account.snapshot.as_ref()).badge(frame));
    badge.push(Span::raw(" "));
    let mut name = Vec::new();
    if marked {
        name.push(Span::raw(" ◆").fg(ACCENT));
    }
    name.push(Span::raw(format!(" {} ", account.name)).bold());
    let name = clip(name, usize::from(area.width).saturating_sub(spans_width(&badge) + 3));
    let (border, color) = if selected { (BorderType::Thick, ACCENT) } else { (BorderType::Plain, Color::DarkGray) };
    let block = Block::bordered()
        .border_type(border)
        .border_style(Style::new().fg(color))
        .title(Line::from(name).left_aligned())
        .title(Line::from(badge).right_aligned())
        .padding(Padding::horizontal(1));
    let width = usize::from(area.width.saturating_sub(4));
    f.render_widget(Paragraph::new(card(account, width)).block(block), area);
}

fn card(account: &Shown, width: usize) -> Vec<Line<'static>> {
    let Some(s) = &account.snapshot else { return vec![Line::raw("waiting for the server").dark_gray()] };
    let online = !is_offline(s);
    let mut about = Vec::new();
    if !s.user.is_empty() && !s.user.eq_ignore_ascii_case(&account.name) {
        about.push(s.user.clone());
    }
    if !s.server.is_empty() {
        about.push(s.server.clone());
    }
    if online && s.up_secs > 0 {
        about.push(format!("up {}", span(s.up_secs)));
    }
    let place = match (trouble(s), &s.game) {
        (Some(t), _) => vec![t],
        (None, Some(g)) => vec![Span::raw(format!("{} {}/{}", g.area, g.cell, g.pad))],
        (None, None) => Vec::new(),
    };
    let mut lines: Vec<Line> = [vec![Span::raw(about.join(" · ")).dark_gray()], place, task(s)].into_iter().map(|spans| fit(spans, width)).collect();
    if online {
        lines.extend(s.progress.first().map(|b| progress(b, width)));
        if let Some(g) = &s.game {
            lines.extend(flow(summary(s, g), width));
        }
    }
    lines
}

fn trouble(s: &Snapshot) -> Option<Span<'static>> {
    (s.phase != "READY" && !s.detail.is_empty()).then(|| Span::raw(s.detail.clone()).fg(if is_offline(s) { Color::Red } else { Color::Yellow }))
}

fn task(s: &Snapshot) -> Field {
    let online = !is_offline(s);
    let mut task = vec![match &s.script {
        Some(_) => Span::raw(s.script_label()).fg(if online { Color::Cyan } else { Color::DarkGray }),
        None => Span::raw("idle").dark_gray(),
    }];
    if online && !s.status.is_empty() {
        task.push(Span::raw(format!("  {}", s.status)));
    }
    task
}

fn draw_account(f: &mut Frame, area: Rect, account: &Shown, app: &App) {
    let width = area.width as usize;
    let mut lines = detail(account, app.details, app.frame, width);
    let pill_row = usize::from(app.scroll > 0);
    let log = log_lines(app, (area.height as usize).saturating_sub(lines.len() + 1 + pill_row), width);
    let mut rule = format!("── log · {}", app.filter.label());
    let scrolled = app.scroll.min(app.max_scroll.get());
    if scrolled > 0 {
        rule.push_str(&format!(" · ↑{scrolled}"));
    }
    let fill = width.saturating_sub(rule.chars().count() + 1);
    lines.push(Line::raw(format!("{rule} {}", "─".repeat(fill))).dark_gray());
    let has_log = !log.is_empty();
    lines.extend(log);
    f.render_widget(Paragraph::new(lines), area);
    if scrolled > 0 && has_log {
        draw_jump(f, area, app);
    }
}

fn draw_jump(f: &mut Frame, area: Rect, app: &App) {
    let unseen = app.unseen_from.map_or(0, |from| app.log.iter().filter(|l| l.seq >= from && app.filter.shows(l)).count());
    let label = if unseen > 0 { format!(" {unseen} new · jump to bottom (end) ↓ ") } else { " jump to bottom (end) ↓ ".to_string() };
    let width = (label.chars().count() as u16).min(area.width);
    let pill = Rect { x: area.x + (area.width - width) / 2, y: area.bottom() - 1, width, height: 1 };
    app.jump_area.set(Some(pill));
    f.render_widget(Paragraph::new(label).style(Style::new().fg(Color::White).bg(Color::DarkGray)), pill);
}

fn detail(account: &Shown, full: bool, frame: usize, width: usize) -> Vec<Line<'static>> {
    let Some(s) = &account.snapshot else {
        return vec![
            spread(vec![Span::raw(account.name.clone()).bold()], Status::of(None).badge(frame), width),
            Line::raw("waiting for the server").dark_gray(),
        ];
    };
    let online = s.phase != "OFFLINE";
    let mut lines = vec![identity(account, s, frame, width)];
    lines.extend(trouble(s).map(|t| fit(vec![t], width)));
    lines.push(fit(task(s), width));
    if online {
        lines.extend(s.progress.iter().map(|b| progress(b, width)));
    }
    if let Some(g) = &s.game {
        lines.push(Line::default());
        lines.extend(flow(if full { everything(s, g, width) } else { summary(s, g) }, width));
    }
    lines
}

fn identity(account: &Shown, s: &Snapshot, frame: usize, width: usize) -> Line<'static> {
    let mut left = vec![Span::raw(account.name.clone()).bold()];
    if !s.user.is_empty() && !s.user.eq_ignore_ascii_case(&account.name) {
        left.push(Span::raw(format!(" ({})", s.user)));
    }
    if !s.server.is_empty() {
        left.push(Span::raw(format!(" · {}", s.server)));
    }
    if let Some(g) = &s.game {
        left.push(Span::raw(format!(" · {} {}/{}", g.area, g.cell, g.pad)));
    }
    let mut right = Status::of(Some(s)).badge(frame);
    if !is_offline(s) && s.up_secs > 0 {
        right.push(Span::raw(format!("  up {}", span(s.up_secs))).dark_gray());
    }
    let left = clip(left, width.saturating_sub(spans_width(&right) + 2));
    spread(left, right, width)
}

fn progress(b: &Bar, width: usize) -> Line<'static> {
    let label: String = b.label.chars().take(15).collect();
    let count = format!("{}/{}", b.current, b.total);
    let eta = b.eta_secs.map(span).unwrap_or_default();
    let mut spans = vec![Span::raw(format!("{label:<16}"))];
    spans.extend(meter(b.current, b.total, width.saturating_sub(16 + 11 + 6).clamp(4, 30), Color::Green));
    spans.push(Span::raw(format!("{count:>11}")));
    spans.push(Span::raw(format!("{eta:>6}")).dark_gray());
    fit(spans, width)
}

fn summary(s: &Snapshot, g: &Game) -> Vec<Vec<Field>> {
    let mut fields = vec![hp(g)];
    if let Some(t) = &g.target {
        fields.push(labeled("Target", vec![Span::raw(t.name.clone())]));
    }
    fields.push(amount(s.kills, "kill"));
    if let Some(gold) = s.gold_gained {
        fields.push(gained(gold));
    }
    vec![fields]
}

fn everything(s: &Snapshot, g: &Game, width: usize) -> Vec<Vec<Field>> {
    let class = if g.class.is_empty() { "no class" } else { &g.class };
    let mut you = vec![
        hp(g),
        vitals("MP", g.mp, g.max_mp, Color::Blue),
        vec![Span::raw(format!("Lv {} {class}", g.level))],
        vec![Span::raw(short(g.gold)), Span::raw(" gold").dark_gray()],
    ];
    if g.afk {
        you.push(vec![Span::raw("away from keyboard").yellow()]);
    }
    let target = g.target.iter().map(|t| {
        let mut field = labeled("Target", vec![Span::raw(format!("{} ", t.name))]);
        field.extend(meter(t.hp, t.max_hp, 8, Color::Red));
        field.push(Span::raw(format!(" {}/{}", t.hp, t.max_hp)));
        field
    });
    let mut combat = Vec::new();
    if !g.skills.is_empty() {
        combat.push(skills(g));
    }
    if !g.auras.is_empty() {
        combat.push(listed("Auras", g.auras.iter().map(|a| (a.name.clone(), (a.stacks > 1).then(|| format!("x{}", a.stacks)))), width));
    }
    let mut around = Vec::new();
    if !g.cell_players.is_empty() {
        around.push(listed("Here", g.cell_players.iter().map(|p| (p.name.clone(), Some(p.level.to_string()))), width));
    }
    if !g.drops.is_empty() {
        around.push(listed("Offered", g.drops.iter().map(|d| (d.name.clone(), Some(format!("x{}", d.qty)))), width));
    }
    let mut stats = vec![labeled("Stats", amount(s.kills, "kill")), amount(s.drops, "drop")];
    stats.extend(s.gold_gained.map(gained));
    stats.push(amount(s.deaths, "death"));
    vec![you, target.collect(), combat, around, stats]
}

fn hp(g: &Game) -> Field {
    let color = match () {
        _ if g.dead => Color::DarkGray,
        _ if g.max_hp > 0 && g.hp * 4 < g.max_hp => Color::Red,
        _ => Color::Green,
    };
    vitals("HP", g.hp, g.max_hp, color)
}

fn vitals(label: &str, current: u32, max: u32, color: Color) -> Field {
    let mut field = vec![Span::raw(format!("{label} ")).dark_gray()];
    field.extend(meter(current, max, 8, color));
    field.push(Span::raw(format!(" {current}/{max}")));
    field
}

fn meter(current: u32, max: u32, width: usize, color: Color) -> Field {
    let filled = if max == 0 { 0 } else { (u64::from(current.min(max)) * width as u64 / u64::from(max)) as usize };
    vec![Span::raw("█".repeat(filled)).fg(color), Span::raw("░".repeat(width - filled)).dark_gray()]
}

fn labeled(label: &str, mut value: Field) -> Field {
    value.insert(0, Span::raw(format!("{label} ")).dark_gray());
    value
}

fn amount(n: u64, noun: &str) -> Field {
    let noun = if n == 1 { noun.to_string() } else { format!("{noun}s") };
    vec![Span::raw(grouped(n)), Span::raw(format!(" {noun}")).dark_gray()]
}

fn gained(gold: i64) -> Field {
    let text = if gold >= 0 { format!("+{}", short(gold)) } else { short(gold) };
    vec![Span::raw(text), Span::raw(" gold").dark_gray()]
}

fn skills(g: &Game) -> Field {
    let mut field = vec![Span::raw("Skills ").dark_gray()];
    for (i, s) in g.skills.iter().enumerate() {
        if i > 0 {
            field.push(Span::raw("  "));
        }
        let label = format!("{} {}", i + 1, s.name);
        let lit = label.chars().count() * usize::from(s.ready_pct.min(100)) / 100;
        let (ready, cooling) = label.split_at(label.char_indices().nth(lit).map_or(label.len(), |(at, _)| at));
        field.push(Span::raw(ready.to_owned()));
        field.push(Span::raw(cooling.to_owned()).dark_gray());
    }
    field
}

fn listed(label: &str, items: impl ExactSizeIterator<Item = (String, Option<String>)>, width: usize) -> Field {
    let total = items.len();
    let mut field = vec![Span::raw(format!("{label} ")).dark_gray()];
    for (i, (name, qualifier)) in items.enumerate() {
        let gap = if i == 0 { "" } else { "  " };
        let mut piece = vec![Span::raw(format!("{gap}{name}"))];
        piece.extend(qualifier.map(|q| Span::raw(format!(" {q}")).dark_gray()));
        let rest = total - i - 1;
        let more = if rest == 0 { 0 } else { format!("  +{rest}").len() };
        if spans_width(&field) + spans_width(&piece) + more > width {
            field.push(Span::raw(format!("{gap}+{}", total - i)).dark_gray());
            break;
        }
        field.extend(piece);
    }
    field
}

fn flow(groups: Vec<Vec<Field>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for group in groups.into_iter().filter(|g| !g.is_empty()) {
        let mut line: Field = Vec::new();
        for field in group {
            if !line.is_empty() && spans_width(&line) + 2 + spans_width(&field) > width {
                lines.push(fit(std::mem::take(&mut line), width));
            }
            if !line.is_empty() {
                line.push(Span::raw("  "));
            }
            line.extend(field);
        }
        lines.push(fit(line, width));
    }
    lines
}

fn fit(spans: Field, width: usize) -> Line<'static> {
    Line::from(clip(spans, width))
}

fn clip(spans: Field, width: usize) -> Field {
    if spans_width(&spans) <= width {
        return spans;
    }
    let mut room = width.saturating_sub(1);
    let mut cut = Vec::new();
    for span in spans {
        let mut text = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if w > room {
                room = 0;
                break;
            }
            room -= w;
            text.push(c);
        }
        cut.push(Span::styled(text, span.style));
        if room == 0 {
            break;
        }
    }
    cut.push(Span::raw("…"));
    cut
}

fn spread(mut left: Field, right: Field, width: usize) -> Line<'static> {
    let gap = width.saturating_sub(spans_width(&left) + spans_width(&right)).max(2);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(left)
}

fn hints(keys: &[(&str, &str)]) -> Field {
    let mut spans = Vec::new();
    for (i, (key, word)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(key.to_string()).bold());
        spans.push(Span::raw(format!(" {word}")).dark_gray());
    }
    spans
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn log_lines(app: &App, height: usize, width: usize) -> Vec<Line<'static>> {
    app.log_width.set(width);
    let tagged = app.filter.tagged();
    let lines: Vec<&LogLine> = app.log.iter().filter(|l| app.filter.shows(l)).collect();
    let sizes: Vec<usize> = lines.iter().map(|l| pieces(l, width, tagged).1.len()).collect();
    let total: usize = sizes.iter().sum();
    app.max_scroll.set(total.saturating_sub(height));
    let end = total - app.scroll.min(app.max_scroll.get());
    let start = end.saturating_sub(height);
    let mut rows = Vec::new();
    let mut at = 0;
    for (line, size) in lines.into_iter().zip(sizes) {
        let (first, last) = (at.max(start), (at + size).min(end));
        if first < last {
            rows.extend(log_rows(line, width, tagged).into_iter().skip(first - at).take(last - first));
        }
        at += size;
        if at >= end {
            break;
        }
    }
    rows
}

fn pieces(l: &LogLine, width: usize, tagged: bool) -> (Vec<Span<'static>>, Vec<&str>) {
    let mut head = vec![Span::raw(clock(l.t)).dark_gray(), Span::raw("  ")];
    if tagged {
        let color = match l.source.as_str() {
            "server" => Color::Yellow,
            "script" => Color::Magenta,
            "game" => Color::Blue,
            _ => Color::Cyan,
        };
        head.extend([Span::styled(format!("{:<6}", l.source), Style::new().fg(color)), Span::raw("  ")]);
    }
    let room = width.saturating_sub(spans_width(&head)).max(10);
    let mut pieces: Vec<&str> = l.text.lines().flat_map(|line| wrap(line, room)).collect();
    if pieces.is_empty() {
        pieces.push("");
    }
    (head, pieces)
}

fn log_rows(l: &LogLine, width: usize, tagged: bool) -> Vec<Line<'static>> {
    let (mut head, pieces) = pieces(l, width, tagged);
    let indent = " ".repeat(spans_width(&head));
    let mut pieces = pieces.into_iter();
    head.push(Span::raw(pieces.next().unwrap_or_default().to_string()));
    let mut rows = vec![Line::from(head)];
    rows.extend(pieces.map(|piece| Line::from(vec![Span::raw(indent.clone()), Span::raw(piece.to_string())])));
    rows
}

fn wrap(text: &str, room: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        let mut used = 0;
        let cut = rest.char_indices().find_map(|(at, c)| {
            used += c.width().unwrap_or(0);
            (used > room).then_some(at)
        });
        let Some(cut) = cut.filter(|&cut| cut > 0) else {
            pieces.push(rest);
            return pieces;
        };
        let at = rest[..cut].rfind(' ').filter(|&at| at > 0).unwrap_or(cut);
        pieces.push(&rest[..at]);
        rest = rest[at..].trim_start();
        if rest.is_empty() {
            return pieces;
        }
    }
}

fn draw_keys(f: &mut Frame, area: Rect) {
    f.buffer_mut().set_style(area, Style::new().fg(Color::DarkGray));
    let cell = |(key, word): (&str, &str), width: usize| -> Field {
        if key.is_empty() {
            return vec![Span::raw(format!("{word:<width$}")).bold()];
        }
        vec![Span::raw(format!("{key:<11}")).bold(), Span::raw(format!("{word:<w$}", w = width - 11))]
    };
    let (left, right) = (32, 27);
    let rows = ACCOUNT_KEYS.len().max(SCRIPT_KEYS.len());
    let mut lines: Vec<Line> = (0..rows)
        .map(|i| {
            let mut spans = ACCOUNT_KEYS.get(i).map_or_else(|| vec![Span::raw(" ".repeat(left))], |&k| cell(k, left));
            spans.extend(SCRIPT_KEYS.get(i).map(|&k| cell(k, right)).unwrap_or_default());
            Line::from(spans)
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::raw("Keys act on the marked accounts, or the selected one.").dark_gray());
    let rect = centered(area, (left + right) as u16 + 4, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" keys ").padding(Padding::horizontal(1))), rect);
}

fn draw_ending(f: &mut Frame, area: Rect, accounts: usize) {
    f.buffer_mut().set_style(area, Style::new().fg(Color::DarkGray));
    let (who, them) = if accounts == 1 { ("the account".to_string(), "it") } else { (format!("all {accounts} accounts"), "them") };
    let lines = vec![
        Line::raw(format!("Log out {who} and end the session?")).bold(),
        Line::raw(format!("Detaching leaves {them} running.")).dark_gray(),
        Line::default(),
        Line::from(hints(&[("enter", "end it"), ("q", "detach"), ("esc", "stay")])),
    ];
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let rect = centered(area, widest + 4, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" end session ").padding(Padding::horizontal(1))), rect);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn short(n: i64) -> String {
    match n.abs() {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        10_000.. => format!("{:.1}K", n as f64 / 1_000.0),
        _ => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use aqx_core::{Event, Phase, World, reduce};
    use aqx_protocol::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::hub::AccountCell;
    use crate::hub::{Board, Hub, LOG_LINES, Player, Print, SkillState};

    fn rows(fleet: &Fleet, app: &mut App, width: u16) -> Vec<Vec<ratatui::buffer::Cell>> {
        let shown = gather(fleet, app);
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|f| draw(f, fleet, app, &shown)).unwrap();
        terminal.backend().buffer().content.chunks(width as usize).map(<[_]>::to_vec).collect()
    }

    fn screen(fleet: &Fleet, app: &mut App, width: u16) -> String {
        rows(fleet, app, width).iter().map(|row| row.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn press(fleet: &Fleet, app: &mut App, code: KeyCode) {
        let shown = gather(fleet, app);
        app.handle(KeyEvent::from(code), fleet, &shown);
    }

    fn farm_world() -> World {
        let mut world = World::default();
        let json = |o: &str| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#);
        for text in [
            "%xt%loginResponse%-1%true%1001%hero%Welcome!%".to_string(),
            "<msg t='sys'><body action='joinOK' r='5'><uLs r='5'><u i='1001'><n><![CDATA[hero]]></n></u><u i='1002'><n><![CDATA[ally]]></n></u></uLs></body></msg>".into(),
            json(r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
                "mondef":[{"MonID":10,"strMonName":"Mosquito"}],
                "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"Enter"},{"MonMapID":2,"MonID":10,"strFrame":"r2"}],
                "monBranch":[{"MonMapID":1,"intHP":540,"intHPMax":540,"intState":1},{"MonMapID":2,"intHP":0,"intHPMax":540,"intState":0}],
                "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":2000,"intHPMax":4000,"intMP":100,"intMPMax":100,"intState":1},
                            {"entID":1002,"uoName":"ally","intLevel":40,"strFrame":"Enter","strPad":"Left","intHP":900,"intHPMax":1000,"intState":1}]}"#),
            json(r#"{"cmd":"initUserData","uid":1001,"data":{"intLevel":100,"intGold":12300000,"strClassName":"Barber"}}"#),
            json(r#"{"cmd":"ct","a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Corruption","dur":20,"stk":2}]}]}"#),
        ] {
            reduce(&mut world, &Message::parse(&text).unwrap(), Instant::now());
        }
        world
    }

    fn fleet(accounts: &[&str]) -> (Fleet, Arc<Hub>) {
        let cells: Vec<Arc<AccountCell>> = accounts.iter().map(|name| Arc::new(AccountCell::mirror(name))).collect();
        let hub = Arc::new(Hub::new("aqx", cells.clone(), Print::Quiet));
        (Fleet { title: "aqx".into(), cells: Arc::new(Cells::new(cells)), socket: PathBuf::new() }, hub)
    }

    fn publish(hub: &Hub, cell: &AccountCell, world: &World) {
        let now = Instant::now();
        let mut board: std::sync::MutexGuard<'_, Board> = cell.board();
        board.server = "Twilly".into();
        board.online = true;
        board.ready_since = Some(now - Duration::from_secs(90));
        board.start_script("/srv/void-aura.ts".into(), Vec::new(), now);
        drop(board);
        hub.record(cell, &Event::Status { text: "Astral Ephemerite Essence in timespace".into() });
        hub.record(cell, &Event::Progress { label: "Void Aura".into(), current: 4120, total: 7500 });
        hub.record(cell, &Event::Killed { id: aqx_protocol::MonMapId(1), monster: "Mosquito".into() });
        hub.record(cell, &Event::ServerText { text: "Profanity filter On.".into() });
        let mut board = cell.board();
        board.snapshot = Some(board.snapshot(&cell.name, "hero", &Phase::Ready, world, now));
    }

    fn pointer(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    fn list_width(app: &App) -> Option<u16> {
        app.list_area.get().map(|list| list.width)
    }

    fn show(cell: &AccountCell, snapshot: Snapshot) {
        cell.board().snapshot = Some(snapshot);
    }

    fn row<'a>(screen: &'a str, has: &str) -> &'a str {
        screen.lines().find(|l| l.contains(has)).unwrap_or_else(|| panic!("no row has {has:?}:\n{screen}"))
    }

    #[test]
    fn the_selected_account_shows_its_task_a_summary_and_its_log() {
        let (fleet, hub) = fleet(&["main"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let mut app = App::default();
        let shown = screen(&fleet, &mut app, 110);
        println!("{shown}");
        assert!(row(&shown, "main (hero) · Twilly · farm-1 Enter/Spawn").contains("⠋ running  up 1m"));
        for expected in [
            "void-aura.ts  Astral Ephemerite Essence in timespace",
            "4120/7500",
            "HP ████░░░░ 2000/4000  1 kill",
            "server  Profanity filter On.",
            "q detach",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
        assert!(!shown.contains("Barber"), "details stay hidden until d");
        assert!(!shown.contains("killed Mosquito"), "game lines stay out of the main log");
        let narrow = screen(&fleet, &mut app, 60);
        for expected in ["HP ████░░░░ 2000/4000", "Void Aura", "Profanity filter", "q detach"] {
            assert!(narrow.contains(expected), "narrow screen is missing {expected:?}");
        }
        press(&fleet, &mut app, KeyCode::Char('l'));
        let shown = screen(&fleet, &mut app, 110);
        assert!(shown.contains("── log · all") && shown.contains("game    killed Mosquito"), "all brings the game lines back");
        for _ in 0..3 {
            press(&fleet, &mut app, KeyCode::Char('l'));
        }
        let shown = screen(&fleet, &mut app, 110);
        assert!(shown.contains("── log · server") && shown.contains("Profanity filter On.") && !shown.contains("killed Mosquito"));
        assert!(!shown.contains("server  Profanity"), "a filtered log leaves out the source column:\n{shown}");
    }

    #[test]
    fn d_shows_every_detail() {
        let (fleet, hub) = fleet(&["main"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('d'));
        let shown = screen(&fleet, &mut app, 110);
        println!("{shown}");
        for expected in [
            "MP ████████ 100/100  Lv 100 Barber  12.3M gold",
            "Auras Corruption x2",
            "Here ally 40",
            "Stats 1 kill  0 drops  0 deaths",
            "d less",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn the_list_shows_each_account_and_who_keys_act_on() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let mut app = App::default();
        let shown = screen(&fleet, &mut app, 120);
        println!("{shown}");
        assert!(row(&shown, "1 of 2 online").contains("? keys  q detach"));
        assert!(row(&shown, "▌ ⠋ main").contains("void-aura.ts"));
        assert!(row(&shown, "⠋ healer").contains("starting"));
        assert!(row(&shown, " → ").contains("→ main"));
        press(&fleet, &mut app, KeyCode::Char(' '));
        press(&fleet, &mut app, KeyCode::Down);
        press(&fleet, &mut app, KeyCode::Char(' '));
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains("2 marked") && shown.contains("→ main, healer"), "{shown}");
        press(&fleet, &mut app, KeyCode::Char('a'));
        assert!(screen(&fleet, &mut app, 120).contains("→ healer"), "a with everything marked clears the marks");
    }

    #[test]
    fn the_list_says_what_each_account_is_doing_and_puts_offline_ones_last() {
        let (fleet, hub) = fleet(&["spare", "main", "healer", "tank", "idle"]);
        publish(&hub, &fleet.cells.all()[1], &farm_world());
        show(&fleet.cells.all()[0], Snapshot { phase: "OFFLINE".into(), ..Snapshot::default() });
        show(&fleet.cells.all()[3], Snapshot { phase: "READY".into(), script: Some("farm.ts".into()), run: "running".into(), paused: true, ..Snapshot::default() });
        show(&fleet.cells.all()[4], Snapshot { phase: "READY".into(), run: "idle".into(), ..Snapshot::default() });
        let mut app = App::default();
        let list = |app: &mut App| -> Vec<String> {
            screen(&fleet, app, 120).lines().skip(1).take(6).map(|l| l.split('│').next().unwrap().trim_end().to_string()).collect()
        };
        assert_eq!(
            list(&mut app),
            [
                "  ⠋ main   void-aura.ts",
                "  ⠋ healer starting",
                "  ‖ tank   farm.ts, paus…",
                "  ● idle   idle",
                "── offline 1 ────────────",
                "▌ ○ spare  logged out",
            ]
        );
        app.frame = 1;
        assert!(list(&mut app)[0].starts_with("  ⠙ main"), "the spinner moves each frame");
        press(&fleet, &mut app, KeyCode::Up);
        assert_eq!(app.selected, 4, "up from the offline group moves to the last online account");
        press(&fleet, &mut app, KeyCode::Tab);
        press(&fleet, &mut app, KeyCode::Tab);
        assert_eq!(app.selected, 1, "tab follows the list and wraps");
        press(&fleet, &mut app, KeyCode::Down);
        press(&fleet, &mut app, KeyCode::Char('}'));
        assert_eq!(app.selected, 0, "}} jumps to the offline group");
        press(&fleet, &mut app, KeyCode::Char('}'));
        assert_eq!(app.selected, 1, "and wraps to the first online account");
        press(&fleet, &mut app, KeyCode::Char('{'));
        assert_eq!(app.selected, 0);

        let shown = gather(&fleet, &mut app);
        let click = |app: &mut App, column, row| app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), column, row), &shown);
        click(&mut app, 6, 3);
        assert_eq!(app.selected, 3, "clicking a row picks its account");
        click(&mut app, 6, 5);
        assert_eq!(app.selected, 0, "clicking the offline heading picks the first offline account");
        click(&mut app, 2, 2);
        click(&mut app, 6, 20);
        click(&mut app, 60, 2);
        assert_eq!(app.selected, 2, "clicks below the list or in the pane leave the pick alone");
    }

    #[test]
    fn the_list_hides_and_resizes_and_names_outlast_the_status() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let mut app = App::default();
        screen(&fleet, &mut app, 120);
        let edge = list_width(&app).unwrap();

        for _ in 0..3 {
            press(&fleet, &mut app, KeyCode::Char(']'));
            screen(&fleet, &mut app, 120);
        }
        assert_eq!(list_width(&app), Some(edge + 3 * LIST_STEP));

        for _ in 0..20 {
            press(&fleet, &mut app, KeyCode::Char('['));
            screen(&fleet, &mut app, 120);
        }
        assert_eq!(list_width(&app), Some(LIST_MIN));
        let narrow = screen(&fleet, &mut app, 120);
        assert!(row(&narrow, "▌ ⠋ main").starts_with("▌ ⠋ main │"), "{narrow}");
        assert!(row(&narrow, "heal").starts_with("  ⠋ heal…│"), "{narrow}");

        app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), LIST_MIN - 1, 5), &[]);
        app.mouse(pointer(MouseEventKind::Drag(MouseButton::Left), 49, 5), &[]);
        app.mouse(pointer(MouseEventKind::Up(MouseButton::Left), 49, 5), &[]);
        screen(&fleet, &mut app, 120);
        assert_eq!(list_width(&app), Some(50), "dragging the border resizes the list");
        app.mouse(pointer(MouseEventKind::Drag(MouseButton::Left), 30, 5), &[]);
        screen(&fleet, &mut app, 120);
        assert_eq!(list_width(&app), Some(50), "a drag that didn't start on the border does nothing");
        app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), 50, 5), &[]);
        app.mouse(pointer(MouseEventKind::Drag(MouseButton::Left), 119, 5), &[]);
        screen(&fleet, &mut app, 120);
        assert_eq!(list_width(&app), Some(120 - PANE_MIN), "the pane keeps room for itself");

        press(&fleet, &mut app, KeyCode::Char('b'));
        let hidden = screen(&fleet, &mut app, 120);
        assert!(row(&hidden, "main 1/2").contains("tab next") && !hidden.contains("healer"), "{hidden}");
        press(&fleet, &mut app, KeyCode::Char('b'));
        assert!(screen(&fleet, &mut app, 120).contains("healer"));
    }

    #[test]
    fn g_shows_every_account_as_a_card_and_arrows_move_across_the_grid() {
        let (fleet, hub) = fleet(&["main", "healer", "spare", "alt"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let detail = "this account logged in somewhere else";
        show(&fleet.cells.all()[2], Snapshot { phase: "OFFLINE".into(), detail: detail.into(), ..Snapshot::default() });
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('g'));
        let shown = screen(&fleet, &mut app, 140);
        println!("{shown}");
        assert!(row(&shown, "1 of 4 online").contains("g dashboard"));
        let top = row(&shown, "┏ main");
        assert!(top.contains("⠋ running") && top.contains("┌ healer") && top.contains("⠋ starting") && top.contains("┌ alt"), "three cards fit across 140 columns: {shown}");
        assert!(row(&shown, "spare").contains("✗ offline"), "offline accounts come last: {shown}");
        for expected in ["hero · Twilly · up 1m", "farm-1 Enter/Spawn", "void-aura.ts  Astral", "HP ████░░░░ 2000/4000  1 kill", "Void Aura ", "waiting for the server", "this account logged in", "enter open"] {
            assert!(shown.contains(expected), "missing {expected:?}: {shown}");
        }
        assert!(!shown.contains("── log"));
        for key in ['d', 'l'] {
            press(&fleet, &mut app, KeyCode::Char(key));
        }
        press(&fleet, &mut app, KeyCode::PageUp);
        assert!(!app.details && app.filter == Filter::Main && app.scroll == 0, "view keys for the hidden pane do nothing on the dashboard");
        press(&fleet, &mut app, KeyCode::Down);
        assert_eq!(app.selected, 2, "down moves one row of three cards, to spare");
        press(&fleet, &mut app, KeyCode::Left);
        press(&fleet, &mut app, KeyCode::Up);
        assert_eq!(app.selected, 0);
        press(&fleet, &mut app, KeyCode::Right);
        press(&fleet, &mut app, KeyCode::Enter);
        assert!(screen(&fleet, &mut app, 140).contains("── log"), "enter opens the picked account");
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn slash_filters_the_accounts_by_name_and_tab_completes_the_name() {
        let (fleet, _hub) = fleet(&["main", "healer", "farmer1", "farmer2", "Healbot"]);
        let mut app = App::default();
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| press(&fleet, app, KeyCode::Char(c)));
        let list = |app: &mut App| -> Vec<String> {
            screen(&fleet, app, 120).lines().skip(1).take(5).map(|l| l.split('│').next().unwrap().trim().to_string()).collect()
        };

        press(&fleet, &mut app, KeyCode::Char('/'));
        typed(&mut app, "ea");
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains(" filter accounts: ea█") && shown.contains(" healer  Healbot"), "{shown}");
        assert_eq!(list(&mut app), ["▌ ⠋ healer  starting", "⠋ Healbot starting", "", "", ""]);
        assert_eq!(app.selected, 1, "the pick moves to the first match");
        press(&fleet, &mut app, KeyCode::Tab);
        assert_eq!(app.query, "heal", "tab completes as far as every match agrees");
        typed(&mut app, "e");
        press(&fleet, &mut app, KeyCode::Tab);
        assert_eq!(app.query, "healer", "and the whole name once one is left");
        press(&fleet, &mut app, KeyCode::Esc);
        assert!(!app.querying && app.query.is_empty() && list(&mut app)[4].contains("Healbot"), "esc clears the filter");

        press(&fleet, &mut app, KeyCode::Char('/'));
        typed(&mut app, "FARM");
        press(&fleet, &mut app, KeyCode::Enter);
        let shown = screen(&fleet, &mut app, 120);
        assert!(row(&shown, "0 of 5 online").contains("· filter \"FARM\"") && !shown.contains("filter accounts"), "enter keeps the filter: {shown}");
        press(&fleet, &mut app, KeyCode::Char('a'));
        assert_eq!(app.marked, BTreeSet::from([2, 3]), "a marks only the accounts the filter shows");
        press(&fleet, &mut app, KeyCode::Tab);
        press(&fleet, &mut app, KeyCode::Tab);
        assert_eq!(app.selected, 2, "tab wraps within the filter");
        press(&fleet, &mut app, KeyCode::Char('a'));
        assert!(app.marked.is_empty());
    }

    #[test]
    fn the_grid_follows_the_selection_past_the_bottom() {
        let names: Vec<String> = (0..30).map(|i| format!("alt{i:02}")).collect();
        let (fleet, _hub) = fleet(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut app = App { grid: true, ..App::default() };
        screen(&fleet, &mut app, 100);
        for _ in 0..8 {
            press(&fleet, &mut app, KeyCode::Down);
        }
        let shown = screen(&fleet, &mut app, 100);
        assert!(row(&shown, "┏ alt16").contains("⠋ starting"), "{shown}");
        assert!(!shown.contains("alt00"));
    }

    #[test]
    fn a_narrow_terminal_hides_the_list_and_tab_switches_accounts() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let mut app = App::default();
        let shown = screen(&fleet, &mut app, 80);
        assert!(row(&shown, "main 1/2").contains("tab next"), "{shown}");
        assert!(!shown.contains("healer"));
        press(&fleet, &mut app, KeyCode::Tab);
        assert!(screen(&fleet, &mut app, 80).contains("healer 2/2"));
    }

    #[test]
    fn an_offline_account_shows_why_and_offers_to_log_in() {
        let (fleet, _hub) = fleet(&["spare"]);
        let detail = "this account logged in somewhere else";
        show(&fleet.cells.all()[0], Snapshot { account: "spare".into(), phase: "OFFLINE".into(), detail: detail.into(), ..Snapshot::default() });
        let shown = screen(&fleet, &mut App::default(), 100);
        assert!(row(&shown, "spare").contains("✗ offline"));
        assert!(shown.contains(detail) && shown.contains("i log in"), "{shown}");
    }

    #[test]
    fn a_crowded_cell_fits_one_line_and_counts_the_rest() {
        let (fleet, _hub) = fleet(&["main"]);
        let players = (0..11).map(|i| Player { name: format!("player{i}"), level: 40 + i }).collect();
        let game = Game { cell_players: players, ..Game::default() };
        show(&fleet.cells.all()[0], Snapshot { phase: "READY".into(), game: Some(game), ..Snapshot::default() });
        let mut app = App { details: true, ..App::default() };
        let shown = screen(&fleet, &mut app, 60);
        let here = row(&shown, "Here").trim_end();
        let named = here.matches("player").count();
        let rest: usize = here.rsplit('+').next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no count: {here}"));
        assert_eq!(named + rest, 11, "{here}");
        assert!(here.chars().count() <= 60);
    }

    #[test]
    fn scrolling_past_the_oldest_line_keeps_the_log_on_screen() {
        let (fleet, hub) = fleet(&["main"]);
        for i in 0..5 {
            hub.say(&fleet.cells.all()[0], "aqx", format!("line {i}"));
        }
        let mut app = App { scroll: 500, ..App::default() };
        let shown = screen(&fleet, &mut app, 100);
        assert!(shown.contains("line 0") && shown.contains("line 4"));
    }

    #[test]
    fn a_scrolled_log_holds_its_place_as_lines_arrive() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        for i in 0..50 {
            hub.say(cell, "aqx", format!("line {i:02}"));
        }
        let mut app = App::default();
        screen(&fleet, &mut app, 100);
        press(&fleet, &mut app, KeyCode::PageUp);
        let before = screen(&fleet, &mut app, 100);
        hub.say(cell, "aqx", "fresh");
        hub.say(cell, "script", format!("long {}", "word ".repeat(40)));
        let after = screen(&fleet, &mut app, 100);
        let log = |shown: &str| shown.lines().filter(|l| l.contains("line ")).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(log(&before), log(&after), "{after}");
        assert!(before.contains(" jump to bottom (end) ↓ ") && !before.contains("new ·"), "{before}");
        assert!(after.contains(" 2 new · jump to bottom (end) ↓ ") && !after.contains("fresh"), "{after}");

        press(&fleet, &mut app, KeyCode::End);
        let tail = screen(&fleet, &mut app, 100);
        assert!(tail.contains("fresh") && !tail.contains("jump to bottom"), "{tail}");
    }

    #[test]
    fn clicking_the_jump_pill_returns_to_the_newest_lines() {
        let (fleet, hub) = fleet(&["main"]);
        for i in 0..50 {
            hub.say(&fleet.cells.all()[0], "aqx", format!("line {i:02}"));
        }
        let mut app = App::default();
        screen(&fleet, &mut app, 100);
        press(&fleet, &mut app, KeyCode::PageUp);
        let shown = screen(&fleet, &mut app, 100);
        let (row, line) = shown.lines().enumerate().find(|(_, l)| l.contains("jump to bottom")).unwrap_or_else(|| panic!("no pill:\n{shown}"));
        let column = line.chars().position(|c| c == 'j').unwrap() as u16;
        let shown = gather(&fleet, &mut app);
        app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), column, row as u16), &shown);
        assert_eq!(app.scroll, 0);
        assert!(screen(&fleet, &mut app, 100).contains("line 49"));
    }

    #[test]
    fn long_log_lines_wrap_under_the_message() {
        let (fleet, hub) = fleet(&["main"]);
        let long = format!("error: {}the end", "word ".repeat(30));
        hub.say(&fleet.cells.all()[0], "script", format!("{long}\n    at main (farm.ts:2:5)"));
        let shown = screen(&fleet, &mut App::default(), 80);
        println!("{shown}");
        let rows: Vec<&str> = shown.lines().filter(|l| l.contains("word") || l.contains("at main")).collect();
        assert!(rows[0].contains("script  error: word"), "{shown}");
        let indent = " ".repeat(1 + clock(0).len() + 10);
        assert!(rows.len() >= 3 && rows[1..].iter().all(|r| r.starts_with(&indent)), "{shown}");
        assert!(shown.contains("the end") && shown.contains("    at main (farm.ts:2:5)"), "{shown}");
    }

    #[test]
    fn a_long_running_session_keeps_every_character_of_a_wrapped_line() {
        let ten_hours = 36_000_000;
        let line = LogLine { seq: 0, t: ten_hours, source: "script".into(), text: format!("error: {}the end", "word ".repeat(30)) };
        let rows: Vec<Line> = log_rows(&line, 60, true);
        let text: Vec<String> = rows.iter().map(Line::to_string).collect();
        assert!(text[0].starts_with("10:00:00.000  script  error: word"), "{text:#?}");
        assert!(text[1..].iter().all(|r| r.starts_with(&" ".repeat(22)) && !r.starts_with(&" ".repeat(23))), "{text:#?}");
        assert!(rows.iter().all(|r| r.width() <= 60), "{text:#?}");
        assert_eq!(text.join(" ").matches("word").count(), 30);
        assert!(text.last().unwrap().ends_with("the end"));
    }

    #[test]
    fn wide_characters_wrap_by_display_width() {
        let line = LogLine { seq: 0, t: 0, source: "server".into(), text: "漢".repeat(50) };
        let rows = log_rows(&line, 60, true);
        assert!(rows.len() > 1 && rows.iter().all(|r| r.width() <= 60));
        assert_eq!(rows.iter().map(|r| r.to_string().matches('漢').count()).sum::<usize>(), 50);
    }

    #[test]
    fn the_list_follows_the_selection_past_the_bottom() {
        let names: Vec<String> = (0..30).map(|i| format!("alt{i:02}")).collect();
        let (fleet, _hub) = fleet(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut app = App::default();
        for _ in 0..25 {
            press(&fleet, &mut app, KeyCode::Down);
        }
        let shown = screen(&fleet, &mut app, 100);
        assert!(row(&shown, "▌ ⠋ alt25").contains("starting"), "{shown}");
        assert!(!shown.contains("alt00"));
    }

    #[test]
    fn many_targets_collapse_to_a_count() {
        let names: Vec<String> = (0..12).map(|i| format!("account{i:03}")).collect();
        let (fleet, _hub) = fleet(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('a'));
        assert!(screen(&fleet, &mut app, 100).contains("→ 12 accounts   o run"));
        press(&fleet, &mut app, KeyCode::Char('o'));
        assert!(screen(&fleet, &mut app, 100).contains(" run on 12 accounts: █"));
    }

    #[test]
    fn the_state_stays_on_screen_when_the_location_is_long() {
        let (fleet, _hub) = fleet(&["main"]);
        let game = Game { area: "battleon-123456789012345678901234567890".into(), cell: "Enter".into(), pad: "Spawn".into(), ..Game::default() };
        show(&fleet.cells.all()[0], Snapshot { phase: "READY".into(), server: "Twilight".into(), up_secs: 3720, game: Some(game), ..Snapshot::default() });
        let shown = screen(&fleet, &mut App::default(), 60);
        assert!(row(&shown, "main").contains("…  ● online  up 1h02m"), "{shown}");
    }

    #[test]
    fn arrows_scroll_the_log_when_there_is_nothing_to_pick() {
        let (one, hub) = fleet(&["main"]);
        for i in 0..50 {
            hub.say(&one.cells.all()[0], "aqx", format!("line {i}"));
        }
        let mut app = App::default();
        screen(&one, &mut app, 100);
        for _ in 0..3 {
            press(&one, &mut app, KeyCode::Up);
        }
        assert_eq!(app.scroll, 3);
        let (two, _hub) = fleet(&["main", "healer"]);
        let mut app = App { max_scroll: Cell::new(10), ..App::default() };
        let shown = gather(&two, &mut app);
        app.handle(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT), &two, &shown);
        assert_eq!((app.scroll, app.selected), (1, 0));
    }

    #[test]
    fn the_wheel_scrolls_the_log_and_never_changes_the_account() {
        let (two, hub) = fleet(&["main", "healer"]);
        for i in 0..50 {
            hub.say(&two.cells.all()[0], "aqx", format!("line {i}"));
        }
        let mut app = App::default();
        let bottom = screen(&two, &mut app, 100);
        app.mouse(pointer(MouseEventKind::ScrollUp, 50, 5), &[]);
        app.mouse(pointer(MouseEventKind::ScrollUp, 50, 5), &[]);
        let up = screen(&two, &mut app, 100);
        assert!(bottom.contains("line 49") && !up.contains("line 49") && up.contains("line 43"), "{up}");
        assert_eq!(app.selected, 0);
        app.mouse(pointer(MouseEventKind::ScrollDown, 50, 5), &[]);
        app.mouse(pointer(MouseEventKind::ScrollDown, 50, 5), &[]);
        assert_eq!(screen(&two, &mut app, 100), bottom);
        app.grid = true;
        app.mouse(pointer(MouseEventKind::ScrollUp, 50, 5), &[]);
        app.grid = false;
        assert_eq!(screen(&two, &mut app, 100), bottom, "the wheel does nothing on the dashboard");
    }

    #[test]
    fn the_run_prompt_expands_home_and_refuses_a_missing_file() {
        let (fleet, _hub) = fleet(&["main"]);
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('o'));
        for c in "~/aqx-no-such-script.ts".chars() {
            press(&fleet, &mut app, KeyCode::Char(c));
        }
        press(&fleet, &mut app, KeyCode::Enter);
        let home = std::env::var("HOME").unwrap();
        let note = app.note.map(|(text, _)| text).unwrap_or_default();
        assert_eq!(note, format!("{home}/aqx-no-such-script.ts isn't a file"));
    }

    #[test]
    fn arrows_in_the_run_prompt_walk_back_through_what_was_typed() {
        let dir = std::env::temp_dir().join(format!("aqx-tui-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (fleet, _hub) = fleet(&["main"]);
        show(&fleet.cells.all()[0], Snapshot { script: Some("/srv/farm.ts".into()), ..Snapshot::default() });
        let mut app = App::default();
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| press(&fleet, app, KeyCode::Char(c)));
        let [a, b] = ["a.ts", "b.ts"].map(|name| {
            std::fs::write(dir.join(name), "").unwrap();
            format!("{} rounds=3", dir.join(name).display())
        });
        for command in [&a, &b] {
            press(&fleet, &mut app, KeyCode::Char('o'));
            typed(&mut app, command);
            press(&fleet, &mut app, KeyCode::Enter);
        }
        press(&fleet, &mut app, KeyCode::Char('o'));
        typed(&mut app, "dra");
        let walk = |app: &mut App, codes: &[KeyCode]| {
            codes
                .iter()
                .map(|&code| {
                    press(&fleet, app, code);
                    app.prompt.as_ref().unwrap().input.clone()
                })
                .collect::<Vec<_>>()
        };
        let (up, down) = (KeyCode::Up, KeyCode::Down);
        let (a, b, farm) = (a.as_str(), b.as_str(), "/srv/farm.ts");
        assert_eq!(walk(&mut app, &[up, up, up, up]), [b, a, farm, farm], "the running script sits behind what was typed");
        assert_eq!(walk(&mut app, &[down, down, down, down]), [a, b, "dra", "dra"], "down past the newest gives back the draft");
        walk(&mut app, &[up]);
        typed(&mut app, "!");
        assert_eq!(walk(&mut app, &[up, down]), [b.to_string(), format!("{b}!")], "an edited line becomes the draft");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn running_a_script_asks_each_account_for_the_args_it_was_not_given() {
        let dir = std::env::temp_dir().join(format!("aqx-tui-ask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("follow.ts");
        std::fs::write(&script, r#"import { script } from "aqx"; script.args({ target: String, rounds: Number, class: "", wait: 5 });"#).unwrap();
        let controls: Vec<aqx_core::Control> = (0..2).map(|_| aqx_core::Control::new().unwrap()).collect();
        let cells: Vec<Arc<AccountCell>> = ["main", "alt"].iter().zip(&controls).map(|(name, c)| Arc::new(AccountCell::local(name, c.stop_handle()))).collect();
        for cell in &cells {
            cell.board().online = true;
        }
        let name = format!("tui-ask-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, cells.clone(), Print::Quiet));
        let (_, listener, _listening) = socket::bind(&name, false).unwrap();
        socket::serve(listener, hub, None).unwrap();
        let fleet = Fleet { title: name.clone(), cells: Arc::new(Cells::new(cells.clone())), socket: socket::pick(Some(&name)).unwrap().1 };
        let mut app = App::default();
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| press(&fleet, app, KeyCode::Char(c)));
        let command = format!("{} rounds=3", script.display());
        let ran = |cell: &AccountCell| match cell.take_command() {
            Some(Command::Run { script: ran, args }) if ran == script => args.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "),
            other => panic!("{other:?}"),
        };
        let run = |app: &mut App| {
            press(&fleet, app, KeyCode::Char('o'));
            typed(app, &command);
            press(&fleet, app, KeyCode::Enter);
        };

        run(&mut app);
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains(" follow.ts needs target: █") && shown.contains("enter next") && !shown.contains("tab"), "one account isn't named: {shown}");
        press(&fleet, &mut app, KeyCode::Enter);
        press(&fleet, &mut app, KeyCode::Esc);
        assert!(app.ask.is_none() && cells[0].take_command().is_none(), "an empty answer and esc run nothing");

        press(&fleet, &mut app, KeyCode::Char('a'));
        run(&mut app);
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains(" follow.ts needs target for main: █") && shown.contains("tab same for all"), "{shown}");
        typed(&mut app, "Bob Smith");
        press(&fleet, &mut app, KeyCode::Enter);
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains(" follow.ts class for main: █optional") && !shown.contains("tab"), "{shown}");
        press(&fleet, &mut app, KeyCode::Enter);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts wait for main: █5"));
        typed(&mut app, "8");
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts wait for main: 8█ "), "typing hides the default");
        press(&fleet, &mut app, KeyCode::Enter);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target for alt: █"));
        press(&fleet, &mut app, KeyCode::Enter);
        assert!(screen(&fleet, &mut app, 120).contains("enter next"));
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target for alt: █"), "a required arg needs a value");
        for answer in ["Ann", "Mage", ""] {
            typed(&mut app, answer);
            press(&fleet, &mut app, KeyCode::Enter);
        }
        assert_eq!((ran(&cells[0]), ran(&cells[1])), ("rounds=3 target=Bob Smith wait=8".into(), "rounds=3 target=Ann class=Mage".into()));
        assert_eq!(app.recent.last().map(|r| &r.typed), Some(&command), "per-account answers stay out of the history");

        run(&mut app);
        press(&fleet, &mut app, KeyCode::Tab);
        let shown = screen(&fleet, &mut app, 120);
        assert!(shown.contains(" follow.ts needs target: █") && shown.contains("tab ask each account"), "{shown}");
        for answer in ["Bob", "", ""] {
            typed(&mut app, answer);
            press(&fleet, &mut app, KeyCode::Enter);
        }
        assert_eq!((ran(&cells[0]), ran(&cells[1])), ("rounds=3 target=Bob".into(), "rounds=3 target=Bob".into()));
        assert_eq!(app.recent.last().map(|r| &r.typed), Some(&format!("{command} target=Bob")));
        run(&mut app);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target: █"), "the choice sticks");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_account_shows_starting_before_the_server_answers() {
        let (fleet, _hub) = fleet(&["main"]);
        let shown = screen(&fleet, &mut App::default(), 100);
        for expected in ["⠋ starting", "waiting for the server", "── log · main"] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn question_mark_opens_the_key_list_and_any_key_closes_it() {
        let (fleet, _hub) = fleet(&["main"]);
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('?'));
        assert!(screen(&fleet, &mut app, 100).contains("filter the log"));
        press(&fleet, &mut app, KeyCode::Char('x'));
        assert!(!screen(&fleet, &mut app, 100).contains("filter the log"));
        assert!(app.note.is_none(), "closing the list doesn't also log out");
    }

    #[test]
    fn enter_in_the_end_dialog_logs_every_account_out() {
        let controls: Vec<aqx_core::Control> = (0..2).map(|_| aqx_core::Control::new().unwrap()).collect();
        let cells: Vec<Arc<AccountCell>> = ["main", "alt"].iter().zip(&controls).map(|(name, c)| Arc::new(AccountCell::local(name, c.stop_handle()))).collect();
        for cell in &cells {
            cell.board().online = true;
        }
        let name = format!("tui-end-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, cells.clone(), Print::Quiet));
        let (_, listener, _listening) = socket::bind(&name, false).unwrap();
        socket::serve(listener, hub, None).unwrap();
        let fleet = Fleet { title: name.clone(), cells: Arc::new(Cells::new(cells.clone())), socket: socket::pick(Some(&name)).unwrap().1 };
        let mut app = App::default();
        let logouts = || cells.iter().filter(|c| matches!(c.take_command(), Some(Command::Logout))).count();

        press(&fleet, &mut app, KeyCode::Char('Q'));
        assert!(screen(&fleet, &mut app, 100).contains("Log out all 2 accounts and end the session?"));
        press(&fleet, &mut app, KeyCode::Char('x'));
        assert!(screen(&fleet, &mut app, 100).contains("end the session?"), "other keys leave the dialog open");
        assert_eq!(logouts(), 0, "and don't act, even x");

        press(&fleet, &mut app, KeyCode::Enter);
        assert_eq!(logouts(), 2);
        assert!(!screen(&fleet, &mut app, 100).contains("end the session?"));
    }

    #[test]
    fn ctrl_c_and_shift_q_ask_whether_to_end_the_session_or_detach() {
        let (fleet, _hub) = fleet(&["main", "alt"]);
        let mut app = App::default();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let key = |app: &mut App, key: KeyEvent| {
            let shown = gather(&fleet, app);
            app.handle(key, &fleet, &shown)
        };

        assert!(!key(&mut app, ctrl_c), "the first ctrl+c only asks");
        let asked = screen(&fleet, &mut app, 100);
        assert!(asked.contains("Log out all 2 accounts and end the session?") && asked.contains("enter end it  q detach  esc stay"), "{asked}");
        assert!(!key(&mut app, KeyEvent::from(KeyCode::Esc)), "esc stays");
        assert!(!screen(&fleet, &mut app, 100).contains("end the session?"));

        key(&mut app, KeyEvent::from(KeyCode::Char('Q')));
        assert!(key(&mut app, KeyEvent::from(KeyCode::Char('q'))), "q detaches");

        let mut app = App::default();
        key(&mut app, ctrl_c);
        assert!(key(&mut app, ctrl_c), "a second ctrl+c detaches");
    }

    #[test]
    fn the_log_copies_only_new_lines_and_keeps_a_ring() {
        let (fleet, hub) = fleet(&["main"]);
        let mut app = App::default();
        hub.say(&fleet.cells.all()[0], "aqx", "first");
        gather(&fleet, &mut app);
        for i in 0..LOG_LINES {
            hub.say(&fleet.cells.all()[0], "aqx", format!("line {i}"));
        }
        gather(&fleet, &mut app);
        assert_eq!(app.log.len(), LOG_LINES);
        assert_eq!(app.log.last().map(|l| l.text.as_str()), Some(format!("line {}", LOG_LINES - 1).as_str()));
        assert_eq!(app.log.iter().next().map(|l| l.seq), Some(1));
    }

    #[test]
    fn game_lines_never_push_script_lines_out_of_the_log() {
        let (fleet, hub) = fleet(&["main"]);
        let mut app = App::default();
        hub.say(&fleet.cells.all()[0], "script", "farming");
        for i in 0..LOG_LINES * 2 {
            hub.say(&fleet.cells.all()[0], "game", format!("killed {i}"));
        }
        gather(&fleet, &mut app);
        let shown: Vec<&str> = app.log.iter().filter(|l| app.filter.shows(l)).map(|l| l.text.as_str()).collect();
        assert_eq!(shown, ["farming"], "the main filter hides game lines");
        assert_eq!(app.log.iter().filter(|l| l.is_game()).count(), LOG_LINES);
        assert_eq!(app.log.iter().rfind(|l| l.is_game()).map(|l| l.text.clone()), Some(format!("killed {}", LOG_LINES * 2 - 1)));
    }

    #[test]
    fn a_closed_account_shows_its_phase_before_pause_or_stall() {
        let snapshot = |phase: &str| Snapshot { phase: phase.into(), paused: true, stalled: true, ..Snapshot::default() };
        assert_eq!(snapshot("READY").state(), "PAUSED");
        assert_eq!(snapshot("RECONNECTING").state(), "RECONNECTING");
        assert_eq!(snapshot("OFFLINE").state(), "OFFLINE");
        assert_eq!(Snapshot { phase: "READY".into(), stalled: true, ..Snapshot::default() }.state(), "STALLED");
    }

    #[test]
    fn the_header_names_the_user_only_when_it_differs_from_the_account() {
        let (fleet, hub) = fleet(&["hero"]);
        publish(&hub, &fleet.cells.all()[0], &farm_world());
        let shown = screen(&fleet, &mut App::default(), 110);
        assert!(shown.contains(" hero · Twilly · farm-1"), "{shown}");
    }

    #[test]
    fn tab_completes_script_paths_and_lists_the_matches() {
        let dir = std::env::temp_dir().join(format!("aqx-tui-tests-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("quests")).unwrap();
        for file in ["farm.ts", "fight.js", "loop.mjs", "aqx.d.ts", "my farm.ts", "notes.md", ".secret.ts", "quests/daily.ts"] {
            std::fs::write(dir.join(file), "").unwrap();
        }
        let root = format!("{}/", dir.display());
        let (fleet, _hub) = fleet(&["main"]);
        let mut app = App::default();
        let press = |app: &mut App, code: KeyCode| app.handle(KeyEvent::from(code), &fleet, &[]);
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| _ = press(app, KeyCode::Char(c)));
        let input = |app: &App| app.prompt.as_ref().unwrap().input.clone();

        press(&mut app, KeyCode::Char('o'));
        typed(&mut app, &format!(" {root}"));
        let shown = screen(&fleet, &mut app, 100);
        assert!(shown.contains(" farm.ts  fight.js  loop.mjs  quests/"), "{shown}");
        assert!(shown.contains("tab complete"), "{shown}");
        for hidden in ["aqx.d.ts", "my farm.ts", "notes.md", ".secret.ts"] {
            assert!(!shown.contains(hidden), "{hidden} is listed: {shown}");
        }
        let narrow = screen(&fleet, &mut app, 20);
        assert!(narrow.contains(" farm.ts  +3 more"), "{narrow}");

        typed(&mut app, "f");
        press(&mut app, KeyCode::Tab);
        assert_eq!(input(&app), format!("{root}f"));
        typed(&mut app, "a");
        press(&mut app, KeyCode::Tab);
        assert_eq!(input(&app), format!("{root}farm.ts "));
        assert!(!screen(&fleet, &mut app, 100).contains("fight.js"));

        app.prompt = Some(Prompt::new());
        typed(&mut app, &format!("{root}q"));
        press(&mut app, KeyCode::Tab);
        assert_eq!(input(&app), format!("{root}quests/"));
        typed(&mut app, "..");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(input(&app), format!("{root}quests/../"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tab_completes_under_the_home_directory() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let name = format!("aqx-tui-home-test-{}", std::process::id());
        std::fs::create_dir_all(home.join(&name)).unwrap();
        std::fs::write(home.join(&name).join("farm.ts"), "").unwrap();
        let matches = scripts_matching(&format!("~/{name}/f"));
        std::fs::remove_dir_all(home.join(&name)).unwrap();
        assert_eq!(matches, [format!("~/{name}/farm.ts")]);
    }

    #[test]
    fn the_last_match_takes_no_gap_after_it() {
        let matches = ["farm.ts".to_string(), "fight.js".to_string()];
        let mut terminal = Terminal::new(TestBackend::new(18, 1)).unwrap();
        terminal.draw(|f| draw_matches(f, f.area(), &matches)).unwrap();
        let row: String = terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect();
        assert_eq!(row, " farm.ts  fight.js");
    }

    #[test]
    fn a_recharging_skill_lights_up_from_the_left() {
        let (fleet, _hub) = fleet(&["main"]);
        let skill = |name: &str, ready_pct| SkillState { name: name.into(), ready_pct };
        let game = Game { skills: vec![skill("Slash", 100), skill("Blur", 50), skill("Rend", 0)], ..Game::default() };
        show(&fleet.cells.all()[0], Snapshot { phase: "READY".into(), game: Some(game), ..Snapshot::default() });
        let mut app = App { details: true, ..App::default() };
        let rows = rows(&fleet, &mut app, 60);
        let row = rows.iter().find(|r| r.iter().map(|c| c.symbol()).collect::<String>().contains("Skills")).unwrap();
        let text: String = row.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Skills 1 Slash  2 Blur  3 Rend"), "{text}");
        let dim: String = row.iter().filter(|c| c.fg == Color::DarkGray).map(|c| c.symbol()).collect();
        assert_eq!(dim, "Skills lur3 Rend");
    }
}
