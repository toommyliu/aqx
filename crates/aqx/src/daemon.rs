use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::config::{self, Script};
use crate::control::{Action, Frame, Source, Want};
use crate::fleet::{Fleet, LastLines, Line, Resume};

const STARTUP: Duration = Duration::from_secs(10);
const REPLY_WITHIN: Duration = Duration::from_secs(40);
const DOWN_WITHIN: Duration = Duration::from_secs(30);
const ACCEPT_RETRY: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_millis(250);
const WATCHERS_FINISH: Duration = Duration::from_secs(1);
const LOCK_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Hello,
    Frame(Want),
    Act { account: usize, action: Action },
    Configured,
    Resumable,
    LastLines,
    End,
    Down,
}

#[derive(Debug, Serialize, Deserialize)]
enum Reply {
    Hello {
        #[serde(default)]
        protocol: u32,
        build: String,
        config: PathBuf,
        #[serde(default)]
        started_ms: u64,
    },
    Frame(Frame),
    Done,
    Refused(String),
    Configured(Vec<Script>),
    Resumable(Vec<Resume>),
    LastLines(Vec<LastLines>),
}

pub fn dir() -> anyhow::Result<PathBuf> {
    let dir = match std::env::var_os("AQX_SESSIONS").filter(|p| !p.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => dirs::data_dir()
            .context("no data folder to keep sessions in")?
            .join("aqx")
            .join("sessions"),
    };
    std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

pub struct Paths {
    pub socket: PathBuf,
    lock: PathBuf,
    pub log: PathBuf,
    last: PathBuf,
}

pub fn paths_for(config: &Path) -> anyhow::Result<Paths> {
    Ok(paths_in(&dir()?, config))
}

fn paths_in(dir: &Path, config: &Path) -> Paths {
    let stem = config
        .file_stem()
        .map_or_else(|| "aqx".into(), |s| s.to_string_lossy().into_owned());
    let base = format!("{stem}-{:016x}", fnv(config.as_os_str().as_encoded_bytes()));
    Paths {
        socket: dir.join(format!("{base}.sock")),
        lock: dir.join(format!("{base}.lock")),
        log: dir.join(format!("{base}.log")),
        last: dir.join(format!("{base}.last.json")),
    }
}

pub struct Ended {
    pub at: SystemTime,
    pub accounts: Vec<LastLines>,
}

fn last_path(config: &Path) -> Option<PathBuf> {
    Some(paths_for(&std::path::absolute(config).ok()?).ok()?.last)
}

pub fn last_session_at(config: &Path) -> Option<SystemTime> {
    std::fs::metadata(last_path(config)?)
        .and_then(|m| m.modified())
        .ok()
}

pub fn last_session(config: &Path) -> Option<Ended> {
    read_ended(&last_path(config)?)
}

fn read_ended(path: &Path) -> Option<Ended> {
    let at = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let accounts = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(Ended { at, accounts })
}

fn keep_last_lines(path: &Path, fleet: &Fleet) {
    let written = serde_json::to_vec(&fleet.last_lines())
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(path, json));
    if let Err(e) = written {
        eprintln!(
            "aqx: couldn't keep the last lines in {}: {e}",
            path.display()
        );
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub const PROTOCOL: u32 = 15;
const BUILD: &str = env!("AQX_BUILD");

pub struct Session {
    pub config: PathBuf,
    pub protocol: u32,
    pub build: String,
    pub started_ms: u64,
    remote: Remote,
}

impl Session {
    pub fn compatible(&self) -> bool {
        self.protocol == PROTOCOL
    }

    pub fn same_build(&self) -> bool {
        self.compatible() && self.build == BUILD
    }

    pub fn source(&self) -> &dyn Source {
        &self.remote
    }

    pub fn resumable(&self) -> anyhow::Result<Vec<Resume>> {
        match self
            .remote
            .request(&Request::Resumable)
            .map_err(anyhow::Error::msg)?
        {
            Reply::Resumable(accounts) => Ok(accounts),
            other => bail!("the session answered {other:?}"),
        }
    }

    pub fn down(&self) -> anyhow::Result<Vec<LastLines>> {
        match self
            .remote
            .request(&Request::Down)
            .map_err(anyhow::Error::msg)?
        {
            Reply::LastLines(lines) => Ok(lines),
            Reply::Refused(why) => bail!(why),
            other => bail!("the session answered {other:?}"),
        }
    }
}

pub fn connect(socket: &Path) -> Option<Session> {
    let stream = UnixStream::connect(socket).ok()?;
    let remote = Remote::new(stream).ok()?;
    match remote.request(&Request::Hello) {
        Ok(Reply::Hello {
            protocol,
            build,
            config,
            started_ms,
        }) => Some(Session {
            config,
            protocol,
            build,
            started_ms,
            remote,
        }),
        _ => None,
    }
}

pub fn session_for(config: &Path) -> anyhow::Result<Option<Session>> {
    Ok(connect(&paths_for(config)?.socket))
}

pub fn sessions() -> anyhow::Result<Vec<Session>> {
    let mut sockets: Vec<PathBuf> = std::fs::read_dir(dir()?)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "sock"))
        .collect();
    sockets.sort();
    Ok(sockets
        .iter()
        .filter_map(|socket| connect(socket))
        .collect())
}

struct Remote {
    io: Mutex<(BufReader<UnixStream>, UnixStream)>,
}

impl Remote {
    fn new(stream: UnixStream) -> std::io::Result<Remote> {
        stream.set_read_timeout(Some(REPLY_WITHIN))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Remote {
            io: Mutex::new((reader, stream)),
        })
    }

    fn request(&self, request: &Request) -> Result<Reply, String> {
        let mut io = self.io.lock().unwrap_or_else(PoisonError::into_inner);
        let (reader, writer) = &mut *io;
        let mut line = serde_json::to_string(request).expect("requests serialize");
        line.push('\n');
        writer
            .write_all(line.as_bytes())
            .map_err(|_| "the aqx session ended".to_owned())?;
        let mut answer = String::new();
        match reader.read_line(&mut answer) {
            Ok(0) => Err("the aqx session ended".into()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err("the aqx session stopped answering".into())
            }
            Err(_) => Err("the aqx session ended".into()),
            Ok(_) => serde_json::from_str(&answer).map_err(|e| format!("the aqx session sent {e}")),
        }
    }
}

impl Source for Remote {
    fn frame(&self, want: &Want) -> Result<Frame, String> {
        match self.request(&Request::Frame(*want))? {
            Reply::Frame(frame) => Ok(frame),
            other => Err(format!("the session answered {other:?} to a frame")),
        }
    }

    fn act(&self, account: usize, action: Action) -> Result<(), String> {
        match self.request(&Request::Act { account, action })? {
            Reply::Done => Ok(()),
            Reply::Refused(why) => Err(why),
            other => Err(format!("the session answered {other:?}")),
        }
    }

    fn configured(&self) -> Vec<Script> {
        match self.request(&Request::Configured) {
            Ok(Reply::Configured(scripts)) => scripts,
            _ => Vec::new(),
        }
    }

    fn end(&self) -> Result<(), String> {
        match self.request(&Request::End)? {
            Reply::Done => Ok(()),
            Reply::Refused(why) => Err(why),
            other => Err(format!("the session answered {other:?}")),
        }
    }
}

pub struct Wanted {
    pub accounts: Vec<String>,
    pub start: Start,
}

pub enum Start {
    Login,
    LoginOnly,
    Run(Script),
}

impl Start {
    pub fn action(&self) -> Action {
        match self {
            Start::Login => Action::Login,
            Start::LoginOnly => Action::LoginOnly,
            Start::Run(script) => Action::Run(script.clone()),
        }
    }
}

impl Wanted {
    fn args(&self) -> Vec<String> {
        let mut args = self.accounts.clone();
        match &self.start {
            Start::Login => {}
            Start::LoginOnly => args.push("--no-script".into()),
            Start::Run(script) => {
                args.push("--script".into());
                args.push(script.path.display().to_string());
                for (key, value) in &script.args {
                    args.push("--arg".into());
                    args.push(format!("{key}={value}"));
                }
            }
        }
        args
    }
}

pub fn start(config: &Path, wanted: &Wanted) -> anyhow::Result<Session> {
    let paths = paths_for(config)?;
    let _ = std::fs::remove_file(&paths.log);
    let log = std::fs::File::create(&paths.log).with_context(|| paths.log.display().to_string())?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["up", "--quiet", "--config"])
        .arg(config)
        .args(wanted.args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().context("couldn't start the session")?;
    let until = Instant::now() + STARTUP;
    loop {
        if let Some(session) = connect(&paths.socket) {
            return Ok(session);
        }
        if let Some(status) = child.try_wait()? {
            let said = std::fs::read_to_string(&paths.log).unwrap_or_default();
            let said = said.trim().trim_start_matches("aqx: ");
            if said.is_empty() {
                bail!("the session exited ({status})");
            }
            bail!("{said}");
        }
        if Instant::now() >= until {
            bail!(
                "the session didn't start within {} s; see {}",
                STARTUP.as_secs(),
                paths.log.display()
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug)]
pub struct Lock {
    _held: std::fs::File,
}

fn lock(path: &Path, wait: Duration) -> anyhow::Result<Lock> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| path.display().to_string())?;
    let until = Instant::now() + wait;
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() >= until {
            bail!("aqx is already running for this config");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(Lock { _held: file })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Print {
    Plain,
    Json,
    Quiet,
}

pub struct Server {
    fleet: Arc<Fleet>,
    config: PathBuf,
    socket: PathBuf,
    started_ms: u64,
    ending: AtomicBool,
    down: AtomicBool,
    answering: Mutex<bool>,
}

impl Server {
    pub fn new(fleet: Arc<Fleet>, config: PathBuf, socket: PathBuf) -> Arc<Server> {
        Arc::new(Server {
            fleet,
            config,
            socket,
            started_ms: now_ms(),
            ending: AtomicBool::new(false),
            down: AtomicBool::new(false),
            answering: Mutex::new(true),
        })
    }

    fn stop_answering(&self) {
        let mut answering = self
            .answering
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *answering {
            let _ = std::fs::remove_file(&self.socket);
            *answering = false;
        }
    }

    fn done(&self) -> bool {
        self.fleet.idle() && (self.fleet.commanded() || self.down.load(Ordering::SeqCst))
    }

    pub fn listen(self: &Arc<Self>, listener: UnixListener) {
        let server = Arc::clone(self);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    thread::sleep(ACCEPT_RETRY);
                    continue;
                };
                let server = Arc::clone(&server);
                thread::spawn(move || server.answer(stream));
            }
        });
    }

    fn answer(&self, stream: UnixStream) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                return;
            };
            let reply = match serde_json::from_str::<Request>(&line) {
                Err(e) => Reply::Refused(format!("couldn't read the request: {e}")),
                Ok(Request::Hello) => Reply::Hello {
                    protocol: PROTOCOL,
                    build: BUILD.to_owned(),
                    config: self.config.clone(),
                    started_ms: self.started_ms,
                },
                Ok(Request::Frame(want)) => match self.fleet.frame(&want) {
                    Ok(mut frame) => {
                        frame.ending = self.ending.load(Ordering::SeqCst);
                        Reply::Frame(frame)
                    }
                    Err(why) => Reply::Refused(why),
                },
                Ok(Request::Act {
                    action:
                        Action::Login
                        | Action::LoginOnly
                        | Action::Relogin
                        | Action::Switch(_)
                        | Action::Restart
                        | Action::Run(_)
                        | Action::Next,
                    ..
                }) if self.down.load(Ordering::SeqCst) => {
                    Reply::Refused("the session is ending".into())
                }
                Ok(Request::Act { account, action }) => match self.fleet.act(account, action) {
                    Ok(()) => Reply::Done,
                    Err(why) => Reply::Refused(why),
                },
                Ok(Request::Configured) => Reply::Configured(self.fleet.configured()),
                Ok(Request::Resumable) => Reply::Resumable(self.fleet.resumable()),
                Ok(Request::LastLines) => Reply::LastLines(self.fleet.last_lines()),
                Ok(Request::End) => {
                    self.end();
                    Reply::Done
                }
                Ok(Request::Down) => self.down(),
            };
            let mut text = serde_json::to_string(&reply).expect("replies serialize");
            text.push('\n');
            if writer.write_all(text.as_bytes()).is_err() {
                return;
            }
        }
    }

    fn end(&self) {
        self.down.store(true, Ordering::SeqCst);
        self.fleet.logout_all();
    }

    fn down(&self) -> Reply {
        self.end();
        let until = Instant::now() + DOWN_WITHIN;
        while !self.fleet.idle() {
            if Instant::now() >= until {
                return Reply::Refused(format!(
                    "the session is still logging out after {} s",
                    DOWN_WITHIN.as_secs()
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.stop_answering();
        Reply::LastLines(self.fleet.last_lines())
    }

    pub fn run_until_done(&self, print: Print, release: impl FnOnce()) {
        let mut printed: Vec<u64> = Vec::new();
        loop {
            self.fleet.supervise(Instant::now());
            print_new(&self.fleet, &mut printed, print);
            if self.done() {
                self.stop_answering();
                self.ending.store(true, Ordering::SeqCst);
                release();
                thread::sleep(WATCHERS_FINISH);
                print_new(&self.fleet, &mut printed, print);
                return;
            }
            thread::sleep(TICK);
        }
    }
}

fn print_new(fleet: &Fleet, printed: &mut Vec<u64>, print: Print) {
    if print == Print::Quiet {
        return;
    }
    let accounts = fleet.accounts();
    printed.resize(accounts.len(), 0);
    for (account, next) in accounts.iter().zip(printed.iter_mut()) {
        let lines: Vec<Line> = account.board().log.since(*next).cloned().collect();
        for line in lines {
            *next = line.seq + 1;
            match print {
                Print::Plain => println!(
                    "{}  {}  {:<6}  {}",
                    crate::view::time_of_day(line.at_ms),
                    account.name,
                    line.channel.label(),
                    line.text
                ),
                Print::Json => println!(
                    "{}",
                    serde_json::json!({
                        "t": line.at_ms,
                        "account": account.name,
                        "channel": line.channel.label(),
                        "text": line.text,
                    })
                ),
                Print::Quiet => {}
            }
        }
    }
}

pub fn serve(config: &Path, wanted: &Wanted, print: Print) -> anyhow::Result<()> {
    unsafe { libc::umask(0o077) };
    let config = std::path::absolute(config)?;
    let paths = paths_for(&config)?;
    let held = lock(&paths.lock, LOCK_WAIT)?;
    let _ = std::fs::remove_file(&paths.socket);
    let listener =
        UnixListener::bind(&paths.socket).with_context(|| paths.socket.display().to_string())?;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600))?;
    let removed = Removed(paths.socket.clone());
    let accounts = config::load(&config).map_err(anyhow::Error::msg)?;
    let first: Vec<String> = match &wanted.accounts[..] {
        [] => accounts
            .iter()
            .filter(|a| a.auto_start)
            .map(|a| a.name.clone())
            .collect(),
        names => names
            .iter()
            .map(|wanted| find_config_account(&accounts, wanted))
            .collect::<anyhow::Result<_>>()?,
    };
    let fleet = Arc::new(Fleet::new(aqx_client::DeviceId::load()?));
    fleet.add_new(accounts);
    for (i, account) in fleet.accounts().iter().enumerate() {
        if first.contains(&account.name)
            && let Err(why) = fleet.act(i, wanted.start.action())
        {
            account.say(format!("couldn't start: {why}"));
        }
    }
    fleet.watch(config.clone());
    let server = Server::new(Arc::clone(&fleet), config, paths.socket.clone());
    server.listen(listener);
    let presses = Arc::new(AtomicUsize::new(0));
    {
        let server = Arc::clone(&server);
        ctrlc::set_handler(move || {
            if presses.fetch_add(1, Ordering::SeqCst) > 0 {
                std::process::exit(130);
            }
            server.end();
        })?;
    }
    let last = paths.last.clone();
    let ending = Arc::clone(&fleet);
    server.run_until_done(print, move || {
        keep_last_lines(&last, &ending);
        drop((removed, held));
    });
    Ok(())
}

struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn find_config_account(accounts: &[config::Account], wanted: &str) -> anyhow::Result<String> {
    accounts
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(wanted) || a.user.eq_ignore_ascii_case(wanted))
        .map(|a| a.name.clone())
        .ok_or_else(|| {
            let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
            anyhow::anyhow!("no account {wanted}; the accounts are {}", names.join(", "))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{Account, Channel};

    #[test]
    fn a_client_drives_the_session_and_down_ends_it() {
        let dir = std::env::temp_dir().join(format!("aqx-daemon-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("aqx.sock");
        let accounts = ["player1", "player2"].map(|name| Arc::new(Account::detached(name)));
        accounts[1]
            .board()
            .log
            .push(Channel::Script, "farming".into());
        let fleet = Arc::new(Fleet::of(accounts.to_vec()));
        let server = Server::new(
            Arc::clone(&fleet),
            PathBuf::from("/srv/aqx.toml"),
            socket.clone(),
        );
        server.listen(UnixListener::bind(&socket).unwrap());

        let session = connect(&socket).expect("the session answers the handshake");
        assert!(session.same_build() && session.started_ms > 0);
        assert_eq!(session.config, Path::new("/srv/aqx.toml"));
        let source = session.source();
        let want = Want {
            selected: 1,
            since: 0,
            full: true,
        };
        let frame = source.frame(&want).unwrap();
        let names: Vec<&str> = frame.accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["player1", "player2"]);
        assert_eq!(
            frame
                .log
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            ["farming"]
        );
        assert!(!frame.ending);
        let later = Want {
            since: frame.log[0].seq + 1,
            ..want
        };
        assert!(source.frame(&later).unwrap().log.is_empty());
        assert_eq!(source.act(0, Action::Logout), Err("already offline".into()));
        assert_eq!(source.act(7, Action::Stop), Err("no such account".into()));
        let daily = Script {
            path: "/srv/daily.ts".into(),
            args: Vec::new(),
        };
        assert_eq!(source.act(1, Action::Enqueue(daily.clone())), Ok(()));
        let queued = source.frame(&want).unwrap().accounts.swap_remove(1).queue;
        assert_eq!(
            queued.iter().map(|q| &q.script).collect::<Vec<_>>(),
            [&daily]
        );
        assert_eq!(source.act(1, Action::Unqueue(queued[0].id)), Ok(()));
        assert_eq!(
            source.act(1, Action::Unqueue(queued[0].id)),
            Err("that script already left the queue".into())
        );
        assert!(
            session.resumable().unwrap().is_empty(),
            "nobody is online to resume"
        );

        assert!(!server.done(), "an untouched session waits for a login");
        let watcher = connect(&socket).unwrap();
        let last = session.down().unwrap();
        assert!(
            connect(&socket).is_none(),
            "an ending session takes no new clients"
        );
        assert_eq!(
            last[1]
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            ["farming"]
        );
        assert!(server.done(), "after down, an idle session is done");
        let config_lock = dir.join("aqx.lock");
        let held = lock(&config_lock, Duration::ZERO).unwrap();
        let ended = thread::spawn(move || server.run_until_done(Print::Quiet, move || drop(held)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !watcher.source().frame(&want).unwrap().ending {
            assert!(
                Instant::now() < deadline,
                "a watcher sees the session ending"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(
                watcher.remote.request(&Request::LastLines),
                Ok(Reply::LastLines(lines)) if lines.len() == 2
            ),
            "a watcher can still read the last lines"
        );
        assert!(
            lock(&config_lock, Duration::ZERO).is_ok() && !ended.is_finished(),
            "a new session can start while watchers finish"
        );
        std::fs::write(&socket, "").unwrap();
        watcher.down().unwrap();
        assert!(
            socket.exists(),
            "a late down leaves the next session's socket"
        );
        ended.join().unwrap();
    }

    #[test]
    fn an_ended_session_leaves_each_accounts_last_lines_behind() {
        let dir = std::env::temp_dir().join(format!("aqx-last-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let account = Account::detached("player1");
        account.say("couldn't log in: the server refused");
        let fleet = Fleet::of(vec![Arc::new(account)]);
        let path = dir.join("aqx.last.json");
        keep_last_lines(&path, &fleet);
        let ended = read_ended(&path).expect("the last lines are kept");
        assert_eq!(ended.accounts, fleet.last_lines());
        assert_eq!(
            ended.accounts[0].lines.last().map(|l| l.text.as_str()),
            Some("couldn't log in: the server refused")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn end_answers_at_once_and_the_session_finishes_when_idle() {
        let dir = std::env::temp_dir().join(format!("aqx-end-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("aqx.sock");
        let script = Script {
            path: PathBuf::from("/srv/farm.ts"),
            args: Vec::new(),
        };
        let account = Account::detached_with("player1", script.clone());
        let fleet = Arc::new(Fleet::of(vec![Arc::new(account)]));
        let server = Server::new(
            Arc::clone(&fleet),
            PathBuf::from("/srv/aqx.toml"),
            socket.clone(),
        );
        server.listen(UnixListener::bind(&socket).unwrap());
        let session = connect(&socket).unwrap();
        assert!(!server.done());
        let asked = Instant::now();
        assert_eq!(session.source().end(), Ok(()));
        assert!(
            asked.elapsed() < Duration::from_secs(1),
            "end doesn't wait for the logouts"
        );
        assert!(server.done(), "with everyone offline, the session is done");
        for action in [
            Action::Login,
            Action::LoginOnly,
            Action::Relogin,
            Action::Restart,
            Action::Run(script),
            Action::Next,
        ] {
            assert_eq!(
                session.source().act(0, action),
                Err("the session is ending".into()),
                "an ending session logs nobody in"
            );
        }
        assert!(server.done());
    }

    fn shape(value: &serde_json::Value, path: &str, out: &mut std::collections::BTreeSet<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, field) in fields {
                    let path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    out.insert(path.clone());
                    shape(field, &path, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| shape(item, path, out)),
            serde_json::Value::String(text) if path.is_empty() => {
                out.insert(format!("={text}"));
            }
            _ => {}
        }
    }

    fn samples() -> Vec<serde_json::Value> {
        use crate::view::{
            Aura, AuraTimer, Game, Player, QuestProgress, Shown, Stack, Standing, State, Step,
            Target, TurnIn,
        };
        let script = Script {
            path: PathBuf::from("/srv/farm.ts"),
            args: vec![("n".into(), "1".into())],
        };
        let stack = Stack {
            id: aqx_client::ItemId(1),
            name: "Bone Dust".into(),
            qty: 1,
            max: 10,
            equipped: false,
            rank: Some(1),
            category: crate::view::ItemCategory::Items,
        };
        let game = Game {
            skills: vec![crate::view::SkillState {
                name: "Slash".into(),
                ready: 1.0,
                left_ms: 0,
            }],
            auras: vec![Aura {
                name: "Haste".into(),
                stacks: 1,
                description: "Increases haste".into(),
                timer: Some(AuraTimer {
                    duration_ms: 2000,
                    left_ms: 1000,
                }),
            }],
            target: Some(Target {
                id: 1,
                auras: vec![Aura {
                    name: "Mark".into(),
                    stacks: 2,
                    description: String::new(),
                    timer: Some(AuraTimer {
                        duration_ms: 4000,
                        left_ms: 3000,
                    }),
                }],
                name: "Frogzard".into(),
                hp: 1,
                max_hp: 2,
            }),
            players: vec![Player {
                name: "player2".into(),
                level: Some(1),
            }],
            drops: vec![stack.clone()],
            quests: vec![QuestProgress {
                id: 1,
                name: "Quest".into(),
                items: vec![TurnIn {
                    name: "Bone Dust".into(),
                    have: 1,
                    need: 2,
                }],
            }],
            factions: vec![Standing {
                name: "Good".into(),
                rank: 1,
                rep: 2,
                next: Some(3),
                gained: true,
            }],
            temp: vec![stack.clone()],
            inventory: vec![stack.clone()],
            bank: Some(vec![stack]),
            ..Game::default()
        };
        let states = [
            State::Offline {
                why: Some("why".into()),
            },
            State::Busy(Step::Connecting),
            State::Busy(Step::LoggingIn),
            State::Busy(Step::Joining),
            State::Busy(Step::Loading),
            State::Reconnecting("in 5 s".into()),
            State::Online,
            State::Running,
            State::Paused,
            State::Stalled,
        ];
        let shown = |state: State| Shown {
            name: "player1".into(),
            user: "player1".into(),
            state,
            server: "Twilly".into(),
            script: Some("/srv/farm.ts".into()),
            args: None,
            status: String::new(),
            registered: Some(aqx_script::Registered {
                drops: vec![
                    aqx_script::Item::Id(1),
                    aqx_script::Item::Name("Bone Dust".into()),
                ],
                quests: vec![aqx_script::RegisteredQuest {
                    id: 1,
                    reward: Some(aqx_script::Item::Id(2)),
                }],
            }),
            up: Some(Duration::from_secs(1)),
            stats: crate::fleet::Stats::default(),
            gold_gained: Some(1),
            game: Some(game.clone()),
            recording: Some("/tmp/a.jsonl".into()),
            queue: vec![crate::fleet::Queued {
                id: 1,
                script: script.clone(),
            }],
        };
        let line = |channel: Channel| Line {
            seq: 0,
            at_ms: 0,
            channel,
            text: String::new(),
        };
        let channels = [
            Channel::Aqx,
            Channel::Script,
            Channel::Server,
            Channel::Chat,
            Channel::Game,
            Channel::Sent,
            Channel::Received,
        ];
        let actions = [
            Action::Login,
            Action::LoginOnly,
            Action::Logout,
            Action::Stop,
            Action::Restart,
            Action::Pause,
            Action::Resume,
            Action::Relogin,
            Action::Switch("Twilly".into()),
            Action::Run(script.clone()),
            Action::Enqueue(script.clone()),
            Action::Unqueue(1),
            Action::MoveQueued { id: 1, to: 0 },
            Action::SetQueuedArgs {
                id: 1,
                args: vec![("a".into(), "1".into())],
            },
            Action::PickUp(vec![1]),
            Action::Registry(aqx_script::Edit::RegisterDrop(aqx_script::Item::Id(1))),
            Action::Registry(aqx_script::Edit::UnregisterDrop(aqx_script::Item::Name(
                "Bone Dust".into(),
            ))),
            Action::Registry(aqx_script::Edit::RegisterQuest(
                aqx_script::RegisteredQuest {
                    id: 1,
                    reward: Some(aqx_script::Item::Id(2)),
                },
            )),
            Action::Registry(aqx_script::Edit::UnregisterQuest(1)),
            Action::Record(Some("/tmp/a.jsonl".into())),
            Action::StopRecording,
        ];
        let want = Want {
            selected: 0,
            since: 0,
            full: true,
        };
        let mut requests = vec![
            Request::Hello,
            Request::Frame(want),
            Request::Configured,
            Request::Resumable,
            Request::LastLines,
            Request::End,
            Request::Down,
        ];
        requests.extend(
            actions
                .into_iter()
                .map(|action| Request::Act { account: 0, action }),
        );
        let replies = [
            Reply::Hello {
                protocol: PROTOCOL,
                build: String::new(),
                config: PathBuf::new(),
                started_ms: 0,
            },
            Reply::Frame(Frame {
                accounts: states.iter().cloned().map(shown).collect(),
                log: channels.into_iter().map(line).collect(),
                ending: false,
            }),
            Reply::Done,
            Reply::Refused(String::new()),
            Reply::Configured(vec![script.clone()]),
            Reply::Resumable(vec![Resume {
                name: "player1".into(),
                online: true,
                script: Some(script.clone()),
                queue: vec![script],
            }]),
            Reply::LastLines(vec![LastLines {
                name: "player1".into(),
                user: "player1".into(),
                lines: vec![line(Channel::Aqx)],
            }]),
        ];
        let mut values: Vec<serde_json::Value> = requests
            .iter()
            .map(|r| serde_json::to_value(r).unwrap())
            .collect();
        values.extend(replies.iter().map(|r| serde_json::to_value(r).unwrap()));
        values.extend(states.iter().map(|v| serde_json::to_value(v).unwrap()));
        values.extend(channels.iter().map(|v| serde_json::to_value(v).unwrap()));
        values.extend(
            [
                Step::Connecting,
                Step::LoggingIn,
                Step::Joining,
                Step::Loading,
            ]
            .iter()
            .map(|v| serde_json::to_value(v).unwrap()),
        );
        values.extend(
            [
                Action::Login,
                Action::LoginOnly,
                Action::Logout,
                Action::Stop,
                Action::Restart,
                Action::Pause,
                Action::Resume,
                Action::Relogin,
                Action::ClearQueue,
                Action::Next,
                Action::StopRecording,
            ]
            .iter()
            .map(|v| serde_json::to_value(v).unwrap()),
        );
        values
    }

    #[test]
    fn the_wire_format_is_pinned_to_its_protocol() {
        let mut keys = std::collections::BTreeSet::new();
        for value in samples() {
            shape(&value, "", &mut keys);
        }
        let shape = keys.into_iter().collect::<Vec<_>>().join(" ");
        let pinned = "=Configured =Connecting =Done =Down =End =Hello =Joining =LastLines =Loading =LoggingIn \
             =Online =Paused =Resumable =Running =Stalled =aqx =chat =clear_queue =game =login \
             =login_only =logout =next =pause =received =relogin =restart =resume =script =sent \
             =server =stop =stop_recording Act Act.account Act.action Act.action.enqueue \
             Act.action.enqueue.args Act.action.enqueue.path Act.action.move_queued \
             Act.action.move_queued.id Act.action.move_queued.to Act.action.pick_up Act.action.record \
             Act.action.registry Act.action.registry.register_drop Act.action.registry.register_quest \
             Act.action.registry.register_quest.id Act.action.registry.register_quest.reward \
             Act.action.registry.unregister_drop Act.action.registry.unregister_quest \
             Act.action.run Act.action.run.args Act.action.run.path Act.action.set_queued_args \
             Act.action.set_queued_args.args Act.action.set_queued_args.id Act.action.switch \
             Act.action.unqueue Busy Configured Configured.args Configured.path Frame Frame.accounts \
             Frame.accounts.args Frame.accounts.game Frame.accounts.game.area \
             Frame.accounts.game.auras Frame.accounts.game.auras.description Frame.accounts.game.auras.name \
             Frame.accounts.game.auras.stacks Frame.accounts.game.auras.timer \
             Frame.accounts.game.auras.timer.duration_ms Frame.accounts.game.auras.timer.left_ms Frame.accounts.game.away Frame.accounts.game.bag_slots \
             Frame.accounts.game.bag_used Frame.accounts.game.bank Frame.accounts.game.bank.category Frame.accounts.game.bank.equipped \
             Frame.accounts.game.bank.id Frame.accounts.game.bank.max Frame.accounts.game.bank.name \
             Frame.accounts.game.bank.qty Frame.accounts.game.bank.rank \
             Frame.accounts.game.bank_slots Frame.accounts.game.bank_used Frame.accounts.game.cell \
             Frame.accounts.game.class Frame.accounts.game.class_rank Frame.accounts.game.dead \
             Frame.accounts.game.drops Frame.accounts.game.drops.category Frame.accounts.game.drops.equipped \
             Frame.accounts.game.drops.id Frame.accounts.game.drops.max \
             Frame.accounts.game.drops.name Frame.accounts.game.drops.qty \
             Frame.accounts.game.drops.rank Frame.accounts.game.exp Frame.accounts.game.exp_to_level \
             Frame.accounts.game.factions \
             Frame.accounts.game.factions.gained Frame.accounts.game.factions.name \
             Frame.accounts.game.factions.next Frame.accounts.game.factions.rank \
             Frame.accounts.game.factions.rep Frame.accounts.game.gold Frame.accounts.game.hp \
             Frame.accounts.game.inventory Frame.accounts.game.inventory.category Frame.accounts.game.inventory.equipped \
             Frame.accounts.game.inventory.id Frame.accounts.game.inventory.max \
             Frame.accounts.game.inventory.name Frame.accounts.game.inventory.qty \
             Frame.accounts.game.inventory.rank Frame.accounts.game.level Frame.accounts.game.max_hp \
             Frame.accounts.game.max_mp Frame.accounts.game.member Frame.accounts.game.misc_used \
             Frame.accounts.game.mp Frame.accounts.game.pad Frame.accounts.game.players \
             Frame.accounts.game.players.level Frame.accounts.game.players.name \
             Frame.accounts.game.quests Frame.accounts.game.quests.id Frame.accounts.game.quests.items \
             Frame.accounts.game.quests.items.have Frame.accounts.game.quests.items.name \
             Frame.accounts.game.quests.items.need Frame.accounts.game.quests.name \
             Frame.accounts.game.skills Frame.accounts.game.skills.left_ms \
             Frame.accounts.game.skills.name Frame.accounts.game.skills.ready \
             Frame.accounts.game.target Frame.accounts.game.target.auras \
             Frame.accounts.game.target.auras.description Frame.accounts.game.target.auras.name \
             Frame.accounts.game.target.auras.stacks Frame.accounts.game.target.auras.timer \
             Frame.accounts.game.target.auras.timer.duration_ms Frame.accounts.game.target.auras.timer.left_ms \
             Frame.accounts.game.target.hp Frame.accounts.game.target.id \
             Frame.accounts.game.target.max_hp Frame.accounts.game.target.name \
             Frame.accounts.game.temp Frame.accounts.game.temp.category Frame.accounts.game.temp.equipped Frame.accounts.game.temp.id \
             Frame.accounts.game.temp.max Frame.accounts.game.temp.name Frame.accounts.game.temp.qty \
             Frame.accounts.game.temp.rank Frame.accounts.gold_gained Frame.accounts.name \
             Frame.accounts.queue Frame.accounts.queue.id Frame.accounts.queue.script \
             Frame.accounts.queue.script.args Frame.accounts.queue.script.path \
             Frame.accounts.recording Frame.accounts.registered Frame.accounts.registered.drops \
             Frame.accounts.registered.quests Frame.accounts.registered.quests.id \
             Frame.accounts.registered.quests.reward Frame.accounts.script Frame.accounts.server \
             Frame.accounts.state Frame.accounts.state.Busy Frame.accounts.state.Offline \
             Frame.accounts.state.Offline.why Frame.accounts.state.Reconnecting Frame.accounts.stats \
             Frame.accounts.stats.deaths Frame.accounts.stats.drops Frame.accounts.stats.gold_from \
             Frame.accounts.stats.kills Frame.accounts.status Frame.accounts.up \
             Frame.accounts.up.nanos Frame.accounts.up.secs Frame.accounts.user Frame.ending \
             Frame.full Frame.log Frame.log.at_ms Frame.log.channel Frame.log.seq Frame.log.text \
             Frame.selected Frame.since Hello Hello.build Hello.config Hello.protocol \
             Hello.started_ms LastLines LastLines.lines LastLines.lines.at_ms LastLines.lines.channel \
             LastLines.lines.seq LastLines.lines.text LastLines.name LastLines.user Offline \
             Offline.why Reconnecting Refused Resumable Resumable.name Resumable.online Resumable.queue \
             Resumable.queue.args Resumable.queue.path Resumable.script Resumable.script.args \
             Resumable.script.path";
        assert_eq!(
            (PROTOCOL, shape.as_str()),
            (15, pinned),
            "the session messages changed: bump PROTOCOL, then update this shape"
        );
    }

    #[test]
    fn each_config_gets_its_own_session_files() {
        let dir = std::env::temp_dir().join(format!("aqx-paths-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = paths_in(&dir, Path::new("/srv/a/aqx.toml"));
        let b = paths_in(&dir, Path::new("/srv/b/aqx.toml"));
        assert_ne!(a.socket, b.socket);
        assert_eq!(
            a.socket,
            paths_in(&dir, Path::new("/srv/a/aqx.toml")).socket
        );
        let name = a.socket.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("aqx-") && name.ends_with(".sock"),
            "{name}"
        );
        let one = paths_in(&dir, Path::new("/srv/a/aqx.one.toml"));
        let two = paths_in(&dir, Path::new("/srv/a/aqx.two.toml"));
        assert_ne!(one.socket, two.socket, "a dot in the name keeps the hash");
        assert_ne!(one.lock, two.lock);
        let held = lock(&a.lock, Duration::ZERO).unwrap();
        assert!(
            lock(&a.lock, Duration::ZERO)
                .unwrap_err()
                .to_string()
                .contains("already running")
        );
        assert!(
            lock(&b.lock, Duration::ZERO).is_ok(),
            "another config can run at the same time"
        );
        let waiting = {
            let path = a.lock.clone();
            thread::spawn(move || lock(&path, Duration::from_secs(5)).map(drop))
        };
        thread::sleep(Duration::from_millis(200));
        drop(held);
        assert!(
            waiting.join().unwrap().is_ok(),
            "a new session waits for one that's still exiting"
        );
    }
}
