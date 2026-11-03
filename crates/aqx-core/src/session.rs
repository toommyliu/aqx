use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aqx_protocol::{AreaId, Decoder, Framer, Message, MonMapId, Out, QuestId};
use mio::net::TcpStream;
use mio::{Events, Interest, Token};

use crate::api::{self, ApiError, Server, ServerError};
use crate::control::{Control, StopHandle};
use crate::event::Event;
use crate::pacer::{Kind, Pacer};
use crate::record::Recorder;
use crate::world::{self, Monster, State, World};

pub const CLIENT_VERSION: &str = "5.31";

const SOCKET: Token = Token(0);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
const VERSION_RETRIES: usize = 3;
const RESPAWN_DELAY: Duration = Duration::from_secs(11);
const RESPAWN_RETRY: Duration = Duration::from_secs(10);
const AGGRO_EVERY: Duration = Duration::from_millis(500);
const SLOW_DOWN: [&str; 3] = ["slow down", "too soon", "spamming"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Handshaking,
    LoggingIn,
    Loading,
    Ready,
    Closed(Disconnect),
    Waiting { reason: String, until: Instant },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disconnect {
    ClientVersion(String),
    Rejected(String),
    LoggedOut,
    LoggedInElsewhere,
    ConnectionLost(String),
    Protocol(String),
}

impl Disconnect {
    pub fn retryable(&self) -> bool {
        !matches!(self, Disconnect::LoggedInElsewhere)
    }
}

impl std::fmt::Display for Disconnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Disconnect::ClientVersion(v) => write!(f, "the server requires client version {v}"),
            Disconnect::Rejected(text) => write!(f, "login rejected: {text}"),
            Disconnect::LoggedOut => write!(f, "logged out"),
            Disconnect::LoggedInElsewhere => write!(f, "this account logged in somewhere else"),
            Disconnect::ConnectionLost(e) => write!(f, "connection lost: {e}"),
            Disconnect::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

#[derive(Debug)]
pub enum Halt {
    Timeout,
    Stopped,
    Interrupted,
    GoalMet,
    Closed(Disconnect),
}

#[derive(Debug)]
pub enum LoginError {
    Api(ApiError),
    Server(ServerError),
    Connect(String),
    Closed(Disconnect),
    NotReady(String),
    Halted(Halt),
}

impl LoginError {
    pub fn retryable(&self) -> bool {
        match self {
            LoginError::Api(ApiError::Rejected(_)) | LoginError::Halted(_) => false,
            LoginError::Server(e) => !e.lasting,
            LoginError::Closed(why) => why.retryable(),
            _ => true,
        }
    }
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Api(e) => write!(f, "{e}"),
            LoginError::Server(e) => f.write_str(&e.message),
            LoginError::Connect(e) => f.write_str(e),
            LoginError::Closed(why) => write!(f, "{why}"),
            LoginError::NotReady(phase) => write!(f, "not ready after {} s, stuck in {phase}", READY_TIMEOUT.as_secs()),
            LoginError::Halted(halt) => write!(f, "{halt:?}"),
        }
    }
}

#[derive(Clone)]
pub struct Credentials {
    pub api: String,
    pub user: String,
    pub password: String,
}

pub type Sink = Rc<dyn Fn(&Event)>;
type WatchFn = Box<dyn FnMut(&Phase, &World)>;
pub type Goal = Box<dyn Fn(&World) -> bool>;
pub type Hook = Box<dyn FnMut(&mut Session)>;
pub type Target = Box<dyn Fn(&World, &Monster) -> bool>;

struct Watcher {
    every: Duration,
    next: Instant,
    f: WatchFn,
}

struct Aggro {
    target: Target,
    next: Instant,
}

pub struct Session {
    control: Control,
    events: Events,
    stream: Option<TcpStream>,
    framer: Framer,
    decoder: Decoder,
    outbox: Vec<u8>,
    recorder: Option<Recorder>,
    pub(crate) sink: Sink,
    creds: Credentials,
    version: String,
    token: String,
    server: Option<Server>,
    ready_at: Option<Instant>,
    pub(crate) refusals: HashMap<String, String>,
    inventory_requested: bool,
    pacer: Pacer,
    counter: u32,
    respawn_at: Option<Instant>,
    respawn_sent: bool,
    watcher: Option<Watcher>,
    hook: Option<Hook>,
    pub(crate) goal: Option<Goal>,
    script_events: Option<VecDeque<Event>>,
    events_queued: u64,
    pub respawn_to: Option<(String, String)>,
    died_in: Option<(String, String)>,
    pub(crate) joined: Option<AreaId>,
    pub(crate) quest_outcomes: HashMap<QuestId, Result<(), String>>,
    pub(crate) slowed: u32,
    paused: bool,
    aggro: Option<Aggro>,
    alarm: Option<Instant>,
    packets: HashSet<String>,
    all_packets: bool,
    pub world: World,
    pub phase: Phase,
}

impl Session {
    pub fn new(control: Control, creds: Credentials, version: &str, recorder: Option<Recorder>, sink: Sink) -> Session {
        Session {
            control,
            events: Events::with_capacity(16),
            stream: None,
            framer: Framer::default(),
            decoder: Decoder::default(),
            outbox: Vec::new(),
            recorder,
            sink,
            creds,
            version: version.into(),
            token: String::new(),
            server: None,
            ready_at: None,
            refusals: HashMap::new(),
            inventory_requested: false,
            pacer: Pacer::new(Instant::now()),
            counter: 0,
            respawn_at: None,
            respawn_sent: false,
            watcher: None,
            hook: None,
            goal: None,
            script_events: None,
            events_queued: 0,
            respawn_to: None,
            died_in: None,
            joined: None,
            quest_outcomes: HashMap::new(),
            slowed: 0,
            paused: false,
            aggro: None,
            alarm: None,
            packets: HashSet::new(),
            all_packets: false,
            world: World::default(),
            phase: Phase::Connecting,
        }
    }

    pub fn login(&mut self, server: Option<&str>) -> Result<(), LoginError> {
        let (hook, goal) = (self.hook.take(), self.goal.take());
        let result = self.log_in(server);
        self.hook = hook;
        self.goal = goal;
        if let Err(e) = &result {
            self.close(Disconnect::ConnectionLost(e.to_string()));
        }
        result
    }

    fn log_in(&mut self, server: Option<&str>) -> Result<(), LoginError> {
        self.hang_up();
        self.ready_at = None;
        for _ in 0..VERSION_RETRIES {
            self.phase = Phase::Connecting;
            let Credentials { api, user, password } = self.creds.clone();
            let account = match self.off_thread(move || api::login(&api, &user, &password), LOGIN_TIMEOUT) {
                Ok(reply) => reply.map_err(LoginError::Api)?,
                Err(Halt::Timeout) => return Err(LoginError::Api(ApiError::Http("the login request timed out".into()))),
                Err(halt) => return Err(LoginError::Halted(halt)),
            };
            let chosen = api::pick_server(&account.servers, server, account.member).map_err(LoginError::Server)?.clone();
            self.token = account.token;
            self.open(&chosen)?;
            let outcome = self.pump_until(Some(Instant::now() + READY_TIMEOUT), |s| match &s.phase {
                Phase::Ready => Some(Ok(())),
                Phase::Closed(why) => Some(Err(why.clone())),
                _ => None,
            });
            match outcome {
                Ok(Ok(())) => {
                    self.ready_at = Some(Instant::now());
                    return Ok(());
                }
                Ok(Err(Disconnect::ClientVersion(wanted))) if wanted != self.version => self.version = wanted,
                Ok(Err(why)) => return Err(LoginError::Closed(why)),
                Err(Halt::Timeout) => {
                    let stuck = format!("{:?}", self.phase);
                    self.close(Disconnect::ConnectionLost("the character never loaded".into()));
                    return Err(LoginError::NotReady(stuck));
                }
                Err(halt) => return Err(LoginError::Halted(halt)),
            }
        }
        Err(LoginError::Closed(Disconnect::ClientVersion(self.version.clone())))
    }

    fn open(&mut self, server: &Server) -> Result<(), LoginError> {
        let unreachable = |e: io::Error| LoginError::Connect(format!("can't reach {}: {e}", server.name));
        let addr = (server.host.as_str(), server.port)
            .to_socket_addrs()
            .map_err(unreachable)?
            .next()
            .ok_or_else(|| LoginError::Connect(format!("{} has no address", server.host)))?;
        let std_stream = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(unreachable)?;
        std_stream.set_nodelay(true).map_err(unreachable)?;
        std_stream.set_nonblocking(true).map_err(unreachable)?;
        let mut stream = TcpStream::from_std(std_stream);
        self.control.poll.registry().register(&mut stream, SOCKET, Interest::READABLE | Interest::WRITABLE).map_err(unreachable)?;
        self.stream = Some(stream);
        self.framer = Framer::default();
        self.decoder = Decoder::default();
        self.outbox.clear();
        self.pacer = Pacer::new(Instant::now());
        self.counter = 0;
        self.respawn_at = None;
        self.respawn_sent = false;
        self.died_in = None;
        self.joined = None;
        self.quest_outcomes.clear();
        self.refusals.clear();
        self.inventory_requested = false;
        self.ready_at = None;
        self.world = World { rev: self.world.rev + 1, ..World::default() };
        self.server = Some(server.clone());
        self.phase = Phase::Handshaking;
        self.publish(Event::Connecting { server: server.name.clone(), version: self.version.clone() });
        self.send(Out::VerChk);
        Ok(())
    }

    pub fn logout(&mut self) {
        self.control.clear_stop();
        self.hang_up();
    }

    fn hang_up(&mut self) {
        if self.stream.is_some() && !matches!(self.phase, Phase::Closed(_)) {
            self.send(Out::Logout);
            let _ = self.pump_until(Some(Instant::now() + LOGOUT_TIMEOUT), |s| matches!(s.phase, Phase::Closed(_)).then_some(()));
        }
        self.close(Disconnect::LoggedOut);
    }

    pub fn wait_to_relogin(&mut self, reason: impl Into<String>, until: Instant) -> Result<(), Halt> {
        let reason = reason.into();
        self.close(Disconnect::ConnectionLost(reason.clone()));
        self.phase = Phase::Waiting { reason, until };
        match self.pump_until(Some(until), |_| None::<()>) {
            Err(Halt::Timeout) => Ok(()),
            other => other,
        }
    }

    pub fn connected_for(&self) -> Duration {
        self.ready_at.map_or(Duration::ZERO, |at| at.elapsed())
    }

    pub fn server(&self) -> Option<&Server> {
        self.server.as_ref()
    }

    pub fn user(&self) -> &str {
        &self.creds.user
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn watch(&mut self, every: Duration, f: impl FnMut(&Phase, &World) + 'static) {
        self.watcher = Some(Watcher { every, next: Instant::now(), f: Box::new(f) });
    }

    pub fn pump_until<T>(&mut self, deadline: Option<Instant>, mut done: impl FnMut(&Session) -> Option<T>) -> Result<T, Halt> {
        loop {
            if let Some(v) = done(self) {
                return Ok(v);
            }
            if let Phase::Closed(why) = &self.phase {
                let why = why.clone();
                self.fire_watcher(Instant::now(), true);
                return Err(Halt::Closed(why));
            }
            if self.control.stop_requested() {
                return Err(Halt::Stopped);
            }
            if self.control.take_interrupt() {
                return Err(Halt::Interrupted);
            }
            if self.goal_met() {
                return Err(Halt::GoalMet);
            }
            if self.run_posted() {
                continue;
            }
            let now = Instant::now();
            self.run_timers(now);
            if deadline.is_some_and(|d| now >= d) {
                return Err(Halt::Timeout);
            }
            if self.run_hook() {
                continue;
            }
            let wake = [deadline, self.respawn_at, self.watcher.as_ref().map(|w| w.next), self.aggro.as_ref().map(|a| a.next), self.alarm]
                .into_iter()
                .flatten()
                .min();
            let timeout = wake.map(|w| w.saturating_duration_since(now));
            match self.control.poll.poll(&mut self.events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => self.close(Disconnect::ConnectionLost(e.to_string())),
            }
            self.read_socket();
            self.flush();
        }
    }

    pub fn set_hook(&mut self, hook: Option<Hook>) {
        if hook.is_none() {
            self.aggro = None;
            self.alarm = None;
            self.packets.clear();
            self.all_packets = false;
        }
        self.script_events = hook.as_ref().map(|_| VecDeque::new());
        self.hook = hook;
    }

    pub(crate) fn goal_met(&self) -> bool {
        self.goal.as_ref().is_some_and(|met| met(&self.world))
    }

    pub fn run_handlers(&mut self) {
        self.run_hook();
    }

    pub fn events_queued(&self) -> u64 {
        self.events_queued
    }

    pub fn take_script_events(&mut self) -> Vec<Event> {
        self.script_events.as_mut().map(|q| q.drain(..).collect()).unwrap_or_default()
    }

    fn run_hook(&mut self) -> bool {
        if self.script_events.as_ref().is_none_or(|q| q.is_empty()) {
            return false;
        }
        let Some(mut hook) = self.hook.take() else { return false };
        let goal = self.goal.take();
        hook(self);
        self.goal = goal;
        self.hook = Some(hook);
        true
    }

    fn run_posted(&mut self) -> bool {
        let posted = self.control.take_posted();
        let any = !posted.is_empty();
        for f in posted {
            f(self);
        }
        any
    }

    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }

    pub fn stop_handle(&self) -> StopHandle {
        self.control.stop_handle()
    }

    pub fn clear_stop(&self) {
        self.control.clear_stop();
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.world.rev += 1;
    }

    pub fn set_aggro(&mut self, target: Option<Target>) {
        self.aggro = target.map(|target| Aggro { target, next: Instant::now() });
    }

    pub fn set_alarm(&mut self, at: Option<Instant>) {
        self.alarm = at;
    }

    pub fn listen(&mut self, cmd: &str, on: bool) {
        if on {
            self.packets.insert(cmd.into());
        } else {
            self.packets.remove(cmd);
        }
    }

    pub fn listen_all(&mut self, on: bool) {
        self.all_packets = on;
    }

    pub(crate) fn idle_until(&mut self, at: Instant) -> Result<(), Halt> {
        match self.pump_until(Some(at), |_| None::<()>) {
            Err(Halt::Timeout) => Ok(()),
            other => other,
        }
    }

    pub(crate) fn send_paced(&mut self, out: Out, kind: Kind) -> Result<(), Halt> {
        let ready = self.pacer.ready_at(kind, Instant::now());
        let hook = self.hook.take();
        let waited = self.idle_until(ready);
        self.hook = hook;
        waited?;
        self.pacer.sent(kind, Instant::now());
        self.send(out);
        Ok(())
    }

    pub(crate) fn pacer_ready_at(&self, kind: Kind, now: Instant) -> Instant {
        self.pacer.ready_at(kind, now)
    }

    pub(crate) fn api(&self) -> &str {
        &self.creds.api
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn off_thread<T: Send + 'static>(&mut self, job: impl FnOnce() -> T + Send + 'static, timeout: Duration) -> Result<T, Halt> {
        let (tx, rx) = mpsc::channel();
        let waker = self.control.waker();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            let _ = waker.wake();
        });
        self.pump_until(Some(Instant::now() + timeout), |_| rx.try_recv().ok())
    }

    pub(crate) fn next_counter(&mut self) -> u32 {
        self.counter += 1;
        self.counter - 1
    }

    fn run_timers(&mut self, now: Instant) {
        if self.respawn_at.is_some_and(|at| now >= at) {
            match (&self.world.me, &self.world.area) {
                (Some(me), Some(area)) if me.vitals.state == State::Dead => {
                    let (room, eid) = (area.id, me.eid);
                    self.send(Out::Respawn { room, eid });
                    self.respawn_sent = true;
                    self.respawn_at = Some(now + RESPAWN_RETRY);
                }
                (Some(me), _) if me.vitals.state != State::Dead => self.respawn_at = None,
                _ => self.respawn_at = Some(now + Duration::from_secs(1)),
            }
        }
        if self.aggro.as_ref().is_some_and(|a| now >= a.next) {
            if let Some(aggro) = &mut self.aggro {
                aggro.next = now + AGGRO_EVERY;
            }
            self.pull(now);
        }
        if self.alarm.is_some_and(|at| now >= at) {
            self.alarm = None;
            self.queue(Event::Alarm);
        }
        self.fire_watcher(now, false);
    }

    fn pull(&mut self, now: Instant) {
        if self.phase != Phase::Ready || self.paused {
            return;
        }
        let (Some(aggro), Some(me), Some(area)) = (&self.aggro, &self.world.me, &self.world.area) else { return };
        if me.vitals.state == State::Dead {
            return;
        }
        let ids: Vec<MonMapId> =
            area.monsters.iter().filter(|m| m.cell == me.cell && m.alive() && (aggro.target)(&self.world, m)).map(|m| m.map_id).collect();
        if !ids.is_empty() {
            let room = area.id;
            self.pacer.sent(Kind::Free, now);
            self.send(Out::Aggro { room, monsters: &ids });
        }
    }

    fn fire_watcher(&mut self, now: Instant, force: bool) {
        if let Some(mut watcher) = self.watcher.take() {
            if force || now >= watcher.next {
                (watcher.f)(&self.phase, &self.world);
                watcher.next = now + watcher.every;
            }
            self.watcher = Some(watcher);
        }
    }

    pub(crate) fn send(&mut self, out: Out) {
        if self.stream.is_none() {
            return;
        }
        let start = self.outbox.len();
        out.encode(&mut self.outbox);
        let sent = String::from_utf8_lossy(&self.outbox[start..self.outbox.len() - 1]).into_owned();
        self.record("out", &sent);
        self.flush();
    }

    fn flush(&mut self) {
        while !self.outbox.is_empty() {
            let Some(stream) = &mut self.stream else { return self.outbox.clear() };
            match stream.write(&self.outbox) {
                Ok(0) => self.close(Disconnect::ConnectionLost("socket closed while writing".into())),
                Ok(n) => {
                    self.outbox.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn read_socket(&mut self) {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let Some(stream) = &mut self.stream else { return };
            match stream.read(&mut buf) {
                Ok(0) => return self.close(Disconnect::ConnectionLost("server closed the connection".into())),
                Ok(n) => self.receive_bytes(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn receive_bytes(&mut self, data: &[u8]) {
        let mut framer = std::mem::take(&mut self.framer);
        let mut decoder = std::mem::take(&mut self.decoder);
        let fed = framer.feed(data, |frame| match decoder.decode(frame) {
            Ok(text) => self.receive(text),
            Err(e) => (self.sink)(&Event::Unreadable { what: format!("{e:?}") }),
        });
        self.framer = framer;
        self.decoder = decoder;
        if let Err(e) = fed {
            self.close(Disconnect::Protocol(format!("{e:?}")));
        }
    }

    fn receive(&mut self, text: &str) {
        if self.stream.is_none() {
            return;
        }
        self.record("in", text);
        if self.phase != Phase::Ready
            && let Some(version) = required_version(text)
        {
            self.close(Disconnect::ClientVersion(version));
            return;
        }
        let msg = match Message::parse(text) {
            Ok(msg) => msg,
            Err(e) => return (self.sink)(&Event::Unreadable { what: format!("{e:?}: {text:.80}") }),
        };
        if let Message::Xml(xml) = &msg {
            match xml.action {
                Some("apiOK") if self.phase == Phase::Handshaking => {
                    let (user, version, token) = (self.creds.user.clone(), self.version.clone(), self.token.clone());
                    self.send(Out::Login { user: &user, version: &version, token: &token });
                    self.phase = Phase::LoggingIn;
                }
                Some("logout") => return self.close(Disconnect::LoggedOut),
                _ => {}
            }
        }
        for event in world::reduce(&mut self.world, &msg, Instant::now()) {
            (self.sink)(&event);
            self.queue(event.clone());
            self.react(&event);
        }
        if (self.all_packets || !self.packets.is_empty()) && self.script_events.is_some() {
            let cmd = match &msg {
                Message::Json(json) => json.cmd,
                Message::Xt(xt) => xt.cmd,
                Message::Xml(xml) => xml.action.unwrap_or_default(),
            };
            if self.all_packets || self.packets.contains(cmd) {
                self.queue(Event::Packet { text: text.into() });
            }
        }
    }

    pub(crate) fn publish(&mut self, event: Event) {
        (self.sink)(&event);
        self.queue(event);
    }

    fn queue(&mut self, event: Event) {
        if let Some(queue) = &mut self.script_events {
            self.events_queued += 1;
            queue.push_back(event);
        }
    }

    fn react(&mut self, event: &Event) {
        match event {
            Event::LoggedIn { .. } if self.phase == Phase::LoggingIn => {
                if let Some(me) = self.world.me.as_mut().filter(|me| me.user.is_empty()) {
                    me.user.clone_from(&self.creds.user);
                }
                self.send(Out::FirstJoin);
                self.send(Out::ClearIgnoreList);
                self.phase = Phase::Loading;
            }
            Event::LoginRejected { reason } => self.close(Disconnect::Rejected(reason.clone())),
            Event::LoggedInElsewhere => self.close(Disconnect::LoggedInElsewhere),
            Event::Joined { .. } => {
                self.on_joined();
                self.joined = self.world.area.as_ref().map(|a| a.id);
            }
            Event::PlayerEntered { eid } => {
                if let Some(room) = self.world.area.as_ref().map(|a| a.id) {
                    self.send(Out::RetrieveUserData { room, eid: *eid });
                }
            }
            Event::Died => {
                self.respawn_at = Some(Instant::now() + RESPAWN_DELAY);
                self.died_in = self.world.me.as_ref().map(|m| (m.cell.clone(), m.pad.clone()));
            }
            Event::QuestAccepted { id, .. } | Event::QuestCompleted { id, .. } => {
                self.quest_outcomes.insert(*id, Ok(()));
            }
            Event::QuestRejected { id, reason } => {
                self.quest_outcomes.insert(*id, Err(reason.clone()));
            }
            Event::Refused { action, reason } => {
                self.refusals.insert(action.clone(), reason.clone());
            }
            Event::Respawned => {
                self.respawn_at = None;
                if std::mem::take(&mut self.respawn_sent)
                    && let Some(room) = self.world.area.as_ref().map(|a| a.id)
                {
                    self.send(Out::MoveToCell { room, cell: "Enter", pad: "Spawn" });
                    self.moved_to("Enter", "Spawn");
                    let died_in = self.died_in.take();
                    if let Some((cell, pad)) = self.respawn_to.clone().or(died_in).filter(|(cell, _)| cell != "Enter") {
                        self.send(Out::MoveToCell { room, cell: &cell, pad: &pad });
                        self.moved_to(&cell, &pad);
                    }
                }
            }
            Event::Afk { afk: true } if self.phase == Phase::Ready => {
                self.pacer.sent(Kind::Free, Instant::now());
                self.send(Out::Afk(false));
            }
            Event::Warning { text } if SLOW_DOWN.iter().any(|w| text.to_lowercase().contains(w)) => {
                self.pacer.warned(Instant::now());
                self.slowed += 1;
            }
            Event::InventoryLoaded { items } if self.phase == Phase::Loading => {
                self.phase = Phase::Ready;
                (self.sink)(&Event::Ready { items: *items });
            }
            _ => {}
        }
    }

    pub(crate) fn moved_to(&mut self, cell: &str, pad: &str) {
        let moved = self.world.me.as_ref().is_some_and(|m| m.cell != cell || m.pad != pad);
        if let Some(me) = self.world.me.as_mut().filter(|_| moved) {
            me.cell = cell.into();
            me.pad = pad.into();
            (me.x, me.y) = (0, 0);
        }
        self.world.rev += 1;
        if moved {
            self.publish(Event::Moved { cell: cell.into(), pad: pad.into() });
        }
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return };
        let aggressive: Vec<MonMapId> = area.monsters.iter().filter(|m| m.cell == me.cell && m.aggressive && m.alive()).map(|m| m.map_id).collect();
        if !aggressive.is_empty() {
            let room = area.id;
            self.send(Out::Aggro { room, monsters: &aggressive });
        }
    }

    fn on_joined(&mut self) {
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return };
        let (eid, room) = (me.eid, area.id);
        let others: Vec<_> = area.players.iter().map(|p| p.eid).collect();
        self.send(Out::MoveToCell { room, cell: "Enter", pad: "Spawn" });
        self.moved_to("Enter", "Spawn");
        self.send(Out::RetrieveUserData { room, eid });
        if !others.is_empty() {
            self.send(Out::RetrieveUserDatas { room, others: &others });
        }
        if !self.inventory_requested {
            self.inventory_requested = true;
            self.send(Out::RetrieveInventory { room, eid });
        }
    }

    fn record(&mut self, dir: &str, text: &str) {
        if let Some(recorder) = &mut self.recorder
            && let Err(e) = recorder.write(dir, text)
        {
            self.recorder = None;
            (self.sink)(&Event::RecordingFailed { error: e.to_string() });
        }
    }

    fn close(&mut self, why: Disconnect) {
        if let Some(mut stream) = self.stream.take() {
            let _ = self.control.poll.registry().deregister(&mut stream);
        }
        self.outbox.clear();
        if !matches!(self.phase, Phase::Closed(_)) {
            self.phase = Phase::Closed(why);
        }
    }
}

fn required_version(text: &str) -> Option<String> {
    const MARK: &str = "requires AQW game version ";
    let rest = &text[text.find(MARK)? + MARK.len()..];
    let version = rest[..rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len())].trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_required_version_from_the_rejection_text() {
        let text = "%xt%loginResponse%-1%false%-1%hero%Your game client is an older version. Server requires AQW game version 5.31 or higher. Please close the AQW game window and try again.%";
        assert_eq!(required_version(text).as_deref(), Some("5.31"));
        assert_eq!(required_version("%xt%server%-1%You joined \"battleon-3\"%"), None);
    }
}
