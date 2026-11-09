mod ask;
mod draw;
mod keymap;
mod library;
mod mask;
mod prompt;
mod servers;
mod skua;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mira_client::ItemId;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input as TextInput, InputRequest};

use crate::control::{Action, Source, Want};
use crate::fleet::{Channel, Line, Log};
use crate::view::{self, Game, Shown, order};
use ask::{Ask, Remembered};
use keymap::Command;
pub use keymap::{Keymap, SCHEMA_NAME as KEYS_SCHEMA_NAME, keymap_path, schema as keys_schema};
use library::{Picker, Update};
use mask::Mask;
use prompt::{History, Prompt, Recent};
use servers::Servers;
use skua::Skua;

const NOTE_FOR: Duration = Duration::from_secs(4);
const OPENING_NOTE_FOR: Duration = Duration::from_secs(10);
const WHEEL_LINES: usize = 3;
const PAGE_LINES: usize = 10;
const SIDEBAR_STEP: u16 = 4;
const FRAME: Duration = Duration::from_millis(125);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Players,
    Drops,
    Quests,
    Temp,
    Inventory,
    Bank,
    Factions,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Players,
        Section::Drops,
        Section::Quests,
        Section::Temp,
        Section::Inventory,
        Section::Bank,
        Section::Factions,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum View {
    #[default]
    Log,
    List(Section),
}

impl View {
    const ALL: [View; 8] = [
        View::Log,
        View::List(Section::Players),
        View::List(Section::Drops),
        View::List(Section::Quests),
        View::List(Section::Temp),
        View::List(Section::Inventory),
        View::List(Section::Bank),
        View::List(Section::Factions),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Filter {
    #[default]
    Main,
    All,
    Only(Channel),
}

impl Filter {
    const ALL: [Filter; 7] = [
        Filter::Main,
        Filter::All,
        Filter::Only(Channel::Mira),
        Filter::Only(Channel::Script),
        Filter::Only(Channel::Server),
        Filter::Only(Channel::Chat),
        Filter::Only(Channel::Game),
    ];

    fn shows(self, line: &Line) -> bool {
        match self {
            Filter::Main => !matches!(line.channel, Channel::Chat | Channel::Game),
            Filter::All => true,
            Filter::Only(channel) => line.channel == channel,
        }
    }

    fn tagged(self) -> bool {
        matches!(self, Filter::Main | Filter::All)
    }

    fn label(self) -> &'static str {
        match self {
            Filter::Main => "main",
            Filter::All => "all",
            Filter::Only(channel) => channel.label(),
        }
    }

    fn step(self, by: isize) -> Filter {
        let at = Filter::ALL.iter().position(|&f| f == self).unwrap_or(0) as isize;
        Filter::ALL[(at + by).clamp(0, Filter::ALL.len() as isize - 1) as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Scope {
    Accounts,
    #[default]
    Log,
    List(Section),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    View(View),
    Filter(Filter),
    Jump,
    Account(usize),
}

#[derive(Default)]
struct Rebind {
    cursor: usize,
    top: usize,
    waiting: bool,
    error: Option<String>,
}

#[derive(Default)]
struct App {
    home: PathBuf,
    selected: usize,
    marked: BTreeSet<usize>,
    details: bool,
    grid: bool,
    keys: bool,
    rebind: Option<Rebind>,
    keymap: Keymap,
    pane_focus: bool,
    zoomed: bool,
    sidebar_width: Option<u16>,
    sidebar: Cell<Option<Rect>>,
    dragging: bool,
    frame: usize,
    view: View,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    log_width: Cell<usize>,
    list_at: Cell<usize>,
    list_max: Cell<usize>,
    list_len: Cell<usize>,
    cursor: usize,
    picked: HashSet<ItemId>,
    unseen_from: Option<u64>,
    sidebar_top: Cell<usize>,
    grid_top: Cell<usize>,
    grid_cols: Cell<usize>,
    hits: RefCell<Vec<(Rect, Hit)>>,
    prompt: Option<Prompt>,
    query: TextInput,
    query_scope: Scope,
    querying: bool,
    ending: bool,
    closing: bool,
    recent: Recent,
    history: History,
    ask: Option<Ask>,
    skua: Option<Skua>,
    picker: Option<Picker>,
    picker_tab: library::Tab,
    servers: Option<Servers>,
    update: Option<Update>,
    targets: Vec<(String, Option<String>)>,
    share_args: bool,
    remembered: Remembered,
    mask: Mask,
    redact: bool,
    names: Vec<String>,
    note: Option<(String, Instant)>,
    log: Log,
    log_of: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    Detach,
    Ended,
}

pub fn run(
    source: &dyn Source,
    home: &Path,
    keymap: Keymap,
    redact: bool,
    note: Option<String>,
) -> std::io::Result<Leave> {
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste
        );
        restore(info);
    }));
    let mut app = App::new(&source.configured(), home, History::in_data_dir(), redact);
    app.keymap = keymap;
    if let Some(note) = note {
        app.note = Some((note, Instant::now() + OPENING_NOTE_FOR));
    }
    let started = Instant::now();
    let result = loop {
        app.frame = (started.elapsed().as_millis() / FRAME.as_millis()) as usize;
        if let Some(skua) = &mut app.skua {
            skua.poll();
        }
        if let Some(servers) = &mut app.servers {
            servers.poll();
        }
        app.poll_update();
        let shown = match gather(source, &mut app) {
            Ok(Some(shown)) => shown,
            Ok(None) => break Ok(Leave::Ended),
            Err(e) => break Err(std::io::Error::other(e)),
        };
        if let Err(e) = terminal.draw(|f| draw::draw(f, &app, &shown)) {
            break Err(e);
        }
        match event::poll(FRAME) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => break Err(e),
        }
        match app.take_input(source, &shown) {
            Ok(None) => {}
            Ok(Some(leave)) => break Ok(leave),
            Err(e) => break Err(e),
        }
    };
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();
    result
}

fn mask(accounts: &[(String, String)], redact: bool) -> Mask {
    if !redact {
        return Mask::default();
    }
    let names = accounts.iter().enumerate().flat_map(|(i, (name, user))| {
        let alias = format!("account {}", i + 1);
        [name.clone(), user.clone()].map(|name| (name, alias.clone()))
    });
    Mask::new(std::env::var("HOME").ok(), names)
}

pub fn last_words(accounts: &[crate::fleet::LastLines], redact: bool) -> Vec<String> {
    let names: Vec<(String, String)> = accounts
        .iter()
        .map(|a| (a.name.clone(), a.user.clone()))
        .collect();
    let mask = mask(&names, redact);
    let tail = if accounts.len() == 1 { 10 } else { 3 };
    let shown: Vec<String> = accounts.iter().map(|a| mask.hide(&a.name)).collect();
    let width = shown
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    for (account, name) in accounts.iter().zip(&shown) {
        let who = if accounts.len() == 1 {
            String::new()
        } else {
            format!("{name:<width$}  ")
        };
        let lines = &account.lines[account.lines.len().saturating_sub(tail)..];
        for line in lines {
            out.push(format!(
                "{}  {who}{:<6}  {}",
                crate::view::time_of_day(line.at_ms),
                line.channel.label(),
                mask.hide(&line.text)
            ));
        }
    }
    out
}

fn gather(source: &dyn Source, app: &mut App) -> Result<Option<Vec<Shown>>, String> {
    if app.log_of != Some(app.selected) {
        app.log = Log::default();
        app.log_of = Some(app.selected);
        app.scroll = 0;
    }
    if app.scroll == 0 {
        app.unseen_from = None;
    }
    let frame = source.frame(&Want {
        selected: app.selected,
        since: app.log.next_seq(),
        full: !app.grid,
    })?;
    if frame.ending {
        return Ok(None);
    }
    if frame.accounts.len() != app.names.len() {
        let names: Vec<(String, String)> = frame
            .accounts
            .iter()
            .map(|s| (s.name.clone(), s.user.clone()))
            .collect();
        app.mask = mask(&names, app.redact);
    }
    for line in frame.log {
        app.receive(line);
    }
    for script in frame.accounts.iter().filter_map(|s| s.script.as_deref()) {
        app.recent.offer(crate::config::Script {
            path: PathBuf::from(script),
            args: Vec::new(),
        });
    }
    app.targets = frame
        .accounts
        .iter()
        .map(|s| (s.user.clone(), s.script.clone()))
        .collect();
    let shown: Vec<Shown> = frame.accounts.into_iter().map(|s| app.masked(s)).collect();
    app.names = shown.iter().map(|s| s.name.clone()).collect();
    if shown.get(app.selected).is_none_or(|s| s.game.is_none()) && app.view != View::Log {
        app.show(View::Log);
    }
    Ok(Some(shown))
}

impl App {
    fn take_input(
        &mut self,
        source: &dyn Source,
        shown: &[Shown],
    ) -> std::io::Result<Option<Leave>> {
        loop {
            match event::read()? {
                Input::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(leave) = self.handle(key, source, shown) {
                        return Ok(Some(leave));
                    }
                }
                Input::Mouse(mouse) => self.mouse(mouse, shown),
                Input::Paste(text) => self.paste(&text, shown),
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                return Ok(None);
            }
        }
    }

    fn paste(&mut self, text: &str, shown: &[Shown]) {
        let insert = |input: &mut TextInput| {
            for c in text.chars().filter(|c| !c.is_control()) {
                input.handle(InputRequest::InsertChar(c));
            }
        };
        if let Some(picker) = &mut self.picker {
            picker.searching = true;
            insert(&mut picker.query);
            picker.edited();
        } else if let Some(prompt) = &mut self.prompt {
            insert(&mut prompt.input);
            prompt.edited();
            prompt.find_matches(&self.home);
        } else if let Some(ask) = &mut self.ask {
            ask.edit();
            insert(&mut ask.input);
            ask.recalled = false;
        } else if self.querying {
            insert(&mut self.query);
            self.refilter(shown);
        }
    }

    fn masked(&self, mut s: Shown) -> Shown {
        s.name = self.mask.hide(&s.name);
        s.user = self.mask.hide(&s.user);
        s.script = s.script.map(|script| self.mask.hide(&script));
        s.status = self.mask.hide(&s.status);
        if let view::State::Offline { why: Some(why) } = &mut s.state {
            *why = self.mask.hide(why);
        }
        if let Some(game) = &mut s.game {
            for player in &mut game.players {
                player.name = self.mask.hide(&player.name);
            }
        }
        s
    }

    fn receive(&mut self, mut line: Line) {
        line.text = self.mask.hide(&line.text);
        if self.scroll > 0 && self.shows(&line) {
            self.scroll +=
                draw::log_rows(&line, self.log_width.get(), self.filter.tagged(), "").len();
            self.unseen_from.get_or_insert(line.seq);
        }
        self.log.insert(line);
    }

    fn new(
        configured: &[crate::config::Script],
        home: &Path,
        history: History,
        redact: bool,
    ) -> App {
        let mut recent = Recent::default();
        for script in configured {
            recent.offer(script.clone());
        }
        history.load(&mut recent);
        App {
            home: home.to_path_buf(),
            recent,
            history,
            redact,
            ..App::default()
        }
    }

    fn on_pane(&self, shown: &[Shown]) -> bool {
        !self.grid && (shown.len() < 2 || self.pane_focus)
    }

    fn focused(&self, shown: &[Shown]) -> Scope {
        match self.view {
            _ if !self.on_pane(shown) => Scope::Accounts,
            View::Log => Scope::Log,
            View::List(section) => Scope::List(section),
        }
    }

    fn search(&self, scope: Scope) -> &str {
        if self.query_scope == scope {
            self.query.value()
        } else {
            ""
        }
    }

    fn clear_search(&mut self) {
        self.query.reset();
        self.querying = false;
    }

    fn shows(&self, line: &Line) -> bool {
        self.filter.shows(line) && view::matches(&line.text, self.search(Scope::Log))
    }

    fn targets(&self) -> Vec<usize> {
        if self.marked.is_empty() {
            vec![self.selected]
        } else {
            self.marked.iter().copied().collect()
        }
    }

    fn all(&self, shown: &[Shown], test: impl Fn(&Shown) -> bool) -> bool {
        self.targets()
            .iter()
            .all(|&i| shown.get(i).is_some_and(&test))
    }

    fn who(&self, shown: &[Shown], room: usize) -> String {
        let targets = self.targets();
        let names = targets
            .iter()
            .filter_map(|&i| shown.get(i))
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if names.chars().count() <= room {
            names
        } else {
            format!("{} accounts", targets.len())
        }
    }

    fn step(&mut self, shown: &[Shown], by: isize) {
        let order = order(shown, self.search(Scope::Accounts));
        let Some(at) = order.iter().position(|&i| i == self.selected) else {
            return;
        };
        let at = (at as isize + by).clamp(0, order.len() as isize - 1);
        self.pick(order[at as usize]);
    }

    fn refilter(&mut self, shown: &[Shown]) {
        match self.query_scope {
            Scope::Accounts => {
                let order = order(shown, self.search(Scope::Accounts));
                if let Some(&first) = order.first().filter(|_| !order.contains(&self.selected)) {
                    self.pick(first);
                }
            }
            Scope::Log => self.scroll = 0,
            Scope::List(_) => {
                self.list_at.set(0);
                self.cursor = 0;
            }
        }
    }

    fn pick(&mut self, i: usize) {
        if i != self.selected {
            self.selected = i;
            self.reset_list();
        }
    }

    fn show(&mut self, view: View) {
        if view != self.view {
            self.view = view;
            self.reset_list();
            self.clear_search();
        }
    }

    fn open(&mut self, view: View) {
        self.show(view);
        if view == View::List(Section::Drops) {
            self.pane_focus = true;
        }
    }

    fn set_filter(&mut self, filter: Filter) {
        self.show(View::Log);
        if filter != self.filter {
            self.filter = filter;
            self.scroll = 0;
        }
    }

    fn reset_list(&mut self) {
        self.list_at.set(0);
        self.cursor = 0;
        self.picked.clear();
    }

    fn picking(&self) -> bool {
        self.view == View::List(Section::Drops) && !self.grid
    }

    fn up(&mut self, lines: usize) {
        let last = self.list_len.get().saturating_sub(1);
        match self.view {
            View::List(Section::Drops) => self.cursor = self.cursor.min(last).saturating_sub(lines),
            View::List(_) => self.list_at.set(
                self.list_at
                    .get()
                    .min(self.list_max.get())
                    .saturating_sub(lines),
            ),
            View::Log => self.scroll = self.scroll.saturating_add(lines).min(self.max_scroll.get()),
        }
    }

    fn down(&mut self, lines: usize) {
        match self.view {
            View::List(Section::Drops) => {
                self.cursor = self
                    .cursor
                    .saturating_add(lines)
                    .min(self.list_len.get().saturating_sub(1));
            }
            View::List(_) => self.list_at.set(
                self.list_at
                    .get()
                    .saturating_add(lines)
                    .min(self.list_max.get()),
            ),
            View::Log => self.scroll = self.scroll.saturating_sub(lines),
        }
    }

    fn game<'a>(&self, shown: &'a [Shown]) -> Option<&'a Game> {
        shown.get(self.selected).and_then(|s| s.game.as_ref())
    }

    fn drops<'a>(&self, shown: &'a [Shown]) -> Vec<&'a view::Stack> {
        let query = self.search(Scope::List(Section::Drops));
        self.game(shown)
            .map_or(&[][..], |g| g.drops.as_slice())
            .iter()
            .filter(|d| view::matches(&d.name, query))
            .collect()
    }

    fn pick_up(&mut self, source: &dyn Source, shown: &[Shown]) {
        let wanted: Vec<&view::Stack> = if self.picked.is_empty() {
            let drops = self.drops(shown);
            drops
                .get(self.cursor.min(drops.len().saturating_sub(1)))
                .copied()
                .into_iter()
                .collect()
        } else {
            self.game(shown)
                .map_or(&[][..], |g| g.drops.as_slice())
                .iter()
                .filter(|d| self.picked.contains(&d.id))
                .collect()
        };
        let done = match wanted.as_slice() {
            [] => return,
            [only] => format!("picking up {}", only.name),
            many => format!("picking up {} drops", many.len()),
        };
        let ids: Vec<i64> = wanted.iter().map(|d| d.id.0).collect();
        self.picked.clear();
        let selected = self.selected;
        self.act_on(source, vec![selected], &done, |_| {
            Action::PickUp(ids.clone())
        });
    }

    fn mouse(&mut self, mouse: MouseEvent, shown: &[Shown]) {
        if self.picker.is_some() || self.servers.is_some() {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(mouse.column, mouse.row, shown),
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.sidebar_width = Some(mouse.column + 1);
            }
            MouseEventKind::Up(_) => self.dragging = false,
            _ if self.grid => {}
            MouseEventKind::ScrollUp => self.up(WHEEL_LINES),
            MouseEventKind::ScrollDown => self.down(WHEEL_LINES),
            _ => {}
        }
    }

    fn click(&mut self, column: u16, row: u16, shown: &[Shown]) {
        let at = Position::new(column, row);
        self.dragging = self
            .sidebar
            .get()
            .is_some_and(|s| column + 1 == s.right() || column == s.right());
        if self.dragging {
            return;
        }
        let hit = self
            .hits
            .borrow()
            .iter()
            .find(|(rect, _)| rect.contains(at))
            .map(|&(_, hit)| hit);
        match hit {
            Some(Hit::View(view)) if view == self.view => self.show(View::Log),
            Some(Hit::View(view)) => self.open(view),
            Some(Hit::Filter(filter)) => self.set_filter(filter),
            Some(Hit::Jump) => self.scroll = 0,
            Some(Hit::Account(i)) if i < shown.len() => {
                self.pick(i);
                self.pane_focus = false;
            }
            Some(Hit::Account(_)) | None => {}
        }
    }

    fn resize_sidebar(&mut self, by: impl Fn(u16) -> u16) {
        if let Some(sidebar) = self.sidebar.get() {
            self.sidebar_width = Some(by(sidebar.width));
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((self.mask.hide(&text.into()), Instant::now() + NOTE_FOR));
    }

    fn act(&mut self, source: &dyn Source, done: &str, action: Action) {
        self.act_on(source, self.targets(), done, |_| action.clone());
    }

    fn act_on(
        &mut self,
        source: &dyn Source,
        targets: Vec<usize>,
        done: &str,
        action: impl Fn(usize) -> Action,
    ) {
        let (mut sent, mut skipped) = (Vec::new(), Vec::<(String, String)>::new());
        for i in targets {
            let name = self
                .names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("account {}", i + 1));
            match source.act(i, action(i)) {
                Ok(()) => sent.push(name),
                Err(why) => skipped.push((name, why)),
            }
        }
        let mut said = Vec::new();
        if !sent.is_empty() {
            said.push(format!("{}: {done}", sent.join(", ")));
        }
        let mut reasons: Vec<&str> = skipped.iter().map(|(_, why)| why.as_str()).collect();
        reasons.dedup();
        for why in reasons {
            let names: Vec<&str> = skipped
                .iter()
                .filter(|(_, w)| w == why)
                .map(|(n, _)| n.as_str())
                .collect();
            said.push(format!("{}: {why}", names.join(", ")));
        }
        self.say(said.join("; "));
    }

    fn handle(&mut self, key: KeyEvent, source: &dyn Source, shown: &[Shown]) -> Option<Leave> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.ending {
                return Some(Leave::Detach);
            }
            self.ending = true;
            return None;
        }
        if self.ending {
            match key.code {
                KeyCode::Enter => {
                    self.ending = false;
                    self.closing = true;
                    if let Err(why) = source.end() {
                        self.say(why);
                    }
                }
                KeyCode::Char('q') => return Some(Leave::Detach),
                KeyCode::Esc => self.ending = false,
                _ => {}
            }
            return None;
        }
        if self.keys {
            self.keys = false;
            if key.code == KeyCode::Char('e') && self.keymap.path.is_some() {
                self.rebind = Some(Rebind::default());
            }
            return None;
        }
        if let Some(rebind) = &mut self.rebind {
            let last = keymap::COMMANDS.len() - 1;
            let change = match key.code {
                _ if rebind.waiting => {
                    rebind.waiting = false;
                    (key.code != KeyCode::Esc).then_some(Some(key))
                }
                KeyCode::Esc => {
                    self.rebind = None;
                    self.keys = true;
                    return None;
                }
                KeyCode::Up => {
                    rebind.cursor = rebind.cursor.saturating_sub(1);
                    rebind.top = rebind.top.min(rebind.cursor);
                    None
                }
                KeyCode::Down => {
                    rebind.cursor = (rebind.cursor + 1).min(last);
                    let bottom = (rebind.cursor + 1).saturating_sub(draw::REBIND_ROWS);
                    rebind.top = rebind.top.max(bottom);
                    None
                }
                KeyCode::Enter => {
                    rebind.waiting = true;
                    rebind.error = None;
                    None
                }
                KeyCode::Backspace | KeyCode::Delete => Some(None),
                _ => None,
            };
            if let Some(key) = change {
                let command = keymap::COMMANDS[rebind.cursor].command;
                match self.keymap.rebind(command, key) {
                    Ok(keymap) => {
                        self.keymap = keymap;
                        rebind.error = None;
                    }
                    Err(why) => rebind.error = Some(why),
                }
            }
            return None;
        }
        if let Some(prompt) = &mut self.prompt {
            match key.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Enter => {
                    let prompt = self.prompt.take().expect("a prompt is open");
                    self.submit(prompt.input.value(), source);
                }
                KeyCode::Up | KeyCode::Down => {
                    prompt.recall(&self.recent.typed(&self.home), key.code == KeyCode::Up);
                }
                KeyCode::Tab => prompt.complete(),
                _ => {
                    if prompt
                        .input
                        .handle_event(&Input::Key(key))
                        .is_some_and(|changed| changed.value)
                    {
                        prompt.edited();
                    }
                }
            }
            if let Some(prompt) = &mut self.prompt {
                prompt.find_matches(&self.home);
            }
            return None;
        }
        if let Some(ask) = &mut self.ask {
            match key.code {
                KeyCode::Esc => self.ask = None,
                KeyCode::Tab if ask.can_share() => {
                    ask.toggle_share(&self.remembered);
                    self.share_args = ask.shared;
                }
                KeyCode::Enter => {
                    if let Some(runs) = ask.answer(&mut self.remembered) {
                        let ask = self.ask.take().expect("an ask is open");
                        self.run(source, &ask.script, runs);
                    }
                }
                KeyCode::Backspace | KeyCode::Delete if ask.recalled => ask.edit(),
                _ => {
                    let typing = matches!(key.code, KeyCode::Char(_))
                        && key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
                    if typing {
                        ask.edit();
                    }
                    if ask.input.handle_event(&Input::Key(key)).is_some() {
                        ask.recalled = false;
                        ask.error = None;
                    }
                }
            }
            return None;
        }
        if let Some(skua) = &mut self.skua {
            let editable = shown.get(skua.account).is_some_and(skua::editable);
            match skua.key(key, editable) {
                skua::Outcome::Close => self.skua = None,
                skua::Outcome::Save(save) => skua.save(save),
                skua::Outcome::Nothing => {}
            }
            return None;
        }
        if let Some(servers) = &mut self.servers {
            match servers.key(key) {
                servers::Outcome::Close => self.servers = None,
                servers::Outcome::Switch(server) => {
                    self.servers = None;
                    let done = format!("switching to {server}");
                    self.act(source, &done, Action::Switch(server));
                }
                servers::Outcome::Nothing => {}
            }
            return None;
        }
        if let Some(picker) = &mut self.picker {
            let outcome = picker.key(key);
            self.picker_tab = picker.tab;
            match outcome {
                library::Outcome::Close => self.picker = None,
                library::Outcome::Run(path) => {
                    self.picker = None;
                    self.start(
                        crate::config::Script {
                            path,
                            args: Vec::new(),
                        },
                        source,
                    );
                }
                library::Outcome::Options(script) => self.skua = self.options(script),
                library::Outcome::Update => self.start_update(),
                library::Outcome::Nothing => {}
            }
            return None;
        }
        if self.querying {
            let accounts = self.query_scope == Scope::Accounts;
            let changed = match key.code {
                KeyCode::Esc => {
                    self.clear_search();
                    true
                }
                KeyCode::Enter => {
                    self.querying = false;
                    false
                }
                KeyCode::Up if accounts => {
                    self.step(shown, -1);
                    false
                }
                KeyCode::Down if accounts => {
                    self.step(shown, 1);
                    false
                }
                KeyCode::Up => {
                    self.up(1);
                    false
                }
                KeyCode::Down => {
                    self.down(1);
                    false
                }
                KeyCode::Tab if accounts => {
                    let names: Vec<&str> = order(shown, self.query.value())
                        .into_iter()
                        .map(|i| shown[i].name.as_str())
                        .collect();
                    self.query = TextInput::new(complete(self.query.value(), &names));
                    true
                }
                _ => self
                    .query
                    .handle_event(&Input::Key(key))
                    .is_some_and(|changed| changed.value),
            };
            if changed {
                self.refilter(shown);
            }
            return None;
        }
        if let Some(command) = self.keymap.command(key) {
            return self.command(command, source, shown);
        }
        None
    }

    fn command(&mut self, command: Command, source: &dyn Source, shown: &[Shown]) -> Option<Leave> {
        let pane = self.on_pane(shown);
        let picking = pane && self.picking();
        match command {
            Command::Detach => return Some(Leave::Detach),
            Command::EndSession => self.ending = true,
            Command::Help => self.keys = true,
            Command::Dashboard if shown.len() > 1 => {
                self.grid = !self.grid;
                self.clear_search();
            }
            Command::Search => {
                let scope = self.focused(shown);
                if scope != self.query_scope {
                    self.query.reset();
                    self.query_scope = scope;
                    self.refilter(shown);
                }
                self.querying = true;
            }
            Command::Back => self.back(shown),
            Command::LogIn => self.act(source, "logging in", Action::Login),
            Command::LogOut => self.act(source, "logging out", Action::Logout),
            Command::Stop => self.act(source, "stopping the script", Action::Stop),
            Command::Restart => self.act(source, "restarting the script", Action::Restart),
            Command::Pause if self.all(shown, Shown::paused) => {
                self.act(source, "resumed", Action::Resume);
            }
            Command::Pause => self.act(source, "paused", Action::Pause),
            Command::PickScript => self.open_picker(),
            Command::SwitchServer => {
                if let Some(account) = shown.get(self.selected) {
                    let listed = source.servers(&self.targets());
                    self.servers = Some(Servers::open(account.server.clone(), listed));
                }
            }
            Command::TypeScript => self.prompt = Some(Prompt::new(&self.home)),
            Command::Mark if picking => {
                if let Some(drop) = self.drops(shown).get(self.cursor)
                    && !self.picked.remove(&drop.id)
                {
                    self.picked.insert(drop.id);
                }
            }
            Command::MarkAll if picking => {
                let ids: Vec<ItemId> = self.drops(shown).iter().map(|d| d.id).collect();
                if ids.iter().all(|id| self.picked.contains(id)) {
                    self.picked.retain(|id| !ids.contains(id));
                } else {
                    self.picked.extend(ids);
                }
            }
            Command::Open if picking => self.pick_up(source, shown),
            Command::Mark | Command::MarkAll | Command::Open if pane => {}
            Command::Mark => {
                if !self.marked.remove(&self.selected) {
                    self.marked.insert(self.selected);
                }
            }
            Command::MarkAll => self.mark_all(shown),
            _ if self.grid => self.on_grid(command, shown),
            Command::Focus if shown.len() > 1 => self.pane_focus = !self.pane_focus,
            Command::Up if pane => self.up(1),
            Command::Down if pane => self.down(1),
            Command::Up => self.step(shown, -1),
            Command::Down => self.step(shown, 1),
            Command::Open => self.pane_focus = true,
            Command::View(view) => {
                let open = shown.get(self.selected).is_some_and(|s| s.game.is_some());
                if open || view == View::Log {
                    self.open(view);
                }
            }
            Command::PreviousChannel => self.set_filter(self.filter.step(-1)),
            Command::NextChannel => self.set_filter(self.filter.step(1)),
            Command::PageUp => self.up(PAGE_LINES),
            Command::PageDown => self.down(PAGE_LINES),
            Command::Oldest => self.up(usize::MAX),
            Command::Newest => self.down(usize::MAX),
            Command::Details => self.details = !self.details,
            Command::Zoom => self.zoomed = !self.zoomed,
            Command::NarrowerSidebar => self.resize_sidebar(|w| w.saturating_sub(SIDEBAR_STEP)),
            Command::WiderSidebar => self.resize_sidebar(|w| w + SIDEBAR_STEP),
            Command::Focus | Command::Left | Command::Right | Command::Dashboard => {}
        }
        None
    }

    fn on_grid(&mut self, command: Command, shown: &[Shown]) {
        let cols = self.grid_cols.get().max(1) as isize;
        match command {
            Command::Left => self.step(shown, -1),
            Command::Right => self.step(shown, 1),
            Command::Up => self.step(shown, -cols),
            Command::Down => self.step(shown, cols),
            Command::Open => {
                self.grid = false;
                self.clear_search();
            }
            _ => {}
        }
    }

    fn mark_all(&mut self, shown: &[Shown]) {
        let visible = order(shown, self.search(Scope::Accounts));
        if visible.iter().all(|i| self.marked.contains(i)) {
            self.marked.retain(|i| !visible.contains(i));
        } else {
            self.marked.extend(visible);
        }
    }

    fn back(&mut self, shown: &[Shown]) {
        if !self.picked.is_empty() {
            self.picked.clear();
        } else if !self.query.value().is_empty() {
            self.query.reset();
            self.refilter(shown);
        } else if self.view != View::Log || (self.pane_focus && self.on_pane(shown)) {
            self.show(View::Log);
            self.pane_focus = false;
        } else {
            self.marked.clear();
        }
    }

    fn open_picker(&mut self) {
        let mut picker = Picker::open(&self.home, self.picker_tab);
        if let Some(script) = self
            .targets
            .get(self.selected)
            .and_then(|(_, s)| s.as_deref())
        {
            picker.select(&self.home.join(script));
        }
        self.picker_tab = picker.tab;
        self.picker = Some(picker);
    }

    fn options(&self, script: PathBuf) -> Option<Skua> {
        let (user, _) = self.targets.get(self.selected)?;
        Some(Skua::open(self.selected, user.clone(), script))
    }

    fn start_update(&mut self) {
        if self.update.is_none() {
            self.update = Some(Update::start());
        }
        if let Some(picker) = &mut self.picker {
            picker.failed = None;
        }
    }

    fn poll_update(&mut self) {
        let Some(done) = self.update.as_mut().and_then(Update::poll) else {
            return;
        };
        self.update = None;
        if let Some(picker) = &mut self.picker {
            picker.reload();
            picker.failed = done.as_ref().err().cloned();
        }
        match done {
            Ok(text) => self.say(text),
            Err(why) => self.say(why.lines().next().unwrap_or_default().to_owned()),
        }
    }

    fn submit(&mut self, typed: &str, source: &dyn Source) {
        if typed.trim().is_empty() {
            return;
        }
        match prompt::parse(typed, &self.home) {
            Ok(script) => self.start(script, source),
            Err(e) => self.say(e),
        }
    }

    fn start(&mut self, script: crate::config::Script, source: &dyn Source) {
        self.recent.add(script.clone());
        if let Err(e) = self.history.save(&self.recent) {
            self.say(format!("couldn't save the run history: {e}"));
        }
        let targets = self.targets();
        let asked = ask::missing(&script);
        if asked.is_empty() {
            let runs = targets.into_iter().map(|i| (i, script.clone())).collect();
            return self.run(source, &script, runs);
        }
        let accounts = targets
            .into_iter()
            .filter_map(|i| Some((i, self.names.get(i)?.clone())))
            .collect();
        self.ask = Some(Ask::new(
            script,
            asked,
            accounts,
            self.share_args,
            &self.remembered,
        ));
    }

    fn run(
        &mut self,
        source: &dyn Source,
        script: &crate::config::Script,
        runs: Vec<(usize, crate::config::Script)>,
    ) {
        let shown = prompt::shown_path(&self.home, &script.path);
        let targets = runs.iter().map(|(i, _)| *i).collect();
        let script_for = |i: usize| {
            let script = runs.iter().find(|(at, _)| *at == i).map(|(_, s)| s.clone());
            Action::Run(script.expect("each target has its run"))
        };
        self.act_on(source, targets, &format!("running {shown}"), script_for);
    }
}

fn skua_script(script: Option<&str>) -> Option<&Path> {
    let script = Path::new(script?);
    script
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cs"))
        .then_some(script)
}

fn complete(query: &str, names: &[&str]) -> String {
    if let [only] = names {
        return (*only).to_owned();
    }
    let mut lower: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    lower.sort();
    let prefix = prompt::shared_prefix(&lower);
    if prefix.contains(&query.to_lowercase()) {
        prefix.to_owned()
    } else {
        query.to_owned()
    }
}
