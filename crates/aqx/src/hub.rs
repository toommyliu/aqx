use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use aqx_core::{Event, Phase, State, StopHandle, World};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::output::{self, clock};

pub const LOG_LINES: usize = 1000;
const WINDOW: Duration = Duration::from_secs(600);
const GOLD_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub t: u64,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Run {
        script: PathBuf,
        #[serde(default)]
        args: Vec<(String, String)>,
    },
    Stop,
    Restart,
    Pause,
    Resume,
    Logout,
    Relogin,
    Chat {
        text: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Print {
    Quiet,
    Plain,
    Json,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub bot: String,
    pub user: String,
    pub server: String,
    pub phase: String,
    pub detail: String,
    pub script: Option<String>,
    pub run: String,
    pub paused: bool,
    pub stalled: bool,
    pub status: String,
    pub progress: Vec<Bar>,
    pub quiet_secs: u64,
    pub up_secs: u64,
    pub kills_h: Option<u64>,
    pub drops_h: Option<u64>,
    pub gold_h: Option<i64>,
    pub deaths: usize,
    pub game: Option<Game>,
}

impl Snapshot {
    pub fn state(&self) -> &str {
        match self.phase.as_str() {
            "READY" if self.paused => "PAUSED",
            "READY" if self.stalled => "STALLED",
            phase => phase,
        }
    }

    pub fn script_label(&self) -> String {
        let Some(script) = &self.script else { return "idle".into() };
        let name = std::path::Path::new(script).file_name().map_or(script.clone(), |n| n.to_string_lossy().into_owned());
        match self.run.as_str() {
            "running" => name,
            run => format!("{name} ({run})"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub label: String,
    pub current: u32,
    pub total: u32,
    pub eta_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Game {
    pub area: String,
    pub cell: String,
    pub pad: String,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub dead: bool,
    pub afk: bool,
    pub class: String,
    pub level: u32,
    pub gold: i64,
    pub skills: Vec<SkillState>,
    pub auras: Vec<String>,
    pub target: Option<String>,
    pub cell_monsters: (usize, usize),
    pub map_monsters: (usize, usize),
    pub players: usize,
    pub cell_players: Vec<String>,
    pub drops: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillState {
    pub name: String,
    pub ready_pct: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Run {
    Idle,
    Running,
    Done,
    Failed,
    Stopped,
}

struct Rate {
    label: String,
    current: u32,
    total: u32,
    per_sec: f64,
    at: Instant,
}

pub struct Board {
    pub snapshot: Option<Snapshot>,
    pub log: VecDeque<LogLine>,
    pub next_seq: u64,
    pub run: Run,
    pub script: Option<String>,
    pub status: String,
    pub paused: bool,
    pub stalled: bool,
    pub ended: bool,
    pub server: String,
    pub ready_since: Option<Instant>,
    pub first_ready: Option<Instant>,
    pub last_progress: Instant,
    bars: Vec<Rate>,
    kills: VecDeque<Instant>,
    drops: VecDeque<Instant>,
    deaths: VecDeque<Instant>,
    gold: VecDeque<(Instant, i64)>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            snapshot: None,
            log: VecDeque::new(),
            next_seq: 0,
            run: Run::Idle,
            script: None,
            status: String::new(),
            paused: false,
            stalled: false,
            ended: false,
            server: String::new(),
            ready_since: None,
            first_ready: None,
            last_progress: Instant::now(),
            bars: Vec::new(),
            kills: VecDeque::new(),
            drops: VecDeque::new(),
            deaths: VecDeque::new(),
            gold: VecDeque::new(),
        }
    }
}

impl Board {
    pub fn push_log(&mut self, t: u64, source: &str, text: String) -> LogLine {
        let line = LogLine { seq: self.next_seq, t, source: source.into(), text };
        self.next_seq += 1;
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.clone());
        line
    }

    pub fn progressed(&mut self, now: Instant) {
        self.last_progress = now;
        self.stalled = false;
    }

    pub fn start_script(&mut self, script: String, now: Instant) {
        self.script = Some(script);
        self.run = Run::Running;
        self.status.clear();
        self.bars.clear();
        self.progressed(now);
    }

    fn note(&mut self, event: &Event, now: Instant) {
        match event {
            Event::Killed { .. } => {
                self.kills.push_back(now);
                self.progressed(now);
            }
            Event::DropOffered { .. } => self.drops.push_back(now),
            Event::Died => self.deaths.push_back(now),
            Event::ItemChanged { .. } | Event::QuestCompleted { .. } | Event::Joined { .. } => self.progressed(now),
            Event::Status { text } => self.status.clone_from(text),
            Event::Progress { label, current, total } => self.advance(label, *current, *total, now),
            _ => {}
        }
        let cutoff = now.checked_sub(WINDOW).unwrap_or(now);
        for times in [&mut self.kills, &mut self.drops, &mut self.deaths] {
            while times.front().is_some_and(|t| *t < cutoff) {
                times.pop_front();
            }
        }
    }

    fn advance(&mut self, label: &str, current: u32, total: u32, now: Instant) {
        let Some(bar) = self.bars.iter_mut().find(|b| b.label == label) else {
            self.bars.push(Rate { label: label.into(), current, total, per_sec: 0.0, at: now });
            return;
        };
        let secs = now.saturating_duration_since(bar.at).as_secs_f64();
        if current > bar.current && secs > 0.0 {
            let rate = f64::from(current - bar.current) / secs;
            let weight = 1.0 - (-secs / WINDOW.as_secs_f64()).exp();
            bar.per_sec = if bar.per_sec == 0.0 { rate } else { bar.per_sec + weight * (rate - bar.per_sec) };
            self.last_progress = now;
            self.stalled = false;
        }
        if current != bar.current {
            bar.at = now;
        }
        bar.current = current;
        bar.total = total;
    }

    pub fn sample_gold(&mut self, world: &World, now: Instant) {
        let Some(me) = &world.me else { return };
        if self.gold.back().is_none_or(|(t, _)| now.saturating_duration_since(*t) >= GOLD_EVERY) {
            self.gold.push_back((now, me.gold));
        }
        let cutoff = now.checked_sub(WINDOW).unwrap_or(now);
        while self.gold.front().is_some_and(|(t, _)| *t < cutoff) {
            self.gold.pop_front();
        }
    }

    pub fn snapshot(&self, bot: &str, user: &str, phase: &Phase, world: &World, now: Instant) -> Snapshot {
        let window = self.first_ready.map_or(Duration::ZERO, |t| now.saturating_duration_since(t)).min(WINDOW);
        let per_hour = |n: usize| (window >= Duration::from_secs(60)).then(|| (n as f64 * 3600.0 / window.as_secs_f64()) as u64);
        let gold_h = match (self.gold.front(), self.gold.back()) {
            (Some((t0, g0)), Some((t1, g1))) if t1.saturating_duration_since(*t0) >= Duration::from_secs(60) => {
                Some(((g1 - g0) as f64 * 3600.0 / t1.saturating_duration_since(*t0).as_secs_f64()) as i64)
            }
            _ => None,
        };
        let (phase, detail) = describe_phase(phase, now);
        Snapshot {
            bot: bot.into(),
            user: user.into(),
            server: self.server.clone(),
            phase: if self.ended { "LOGGED OUT".into() } else { phase },
            detail,
            script: self.script.clone(),
            run: match self.run {
                Run::Idle => "idle",
                Run::Running => "running",
                Run::Done => "done",
                Run::Failed => "failed",
                Run::Stopped => "stopped",
            }
            .into(),
            paused: self.paused,
            stalled: self.stalled,
            status: self.status.clone(),
            progress: self
                .bars
                .iter()
                .map(|b| Bar {
                    label: b.label.clone(),
                    current: b.current,
                    total: b.total,
                    eta_secs: (b.per_sec > 0.0 && b.total > b.current).then(|| (f64::from(b.total - b.current) / b.per_sec) as u64),
                })
                .collect(),
            quiet_secs: now.saturating_duration_since(self.last_progress).as_secs(),
            up_secs: self.ready_since.map_or(0, |t| now.saturating_duration_since(t).as_secs()),
            kills_h: per_hour(self.kills.len()),
            drops_h: per_hour(self.drops.len()),
            gold_h,
            deaths: self.deaths.len(),
            game: game(world, now),
        }
    }
}

fn describe_phase(phase: &Phase, now: Instant) -> (String, String) {
    match phase {
        Phase::Connecting => ("CONNECTING".into(), String::new()),
        Phase::Handshaking | Phase::LoggingIn => ("LOGGING IN".into(), String::new()),
        Phase::Loading => ("LOADING".into(), String::new()),
        Phase::Ready => ("READY".into(), String::new()),
        Phase::Closed(why) => ("CLOSED".into(), why.to_string()),
        Phase::Waiting { reason, until } => {
            ("RECONNECTING".into(), format!("in {}: {reason}", output::span(until.saturating_duration_since(now).as_secs())))
        }
    }
}

fn game(world: &World, now: Instant) -> Option<Game> {
    let me = world.me.as_ref()?;
    let area = world.area.as_ref();
    let count = |cell: Option<&str>| {
        let monsters = area.into_iter().flat_map(|a| &a.monsters).filter(|m| cell.is_none_or(|c| m.cell == c));
        monsters.fold((0, 0), |(total, alive), m| (total + 1, alive + usize::from(m.alive())))
    };
    let target = world.target.and_then(|id| area?.monsters.iter().find(|m| m.map_id == id)).map(|m| {
        let pct = if m.vitals.max_hp == 0 { 0 } else { u64::from(m.vitals.hp) * 100 / u64::from(m.vitals.max_hp) };
        format!("{} #{} {pct}%", m.name, m.map_id)
    });
    Some(Game {
        area: area.map(|a| a.name.clone()).unwrap_or_default(),
        cell: me.cell.clone(),
        pad: me.pad.clone(),
        hp: me.vitals.hp,
        max_hp: me.vitals.max_hp,
        mp: me.vitals.mp,
        max_mp: me.vitals.max_mp,
        dead: me.vitals.state == State::Dead,
        afk: me.afk,
        class: me.class.clone(),
        level: me.level,
        gold: me.gold,
        skills: me
            .skills
            .iter()
            .skip(1)
            .take(4)
            .map(|s| SkillState { name: s.name.clone(), ready_pct: (world.recharged(s, now) * 100.0) as u8 })
            .collect(),
        auras: me.auras.iter().map(|a| if a.stacks > 1 { format!("{} x{}", a.name, a.stacks) } else { a.name.clone() }).collect(),
        target,
        cell_monsters: count(Some(&me.cell)),
        map_monsters: count(None),
        players: area.map_or(0, |a| a.players.len()) + 1,
        cell_players: area.into_iter().flat_map(|a| &a.players).filter(|p| p.cell == me.cell).map(|p| format!("{} Lv {}", p.name, p.level)).collect(),
        drops: world.drops.iter().map(|d| format!("{} x{}", d.name, d.qty)).collect(),
    })
}

pub struct BotCell {
    pub name: String,
    pub board: Mutex<Board>,
    stop: Option<StopHandle>,
    commands: Mutex<VecDeque<Command>>,
}

impl BotCell {
    pub fn local(name: &str, stop: StopHandle) -> Self {
        BotCell { name: name.into(), board: Mutex::new(Board::default()), stop: Some(stop), commands: Mutex::new(VecDeque::new()) }
    }

    pub fn mirror(name: &str) -> Self {
        BotCell { name: name.into(), board: Mutex::new(Board::default()), stop: None, commands: Mutex::new(VecDeque::new()) }
    }

    pub fn board(&self) -> MutexGuard<'_, Board> {
        self.board.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        let Some(stop) = &self.stop else { return Err(format!("{} runs in another process", self.name)) };
        if self.board().ended {
            return Err(format!("{} has logged out", self.name));
        }
        match command {
            Command::Pause | Command::Resume => {
                let paused = matches!(command, Command::Pause);
                self.board().paused = paused;
                stop.post(move |s| s.set_paused(paused));
            }
            Command::Chat { text } => stop.post(move |s| s.chat(&text)),
            command => {
                self.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(command);
                stop.stop();
            }
        }
        Ok(())
    }

    pub fn take_command(&self) -> Option<Command> {
        self.commands.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }

    pub fn has_commands(&self) -> bool {
        !self.commands.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }
}

pub struct Hub {
    pub name: String,
    pub cells: Vec<Arc<BotCell>>,
    watchers: AtomicUsize,
    print: Print,
}

impl Hub {
    pub fn new(name: &str, cells: Vec<Arc<BotCell>>, print: Print) -> Self {
        Hub { name: name.into(), cells, watchers: AtomicUsize::new(0), print }
    }

    pub fn cell(&self, name: &str) -> Option<&Arc<BotCell>> {
        self.cells.iter().find(|c| c.name == name)
    }

    pub fn watched(&self) -> bool {
        self.watchers.load(Ordering::Relaxed) > 0
    }

    pub fn watch(self: &Arc<Self>) -> Watching {
        self.watchers.fetch_add(1, Ordering::Relaxed);
        Watching(self.clone())
    }

    pub fn record(&self, cell: &BotCell, event: &Event) {
        let now = Instant::now();
        let t = output::now();
        let line = {
            let mut board = cell.board();
            board.note(event, now);
            output::describe(event).map(|(source, text)| board.push_log(t, source, text))
        };
        match self.print {
            Print::Json => self.json(cell, serde_json::to_value(event).unwrap_or(Value::Null)),
            Print::Plain => {
                if let Some(line) = line {
                    self.plain(cell, &line);
                }
            }
            Print::Quiet => {}
        }
    }

    pub fn say(&self, cell: &BotCell, source: &str, text: impl Into<String>) {
        let line = cell.board().push_log(output::now(), source, text.into());
        match self.print {
            Print::Json => self.json(cell, json!({ "event": "log", "source": line.source, "text": line.text })),
            Print::Plain => self.plain(cell, &line),
            Print::Quiet => {}
        }
    }

    pub fn status(&self, cell: &BotCell, world: &World) {
        let Some(me) = &world.me else { return };
        let area = world.area.as_ref().map_or("", |a| a.name.as_str());
        let players = world.area.as_ref().map_or(0, |a| a.players.len()) + 1;
        let v = me.vitals;
        match self.print {
            Print::Json => self.json(
                cell,
                json!({
                    "event": "status", "area": area, "cell": me.cell, "pad": me.pad,
                    "hp": v.hp, "max_hp": v.max_hp, "mp": v.mp, "max_mp": v.max_mp,
                    "items": world.inventory.len(), "players": players,
                }),
            ),
            Print::Plain => {
                let text = format!(
                    "{area} {}/{}, hp {}/{}, mp {}/{}, {} items, {players} players",
                    me.cell,
                    me.pad,
                    v.hp,
                    v.max_hp,
                    v.mp,
                    v.max_mp,
                    world.inventory.len()
                );
                self.plain(cell, &LogLine { seq: 0, t: output::now(), source: "aqx".into(), text });
            }
            Print::Quiet => {}
        }
    }

    fn plain(&self, cell: &BotCell, line: &LogLine) {
        let mut out = std::io::stdout().lock();
        let _ = if self.cells.len() > 1 {
            writeln!(out, "{}  {:<10}  {:<6}  {}", clock(line.t), cell.name, line.source, line.text)
        } else {
            writeln!(out, "{}  {:<6}  {}", clock(line.t), line.source, line.text)
        };
    }

    fn json(&self, cell: &BotCell, mut value: Value) {
        if let Value::Object(fields) = &mut value {
            fields.insert("t".into(), output::now().into());
            fields.insert("bot".into(), cell.name.clone().into());
        }
        let _ = writeln!(std::io::stdout().lock(), "{value}");
    }
}

pub struct Watching(Arc<Hub>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.watchers.fetch_sub(1, Ordering::Relaxed);
    }
}
