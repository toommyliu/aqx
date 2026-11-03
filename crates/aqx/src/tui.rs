use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
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
use tui_input::Input as TextInput;
use tui_input::backend::crossterm::EventHandler;
use unicode_width::UnicodeWidthChar;

use aqx_script::Arg;

use crate::account::{expand_home, script_path};
use crate::hub::{AccountCell, Bar, Cells, Command, Game, Log, LogLine, Snapshot, Stack};
use crate::mask::Mask;
use crate::output::{span, time_of_day};
use crate::socket;

const WIDE: u16 = 90;
const NOTE_FOR: Duration = Duration::from_secs(4);
const RECENT_KEPT: usize = 100;
const ACCENT: Color = Color::Blue;
const CARD_WIDTH: u16 = 44;
const CARD_HEIGHT: u16 = 8;
const WHEEL_LINES: usize = 3;
const PAGE_LINES: usize = 10;
const RAIL_WIDTH: u16 = 34;
const LOG_MIN: u16 = 76;
const COLUMN_MIN: usize = 20;
const COLUMN_MAX: usize = 40;
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
    ("esc", "close list, unmark"),
    ("i", "log in"),
    ("x", "log out"),
    ("q", "detach"),
    ("Q", "log out all and end"),
    ("ctrl+c", "end or detach"),
];

const SCRIPT_KEYS: [(&str, &str); 16] = [
    ("", "Scripts"),
    ("o", "run a script"),
    ("s", "stop"),
    ("r", "restart"),
    ("p", "pause / resume"),
    ("", "View"),
    ("g", "dashboard"),
    ("d", "more details"),
    ("b", "hide sidebar"),
    ("B", "hide side panel"),
    ("[ ]", "resize sidebar"),
    ("l", "filter the log"),
    ("v", "next list"),
    ("⇧↑ ⇧↓", "scroll a line"),
    ("PgUp PgDn", "scroll a page"),
    ("End", "newest lines"),
];

pub struct Fleet {
    pub title: String,
    pub cells: Arc<Cells>,
    pub socket: PathBuf,
    pub home: PathBuf,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Players,
    Drops,
    Temp,
    Inventory,
    Bank,
}

impl Section {
    const ALL: [Section; 5] = [Section::Players, Section::Drops, Section::Temp, Section::Inventory, Section::Bank];

    fn after(open: Option<Section>) -> Option<Section> {
        match open {
            None => Some(Section::Players),
            Some(Section::Players) => Some(Section::Drops),
            Some(Section::Drops) => Some(Section::Temp),
            Some(Section::Temp) => Some(Section::Inventory),
            Some(Section::Inventory) => Some(Section::Bank),
            Some(Section::Bank) => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Players => "Players",
            Section::Drops => "Drops",
            Section::Temp => "Temp",
            Section::Inventory => "Inventory",
            Section::Bank => "Bank",
        }
    }

    fn rail_rows(self) -> usize {
        match self {
            Section::Players | Section::Temp | Section::Inventory => 4,
            Section::Drops => 6,
            Section::Bank => 0,
        }
    }

    fn len(self, g: &Game) -> usize {
        match self {
            Section::Players => g.cell_players.len(),
            Section::Drops => g.drops.len(),
            Section::Temp => g.temp.len(),
            Section::Inventory => g.inventory.len(),
            Section::Bank => g.bank.as_ref().map_or(0, Vec::len),
        }
    }

    fn count(self, g: &Game) -> Field {
        match self {
            Section::Inventory if g.misc_slots > 0 => vec![tally(g.bag_used, g.bag_slots), Span::raw(" · misc "), tally(g.misc_used, g.misc_slots)],
            Section::Inventory => vec![tally(g.bag_used, g.bag_slots)],
            Section::Bank => vec![tally(g.bank_used, g.bank_slots)],
            _ => vec![Span::raw(self.len(g).to_string())],
        }
    }

    fn title(self, g: &Game) -> Field {
        let mut title = vec![Span::raw(format!("{} ", self.label().to_lowercase()))];
        title.extend(self.count(g));
        match self {
            Section::Players => {}
            Section::Inventory => title.push(Span::raw(" · equipped first, then last changed")),
            _ => title.push(Span::raw(" · last changed first")),
        }
        title
    }

    fn entries(self, g: &Game) -> Vec<(String, Field)> {
        let stacks = |stacks: &[Stack]| stacks.iter().map(stacked).collect();
        match self {
            Section::Players => g.cell_players.iter().map(|p| (p.name.clone(), vec![Span::raw(p.level.to_string())])).collect(),
            Section::Drops => g.drops.iter().map(|d| (d.name.clone(), vec![Span::raw(d.qty.to_string())])).collect(),
            Section::Temp => stacks(&g.temp),
            Section::Inventory => {
                let (worn, carried): (Vec<&Stack>, Vec<&Stack>) = g.inventory.iter().partition(|s| s.equipped);
                worn.into_iter().chain(carried).map(stacked).collect()
            }
            Section::Bank => g.bank.as_deref().map(stacks).unwrap_or_default(),
        }
    }

    fn nothing(self, g: &Game) -> &'static str {
        match self {
            Section::Players => "Nobody else is in this cell.",
            Section::Drops => "No drops are waiting.",
            Section::Temp => "No temporary items.",
            Section::Inventory => "The inventory is empty.",
            Section::Bank if g.bank.is_none() => "aqx lists the bank once a script opens it.",
            Section::Bank => "The bank is empty.",
        }
    }
}

struct Prompt {
    input: TextInput,
    draft: TextInput,
    pick: Option<usize>,
    matches: Vec<String>,
}

impl Prompt {
    fn new(home: &Path) -> Self {
        let mut prompt = Prompt { input: TextInput::default(), draft: TextInput::default(), pick: None, matches: Vec::new() };
        prompt.find_matches(home);
        prompt
    }

    fn find_matches(&mut self, home: &Path) {
        self.matches = scripts_matching(self.input.value(), home);
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
        self.input = TextInput::new(recent[recent.len() - 1 - pick].typed.clone());
    }
}

struct Recent {
    typed: String,
    script: PathBuf,
}

type Slot = (PathBuf, String, Option<usize>);

struct Ask {
    typed: String,
    script: PathBuf,
    given: Vec<(String, String)>,
    asked: Vec<Arg>,
    accounts: Vec<(usize, String)>,
    answers: Vec<String>,
    input: TextInput,
    recalled: bool,
}

impl Ask {
    fn can_share(&self) -> bool {
        self.accounts.len() > 1 && self.answers.is_empty()
    }

    fn per_account(&self, shared: bool) -> bool {
        self.accounts.len() > 1 && !shared
    }

    fn asking(&self, shared: bool) -> (&Arg, Option<&(usize, String)>) {
        let n = self.answers.len();
        let account = self.per_account(shared).then(|| &self.accounts[n / self.asked.len()]);
        (&self.asked[n % self.asked.len()], account)
    }

    fn slot(&self, shared: bool) -> Slot {
        let (arg, account) = self.asking(shared);
        (self.script.clone(), arg.key.clone(), account.map(|&(i, _)| i))
    }

    fn recall(&mut self, remembered: &HashMap<Slot, String>, shared: bool) {
        let (script, key, account) = self.slot(shared);
        let value = account.and_then(|i| remembered.get(&(script.clone(), key.clone(), Some(i)))).or_else(|| remembered.get(&(script, key, None)));
        self.input = value.cloned().map(TextInput::new).unwrap_or_default();
        self.recalled = !self.input.value().is_empty();
    }

    fn edit(&mut self) {
        if std::mem::take(&mut self.recalled) {
            self.input.reset();
        }
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
    rail_hidden: bool,
    list_width: Option<u16>,
    list_area: Cell<Option<Rect>>,
    dragging: bool,
    frame: usize,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    log_width: Cell<usize>,
    list: Option<Section>,
    list_at: Cell<usize>,
    list_len: Cell<usize>,
    cursor: usize,
    picked: BTreeSet<String>,
    list_max: Cell<usize>,
    rail: Cell<Option<(Rect, [usize; 5])>>,
    unseen_from: Option<u64>,
    jump_area: Cell<Option<Rect>>,
    list_top: Cell<usize>,
    grid_top: Cell<usize>,
    grid_cols: Cell<usize>,
    prompt: Option<Prompt>,
    query: TextInput,
    querying: bool,
    ask: Option<Ask>,
    share_args: bool,
    remembered: HashMap<Slot, String>,
    ending: bool,
    recent: Vec<Recent>,
    history: Option<PathBuf>,
    note: Option<(String, Instant)>,
    log: Log,
    log_of: Option<usize>,
    redact: bool,
    mask: Mask,
}

struct Shown {
    name: String,
    snapshot: Option<Snapshot>,
}

pub fn run(fleet: &Fleet, redact: bool, done: impl Fn() -> bool) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        restore(info);
    }));
    let app = crate::config::user_state().map_or_else(App::default, |dir| App::remembering(dir.join("history"), &fleet.home));
    let mut app = App { redact, ..app };
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
    let cells = fleet.cells.all();
    app.mask = if app.redact { mask(&cells) } else { Mask::default() };
    let shown: Vec<Shown> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let board = cell.board();
            if i == app.selected {
                let from = app.log.last().map_or(0, |l| l.seq + 1);
                let fresh = board.log.iter().rev().take_while(|l| l.seq >= from).count();
                let fresh = board.log.iter().skip(board.log.len() - fresh);
                for line in fresh {
                    let line = LogLine { text: app.mask.hide(&line.text), ..line.clone() };
                    if app.scroll > 0 && app.filter.shows(&line) {
                        app.scroll += pieces(&line, app.log_width.get(), app.filter.tagged()).1.len();
                        app.unseen_from.get_or_insert(line.seq);
                    }
                    app.log.push(line);
                }
            }
            Shown { name: app.mask.hide(&cell.name), snapshot: board.snapshot.as_ref().and_then(|s| app.mask.hide_in(s)) }
        })
        .collect();
    if snapshot(&shown, app.selected).and_then(|s| s.game.as_ref()).is_none() {
        app.list = None;
    }
    for script in shown.iter().filter_map(|s| s.snapshot.as_ref()?.script.as_ref()).map(PathBuf::from) {
        if !app.recent.iter().any(|r| r.script == script) {
            app.recent.insert(0, Recent { typed: shown_path(&fleet.home, &script), script });
        }
    }
    shown
}

pub fn mask(cells: &[Arc<AccountCell>]) -> Mask {
    let names = cells.iter().enumerate().flat_map(|(i, cell)| {
        let user = cell.board().snapshot.as_ref().map(|s| s.user.clone());
        let alias = format!("account {}", i + 1);
        [Some(cell.name.clone()), user].into_iter().flatten().map(move |name| (name, alias.clone()))
    });
    Mask::new(std::env::var("HOME").ok(), names)
}

fn snapshot(shown: &[Shown], i: usize) -> Option<&Snapshot> {
    shown.get(i)?.snapshot.as_ref()
}

fn drop_names(shown: &[Shown], i: usize) -> Vec<String> {
    snapshot(shown, i).iter().flat_map(|s| &s.game).flat_map(|g| &g.drops).map(|d| d.name.clone()).collect()
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
    fn remembering(history: PathBuf, home: &Path) -> App {
        let text = std::fs::read_to_string(&history).unwrap_or_default();
        let mut recent: Vec<Recent> = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (script, args) = split_script(line);
            let script = expand_home(Path::new(script));
            let typed = format!("{}{args}", shown_path(home, &script));
            recent.retain(|r| r.typed != typed);
            recent.push(Recent { typed, script });
        }
        recent.drain(..recent.len().saturating_sub(RECENT_KEPT));
        App { recent, history: Some(history), ..App::default() }
    }

    fn save_recent(&self, typed: &str, script: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(history) = &self.history else { return Ok(()) };
        if let Some(dir) = history.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(history)?;
        file.write_all(format!("{}{}\n", script.display(), split_script(typed).1).as_bytes())
    }

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
        let order = order(shown, self.query.value());
        let Some(at) = order.iter().position(|&i| i == self.selected) else { return };
        let (at, n) = (at as isize + by, order.len() as isize);
        self.pick(order[if wrap { at.rem_euclid(n) } else { at.clamp(0, n - 1) } as usize]);
    }

    fn jump(&mut self, shown: &[Shown], by: isize) {
        let order = order(shown, self.query.value());
        let offline = |i| snapshot(shown, i).is_some_and(is_offline);
        let starts: Vec<usize> = (0..order.len()).filter(|&at| at == 0 || offline(order[at]) != offline(order[at - 1])).collect();
        let Some(at) = order.iter().position(|&i| i == self.selected) else { return };
        let group = starts.iter().rposition(|&start| start <= at).unwrap_or(0) as isize;
        self.pick(order[starts[(group + by).rem_euclid(starts.len() as isize) as usize]]);
    }

    fn refilter(&mut self, shown: &[Shown]) {
        let order = order(shown, self.query.value());
        if let Some(&first) = order.first().filter(|_| !order.contains(&self.selected)) {
            self.pick(first);
        }
    }

    fn pick(&mut self, i: usize) {
        if i != self.selected {
            self.selected = i;
            self.scroll = 0;
            self.reset_list();
        }
    }

    fn open(&mut self, list: Option<Section>) {
        self.list = list;
        self.reset_list();
    }

    fn reset_list(&mut self) {
        self.list_at.set(0);
        self.cursor = 0;
        self.picked.clear();
    }

    fn up(&mut self, lines: usize) {
        let last = self.list_len.get().saturating_sub(1);
        match self.list {
            Some(Section::Drops) => self.cursor = self.cursor.min(last).saturating_sub(lines),
            Some(_) => self.list_at.set(self.list_at.get().min(self.list_max.get()).saturating_sub(lines)),
            None => self.scroll = self.scroll.saturating_add(lines).min(self.max_scroll.get()),
        }
    }

    fn down(&mut self, lines: usize) {
        match self.list {
            Some(Section::Drops) => self.cursor = self.cursor.saturating_add(lines).min(self.list_len.get().saturating_sub(1)),
            Some(_) => self.list_at.set(self.list_at.get().saturating_add(lines).min(self.list_max.get())),
            None => self.scroll = self.scroll.saturating_sub(lines),
        }
    }

    fn pick_up(&mut self, fleet: &Fleet, shown: &[Shown]) {
        let mut wanted = self.picked.clone();
        if wanted.is_empty() {
            let drops = drop_names(shown, self.selected);
            wanted.extend(drops.get(self.cursor.min(drops.len().saturating_sub(1))).cloned());
        }
        let Some(cell) = fleet.cells.all().get(self.selected).cloned() else { return };
        let items: Vec<String> =
            cell.board().snapshot.iter().flat_map(|s| &s.game).flat_map(|g| &g.drops).filter(|d| wanted.contains(&self.mask.hide(&d.name))).map(|d| d.name.clone()).collect();
        let done = match items.as_slice() {
            [] => return,
            [only] => format!("picking up {only}"),
            many => format!("picking up {} drops", many.len()),
        };
        self.picked.clear();
        self.act_on(fleet, vec![(self.selected, Command::PickUp { items })], &done);
    }

    fn mouse(&mut self, mouse: MouseEvent, shown: &[Shown]) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(mouse.column, mouse.row, shown),
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => self.list_width = Some(mouse.column + 1),
            MouseEventKind::Up(_) => self.dragging = false,
            _ if self.grid => {}
            MouseEventKind::ScrollUp => self.up(WHEEL_LINES),
            MouseEventKind::ScrollDown => self.down(WHEEL_LINES),
            _ => {}
        }
    }

    fn click(&mut self, column: u16, row: u16, shown: &[Shown]) {
        let at = Position::new(column, row);
        if self.jump_area.get().is_some_and(|pill| pill.contains(at)) {
            self.scroll = 0;
            return;
        }
        if let Some((rail, starts)) = self.rail.get().filter(|(rail, _)| rail.contains(at)) {
            let section = Section::ALL[starts.iter().rposition(|&start| start <= usize::from(row - rail.y)).unwrap_or(0)];
            return self.open(if self.list == Some(section) { None } else { Some(section) });
        }
        let list = self.list_area.get();
        self.dragging = list.is_some_and(|l| column + 1 == l.right() || column == l.right());
        let Some(list) = list.filter(|l| !self.dragging && l.contains(Position::new(column, row))) else { return };
        let at = self.list_top.get() + usize::from(row - list.y);
        if let Some(&i) = list_rows(shown, self.query.value()).get(at..).and_then(|rows| rows.iter().flatten().next()) {
            self.pick(i);
        }
    }

    fn resize_list(&mut self, by: impl Fn(u16) -> u16) {
        if let Some(list) = self.list_area.get() {
            self.list_width = Some(by(list.width));
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((self.mask.hide(&text.into()), Instant::now() + NOTE_FOR));
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
                KeyCode::Tab => {
                    prompt.pick = None;
                    match prompt.matches.as_slice() {
                        [] => {}
                        [only] if !only.ends_with('/') => prompt.input = TextInput::new(format!("{only} ")),
                        many => prompt.input = TextInput::new(shared_prefix(many).to_string()),
                    }
                }
                _ => {
                    if prompt.input.handle_event(&Input::Key(key)).is_some_and(|changed| changed.value) {
                        prompt.pick = None;
                    }
                }
            }
            if let Some(prompt) = &mut self.prompt {
                prompt.find_matches(&fleet.home);
            }
            return false;
        }
        if self.querying {
            match key.code {
                KeyCode::Esc => {
                    self.query.reset();
                    self.querying = false;
                }
                KeyCode::Enter => self.querying = false,
                KeyCode::Up => self.step(shown, -1, false),
                KeyCode::Down => self.step(shown, 1, false),
                KeyCode::Tab => {
                    let names: Vec<&str> = order(shown, self.query.value()).into_iter().map(|i| shown[i].name.as_str()).collect();
                    self.query = TextInput::new(complete(self.query.value(), &names));
                }
                _ => {
                    self.query.handle_event(&Input::Key(key));
                }
            }
            self.refilter(shown);
            return false;
        }
        if let Some(ask) = &mut self.ask {
            match key.code {
                KeyCode::Esc => self.ask = None,
                KeyCode::Tab if ask.can_share() => {
                    self.share_args = !self.share_args;
                    if ask.recalled || ask.input.value().is_empty() {
                        ask.recall(&self.remembered, self.share_args);
                    }
                }
                KeyCode::Enter if !ask.input.value().trim().is_empty() || ask.asking(self.share_args).0.default.is_some() => {
                    let value = std::mem::take(&mut ask.input).value().trim().to_string();
                    self.remembered.insert(ask.slot(self.share_args), value.clone());
                    ask.answers.push(value);
                    if ask.left(self.share_args) == 0 {
                        let ask = self.ask.take().expect("an ask is open");
                        self.run(fleet, ask.typed.clone(), &ask.script, ask.args(self.share_args));
                    } else {
                        ask.recall(&self.remembered, self.share_args);
                    }
                }
                KeyCode::Backspace | KeyCode::Delete if ask.recalled => ask.edit(),
                _ => {
                    if matches!(key.code, KeyCode::Char(_)) && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
                        ask.edit();
                    }
                    if ask.input.handle_event(&Input::Key(key)).is_some() {
                        ask.recalled = false;
                    }
                }
            }
            return false;
        }
        let accounts = shown.len();
        let visible = order(shown, self.query.value());
        let cols = self.grid_cols.get() as isize;
        let line = accounts < 2 || key.modifiers.contains(KeyModifiers::SHIFT);
        let picking = self.list == Some(Section::Drops) && !self.grid;
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('Q') => self.ending = true,
            KeyCode::Char('?') => self.keys = true,
            KeyCode::Char('g') => self.grid = !self.grid,
            KeyCode::Char('d' | 'l' | 'v') | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End if self.grid => {}
            KeyCode::Enter if self.grid => self.grid = false,
            KeyCode::Left if self.grid => self.step(shown, -1, false),
            KeyCode::Right if self.grid => self.step(shown, 1, false),
            KeyCode::Up if self.grid => self.step(shown, -cols, false),
            KeyCode::Down if self.grid => self.step(shown, cols, false),
            KeyCode::Up if picking => self.up(1),
            KeyCode::Down if picking => self.down(1),
            KeyCode::Char(' ') if picking => {
                if let Some(name) = drop_names(shown, self.selected).get(self.cursor).cloned()
                    && !self.picked.remove(&name)
                {
                    self.picked.insert(name);
                }
            }
            KeyCode::Char('a') if picking => {
                let drops = drop_names(shown, self.selected);
                if drops.iter().all(|d| self.picked.contains(d)) {
                    self.picked.clear();
                } else {
                    self.picked.extend(drops);
                }
            }
            KeyCode::Enter if picking => self.pick_up(fleet, shown),
            KeyCode::Esc if !self.picked.is_empty() => self.picked.clear(),
            KeyCode::Up if line => self.up(1),
            KeyCode::Down if line => self.down(1),
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
            KeyCode::Esc | KeyCode::Char('l') if self.list.is_some() => self.open(None),
            KeyCode::Esc => self.marked.clear(),
            KeyCode::PageUp => self.up(PAGE_LINES),
            KeyCode::PageDown => self.down(PAGE_LINES),
            KeyCode::Home => self.up(usize::MAX),
            KeyCode::End => self.down(usize::MAX),
            KeyCode::Char('v') => self.open(Section::after(self.list)),
            KeyCode::Char('l') => {
                self.filter = self.filter.next();
                self.scroll = 0;
            }
            KeyCode::Char('d') => self.details = !self.details,
            KeyCode::Char('b') => self.list_hidden = !self.list_hidden,
            KeyCode::Char('B') => self.rail_hidden = !self.rail_hidden,
            KeyCode::Char('[') => self.resize_list(|w| w.saturating_sub(LIST_STEP)),
            KeyCode::Char(']') => self.resize_list(|w| w + LIST_STEP),
            KeyCode::Char('o') => self.prompt = Some(Prompt::new(&fleet.home)),
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
        let input = prompt.input.value().trim().to_string();
        if input.is_empty() {
            return;
        }
        let (word, rest) = split_script(&input);
        let path = match script_path(&fleet.home.join(expand_home(Path::new(word)))) {
            Ok(path) => path,
            Err(e) => return self.say(e),
        };
        let args = match typed_args(rest.split_whitespace()) {
            Ok(args) => args,
            Err(e) => return self.say(e),
        };
        let input = format!("{}{rest}", shown_path(&fleet.home, &path));
        let asked: Vec<Arg> = aqx_script::script_args(&path).into_iter().filter(|arg| !args.iter().any(|(given, _)| *given == arg.key)).collect();
        let targets = self.targets();
        if asked.is_empty() {
            let each = targets.into_iter().map(|i| (i, args.clone())).collect();
            return self.run(fleet, input, &path, each);
        }
        let cells = fleet.cells.all();
        let accounts = targets.into_iter().filter_map(|i| Some((i, self.mask.hide(&cells.get(i)?.name)))).collect();
        let mut ask = Ask { typed: input, script: path, given: args, asked, accounts, answers: Vec::new(), input: TextInput::default(), recalled: false };
        ask.recall(&self.remembered, self.share_args);
        self.ask = Some(ask);
    }

    fn run(&mut self, fleet: &Fleet, typed: String, script: &Path, each: Vec<(usize, Vec<(String, String)>)>) {
        let saved = self.save_recent(&typed, script);
        self.recent.retain(|r| r.typed != typed);
        self.recent.push(Recent { typed, script: script.to_path_buf() });
        let commands = each.into_iter().map(|(i, args)| (i, Command::Run { script: script.to_path_buf(), args })).collect();
        self.act_on(fleet, commands, &format!("running {}", script.display()));
        if let (Err(e), Some(history)) = (saved, &self.history) {
            self.say(format!("couldn't save to {}: {e}", history.display()));
        }
    }
}

fn draw(f: &mut Frame, fleet: &Fleet, app: &App, shown: &[Shown]) {
    let matches: Vec<String> = match &app.prompt {
        _ if app.querying => order(shown, app.query.value()).into_iter().map(|i| shown[i].name.clone()).collect(),
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
    app.rail.set(None);
    draw_header(f, header, fleet, app, shown, list || app.grid);
    if app.grid {
        draw_grid(f, body, app, shown);
    } else if let Some(account) = shown.get(app.selected) {
        let pane = if list {
            let fitted = (4 + name_width(shown) + 1 + ABOUT_WIDTH + 1) as u16;
            let width = app.list_width.unwrap_or(fitted).clamp(LIST_MIN, body.width - PANE_MIN);
            let [left, _, pane, _] = Layout::horizontal([Constraint::Length(width), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(body);
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

fn typed_args<'a>(words: impl Iterator<Item = &'a str>) -> Result<Vec<(String, String)>, String> {
    let mut args: Vec<(String, String)> = Vec::new();
    for word in words {
        match (word.trim_start_matches("--arg=").split_once('='), args.last_mut()) {
            (Some((key, value)), _) if !key.is_empty() => args.push((key.to_string(), value.to_string())),
            (_, Some((_, value))) => {
                value.push(' ');
                value.push_str(word);
            }
            (_, None) => return Err(format!("{word} isn't KEY=VALUE")),
        }
    }
    Ok(args)
}

fn shown_path(home: &Path, script: &Path) -> String {
    script.strip_prefix(home).unwrap_or(script).display().to_string()
}

fn split_script(typed: &str) -> (&str, &str) {
    typed.split_at(typed.find(char::is_whitespace).unwrap_or(typed.len()))
}

fn scripts_matching(input: &str, home: &Path) -> Vec<String> {
    let input = input.trim_start();
    if input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (dir, prefix) = input.split_at(input.rfind('/').map_or(0, |i| i + 1));
    let Ok(entries) = std::fs::read_dir(home.join(expand_home(Path::new(dir)))) else { return Vec::new() };
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
        let order = order(shown, app.query.value());
        let at = order.iter().position(|&i| i == app.selected).unwrap_or(0);
        left.push(Span::raw(format!("  {} {}/{}", account.name, at + 1, order.len())).dark_gray());
    }
    if !app.query.value().is_empty() {
        left.push(Span::raw(format!(" · filter \"{}\"", app.query.value())).dark_gray());
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
    let rows = list_rows(shown, app.query.value());
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

fn typed_spans(input: &TextInput) -> Vec<Span<'static>> {
    let value = input.value();
    let at = value.char_indices().nth(input.cursor()).map_or(value.len(), |(i, _)| i);
    let (before, after) = value.split_at(at);
    let mut after = after.chars();
    let cursor = match after.next() {
        Some(c) => Span::raw(c.to_string()).reversed(),
        None => Span::raw("█"),
    };
    vec![Span::raw(before.to_string()), cursor, Span::raw(after.as_str().to_string())]
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let width = area.width as usize;
    let who = app.who(shown, width / 3);
    if app.querying {
        let keys = hints(&[("↑↓", "pick"), ("tab", "complete"), ("enter", "done"), ("esc", "clear")]);
        let typed = [vec![Span::raw(" filter accounts: ").bold()], typed_spans(&app.query)].concat();
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some(prompt) = &app.prompt {
        let label = format!(" run on {who}: ");
        let keys = hints(&[("tab", "complete"), ("↑↓", "recent"), ("esc", "cancel")]);
        let typed = [vec![Span::raw(label).bold()], typed_spans(&prompt.input)].concat();
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some(ask) = &app.ask {
        let script = ask.script.file_name().map_or_else(|| ask.script.display().to_string(), |name| name.to_string_lossy().into_owned());
        let (arg, account) = ask.asking(app.share_args);
        let account = account.map(|(_, a)| format!(" for {a}")).unwrap_or_default();
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
        let input = if ask.recalled { vec![Span::raw(ask.input.value().to_string()).reversed(), Span::raw("█")] } else { typed_spans(&ask.input) };
        let mut typed = [vec![Span::raw(label).bold()], input].concat();
        if let Some(default) = arg.default.as_deref().filter(|_| ask.input.value().is_empty()) {
            typed.push(Span::raw(if default.is_empty() { "optional" } else { default }.to_string()).dark_gray());
        }
        return f.render_widget(Paragraph::new(spread(typed, keys, width.saturating_sub(1))), area);
    }
    if let Some((note, until)) = &app.note
        && Instant::now() < *until
    {
        return f.render_widget(Paragraph::new(format!(" {note}")).yellow(), area);
    }
    let keys = if app.list.is_some() && !app.grid { list_keys(app) } else { account_keys(app, shown) };
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

fn list_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let mut keys = Vec::new();
    if app.list == Some(Section::Drops) && app.list_len.get() > 0 {
        let accept = if app.picked.is_empty() { "accept" } else { "accept marked" };
        keys.extend([("↑↓", "pick"), ("space", "mark"), ("a", "mark all"), ("enter", accept)]);
    } else if app.list_max.get() > 0 {
        keys.push(("PgUp PgDn", "scroll"));
    }
    keys.extend([("v", "next list"), ("esc", if app.picked.is_empty() { "back to log" } else { "clear marks" })]);
    keys
}

fn account_keys(app: &App, shown: &[Shown]) -> Vec<(&'static str, &'static str)> {
    let details = if app.details { "less" } else { "details" };
    let offline = app.all(shown, is_offline);
    let mut keys = if offline {
        vec![("i", "log in"), ("o", "log in and run")]
    } else {
        let pause = if app.all(shown, |s| s.paused) { "resume" } else { "pause" };
        vec![("o", "run"), ("s", "stop"), ("r", "restart"), ("p", pause), ("x", "log out")]
    };
    keys.push(("space", "mark"));
    match (app.grid, offline) {
        (true, _) => keys.push(("enter", "open")),
        (false, true) => keys.push(("d", details)),
        (false, false) => keys.extend([("d", details), ("l", "filter"), ("v", "lists")]),
    }
    keys
}

fn draw_grid(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let cols = usize::from(area.width / CARD_WIDTH).max(1);
    let rows = usize::from(area.height / CARD_HEIGHT).max(1);
    app.grid_cols.set(cols);
    let order = order(shown, app.query.value());
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
            let mut fields = summary(s, g);
            let pools = [("inventory", g.bag_used, g.bag_slots), ("misc", g.misc_used, g.misc_slots)];
            fields.extend(pools.into_iter().filter_map(|(pool, used, slots)| fill(used, slots).map(|(color, word)| vec![Span::raw(format!("{pool} {used}/{slots} {word}")).fg(color)])));
            lines.extend(flow(vec![fields], width));
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
    let head = detail(account, app.details, app.frame, usize::from(area.width));
    let [top, divider, below] = Layout::vertical([Constraint::Length(head.len() as u16), Constraint::Length(1), Constraint::Min(0)]).areas(area);
    f.render_widget(Paragraph::new(head), top);
    let game = account.snapshot.as_ref().and_then(|s| s.game.as_ref());
    let rail = game.filter(|_| !app.rail_hidden && area.width >= LOG_MIN + RAIL_WIDTH);
    let [main, side] = Layout::horizontal([Constraint::Min(0), Constraint::Length(if rail.is_some() { RAIL_WIDTH } else { 0 })]).areas(below);
    let mut rule = vec![Span::raw("── ")];
    let tabs = game.filter(|_| rail.is_none());
    match app.list.zip(game) {
        Some((section, g)) => {
            draw_section(f, main, g, section, app);
            match tabs {
                Some(g) => rule.extend(counts(g, app.list)),
                None => rule.extend(section.title(g)),
            }
            if !app.picked.is_empty() {
                rule.push(Span::raw(format!(" · {} marked", app.picked.len())));
            }
            let (at, max) = (app.list_at.get().min(app.list_max.get()), app.list_max.get());
            if at > 0 {
                rule.push(Span::raw(format!(" · ↑{at}")));
            }
            if at < max {
                rule.push(Span::raw(format!(" · ↓{}", max - at)));
            }
        }
        None => {
            rule.push(Span::raw(format!("log · {}", app.filter.label())));
            let scrolled = draw_log(f, main, app);
            if scrolled > 0 {
                rule.push(Span::raw(format!(" · ↑{scrolled}")));
            }
            if let Some(g) = tabs {
                rule.push(Span::raw(" ── "));
                rule.extend(counts(g, None));
            }
        }
    }
    rule.push(Span::raw(" "));
    let width = usize::from(main.width);
    rule.push(Span::raw("─".repeat(width.saturating_sub(spans_width(&rule)))));
    let mut rule = clip(rule, width);
    if let Some(g) = rail {
        rule.push(Span::raw(format!("┬{}", "─".repeat(usize::from(RAIL_WIDTH) - 1))));
        draw_rail(f, side, g, app);
    }
    f.render_widget(Paragraph::new(Line::from(rule)).dark_gray(), divider);
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) -> usize {
    let pill_row = usize::from(app.scroll > 0);
    let log = log_lines(app, usize::from(area.height).saturating_sub(pill_row), usize::from(area.width));
    let scrolled = app.scroll.min(app.max_scroll.get());
    let has_log = !log.is_empty();
    f.render_widget(Paragraph::new(log), area);
    if scrolled > 0 && has_log {
        draw_jump(f, area, app);
    }
    scrolled
}

fn counts(g: &Game, open: Option<Section>) -> Field {
    let mut spans = Vec::new();
    for section in Section::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw(" · "));
        }
        let mut tab = vec![Span::raw(format!("{} ", section.label().to_lowercase()))];
        tab.extend(section.count(g));
        if open == Some(section) {
            tab = tab.into_iter().map(|span| span.fg(ACCENT).bold()).collect();
        }
        spans.extend(tab);
    }
    spans
}

fn draw_rail(f: &mut Frame, area: Rect, g: &Game, app: &App) {
    let width = usize::from(area.width.saturating_sub(1));
    let sizes = Section::ALL.map(|s| if s == Section::Bank || app.list == Some(s) { 0 } else { s.len(g) });
    let (gaps, rows) = rail_rows(sizes, usize::from(area.height));
    let mut lines = Vec::new();
    let mut starts = [0; 5];
    for (i, section) in Section::ALL.into_iter().enumerate() {
        if gaps && i > 0 {
            lines.push(Line::default());
        }
        starts[i] = lines.len();
        lines.push(fit(heading(g, section, app.list == Some(section)), width));
        let entries = if sizes[i] == 0 { Vec::new() } else { section.entries(g) };
        let cut = entries.len() > rows[i] && rows[i] >= 2;
        let fits = if cut { rows[i] - 1 } else { entries.len().min(rows[i]) };
        let more = entries.len() - fits;
        lines.extend(entries.into_iter().take(fits).map(|(name, value)| Line::from(entry(3, name, value, width))));
        if cut {
            lines.push(Line::raw(format!("   +{more}")).dark_gray());
        }
    }
    app.rail.set(Some((Rect { height: area.height.min(lines.len() as u16), ..area }, starts)));
    let rule = Block::new().borders(Borders::LEFT).border_style(Style::new().dark_gray());
    f.render_widget(Paragraph::new(lines).block(rule), area);
}

fn rail_rows(sizes: [usize; 5], height: usize) -> (bool, [usize; 5]) {
    let gaps = Section::ALL.len() - 1;
    let mut left = height.saturating_sub(Section::ALL.len());
    let mut rows = [0; 5];
    for (i, section) in Section::ALL.into_iter().enumerate() {
        rows[i] = sizes[i].min(section.rail_rows()).min(left);
        left -= rows[i];
    }
    let spaced = left >= gaps;
    if spaced {
        left -= gaps;
    }
    for i in [Section::Inventory, Section::Players, Section::Drops, Section::Temp].map(|s| s as usize) {
        let more = (sizes[i] - rows[i]).min(left);
        rows[i] += more;
        left -= more;
    }
    (spaced, rows)
}

fn heading(g: &Game, section: Section, open: bool) -> Field {
    let mut spans = vec![Span::raw(if open { "▌" } else { " " }).fg(ACCENT)];
    let empty = match section {
        Section::Players | Section::Drops | Section::Temp => "none",
        Section::Inventory | Section::Bank => "",
    };
    if section.len(g) == 0 && !empty.is_empty() {
        spans.push(Span::raw(format!("{} {empty}", section.label())).dark_gray());
        return spans;
    }
    spans.extend([Span::raw(section.label()).bold(), Span::raw(" ")]);
    spans.extend(section.count(g));
    if section == Section::Bank && g.bank.is_none() {
        spans.push(Span::raw("  not opened").dark_gray());
    }
    spans
}

fn tally(used: u32, slots: u32) -> Span<'static> {
    if slots == 0 {
        return Span::raw(used.to_string());
    }
    let text = Span::raw(format!("{used}/{slots}"));
    match fill(used, slots) {
        Some((color, _)) => text.fg(color),
        None => text,
    }
}

fn fill(used: u32, slots: u32) -> Option<(Color, &'static str)> {
    match () {
        _ if slots == 0 => None,
        _ if used >= slots => Some((Color::Red, "full")),
        _ if used * 10 >= slots * 9 => Some((Color::Yellow, "nearly full")),
        _ => None,
    }
}

fn stacked(s: &Stack) -> (String, Field) {
    (s.name.clone(), held(s))
}

fn held(s: &Stack) -> Field {
    match s.rank {
        _ if s.equipped => vec![Span::raw("equipped").dark_gray()],
        Some(rank) => vec![Span::raw(format!("rank {rank}"))],
        None if s.max > 1 && s.qty >= s.max => vec![Span::raw(format!("{}/{} ✓", s.qty, s.max)).green()],
        None if s.max > 1 => vec![Span::raw(format!("{}/{}", s.qty, s.max))],
        None if s.qty > 1 => vec![Span::raw(s.qty.to_string())],
        None => Vec::new(),
    }
}

fn entry(indent: usize, name: String, value: Field, width: usize) -> Field {
    let value_width = spans_width(&value);
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    spans.extend(clip(vec![Span::raw(name)], width.saturating_sub(indent + value_width + 1)));
    spans.push(Span::raw(" ".repeat(width.saturating_sub(spans_width(&spans) + value_width))));
    spans.extend(value);
    clip(spans, width)
}

fn draw_section(f: &mut Frame, area: Rect, g: &Game, section: Section, app: &App) {
    let entries = section.entries(g);
    app.list_len.set(entries.len());
    if entries.is_empty() {
        app.list_max.set(0);
        return f.render_widget(Paragraph::new(Line::raw(format!("  {}", section.nothing(g))).dark_gray()), area);
    }
    let width = usize::from(area.width);
    let widest = entries.iter().map(|(name, value)| Span::raw(name.as_str()).width() + spans_width(value)).max().unwrap_or(0);
    let picker = section == Section::Drops;
    let cols = if picker { 1 } else { (width / (widest + 4).clamp(COLUMN_MIN, COLUMN_MAX)).max(1) };
    let column = if picker { COLUMN_MAX.min(width) } else { width / cols };
    let rows = entries.len().div_ceil(cols);
    let height = usize::from(area.height);
    app.list_max.set(rows.saturating_sub(height));
    let cursor = picker.then(|| app.cursor.min(entries.len() - 1));
    let mut at = app.list_at.get().min(app.list_max.get());
    if let Some(row) = cursor.map(|i| i % rows) {
        at = at.min(row).max((row + 1).saturating_sub(height));
    }
    app.list_at.set(at);
    let marks = |i: usize, name: &String| -> Field {
        let Some(cursor) = cursor else { return vec![Span::raw("  ")] };
        vec![Span::raw(if i == cursor { "▌" } else { " " }).fg(ACCENT), Span::raw(if app.picked.contains(name) { "◆" } else { " " }).fg(ACCENT)]
    };
    let lines: Vec<Line> = (at..rows.min(at + height))
        .map(|row| {
            let cells = (0..cols).map(|col| col * rows + row).filter_map(|i| Some((i, entries.get(i)?)));
            let spans = cells.flat_map(|(i, (name, value))| {
                marks(i, name).into_iter().chain(entry(0, name.clone(), value.clone(), column.saturating_sub(4))).chain([Span::raw("  ")])
            });
            Line::from(spans.collect::<Field>())
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
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
        if full {
            lines.extend(flow(everything(s, g, width), width));
        } else {
            lines.push(fit(summary(s, g).join(&Span::raw("  ")), width));
        }
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

fn summary(s: &Snapshot, g: &Game) -> Vec<Field> {
    let mut fields = vec![hp(g), amount(s.kills, "kill")];
    fields.extend(s.gold_gained.map(gained));
    fields.extend(g.target.as_ref().map(|t| labeled("Target", vec![Span::raw(t.name.clone())])));
    fields
}

fn everything(s: &Snapshot, g: &Game, width: usize) -> Vec<Vec<Field>> {
    let class = match g.class.as_str() {
        "" => vec![Span::raw("no class").dark_gray()],
        name => vec![Span::raw(format!("{name} ")), Span::raw("rank ").dark_gray(), Span::raw(g.class_rank.to_string())],
    };
    let mut you = vec![
        hp(g),
        vitals("MP", g.mp, g.max_mp, Color::Blue),
        labeled("Lv", vec![Span::raw(g.level.to_string())]),
        class,
        vec![Span::raw(short(g.gold)), Span::raw(" gold").dark_gray()],
    ];
    if g.afk {
        you.push(vec![Span::raw("away from keyboard").yellow()]);
    }
    let target = match &g.target {
        Some(t) => {
            let mut field = labeled("Target", vec![Span::raw(format!("{} ", t.name))]);
            field.extend(meter(t.hp, t.max_hp, 8, Color::Red));
            field.push(Span::raw(format!(" {}/{}", t.hp, t.max_hp)));
            field
        }
        None => vec![Span::raw("no target").dark_gray()],
    };
    let mut combat = Vec::new();
    if !g.skills.is_empty() {
        combat.push(skills(g));
    }
    if !g.auras.is_empty() {
        combat.push(listed("Auras", g.auras.iter().map(|a| (a.name.clone(), (a.stacks > 1).then(|| format!("x{}", a.stacks)))), width));
    }
    let mut stats = vec![labeled("Stats", amount(s.kills, "kill")), amount(s.drops, "drop")];
    stats.extend(s.gold_gained.map(gained));
    stats.push(amount(s.deaths, "death"));
    vec![you, vec![target], combat, stats]
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
    field.push(Span::raw(format!(" {current:>digits$}/{max}", digits = max.to_string().len())));
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
    let mut head = vec![Span::raw(time_of_day(l.t)).dark_gray(), Span::raw("  ")];
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
    use crate::hub::{Board, Hub, LOG_LINES, Player, Print, SkillState, Stack};

    fn rows_of(fleet: &Fleet, app: &mut App, width: u16, height: u16) -> Vec<Vec<ratatui::buffer::Cell>> {
        let shown = gather(fleet, app);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, fleet, app, &shown)).unwrap();
        terminal.backend().buffer().content.chunks(width as usize).map(<[_]>::to_vec).collect()
    }

    fn screen_of(fleet: &Fleet, app: &mut App, width: u16, height: u16) -> String {
        rows_of(fleet, app, width, height).iter().map(|row| row.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn rows(fleet: &Fleet, app: &mut App, width: u16) -> Vec<Vec<ratatui::buffer::Cell>> {
        rows_of(fleet, app, width, 24)
    }

    fn screen(fleet: &Fleet, app: &mut App, width: u16) -> String {
        screen_of(fleet, app, width, 24)
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
            json(r#"{"cmd":"updateClass","uid":1001,"sClassName":"Barber","iCP":22500}"#),
            json(r#"{"cmd":"ct","a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Corruption","dur":20,"stk":2}]}]}"#),
        ] {
            reduce(&mut world, &Message::parse(&text).unwrap(), Instant::now());
        }
        world
    }

    fn stock(cell: &AccountCell, items: usize) {
        let stack = |name: &str, qty, max| Stack { name: name.into(), qty, max, ..Stack::default() };
        let named = [
            stack("Fishing Bait", 533, 999),
            Stack { equipped: true, rank: Some(10), ..stack("Master Ranger", 302500, 1) },
            stack("Treasure Chest", 6, 99),
            stack("Paint", 99, 99),
            Stack { rank: Some(4), ..stack("Rogue", 21980, 1) },
        ];
        let mut board = cell.board();
        let g = board.snapshot.as_mut().and_then(|s| s.game.as_mut()).expect("a published game");
        g.drops = (0..8).map(|i| stack(&format!("Spider Drop {i}"), i + 1, 99)).collect();
        g.temp = vec![stack("Spider Silk", 4, 10), stack("Slime Sample", 10, 10)];
        g.inventory = (0..items).map(|i| named.get(i).cloned().unwrap_or_else(|| stack(&format!("Spare Part {i:02}"), 1, 1))).collect();
        (g.bag_used, g.bag_slots, g.misc_used, g.misc_slots, g.bank_used, g.bank_slots) = (items as u32, 40, 40, 100, 120, 200);
    }

    fn rail(screen: &str) -> Vec<(String, usize, usize)> {
        let mut sections: Vec<(String, usize, usize)> = Vec::new();
        for line in screen.lines().skip_while(|l| !l.contains('┬')).skip(1) {
            let Some((_, text)) = line.rsplit_once('│') else { continue };
            match (text.trim(), sections.last_mut()) {
                ("", _) => {}
                (more, Some(last)) if more.starts_with('+') => last.2 = more[1..].parse().unwrap(),
                (_, Some(last)) if text.starts_with("   ") => last.1 += 1,
                (heading, _) => sections.push((heading.trim_start_matches('▌').to_string(), 0, 0)),
            }
        }
        sections
    }

    fn fleet(accounts: &[&str]) -> (Fleet, Arc<Hub>) {
        let cells: Vec<Arc<AccountCell>> = accounts.iter().map(|name| Arc::new(AccountCell::mirror(name))).collect();
        let hub = Arc::new(Hub::new("aqx", cells.clone(), Print::Quiet));
        (Fleet { title: "aqx".into(), cells: Arc::new(Cells::new(cells)), socket: PathBuf::new(), home: PathBuf::new() }, hub)
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
            "MP ████████ 100/100  Lv 100  Barber rank 5  12.3M gold",
            "Auras Corruption x2",
            "Stats 1 kill  0 drops  0 deaths",
            "d less",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
        assert!(!shown.contains("Here ally"), "players nearby leave the header for their own list: {shown}");
        let rows = rows(&fleet, &mut app, 110);
        let you = rows.iter().find(|r| r.iter().map(|c| c.symbol()).collect::<String>().contains("Barber")).unwrap();
        let dim: String = you.iter().filter(|c| c.fg == Color::DarkGray).map(|c| c.symbol()).collect();
        assert_eq!(dim, "HP ░░░░MP Lv rank  gold", "labels are dim so the values stand apart");
    }

    #[test]
    fn the_header_keeps_its_shape_as_the_target_and_hp_change() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        let set = |target: Option<&str>, hp: u32| {
            let mut board = cell.board();
            let g = board.snapshot.as_mut().and_then(|s| s.game.as_mut()).expect("a published game");
            g.target = target.map(|name| crate::hub::Target { name: name.into(), hp: 540, max_hp: 540 });
            g.hp = hp;
        };
        let layout = |app: &mut App| {
            let shown = screen(&fleet, app, 60);
            let divider = shown.lines().position(|l| l.contains("── log")).unwrap_or_else(|| panic!("no divider:\n{shown}"));
            let kills = row(&shown, " kill").find(" kill").unwrap();
            (divider, kills, shown)
        };
        let mut app = App::default();
        for details in [false, true] {
            set(None, 2000);
            let (divider, kills, before) = layout(&mut app);
            assert_eq!(details, before.contains("no target"), "{before}");
            for (target, hp) in [(Some("Ultra Chaos Vordred of the Void"), 2000), (None, 999)] {
                set(target, hp);
                let (moved, shifted, after) = layout(&mut app);
                assert_eq!((moved, shifted), (divider, kills), "before:\n{before}\nafter:\n{after}");
            }
            set(Some("Ultra Chaos Vordred of the Void"), 2000);
            assert!(layout(&mut app).2.contains("Target Ultra Chaos"));
            press(&fleet, &mut app, KeyCode::Char('d'));
        }
    }

    #[test]
    fn a_wide_pane_lists_what_is_nearby_and_held_beside_the_log() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        let mut app = App::default();
        let shown = screen_of(&fleet, &mut app, 150, 30);
        println!("{shown}");
        assert!(row(&shown, "── log · main").contains('┬'), "{shown}");
        for (log, rail) in [
            ("script  Astral Ephemerite", "│ Players 1"),
            ("", "│   ally                        40"),
            ("", "│ Drops 8"),
            ("", "│   Spider Silk               4/10"),
            ("", "│   Slime Sample           10/10 ✓"),
            ("", "│ Inventory 38/40 · misc 40/100 "),
            ("", "│   Fishing Bait           533/999"),
            ("", "│   Master Ranger         equipped"),
            ("", "│ Bank 120/200  not opened"),
        ] {
            assert!(row(&shown, rail).contains(log), "missing {rail:?} beside {log:?}:\n{shown}");
        }
        let after = |has: &str| shown.lines().skip_while(|l| !l.contains(has)).nth(1).unwrap().to_string();
        assert!(after("│ Inventory").contains("│   Master Ranger"), "equipped gear leads the inventory:\n{shown}");
        let headings: Vec<String> = rail(&shown).into_iter().map(|(heading, _, _)| heading).collect();
        assert_eq!(headings.iter().map(|h| h.split(' ').next().unwrap()).collect::<Vec<_>>(), ["Players", "Drops", "Temp", "Inventory", "Bank"]);

        let colored = |app: &mut App, has: &str, color: Color| -> String {
            let rows = rows_of(&fleet, app, 150, 30);
            let heading = rows.iter().find(|r| r.iter().map(|c| c.symbol()).collect::<String>().contains(has)).unwrap_or_else(|| panic!("no {has:?}"));
            heading.iter().filter(|c| c.fg == color).map(|c| c.symbol()).collect()
        };
        assert_eq!(colored(&mut app, "Inventory 38/40", Color::Yellow), "38/40", "a nearly full inventory turns yellow");

        let mut board = cell.board();
        let g = board.snapshot.as_mut().unwrap().game.as_mut().unwrap();
        (g.bag_used, g.misc_used) = (25, 100);
        drop(board);
        assert_eq!(colored(&mut app, "Inventory 25/40 · misc 100/100", Color::Red), "100/100", "the counts come from each pool, and a full one turns red");
    }

    #[test]
    fn the_pane_keeps_a_column_clear_of_the_right_edge() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        let shown = screen_of(&fleet, &mut App::default(), 160, 30);
        assert!(row(&shown, "Fishing Bait").ends_with("533/999 "), "{shown}");
        assert!(shown.lines().all(|l| l.ends_with(' ')), "{shown}");
    }

    #[test]
    fn a_short_rail_keeps_every_heading_and_counts_what_it_cuts() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        cell.board().snapshot.as_mut().unwrap().game.as_mut().unwrap().cell_players = (0..11).map(|i| Player { name: format!("player{i}"), level: 40 }).collect();
        let mut app = App::default();
        let expected = [("Players", 11), ("Drops", 8), ("Temp", 2), ("Inventory", 38), ("Bank", 0)];
        for height in [40, 30, 20, 15, 13] {
            let shown = screen_of(&fleet, &mut app, 150, height);
            let sections = rail(&shown);
            let found: Vec<(&str, usize)> = sections.iter().map(|(heading, shown, more)| (heading.split(' ').next().unwrap(), shown + more)).collect();
            let wanted: Vec<(&str, usize)> = expected.iter().map(|&(name, n)| (name, if sections.iter().any(|s| s.0.starts_with(name) && s.1 + s.2 > 0) { n } else { 0 })).collect();
            assert_eq!(found, wanted, "at {height} rows every heading shows and each list's rows add up to its count:\n{shown}");
        }
        let short = screen_of(&fleet, &mut app, 150, 13);
        assert!(!short.lines().skip_while(|l| !l.contains('┬')).any(|l| l.ends_with("│ ") || l.trim_end().ends_with('│')), "a short rail drops the blank lines first:\n{short}");
    }

    #[test]
    fn v_opens_each_list_in_place_of_the_log_and_esc_goes_back() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        let mut app = App { marked: BTreeSet::from([0]), ..App::default() };
        let press_v = |app: &mut App| press(&fleet, app, KeyCode::Char('v'));

        press_v(&mut app);
        let players = screen_of(&fleet, &mut app, 150, 30);
        assert!(players.contains("── players 1 ─"), "{players}");
        assert!(row(&players, "  ally ").contains("40  ") && row(&players, "  ally ").contains("│▌Players 1"), "{players}");
        assert!(!players.contains("Profanity filter"), "the list takes the log's place: {players}");
        assert!(players.contains("v next list  esc back to log"), "{players}");

        for _ in 0..3 {
            press_v(&mut app);
        }
        let inventory = screen_of(&fleet, &mut app, 150, 30);
        println!("{inventory}");
        assert!(inventory.contains("── inventory 38/40 · misc 40/100 · equipped first, then last changed ─"), "{inventory}");
        let first = inventory.lines().skip_while(|l| !l.contains("── inventory")).nth(1).unwrap();
        assert!(first.starts_with("   Master Ranger") && first.contains("equipped"), "equipped items lead the full list: {inventory}");
        assert_eq!(inventory.matches("Spare Part").count(), 33, "a 40-slot inventory fits without scrolling: {inventory}");
        assert!(row(&inventory, "Rogue").contains("rank 4") && !inventory.contains("21980"), "a class shows its rank, not its points: {inventory}");
        assert!(row(&inventory, "Fishing Bait").contains("533/999") && row(&inventory, "Paint").contains("99/99 ✓"));
        assert!(!rail(&inventory).iter().any(|(heading, shown, _)| heading.starts_with("Inventory") && *shown > 0), "the open list shrinks to its heading in the rail: {inventory}");

        press_v(&mut app);
        assert!(screen_of(&fleet, &mut app, 150, 30).contains("aqx lists the bank once a script opens it."));
        press_v(&mut app);
        assert!(screen_of(&fleet, &mut app, 150, 30).contains("── log · main"), "v after the bank returns to the log");

        press_v(&mut app);
        press(&fleet, &mut app, KeyCode::Esc);
        assert!(app.list.is_none() && app.marked.len() == 1, "esc closes the list first");
        press(&fleet, &mut app, KeyCode::Esc);
        assert!(app.marked.is_empty(), "then clears the marks");
        press_v(&mut app);
        press(&fleet, &mut app, KeyCode::Char('l'));
        assert!(app.list.is_none() && app.filter == Filter::Main, "l goes back to the log without changing its filter");
    }

    #[test]
    fn the_drops_list_marks_drops_and_enter_accepts_them() {
        let control = aqx_core::Control::new().unwrap();
        let cell = Arc::new(AccountCell::local("main", control.stop_handle()));
        let name = format!("tui-drops-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, vec![cell.clone()], Print::Quiet));
        let (_, listener, _listening) = socket::bind(&name, false).unwrap();
        socket::serve(listener, hub.clone(), None).unwrap();
        let fleet = Fleet { title: name.clone(), cells: Arc::new(Cells::new(vec![cell.clone()])), socket: socket::pick(Some(&name)).unwrap().1, home: PathBuf::new() };
        publish(&hub, &cell, &farm_world());
        stock(&cell, 10);
        let mut app = App { list: Some(Section::Drops), ..App::default() };
        let keys = |app: &mut App, codes: &[KeyCode]| codes.iter().for_each(|&code| press(&fleet, app, code));
        let note = |app: &App| app.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();

        let shown = screen_of(&fleet, &mut app, 150, 30);
        assert!(row(&shown, "Spider Drop 0").starts_with(" ▌ Spider Drop 0"), "the cursor starts on the newest drop: {shown}");
        assert!(shown.contains("↑↓ pick  space mark  a mark all  enter accept  v next list  esc back to log"), "{shown}");
        keys(&mut app, &[KeyCode::Char(' '), KeyCode::Down, KeyCode::Down, KeyCode::Char(' ')]);
        let marked = screen_of(&fleet, &mut app, 150, 30);
        assert!(row(&marked, "Spider Drop 0").starts_with("  ◆Spider Drop 0") && row(&marked, "Spider Drop 2").starts_with(" ▌◆Spider Drop 2"), "{marked}");
        assert!(row(&marked, "Spider Drop 1").starts_with("   Spider Drop 1"), "{marked}");
        assert!(marked.contains("── drops 8 · last changed first · 2 marked") && marked.contains("enter accept marked"), "{marked}");

        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(note(&app), "main: picking up 2 drops");
        assert!(app.picked.is_empty() && app.list == Some(Section::Drops), "accepting clears the marks and keeps the list open");
        keys(&mut app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(note(&app), "main: picking up Spider Drop 3", "with nothing marked, enter takes the drop under the cursor");

        keys(&mut app, &[KeyCode::Char('a')]);
        assert_eq!(app.picked.len(), 8);
        keys(&mut app, &[KeyCode::Esc]);
        assert!(app.picked.is_empty() && app.list == Some(Section::Drops), "esc clears the marks first");
        keys(&mut app, &[KeyCode::Esc]);
        assert!(app.list.is_none());
    }

    #[test]
    fn a_long_list_scrolls_with_the_log_keys() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 10);
        cell.board().snapshot.as_mut().unwrap().game.as_mut().unwrap().bank =
            Some((0..300).map(|i| Stack { name: format!("Banked {i:03}"), qty: 1, max: 1, ..Stack::default() }).collect());
        let mut app = App { list: Some(Section::Bank), ..App::default() };
        let top = screen_of(&fleet, &mut app, 150, 30);
        println!("{top}");
        let below: usize = row(&top, "── bank 120/200").split('↓').nth(1).and_then(|n| n.split(' ').next()?.parse().ok()).unwrap_or_else(|| panic!("no ↓ count: {top}"));
        assert!(top.contains("Banked 000") && !top.contains("Banked 299") && below > 0, "{top}");
        assert!(top.contains("PgUp PgDn scroll"), "{top}");
        press(&fleet, &mut app, KeyCode::PageDown);
        assert!(screen_of(&fleet, &mut app, 150, 30).contains(&format!("· ↑10 · ↓{} ", below - 10)));
        press(&fleet, &mut app, KeyCode::End);
        let end = screen_of(&fleet, &mut app, 150, 30);
        assert!(end.contains("Banked 299") && !end.contains("Banked 000") && end.contains(&format!("· ↑{below} ─")), "{end}");
        app.mouse(pointer(MouseEventKind::ScrollUp, 50, 15), &[]);
        assert_eq!(app.list_at.get(), below - WHEEL_LINES);
        press(&fleet, &mut app, KeyCode::Home);
        assert!(screen_of(&fleet, &mut app, 150, 30).contains("Banked 000"));
        assert_eq!(app.scroll, 0, "the log keeps its own place");
    }

    #[test]
    fn clicking_a_section_in_the_rail_opens_its_list() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 4);
        let mut app = App::default();
        let shown = screen_of(&fleet, &mut app, 150, 34);
        let at = |shown: &str, has: &str| shown.lines().position(|l| l.contains(has)).unwrap_or_else(|| panic!("no {has:?}:\n{shown}")) as u16;
        let (x, heading, item) = (130, at(&shown, "│ Drops 8"), at(&shown, "Spider Drop 4"));
        let click = |app: &mut App, y| {
            screen_of(&fleet, app, 150, 34);
            app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), x, y), &[]);
            screen_of(&fleet, app, 150, 34)
        };
        assert!(click(&mut app, item).contains("── drops 8 · last changed first"), "clicking a row opens its section");
        assert!(click(&mut app, heading).contains("── log · main"), "clicking the open section's heading closes it");
        assert!(click(&mut app, heading).contains("── drops 8 · last changed first"));
        click(&mut app, heading);
        assert!(shown.lines().nth(32).unwrap().trim_end().ends_with('│'), "{shown}");
        click(&mut app, 32);
        assert!(app.list.is_none(), "the empty space under the rail opens nothing");
        app.grid = true;
        screen_of(&fleet, &mut app, 150, 34);
        app.mouse(pointer(MouseEventKind::Down(MouseButton::Left), x, heading), &[]);
        assert!(app.list.is_none(), "the dashboard has no rail to click");
    }

    #[test]
    fn a_narrow_pane_puts_the_counts_on_the_divider() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        let mut app = App::default();
        let shown = screen_of(&fleet, &mut app, 100, 24);
        assert!(row(&shown, "── log · main ── players 1 · drops 8 · temp 2 · inventory 38/40 · misc 40/100 · bank 120/200 ─").ends_with("─ "), "{shown}");
        assert!(!shown.contains('┬'), "{shown}");
        press(&fleet, &mut app, KeyCode::Char('v'));
        press(&fleet, &mut app, KeyCode::Char('v'));
        let drops = screen_of(&fleet, &mut app, 100, 24);
        assert!(row(&drops, "── players 1 · drops 8 · temp 2").ends_with("─ ") && drops.contains("Spider Drop 7"), "the counts double as tabs: {drops}");
        assert!(!drops.contains("log ·"), "{drops}");
        let rows = rows_of(&fleet, &mut app, 100, 24);
        let tab: String = rows[6].iter().filter(|c| c.fg == ACCENT).map(|c| c.symbol()).collect();
        assert_eq!(tab, "drops 8", "the open list's tab is highlighted");
    }

    #[test]
    fn shift_b_hides_the_side_panel_and_moves_its_counts_to_the_divider() {
        let (fleet, hub) = fleet(&["main"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        stock(cell, 38);
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('B'));
        let hidden = screen_of(&fleet, &mut app, 150, 30);
        assert!(!hidden.contains('┬') && !hidden.contains("│ Players"), "{hidden}");
        assert!(hidden.contains("── log · main ── players 1 · drops 8 · temp 2 · inventory 38/40"), "{hidden}");
        press(&fleet, &mut app, KeyCode::Char('B'));
        assert!(screen_of(&fleet, &mut app, 150, 30).contains("│ Players 1"));
    }

    #[test]
    fn a_card_warns_when_the_inventory_is_nearly_full() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        let cell = &fleet.cells.all()[0];
        let mut app = App { grid: true, ..App::default() };
        for (items, warning) in [(30, None), (36, Some("inventory 36/40 nearly full")), (40, Some("inventory 40/40 full"))] {
            publish(&hub, cell, &farm_world());
            stock(cell, items);
            let shown = screen_of(&fleet, &mut app, 120, 24);
            match warning {
                Some(warning) => assert!(shown.contains(warning), "{shown}"),
                None => assert!(!shown.contains("inventory 3"), "{shown}"),
            }
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
        assert_eq!(app.query.value(), "heal", "tab completes as far as every match agrees");
        typed(&mut app, "e");
        press(&fleet, &mut app, KeyCode::Tab);
        assert_eq!(app.query.value(), "healer", "and the whole name once one is left");
        press(&fleet, &mut app, KeyCode::Esc);
        assert!(!app.querying && app.query.value().is_empty() && list(&mut app)[4].contains("Healbot"), "esc clears the filter");

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
        let indent = " ".repeat(1 + time_of_day(0).len() + 10);
        assert!(rows.len() >= 3 && rows[1..].iter().all(|r| r.starts_with(&indent)), "{shown}");
        let words = shown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("the end") && shown.contains("    at main (farm.ts:2:5)"), "{shown}");
    }

    #[test]
    fn a_long_running_session_keeps_every_character_of_a_wrapped_line() {
        let ten_hours = 36_000_000;
        let line = LogLine { seq: 0, t: ten_hours, source: "script".into(), text: format!("error: {}the end", "word ".repeat(30)) };
        let rows: Vec<Line> = log_rows(&line, 60, true);
        let text: Vec<String> = rows.iter().map(Line::to_string).collect();
        assert!(text[0].starts_with(&format!("{}  script  error: word", time_of_day(ten_hours))), "{text:#?}");
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
    fn the_run_prompt_edits_anywhere_in_the_line() {
        let (fleet, _hub) = fleet(&["main"]);
        let mut app = App::default();
        let key = |app: &mut App, code: KeyCode, modifiers: KeyModifiers| {
            let shown = gather(&fleet, app);
            app.handle(KeyEvent::new(code, modifiers), &fleet, &shown);
        };
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| press(&fleet, app, KeyCode::Char(c)));
        let line = |app: &mut App| screen(&fleet, app, 100).lines().find(|l| l.contains(" run on ")).unwrap_or_default().to_string();

        press(&fleet, &mut app, KeyCode::Char('o'));
        typed(&mut app, "/srv/frm.ts rounds=3");
        press(&fleet, &mut app, KeyCode::Home);
        (0..6).for_each(|_| press(&fleet, &mut app, KeyCode::Right));
        typed(&mut app, "a");
        assert!(line(&mut app).contains(": /srv/farm.ts rounds=3 ") && !line(&mut app).contains('█'), "{}", line(&mut app));
        press(&fleet, &mut app, KeyCode::End);
        assert!(line(&mut app).contains(": /srv/farm.ts rounds=3█"), "{}", line(&mut app));
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert!(line(&mut app).contains(": /srv/farm.ts rounds=█"), "ctrl+w deletes back to the =: {}", line(&mut app));
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(line(&mut app).contains(": █"), "ctrl+u clears the line: {}", line(&mut app));
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
                    app.prompt.as_ref().unwrap().input.value().to_string()
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
    fn the_prompt_finds_scripts_from_the_config_folder_and_the_next_session_recalls_them() {
        let dir = std::env::temp_dir().join(format!("aqx-tui-history-{}", std::process::id()));
        let home = dir.join("cfg");
        std::fs::create_dir_all(home.join("scripts")).unwrap();
        for name in ["scripts/a.ts", "scripts/b.ts"] {
            std::fs::write(home.join(name), "").unwrap();
        }
        let outside = dir.join("c.ts");
        std::fs::write(&outside, "").unwrap();
        let history = dir.join("state/history");
        let (mut fleet, _hub) = fleet(&["main"]);
        fleet.home = home.clone();
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| press(&fleet, app, KeyCode::Char(c)));
        let mut first = App::remembering(history.clone(), &home);
        press(&fleet, &mut first, KeyCode::Char('o'));
        typed(&mut first, "scripts/a");
        press(&fleet, &mut first, KeyCode::Tab);
        typed(&mut first, "rounds=3");
        press(&fleet, &mut first, KeyCode::Enter);
        for command in ["scripts/b.ts".to_string(), outside.display().to_string(), "scripts/a.ts rounds=3".to_string()] {
            press(&fleet, &mut first, KeyCode::Char('o'));
            typed(&mut first, &command);
            press(&fleet, &mut first, KeyCode::Enter);
        }
        let walk = |home: &Path| {
            let mut app = App::remembering(history.clone(), home);
            press(&fleet, &mut app, KeyCode::Char('o'));
            (0..3)
                .map(|_| {
                    press(&fleet, &mut app, KeyCode::Up);
                    app.prompt.as_ref().unwrap().input.value().to_string()
                })
                .collect::<Vec<_>>()
        };
        let outside = outside.display().to_string();
        assert_eq!(walk(&home), ["scripts/a.ts rounds=3", outside.as_str(), "scripts/b.ts"], "newest first, each command once");
        assert_eq!(walk(&dir)[0], "cfg/scripts/a.ts rounds=3", "another config folder still finds the script");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&history).unwrap().permissions().mode() & 0o777, 0o600);
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
        let fleet = Fleet { title: name.clone(), cells: Arc::new(Cells::new(cells.clone())), socket: socket::pick(Some(&name)).unwrap().1, home: PathBuf::new() };
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
        assert_eq!(app.recent.last().map(|r| &r.typed), Some(&command), "answers stay out of the history");
        run(&mut app);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target: Bob█"), "the choice and the last answer stick");
        typed(&mut app, "Ann");
        for _ in 0..3 {
            press(&fleet, &mut app, KeyCode::Enter);
        }
        assert_eq!((ran(&cells[0]), ran(&cells[1])), ("rounds=3 target=Ann".into(), "rounds=3 target=Ann".into()), "typing replaces a recalled answer");
        run(&mut app);
        press(&fleet, &mut app, KeyCode::Backspace);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target: █"), "backspace clears a recalled answer");
        press(&fleet, &mut app, KeyCode::Tab);
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target for main: Bob Smith█"), "each account recalls its own answer");
        press(&fleet, &mut app, KeyCode::Left);
        typed(&mut app, "x");
        assert!(screen(&fleet, &mut app, 120).contains(" follow.ts needs target for main: Bob Smitxh "), "moving the cursor keeps a recalled answer to edit");
        press(&fleet, &mut app, KeyCode::Esc);

        let spaced = "target=Bob Smith rounds=3 class=Dark Caster wait=5";
        press(&fleet, &mut app, KeyCode::Char('o'));
        typed(&mut app, &format!("{} {spaced}", script.display()));
        press(&fleet, &mut app, KeyCode::Enter);
        assert_eq!((ran(&cells[0]), ran(&cells[1])), (spaced.into(), spaced.into()), "a value runs to the next KEY=");
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
        let fleet = Fleet { title: name.clone(), cells: Arc::new(Cells::new(cells.clone())), socket: socket::pick(Some(&name)).unwrap().1, home: PathBuf::new() };
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
    fn redact_shows_each_account_by_number_in_the_header_and_the_log() {
        let (fleet, hub) = fleet(&["sam"]);
        let cell = &fleet.cells.all()[0];
        publish(&hub, cell, &farm_world());
        hub.record(cell, &Event::ServerText { text: "Hero joined; sam waved".into() });
        let shown = screen(&fleet, &mut App { redact: true, ..App::default() }, 110);
        assert!(shown.contains(" account 1 · Twilly · farm-1"), "{shown}");
        assert!(shown.contains("account 1 joined; account 1 waved"), "{shown}");
        assert!(!shown.contains("sam") && !shown.to_lowercase().contains("hero"), "{shown}");
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
        let input = |app: &App| app.prompt.as_ref().unwrap().input.value().to_string();

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

        app.prompt = Some(Prompt::new(&fleet.home));
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
        let matches = scripts_matching(&format!("~/{name}/f"), Path::new(""));
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
