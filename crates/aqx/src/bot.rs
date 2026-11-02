use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aqx_core::{Control, Credentials, Disconnect, Event, Halt, LoginError, Phase, Recorder, Session, Sink, State, World};
use aqx_script::{Outcome, ScriptError};

use crate::config::{OnStall, Restart};
use crate::hub::{BotCell, Command, Hub, Run};
use crate::output::span;

const WATCH_EVERY: Duration = Duration::from_millis(250);
const STATUS_EVERY: Duration = Duration::from_secs(60);
pub const RELOGIN_FIRST: Duration = Duration::from_secs(35);
pub const RELOGIN_MAX: Duration = Duration::from_secs(300);
pub const RESTART_FIRST: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Script {
    pub path: PathBuf,
    pub args: Vec<(String, String)>,
}

pub fn expand_home(typed: &Path) -> PathBuf {
    match (typed.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => typed.to_path_buf(),
    }
}

pub fn script_path(typed: &Path) -> Result<PathBuf, String> {
    let path = expand_home(typed);
    let path = std::path::absolute(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.is_file() { Ok(path) } else { Err(format!("{} isn't a file", path.display())) }
}

pub struct Spec {
    pub creds: Credentials,
    pub server: Option<String>,
    pub version: String,
    pub script: Option<Script>,
    pub room: Option<u32>,
    pub restart: Restart,
    pub stall_after: Duration,
    pub on_stall: OnStall,
    pub exit_when_done: bool,
    pub relogin: Backoff,
    pub restarts: Backoff,
}

pub struct Backoff {
    first: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(first: Duration, max: Duration) -> Self {
        Backoff { first, max, next: first }
    }

    pub fn after(&mut self, lasted: Duration) -> Duration {
        if lasted >= self.max {
            self.next = self.first;
        }
        let delay = self.next;
        self.next = (self.next * 2).min(self.max);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        delay.mul_f64(0.8 + f64::from(nanos % 1000) / 1000.0 * 0.4)
    }
}

enum Next {
    Login,
    Idle,
    Run(Script),
    Quit(Result<(), String>),
}

struct Bot {
    spec: Spec,
    cell: Arc<BotCell>,
    hub: Arc<Hub>,
    wanted: Option<Script>,
    last: Option<Script>,
    ever_ready: bool,
}

pub fn run(spec: Spec, recorder: Option<Recorder>, control: Control, cell: Arc<BotCell>, hub: Arc<Hub>) -> Result<(), String> {
    let sink: Sink = {
        let (hub, cell) = (hub.clone(), cell.clone());
        Rc::new(move |event: &Event| hub.record(&cell, event))
    };
    let mut session = Session::new(control, spec.creds.clone(), &spec.version, recorder, sink);
    session.watch(WATCH_EVERY, watcher(&spec, &cell, &hub));
    let mut bot = Bot { spec, cell, hub, wanted: None, last: None, ever_ready: false };
    bot.publish(&session, String::new());
    let mut result = Ok(());
    while let Some(login) = bot.cell.next_login() {
        session.clear_stop();
        if bot.cell.has_commands() {
            session.stop_handle().stop();
        }
        bot.begin(match login {
            Command::Run { script, args } => Some(Script { path: script, args }),
            _ => bot.spec.script.clone(),
        });
        result = bot.drive(&mut session);
        bot.cell.board().leaving = true;
        if let Err(e) = &result {
            bot.say(format!("stopped: {e}"));
        }
        session.logout();
        bot.say("logged out");
        bot.cell.go_offline();
        bot.publish(&session, result.as_ref().err().cloned().unwrap_or_default());
    }
    result
}

fn watcher(spec: &Spec, cell: &Arc<BotCell>, hub: &Arc<Hub>) -> impl FnMut(&Phase, &World) + 'static {
    let (cell, hub, user) = (cell.clone(), hub.clone(), spec.creds.user.clone());
    let (stall_after, on_stall) = (spec.stall_after, spec.on_stall);
    let mut status_at = Instant::now() + STATUS_EVERY;
    move |phase, world| {
        let now = Instant::now();
        let mut board = cell.board();
        if *phase == Phase::Ready {
            board.sample_gold(world);
        }
        let stalled = *phase == Phase::Ready
            && board.run == Run::Running
            && !board.paused
            && !board.stalled
            && now.saturating_duration_since(board.last_progress) >= stall_after;
        board.stalled |= stalled;
        if hub.watched() {
            board.snapshot = Some(board.snapshot(&cell.name, &user, phase, world, now));
        }
        drop(board);
        if stalled {
            hub.say(&cell, "aqx", format!("warn: stalled, no progress for {}", span(stall_after.as_secs())));
            let _ = match on_stall {
                OnStall::Warn => Ok(true),
                OnStall::Restart => cell.send(Command::Restart),
                OnStall::Relogin => cell.send(Command::Relogin),
            };
        }
        if *phase == Phase::Ready && now >= status_at {
            status_at = now + STATUS_EVERY;
            hub.status(&cell, world);
        }
    }
}

impl Bot {
    fn begin(&mut self, script: Option<Script>) {
        self.cell.board().reset_stats();
        self.wanted = script;
    }

    fn publish(&self, session: &Session, detail: String) {
        let mut board = self.cell.board();
        let mut last = board.snapshot(&self.cell.name, &self.spec.creds.user, &session.phase, &session.world, Instant::now());
        last.detail = detail;
        board.snapshot = Some(last);
    }

    fn say(&self, text: impl Into<String>) {
        self.hub.say(&self.cell, "aqx", text);
    }

    fn drive(&mut self, session: &mut Session) -> Result<(), String> {
        let mut next = Next::Login;
        loop {
            next = match next {
                Next::Login => self.login(session),
                Next::Idle => self.idle(session),
                Next::Run(script) => self.run_script(session, script),
                Next::Quit(result) => return result,
            };
        }
    }

    fn login(&mut self, session: &mut Session) -> Next {
        self.say(format!("logging in as {}", self.spec.creds.user));
        match session.login(self.spec.server.as_deref()) {
            Ok(()) => {
                self.ever_ready = true;
                let now = Instant::now();
                let mut board = self.cell.board();
                board.server = session.server().map(|s| s.name.clone()).unwrap_or_default();
                board.ready_since = Some(now);
                board.progressed(now);
                drop(board);
                match self.wanted.clone() {
                    Some(script) => Next::Run(script),
                    None => Next::Idle,
                }
            }
            Err(LoginError::Halted(Halt::Stopped)) => self.command(session),
            Err(e) if e.retryable() && (self.ever_ready || !self.spec.exit_when_done) => self.retry(session, e.to_string()),
            Err(e) => Next::Quit(Err(e.to_string())),
        }
    }

    fn retry(&mut self, session: &mut Session, reason: String) -> Next {
        let delay = self.spec.relogin.after(session.connected_for());
        self.say(format!("{reason}; logging in again in {}", span(delay.as_secs())));
        match session.wait_to_relogin(reason, Instant::now() + delay) {
            Ok(()) => Next::Login,
            Err(Halt::Stopped) => self.command(session),
            Err(halt) => Next::Quit(Err(format!("{halt:?}"))),
        }
    }

    fn lost(&mut self, session: &mut Session, why: Disconnect) -> Next {
        if why.retryable() { self.retry(session, why.to_string()) } else { Next::Quit(Err(why.to_string())) }
    }

    fn idle(&mut self, session: &mut Session) -> Next {
        match session.pump_until(None, |_| None::<()>) {
            Err(Halt::Stopped) => self.command(session),
            Err(Halt::Closed(why)) => self.lost(session, why),
            _ => Next::Idle,
        }
    }

    fn run_script(&mut self, session: &mut Session, script: Script) -> Next {
        self.wanted = Some(script.clone());
        self.last = Some(script.clone());
        let shown = script.path.display().to_string();
        self.cell.board().start_script(shown.clone(), script.args.clone(), Instant::now());
        self.say(format!("running {shown}"));
        let started = Instant::now();
        match aqx_script::run(session, &script.path, &script.args, self.spec.room) {
            Ok(Outcome::Finished) => self.ended(session, Run::Done, "the script finished".into()),
            Ok(Outcome::Stopped(reason)) => {
                self.ended(session, Run::Done, format!("the script stopped: {}", reason.as_deref().unwrap_or("no reason given")))
            }
            Err(ScriptError::Halted(Halt::Stopped)) => self.command(session),
            Err(ScriptError::Halted(Halt::Closed(why))) => {
                if why.retryable() {
                    self.say("the script starts again once the bot is back");
                }
                self.lost(session, why)
            }
            Err(e) => {
                self.cell.board().run = Run::Failed;
                let failure = e.to_string();
                self.hub.say(&self.cell, "script", format!("error: {failure}"));
                if self.spec.restart == Restart::OnError {
                    let delay = self.spec.restarts.after(started.elapsed());
                    self.say(format!("restarting the script in {}", span(delay.as_secs())));
                    return match session.pump_until(Some(Instant::now() + delay), |_| None::<()>) {
                        Err(Halt::Timeout) => Next::Run(script),
                        Err(Halt::Stopped) => self.command(session),
                        Err(Halt::Closed(why)) => self.lost(session, why),
                        _ => Next::Run(script),
                    };
                }
                self.wanted = None;
                if self.spec.exit_when_done { Next::Quit(Err(failure)) } else { self.go_idle(session) }
            }
        }
    }

    fn ended(&mut self, session: &mut Session, run: Run, text: String) -> Next {
        self.wanted = None;
        self.cell.board().run = run;
        self.say(text);
        if self.spec.exit_when_done { Next::Quit(Ok(())) } else { self.go_idle(session) }
    }

    fn go_idle(&mut self, session: &mut Session) -> Next {
        let fighting = session.phase == Phase::Ready
            && !session.paused()
            && session.world.me.as_ref().is_some_and(|m| m.vitals.state == State::Combat);
        if fighting && let Err(e) = session.exit_combat() {
            self.say(format!("couldn't leave combat: {e}"));
        }
        Next::Idle
    }

    fn command(&mut self, session: &mut Session) -> Next {
        session.clear_stop();
        let next = match self.cell.take_command() {
            Some(Command::Run { script, args }) => {
                let script = Script { path: script, args };
                self.begin(Some(script.clone()));
                Next::Run(script)
            }
            Some(Command::Restart) => match self.last.clone() {
                Some(script) => Next::Run(script),
                None => {
                    self.say("there is no script to restart");
                    Next::Idle
                }
            },
            Some(Command::Stop) => {
                self.wanted = None;
                let mut board = self.cell.board();
                let running = board.run == Run::Running;
                if running {
                    board.run = Run::Stopped;
                }
                drop(board);
                if running {
                    self.say("stopped the script");
                }
                if self.spec.exit_when_done && running { Next::Quit(Ok(())) } else { self.go_idle(session) }
            }
            Some(Command::Relogin) => {
                self.say("logging out to log in again");
                session.logout();
                Next::Login
            }
            Some(Command::Logout) | None => Next::Quit(Ok(())),
            Some(Command::Login | Command::Pause | Command::Resume) => Next::Idle,
        };
        if self.cell.has_commands() {
            session.stop_handle().stop();
        }
        if session.phase == Phase::Ready {
            return next;
        }
        match next {
            Next::Run(script) => {
                self.wanted = Some(script);
                Next::Login
            }
            Next::Idle => Next::Login,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use aqx_core::Control;

    use super::*;
    use crate::fake::{FARM, Servers, Wire};
    use crate::hub::{Board, BotCell, Hub, Print};

    struct Running {
        cell: Arc<BotCell>,
        thread: JoinHandle<Result<(), String>>,
    }

    fn script(name: &str, source: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aqx-bot-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.ts"));
        std::fs::write(&path, source).unwrap();
        path
    }

    fn launch(servers: &Servers, path: Option<PathBuf>, tweak: impl FnOnce(&mut Spec)) -> Running {
        let quick = || Backoff::new(Duration::from_millis(100), Duration::from_millis(400));
        let mut spec = Spec {
            creds: servers.credentials(),
            server: Some("Fake".into()),
            version: "5.31".into(),
            script: path.map(|path| Script { path, args: Vec::new() }),
            room: None,
            restart: Restart::Never,
            stall_after: Duration::from_secs(300),
            on_stall: OnStall::Warn,
            exit_when_done: true,
            relogin: quick(),
            restarts: quick(),
        };
        tweak(&mut spec);
        let control = Control::new().unwrap();
        let cell = Arc::new(BotCell::local("main", control.stop_handle()));
        let hub = Arc::new(Hub::new("test", vec![cell.clone()], Print::Quiet));
        cell.send(Command::Login).unwrap();
        let thread = {
            let cell = cell.clone();
            thread::spawn(move || run(spec, None, control, cell, hub))
        };
        Running { cell, thread }
    }

    impl Running {
        #[track_caller]
        fn finish(self) -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.cell.offline() {
                assert!(Instant::now() < deadline, "never went offline: {:#?}", log(&self.cell));
                thread::sleep(Duration::from_millis(20));
            }
            self.cell.close();
            self.thread.join().unwrap()
        }
    }

    fn log(cell: &BotCell) -> Vec<String> {
        cell.board().log.iter().map(|l| l.text.clone()).collect()
    }

    fn count(cell: &BotCell, prefix: &str) -> usize {
        log(cell).iter().filter(|l| l.starts_with(prefix)).count()
    }

    #[track_caller]
    fn wait_for(cell: &BotCell, what: &str, done: impl Fn(&Board) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&cell.board()) {
            assert!(Instant::now() < deadline, "never {what}: {:#?}", log(cell));
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_dropped_connection_logs_in_again_and_restarts_the_script() {
        let mut dropped = false;
        let servers = Servers::start(FARM, move |msg: &str, wire: &mut Wire| {
            if msg.contains("%moveToCell%5%r1%") && !dropped {
                dropped = true;
                wire.hang_up();
            }
        });
        let path = script("restart-after-drop", r#"import { map, wait } from "aqx"; map.jump("r1"); wait.ms(300); console.log("made it");"#);
        let bot = launch(&servers, Some(path), |_| {});
        let cell = bot.cell.clone();
        let result = bot.finish();
        assert!(result.is_ok(), "{result:?}: {:#?}", log(&cell));
        assert_eq!(servers.logins.load(Ordering::SeqCst), 2);
        assert_eq!(count(&cell, "running "), 2, "{:#?}", log(&cell));
        assert_eq!(count(&cell, "made it"), 1);
        assert!(log(&cell).iter().any(|l| l.contains("logging in again in")));
        servers.stop();
    }

    #[test]
    fn logging_in_elsewhere_takes_the_bot_offline() {
        let servers = Servers::start(FARM, |msg: &str, wire: &mut Wire| {
            if msg.contains("%moveToCell%5%r1%") {
                wire.send("%xt%multiLoginWarning%-1%");
            }
        });
        let path = script("elsewhere", r#"import { map, wait } from "aqx"; map.jump("r1"); wait.ms(3000);"#);
        let bot = launch(&servers, Some(path), |_| {});
        let cell = bot.cell.clone();
        let result = bot.finish();
        assert!(matches!(&result, Err(e) if e.contains("logged in somewhere else")), "{result:?}");
        assert_eq!(servers.logins.load(Ordering::SeqCst), 1);
        let board = cell.board();
        let last = board.snapshot.as_ref().expect("a final snapshot though nobody watched");
        assert_eq!((last.state(), last.detail.as_str()), ("OFFLINE", "this account logged in somewhere else"));
        servers.stop();
    }

    #[test]
    fn commands_run_pause_stop_and_log_out_an_idle_bot() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |s| s.exit_when_done = false);
        wait_for(&bot.cell, "logged in", |b| b.ready_since.is_some());
        let path = script("forever", r#"import { wait } from "aqx"; wait.until(() => false);"#);
        bot.cell.send(Command::Run { script: path, args: Vec::new() }).unwrap();
        wait_for(&bot.cell, "ran the script", |b| b.run == Run::Running);
        bot.cell.send(Command::Pause).unwrap();
        assert!(bot.cell.board().paused);
        bot.cell.send(Command::Stop).unwrap();
        wait_for(&bot.cell, "stopped the script", |b| b.run == Run::Stopped);
        bot.cell.send(Command::Logout).unwrap();
        let cell = bot.cell.clone();
        assert!(bot.finish().is_ok());
        assert!(cell.send(Command::Stop).is_err(), "an offline bot takes no commands");
        servers.stop();
    }

    #[test]
    fn a_logged_out_bot_logs_in_again() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |s| s.exit_when_done = false);
        wait_for(&bot.cell, "logged in", |b| b.ready_since.is_some());
        let first = bot.cell.board().ready_since;
        bot.cell.send(Command::Logout).unwrap();
        wait_for(&bot.cell, "went offline", |b| !b.online);
        let offline = bot.cell.board().snapshot.clone().expect("a snapshot once offline");
        assert_eq!((offline.state(), &offline.game, offline.up_secs), ("OFFLINE", &None, 0));
        bot.cell.send(Command::Login).unwrap();
        wait_for(&bot.cell, "logged in again", |b| b.ready_since > first);
        assert_eq!(servers.logins.load(Ordering::SeqCst), 2);
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn running_a_script_on_an_offline_bot_logs_it_in() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |s| s.exit_when_done = false);
        wait_for(&bot.cell, "logged in", |b| b.ready_since.is_some());
        bot.cell.send(Command::Logout).unwrap();
        wait_for(&bot.cell, "went offline", |b| !b.online);
        let path = script("after-offline", r#"console.log("back online");"#);
        bot.cell.send(Command::Run { script: path, args: Vec::new() }).unwrap();
        wait_for(&bot.cell, "ran the script", |b| b.log.iter().any(|l| l.text == "back online"));
        assert_eq!(servers.logins.load(Ordering::SeqCst), 2);
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn a_script_error_names_its_path_once() {
        let missing = std::env::temp_dir().join("aqx-bot-tests-no-such-script.ts");
        for path in [missing, script("broken", "let x = ;")] {
            let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
            let bot = launch(&servers, Some(path.clone()), |_| {});
            let cell = bot.cell.clone();
            let result = bot.finish();
            let shown = path.display().to_string();
            assert!(matches!(&result, Err(e) if e.matches(&shown).count() == 1), "{result:?}");
            let error = log(&cell).into_iter().find(|l| l.starts_with("error: ")).expect("an error line");
            assert_eq!(error.matches(&shown).count(), 1, "{error}");
            servers.stop();
        }
    }

    #[test]
    fn logging_in_right_after_logging_out_comes_back_online() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |s| s.exit_when_done = false);
        wait_for(&bot.cell, "logged in", |b| b.ready_since.is_some());
        let first = bot.cell.board().ready_since;
        bot.cell.send(Command::Logout).unwrap();
        bot.cell.send(Command::Login).unwrap();
        wait_for(&bot.cell, "logged in again", |b| b.ready_since > first);
        assert_eq!(servers.logins.load(Ordering::SeqCst), 2);
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn a_stalled_script_is_restarted() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let path = script("stalls", r#"import { wait } from "aqx"; wait.ms(60000);"#);
        let bot = launch(&servers, Some(path), |s| {
            s.stall_after = Duration::from_millis(300);
            s.on_stall = OnStall::Restart;
        });
        wait_for(&bot.cell, "restarted", |b| b.log.iter().filter(|l| l.text.starts_with("running ")).count() >= 2);
        assert!(log(&bot.cell).iter().any(|l| l.contains("stalled")));
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn a_failing_script_restarts_on_error() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let path = script("fails", r#"throw new Error("boom");"#);
        let bot = launch(&servers, Some(path), |s| s.restart = Restart::OnError);
        wait_for(&bot.cell, "retried", |b| b.log.iter().filter(|l| l.text.starts_with("running ")).count() >= 2);
        assert!(log(&bot.cell).iter().any(|l| l.contains("Error: boom")));
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn stopping_aqx_run_without_a_script_keeps_it_online() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |_| {});
        wait_for(&bot.cell, "logged in", |b| b.ready_since.is_some());
        bot.cell.send(Command::Stop).unwrap();
        thread::sleep(Duration::from_millis(300));
        assert!(!bot.cell.offline(), "{:#?}", log(&bot.cell));
        bot.cell.send(Command::Logout).unwrap();
        assert!(bot.finish().is_ok());
        servers.stop();
    }

    #[test]
    fn aqx_run_gives_up_when_the_first_login_fails() {
        let servers = Servers::start(FARM, |_: &str, _: &mut Wire| {});
        let bot = launch(&servers, None, |s| s.creds.api = "http://127.0.0.1:1".into());
        let result = bot.finish();
        assert!(matches!(&result, Err(e) if e.contains("login request failed")), "{result:?}");
        servers.stop();
    }

    #[test]
    fn waiting_to_log_in_again_is_not_a_stall() {
        let mut dropped = false;
        let servers = Servers::start(FARM, move |msg: &str, wire: &mut Wire| {
            if msg.contains("%moveToCell%5%r1%") && !dropped {
                dropped = true;
                wire.hang_up();
            }
        });
        let path = script("no-stall", r#"import { map, wait } from "aqx"; map.jump("r1"); wait.ms(100);"#);
        let bot = launch(&servers, Some(path), |s| {
            s.stall_after = Duration::from_millis(1000);
            s.on_stall = OnStall::Restart;
            s.relogin = Backoff::new(Duration::from_millis(2500), Duration::from_millis(2500));
        });
        let cell = bot.cell.clone();
        assert!(bot.finish().is_ok());
        assert!(!log(&cell).iter().any(|l| l.contains("stalled")), "{:#?}", log(&cell));
        assert_eq!(count(&cell, "running "), 2);
        servers.stop();
    }
}
