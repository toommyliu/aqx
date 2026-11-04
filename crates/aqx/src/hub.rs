use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use aqx_core::{Event, Faction, Item, Phase, Pool, Spot, State, StopHandle, World};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::output::{self, time_of_day};

pub const LOG_LINES: usize = 1000;
const WINDOW: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: u64,
    pub t: u64,
    pub source: String,
    pub text: String,
}

impl LogLine {
    pub fn is_game(&self) -> bool {
        self.source == "game"
    }
}

#[derive(Default)]
pub struct Log {
    lines: VecDeque<LogLine>,
    game: usize,
}

impl Log {
    pub fn push(&mut self, line: LogLine) {
        let game = line.is_game();
        self.game += usize::from(game);
        self.lines.push_back(line);
        let kept = if game { self.game } else { self.lines.len() - self.game };
        if kept > LOG_LINES
            && let Some(at) = self.lines.iter().position(|l| l.is_game() == game)
        {
            self.lines.remove(at);
            self.game -= usize::from(game);
        }
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, LogLine> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn last(&self) -> Option<&LogLine> {
        self.lines.back()
    }

    pub fn clear(&mut self) {
        *self = Log::default();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Login,
    Run {
        script: PathBuf,
        #[serde(default)]
        args: Vec<(String, String)>,
    },
    Stop,
    Restart,
    Pause,
    Resume,
    PickUp {
        items: Vec<String>,
    },
    Logout,
    Relogin,
}

impl Command {
    pub fn unchanged(&self) -> &'static str {
        match self {
            Command::Login => "already online",
            Command::Logout => "already offline",
            _ => "nothing to do",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Print {
    Quiet,
    Plain,
    Json,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub account: String,
    pub user: String,
    pub server: String,
    pub phase: String,
    pub detail: String,
    pub script: Option<String>,
    #[serde(default)]
    pub args: Vec<(String, String)>,
    pub run: String,
    pub paused: bool,
    pub stalled: bool,
    pub status: String,
    pub progress: Vec<Bar>,
    pub up_secs: u64,
    pub kills: u64,
    pub drops: u64,
    pub deaths: u64,
    pub gold_gained: Option<i64>,
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
    pub class_rank: u32,
    pub level: u32,
    pub gold: i64,
    pub skills: Vec<SkillState>,
    pub auras: Vec<Aura>,
    pub target: Option<Target>,
    pub cell_players: Vec<Player>,
    pub drops: Vec<Stack>,
    pub quests: Vec<QuestProgress>,
    pub factions: Vec<Standing>,
    pub temp: Vec<Stack>,
    pub inventory: Vec<Stack>,
    pub bag_used: u32,
    pub bag_slots: u32,
    pub misc_used: u32,
    pub misc_slots: u32,
    pub bank: Option<Vec<Stack>>,
    pub bank_used: u32,
    pub bank_slots: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Aura {
    pub name: String,
    pub stacks: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub hp: u32,
    pub max_hp: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub level: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuestProgress {
    pub name: String,
    pub have: u32,
    pub need: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub name: String,
    pub rank: u32,
    pub rep: u32,
    pub next: u32,
    pub gained: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub name: String,
    pub qty: u32,
    pub max: u32,
    pub equipped: bool,
    pub rank: Option<u32>,
}

impl Stack {
    fn of(item: &Item) -> Self {
        Stack { name: item.name.clone(), qty: item.qty, max: item.max_stack, equipped: item.equipped, rank: item.class_rank() }
    }
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
    pub log: Log,
    pub next_seq: u64,
    pub run: Run,
    pub script: Option<String>,
    pub args: Vec<(String, String)>,
    pub status: String,
    pub paused: bool,
    pub stalled: bool,
    pub online: bool,
    pub leaving: bool,
    pub server: String,
    pub spot: Option<Spot>,
    pub ready_since: Option<Instant>,
    pub last_progress: Instant,
    bars: Vec<Rate>,
    kills: u64,
    drops: u64,
    deaths: u64,
    gold_from: Option<i64>,
    gold_now: i64,
    touched: HashMap<String, Instant>,
    gained: HashMap<String, Instant>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            snapshot: None,
            log: Log::default(),
            next_seq: 0,
            run: Run::Idle,
            script: None,
            args: Vec::new(),
            status: String::new(),
            paused: false,
            stalled: false,
            online: false,
            leaving: false,
            server: String::new(),
            spot: None,
            ready_since: None,
            last_progress: Instant::now(),
            bars: Vec::new(),
            kills: 0,
            drops: 0,
            deaths: 0,
            gold_from: None,
            gold_now: 0,
            touched: HashMap::new(),
            gained: HashMap::new(),
        }
    }
}

impl Board {
    pub fn push_log(&mut self, t: u64, source: &str, text: String) -> LogLine {
        let line = LogLine { seq: self.next_seq, t, source: source.into(), text };
        self.next_seq += 1;
        self.log.push(line.clone());
        line
    }

    pub fn progressed(&mut self, now: Instant) {
        self.last_progress = now;
        self.stalled = false;
    }

    pub fn start_script(&mut self, script: String, args: Vec<(String, String)>, now: Instant) {
        self.script = Some(script);
        self.args = args;
        self.run = Run::Running;
        self.status.clear();
        self.bars.clear();
        self.progressed(now);
    }

    fn note(&mut self, event: &Event, now: Instant) {
        match event {
            Event::Killed { .. } => {
                self.kills += 1;
                self.progressed(now);
            }
            Event::DropOffered { item, .. } => {
                self.drops += 1;
                self.touched.insert(item.clone(), now);
            }
            Event::Died => self.deaths += 1,
            Event::ItemChanged { item, .. } => {
                self.touched.insert(item.clone(), now);
                self.progressed(now);
            }
            Event::RepGained { faction, .. } => {
                self.gained.insert(faction.clone(), now);
            }
            Event::QuestCompleted { .. } | Event::Joined { .. } => self.progressed(now),
            Event::Status { text } => self.status.clone_from(text),
            Event::Progress { label, current, total } => self.advance(label, *current, *total, now),
            _ => {}
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

    pub fn sample_gold(&mut self, world: &World) {
        if let Some(me) = &world.me {
            self.gold_from.get_or_insert(me.gold);
            self.gold_now = me.gold;
        }
    }

    pub fn reset_stats(&mut self) {
        self.kills = 0;
        self.drops = 0;
        self.deaths = 0;
        self.gold_from = None;
    }

    pub fn snapshot(&self, account: &str, user: &str, phase: &Phase, world: &World, now: Instant) -> Snapshot {
        let (phase, detail) = describe_phase(phase, now);
        Snapshot {
            account: account.into(),
            user: user.into(),
            server: self.server.clone(),
            phase: if self.online { phase } else { "OFFLINE".into() },
            detail,
            script: self.script.clone(),
            args: self.args.clone(),
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
            up_secs: self.ready_since.filter(|_| self.online).map_or(0, |t| now.saturating_duration_since(t).as_secs()),
            kills: self.kills,
            drops: self.drops,
            deaths: self.deaths,
            gold_gained: self.gold_from.map(|from| self.gold_now - from),
            game: if self.online { game(world, &self.touched, &self.gained, now) } else { None },
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

fn game(world: &World, touched: &HashMap<String, Instant>, gained: &HashMap<String, Instant>, now: Instant) -> Option<Game> {
    let me = world.me.as_ref()?;
    let latest_first = |items: &[Item]| -> Vec<Stack> {
        let mut items: Vec<&Item> = items.iter().collect();
        items.sort_by_key(|i| Reverse(touched.get(&i.name)));
        items.into_iter().map(Stack::of).collect()
    };
    let mut factions: Vec<&Faction> = world.factions.iter().collect();
    factions.sort_by_key(|f| (Reverse(gained.get(&f.name)), &f.name));
    let area = world.area.as_ref();
    let target = world
        .target
        .and_then(|id| area?.monsters.iter().find(|m| m.map_id == id))
        .map(|m| Target { name: m.name.clone(), hp: m.vitals.hp, max_hp: m.vitals.max_hp });
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
        class_rank: me.class_rank(),
        level: me.level,
        gold: me.gold,
        skills: me
            .skills
            .iter()
            .skip(1)
            .take(4)
            .map(|s| SkillState { name: s.name.clone(), ready_pct: (world.recharged(s, now) * 100.0) as u8 })
            .collect(),
        auras: me.auras.iter().map(|a| Aura { name: a.name.clone(), stacks: a.stacks }).collect(),
        target,
        cell_players: area
            .into_iter()
            .flat_map(|a| &a.players)
            .filter(|p| p.cell == me.cell)
            .map(|p| Player { name: p.name.clone(), level: p.level })
            .collect(),
        drops: latest_first(&world.drops),
        quests: world
            .active
            .iter()
            .filter_map(|id| world.quests.get(id))
            .map(|q| QuestProgress {
                name: q.name.clone(),
                have: q.turn_in.iter().map(|n| world.count(n.item).min(n.qty)).sum(),
                need: q.turn_in.iter().map(|n| n.qty).sum(),
            })
            .collect(),
        factions: factions
            .into_iter()
            .map(|f| Standing { name: f.name.clone(), rank: f.rank(), rep: f.rep_in_rank(), next: f.rep_for_next_rank(), gained: gained.contains_key(&f.name) })
            .collect(),
        temp: latest_first(&world.temp),
        inventory: latest_first(&world.inventory),
        bag_used: world.used(Pool::Bag),
        bag_slots: me.bag_slots,
        misc_used: world.used(Pool::Misc),
        misc_slots: world.misc_slots,
        bank: world.bank_loaded.then(|| latest_first(&world.bank)),
        bank_used: world.bank_count,
        bank_slots: me.bank_slots,
    })
}

pub struct AccountCell {
    pub name: String,
    pub board: Mutex<Board>,
    stop: Option<StopHandle>,
    inbox: Mutex<Inbox>,
    wake: Condvar,
}

#[derive(Default)]
struct Inbox {
    commands: VecDeque<Command>,
    closed: bool,
    commanded: bool,
}

impl AccountCell {
    pub fn local(name: &str, stop: StopHandle) -> Self {
        AccountCell { name: name.into(), board: Mutex::new(Board::default()), stop: Some(stop), inbox: Mutex::default(), wake: Condvar::new() }
    }

    pub fn mirror(name: &str) -> Self {
        AccountCell { name: name.into(), board: Mutex::new(Board::default()), stop: None, inbox: Mutex::default(), wake: Condvar::new() }
    }

    pub fn board(&self) -> MutexGuard<'_, Board> {
        self.board.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn send(&self, command: Command) -> Result<bool, String> {
        let Some(stop) = &self.stop else { return Err(format!("{} runs in another process", self.name)) };
        let mut board = self.board();
        let mut inbox = self.inbox();
        inbox.commanded |= matches!(command, Command::Login | Command::Run { .. } | Command::Logout);
        match &command {
            Command::Login if board.online => {
                let leaving = board.leaving || inbox.commands.iter().any(|c| matches!(c, Command::Logout));
                if leaving {
                    inbox.commands.push_back(command);
                }
                return Ok(leaving);
            }
            Command::Logout if !board.online => return Ok(false),
            Command::Login | Command::Run { .. } => board.online = true,
            _ if !board.online => return Err(format!("{} is offline", self.name)),
            Command::Pause | Command::Resume => {
                let paused = matches!(command, Command::Pause);
                board.paused = paused;
                stop.post(move |s| s.set_paused(paused));
                return Ok(true);
            }
            Command::PickUp { items } => {
                let items = items.clone();
                stop.post(move |s| items.iter().for_each(|item| s.request_pickup(item)));
                return Ok(true);
            }
            _ => {}
        }
        inbox.commands.push_back(command);
        drop((board, inbox));
        self.wake.notify_one();
        stop.stop();
        Ok(true)
    }

    pub fn take_command(&self) -> Option<Command> {
        self.inbox().commands.pop_front()
    }

    pub fn has_commands(&self) -> bool {
        !self.inbox().commands.is_empty()
    }

    pub fn next_login(&self) -> Option<Command> {
        let mut inbox = self.inbox();
        loop {
            if inbox.closed {
                return None;
            }
            match inbox.commands.pop_front() {
                Some(command @ (Command::Login | Command::Run { .. })) => return Some(command),
                Some(_) => {}
                None => inbox = self.wake.wait(inbox).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }

    pub fn go_offline(&self) {
        let mut board = self.board();
        board.online = self.inbox().commands.iter().any(|c| matches!(c, Command::Login | Command::Run { .. }));
        board.leaving = false;
        board.spot = None;
        board.stalled = false;
        board.paused = false;
        if board.run == Run::Running {
            board.run = Run::Stopped;
        }
    }

    pub fn offline(&self) -> bool {
        !self.board().online && self.inbox().commands.is_empty()
    }

    pub fn commanded(&self) -> bool {
        self.inbox().commanded
    }

    pub fn close(&self) {
        let mut inbox = self.inbox();
        inbox.closed = true;
        inbox.commands.clear();
        drop(inbox);
        self.wake.notify_one();
        if let Some(stop) = &self.stop {
            stop.stop();
        }
    }
}

#[derive(Default)]
pub struct Cells(RwLock<Vec<Arc<AccountCell>>>);

impl Cells {
    pub fn new(cells: Vec<Arc<AccountCell>>) -> Self {
        Cells(RwLock::new(cells))
    }

    pub fn all(&self) -> Vec<Arc<AccountCell>> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn add(&self, cell: Arc<AccountCell>) {
        self.0.write().unwrap_or_else(|e| e.into_inner()).push(cell);
    }
}

pub struct Hub {
    pub name: String,
    pub cells: Cells,
    watchers: AtomicUsize,
    ending: Mutex<bool>,
    print: Print,
}

impl Hub {
    pub fn new(name: &str, cells: Vec<Arc<AccountCell>>, print: Print) -> Self {
        Hub { name: name.into(), cells: Cells::new(cells), watchers: AtomicUsize::new(0), ending: Mutex::new(false), print }
    }

    pub fn send(&self, account: &str, command: Command) -> Result<bool, String> {
        let ending = self.ending.lock().unwrap_or_else(|e| e.into_inner());
        if *ending {
            return Err(format!("{} is ending", self.name));
        }
        let cell = self.cells.all().into_iter().find(|c| c.name == account).ok_or_else(|| format!("no account named {account} in {}", self.name))?;
        cell.send(command)
    }

    pub fn end_if_idle(&self) -> bool {
        let mut ending = self.ending.lock().unwrap_or_else(|e| e.into_inner());
        let cells = self.cells.all();
        *ending |= cells.iter().all(|c| c.offline()) && cells.iter().any(|c| c.commanded());
        *ending
    }

    pub fn ended(&self) -> bool {
        *self.ending.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn watched(&self) -> bool {
        self.watchers.load(Ordering::Relaxed) > 0
    }

    pub fn let_watchers_finish(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while self.watched() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn watch(self: &Arc<Self>) -> Watching {
        self.watchers.fetch_add(1, Ordering::Relaxed);
        Watching(self.clone())
    }

    pub fn record(&self, cell: &AccountCell, event: &Event) {
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

    pub fn say(&self, cell: &AccountCell, source: &str, text: impl Into<String>) {
        let line = cell.board().push_log(output::now(), source, text.into());
        match self.print {
            Print::Json => self.json(cell, json!({ "event": "log", "source": line.source, "text": line.text })),
            Print::Plain => self.plain(cell, &line),
            Print::Quiet => {}
        }
    }

    pub fn status(&self, cell: &AccountCell, world: &World) {
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

    fn plain(&self, cell: &AccountCell, line: &LogLine) {
        let mut out = std::io::stdout().lock();
        let _ = if self.cells.all().len() > 1 {
            writeln!(out, "{}  {:<10}  {:<6}  {}", time_of_day(line.t), cell.name, line.source, line.text)
        } else {
            writeln!(out, "{}  {:<6}  {}", time_of_day(line.t), line.source, line.text)
        };
    }

    fn json(&self, cell: &AccountCell, mut value: Value) {
        if let Value::Object(fields) = &mut value {
            fields.insert("t".into(), output::now().into());
            fields.insert("account".into(), cell.name.clone().into());
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

#[cfg(test)]
mod tests {
    use aqx_protocol::Message;

    use super::*;

    fn world(gold: Option<i64>) -> World {
        let mut world = World::default();
        let mut feed = vec!["%xt%loginResponse%-1%true%1001%hero%Welcome!%".to_string()];
        feed.extend(gold.map(|g| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{{"cmd":"initUserData","uid":1001,"data":{{"intGold":{g}}}}}}}}}"#)));
        for text in feed {
            aqx_core::reduce(&mut world, &Message::parse(&text).unwrap(), Instant::now());
        }
        world
    }

    #[test]
    fn gold_gained_holds_its_last_value_while_reconnecting() {
        let now = Instant::now();
        let mut board = Board { online: true, ..Board::default() };
        board.sample_gold(&world(Some(1000)));
        let reloading = world(None);
        assert_eq!(reloading.me.as_ref().map(|m| m.gold), Some(0));
        assert_eq!(board.snapshot("main", "hero", &Phase::Loading, &reloading, now).gold_gained, Some(0));
        board.sample_gold(&world(Some(1500)));
        assert_eq!(board.snapshot("main", "hero", &Phase::Ready, &world(Some(1500)), now).gold_gained, Some(500));
    }

    #[test]
    fn item_lists_put_what_changed_last_first() {
        let start = Instant::now();
        let mut board = Board { online: true, ..Board::default() };
        let mut world = world(Some(0));
        let json = |o: &str| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#);
        let messages = [
            r#"{"cmd":"loadInventoryBig","items":[{"ItemID":1,"CharItemID":11,"sName":"Bone Dust","iQty":3,"iStk":50},
                {"ItemID":2,"CharItemID":12,"sName":"Rogue","iQty":302500,"iStk":1,"bEquip":1,"sType":"Class"},
                {"ItemID":3,"CharItemID":13,"sName":"Fabric","iQty":5,"iStk":99}],"factions":[]}"#,
            r#"{"cmd":"addItems","items":{"3":{"iQty":1,"iQtyNow":6,"CharItemID":13,"bBank":0}}}"#,
            r#"{"cmd":"dropItem","items":{"7":{"ItemID":7,"sName":"Old Map","iQty":1}}}"#,
            r#"{"cmd":"dropItem","items":{"8":{"ItemID":8,"sName":"Wolf Pelt","iQty":2}}}"#,
        ];
        for (i, message) in messages.iter().enumerate() {
            let now = start + Duration::from_secs(i as u64);
            for event in aqx_core::reduce(&mut world, &Message::parse(&json(message)).unwrap(), now) {
                board.note(&event, now);
            }
        }
        let game = board.snapshot("main", "hero", &Phase::Ready, &world, start).game.unwrap();
        let names = |stacks: &[Stack]| stacks.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&game.inventory), ["Fabric", "Bone Dust", "Rogue"], "items nothing touched keep the server's order");
        assert_eq!(names(&game.drops), ["Wolf Pelt", "Old Map"], "the newest drop comes first");
        assert_eq!((game.inventory[0].qty, game.inventory[0].max, game.inventory[2].equipped, game.inventory[2].rank), (6, 99, true, Some(10)));
        assert_eq!(game.bank, None, "the bank is unknown until it is opened");
        assert_eq!((game.inventory.len(), game.bag_used, game.misc_used, game.misc_slots), (3, 2, 0, 100), "a class takes no bag slot");
    }
    #[test]
    fn factions_list_what_gained_rep_first_then_by_name() {
        let start = Instant::now();
        let mut board = Board { online: true, ..Board::default() };
        let mut world = world(Some(0));
        let json = |o: &str| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#);
        let messages = [
            r#"{"cmd":"loadInventoryBig","items":[],"factions":[{"FactionID":2,"sName":"Chaos","iRep":302500},
                {"FactionID":3,"sName":"Brightoak","iRep":1000},{"FactionID":4,"sName":"Aegis","iRep":0}]}"#,
            r#"{"cmd":"addGoldExp","iRep":250,"FactionID":3,"typ":"q"}"#,
            r#"{"cmd":"addGoldExp","iRep":250,"FactionID":2,"typ":"q"}"#,
        ];
        for (i, message) in messages.iter().enumerate() {
            let now = start + Duration::from_secs(i as u64);
            for event in aqx_core::reduce(&mut world, &Message::parse(&json(message)).unwrap(), now) {
                board.note(&event, now);
            }
        }
        let game = board.snapshot("main", "hero", &Phase::Ready, &world, start).game.unwrap();
        let shown: Vec<(&str, u32, u32, u32, bool)> = game.factions.iter().map(|f| (f.name.as_str(), f.rank, f.rep, f.next, f.gained)).collect();
        assert_eq!(shown, [("Chaos", 10, 0, 0, true), ("Brightoak", 2, 350, 2700, true), ("Aegis", 1, 0, 900, false)]);
    }

    #[test]
    fn accepted_quests_count_what_is_held_toward_each_turn_in() {
        let mut world = world(Some(0));
        let json = |o: &str| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#);
        for message in [
            r#"{"cmd":"getQuests","quests":{
                "11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"turnin":[{"ItemID":87,"iQty":1}]},
                "12":{"QuestID":12,"sName":"Spare Parts","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"turnin":[{"ItemID":3,"iQty":4},{"ItemID":4,"iQty":10}]},
                "13":{"QuestID":13,"sName":"Never Taken","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"turnin":[]}}}"#,
            r#"{"cmd":"loadInventoryBig","items":[{"ItemID":3,"CharItemID":13,"sName":"Fabric","iQty":6,"iStk":99},
                {"ItemID":4,"CharItemID":14,"sName":"Bolt","iQty":7,"iStk":99}],"factions":[]}"#,
            r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":12,"msg":"success"}"#,
            r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#,
        ] {
            aqx_core::reduce(&mut world, &Message::parse(&json(message)).unwrap(), Instant::now());
        }
        let board = Board { online: true, ..Board::default() };
        let game = board.snapshot("main", "hero", &Phase::Ready, &world, Instant::now()).game.unwrap();
        let shown: Vec<(&str, u32, u32)> = game.quests.iter().map(|q| (q.name.as_str(), q.have, q.need)).collect();
        assert_eq!(shown, [("Spare Parts", 11, 14), ("Twilly's New Staff", 0, 1)], "in the order accepted, extra copies count no further");
    }
}
