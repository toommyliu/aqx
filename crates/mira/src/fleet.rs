use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mira_client::{ActionError, CombatState, DeviceId, Event, Handle, ItemId, Phase, Server, api};
use mira_script::{Cancel, Level, Outcome, Output};
use serde::{Deserialize, Serialize};

use crate::config::{self, OnStall, Script};
use crate::describe::describe;
use crate::runner::{self, Job, Login, Note};

pub const LOG_LINES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Mira,
    Script,
    Server,
    Chat,
    Game,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Channel::Mira => "mira",
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
            | Event::Packet { .. } => Channel::Mira,
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

    pub fn next_seq(&self) -> u64 {
        self.next
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
    last: Option<Script>,
    runs: u64,
    links: u64,
    queued_runs: usize,
    queued_logins: usize,
    last_progress: Instant,
    stalled: bool,
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
        if let Some(text) = describe(event) {
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
        self.board().log.push(Channel::Mira, text.into());
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
            .push(Channel::Mira, "logging out to log in again".into());
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

pub struct Fleet {
    device: DeviceId,
    accounts: RwLock<Vec<Arc<Account>>>,
}

impl Fleet {
    pub fn new(device: DeviceId) -> Fleet {
        Fleet {
            device,
            accounts: RwLock::default(),
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
            login: Login {
                device: self.device.clone(),
                user: config.user,
                pass: config.password,
                client_version: config.client_version,
                reconnect: config.reconnect,
                api: config.api,
                record: None,
            },
        });
        thread::Builder::new()
            .name(format!("mira-account-{}", account.name))
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
                    "{} changed this account; the change applies once the session restarts (mira down, then mira)",
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
                        Channel::Mira,
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
            Channel::Mira,
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
        let server = self.account.server.clone();
        {
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
        }
        self.account.say(format!("logging in to {server}"));
        let (attempts, mut delay) = match self.login.reconnect {
            Some(reconnect) => (reconnect.attempts.max(1), reconnect.delay),
            None => (1, Duration::ZERO),
        };
        for attempt in 1..=attempts {
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
                    Channel::Mira,
                    format!(
                        "couldn't log in: {why}; trying again in {} s (attempt {} of {attempts})",
                        delay.as_secs(),
                        attempt + 1
                    ),
                );
                board.link = Link::Waiting { why, until };
            }
            while Instant::now() < until {
                if self.account.board().links != link {
                    self.account.board().link = Link::Offline { why: None };
                    return None;
                }
                thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
            }
            self.account.board().link = Link::Connecting;
            delay = delay.saturating_mul(2).min(LOGIN_RETRY_MAX);
        }
        None
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
                .push(Channel::Mira, format!("couldn't log in: {why}"));
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
        let (how, restartable) = match loaded {
            Err((why, restartable)) => (Ended::Failed(why), restartable),
            Ok((loaded, args)) => {
                let relogin: Arc<dyn mira_script::Account> = Arc::clone(self) as _;
                let ran = runner::drive(&job, loaded, &args, &relogin, session, server, |note| {
                    match note {
                    Note::Session(session) => {
                        let server = match &self.account.board().link {
                            Link::Session { server, .. } => server.clone(),
                            _ => self.account.server.clone(),
                        };
                        self.attach(session, &server);
                    }
                    Note::Restarting => self.account.say("restarting the script"),
                    Note::Relogging {
                        server,
                        delay,
                        attempt,
                        attempts,
                    } => self.account.say(format!(
                        "the session closed; logging back in to {server} in {:.1} s (attempt {attempt} of {attempts})",
                        delay.as_secs_f64()
                    )),
                    Note::ReloginFailed(e) => self.account.say(format!("couldn't log back in: {e}")),
                }
                });
                self.attach(&ran.session, &ran.server);
                let how = match ran.outcome {
                    Outcome::Finished => Ended::Finished,
                    Outcome::Stopped(reason) => Ended::Stopped(reason),
                    Outcome::Cancelled => Ended::Stopped(None),
                    Outcome::LoggedOut => Ended::Stopped(Some("the script logged out".into())),
                    Outcome::Disconnected(why) => Ended::Closed(why),
                    Outcome::Failed(why) => Ended::Failed(why),
                    Outcome::Restart | Outcome::Relogin(_) => unreachable!("handled by the runner"),
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

impl mira_script::Account for Linked {
    fn servers(&self) -> Result<Vec<Server>, String> {
        match &self.login.api {
            Some(host) => api::servers_on(host, &self.login.device),
            None => api::servers(&self.login.device),
        }
        .map_err(|e| e.to_string())
    }

    fn login(&self, server: &str) -> Result<Handle, String> {
        self.connect(server, &self.login.user, &self.login.pass)
            .map_err(Failure::into_reason)
    }

    fn login_as(&self, server: &str, username: &str, password: &str) -> Result<Handle, String> {
        self.connect(server, username, password)
            .map_err(Failure::into_reason)
    }
}

#[cfg(test)]
impl Fleet {
    pub(crate) fn of(accounts: Vec<Arc<Account>>) -> Fleet {
        Fleet {
            device: mira_client::testing::device_id(),
            accounts: RwLock::new(accounts),
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
    use mira_client::{MonMapId, NoticeKind};

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
            login: Login {
                device: mira_client::testing::device_id(),
                user: "player1".into(),
                pass: "x".into(),
                client_version: None,
                reconnect: None,
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
            id: mira_client::MonMapId(1),
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
        let path = Path::new("/srv/mira.toml");
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
            autologin: true,
            ..unchanged
        };
        fleet.refresh(vec![changed.clone()], path);
        fleet.refresh(vec![changed, config::Account::for_tests("player2")], path);
        assert_eq!(noted(&fleet), 1, "a change is noted once");
        assert_eq!(fleet.accounts().len(), 2, "a new name still joins");
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
        assert_eq!(log.since(log.next_seq() - 2).count(), 2);
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
                (Channel::Mira, "ready: level 50, 1200 gold, 2 items"),
                (Channel::Mira, "ready: level 50, 9999 gold, 2 items"),
                (Channel::Server, "server: Profanity filter On."),
                (Channel::Chat, "[zone] player2: hi"),
                (Channel::Chat, "[whisper] player2: psst"),
            ]
        );
    }
}
