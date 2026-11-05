use std::convert::Infallible;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Waker};
use mira_protocol::{
    AreaId, EntityId, Framer, JsonMessage, Message, XmlMessage, XmlUser, XtMessage, flex, packet,
};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::api::{Account, Server, Token};
use crate::event::{Direction, Event, NoticeKind};

pub const DEFAULT_CLIENT_VERSION: &str = "5.31";

const SOCKET: mio::Token = mio::Token(0);
const WAKER: mio::Token = mio::Token(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_COOLDOWN: Duration = Duration::from_millis(3000);
const VERSION_RETRIES: usize = 3;
const SPAWN: (&str, &str) = ("Enter", "Spawn");
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
pub enum LoginError {
    #[error("can't reach {server}: {error}")]
    Connect { server: String, error: io::Error },
    #[error("{0}")]
    Closed(Disconnect),
    #[error("the character did not load within {} s", READY_TIMEOUT.as_secs())]
    NotReady,
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
}

enum Halt {
    Timeout,
    Closed(Disconnect),
}

#[derive(Clone)]
pub struct Control {
    logout: Arc<AtomicBool>,
    waker: Arc<Waker>,
}

impl Control {
    pub fn logout(&self) {
        self.logout.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn wake(&self) {
        let _ = self.waker.wake();
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub client_version: Option<String>,
    pub trace_packets: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Room {
    #[serde(skip)]
    area: AreaId,
    pub name: String,
    pub cell: String,
    pub pad: String,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Player {
    #[serde(skip)]
    eid: EntityId,
    pub name: String,
    pub cell: Option<String>,
    #[serde(skip)]
    fetched: bool,
}

#[derive(Clone, Copy)]
struct Character {
    level: u32,
    gold: u64,
}

pub struct Session {
    username: String,
    token: Token,
    options: Options,
    version: String,
    sink: Box<dyn FnMut(&Event)>,
    poll: Poll,
    events: Events,
    control: Control,
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
    character: Option<Character>,
    closing: Option<Disconnect>,
    logout_deadline: Option<Instant>,
}

impl Session {
    pub fn new(
        account: &Account,
        options: Options,
        sink: impl FnMut(&Event) + 'static,
    ) -> io::Result<Self> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKER)?);
        Ok(Self {
            username: account.username.clone(),
            token: account.token.clone(),
            options,
            version: String::new(),
            sink: Box::new(sink),
            poll,
            events: Events::with_capacity(16),
            control: Control {
                logout: Arc::new(AtomicBool::new(false)),
                waker,
            },
            stream: None,
            framer: Framer::default(),
            outbox: Vec::new(),
            phase: Phase::Closed {
                reason: Disconnect::LoggedOut,
            },
            me: None,
            arriving: None,
            room: None,
            joins: 0,
            transferred_at: None,
            refusal: None,
            character: None,
            closing: None,
            logout_deadline: None,
        })
    }

    pub fn control(&self) -> Control {
        self.control.clone()
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn room(&self) -> Option<&Room> {
        self.room.as_ref()
    }

    pub fn login(&mut self, server: &Server) -> Result<(), LoginError> {
        let pinned = self.options.client_version.clone();
        let mut version = pinned
            .clone()
            .unwrap_or_else(|| DEFAULT_CLIENT_VERSION.to_owned());
        for _ in 0..VERSION_RETRIES {
            self.open(server, &version)?;
            let ready = self.pump_until(Some(Instant::now() + READY_TIMEOUT), |s| {
                (s.phase == Phase::Ready).then_some(())
            });
            match ready {
                Ok(()) => return Ok(()),
                Err(Halt::Closed(Disconnect::ClientVersion(required)))
                    if pinned.is_none() && required != version =>
                {
                    version = required;
                }
                Err(Halt::Closed(why)) => return Err(LoginError::Closed(why)),
                Err(Halt::Timeout) => {
                    self.close(Disconnect::ConnectionLost(
                        "the character never loaded".into(),
                    ));
                    return Err(LoginError::NotReady);
                }
            }
        }
        Err(LoginError::Closed(Disconnect::ClientVersion(version)))
    }

    pub fn run(&mut self) -> Disconnect {
        match self.run_until(|| None::<Infallible>) {
            Ok(never) => match never {},
            Err(why) => why,
        }
    }

    pub fn run_until<T>(&mut self, mut next: impl FnMut() -> Option<T>) -> Result<T, Disconnect> {
        match self.pump_until(None, |_| next()) {
            Ok(value) => Ok(value),
            Err(Halt::Closed(why)) => Err(why),
            Err(Halt::Timeout) => unreachable!("an unbounded pump only ends on close"),
        }
    }

    pub fn join(&mut self, map: &str) -> Result<(), ActionError> {
        self.ensure_ready()?;
        if self.room.as_ref().is_some_and(|room| room.is_in(map)) {
            return Ok(());
        }
        if let Some(at) = self.transferred_at {
            self.idle_until(at + TRANSFER_COOLDOWN)?;
        }
        let joins = self.joins;
        self.refusal = None;
        self.send(&packet::transfer(&self.username, map));
        self.transferred_at = Some(Instant::now());
        let outcome = self.pump_until(Some(Instant::now() + JOIN_TIMEOUT), |s| {
            if s.joins != joins {
                Some(Ok(()))
            } else {
                s.refusal.clone().map(|why| Err(ActionError::Refused(why)))
            }
        });
        match outcome {
            Ok(result) => result,
            Err(Halt::Timeout) => Err(ActionError::NoReply),
            Err(Halt::Closed(why)) => Err(ActionError::Closed(why)),
        }
    }

    pub fn move_to_cell(&mut self, cell: &str, pad: &str) -> Result<(), ActionError> {
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
        match &self.phase {
            Phase::Ready => Ok(()),
            Phase::Closed { reason } => Err(ActionError::Closed(reason.clone())),
            _ => Err(ActionError::NotReady),
        }
    }

    fn idle_until(&mut self, until: Instant) -> Result<(), ActionError> {
        match self.pump_until(Some(until), |_| None::<Infallible>) {
            Ok(never) => match never {},
            Err(Halt::Timeout) => Ok(()),
            Err(Halt::Closed(why)) => Err(ActionError::Closed(why)),
        }
    }

    fn pump_until<T>(
        &mut self,
        deadline: Option<Instant>,
        mut done: impl FnMut(&Self) -> Option<T>,
    ) -> Result<T, Halt> {
        loop {
            if let Some(value) = done(self) {
                return Ok(value);
            }
            if let Phase::Closed { reason } = &self.phase {
                return Err(Halt::Closed(reason.clone()));
            }
            let now = Instant::now();
            if self.logout_deadline.is_some_and(|d| now >= d) {
                self.close(Disconnect::LoggedOut);
                continue;
            }
            if deadline.is_some_and(|d| now >= d) {
                return Err(Halt::Timeout);
            }
            let wake = deadline.into_iter().chain(self.logout_deadline).min();
            match self.poll.poll(&mut self.events, wake.map(|w| w - now)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.close(Disconnect::ConnectionLost(format!("poll failed: {e}")));
                    continue;
                }
            }
            if self.control.logout.swap(false, Ordering::SeqCst) {
                self.begin_logout();
            }
            self.read_socket();
            self.flush();
        }
    }

    fn open(&mut self, server: &Server, version: &str) -> Result<(), LoginError> {
        self.set_phase(Phase::Connecting {
            server: server.name.clone(),
            version: version.to_owned(),
        });
        let mut stream = connect(server).map_err(|error| {
            let error = LoginError::Connect {
                server: server.name.clone(),
                error,
            };
            self.close(Disconnect::ConnectionLost(error.to_string()));
            error
        })?;
        self.poll
            .registry()
            .register(&mut stream, SOCKET, Interest::READABLE | Interest::WRITABLE)
            .map_err(|error| LoginError::Connect {
                server: server.name.clone(),
                error,
            })?;
        self.stream = Some(stream);
        self.framer = Framer::default();
        self.outbox.clear();
        self.me = None;
        self.arriving = None;
        self.room = None;
        self.transferred_at = None;
        self.refusal = None;
        self.character = None;
        self.closing = None;
        self.logout_deadline = None;
        self.version = version.to_owned();
        self.set_phase(Phase::Handshaking);
        self.send(&packet::version_check());
        Ok(())
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
        (self.sink)(&event);
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
        let unreadable = |e: serde_json::Error| format!("{}: {e}", json.cmd);
        match json.cmd.as_str() {
            "moveToArea" => {
                let area: MoveToArea = serde_json::from_value(json.data).map_err(unreadable)?;
                let me = self.me;
                let (cell, pad) = SPAWN;
                self.arriving = Some(Room {
                    area: area.area,
                    name: area.name,
                    cell: cell.to_owned(),
                    pad: pad.to_owned(),
                    players: area
                        .players
                        .into_iter()
                        .filter(|p| Some(p.eid) != me)
                        .map(|p| Player {
                            eid: p.eid,
                            name: p.name,
                            cell: Some(p.cell).filter(|cell| !cell.is_empty()),
                            fetched: false,
                        })
                        .collect(),
                });
            }
            "initUserData"
                if self.phase
                    == (Phase::Loading {
                        step: Load::Character,
                    }) =>
            {
                let user: InitUserData = serde_json::from_value(json.data).map_err(unreadable)?;
                let (Some(me), Some(room)) = (self.me, &self.room) else {
                    return Ok(());
                };
                if user.uid == me {
                    let room = room.area;
                    self.character = Some(Character {
                        level: user.data.level,
                        gold: user.data.gold,
                    });
                    self.send(&packet::retrieve_inventory(room, me));
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                }
            }
            "loadInventoryBig"
                if self.phase
                    == (Phase::Loading {
                        step: Load::Inventory,
                    }) =>
            {
                let inventory: LoadInventory =
                    serde_json::from_value(json.data).map_err(unreadable)?;
                let Some(character) = self.character else {
                    return Ok(());
                };
                self.set_phase(Phase::Ready);
                self.emit(Event::Ready {
                    level: character.level,
                    gold: character.gold,
                    items: inventory.items.len(),
                });
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
        let eids: Vec<EntityId> = room
            .players
            .iter_mut()
            .filter(|p| !p.fetched && p.cell.as_ref() == Some(&room.cell))
            .map(|p| {
                p.fetched = true;
                p.eid
            })
            .collect();
        if !eids.is_empty() {
            let area = room.area;
            self.send(&packet::retrieve_user_datas(area, &eids));
        }
    }

    fn on_player_entered(&mut self, user: &XmlUser) {
        let (Some(me), Some(room), Some(name)) = (self.me, &mut self.room, &user.name) else {
            return;
        };
        if user.id == me || room.players.iter().any(|p| p.eid == user.id) {
            return;
        }
        room.players.push(Player {
            eid: user.id,
            name: name.clone(),
            cell: None,
            fetched: true,
        });
        let area = room.area;
        self.send(&packet::retrieve_user_data(area, user.id));
        self.emit(Event::PlayerEntered { name: name.clone() });
    }

    fn on_player_left(&mut self, eid: EntityId) {
        let Some(room) = &mut self.room else { return };
        let Some(at) = room.players.iter().position(|p| p.eid == eid) else {
            return;
        };
        let name = room.players.remove(at).name;
        self.emit(Event::PlayerLeft { name });
    }

    fn on_player_moved(&mut self, name: &str, fields: &str) {
        let Some(room) = &mut self.room else { return };
        let Some(player) = room
            .players
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        let cell = fields
            .split(',')
            .filter_map(|pair| pair.split_once(':'))
            .find_map(|(key, value)| (key == "strFrame").then_some(value));
        if let Some(cell) = cell {
            player.cell = Some(cell.to_owned());
        }
    }
}

impl Room {
    fn is_in(&self, map: &str) -> bool {
        let base = self
            .name
            .rsplit_once('-')
            .map_or(self.name.as_str(), |(base, _)| base);
        self.name.eq_ignore_ascii_case(map) || base.eq_ignore_ascii_case(map)
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

#[derive(Deserialize)]
struct MoveToArea {
    #[serde(rename = "areaId")]
    area: AreaId,
    #[serde(rename = "areaName")]
    name: String,
    #[serde(rename = "uoBranch", default)]
    players: Vec<AreaPlayer>,
}

#[derive(Deserialize)]
struct AreaPlayer {
    #[serde(rename = "entID")]
    eid: EntityId,
    #[serde(rename = "strUsername")]
    name: String,
    #[serde(rename = "strFrame", default)]
    cell: String,
}

#[derive(Deserialize)]
struct InitUserData {
    uid: EntityId,
    data: UserData,
}

#[derive(Deserialize)]
struct UserData {
    #[serde(rename = "intLevel", deserialize_with = "flex::int")]
    level: u32,
    #[serde(rename = "intGold", deserialize_with = "flex::int")]
    gold: u64,
}

#[derive(Deserialize)]
struct LoadInventory {
    #[serde(default)]
    items: Vec<IgnoredAny>,
}

#[cfg(test)]
mod tests;
