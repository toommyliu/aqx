use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell as TableCell, Padding, Paragraph, Row, Table};
use crate::hub::{BotCell, Command, LOG_LINES, LogLine, Snapshot};
use crate::output::{clock, span};
use crate::socket;

const WIDE: u16 = 90;
const NOTE_FOR: Duration = Duration::from_secs(4);

pub enum Commander {
    Local,
    Remote(PathBuf),
}

pub struct Fleet {
    pub title: String,
    pub cells: Vec<Arc<BotCell>>,
    pub commander: Commander,
}

impl Fleet {
    fn send(&self, cell: &BotCell, command: Command) -> Result<(), String> {
        match &self.commander {
            Commander::Local => cell.send(command),
            Commander::Remote(path) => socket::ctl(path, &cell.name, &command),
        }
    }

    fn remote(&self) -> bool {
        matches!(self.commander, Commander::Remote(_))
    }
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

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Run,
    Chat,
}

struct Prompt {
    ask: Ask,
    input: String,
    pick: Option<usize>,
    matches: Vec<String>,
}

impl Prompt {
    fn new(ask: Ask) -> Self {
        let mut prompt = Prompt { ask, input: String::new(), pick: None, matches: Vec::new() };
        prompt.find_matches();
        prompt
    }

    fn find_matches(&mut self) {
        self.matches = if self.ask == Ask::Run { scripts_matching(&self.input) } else { Vec::new() };
    }
}

#[derive(Default)]
struct App {
    open: Option<usize>,
    selected: usize,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    prompt: Option<Prompt>,
    recent: Vec<String>,
    note: Option<(String, Instant)>,
    quitting: bool,
    leave: bool,
    log: Vec<LogLine>,
    log_of: Option<usize>,
}

struct Shown {
    name: String,
    snapshot: Option<Snapshot>,
}

pub fn run(fleet: Fleet, done: impl Fn() -> bool, quit: impl Fn()) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let mut app = App { open: (fleet.cells.len() == 1).then_some(0), ..App::default() };
    loop {
        let shown = gather(&fleet, &mut app);
        for script in shown.iter().filter_map(|s| s.snapshot.as_ref()?.script.clone()) {
            if !app.recent.contains(&script) {
                app.recent.push(script);
            }
        }
        let _ = terminal.draw(|f| draw(f, &fleet, &app, &shown));
        if done() || app.leave {
            break;
        }
        if !event::poll(Duration::from_millis(250)).unwrap_or(false) {
            continue;
        }
        let Ok(Input::Key(key)) = event::read() else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.handle(key, &fleet, &shown) && !app.quitting {
            if fleet.remote() {
                app.leave = true;
            } else {
                app.quitting = true;
                app.note = Some(("logging out".into(), Instant::now() + Duration::from_secs(3600)));
                quit();
            }
        }
    }
    ratatui::restore();
    Ok(())
}

fn gather(fleet: &Fleet, app: &mut App) -> Vec<Shown> {
    if app.log_of != app.open {
        app.log.clear();
        app.log_of = app.open;
    }
    fleet
        .cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let board = cell.board();
            if app.open == Some(i) {
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

impl App {
    fn current(&self) -> usize {
        self.open.unwrap_or(self.selected)
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), Instant::now() + NOTE_FOR));
    }

    fn act(&mut self, fleet: &Fleet, command: Command, done: &str) {
        let Some(cell) = fleet.cells.get(self.current()) else { return };
        match fleet.send(cell, command) {
            Ok(()) => self.say(format!("{}: {done}", cell.name)),
            Err(e) => self.say(e),
        }
    }

    fn handle(&mut self, key: KeyEvent, fleet: &Fleet, shown: &[Shown]) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
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
                KeyCode::Up | KeyCode::Down if prompt.ask == Ask::Run && !self.recent.is_empty() => {
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
        let many = fleet.cells.len() > 1;
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Enter if self.open.is_none() => {
                self.open = Some(self.selected);
                self.scroll = 0;
            }
            KeyCode::Esc if many => self.open = None,
            KeyCode::Tab if many => {
                let next = (self.current() + 1) % fleet.cells.len();
                self.selected = next;
                if self.open.is_some() {
                    self.open = Some(next);
                    self.scroll = 0;
                }
            }
            KeyCode::Up if self.open.is_none() => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down if self.open.is_none() => self.selected = (self.selected + 1).min(fleet.cells.len().saturating_sub(1)),
            KeyCode::Up => self.scroll = (self.scroll + 1).min(self.max_scroll.get()),
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll = (self.scroll + 10).min(self.max_scroll.get()),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::End => self.scroll = 0,
            KeyCode::Char('l') => {
                self.filter = self.filter.next();
                self.scroll = 0;
            }
            KeyCode::Char('o') => self.prompt = Some(Prompt::new(Ask::Run)),
            KeyCode::Char('c') => self.prompt = Some(Prompt::new(Ask::Chat)),
            KeyCode::Char('s') => self.act(fleet, Command::Stop, "stopping the script"),
            KeyCode::Char('r') => self.act(fleet, Command::Restart, "restarting the script"),
            KeyCode::Char('x') => self.act(fleet, Command::Logout, "logging out"),
            KeyCode::Char('p') => {
                let paused = shown.get(self.current()).and_then(|s| s.snapshot.as_ref()).is_some_and(|s| s.paused);
                if paused {
                    self.act(fleet, Command::Resume, "resumed");
                } else {
                    self.act(fleet, Command::Pause, "paused");
                }
            }
            _ => {}
        }
        false
    }

    fn submit(&mut self, prompt: Prompt, fleet: &Fleet) {
        let input = prompt.input.trim().to_string();
        if input.is_empty() {
            return;
        }
        match prompt.ask {
            Ask::Chat => self.act(fleet, Command::Chat { text: input }, "said it in zone chat"),
            Ask::Run => {
                let mut words = input.split_whitespace();
                let path = PathBuf::from(words.next().unwrap_or_default());
                let path = std::path::absolute(&path).unwrap_or(path);
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
    }
}

fn draw(f: &mut Frame, fleet: &Fleet, app: &App, shown: &[Shown]) {
    let matches = app.prompt.as_ref().map_or(&[][..], |p| &p.matches);
    let [body, suggestions, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(u16::from(!matches.is_empty())), Constraint::Length(1)]).areas(f.area());
    match app.open.and_then(|i| shown.get(i)) {
        Some(bot) => draw_bot(f, body, bot, app),
        None => draw_fleet(f, body, fleet, app, shown),
    }
    draw_matches(f, suggestions, matches);
    draw_footer(f, footer, fleet, app);
}

fn scripts_matching(input: &str) -> Vec<String> {
    let input = input.trim_start();
    if input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let (dir, prefix) = input.split_at(input.rfind('/').map_or(0, |i| i + 1));
    let Ok(entries) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return Vec::new() };
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

fn draw_footer(f: &mut Frame, area: Rect, fleet: &Fleet, app: &App) {
    if let Some(prompt) = &app.prompt {
        let label = match prompt.ask {
            Ask::Run => " run (script.ts KEY=VALUE…, tab complete, ↑↓ recent): ",
            Ask::Chat => " zone chat: ",
        };
        return f.render_widget(Paragraph::new(Line::from(vec![Span::raw(label).bold(), Span::raw(prompt.input.as_str()), Span::raw("█")])), area);
    }
    if let Some((note, until)) = &app.note
        && Instant::now() < *until
    {
        return f.render_widget(Paragraph::new(format!(" {note}")).yellow(), area);
    }
    let quit = if fleet.remote() { "q detach" } else { "q quit" };
    let keys = match (app.open, fleet.cells.len() > 1) {
        (None, _) => format!(" {quit} · ↑↓ pick · enter open · o run · s stop · r restart · p pause · x logout · c chat"),
        (Some(_), true) => format!(" {quit} · esc fleet · tab next · o run · s stop · r restart · p pause · x logout · l log · c chat"),
        (Some(_), false) => format!(" {quit} · o run · s stop · r restart · p pause · x logout · l log · c chat · ↑↓ scroll"),
    };
    f.render_widget(Paragraph::new(keys).dark_gray(), area);
}

fn state_style(snapshot: &Snapshot) -> (String, Style) {
    let state = snapshot.state();
    let color = match state {
        "READY" => Color::Green,
        "CLOSED" | "LOGGED OUT" => Color::Red,
        _ => Color::Yellow,
    };
    (state.to_string(), Style::new().bold().fg(color))
}

fn draw_fleet(f: &mut Frame, area: Rect, fleet: &Fleet, app: &App, shown: &[Shown]) {
    let [header, table] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let attached = if fleet.remote() { " · attached" } else { "" };
    f.render_widget(Paragraph::new(format!(" {} · {}{attached}", fleet.title, count(shown.len(), "bot"))).bold(), header);
    let rows = shown.iter().enumerate().map(|(i, bot)| {
        let Some(s) = &bot.snapshot else {
            return Row::new(vec![TableCell::from(bot.name.clone()), TableCell::from("STARTING").yellow()]);
        };
        let (state, style) = state_style(s);
        let place = s.game.as_ref().map_or(String::new(), |g| format!("{} {}", g.area, g.cell));
        let progress = s.progress.first().map_or(String::new(), |b| {
            let pct = if b.total == 0 { 0 } else { u64::from(b.current.min(b.total)) * 100 / u64::from(b.total) };
            format!("{} {pct:>3}% {}", bar(b.current, b.total, 8), b.eta_secs.map(span).unwrap_or_default())
        });
        let quiet = if s.run == "running" { span(s.quiet_secs) } else { String::new() };
        let row = Row::new(vec![
            TableCell::from(bot.name.clone()),
            TableCell::from(state).style(style),
            TableCell::from(s.script_label()),
            TableCell::from(place),
            TableCell::from(s.status.clone()),
            TableCell::from(progress),
            TableCell::from(s.kills_h.map_or("-".into(), |k| k.to_string())),
            TableCell::from(quiet),
        ]);
        if i == app.selected { row.reversed() } else { row }
    });
    let widths = [
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Length(22),
        Constraint::Min(16),
        Constraint::Length(24),
        Constraint::Length(8),
        Constraint::Length(7),
    ];
    let head = Row::new(["BOT", "STATE", "SCRIPT", "WHERE", "STATUS", "PROGRESS", "KILLS/H", "QUIET"]).dark_gray();
    f.render_widget(Table::new(rows, widths).header(head).block(panel(" Fleet ")), table);
}

fn draw_bot(f: &mut Frame, area: Rect, bot: &Shown, app: &App) {
    let [header, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    draw_header(f, header, bot);
    let s = bot.snapshot.as_ref();
    let bars = s.map_or(0, |s| s.progress.len()) as u16;
    if body.width >= WIDE {
        let [left, right] = Layout::horizontal([Constraint::Min(40), Constraint::Length(32)]).areas(body);
        let [task, combat, log] = Layout::vertical([Constraint::Length(3 + bars), Constraint::Length(7), Constraint::Min(3)]).areas(left);
        let [vitals, rates, place] = Layout::vertical([Constraint::Length(7), Constraint::Length(6), Constraint::Min(3)]).areas(right);
        draw_task(f, task, s);
        draw_combat(f, combat, s);
        draw_log(f, log, app);
        draw_vitals(f, vitals, s);
        draw_rates(f, rates, s);
        draw_area(f, place, s);
    } else {
        let [vitals, task, log] = Layout::vertical([Constraint::Length(7), Constraint::Length(3 + bars), Constraint::Min(3)]).areas(body);
        draw_vitals(f, vitals, s);
        draw_task(f, task, s);
        draw_log(f, log, app);
    }
}

fn draw_header(f: &mut Frame, area: Rect, bot: &Shown) {
    let Some(s) = &bot.snapshot else {
        return f.render_widget(Paragraph::new(Line::from(vec![Span::raw(format!(" {} · ", bot.name)).bold(), "STARTING".yellow().bold()])), area);
    };
    let who = if s.user.is_empty() || s.user.eq_ignore_ascii_case(&bot.name) { bot.name.clone() } else { format!("{} ({})", bot.name, s.user) };
    let mut spans = vec![Span::raw(format!(" {who} · {} · ", s.server)).bold()];
    if let Some(g) = &s.game {
        spans.push(Span::raw(format!("{} · {}/{} · ", g.area, g.cell, g.pad)));
    }
    let (state, style) = state_style(s);
    spans.push(Span::styled(state, style));
    spans.push(Span::raw(format!(" · up {}", span(s.up_secs))));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn waiting<'a>(title: &'a str) -> Paragraph<'a> {
    Paragraph::new("waiting for the server").dark_gray().block(panel(format!(" {title} ")))
}

fn draw_task(f: &mut Frame, area: Rect, s: Option<&Snapshot>) {
    let Some(s) = s else { return f.render_widget(waiting("Task"), area) };
    let headline = if s.phase != "READY" && !s.detail.is_empty() {
        Line::raw(s.detail.clone()).yellow()
    } else if !s.status.is_empty() {
        Line::raw(s.status.clone())
    } else {
        Line::raw(s.script_label()).dark_gray()
    };
    let width = area.width.saturating_sub(4) as usize;
    let mut lines = vec![headline];
    for b in &s.progress {
        let eta = b.eta_secs.map(span).unwrap_or_default();
        let label: String = b.label.chars().take(18).collect();
        let bar_width = width.saturating_sub(18 + 1 + 1 + 14 + 7).clamp(4, 24) as u32;
        lines.push(Line::raw(format!("{label:<18} {} {:>13} {eta:>6}", bar(b.current, b.total, bar_width), format!("{}/{}", b.current, b.total))));
    }
    f.render_widget(Paragraph::new(lines).block(panel(" Task ")), area);
}

fn draw_vitals(f: &mut Frame, area: Rect, s: Option<&Snapshot>) {
    let Some(g) = s.and_then(|s| s.game.as_ref()) else { return f.render_widget(waiting("Vitals"), area) };
    let hp_color = match () {
        _ if g.dead => Color::DarkGray,
        _ if g.max_hp > 0 && g.hp * 4 < g.max_hp => Color::Red,
        _ => Color::Green,
    };
    let mut lines = vec![
        Line::from(vec![Span::raw("HP "), Span::styled(bar(g.hp, g.max_hp, 10), Style::new().fg(hp_color)), Span::raw(format!(" {}/{}", g.hp, g.max_hp))]),
        Line::from(vec![Span::raw("MP "), Span::styled(bar(g.mp, g.max_mp, 10), Style::new().fg(Color::Blue)), Span::raw(format!(" {}/{}", g.mp, g.max_mp))]),
        Line::raw(format!("{} · Lv {}", if g.class.is_empty() { "no class" } else { &g.class }, g.level)),
        Line::raw(format!("Gold {}", short(g.gold))),
    ];
    if g.afk {
        lines.push(Line::raw("away from keyboard").yellow());
    }
    f.render_widget(Paragraph::new(lines).block(panel(" Vitals ")), area);
}

fn draw_combat(f: &mut Frame, area: Rect, s: Option<&Snapshot>) {
    let Some(g) = s.and_then(|s| s.game.as_ref()) else { return f.render_widget(waiting("Combat"), area) };
    let mut skills = vec![Span::raw("Skills  ")];
    if g.skills.is_empty() {
        skills.push(Span::raw("none"));
    }
    for (i, s) in g.skills.iter().enumerate() {
        if i > 0 {
            skills.push(Span::raw("  "));
        }
        let label = format!("{} {}", i + 1, s.name);
        let lit = label.chars().count() * usize::from(s.ready_pct.min(100)) / 100;
        let (ready, cooling) = label.split_at(label.char_indices().nth(lit).map_or(label.len(), |(at, _)| at));
        skills.push(Span::raw(ready.to_owned()));
        skills.push(Span::raw(cooling.to_owned()).dark_gray());
    }
    let lines = vec![
        Line::raw(format!("Target  {}", g.target.as_deref().unwrap_or("none"))),
        Line::raw(format!("Cell    {}", monsters(g.cell_monsters))),
        Line::raw(format!("Map     {}", monsters(g.map_monsters))),
        Line::from(skills),
        Line::raw(format!("Auras   {}", if g.auras.is_empty() { "none".into() } else { g.auras.join(", ") })),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Combat ")), area);
}

fn draw_rates(f: &mut Frame, area: Rect, s: Option<&Snapshot>) {
    let Some(s) = s else { return f.render_widget(waiting("Last 10 min"), area) };
    let row = |label: &str, value: String| Line::raw(format!("{label:<12}{value:>14}"));
    let rate = |n: Option<u64>| n.map_or("-".into(), |n| short(n as i64));
    let lines = vec![
        row("kills/h", rate(s.kills_h)),
        row("drops/h", rate(s.drops_h)),
        row("gold/h", s.gold_h.map_or("-".into(), short)),
        row("deaths", s.deaths.to_string()),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Last 10 min ")), area);
}

fn draw_area(f: &mut Frame, area: Rect, s: Option<&Snapshot>) {
    let Some(g) = s.and_then(|s| s.game.as_ref()) else { return f.render_widget(waiting("Area"), area) };
    let mut lines = vec![Line::raw(format!("{}, {} in your cell", count(g.players, "player"), g.cell_players.len() + 1))];
    lines.extend(g.cell_players.iter().map(|p| Line::raw(format!("  {p}")).dark_gray()));
    if !g.drops.is_empty() {
        lines.push(Line::raw("Drops on offer"));
        lines.extend(g.drops.iter().map(|d| Line::raw(format!("  {d}"))));
    }
    f.render_widget(Paragraph::new(lines).block(panel(" Area ")), area);
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    let shown: Vec<&LogLine> = app.log.iter().filter(|l| app.filter.shows(&l.source)).collect();
    let height = area.height.saturating_sub(2) as usize;
    app.max_scroll.set(shown.len().saturating_sub(height));
    let end = shown.len().saturating_sub(app.scroll.min(app.max_scroll.get()));
    let start = end.saturating_sub(height);
    let lines: Vec<Line> = shown[start..end]
        .iter()
        .map(|l| {
            let color = match l.source.as_str() {
                "server" => Color::Yellow,
                "script" => Color::Magenta,
                _ => Color::Cyan,
            };
            Line::from(vec![
                Span::raw(clock(l.t)).dark_gray(),
                Span::raw("  "),
                Span::styled(format!("{:<6}", l.source), Style::new().fg(color)),
                Span::raw("  "),
                Span::raw(l.text.as_str()),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(panel(format!(" Log · {} ", app.filter.label()))), area);
}

fn monsters((total, alive): (usize, usize)) -> String {
    format!("{}, {alive} alive", count(total, "monster"))
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered().title(title).padding(Padding::horizontal(1))
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

fn bar(current: u32, max: u32, width: u32) -> String {
    let filled = if max == 0 { 0 } else { (u64::from(current.min(max)) * u64::from(width) / u64::from(max)) as usize };
    format!("{}{}", "█".repeat(filled), "░".repeat(width as usize - filled))
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
    use crate::hub::{Board, Hub, Print};

    fn screen(fleet: &Fleet, app: &mut App, width: u16) -> String {
        let shown = gather(fleet, app);
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|f| draw(f, fleet, app, &shown)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer.content.chunks(width as usize).map(|row| row.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
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

    fn fleet(bots: &[&str]) -> (Fleet, Arc<Hub>) {
        let cells: Vec<Arc<BotCell>> = bots.iter().map(|b| Arc::new(BotCell::mirror(b))).collect();
        let hub = Arc::new(Hub::new("aqx", cells.clone(), Print::Quiet));
        (Fleet { title: "aqx".into(), cells, commander: Commander::Local }, hub)
    }

    fn publish(hub: &Hub, cell: &BotCell, world: &World) {
        let now = Instant::now();
        let mut board: std::sync::MutexGuard<'_, Board> = cell.board();
        board.server = "Twilly".into();
        board.ready_since = Some(now - Duration::from_secs(90));
        board.first_ready = board.ready_since;
        board.start_script("/srv/void-aura.ts".into(), now);
        drop(board);
        hub.record(cell, &Event::Status { text: "Astral Ephemerite Essence in timespace".into() });
        hub.record(cell, &Event::Progress { label: "Void Aura".into(), current: 4120, total: 7500 });
        hub.record(cell, &Event::Killed { id: aqx_protocol::MonMapId(1), monster: "Mosquito".into() });
        hub.record(cell, &Event::ServerText { text: "Profanity filter On.".into() });
        let mut board = cell.board();
        board.snapshot = Some(board.snapshot(&cell.name, "hero", &Phase::Ready, world, now));
    }

    #[test]
    fn the_bot_view_shows_the_task_player_map_and_log() {
        let (fleet, hub) = fleet(&["main"]);
        publish(&hub, &fleet.cells[0], &farm_world());
        let mut app = App { open: Some(0), ..App::default() };
        let shown = screen(&fleet, &mut app, 110);
        println!("{shown}");
        for expected in [
            "main (hero) · Twilly · farm-1 · Enter/Spawn · READY · up 1m",
            "Astral Ephemerite Essence in timespace",
            "Void Aura",
            "4120/7500",
            "HP █████░░░░░ 2000/4000",
            "Barber · Lv 100",
            "Gold 12.3M",
            "Cell    1 monster, 1 alive",
            "Map     2 monsters, 1 alive",
            "Auras   Corruption x2",
            "2 players, 2 in your cell",
            "aqx     killed Mosquito",
            "server  Profanity filter On.",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }
        let narrow = screen(&fleet, &mut app, 60);
        for expected in ["HP █████░░░░░ 2000/4000", "Void Aura", "killed Mosquito", "q quit"] {
            assert!(narrow.contains(expected), "narrow screen is missing {expected:?}");
        }
        app.filter = Filter::Server;
        let shown = screen(&fleet, &mut app, 110);
        assert!(shown.contains("Profanity filter On.") && !shown.contains("killed Mosquito"));
    }

    #[test]
    fn the_fleet_view_has_a_row_per_bot() {
        let (fleet, hub) = fleet(&["main", "healer"]);
        publish(&hub, &fleet.cells[0], &farm_world());
        let shown = screen(&fleet, &mut App::default(), 140);
        println!("{shown}");
        let main = shown.lines().find(|l| l.contains("main")).expect("a row for main");
        for expected in ["READY", "void-aura.ts", "farm-1 Enter", "Astral Ephemerite", "54%"] {
            assert!(main.contains(expected), "main's row is missing {expected:?}: {main}");
        }
        assert!(shown.lines().any(|l| l.contains("healer") && l.contains("STARTING")));
    }

    #[test]
    fn scrolling_past_the_oldest_line_keeps_the_log_on_screen() {
        let (fleet, hub) = fleet(&["main"]);
        for i in 0..5 {
            hub.say(&fleet.cells[0], "aqx", format!("line {i}"));
        }
        let mut app = App { open: Some(0), scroll: 500, ..App::default() };
        let shown = screen(&fleet, &mut app, 100);
        assert!(shown.contains("line 0") && shown.contains("line 4"));
    }

    #[test]
    fn draws_every_panel_before_the_server_answers() {
        let (fleet, _hub) = fleet(&["main"]);
        let shown = screen(&fleet, &mut App { open: Some(0), ..App::default() }, 100);
        assert!(shown.contains("STARTING"));
        for title in ["Task", "Combat", "Vitals", "Last 10 min", "Area", "Log"] {
            assert!(shown.contains(title), "missing panel {title}");
        }
    }

    #[test]
    fn the_log_copies_only_new_lines_and_keeps_a_ring() {
        let (fleet, hub) = fleet(&["main"]);
        let mut app = App { open: Some(0), ..App::default() };
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
    fn a_closed_bot_shows_its_phase_before_pause_or_stall() {
        let snapshot = |phase: &str| Snapshot { phase: phase.into(), paused: true, stalled: true, ..Snapshot::default() };
        assert_eq!(snapshot("READY").state(), "PAUSED");
        assert_eq!(snapshot("RECONNECTING").state(), "RECONNECTING");
        assert_eq!(snapshot("LOGGED OUT").state(), "LOGGED OUT");
        assert_eq!(Snapshot { phase: "READY".into(), stalled: true, ..Snapshot::default() }.state(), "STALLED");
    }

    #[test]
    fn the_header_names_the_account_only_when_it_differs_from_the_bot() {
        let (fleet, hub) = fleet(&["hero"]);
        publish(&hub, &fleet.cells[0], &farm_world());
        let shown = screen(&fleet, &mut App { open: Some(0), ..App::default() }, 110);
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
        let mut app = App { open: Some(0), ..App::default() };
        let press = |app: &mut App, code: KeyCode| app.handle(KeyEvent::from(code), &fleet, &[]);
        let typed = |app: &mut App, text: &str| text.chars().for_each(|c| _ = press(app, KeyCode::Char(c)));
        let input = |app: &App| app.prompt.as_ref().unwrap().input.clone();

        press(&mut app, KeyCode::Char('o'));
        typed(&mut app, &format!(" {root}"));
        let shown = screen(&fleet, &mut app, 100);
        assert!(shown.contains(" farm.ts  fight.js  loop.mjs  quests/"), "{shown}");
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

        app.prompt = Some(Prompt::new(Ask::Run));
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
    fn the_last_match_takes_no_gap_after_it() {
        let matches = ["farm.ts".to_string(), "fight.js".to_string()];
        let mut terminal = Terminal::new(TestBackend::new(18, 1)).unwrap();
        terminal.draw(|f| draw_matches(f, f.area(), &matches)).unwrap();
        let row: String = terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect();
        assert_eq!(row, " farm.ts  fight.js");
    }

    #[test]
    fn a_recharging_skill_lights_up_from_the_left() {
        use crate::hub::{Game, SkillState};
        let skill = |name: &str, ready_pct| SkillState { name: name.into(), ready_pct };
        let game = Game { skills: vec![skill("Slash", 100), skill("Blur", 50), skill("Rend", 0)], ..Game::default() };
        let snapshot = Snapshot { game: Some(game), ..Snapshot::default() };
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal.draw(|f| draw_combat(f, f.area(), Some(&snapshot))).unwrap();
        let rows: Vec<_> = terminal.backend().buffer().content.chunks(60).map(<[_]>::to_vec).collect();
        let row = rows.iter().find(|r| r.iter().map(|c| c.symbol()).collect::<String>().contains("Skills")).unwrap();
        let text: String = row.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Skills  1 Slash  2 Blur  3 Rend"), "{text}");
        let dim: String = row.iter().filter(|c| c.fg == Color::DarkGray).map(|c| c.symbol()).collect();
        assert_eq!(dim, "lur3 Rend");
    }
}
