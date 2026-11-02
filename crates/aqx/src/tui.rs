use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::account::{expand_home, script_path};
use crate::hub::{Bar, AccountCell, Command, Game, LOG_LINES, LogLine, Snapshot};
use crate::output::{clock, span};
use crate::socket;

const WIDE: u16 = 90;
const NOTE_FOR: Duration = Duration::from_secs(4);
const STATE_WIDTH: usize = 12;
const ACCENT: Color = Color::Blue;
const CARD_WIDTH: u16 = 44;
const CARD_HEIGHT: u16 = 8;

type Field = Vec<Span<'static>>;

const ACCOUNT_KEYS: [(&str, &str); 11] = [
    ("", "Accounts"),
    ("↑ ↓", "pick"),
    ("tab", "next"),
    ("space", "mark"),
    ("a", "mark all / none"),
    ("esc", "clear marks"),
    ("i", "log in"),
    ("x", "log out"),
    ("q", "detach"),
    ("Q", "log out all and end"),
    ("ctrl+c", "end or detach"),
];

const SCRIPT_KEYS: [(&str, &str); 12] = [
    ("", "Scripts"),
    ("o", "run a script"),
    ("s", "stop"),
    ("r", "restart"),
    ("p", "pause / resume"),
    ("", "View"),
    ("g", "dashboard"),
    ("d", "more details"),
    ("l", "filter the log"),
    ("⇧↑ ⇧↓", "scroll a line"),
    ("PgUp PgDn", "scroll a page"),
    ("End", "newest lines"),
];

pub struct Fleet {
    pub title: String,
    pub cells: Vec<Arc<AccountCell>>,
    pub socket: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Filter {
    #[default]
    All,
    Aqx,
    Script,
    Server,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Aqx,
            Filter::Aqx => Filter::Script,
            Filter::Script => Filter::Server,
            Filter::Server => Filter::All,
        }
    }

    fn shows(self, source: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Aqx => source == "aqx",
            Filter::Script => source == "script",
            Filter::Server => source == "server",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Aqx => "aqx",
            Filter::Script => "script",
            Filter::Server => "server",
        }
    }
}

struct Prompt {
    input: String,
    pick: Option<usize>,
    matches: Vec<String>,
}

impl Prompt {
    fn new() -> Self {
        let mut prompt = Prompt { input: String::new(), pick: None, matches: Vec::new() };
        prompt.find_matches();
        prompt
    }

    fn find_matches(&mut self) {
        self.matches = scripts_matching(&self.input);
    }
}

#[derive(Default)]
struct App {
    selected: usize,
    marked: BTreeSet<usize>,
    details: bool,
    grid: bool,
    keys: bool,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    list_top: Cell<usize>,
    grid_top: Cell<usize>,
    grid_cols: Cell<usize>,
    prompt: Option<Prompt>,
    ending: bool,
    recent: Vec<String>,
    note: Option<(String, Instant)>,
    log: Vec<LogLine>,
    log_of: Option<usize>,
}

struct Shown {
    name: String,
    snapshot: Option<Snapshot>,
}

pub fn run(fleet: &Fleet, done: impl Fn() -> bool) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let mut app = App::default();
    loop {
        let shown = gather(fleet, &mut app);
        for script in shown.iter().filter_map(|s| s.snapshot.as_ref()?.script.clone()) {
            if !app.recent.contains(&script) {
                app.recent.push(script);
            }
        }
        let _ = terminal.draw(|f| draw(f, fleet, &app, &shown));
        if done() {
            break;
        }
        if !event::poll(Duration::from_millis(250)).unwrap_or(false) {
            continue;
        }
        let Ok(Input::Key(key)) = event::read() else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.handle(key, fleet, &shown) {
            break;
        }
    }
    ratatui::restore();
    Ok(())
}

fn gather(fleet: &Fleet, app: &mut App) -> Vec<Shown> {
    if app.log_of != Some(app.selected) {
        app.log.clear();
        app.log_of = Some(app.selected);
    }
    fleet
        .cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let board = cell.board();
            if i == app.selected {
                let from = app.log.last().map_or(0, |l| l.seq + 1);
                let fresh = board.log.iter().rev().take_while(|l| l.seq >= from).count();
                app.log.extend(board.log.iter().skip(board.log.len() - fresh).cloned());
                let over = app.log.len().saturating_sub(LOG_LINES);
                app.log.drain(..over);
            }
            Shown { name: cell.name.clone(), snapshot: board.snapshot.clone() }
        })
        .collect()
}

fn snapshot(shown: &[Shown], i: usize) -> Option<&Snapshot> {
    shown.get(i)?.snapshot.as_ref()
}

fn is_offline(s: &Snapshot) -> bool {
    s.phase == "OFFLINE"
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

    fn pick(&mut self, i: usize) {
        if i != self.selected {
            self.selected = i;
            self.scroll = 0;
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), Instant::now() + NOTE_FOR));
    }

    fn act(&mut self, fleet: &Fleet, command: Command, done: &str) {
        self.act_on(self.targets(), fleet, command, done);
    }

    fn act_on(&mut self, accounts: Vec<usize>, fleet: &Fleet, command: Command, done: &str) {
        let (mut sent, mut same, mut said) = (Vec::new(), Vec::new(), Vec::new());
        for i in accounts {
            let Some(cell) = fleet.cells.get(i) else { continue };
            match socket::ctl(&fleet.socket, &cell.name, &command) {
                Ok(true) => sent.push(cell.name.as_str()),
                Ok(false) => same.push(cell.name.as_str()),
                Err(e) => said.push(e),
            }
        }
        if !same.is_empty() {
            said.insert(0, format!("{}: {}", same.join(", "), command.unchanged()));
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
                    self.act_on((0..fleet.cells.len()).collect(), fleet, Command::Logout, "logging out; the session ends once every account is offline");
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
                KeyCode::Backspace => {
                    prompt.input.pop();
                }
                KeyCode::Up | KeyCode::Down if !self.recent.is_empty() => {
                    let n = self.recent.len();
                    let at = match (prompt.pick, key.code) {
                        (None, _) => 0,
                        (Some(i), KeyCode::Up) => (i + 1) % n,
                        (Some(i), _) => (i + n - 1) % n,
                    };
                    prompt.pick = Some(at);
                    prompt.input.clone_from(&self.recent[n - 1 - at]);
                }
                KeyCode::Tab => match prompt.matches.as_slice() {
                    [] => {}
                    [only] if !only.ends_with('/') => prompt.input = format!("{only} "),
                    many => prompt.input = shared_prefix(many).to_string(),
                },
                KeyCode::Char(c) => prompt.input.push(c),
                _ => {}
            }
            if let Some(prompt) = &mut self.prompt {
                prompt.find_matches();
            }
            return false;
        }
        let accounts = fleet.cells.len();
        let last = accounts.saturating_sub(1);
        let line = accounts < 2 || key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('Q') => self.ending = true,
            KeyCode::Char('?') => self.keys = true,
            KeyCode::Char('g') => self.grid = !self.grid,
            KeyCode::Char('d' | 'l') | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End if self.grid => {}
            KeyCode::Enter if self.grid => self.grid = false,
            KeyCode::Left if self.grid => self.pick(self.selected.saturating_sub(1)),
            KeyCode::Right if self.grid => self.pick((self.selected + 1).min(last)),
            KeyCode::Up if self.grid => self.pick(self.selected.saturating_sub(self.grid_cols.get())),
            KeyCode::Down if self.grid => self.pick((self.selected + self.grid_cols.get()).min(last)),
            KeyCode::Up if line => self.scroll = (self.scroll + 1).min(self.max_scroll.get()),
            KeyCode::Down if line => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Up => self.pick(self.selected.saturating_sub(1)),
            KeyCode::Down => self.pick((self.selected + 1).min(last)),
            KeyCode::Tab => self.pick(if self.selected >= last { 0 } else { self.selected + 1 }),
            KeyCode::BackTab => self.pick(if self.selected == 0 { last } else { self.selected - 1 }),
            KeyCode::Char(' ') => {
                if !self.marked.remove(&self.selected) {
                    self.marked.insert(self.selected);
                }
            }
            KeyCode::Char('a') if self.marked.len() == accounts => self.marked.clear(),
            KeyCode::Char('a') => self.marked = (0..accounts).collect(),
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
        self.recent.retain(|r| *r != input);
        self.recent.push(input);
        let shown = path.display().to_string();
        self.act(fleet, Command::Run { script: path, args }, &format!("running {shown}"));
    }
}

fn draw(f: &mut Frame, fleet: &Fleet, app: &App, shown: &[Shown]) {
    let matches = app.prompt.as_ref().map_or(&[][..], |p| &p.matches);
    let [header, body, suggestions, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(!matches.is_empty())),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_matches(f, suggestions, matches);
    let list = !app.grid && body.width >= WIDE && shown.len() > 1;
    draw_header(f, header, fleet, app, shown, list || app.grid);
    if app.grid {
        draw_grid(f, body, app, shown);
    } else if let Some(account) = shown.get(app.selected) {
        let pane = if list {
            let width = (3 + name_width(shown) + 1 + STATE_WIDTH) as u16;
            let [left, _, pane] = Layout::horizontal([Constraint::Length(width + 1), Constraint::Length(1), Constraint::Min(0)]).areas(body);
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
        left.push(Span::raw(format!("  {} {}/{}", account.name, app.selected + 1, shown.len())).dark_gray());
    }
    if !app.marked.is_empty() {
        left.push(Span::raw(format!(" · {} marked", app.marked.len())).dark_gray());
    }
    let mut keys = Vec::new();
    if !overview && shown.len() > 1 {
        keys.push(("tab", "next"));
    }
    if shown.len() > 1 {
        keys.push(("g", "dashboard"));
    }
    keys.extend([("?", "keys"), ("q", "detach")]);
    f.render_widget(Paragraph::new(spread(left, hints(&keys), area.width.saturating_sub(1) as usize)), area);
}

fn name_width(shown: &[Shown]) -> usize {
    shown.iter().map(|s| s.name.chars().count()).max().unwrap_or(0).clamp(4, 14)
}

fn draw_list(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let width = name_width(shown);
    let height = (area.height as usize).max(1);
    let top = app.list_top.get().min(app.selected).max((app.selected + 1).saturating_sub(height));
    app.list_top.set(top);
    let lines: Vec<Line> = shown
        .iter()
        .enumerate()
        .skip(top)
        .take(height)
        .map(|(i, account)| {
            let selected = i == app.selected;
            let name: String = account.name.chars().take(width).collect();
            let name = Span::raw(format!("{name:<width$} "));
            let (state, style) = state_of(account.snapshot.as_ref());
            Line::from(vec![
                Span::raw(if selected { "▌" } else { " " }).fg(ACCENT),
                Span::raw(if app.marked.contains(&i) { "●" } else { " " }).fg(ACCENT),
                Span::raw(" "),
                if selected { name.bold() } else { name },
                Span::styled(state, if selected { style.bold() } else { style }),
            ])
        })
        .collect();
    let rule = Block::new().borders(Borders::RIGHT).border_style(Style::new().dark_gray());
    f.render_widget(Paragraph::new(lines).block(rule), area);
}

fn state_of(snapshot: Option<&Snapshot>) -> (String, Style) {
    let Some(s) = snapshot else { return ("STARTING".into(), Style::new().fg(Color::Yellow)) };
    let state = s.state();
    let color = match state {
        "READY" => Color::Green,
        "OFFLINE" if s.detail.is_empty() => Color::DarkGray,
        "CLOSED" | "OFFLINE" => Color::Red,
        _ => Color::Yellow,
    };
    (state.to_string(), Style::new().fg(color))
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let width = area.width as usize;
    let who = app.who(shown, width / 3);
    if let Some(prompt) = &app.prompt {
        let label = format!(" run on {who}: ");
        let keys = hints(&[("tab", "complete"), ("↑↓", "recent"), ("esc", "cancel")]);
        let typed = vec![Span::raw(label).bold(), Span::raw(prompt.input.clone()), Span::raw("█")];
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
    let row = app.selected / cols;
    let top = app.grid_top.get().min(row).max((row + 1).saturating_sub(rows));
    app.grid_top.set(top);
    for (i, account) in shown.iter().enumerate().skip(top * cols).take(rows * cols) {
        let at = i - top * cols;
        let edge = |col: usize| (usize::from(area.width) * col / cols) as u16;
        let rect = Rect {
            x: area.x + edge(at % cols),
            y: area.y + (at / cols) as u16 * CARD_HEIGHT,
            width: edge(at % cols + 1) - edge(at % cols),
            height: CARD_HEIGHT,
        };
        draw_card(f, rect.intersection(area), account, i == app.selected, app.marked.contains(&i));
    }
}

fn draw_card(f: &mut Frame, area: Rect, account: &Shown, selected: bool, marked: bool) {
    let (state, style) = state_of(account.snapshot.as_ref());
    let state = format!(" {state} ");
    let mut name = Vec::new();
    if marked {
        name.push(Span::raw(" ●").fg(ACCENT));
    }
    name.push(Span::raw(format!(" {} ", account.name)).bold());
    let name = clip(name, usize::from(area.width).saturating_sub(state.len() + 3));
    let (border, color) = if selected { (BorderType::Thick, ACCENT) } else { (BorderType::Plain, Color::DarkGray) };
    let block = Block::bordered()
        .border_type(border)
        .border_style(Style::new().fg(color))
        .title(Line::from(name).left_aligned())
        .title(Line::from(Span::styled(state, style.bold())).right_aligned())
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
    let mut lines = detail(account, app.details, width);
    let log = log_lines(app, (area.height as usize).saturating_sub(lines.len() + 1), width);
    let mut rule = format!("── log · {}", app.filter.label());
    let scrolled = app.scroll.min(app.max_scroll.get());
    if scrolled > 0 {
        rule.push_str(&format!(" · ↑{scrolled}"));
    }
    let fill = width.saturating_sub(rule.chars().count() + 1);
    lines.push(Line::raw(format!("{rule} {}", "─".repeat(fill))).dark_gray());
    lines.extend(log);
    f.render_widget(Paragraph::new(lines), area);
}

fn detail(account: &Shown, full: bool, width: usize) -> Vec<Line<'static>> {
    let Some(s) = &account.snapshot else {
        let (state, style) = state_of(None);
        return vec![
            spread(vec![Span::raw(account.name.clone()).bold()], vec![Span::styled(state, style.bold())], width),
            Line::raw("waiting for the server").dark_gray(),
        ];
    };
    let online = s.phase != "OFFLINE";
    let mut lines = vec![identity(account, s, width)];
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

fn identity(account: &Shown, s: &Snapshot, width: usize) -> Line<'static> {
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
    let (state, style) = state_of(Some(s));
    let mut right = vec![Span::styled(state, style.bold())];
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
    let lines: Vec<&LogLine> = app.log.iter().filter(|l| app.filter.shows(&l.source)).collect();
    let sizes: Vec<usize> = lines.iter().map(|l| pieces(l, width).1.len()).collect();
    let total: usize = sizes.iter().sum();
    app.max_scroll.set(total.saturating_sub(height));
    let end = total - app.scroll.min(app.max_scroll.get());
    let start = end.saturating_sub(height);
    let mut rows = Vec::new();
    let mut at = 0;
    for (line, size) in lines.into_iter().zip(sizes) {
        let (first, last) = (at.max(start), (at + size).min(end));
        if first < last {
            rows.extend(log_rows(line, width).into_iter().skip(first - at).take(last - first));
        }
        at += size;
        if at >= end {
            break;
        }
    }
    rows
}

fn pieces(l: &LogLine, width: usize) -> (String, Vec<&str>) {
    let prefix = clock(l.t);
    let room = width.saturating_sub(prefix.len() + 10).max(10);
    let mut pieces: Vec<&str> = l.text.lines().flat_map(|line| wrap(line, room)).collect();
    if pieces.is_empty() {
        pieces.push("");
    }
    (prefix, pieces)
}

fn log_rows(l: &LogLine, width: usize) -> Vec<Line<'static>> {
    let color = match l.source.as_str() {
        "server" => Color::Yellow,
        "script" => Color::Magenta,
        _ => Color::Cyan,
    };
    let (prefix, pieces) = pieces(l, width);
    let indent = " ".repeat(prefix.len() + 10);
    let mut pieces = pieces.into_iter();
    let mut rows = vec![Line::from(vec![
        Span::raw(prefix).dark_gray(),
        Span::raw("  "),
        Span::styled(format!("{:<6}", l.source), Style::new().fg(color)),
        Span::raw("  "),
        Span::raw(pieces.next().unwrap_or_default().to_string()),
    ])];
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
    let rows = ACCOUNT_KEYS.len().max(SCRIPT_KEYS.len());
    let mut lines: Vec<Line> = (0..rows)
        .map(|i| {
            let mut spans = ACCOUNT_KEYS.get(i).map_or_else(|| vec![Span::raw(" ".repeat(30))], |&k| cell(k, 30));
            spans.extend(SCRIPT_KEYS.get(i).map(|&k| cell(k, 27)).unwrap_or_default());
            Line::from(spans)
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::raw("Keys act on the marked accounts, or the selected one.").dark_gray());
    let rect = centered(area, 61, lines.len() as u16 + 2);
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
    use crate::hub::{Board, Hub, Player, Print, SkillState};

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
        (Fleet { title: "aqx".into(), cells, socket: PathBuf::new() }, hub)
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

    fn show(cell: &AccountCell, snapshot: Snapshot) {
        cell.board().snapshot = Some(snapshot);
    }

    fn row<'a>(screen: &'a str, has: &str) -> &'a str {
        screen.lines().find(|l| l.contains(has)).unwrap_or_else(|| panic!("no row has {has:?}:\n{screen}"))
    }

    #[test]
    fn the_selected_account_shows_its_task_a_summary_and_its_log() {
        let (fleet, hub) = fleet(&["main"]);
        publish(&hub, &fleet.cells[0], &farm_world());
        let mut app = App::default();
        let shown = screen(&fleet, &mut app, 110);
        println!("{shown}");
        assert!(row(&shown, "main (hero) · Twilly · farm-1 Enter/Spawn").contains("READY  up 1m"));
        for expected in [
            "void-aura.ts  Astral Ephemerite Essence in timespace",
            "4120/7500",
            "HP ████░░░░ 2000/4000  1 kill",
            "aqx     killed Mosquito",
            "server  Profanity filter On.",
            "q detach",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
        assert!(!shown.contains("Barber"), "details stay hidden until d");
        let narrow = screen(&fleet, &mut app, 60);
        for expected in ["HP ████░░░░ 2000/4000", "Void Aura", "killed Mosquito", "q detach"] {
            assert!(narrow.contains(expected), "narrow screen is missing {expected:?}");
        }
        for _ in 0..3 {
            press(&fleet, &mut app, KeyCode::Char('l'));
        }
        let shown = screen(&fleet, &mut app, 110);
        assert!(shown.contains("── log · server") && shown.contains("Profanity filter On.") && !shown.contains("killed Mosquito"));
    }

    #[test]
    fn d_shows_every_detail() {
        let (fleet, hub) = fleet(&["main"]);
        publish(&hub, &fleet.cells[0], &farm_world());
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
        publish(&hub, &fleet.cells[0], &farm_world());
        let mut app = App::default();
        let shown = screen(&fleet, &mut app, 120);
        println!("{shown}");
        assert!(row(&shown, "1 of 2 online").contains("? keys  q detach"));
        assert!(row(&shown, "▌  main").contains("READY"));
        assert!(row(&shown, "healer").contains("STARTING"));
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
    fn g_shows_every_account_as_a_card_and_arrows_move_across_the_grid() {
        let (fleet, hub) = fleet(&["main", "healer", "spare", "alt"]);
        publish(&hub, &fleet.cells[0], &farm_world());
        let detail = "this account logged in somewhere else";
        show(&fleet.cells[2], Snapshot { phase: "OFFLINE".into(), detail: detail.into(), ..Snapshot::default() });
        let mut app = App::default();
        press(&fleet, &mut app, KeyCode::Char('g'));
        let shown = screen(&fleet, &mut app, 140);
        println!("{shown}");
        assert!(row(&shown, "1 of 4 online").contains("g dashboard"));
        assert!(row(&shown, "┏ main").contains("READY") && row(&shown, "┏ main").contains("┌ healer"), "{shown}");
        assert!(row(&shown, "spare").contains("OFFLINE") && row(&shown, "spare").contains("healer"), "three cards fit across 140 columns: {shown}");
        for expected in ["hero · Twilly · up 1m", "farm-1 Enter/Spawn", "void-aura.ts  Astral", "HP ████░░░░ 2000/4000  1 kill", "Void Aura ", "waiting for the server", "this account logged in", "enter open"] {
            assert!(shown.contains(expected), "missing {expected:?}: {shown}");
        }
        assert!(!shown.contains("── log"));
        for key in ['d', 'l'] {
            press(&fleet, &mut app, KeyCode::Char(key));
        }
        press(&fleet, &mut app, KeyCode::PageUp);
        assert!(!app.details && app.filter == Filter::All && app.scroll == 0, "view keys for the hidden pane do nothing on the dashboard");
        press(&fleet, &mut app, KeyCode::Down);
        assert_eq!(app.selected, 3, "down moves one row of three cards");
        press(&fleet, &mut app, KeyCode::Left);
        press(&fleet, &mut app, KeyCode::Up);
        assert_eq!(app.selected, 0);
        press(&fleet, &mut app, KeyCode::Right);
        press(&fleet, &mut app, KeyCode::Enter);
        assert!(screen(&fleet, &mut app, 140).contains("── log"), "enter opens the picked account");
        assert_eq!(app.selected, 1);
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
        assert!(row(&shown, "┏ alt16").contains("STARTING"), "{shown}");
        assert!(!shown.contains("alt00"));
    }

    #[test]
    fn a_narrow_terminal_hides_the_list_and_tab_switches_accounts() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        publish(&hub, &fleet.cells[0], &farm_world());
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
        show(&fleet.cells[0], Snapshot { account: "spare".into(), phase: "OFFLINE".into(), detail: detail.into(), ..Snapshot::default() });
        let shown = screen(&fleet, &mut App::default(), 100);
        assert!(row(&shown, "spare").contains("OFFLINE"));
        assert!(shown.contains(detail) && shown.contains("i log in"), "{shown}");
    }

    #[test]
    fn a_crowded_cell_fits_one_line_and_counts_the_rest() {
        let (fleet, _hub) = fleet(&["main"]);
        let players = (0..11).map(|i| Player { name: format!("player{i}"), level: 40 + i }).collect();
        let game = Game { cell_players: players, ..Game::default() };
        show(&fleet.cells[0], Snapshot { phase: "READY".into(), game: Some(game), ..Snapshot::default() });
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
            hub.say(&fleet.cells[0], "aqx", format!("line {i}"));
        }
        let mut app = App { scroll: 500, ..App::default() };
        let shown = screen(&fleet, &mut app, 100);
        assert!(shown.contains("line 0") && shown.contains("line 4"));
    }

    #[test]
    fn long_log_lines_wrap_under_the_message() {
        let (fleet, hub) = fleet(&["main"]);
        let long = format!("error: {}the end", "word ".repeat(30));
        hub.say(&fleet.cells[0], "script", format!("{long}\n    at main (farm.ts:2:5)"));
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
        let rows: Vec<Line> = log_rows(&line, 60);
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
        let rows = log_rows(&line, 60);
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
        assert!(row(&shown, "▌  alt25").contains("STARTING"), "{shown}");
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
        show(&fleet.cells[0], Snapshot { phase: "READY".into(), server: "Twilight".into(), up_secs: 3720, game: Some(game), ..Snapshot::default() });
        let shown = screen(&fleet, &mut App::default(), 60);
        assert!(row(&shown, "main").contains("…  READY  up 1h02m"), "{shown}");
    }

    #[test]
    fn arrows_scroll_the_log_when_there_is_nothing_to_pick() {
        let (one, hub) = fleet(&["main"]);
        for i in 0..50 {
            hub.say(&one.cells[0], "aqx", format!("line {i}"));
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
    fn an_account_shows_starting_before_the_server_answers() {
        let (fleet, _hub) = fleet(&["main"]);
        let shown = screen(&fleet, &mut App::default(), 100);
        for expected in ["STARTING", "waiting for the server", "── log · all"] {
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
        let fleet = Fleet { title: name.clone(), cells: cells.clone(), socket: socket::pick(Some(&name)).unwrap().1 };
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
        hub.say(&fleet.cells[0], "aqx", "first");
        gather(&fleet, &mut app);
        for i in 0..LOG_LINES {
            hub.say(&fleet.cells[0], "aqx", format!("line {i}"));
        }
        gather(&fleet, &mut app);
        assert_eq!(app.log.len(), LOG_LINES);
        assert_eq!(app.log.last().map(|l| l.text.as_str()), Some(format!("line {}", LOG_LINES - 1).as_str()));
        assert_eq!(app.log.first().map(|l| l.seq), Some(1));
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
        publish(&hub, &fleet.cells[0], &farm_world());
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
        show(&fleet.cells[0], Snapshot { phase: "READY".into(), game: Some(game), ..Snapshot::default() });
        let mut app = App { details: true, ..App::default() };
        let rows = rows(&fleet, &mut app, 60);
        let row = rows.iter().find(|r| r.iter().map(|c| c.symbol()).collect::<String>().contains("Skills")).unwrap();
        let text: String = row.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Skills 1 Slash  2 Blur  3 Rend"), "{text}");
        let dim: String = row.iter().filter(|c| c.fg == Color::DarkGray).map(|c| c.symbol()).collect();
        assert_eq!(dim, "Skills lur3 Rend");
    }
}
