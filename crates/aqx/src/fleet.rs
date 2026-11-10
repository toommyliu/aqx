use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aqx_client::{
    ActionError, CombatState, DeviceId, Event, Handle, ItemId, NoticeKind, Phase, Server, Spot, api,
};
use aqx_script::{Cancel, Level, Outcome, Output};
use serde::{Deserialize, Serialize};

use crate::config::{self, OnStall, Script};
use crate::describe::describe;
use crate::runner::{self, Job, Login, Note};

pub const LOG_LINES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Aqx,
    Script,
    Server,
    Chat,
    Game,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Channel::Aqx => "aqx",
            Channel::Script => "script",
            Channel::Server => "server",
            Channel::Chat => "chat",
            Channel::Game => "game",
        }
    }

    fn of(event: &Event) -> Channel {
        match event {
            Event::Phase(_)
            | Event::Ready { .. }
            | Event::Unreadable { .. }
            | Event::Packet { .. } => Channel::Aqx,
            Event::Notice { .. } => Channel::Server,
            Event::Chat { .. } | Event::Whisper { .. } => Channel::Chat,
            _ => Channel::Game,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub at_ms: u64,
    pub channel: Channel,
    pub text: String,
}

#[derive(Default)]
pub struct Log {
    lines: VecDeque<Line>,
    kept: HashMap<Channel, usize>,
    next: u64,
}

impl Log {
    pub fn push(&mut self, channel: Channel, text: String) {
        let line = Line {
            seq: self.next,
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            channel,
            text,
        };
        self.insert(line);
    }

    pub fn insert(&mut self, line: Line) {
        self.next = line.seq + 1;
        let channel = line.channel;
        self.lines.push_back(line);
        let kept = self.kept.entry(channel).or_default();
        *kept += 1;
        if *kept > LOG_LINES {
            *kept -= 1;
            if let Some(at) = self.lines.iter().position(|l| l.channel == channel) {
                self.lines.remove(at);
            }
        }
    }

    pub fn since(&self, seq: u64) -> impl Iterator<Item = &Line> {
        let skip = self.lines.partition_point(|l| l.seq < seq);
        self.lines.range(skip..)
    }

    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &Line> {
        self.lines.iter()
    }
}

pub enum Link {
    Offline {
        why: Option<String>,
    },
    Connecting,
    Waiting {
        why: String,
        until: Instant,
    },
    Session {
        session: Handle,
        server: String,
        since: Instant,
    },
}

pub enum Run {
    Idle,
    Starting { script: PathBuf },
    Running { script: PathBuf, cancel: Cancel },
    Restarting { script: PathBuf, at: Instant },
    Ended { script: PathBuf, how: Ended },
}

pub enum Ended {
    Finished,
    Stopped(Option<String>),
    Failed(String),
    Closed(String),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub kills: u64,
    pub deaths: u64,
    pub drops: u64,
    pub gold_from: Option<u64>,
}

pub struct Board {
    pub link: Link,
    pub run: Run,
    pub status: String,
    pub log: Log,
    pub stats: Stats,
    pub touched: HashMap<ItemId, Instant>,
    pub rep_from: HashMap<String, u32>,
    pub last: Option<Script>,
    runs: u64,
    links: u64,
    queued_runs: usize,
    queued_logins: usize,
    last_progress: Instant,
    stalled: bool,
    switched: Option<String>,
}

impl Default for Board {
    fn default() -> Board {
        Board {
            link: Link::Offline { why: None },
            run: Run::Idle,
            status: String::new(),
            log: Log::default(),
            stats: Stats::default(),
            touched: HashMap::new(),
            rep_from: HashMap::new(),
            last: None,
            runs: 0,
            links: 0,
            queued_runs: 0,
            queued_logins: 0,
            last_progress: Instant::now(),
            stalled: false,
            switched: None,
        }
    }
}

impl Board {
    pub fn session(&self) -> Option<&Handle> {
        match &self.link {
            Link::Session { session, .. } => Some(session),
            _ => None,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.run, Run::Running { .. })
    }

    pub fn stalled(&self) -> bool {
        self.stalled && self.running()
    }

    fn progressed(&mut self, now: Instant) {
        self.last_progress = now;
        self.stalled = false;
    }

    fn idle(&self) -> bool {
        let offline = match &self.link {
            Link::Offline { .. } => true,
            Link::Connecting | Link::Waiting { .. } => false,
            Link::Session { session, .. } => closed(session),
        };
        offline
            && self.queued_runs + self.queued_logins == 0
            && !matches!(
                self.run,
                Run::Starting { .. } | Run::Running { .. } | Run::Restarting { .. }
            )
    }

    pub fn paused(&self) -> bool {
        matches!(&self.run, Run::Running { cancel, .. } if cancel.is_paused())
    }

    fn script_output(&mut self, out: Output) {
        match out {
            Output::Log { level, text } => {
                let text = match level {
                    Level::Log => text,
                    Level::Warn => format!("warning: {text}"),
                    Level::Error => format!("error: {text}"),
                };
                self.log.push(Channel::Script, text);
            }
            Output::Status(text) if text == self.status => {}
            Output::Status(text) => {
                self.log.push(Channel::Script, format!("status: {text}"));
                self.status = text;
            }
        }
    }

    pub(crate) fn record(&mut self, event: &Event) {
        let now = Instant::now();
        match event {
            Event::Killed { .. } => {
                self.stats.kills += 1;
                self.progressed(now);
            }
            Event::Died => self.stats.deaths += 1,
            Event::DropOffered { id, .. } => {
                self.stats.drops += 1;
                self.touched.insert(*id, now);
            }
            Event::ItemChanged { id, .. } | Event::ItemReceived { id, .. } => {
                self.touched.insert(*id, now);
                self.progressed(now);
            }
            Event::QuestCompleted { .. } | Event::Joined { .. } => self.progressed(now),
            Event::Ready { gold, .. } => {
                self.stats.gold_from.get_or_insert(*gold);
            }
            _ => {}
        }
        let text = match event {
            Event::Notice {
                kind: NoticeKind::Server,
                text,
            } => Some(text.clone()),
            _ => describe(event),
        };
        if let Some(text) = text {
            self.log.push(Channel::of(event), text);
        }
    }
}

enum Command {
    Login { link: u64 },
    Run { script: Script, run: u64 },
    Logout,
}

pub struct Account {
    pub name: String,
    pub user: String,
    pub server: String,
    script: Option<Script>,
    restart_on_error: bool,
    room: Option<u32>,
    stall_after: Duration,
    on_stall: OnStall,
    commanded: AtomicBool,
    seen: Mutex<config::Account>,
    board: Mutex<Board>,
    commands: Sender<Command>,
}

impl Account {
    pub fn board(&self) -> MutexGuard<'_, Board> {
        self.board.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn say(&self, text: impl Into<String>) {
        self.board().log.push(Channel::Aqx, text.into());
    }

    pub fn script(&self) -> Option<&Script> {
        self.script.as_ref()
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn commanded(&self) -> bool {
        self.commanded.load(Ordering::SeqCst)
    }

    pub fn idle(&self) -> bool {
        self.board().idle()
    }

    fn server_for(&self, board: &Board) -> String {
        board
            .switched
            .clone()
            .unwrap_or_else(|| self.server.clone())
    }

    pub fn switch(&self, server: String) -> Result<(), &'static str> {
        {
            let mut board = self.board();
            if let Link::Session {
                session,
                server: on,
                ..
            } = &board.link
                && on.eq_ignore_ascii_case(&server)
                && !closed(session)
            {
                return Err("already on that server");
            }
            board.switched = Some(server);
        }
        self.relogin();
        Ok(())
    }

    pub fn relogin(&self) {
        self.commanded.store(true, Ordering::SeqCst);
        let mut board = self.board();
        let running = matches!(
            board.run,
            Run::Starting { .. } | Run::Running { .. } | Run::Restarting { .. }
        );
        let script =
            if running { board.last.clone() } else { None }.or_else(|| self.script.clone());
        board.runs += 1;
        if let Run::Running { cancel, .. } = &board.run {
            cancel.cancel();
        }
        board
            .log
            .push(Channel::Aqx, "logging out to log in again".into());
        self.send(Command::Logout);
        board.queued_logins += 1;
        self.send(Command::Login { link: board.links });
        if let Some(script) = script {
            board.queued_runs += 1;
            self.send(Command::Run {
                script,
                run: board.runs,
            });
        }
    }

    pub fn login(&self) -> Result<(), &'static str> {
        self.commanded.store(true, Ordering::SeqCst);
        let mut board = self.board();
        if board.queued_logins > 0 || matches!(board.link, Link::Connecting | Link::Waiting { .. })
        {
            return Err("already logging in");
        }
        if board.session().is_some_and(|s| !closed(s)) {
            return Err("already online");
        }
        board.queued_logins += 1;
        self.send(Command::Login { link: board.links });
        if let Some(script) = self.script.clone() {
            board.queued_runs += 1;
            self.send(Command::Run {
                script,
                run: board.runs,
            });
        }
        Ok(())
    }

    pub fn run(&self, script: Script) {
        self.commanded.store(true, Ordering::SeqCst);
        let mut board = self.board();
        board.runs += 1;
        if let Run::Running { cancel, .. } = &board.run {
            cancel.cancel();
        }
        board.queued_runs += 1;
        self.send(Command::Run {
            script,
            run: board.runs,
        });
    }

    pub fn restart(&self) -> Result<(), &'static str> {
        let script = self.board().last.clone().or_else(|| self.script.clone());
        let script = script.ok_or("no script to restart")?;
        self.run(script);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), &'static str> {
        let mut board = self.board();
        board.runs += 1;
        let pending = board.queued_runs > 0;
        match &board.run {
            Run::Running { cancel, .. } => {
                cancel.cancel();
                Ok(())
            }
            Run::Starting { .. } => Ok(()),
            Run::Restarting { script, .. } => {
                board.run = Run::Ended {
                    script: script.clone(),
                    how: Ended::Stopped(None),
                };
                Ok(())
            }
            _ if pending => Ok(()),
            _ => Err("no script is running"),
        }
    }

    pub fn pause(&self) -> Result<(), &'static str> {
        match &self.board().run {
            Run::Running { script, .. } if is_skua(script) => Err("Skua scripts can't pause"),
            Run::Running { cancel, .. } if cancel.is_paused() => Err("already paused"),
            Run::Running { cancel, .. } => {
                cancel.pause();
                Ok(())
            }
            _ => Err("no script is running"),
        }
    }

    pub fn resume(&self) -> Result<(), &'static str> {
        let mut board = self.board();
        match &board.run {
            Run::Running { cancel, .. } if cancel.is_paused() => cancel.resume(),
            _ => return Err("not paused"),
        }
        board.progressed(Instant::now());
        Ok(())
    }

    pub fn logout(&self) -> Result<(), &'static str> {
        let mut board = self.board();
        board.switched = None;
        board.runs += 1;
        board.links += 1;
        let pending = board.queued_runs + board.queued_logins > 0;
        let busy = matches!(board.run, Run::Starting { .. } | Run::Restarting { .. });
        if let Run::Restarting { script, .. } = &board.run {
            board.run = Run::Ended {
                script: script.clone(),
                how: Ended::Stopped(None),
            };
        }
        match (&board.run, &board.link) {
            (Run::Running { cancel, .. }, _) => cancel.cancel(),
            (_, Link::Session { session, .. }) if !closed(session) => session.logout(),
            (_, Link::Connecting | Link::Waiting { .. }) => {}
            _ if pending || busy => {}
            _ => return Err("already offline"),
        }
        self.send(Command::Logout);
        Ok(())
    }

    pub fn pick_up(self: &Arc<Self>, items: Vec<ItemId>) -> Result<(), &'static str> {
        let session = self.board().session().cloned().ok_or("offline")?;
        let account = Arc::clone(self);
        thread::spawn(move || {
            for item in items {
                if let Err(e) = session.pick_drop(item).wait() {
                    account.say(format!("couldn't pick up a drop: {e}"));
                }
            }
        });
        Ok(())
    }
}

fn is_skua(script: &Path) -> bool {
    script
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cs"))
}

fn leave_combat(session: &Handle) {
    let world = session.snapshot();
    let fighting = matches!(world.phase, Phase::Ready)
        && world
            .me
            .as_ref()
            .is_some_and(|me| me.state == CombatState::InCombat);
    if fighting {
        let _ = session.exit_combat().wait();
    }
}

fn locator(fleet: Accounts, server: String) -> impl Fn(&str) -> Option<Spot> + Send {
    move |name| {
        let accounts = fleet.read().unwrap_or_else(PoisonError::into_inner).clone();
        accounts.iter().find_map(|account| {
            let session = match &account.board().link {
                Link::Session {
                    session,
                    server: on,
                    ..
                } if on.eq_ignore_ascii_case(&server) => session.clone(),
                _ => return None,
            };
            let world = session.snapshot();
            let (Phase::Ready, Some(me), Some(room)) = (&world.phase, &world.me, &world.room)
            else {
                return None;
            };
            me.name.eq_ignore_ascii_case(name).then(|| Spot {
                map: room.name.clone(),
                cell: room.cell.clone(),
                pad: room.pad.clone(),
            })
        })
    }
}

fn closed(session: &Handle) -> bool {
    matches!(session.snapshot().phase, Phase::Closed { .. })
}

const WATCH_EVERY: Duration = Duration::from_secs(2);
const LOGIN_RETRY_MAX: Duration = Duration::from_secs(300);

enum Failure {
    Retry(String),
    Stop(String),
}

impl Failure {
    fn into_reason(self) -> String {
        match self {
            Failure::Retry(why) | Failure::Stop(why) => why,
        }
    }
}
const LAST_LINES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastLines {
    pub name: String,
    pub user: String,
    pub lines: Vec<Line>,
}

type Accounts = Arc<RwLock<Vec<Arc<Account>>>>;

const LOGIN_GAP: Duration = Duration::from_secs(2);
const LOGIN_JITTER: Duration = Duration::from_secs(1);

struct Spacing {
    gap: Duration,
    jitter: Duration,
    next: Mutex<Option<Instant>>,
}

impl Spacing {
    fn new(gap: Duration, jitter: Duration) -> Spacing {
        Spacing {
            gap,
            jitter,
            next: Mutex::new(None),
        }
    }

    fn reserve(&self) -> Instant {
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let start = next.map_or(now, |at| at.max(now));
        let fraction = RandomState::new().hash_one(()) as f64 / u64::MAX as f64;
        *next = Some(start + self.gap + self.jitter.mul_f64(fraction));
        start
    }

    fn wait(&self) {
        thread::sleep(self.reserve().saturating_duration_since(Instant::now()));
    }
}

pub struct Fleet {
    device: DeviceId,
    accounts: Accounts,
    logins: Arc<Spacing>,
}

impl Fleet {
    pub fn new(device: DeviceId) -> Fleet {
        Fleet {
            device,
            accounts: Accounts::default(),
            logins: Arc::new(Spacing::new(LOGIN_GAP, LOGIN_JITTER)),
        }
    }

    pub fn accounts(&self) -> Vec<Arc<Account>> {
        self.accounts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn add_new(&self, configs: Vec<config::Account>) -> Vec<String> {
        let mut accounts = self
            .accounts
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut added = Vec::new();
        for config in configs {
            if accounts
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case(&config.name))
            {
                continue;
            }
            added.push(config.name.clone());
            accounts.push(self.spawn(config));
        }
        added
    }

    fn spawn(&self, config: config::Account) -> Arc<Account> {
        let seen = Mutex::new(config.clone());
        let (commands, inbox) = mpsc::channel();
        let account = Arc::new(Account {
            name: config.name,
            user: config.user.clone(),
            server: config.server,
            script: config.script,
            restart_on_error: config.restart_on_error,
            room: config.room,
            stall_after: config.stall_after,
            on_stall: config.on_stall,
            commanded: AtomicBool::new(false),
            seen,
            board: Mutex::new(Board::default()),
            commands,
        });
        let linked = Arc::new(Linked {
            account: Arc::clone(&account),
            fleet: Arc::clone(&self.accounts),
            logins: Arc::clone(&self.logins),
            login: Login {
                device: self.device.clone(),
                user: config.user,
                pass: config.password,
                client_version: config.client_version,
                relogin: config.relogin,
                api: None,
                record: None,
            },
        });
        thread::Builder::new()
            .name(format!("aqx-account-{}", account.name))
            .spawn(move || linked.work(&inbox))
            .expect("an account thread starts");
        account
    }

    pub fn watch(self: &Arc<Self>, path: PathBuf) {
        let fleet = Arc::downgrade(self);
        let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut seen = modified(&path);
        thread::spawn(move || {
            loop {
                thread::sleep(WATCH_EVERY);
                let Some(fleet) = fleet.upgrade() else {
                    return;
                };
                let now = modified(&path);
                if now == seen {
                    continue;
                }
                seen = now;
                if let Ok(accounts) = config::load_quietly(&path) {
                    fleet.refresh(accounts, &path);
                }
            }
        });
    }

    pub fn refresh(&self, configs: Vec<config::Account>, path: &Path) {
        let running = self.accounts();
        let mut fresh = Vec::new();
        for config in configs {
            let Some(account) = running
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(&config.name))
            else {
                fresh.push(config);
                continue;
            };
            let mut seen = account.seen.lock().unwrap_or_else(PoisonError::into_inner);
            if *seen != config {
                *seen = config;
                drop(seen);
                account.say(format!(
                    "{} changed this account; the change applies once the session restarts (aqx down, then aqx)",
                    path.display()
                ));
            }
        }
        for name in self.add_new(fresh) {
            if let Some(account) = self.accounts().iter().find(|a| a.name == name) {
                account.say(format!("added from {}", path.display()));
            }
        }
    }

    pub fn last_lines(&self) -> Vec<LastLines> {
        self.accounts()
            .iter()
            .map(|account| {
                let board = account.board();
                let kept = board
                    .log
                    .lines()
                    .rev()
                    .filter(|l| !matches!(l.channel, Channel::Chat | Channel::Game))
                    .take(LAST_LINES);
                let mut lines: Vec<Line> = kept.cloned().collect();
                lines.reverse();
                LastLines {
                    name: account.name.clone(),
                    user: account.user.clone(),
                    lines,
                }
            })
            .collect()
    }

    pub fn idle(&self) -> bool {
        self.accounts().iter().all(|a| a.idle())
    }

    pub fn commanded(&self) -> bool {
        self.accounts().iter().any(|a| a.commanded())
    }

    pub fn logout_all(&self) {
        for account in self.accounts() {
            let _ = account.logout();
        }
    }

    pub fn resumable(&self) -> Vec<(String, Option<Script>)> {
        self.accounts()
            .iter()
            .filter(|a| !a.idle())
            .map(|a| {
                let board = a.board();
                let running = matches!(
                    board.run,
                    Run::Starting { .. } | Run::Running { .. } | Run::Restarting { .. }
                );
                (
                    a.name.clone(),
                    if running { board.last.clone() } else { None },
                )
            })
            .collect()
    }

    pub fn supervise(&self, now: Instant) {
        for account in self.accounts() {
            let stalled = {
                let mut board = account.board();
                let due = matches!(&board.run, Run::Running { cancel, .. } if !cancel.is_paused())
                    && !board.stalled
                    && now.saturating_duration_since(board.last_progress) >= account.stall_after;
                if due {
                    board.stalled = true;
                    let quiet = account.stall_after.as_secs();
                    board.log.push(
                        Channel::Aqx,
                        format!("warning: stalled, no progress for {quiet} s"),
                    );
                }
                due
            };
            if stalled {
                match account.on_stall {
                    OnStall::Warn => {}
                    OnStall::Restart => {
                        let _ = account.restart();
                    }
                    OnStall::Relogin => account.relogin(),
                }
            }
        }
    }
}

const RESTART_FIRST: Duration = Duration::from_secs(10);
const RESTART_MAX: Duration = Duration::from_secs(300);

struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff {
            next: RESTART_FIRST,
        }
    }
}

impl Backoff {
    fn after(&mut self, lasted: Duration) -> Duration {
        if lasted >= RESTART_MAX {
            self.next = RESTART_FIRST;
        }
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(RESTART_MAX);
        delay
    }
}

struct Linked {
    account: Arc<Account>,
    fleet: Accounts,
    logins: Arc<Spacing>,
    login: Login,
}

impl Linked {
    fn work(self: Arc<Self>, inbox: &Receiver<Command>) {
        let mut restarts = Backoff::default();
        let mut pending = VecDeque::new();
        loop {
            let Some(command) = pending.pop_front().or_else(|| inbox.recv().ok()) else {
                return;
            };
            match command {
                Command::Login { link } => {
                    self.ensure_online(link, true);
                }
                Command::Run { script, run } => {
                    let Some(link) = self.take_run(&script, run) else {
                        continue;
                    };
                    let Some((session, server)) = self.ensure_online(link, false) else {
                        self.unstart();
                        continue;
                    };
                    let started = Instant::now();
                    let failed = self.run(script.clone(), run, session, server);
                    if !(failed && self.account.restart_on_error) {
                        continue;
                    }
                    let delay = restarts.after(started.elapsed());
                    if !self.restarting(&script, run, delay) {
                        continue;
                    }
                    match inbox.recv_timeout(delay) {
                        Ok(Command::Login { link }) => {
                            self.account.board().queued_runs += 1;
                            pending.extend([Command::Login { link }, Command::Run { script, run }]);
                        }
                        Ok(command) => {
                            self.abandon_restart();
                            pending.push_back(command);
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            self.account.board().queued_runs += 1;
                            pending.push_back(Command::Run { script, run });
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                Command::Logout => self.logout(),
            }
        }
    }

    fn take_run(&self, script: &Script, run: u64) -> Option<u64> {
        let mut board = self.account.board();
        board.queued_runs -= 1;
        if board.runs != run {
            return None;
        }
        board.run = Run::Starting {
            script: script.path.clone(),
        };
        Some(board.links)
    }

    fn unstart(&self) {
        let mut board = self.account.board();
        if matches!(board.run, Run::Starting { .. }) {
            board.run = Run::Idle;
        }
    }

    fn restarting(&self, script: &Script, run: u64, delay: Duration) -> bool {
        let mut board = self.account.board();
        if board.runs != run {
            return false;
        }
        board.run = Run::Restarting {
            script: script.path.clone(),
            at: Instant::now() + delay,
        };
        board.log.push(
            Channel::Aqx,
            format!("restarting the script in {} s", delay.as_secs()),
        );
        true
    }

    fn abandon_restart(&self) {
        let mut board = self.account.board();
        if let Run::Restarting { script, .. } = &board.run {
            board.run = Run::Ended {
                script: script.clone(),
                how: Ended::Stopped(None),
            };
        }
    }

    fn ensure_online(&self, link: u64, login: bool) -> Option<(Handle, String)> {
        let server = {
            let mut board = self.account.board();
            if login {
                board.queued_logins -= 1;
            }
            if let Link::Session {
                session, server, ..
            } = &board.link
                && !closed(session)
            {
                return Some((session.clone(), server.clone()));
            }
            if board.links != link {
                return None;
            }
            board.link = Link::Connecting;
            board.stats = Stats::default();
            board.rep_from.clear();
            self.account.server_for(&board)
        };
        self.account.say(format!("logging in to {server}"));
        let (attempts, mut delay) = match self.login.relogin {
            Some(relogin) => (relogin.attempts.unwrap_or(u32::MAX).max(1), relogin.delay),
            None => (1, Duration::ZERO),
        };
        for attempt in 1..=attempts {
            if !self.hold(link, self.logins.reserve()) {
                return None;
            }
            let why = match self.connect(&server, &self.login.user, &self.login.pass) {
                Ok(session) => {
                    let world = session.snapshot();
                    self.account.board().rep_from = world
                        .factions
                        .iter()
                        .map(|f| (f.name.clone(), f.rep))
                        .collect();
                    return Some((session, server));
                }
                Err(Failure::Retry(why)) if attempt < attempts => why,
                Err(Failure::Retry(why) | Failure::Stop(why)) => return self.failed(link, why),
            };
            let until = Instant::now() + delay;
            {
                let mut board = self.account.board();
                if board.links != link {
                    board.link = Link::Offline { why: None };
                    return None;
                }
                board.log.push(
                    Channel::Aqx,
                    format!(
                        "couldn't log in: {why}; trying again in {} s (attempt {} of {attempts})",
                        delay.as_secs(),
                        attempt + 1
                    ),
                );
                board.link = Link::Waiting { why, until };
            }
            if !self.hold(link, until) {
                return None;
            }
            self.account.board().link = Link::Connecting;
            delay = delay.saturating_mul(2).min(LOGIN_RETRY_MAX);
        }
        None
    }

    fn hold(&self, link: u64, until: Instant) -> bool {
        loop {
            if self.account.board().links != link {
                self.account.board().link = Link::Offline { why: None };
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            thread::sleep(Duration::from_millis(100).min(until - now));
        }
    }

    fn failed(&self, link: u64, why: String) -> Option<(Handle, String)> {
        let mut board = self.account.board();
        let cancelled = board.links != link;
        board.link = Link::Offline {
            why: (!cancelled).then(|| why.clone()),
        };
        board.runs += 1;
        if !cancelled {
            board
                .log
                .push(Channel::Aqx, format!("couldn't log in: {why}"));
        }
        None
    }

    fn connect(&self, server: &str, user: &str, pass: &str) -> Result<Handle, Failure> {
        let (session, events) = self.login.spawn(server, user, pass).map_err(|e| {
            let why = format!("{e:#}");
            match e.downcast_ref::<api::ApiError>() {
                Some(api::ApiError::Rejected(_)) => Failure::Stop(why),
                _ => Failure::Retry(why),
            }
        })?;
        self.attach(&session, server);
        if let Some(room) = self.account.room {
            session.set_room(Some(room));
        }
        session.set_locate(locator(Arc::clone(&self.fleet), server.to_owned()));
        let account = Arc::clone(&self.account);
        thread::spawn(move || {
            for event in events {
                account.board().record(&event);
            }
        });
        session.ready().wait().map_err(|e| match &e {
            ActionError::Closed(reason) if !reason.retryable() => Failure::Stop(e.to_string()),
            _ => Failure::Retry(e.to_string()),
        })?;
        Ok(session)
    }

    fn attach(&self, session: &Handle, server: &str) {
        let mut board = self.account.board();
        if board.session().is_some_and(|s| s.same_session(session)) {
            return;
        }
        board.link = Link::Session {
            session: session.clone(),
            server: server.to_owned(),
            since: Instant::now(),
        };
    }

    fn run(self: &Arc<Self>, script: Script, run: u64, session: Handle, server: String) -> bool {
        let cancel = Cancel::default();
        let shown = script.path.display().to_string();
        {
            let mut board = self.account.board();
            if board.runs != run {
                if matches!(board.run, Run::Starting { .. }) {
                    board.run = Run::Ended {
                        script: script.path.clone(),
                        how: Ended::Stopped(None),
                    };
                }
                return false;
            }
            board.last = Some(script.clone());
            board.status.clear();
            board.progressed(Instant::now());
            board.run = Run::Running {
                script: script.path.clone(),
                cancel: cancel.clone(),
            };
        }
        self.account.say(format!("running {shown}"));
        let output = {
            let account = Arc::clone(&self.account);
            move |out: Output| account.board().script_output(out)
        };
        let job = Job::new(&script.path, &cancel, output);
        let loaded = job
            .load()
            .map_err(|e| (e.to_string(), true))
            .and_then(|loaded| {
                let args = loaded.parse_args(&script.args).map_err(|e| (e, false))?;
                Ok((loaded, args))
            });
        let (how, restartable) =
            match loaded {
                Err((why, restartable)) => (Ended::Failed(why), restartable),
                Ok((loaded, args)) => {
                    let relogin: Arc<dyn aqx_script::Account> = Arc::clone(self) as _;
                    let ran =
                        runner::drive(&job, loaded, &args, &relogin, session, server, |note| {
                            match note {
                                Note::Session(session) => {
                                    let server = {
                                        let board = self.account.board();
                                        match &board.link {
                                            Link::Session { server, .. } => server.clone(),
                                            _ => self.account.server_for(&board),
                                        }
                                    };
                                    self.attach(session, &server);
                                }
                                Note::Restarting => self.account.say("restarting the script"),
                                Note::Reconnecting(_) => self
                                    .account
                                    .say("the script starts again once the account is back"),
                            }
                        });
                    self.attach(&ran.session, &ran.server);
                    if matches!(ran.outcome, Outcome::LoggedOut) {
                        self.account.board().switched = None;
                    }
                    let how = match ran.outcome {
                        Outcome::Finished => Ended::Finished,
                        Outcome::Stopped(reason) => Ended::Stopped(reason),
                        Outcome::Cancelled => Ended::Stopped(None),
                        Outcome::LoggedOut => Ended::Stopped(Some("the script logged out".into())),
                        Outcome::Disconnected(why) => Ended::Closed(why),
                        Outcome::Failed(why) => Ended::Failed(why),
                        Outcome::Restart => unreachable!("handled by the runner"),
                    };
                    leave_combat(&ran.session);
                    let restartable = matches!(how, Ended::Failed(_));
                    (how, restartable)
                }
            };
        let (how, restartable) = match how {
            Ended::Failed(_) if cancel.is_cancelled() => (Ended::Stopped(None), false),
            how => (how, restartable),
        };
        match &how {
            Ended::Finished => self.account.say("the script finished"),
            Ended::Stopped(None) => self.account.say("the script stopped"),
            Ended::Stopped(Some(reason)) => {
                self.account.say(format!("the script stopped: {reason}"))
            }
            Ended::Failed(why) => self
                .account
                .board()
                .log
                .push(Channel::Script, format!("error: {why}")),
            Ended::Closed(why) => self.account.say(format!("the session closed: {why}")),
        }
        self.account.board().run = Run::Ended {
            script: script.path,
            how,
        };
        restartable
    }

    fn logout(&self) {
        let Some(session) = self.account.board().session().cloned() else {
            return;
        };
        if closed(&session) {
            return;
        }
        let closing = session.subscribe();
        session.logout();
        for event in closing {
            if matches!(event, Event::Phase(Phase::Closed { .. })) {
                break;
            }
        }
    }
}

impl aqx_script::Account for Linked {
    fn servers(&self) -> Result<Vec<Server>, String> {
        match &self.login.api {
            Some(host) => api::servers_on(host, &self.login.device),
            None => api::servers(&self.login.device),
        }
        .map_err(|e| e.to_string())
    }

    fn login(&self, server: &str) -> Result<Handle, String> {
        self.logins.wait();
        self.connect(server, &self.login.user, &self.login.pass)
            .map_err(Failure::into_reason)
    }

    fn login_as(&self, server: &str, username: &str, password: &str) -> Result<Handle, String> {
        self.logins.wait();
        self.connect(server, username, password)
            .map_err(Failure::into_reason)
    }
}

#[cfg(test)]
impl Fleet {
    pub(crate) fn of(accounts: Vec<Arc<Account>>) -> Fleet {
        Fleet {
            device: aqx_client::testing::device_id(),
            accounts: Arc::new(RwLock::new(accounts)),
            logins: Arc::new(Spacing::new(Duration::ZERO, Duration::ZERO)),
        }
    }
}

#[cfg(test)]
impl Account {
    pub(crate) fn detached(name: &str) -> Account {
        Account::queueing(name, None).0
    }

    pub(crate) fn detached_with(name: &str, script: Script) -> Account {
        Account::queueing(name, Some(script)).0
    }

    fn queueing(name: &str, script: Option<Script>) -> (Account, Receiver<Command>) {
        let (commands, inbox) = mpsc::channel();
        let account = Account {
            name: name.into(),
            user: name.into(),
            server: "Test".into(),
            script,
            restart_on_error: false,
            room: None,
            stall_after: Duration::from_secs(300),
            on_stall: OnStall::Warn,
            commanded: AtomicBool::new(false),
            seen: Mutex::new(config::Account::for_tests(name)),
            board: Mutex::new(Board::default()),
            commands,
        };
        (account, inbox)
    }
}

#[cfg(test)]
mod tests {
    use aqx_client::{MonMapId, NoticeKind};

    use super::*;

    #[test]
    fn accounts_appended_to_the_config_join_once_by_name() {
        let config = |names: &[&str]| -> Vec<config::Account> {
            names
                .iter()
                .map(|name| config::Account::for_tests(name))
                .collect()
        };
        let fleet = Fleet::of(Vec::new());
        assert_eq!(fleet.add_new(config(&["player1"])), ["player1"]);
        assert_eq!(
            fleet.add_new(config(&["Player1", "player2"])),
            ["player2"],
            "a name already running is skipped"
        );
        let names: Vec<String> = fleet.accounts().iter().map(|a| a.name.clone()).collect();
        assert_eq!(
            names,
            ["player1", "player2"],
            "new accounts go last, so numbers stay put"
        );
    }

    fn farm() -> Script {
        Script {
            path: PathBuf::from("/srv/farm.ts"),
            args: Vec::new(),
        }
    }

    fn linked(account: Account) -> Arc<Linked> {
        Arc::new(Linked {
            account: Arc::new(account),
            fleet: Accounts::default(),
            logins: Arc::new(Spacing::new(Duration::ZERO, Duration::ZERO)),
            login: Login {
                device: aqx_client::testing::device_id(),
                user: "player1".into(),
                pass: "x".into(),
                client_version: None,
                relogin: None,
                api: None,
                record: None,
            },
        })
    }

    fn take(linked: &Linked, inbox: &Receiver<Command>) -> Option<PathBuf> {
        match inbox.try_recv().expect("a queued command") {
            Command::Login { link } => {
                let mut board = linked.account.board();
                board.queued_logins -= 1;
                (board.links == link).then(|| PathBuf::from("login"))
            }
            Command::Run { script, run } => linked.take_run(&script, run).map(|_| script.path),
            Command::Logout => Some(PathBuf::from("logout")),
        }
    }

    #[test]
    fn a_running_script_without_progress_stalls_once_until_it_progresses() {
        let (account, inbox) = Account::queueing("player1", Some(farm()));
        let account = Arc::new(Account {
            on_stall: OnStall::Restart,
            ..account
        });
        let fleet = Fleet::of(vec![Arc::clone(&account)]);
        let now = Instant::now();
        {
            let mut board = account.board();
            board.run = Run::Running {
                script: PathBuf::from("/srv/farm.ts"),
                cancel: Cancel::default(),
            };
            board.last = Some(farm());
            board.last_progress = now - Duration::from_secs(299);
        }
        fleet.supervise(now);
        assert!(
            !account.board().stalled(),
            "under stall_after it isn't stalled"
        );
        fleet.supervise(now + Duration::from_secs(2));
        assert!(account.board().stalled());
        fleet.supervise(now + Duration::from_secs(3));
        let warned = account
            .board()
            .log
            .lines()
            .filter(|l| l.text.starts_with("warning: stalled"))
            .count();
        assert_eq!(warned, 1, "a stall warns once");
        assert!(
            matches!(inbox.try_recv(), Ok(Command::Run { .. })),
            "on_stall = restart reruns the script"
        );
        account.board().record(&Event::Killed {
            id: aqx_client::MonMapId(1),
            name: "Frogzard".into(),
            gold: 0,
            exp: 0,
        });
        assert!(!account.board().stalled(), "progress clears the stall");
    }

    #[test]
    fn relogin_logs_out_then_back_in_and_reruns_the_script() {
        let (account, inbox) = Account::queueing("player1", Some(farm()));
        let fleet = Fleet::of(Vec::new());
        assert!(fleet.idle() && !fleet.commanded());
        account.relogin();
        assert!(
            account.commanded() && !account.idle(),
            "a queued login keeps the session going"
        );
        let linked = linked(account);
        let taken: Vec<Option<PathBuf>> = (0..3).map(|_| take(&linked, &inbox)).collect();
        assert_eq!(
            taken,
            [
                Some("logout".into()),
                Some("login".into()),
                Some("/srv/farm.ts".into())
            ]
        );
    }

    #[test]
    fn a_switched_server_holds_until_a_logout() {
        let (account, inbox) = Account::queueing("player1", Some(farm()));
        assert_eq!(account.switch("Twilly".into()), Ok(()));
        let linked = linked(account);
        let taken: Vec<Option<PathBuf>> = (0..3).map(|_| take(&linked, &inbox)).collect();
        assert_eq!(
            taken,
            [
                Some("logout".into()),
                Some("login".into()),
                Some("/srv/farm.ts".into())
            ],
            "a switch logs out, back in, and reruns the script"
        );
        let account = &linked.account;
        let login_server = || account.server_for(&account.board());
        assert_eq!(login_server(), "Twilly");
        account.relogin();
        assert_eq!(login_server(), "Twilly", "a relogin keeps it");
        let _ = account.logout();
        assert_eq!(login_server(), "Test");
    }

    #[test]
    fn stop_drops_queued_runs_but_keeps_the_login() {
        let (account, inbox) = Account::queueing("player1", Some(farm()));
        assert_eq!(account.login(), Ok(()));
        assert_eq!(
            account.login(),
            Err("already logging in"),
            "a second press doesn't queue the script twice"
        );
        assert_eq!(
            account.stop(),
            Ok(()),
            "stopping a queued run did something"
        );
        let linked = linked(account);
        assert_eq!(
            take(&linked, &inbox),
            Some(PathBuf::from("login")),
            "the login still happens"
        );
        assert_eq!(take(&linked, &inbox), None, "the configured script doesn't");
        assert_eq!(linked.account.stop(), Err("no script is running"));
    }

    #[test]
    fn a_new_run_supersedes_one_still_queued() {
        let (account, inbox) = Account::queueing("player1", Some(farm()));
        account.login().unwrap();
        let other = Script {
            path: PathBuf::from("/srv/other.ts"),
            args: Vec::new(),
        };
        account.run(other.clone());
        account.run(other);
        let linked = linked(account);
        let taken: Vec<Option<PathBuf>> = (0..4).map(|_| take(&linked, &inbox)).collect();
        assert_eq!(
            taken,
            [
                Some("login".into()),
                None,
                None,
                Some("/srv/other.ts".into())
            ],
            "only the newest run starts, once"
        );
        assert!(matches!(linked.account.board().run, Run::Starting { .. }));
    }

    #[test]
    fn logout_cancels_a_queued_login_and_says_so() {
        let (account, inbox) = Account::queueing("player1", None);
        assert_eq!(account.logout(), Err("already offline"));
        let _ = inbox.try_recv();
        account.login().unwrap();
        assert_eq!(account.logout(), Ok(()));
        let linked = linked(account);
        assert_eq!(take(&linked, &inbox), None, "the login is dropped");
        assert_eq!(take(&linked, &inbox), Some(PathBuf::from("logout")));
    }

    #[test]
    fn a_logout_while_waiting_for_a_login_slot_cancels_the_login() {
        for (slot, logout_after) in [
            (Duration::from_secs(5), Duration::from_millis(100)),
            (Duration::from_millis(150), Duration::from_millis(120)),
        ] {
            logout_while_waiting(slot, logout_after);
        }
    }

    fn logout_while_waiting(slot: Duration, logout_after: Duration) {
        let (account, _inbox) = Account::queueing("player1", None);
        account.login().unwrap();
        let link = account.board().links;
        let logins = Arc::new(Spacing::new(slot, Duration::ZERO));
        logins.reserve();
        let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", api.local_addr().unwrap());
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let requests = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in api.incoming() {
                    requests.fetch_add(1, Ordering::SeqCst);
                    drop(stream);
                }
            });
        }
        let linked = Arc::new(Linked {
            account: Arc::new(account),
            fleet: Accounts::default(),
            logins,
            login: Login {
                device: aqx_client::testing::device_id(),
                user: "player1".into(),
                pass: "x".into(),
                client_version: None,
                relogin: None,
                api: Some(host),
                record: None,
            },
        });
        let waiting = {
            let linked = Arc::clone(&linked);
            thread::spawn(move || linked.ensure_online(link, true).is_none())
        };
        thread::sleep(logout_after);
        let asked = Instant::now();
        assert_eq!(linked.account.logout(), Ok(()));
        assert!(
            waiting.join().unwrap(),
            "the login is cancelled ({slot:?} slot)"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "no login request goes out ({slot:?} slot)"
        );
        assert!(
            asked.elapsed() < Duration::from_secs(1),
            "{:?}",
            asked.elapsed()
        );
        assert!(matches!(
            linked.account.board().link,
            Link::Offline { why: None }
        ));
    }

    #[test]
    fn stopping_during_a_restart_wait_ends_it() {
        let (account, _inbox) = Account::queueing("player1", None);
        account.board().run = Run::Restarting {
            script: PathBuf::from("/srv/farm.ts"),
            at: Instant::now() + Duration::from_secs(10),
        };
        assert_eq!(account.stop(), Ok(()));
        assert!(matches!(
            account.board().run,
            Run::Ended {
                how: Ended::Stopped(None),
                ..
            }
        ));
    }

    #[test]
    fn only_a_running_typescript_script_pauses() {
        let account = Account::detached("player1");
        assert_eq!(account.pause(), Err("no script is running"));
        let cancel = Cancel::default();
        account.board().run = Run::Running {
            script: PathBuf::from("/srv/farm.ts"),
            cancel: cancel.clone(),
        };
        assert_eq!(account.pause(), Ok(()));
        assert!(cancel.is_paused() && account.board().paused());
        assert_eq!(account.pause(), Err("already paused"));
        assert_eq!(account.resume(), Ok(()));
        assert_eq!(account.resume(), Err("not paused"));
        account.board().run = Run::Running {
            script: PathBuf::from("/srv/Farm.CS"),
            cancel: Cancel::default(),
        };
        assert_eq!(account.pause(), Err("Skua scripts can't pause"));
    }

    #[test]
    fn a_repeated_status_stays_out_of_the_log() {
        let mut board = Board::default();
        for text in [
            "going to player2",
            "going to player2",
            "with player2 in r3",
            "going to player2",
        ] {
            board.script_output(Output::Status(text.into()));
        }
        board.script_output(Output::Log {
            level: Level::Warn,
            text: "low hp".into(),
        });
        let logged: Vec<&str> = board.log.lines().map(|l| l.text.as_str()).collect();
        assert_eq!(
            logged,
            [
                "status: going to player2",
                "status: with player2 in r3",
                "status: going to player2",
                "warning: low hp"
            ]
        );
        assert_eq!(board.status, "going to player2");
    }

    #[test]
    fn a_config_change_to_a_running_account_is_noted_once() {
        let fleet = Fleet::of(vec![Arc::new(Account::detached("player1"))]);
        let path = Path::new("/srv/aqx.toml");
        let unchanged = config::Account::for_tests("player1");
        fleet.refresh(vec![unchanged.clone()], path);
        let noted = |fleet: &Fleet| {
            fleet.accounts()[0]
                .board()
                .log
                .lines()
                .filter(|l| l.text.contains("applies once the session restarts"))
                .count()
        };
        assert_eq!(noted(&fleet), 0, "an unchanged account says nothing");
        let changed = config::Account {
            auto_start: true,
            ..unchanged
        };
        fleet.refresh(vec![changed.clone()], path);
        fleet.refresh(vec![changed, config::Account::for_tests("player2")], path);
        assert_eq!(noted(&fleet), 1, "a change is noted once");
        assert_eq!(fleet.accounts().len(), 2, "a new name still joins");
    }

    #[test]
    fn logins_started_together_go_out_a_gap_apart() {
        let gap = Duration::from_millis(60);
        let logins = Arc::new(Spacing::new(gap, Duration::from_millis(20)));
        let begun = Instant::now();
        let mut started: Vec<Duration> = (0..3)
            .map(|_| {
                let logins = Arc::clone(&logins);
                thread::spawn(move || {
                    logins.wait();
                    begun.elapsed()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        started.sort();
        assert!(
            started[0] < gap,
            "the first login waits for nothing: {started:?}"
        );
        assert!(started[1] >= gap && started[2] >= gap * 2, "{started:?}");
    }

    #[test]
    fn restarts_back_off_and_reset_after_a_long_run() {
        let mut backoff = Backoff::default();
        let quick = Duration::from_secs(1);
        let waits: Vec<u64> = (0..7).map(|_| backoff.after(quick).as_secs()).collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 300, 300]);
        assert_eq!(
            backoff.after(RESTART_MAX).as_secs(),
            10,
            "a run that lasted starts over"
        );
        assert_eq!(backoff.after(quick).as_secs(), 20);
    }

    #[test]
    fn each_channel_keeps_its_own_last_lines() {
        let mut log = Log::default();
        for i in 0..10 {
            log.push(Channel::Script, format!("script {i}"));
        }
        for i in 0..LOG_LINES + 500 {
            log.push(Channel::Game, format!("game {i}"));
        }
        let lines: Vec<&Line> = log.lines().collect();
        let script = lines
            .iter()
            .filter(|l| l.channel == Channel::Script)
            .count();
        let game: Vec<&&Line> = lines
            .iter()
            .filter(|l| l.channel == Channel::Game)
            .collect();
        assert_eq!(script, 10, "a noisy channel doesn't push out a quiet one");
        assert_eq!(game.len(), LOG_LINES);
        assert_eq!(game[0].text, "game 500");
        assert_eq!(log.since(log.next - 2).count(), 2);
    }

    #[test]
    fn a_board_counts_what_happens_and_logs_it_under_its_channel() {
        let mut board = Board::default();
        for event in [
            Event::Killed {
                id: MonMapId(1),
                name: "Mosquito".into(),
                gold: 10,
                exp: 5,
            },
            Event::Died,
            Event::Ready {
                level: 50,
                gold: 1200,
                items: 2,
            },
            Event::Ready {
                level: 50,
                gold: 9999,
                items: 2,
            },
            Event::Notice {
                kind: NoticeKind::Server,
                text: "Profanity filter On.".into(),
            },
            Event::Notice {
                kind: NoticeKind::Warning,
                text: "You are being sent back to town.".into(),
            },
            Event::Chat {
                channel: "zone".into(),
                sender: "player2".into(),
                text: "hi".into(),
            },
            Event::Whisper {
                sender: "player2".into(),
                text: "psst".into(),
            },
        ] {
            board.record(&event);
        }
        assert_eq!(
            (board.stats.kills, board.stats.deaths, board.stats.gold_from),
            (1, 1, Some(1200))
        );
        let logged: Vec<(Channel, &str)> = board
            .log
            .lines()
            .map(|l| (l.channel, l.text.as_str()))
            .collect();
        assert_eq!(
            logged,
            [
                (Channel::Game, "killed Mosquito, +10 gold, +5 xp"),
                (Channel::Game, "died"),
                (Channel::Aqx, "ready: level 50, 1200 gold, 2 items"),
                (Channel::Aqx, "ready: level 50, 9999 gold, 2 items"),
                (Channel::Server, "Profanity filter On."),
                (Channel::Server, "warning: You are being sent back to town."),
                (Channel::Chat, "[zone] player2: hi"),
                (Channel::Chat, "[whisper] player2: psst"),
            ]
        );
    }
}
