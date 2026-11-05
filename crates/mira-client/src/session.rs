use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Waker};
use mira_protocol::{
    EntityId, Framer, JsonMessage, Message, QuestId, XmlMessage, XmlUser, XtMessage, packet,
};
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::api::{Account, Server, Token};
use crate::aura::{self, AuraChange, Target};
use crate::event::{Direction, Event, NoticeKind};
use crate::faction::{self, AddedFaction, Faction, FactionData};
use crate::inventory::{AddedItems, Inventory, LoadInventory, Picked, Sold, TurnIn, Worn};
use crate::player::{ClassChange, InitUserData, LevelUp, Me, Reward, SkillList, Stats};
use crate::quest::{self, AchievementUpdate, Quest, QuestList, QuestUpdate};
use crate::room::{MonsterUpdate, MoveToArea, PlayerUpdate, Room, Tick, Vitals};
use crate::world::World;

pub const DEFAULT_CLIENT_VERSION: &str = "5.31";

const SOCKET: mio::Token = mio::Token(0);
const WAKER: mio::Token = mio::Token(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_COOLDOWN: Duration = Duration::from_millis(3000);
const VERSION_RETRIES: usize = 3;
const REDACTED_KEYS: [&str; 2] = ["strEmail", "strIP"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Connecting { server: String, version: String },
    Handshaking,
    LoggingIn,
    Loading { step: Load },
    Ready,
    Closed { reason: Disconnect },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Load {
    Joining,
    Character,
    Inventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disconnect {
    ClientVersion(String),
    Rejected(String),
    LoggedOut,
    LoggedInElsewhere,
    InvalidSession,
    Ended,
    ConnectionLost(String),
    Protocol(String),
}

impl fmt::Display for Disconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Disconnect::ClientVersion(v) => write!(f, "the server requires client version {v}"),
            Disconnect::Rejected(text) => write!(f, "login rejected: {text}"),
            Disconnect::LoggedOut => f.write_str("logged out"),
            Disconnect::LoggedInElsewhere => f.write_str("this account logged in somewhere else"),
            Disconnect::InvalidSession => f.write_str("the server says the session is invalid"),
            Disconnect::Ended => f.write_str("the server ended the session"),
            Disconnect::ConnectionLost(e) => write!(f, "connection lost: {e}"),
            Disconnect::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

impl Serialize for Disconnect {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("the character has not loaded yet")]
    NotReady,
    #[error("the server refused: {0}")]
    Refused(String),
    #[error("the server did not answer within {} s", JOIN_TIMEOUT.as_secs())]
    NoReply,
    #[error("{0}")]
    Closed(Disconnect),
    #[error("the session stopped without closing")]
    Stopped,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub client_version: Option<String>,
    pub trace_packets: bool,
}

type Slot = Arc<Mutex<Arc<World>>>;
type Reply = mpsc::SyncSender<Result<(), ActionError>>;

fn latest(slot: &Slot) -> Arc<World> {
    Arc::clone(&slot.lock().unwrap_or_else(PoisonError::into_inner))
}

enum Request {
    Act(Action, Reply),
    Logout,
}

enum Action {
    Ready,
    Join { map: String },
    MoveToCell { cell: String, pad: String },
}

enum Op {
    Login {
        server: Server,
        version: String,
        attempts: usize,
        deadline: Instant,
    },
    Join {
        reply: Reply,
        stage: JoinStage,
    },
}

enum JoinStage {
    Cooling { map: String, until: Instant },
    Sent { joins: u64, deadline: Instant },
}

#[derive(Clone)]
pub struct Handle {
    requests: mpsc::Sender<Request>,
    waker: Arc<Waker>,
    world: Slot,
}

impl Handle {
    pub fn ready(&self) -> Pending {
        self.act(Action::Ready)
    }

    pub fn join(&self, map: &str) -> Pending {
        self.act(Action::Join {
            map: map.to_owned(),
        })
    }

    pub fn move_to_cell(&self, cell: &str, pad: &str) -> Pending {
        self.act(Action::MoveToCell {
            cell: cell.to_owned(),
            pad: pad.to_owned(),
        })
    }

    pub fn logout(&self) {
        self.request(Request::Logout);
    }

    pub fn snapshot(&self) -> Arc<World> {
        latest(&self.world)
    }

    fn act(&self, action: Action) -> Pending {
        let (reply, result) = mpsc::sync_channel(1);
        self.request(Request::Act(action, reply));
        Pending {
            result,
            world: Arc::clone(&self.world),
        }
    }

    fn request(&self, request: Request) {
        if self.requests.send(request).is_ok() {
            let _ = self.waker.wake();
        }
    }
}

pub struct Pending {
    result: mpsc::Receiver<Result<(), ActionError>>,
    world: Slot,
}

impl Pending {
    pub fn wait(self) -> Result<(), ActionError> {
        self.result
            .recv()
            .unwrap_or_else(|_| match &latest(&self.world).phase {
                Phase::Closed { reason } => Err(ActionError::Closed(reason.clone())),
                _ => Err(ActionError::Stopped),
            })
    }
}

pub fn spawn(
    account: &Account,
    server: Server,
    options: Options,
) -> io::Result<(Handle, mpsc::Receiver<Event>)> {
    let poll = Poll::new()?;
    let waker = Arc::new(Waker::new(poll.registry(), WAKER)?);
    let (requests_out, requests) = mpsc::channel();
    let (events, events_in) = mpsc::channel();
    let version = options
        .client_version
        .clone()
        .unwrap_or_else(|| DEFAULT_CLIENT_VERSION.to_owned());
    let phase = Phase::Connecting {
        server: server.name.clone(),
        version: version.clone(),
    };
    let world = Arc::new(Mutex::new(Arc::new(World::connecting(phase.clone()))));
    let session = Session {
        username: account.username.clone(),
        token: account.token.clone(),
        options,
        version: String::new(),
        poll,
        ready: Events::with_capacity(16),
        requests,
        events,
        world: Arc::clone(&world),
        outgoing: Vec::new(),
        replies: Vec::new(),
        queue: VecDeque::new(),
        current: None,
        stream: None,
        framer: Framer::default(),
        outbox: Vec::new(),
        phase,
        me: None,
        arriving: None,
        room: None,
        joins: 0,
        transferred_at: None,
        refusal: None,
        character: None,
        inventory: Inventory::default(),
        factions: Vec::new(),
        quests: Arc::default(),
        closing: None,
        logout_deadline: None,
    };
    thread::Builder::new()
        .name("mira-session".into())
        .spawn(move || session.run(server, version))?;
    let handle = Handle {
        requests: requests_out,
        waker,
        world,
    };
    Ok((handle, events_in))
}

struct Session {
    username: String,
    token: Token,
    options: Options,
    version: String,
    poll: Poll,
    ready: Events,
    requests: mpsc::Receiver<Request>,
    events: mpsc::Sender<Event>,
    world: Slot,
    outgoing: Vec<Event>,
    replies: Vec<(Reply, Result<(), ActionError>)>,
    queue: VecDeque<(Action, Reply)>,
    current: Option<Op>,
    stream: Option<TcpStream>,
    framer: Framer,
    outbox: Vec<u8>,
    phase: Phase,
    me: Option<EntityId>,
    arriving: Option<Room>,
    room: Option<Room>,
    joins: u64,
    transferred_at: Option<Instant>,
    refusal: Option<String>,
    character: Option<Me>,
    inventory: Inventory,
    factions: Vec<Faction>,
    quests: Arc<BTreeMap<QuestId, Quest>>,
    closing: Option<Disconnect>,
    logout_deadline: Option<Instant>,
}

impl Session {
    fn run(mut self, server: Server, version: String) {
        self.current = Some(self.start_login(server, version, 1));
        loop {
            self.take_requests();
            self.advance(Instant::now());
            if matches!(self.phase, Phase::Closed { .. }) && self.current.is_none() {
                self.take_requests();
                self.advance(Instant::now());
                return self.publish();
            }
            self.publish();
            let timeout = self
                .next_deadline()
                .map(|at| at.saturating_duration_since(Instant::now()));
            match self.poll.poll(&mut self.ready, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.close(Disconnect::ConnectionLost(format!("poll failed: {e}")));
                    continue;
                }
            }
            self.read_socket();
            self.flush();
        }
    }

    fn take_requests(&mut self) {
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Request::Logout => self.begin_logout(),
                Request::Act(action, reply) => self.queue.push_back((action, reply)),
            }
        }
    }

    fn advance(&mut self, now: Instant) {
        if self.logout_deadline.is_some_and(|d| now >= d) {
            self.close(Disconnect::LoggedOut);
        }
        loop {
            match self.current.take() {
                Some(op) => {
                    self.current = self.step(op, now);
                    if self.current.is_some() {
                        return;
                    }
                }
                None => {
                    let Some((action, reply)) = self.queue.pop_front() else {
                        return;
                    };
                    self.current = self.start(action, reply, now);
                }
            }
        }
    }

    fn start(&mut self, action: Action, reply: Reply, now: Instant) -> Option<Op> {
        let result = match action {
            Action::Ready => self.ensure_ready(),
            Action::MoveToCell { cell, pad } => self.move_to_cell(&cell, &pad),
            Action::Join { map } => match self.start_join(map, now) {
                Ok(Some(stage)) => return Some(Op::Join { reply, stage }),
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
        };
        self.replies.push((reply, result));
        None
    }

    fn step(&mut self, op: Op, now: Instant) -> Option<Op> {
        match op {
            Op::Login {
                server,
                version,
                attempts,
                deadline,
            } => match &self.phase {
                Phase::Ready => None,
                Phase::Closed {
                    reason: Disconnect::ClientVersion(required),
                } if self.options.client_version.is_none()
                    && *required != version
                    && attempts < VERSION_RETRIES =>
                {
                    let required = required.clone();
                    let retry = self.start_login(server, required, attempts + 1);
                    self.step(retry, now)
                }
                Phase::Closed { .. } => None,
                _ if now >= deadline => {
                    self.close(Disconnect::ConnectionLost(
                        "the character never loaded".into(),
                    ));
                    None
                }
                _ => Some(Op::Login {
                    server,
                    version,
                    attempts,
                    deadline,
                }),
            },
            Op::Join {
                reply,
                stage: JoinStage::Cooling { map, until },
            } => {
                if let Err(e) = self.ensure_ready() {
                    self.replies.push((reply, Err(e)));
                    return None;
                }
                if now < until {
                    return Some(Op::Join {
                        reply,
                        stage: JoinStage::Cooling { map, until },
                    });
                }
                let joins = self.joins;
                self.refusal = None;
                self.send(&packet::transfer(&self.username, &map));
                self.transferred_at = Some(now);
                Some(Op::Join {
                    reply,
                    stage: JoinStage::Sent {
                        joins,
                        deadline: now + JOIN_TIMEOUT,
                    },
                })
            }
            Op::Join {
                reply,
                stage: JoinStage::Sent { joins, deadline },
            } => {
                let result = if let Err(e) = self.ensure_ready() {
                    Err(e)
                } else if self.joins != joins {
                    Ok(())
                } else if let Some(why) = self.refusal.clone() {
                    Err(ActionError::Refused(why))
                } else if now >= deadline {
                    Err(ActionError::NoReply)
                } else {
                    return Some(Op::Join {
                        reply,
                        stage: JoinStage::Sent { joins, deadline },
                    });
                };
                self.replies.push((reply, result));
                None
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let op = match &self.current {
            Some(Op::Login { deadline, .. })
            | Some(Op::Join {
                stage: JoinStage::Sent { deadline, .. },
                ..
            }) => Some(*deadline),
            Some(Op::Join {
                stage: JoinStage::Cooling { until, .. },
                ..
            }) => Some(*until),
            None => None,
        };
        op.into_iter().chain(self.logout_deadline).min()
    }

    fn publish(&mut self) {
        *self.world.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(World {
            phase: self.phase.clone(),
            room: self.room.clone(),
            me: self.character.clone(),
            inventory: Arc::clone(&self.inventory.items),
            temp: self.inventory.temp.clone(),
            drops: self.inventory.drops.clone(),
            factions: self.factions.clone(),
            quests: Arc::clone(&self.quests),
        });
        for event in self.outgoing.drain(..) {
            let _ = self.events.send(event);
        }
        for (reply, result) in self.replies.drain(..) {
            let _ = reply.send(result);
        }
    }

    fn start_login(&mut self, server: Server, version: String, attempts: usize) -> Op {
        self.open(&server, &version);
        Op::Login {
            server,
            version,
            attempts,
            deadline: Instant::now() + READY_TIMEOUT,
        }
    }

    fn start_join(&mut self, map: String, now: Instant) -> Result<Option<JoinStage>, ActionError> {
        self.ensure_ready()?;
        if self.room.as_ref().is_some_and(|room| room.is_in(&map)) {
            return Ok(None);
        }
        let until = self.transferred_at.map_or(now, |at| at + TRANSFER_COOLDOWN);
        Ok(Some(JoinStage::Cooling { map, until }))
    }

    fn move_to_cell(&mut self, cell: &str, pad: &str) -> Result<(), ActionError> {
        self.ensure_ready()?;
        let Some(room) = &mut self.room else {
            return Err(ActionError::NotReady);
        };
        room.cell = cell.to_owned();
        room.pad = pad.to_owned();
        let area = room.area;
        self.send(&packet::move_to_cell(area, cell, pad));
        self.fetch_cellmates();
        self.ensure_ready()?;
        self.emit(Event::Moved {
            cell: cell.to_owned(),
            pad: pad.to_owned(),
        });
        Ok(())
    }

    fn ensure_ready(&self) -> Result<(), ActionError> {
        if let Some(why) = &self.closing {
            return Err(ActionError::Closed(why.clone()));
        }
        match &self.phase {
            Phase::Ready => Ok(()),
            Phase::Closed { reason } => Err(ActionError::Closed(reason.clone())),
            _ => Err(ActionError::NotReady),
        }
    }

    fn open(&mut self, server: &Server, version: &str) {
        self.set_phase(Phase::Connecting {
            server: server.name.clone(),
            version: version.to_owned(),
        });
        let connected = connect(server).and_then(|mut stream| {
            self.poll.registry().register(
                &mut stream,
                SOCKET,
                Interest::READABLE | Interest::WRITABLE,
            )?;
            Ok(stream)
        });
        let stream = match connected {
            Ok(stream) => stream,
            Err(error) => {
                return self.close(Disconnect::ConnectionLost(format!(
                    "can't reach {}: {error}",
                    server.name
                )));
            }
        };
        self.stream = Some(stream);
        self.framer = Framer::default();
        self.outbox.clear();
        self.me = None;
        self.arriving = None;
        self.room = None;
        self.transferred_at = None;
        self.refusal = None;
        self.character = None;
        self.inventory = Inventory::default();
        self.factions.clear();
        self.quests = Arc::default();
        self.closing = None;
        self.logout_deadline = None;
        self.version = version.to_owned();
        self.set_phase(Phase::Handshaking);
        self.send(&packet::version_check());
    }

    fn begin_logout(&mut self) {
        if self.stream.is_none() {
            return self.close(Disconnect::LoggedOut);
        }
        if self.logout_deadline.is_none() {
            self.send(&packet::logout());
            self.closing = Some(Disconnect::LoggedOut);
            self.logout_deadline = Some(Instant::now() + LOGOUT_TIMEOUT);
        }
    }

    fn close(&mut self, why: Disconnect) {
        if let Some(mut stream) = self.stream.take() {
            let _ = self.poll.registry().deregister(&mut stream);
        }
        self.outbox.clear();
        self.logout_deadline = None;
        if !matches!(self.phase, Phase::Closed { .. }) {
            self.set_phase(Phase::Closed { reason: why });
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase.clone();
        self.emit(Event::Phase(phase));
    }

    fn emit(&mut self, event: Event) {
        self.outgoing.push(event);
    }

    fn send(&mut self, packet: &str) {
        if self.stream.is_none() {
            return;
        }
        if self.options.trace_packets {
            let text = packet.replace(self.token.as_str(), "<token>");
            self.emit(Event::Packet {
                direction: Direction::Out,
                text,
            });
        }
        self.outbox.extend_from_slice(packet.as_bytes());
        self.outbox.push(0);
        self.flush();
    }

    fn flush(&mut self) {
        while !self.outbox.is_empty() {
            let Some(stream) = &mut self.stream else {
                return;
            };
            match stream.write(&self.outbox) {
                Ok(0) => {
                    return self.close(Disconnect::ConnectionLost(
                        "socket closed while writing".into(),
                    ));
                }
                Ok(n) => {
                    self.outbox.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn read_socket(&mut self) {
        let mut buf = [0u8; 64 * 1024];
        loop {
            let Some(stream) = &mut self.stream else {
                return;
            };
            match stream.read(&mut buf) {
                Ok(0) => {
                    let why = self.closing.take().unwrap_or_else(|| {
                        Disconnect::ConnectionLost("the server closed the connection".into())
                    });
                    return self.close(why);
                }
                Ok(n) => self.receive_bytes(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn receive_bytes(&mut self, data: &[u8]) {
        let mut framer = std::mem::take(&mut self.framer);
        if let Err(e) = framer.push(data) {
            return self.close(Disconnect::Protocol(e.to_string()));
        }
        while self.stream.is_some()
            && let Some(frame) = framer.next_frame()
        {
            self.receive(frame);
        }
        self.framer = framer;
    }

    fn receive(&mut self, frame: &[u8]) {
        let message = mira_protocol::unpack(frame).and_then(|text| {
            if self.options.trace_packets {
                self.emit(Event::Packet {
                    direction: Direction::In,
                    text: redact(&text),
                });
            }
            mira_protocol::parse(&text)
        });
        let handled = match message {
            Ok(Message::Xml(xml)) => {
                self.on_xml(&xml);
                Ok(())
            }
            Ok(Message::Xt(xt)) => {
                self.on_xt(&xt);
                Ok(())
            }
            Ok(Message::Json(json)) => self.on_json(json),
            Err(e) => Err(e.to_string()),
        };
        if let Err(error) = handled {
            self.emit(Event::Unreadable { error });
        }
    }

    fn on_xml(&mut self, xml: &XmlMessage) {
        match xml.action.as_str() {
            "apiOK" if self.phase == Phase::Handshaking => {
                let login = packet::login(&self.username, &self.version, self.token.as_str());
                self.send(&login);
                self.set_phase(Phase::LoggingIn);
            }
            "logout" => {
                let why = self.closing.take().unwrap_or(Disconnect::Ended);
                self.close(why);
            }
            "uER" => {
                if let Some(user) = &xml.user {
                    self.on_player_entered(user);
                }
            }
            "userGone" => {
                if let Some(user) = &xml.user {
                    self.on_player_left(user.id);
                }
            }
            _ => {}
        }
    }

    fn on_xt(&mut self, xt: &XtMessage) {
        let text = || xt.field(4).unwrap_or_default().to_owned();
        match xt.cmd() {
            "loginResponse" if self.phase == Phase::LoggingIn => self.on_login_response(xt),
            "server" => {
                let text = text();
                if text.to_ascii_lowercase().contains("invalid session") {
                    self.closing = Some(Disconnect::InvalidSession);
                }
                let joined = text.starts_with("You joined ");
                self.emit(Event::Notice {
                    kind: NoticeKind::Server,
                    text,
                });
                if joined && let Some(room) = self.arriving.take() {
                    self.enter(room);
                }
            }
            "notify" => self.emit(Event::Notice {
                kind: NoticeKind::Notify,
                text: text(),
            }),
            "warning" => {
                let text = text();
                self.refusal = Some(text.clone());
                self.emit(Event::Notice {
                    kind: NoticeKind::Warning,
                    text,
                });
            }
            "chatm" => {
                let raw = text();
                let (channel, text) = raw.split_once('~').unwrap_or(("", &raw));
                self.emit(Event::Chat {
                    channel: channel.to_owned(),
                    sender: xt.field(5).unwrap_or_default().to_owned(),
                    text: text.to_owned(),
                });
            }
            "whisper" => self.emit(Event::Whisper {
                sender: xt.field(5).unwrap_or_default().to_owned(),
                text: text(),
            }),
            "multiLoginWarning" => self.closing = Some(Disconnect::LoggedInElsewhere),
            "exitArea" => {
                if let Some(eid) = xt.field(4).and_then(|eid| eid.parse().ok()) {
                    self.on_player_left(EntityId(eid));
                }
            }
            "uotls" => {
                if let (Some(name), Some(fields)) = (xt.field(4), xt.field(5)) {
                    self.on_player_moved(name, fields);
                }
            }
            _ => {}
        }
    }

    fn on_login_response(&mut self, xt: &XtMessage) {
        let accepted = xt.field(4) == Some("true");
        match xt.field(5).and_then(|eid| eid.parse().ok()) {
            Some(eid) if accepted => {
                self.me = Some(EntityId(eid));
                self.character = Some(Me::default());
                self.send(&packet::first_join());
                self.send(&packet::clear_ignore_list());
                self.set_phase(Phase::Loading {
                    step: Load::Joining,
                });
            }
            _ => {
                let message = xt.field(7).unwrap_or_default();
                let why = match required_version(message) {
                    Some(version) => Disconnect::ClientVersion(version),
                    None => Disconnect::Rejected(message.to_owned()),
                };
                self.close(why);
            }
        }
    }

    fn on_json(&mut self, json: JsonMessage) -> Result<(), String> {
        fn read<T: DeserializeOwned>(json: &JsonMessage) -> Result<T, String> {
            T::deserialize(&json.data).map_err(|e| format!("{}: {e}", json.cmd))
        }
        match json.cmd.as_str() {
            "moveToArea" => {
                let (room, mine) = Room::arriving(read::<MoveToArea>(&json)?, self.me);
                if let (Some(mine), Some(character)) = (mine, &mut self.character) {
                    character.apply(&mine);
                }
                self.arriving = Some(room);
            }
            "initUserData"
                if self.phase
                    == (Phase::Loading {
                        step: Load::Character,
                    }) =>
            {
                let user: InitUserData = read(&json)?;
                let (Some(me), Some(room)) = (self.me, &self.room) else {
                    return Ok(());
                };
                if user.uid == me {
                    let room = room.area;
                    if let Some(character) = &mut self.character {
                        character.load(&user.data);
                    }
                    self.send(&packet::retrieve_inventory(room, me));
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                }
            }
            "mtls" => {
                let update: MonsterUpdate = read(&json)?;
                self.patch_monster(update.id, &update.o);
            }
            "ct" | "cb" => {
                let tick: Tick = read(&json)?;
                for (id, patch) in &tick.monsters {
                    self.patch_monster(*id, patch);
                }
                for (name, patch) in &tick.players {
                    self.patch_player(name, patch);
                }
                for change in tick.auras {
                    self.apply_aura(change);
                }
            }
            "aura+" | "aura++" | "aura+p" | "aura-" | "aura--" => {
                self.apply_aura(read(&json)?);
            }
            "clearAuras" => {
                if let Some(character) = &mut self.character {
                    character.auras.clear_unless_persistent();
                }
            }
            "uotls" => {
                let update: PlayerUpdate = read(&json)?;
                self.patch_player(&update.name, &update.o);
            }
            "stu" => {
                let stats: Stats = read(&json)?;
                if let (Some(haste), Some(character)) = (stats.sta.haste, &mut self.character) {
                    character.haste = haste;
                }
            }
            "sAct" => {
                let skills: SkillList = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.learn_skills(skills);
                }
            }
            "loadInventoryBig"
                if self.phase
                    == (Phase::Loading {
                        step: Load::Inventory,
                    }) =>
            {
                let loaded: LoadInventory = read(&json)?;
                let Some(character) = &self.character else {
                    return Ok(());
                };
                let (level, gold) = (character.level, character.gold);
                self.inventory.items = Arc::new(self.parse_each("inventory item", loaded.items));
                self.factions = self
                    .parse_each::<FactionData>("faction", loaded.factions)
                    .into_iter()
                    .map(Faction::from)
                    .collect();
                self.set_phase(Phase::Ready);
                self.emit(Event::Ready {
                    level,
                    gold,
                    items: self.inventory.items.len(),
                });
            }
            "addItems" => {
                for (key, item) in read::<AddedItems>(&json)?.items {
                    self.inventory.add(&key, item);
                }
            }
            "dropItem" => {
                for (key, item) in read::<AddedItems>(&json)?.items {
                    self.inventory.offer(&key, item);
                }
            }
            "getDrop" => self.inventory.pick(read::<Picked>(&json)?),
            "turnIn" => self.inventory.turn_in(&read::<TurnIn>(&json)?),
            "sellItem" => {
                let sold: Sold = read(&json)?;
                self.inventory.sell(&sold);
                if let Some(character) = &mut self.character {
                    character.receive(sold.amount, sold.coins);
                }
            }
            "equipItem" | "unequipItem" => {
                let worn: Worn = read(&json)?;
                if Some(worn.uid) == self.me {
                    self.inventory.wear(worn.id, json.cmd == "equipItem");
                }
            }
            "addGoldExp" => {
                let reward: Reward = read(&json)?;
                if let Some(points) = self.character.as_mut().and_then(|c| c.gain(&reward)) {
                    self.inventory.set_class_points(points);
                }
                if let Some(id) = reward.faction {
                    faction::add_rep(&mut self.factions, id, reward.rep);
                }
            }
            "levelUp" => {
                let level: LevelUp = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.level_up(&level);
                }
            }
            "updateClass" => {
                let class: ClassChange = read(&json)?;
                if let Some(character) = self
                    .character
                    .as_mut()
                    .filter(|_| Some(class.uid) == self.me)
                {
                    character.change_class(&class);
                }
            }
            "addFaction" => {
                let added: AddedFaction = read(&json)?;
                faction::learn(&mut self.factions, added.faction.into());
            }
            "updateQuest" => {
                let update: QuestUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.quests.update(&update);
                }
            }
            "setAchievement" => {
                let update: AchievementUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.quests.set_achievement(&update);
                }
            }
            "getQuests" => {
                for error in quest::learn(&mut self.quests, read::<QuestList>(&json)?) {
                    self.emit(Event::Unreadable { error });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn enter(&mut self, room: Room) {
        let Some(me) = self.me else { return };
        let first = self.phase
            == (Phase::Loading {
                step: Load::Joining,
            });
        self.send(&packet::move_to_cell(room.area, &room.cell, &room.pad));
        if first {
            self.send(&packet::retrieve_user_data(room.area, me));
        }
        let joined = Event::Joined {
            map: room.name.clone(),
            cell: room.cell.clone(),
            pad: room.pad.clone(),
            players: room.players.len(),
        };
        self.room = Some(room);
        self.joins += 1;
        self.fetch_cellmates();
        self.emit(joined);
        if first {
            self.set_phase(Phase::Loading {
                step: Load::Character,
            });
        }
    }

    fn fetch_cellmates(&mut self) {
        let Some(room) = &mut self.room else { return };
        let eids = room.unfetched_cellmates();
        if !eids.is_empty() {
            let area = room.area;
            self.send(&packet::retrieve_user_datas(area, &eids));
        }
    }

    fn on_player_entered(&mut self, user: &XmlUser) {
        let (Some(me), Some(room)) = (self.me, &mut self.room) else {
            return;
        };
        if user.id == me {
            return;
        }
        let Some(name) = room.add_player(user) else {
            return;
        };
        let area = room.area;
        self.send(&packet::retrieve_user_data(area, user.id));
        self.emit(Event::PlayerEntered { name });
    }

    fn on_player_left(&mut self, eid: EntityId) {
        if let Some(name) = self.room.as_mut().and_then(|room| room.remove_player(eid)) {
            self.emit(Event::PlayerLeft { name });
        }
    }

    fn on_player_moved(&mut self, name: &str, fields: &str) {
        if let Some(room) = &mut self.room {
            room.move_player(name, fields);
        }
    }

    fn parse_each<T: DeserializeOwned>(&mut self, what: &str, values: Vec<Value>) -> Vec<T> {
        values
            .into_iter()
            .filter_map(|value| match serde_json::from_value(value) {
                Ok(parsed) => Some(parsed),
                Err(e) => {
                    self.emit(Event::Unreadable {
                        error: format!("{what}: {e}"),
                    });
                    None
                }
            })
            .collect()
    }

    fn apply_aura(&mut self, change: AuraChange) {
        let auras = match change.target.as_deref().and_then(aura::target) {
            Some(Target::Player(eid)) if Some(eid) == self.me => {
                self.character.as_mut().map(|c| &mut c.auras)
            }
            Some(Target::Monster(id)) => self
                .arriving
                .as_mut()
                .or(self.room.as_mut())
                .and_then(|room| room.monster_auras(id)),
            _ => None,
        };
        if let Some(auras) = auras {
            auras.apply(change);
        }
    }

    fn patch_player(&mut self, name: &str, patch: &Vitals) {
        if let Some(character) = &mut self.character
            && name.eq_ignore_ascii_case(&self.username)
        {
            character.apply(patch);
        }
    }

    fn patch_monster(&mut self, id: mira_protocol::MonMapId, patch: &Vitals) {
        if let Some(room) = self.arriving.as_mut().or(self.room.as_mut()) {
            room.patch_monster(id, patch);
        }
    }
}

fn connect(server: &Server) -> io::Result<TcpStream> {
    let addr = (server.host.as_str(), server.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} has no address", server.host),
            )
        })?;
    let stream = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_nonblocking(true)?;
    Ok(TcpStream::from_std(stream))
}

fn required_version(message: &str) -> Option<String> {
    const MARK: &str = "requires AQW game version ";
    let rest = &message[message.find(MARK)? + MARK.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let version = rest[..end].trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_owned())
}

fn redact(text: &str) -> String {
    fn scrub(value: &mut Value) -> bool {
        match value {
            Value::Object(map) => {
                let mut changed = false;
                for (key, field) in map.iter_mut() {
                    if REDACTED_KEYS.contains(&key.as_str()) {
                        *field = Value::String("<redacted>".into());
                        changed = true;
                    } else {
                        changed |= scrub(field);
                    }
                }
                changed
            }
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| scrub(item) | changed),
            _ => false,
        }
    }
    if !REDACTED_KEYS.iter().any(|key| text.contains(key)) {
        return text.to_owned();
    }
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_owned();
    };
    if scrub(&mut value) {
        value.to_string()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests;
