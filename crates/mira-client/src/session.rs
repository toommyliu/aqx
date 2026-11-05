use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Waker};
use mira_protocol::{
    AreaId, CharItemId, EntityId, FactionId, Framer, ItemId, JsonMessage, Message, MonId, MonMapId,
    QuestId, XmlMessage, XmlUser, XtMessage, flex, packet,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::api::{Account, Server, Token};
use crate::event::{Direction, Event, NoticeKind};
use crate::quest::{self, Quest};

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

#[derive(Debug, Clone)]
pub struct World {
    pub phase: Phase,
    pub room: Option<Room>,
    pub me: Option<Me>,
    pub inventory: Arc<Vec<Item>>,
    pub temp: Vec<TempItem>,
    pub drops: Vec<DropOffer>,
    pub factions: Vec<Faction>,
    pub quests: Arc<BTreeMap<QuestId, Quest>>,
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
    let world = Arc::new(Mutex::new(Arc::new(World {
        phase: phase.clone(),
        room: None,
        me: None,
        inventory: Arc::default(),
        temp: Vec::new(),
        drops: Vec::new(),
        factions: Vec::new(),
        quests: Arc::default(),
    })));
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
        inventory: Arc::default(),
        temp: Vec::new(),
        drops: Vec::new(),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Room {
    #[serde(skip)]
    area: AreaId,
    pub name: String,
    pub cell: String,
    pub pad: String,
    pub players: Vec<Player>,
    pub monsters: Vec<Monster>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Monster {
    pub id: MonMapId,
    pub name: String,
    pub cell: String,
    pub hp: u32,
    pub max_hp: u32,
    pub state: CombatState,
    pub aggressive: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatState {
    Dead,
    #[default]
    Idle,
    InCombat,
}

impl<'de> Deserialize<'de> for CombatState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match flex::int::<_, i64>(d)? {
            0 => Ok(CombatState::Dead),
            1 => Ok(CombatState::Idle),
            2 => Ok(CombatState::InCombat),
            n => Err(serde::de::Error::custom(format!(
                "unknown combat state {n}"
            ))),
        }
    }
}

impl Monster {
    fn apply(&mut self, patch: &Vitals) {
        self.hp = patch.hp.unwrap_or(self.hp);
        self.max_hp = patch.max_hp.unwrap_or(self.max_hp);
        self.state = patch.state.unwrap_or(self.state);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Me {
    pub level: u32,
    pub exp: u64,
    pub exp_to_level: u64,
    pub gold: u64,
    pub coins: u64,
    pub member: bool,
    pub class_points: u32,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub state: CombatState,
    pub haste: f64,
    pub skills: Vec<Skill>,
    pub quests: QuestProgress,
}

const QUEST_STRINGS: usize = 8;
const ACHIEVEMENT_FIELDS: [&str; 13] = [
    "ia0", "ia1", "iq0", "id0", "id1", "id2", "id3", "id4", "id5", "id6", "id7", "iw0", "im0",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QuestProgress {
    strings: [Option<String>; QUEST_STRINGS],
    achievements: BTreeMap<String, i64>,
}

impl QuestProgress {
    pub fn value(&self, slot: u32) -> Option<u32> {
        let string = self.strings.get((slot / 100) as usize)?.as_ref()?;
        string.chars().nth((slot % 100) as usize)?.to_digit(36)
    }

    pub(crate) fn client_value(&self, slot: i64) -> f64 {
        let Ok(slot) = usize::try_from(slot) else {
            return -1.0;
        };
        match self.strings.get(slot / 100) {
            Some(Some(string)) => string
                .chars()
                .nth(slot % 100)
                .and_then(|c| c.to_digit(36))
                .map_or(f64::NAN, f64::from),
            _ => -1.0,
        }
    }

    pub fn achievement(&self, field: &str, bit: u32) -> Option<bool> {
        if bit > 31 {
            return None;
        }
        Some(self.achievements.get(field)? & (1 << bit) != 0)
    }

    fn from_user_data(data: &serde_json::Map<String, Value>) -> QuestProgress {
        let int = |value: &Value| flex::int::<_, i64>(value.clone()).ok();
        QuestProgress {
            strings: std::array::from_fn(|i| {
                let key = match i {
                    0 => "strQuests".to_owned(),
                    n => format!("strQuests{}", n + 1),
                };
                data.get(&key).and_then(Value::as_str).map(str::to_owned)
            }),
            achievements: ACHIEVEMENT_FIELDS
                .iter()
                .filter_map(|&field| Some((field.to_owned(), int(data.get(field)?)?)))
                .collect(),
        }
    }

    fn set_value(&mut self, slot: u32, value: i64) {
        let Some(Some(string)) = self.strings.get_mut((slot / 100) as usize) else {
            return;
        };
        let digit = u32::try_from(value)
            .ok()
            .and_then(|value| char::from_digit(value, 36))
            .map_or('0', |c| c.to_ascii_uppercase());
        let at = (slot % 100) as usize;
        let mut chars: Vec<char> = string.chars().collect();
        match chars.get_mut(at) {
            Some(c) => *c = digit,
            None => chars.push(digit),
        }
        *string = chars.into_iter().collect();
    }

    fn set_achievement(&mut self, field: &str, bit: u32, value: u8) {
        if bit > 31 || value > 1 {
            return;
        }
        let flags = self.achievements.entry(field.to_owned()).or_insert(0);
        if value == 1 {
            *flags |= 1 << bit;
        } else {
            *flags &= !(1 << bit);
        }
    }
}

impl Me {
    fn apply(&mut self, patch: &Vitals) {
        self.hp = patch.hp.unwrap_or(self.hp);
        self.max_hp = patch.max_hp.unwrap_or(self.max_hp);
        self.mp = patch.mp.unwrap_or(self.mp);
        self.max_mp = patch.max_mp.unwrap_or(self.max_mp);
        self.state = patch.state.unwrap_or(self.state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename(deserialize = "ItemID"))]
    pub id: ItemId,
    #[serde(rename(deserialize = "CharItemID"))]
    pub char_item_id: CharItemId,
    #[serde(rename(deserialize = "sName"))]
    pub name: String,
    #[serde(rename(deserialize = "iQty"), deserialize_with = "flex::int")]
    pub qty: u32,
    #[serde(rename(deserialize = "iStk"), default, deserialize_with = "flex::int")]
    pub max_stack: u32,
    #[serde(rename(deserialize = "sType"), default)]
    pub kind: String,
    #[serde(rename(deserialize = "sES"), default)]
    pub slot: String,
    #[serde(
        rename(deserialize = "bEquip"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub equipped: bool,
    #[serde(
        rename(deserialize = "bCoins"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub coins: bool,
    #[serde(rename(deserialize = "bUpg"), default, deserialize_with = "flex::bool")]
    pub member: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Faction {
    pub id: FactionId,
    pub name: String,
    pub rep: u32,
    pub rank: u32,
}

const RANK_THRESHOLDS: [u32; 9] = [
    900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500,
];

impl Faction {
    fn new(id: FactionId, name: String, rep: u32) -> Faction {
        let mut faction = Faction {
            id,
            name,
            rep: 0,
            rank: 1,
        };
        faction.set_rep(rep);
        faction
    }

    fn set_rep(&mut self, rep: u32) {
        self.rep = rep;
        self.rank = 1 + RANK_THRESHOLDS
            .iter()
            .filter(|&&floor| rep >= floor)
            .count() as u32;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DropOffer {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub kind: String,
    pub slot: String,
    pub coins: bool,
    pub member: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TempItem {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    #[serde(rename(deserialize = "ref"))]
    pub slot: String,
    #[serde(rename(deserialize = "nam"))]
    pub name: String,
    #[serde(rename(deserialize = "cd"), default, deserialize_with = "flex::int")]
    pub cooldown_ms: u32,
    #[serde(rename(deserialize = "mp"), default, deserialize_with = "flex::int")]
    pub mana: i32,
    #[serde(rename(deserialize = "tgt"))]
    pub target: SkillTarget,
    #[serde(
        rename(deserialize = "tgtMax"),
        default = "one",
        deserialize_with = "max_targets"
    )]
    pub max_targets: u32,
    #[serde(
        rename(deserialize = "isOK"),
        default = "yes",
        deserialize_with = "flex::bool"
    )]
    pub unlocked: bool,
}

fn one() -> u32 {
    1
}

fn max_targets<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    #[derive(Deserialize)]
    struct Count(#[serde(deserialize_with = "flex::int")] u32);
    Ok(Option::<Count>::deserialize(d)?.map_or(1, |count| count.0))
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillTarget {
    #[serde(rename(serialize = "hostile", deserialize = "h"))]
    Hostile,
    #[serde(rename(serialize = "friendly", deserialize = "f"))]
    Friendly,
    #[serde(rename(serialize = "self", deserialize = "s"))]
    Myself,
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
    inventory: Arc<Vec<Item>>,
    temp: Vec<TempItem>,
    drops: Vec<DropOffer>,
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
            inventory: Arc::clone(&self.inventory),
            temp: self.temp.clone(),
            drops: self.drops.clone(),
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
        self.inventory = Arc::default();
        self.temp.clear();
        self.drops.clear();
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
        let unreadable = |e: serde_json::Error| format!("{}: {e}", json.cmd);
        match json.cmd.as_str() {
            "moveToArea" => {
                let area: MoveToArea = serde_json::from_value(json.data).map_err(unreadable)?;
                let me = self.me;
                if let (Some(mine), Some(character)) = (
                    area.players.iter().find(|p| Some(p.eid) == me),
                    &mut self.character,
                ) {
                    character.apply(&mine.vitals);
                }
                let (cell, pad) = SPAWN;
                self.arriving = Some(Room {
                    area: area.area,
                    name: area.name,
                    cell: cell.to_owned(),
                    pad: pad.to_owned(),
                    monsters: room_monsters(area.monster_types, area.monster_spots, area.monsters),
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
                    if let Some(character) = &mut self.character {
                        character.level = user.data.level;
                        character.gold = user.data.gold;
                        character.coins = user.data.coins;
                        character.member = user.data.member_days >= 0;
                        character.class_points = user.data.class_points;
                        character.exp = user.data.exp;
                        character.exp_to_level = user.data.exp_to_level;
                        character.quests = QuestProgress::from_user_data(&user.data.rest);
                    }
                    self.send(&packet::retrieve_inventory(room, me));
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                }
            }
            "mtls" => {
                let update: MonsterUpdate =
                    serde_json::from_value(json.data).map_err(unreadable)?;
                self.patch_monster(update.id, &update.o);
            }
            "ct" | "cb" => {
                let tick: Tick = serde_json::from_value(json.data).map_err(unreadable)?;
                for (id, patch) in &tick.monsters {
                    self.patch_monster(*id, patch);
                }
                for (name, patch) in &tick.players {
                    self.patch_player(name, patch);
                }
            }
            "uotls" => {
                let update: PlayerUpdate = serde_json::from_value(json.data).map_err(unreadable)?;
                self.patch_player(&update.name, &update.o);
            }
            "stu" => {
                let stats: Stats = serde_json::from_value(json.data).map_err(unreadable)?;
                if let (Some(haste), Some(character)) = (stats.sta.haste, &mut self.character) {
                    character.haste = haste;
                }
            }
            "sAct" => {
                let skills: SkillList = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = &mut self.character {
                    character.skills = skills.actions.active;
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
                let Some(character) = &self.character else {
                    return Ok(());
                };
                let (level, gold) = (character.level, character.gold);
                self.inventory = Arc::new(self.parse_each("inventory item", inventory.items));
                self.factions = self
                    .parse_each::<FactionData>("faction", inventory.factions)
                    .into_iter()
                    .map(FactionData::into_faction)
                    .collect();
                self.set_phase(Phase::Ready);
                self.emit(Event::Ready {
                    level,
                    gold,
                    items: self.inventory.len(),
                });
            }
            "addItems" => {
                let added: AddedItems = serde_json::from_value(json.data).map_err(unreadable)?;
                for (key, item) in added.items {
                    self.add_item(&key, item);
                }
            }
            "turnIn" => {
                let turned_in: TurnIn = serde_json::from_value(json.data).map_err(unreadable)?;
                for (id, qty) in turned_in.items.split(',').filter_map(|pair| {
                    let (id, qty) = pair.split_once(':')?;
                    Some((ItemId(id.parse().ok()?), qty.parse().ok()?))
                }) {
                    self.consume(id, qty);
                }
            }
            "sellItem" => {
                let sold: Sold = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(at) = self
                    .inventory
                    .iter()
                    .position(|i| i.char_item_id == sold.char_item_id)
                {
                    take(Arc::make_mut(&mut self.inventory), at, sold.qty);
                }
                if let Some(character) = &mut self.character {
                    let purse = if sold.coins {
                        &mut character.coins
                    } else {
                        &mut character.gold
                    };
                    *purse += sold.amount;
                }
            }
            "addGoldExp" => {
                let reward: Reward = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = &mut self.character {
                    character.gold += reward.gold;
                    character.exp += reward.exp;
                    if let Some(gained) = reward.class_points {
                        character.class_points = character.class_points.saturating_add(gained);
                        let points = character.class_points;
                        if let Some(class) = Arc::make_mut(&mut self.inventory)
                            .iter_mut()
                            .find(|i| i.slot == "ar" && i.equipped)
                        {
                            class.qty = points;
                        }
                    }
                }
                if let Some(faction) = reward
                    .faction
                    .and_then(|id| self.factions.iter_mut().find(|f| f.id == id))
                {
                    let rep = (i64::from(faction.rep) + reward.rep).clamp(0, i64::from(u32::MAX));
                    faction.set_rep(rep as u32);
                }
            }
            "updateQuest" => {
                let update: QuestUpdate = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = &mut self.character {
                    character.quests.set_value(update.slot, update.value);
                }
            }
            "setAchievement" => {
                let update: AchievementUpdate =
                    serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = &mut self.character {
                    character
                        .quests
                        .set_achievement(&update.field, update.index, update.value);
                }
            }
            "getQuests" => {
                let list: quest::QuestList =
                    serde_json::from_value(json.data).map_err(unreadable)?;
                for (key, value) in list.quests {
                    match quest::parse(&key, value) {
                        Ok(parsed) if !self.quests.contains_key(&parsed.id) => {
                            Arc::make_mut(&mut self.quests).insert(parsed.id, parsed);
                        }
                        Ok(_) => {}
                        Err(e) => self.emit(Event::Unreadable {
                            error: format!("quest {key}: {e}"),
                        }),
                    }
                }
            }
            "addFaction" => {
                let added: AddedFaction = serde_json::from_value(json.data).map_err(unreadable)?;
                let faction = added.faction.into_faction();
                match self.factions.iter_mut().find(|f| f.id == faction.id) {
                    Some(known) => *known = faction,
                    None => self.factions.push(faction),
                }
            }
            "updateClass" => {
                let class: ClassChange = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = self
                    .character
                    .as_mut()
                    .filter(|_| Some(class.uid) == self.me)
                {
                    character.class_points = class.class_points;
                }
            }
            "levelUp" => {
                let level: LevelUp = serde_json::from_value(json.data).map_err(unreadable)?;
                if let Some(character) = &mut self.character {
                    character.level = level.level;
                    character.exp = level.exp;
                    character.exp_to_level = level.exp_to_level;
                }
            }
            "dropItem" => {
                let dropped: AddedItems = serde_json::from_value(json.data).map_err(unreadable)?;
                for (key, item) in dropped.items {
                    self.offer_drop(&key, item);
                }
            }
            "getDrop" => {
                let picked: Picked = serde_json::from_value(json.data).map_err(unreadable)?;
                self.pick_drop(picked);
            }
            "equipItem" | "unequipItem" => {
                let worn: Worn = serde_json::from_value(json.data).map_err(unreadable)?;
                if Some(worn.uid) == self.me {
                    let on = json.cmd == "equipItem";
                    let inventory = Arc::make_mut(&mut self.inventory);
                    let slot = inventory
                        .iter()
                        .find(|i| i.id == worn.id)
                        .map(|i| i.slot.clone());
                    for item in inventory.iter_mut() {
                        if item.id == worn.id {
                            item.equipped = on;
                        } else if on && Some(&item.slot) == slot.as_ref() {
                            item.equipped = false;
                        }
                    }
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

    fn add_item(&mut self, key: &str, added: AddedItem) {
        let Some(id) = added.id.or_else(|| key.parse().ok().map(ItemId)) else {
            return;
        };
        if added.bank {
            return;
        }
        match added.char_item_id.filter(|_| !added.temp) {
            None => match self.temp.iter_mut().find(|i| i.id == id) {
                Some(held) => held.qty = added.qty_now.unwrap_or(held.qty + added.qty),
                None => self.temp.push(TempItem {
                    id,
                    name: added.name,
                    qty: added.qty_now.unwrap_or(added.qty),
                    max_stack: added.max_stack,
                }),
            },
            Some(char_item_id) => {
                let inventory = Arc::make_mut(&mut self.inventory);
                match inventory.iter_mut().find(|i| i.id == id) {
                    Some(held) => held.qty = added.qty_now.unwrap_or(held.qty + added.qty),
                    None => inventory.push(Item {
                        id,
                        char_item_id,
                        name: added.name,
                        qty: added.qty_now.unwrap_or(added.qty),
                        max_stack: added.max_stack,
                        kind: added.kind,
                        slot: added.slot,
                        equipped: false,
                        coins: added.coins,
                        member: added.member,
                    }),
                }
            }
        }
    }

    fn offer_drop(&mut self, key: &str, offered: AddedItem) {
        let Some(id) = offered.id.or_else(|| key.parse().ok().map(ItemId)) else {
            return;
        };
        let qty = offered.qty.max(1);
        match self.drops.iter_mut().find(|d| d.id == id) {
            Some(offer) => offer.qty += qty,
            None => self.drops.push(DropOffer {
                id,
                name: offered.name,
                qty,
                max_stack: offered.max_stack,
                kind: offered.kind,
                slot: offered.slot,
                coins: offered.coins,
                member: offered.member,
            }),
        }
    }

    fn pick_drop(&mut self, picked: Picked) {
        if !picked.ok {
            return;
        }
        let offer = self
            .drops
            .iter()
            .position(|d| d.id == picked.id)
            .map(|at| self.drops.remove(at));
        if picked.bank {
            return;
        }
        let inventory = Arc::make_mut(&mut self.inventory);
        if let Some(held) = inventory.iter_mut().find(|i| i.id == picked.id) {
            held.qty = picked.qty_now.unwrap_or(held.qty + picked.qty);
        } else if let (Some(offer), Some(char_item_id)) = (offer, picked.char_item_id) {
            inventory.push(Item {
                id: offer.id,
                char_item_id,
                name: offer.name,
                qty: picked.qty_now.unwrap_or(picked.qty),
                max_stack: offer.max_stack,
                kind: offer.kind,
                slot: offer.slot,
                equipped: false,
                coins: offer.coins,
                member: offer.member,
            });
        }
    }

    fn consume(&mut self, id: ItemId, qty: u32) {
        if let Some(at) = self.inventory.iter().position(|i| i.id == id) {
            take(Arc::make_mut(&mut self.inventory), at, qty);
        } else if let Some(at) = self.temp.iter().position(|i| i.id == id) {
            self.temp[at].qty = self.temp[at].qty.saturating_sub(qty);
            if self.temp[at].qty == 0 {
                self.temp.remove(at);
            }
        }
    }

    fn patch_player(&mut self, name: &str, patch: &Vitals) {
        if let Some(character) = &mut self.character
            && name.eq_ignore_ascii_case(&self.username)
        {
            character.apply(patch);
        }
    }

    fn patch_monster(&mut self, id: MonMapId, patch: &Vitals) {
        let room = self.arriving.as_mut().or(self.room.as_mut());
        if let Some(monster) = room.and_then(|room| room.monsters.iter_mut().find(|m| m.id == id)) {
            monster.apply(patch);
        }
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
    #[serde(rename = "mondef", default)]
    monster_types: Vec<MonsterType>,
    #[serde(rename = "monmap", default)]
    monster_spots: Vec<MonsterSpot>,
    #[serde(rename = "monBranch", default)]
    monsters: Vec<MonsterBranch>,
}

#[derive(Deserialize)]
struct MonsterType {
    #[serde(rename = "MonID")]
    id: MonId,
    #[serde(rename = "strMonName")]
    name: String,
}

#[derive(Deserialize)]
struct MonsterSpot {
    #[serde(rename = "MonMapID")]
    id: MonMapId,
    #[serde(rename = "strFrame", default)]
    cell: String,
    #[serde(rename = "bRed", default, deserialize_with = "flex::bool")]
    aggressive: bool,
}

#[derive(Deserialize)]
struct MonsterBranch {
    #[serde(rename = "MonMapID")]
    id: MonMapId,
    #[serde(rename = "MonID")]
    kind: MonId,
    #[serde(rename = "intHP", deserialize_with = "flex::int")]
    hp: u32,
    #[serde(rename = "intHPMax", deserialize_with = "flex::int")]
    max_hp: u32,
    #[serde(rename = "intState")]
    state: CombatState,
}

#[derive(Deserialize)]
struct Vitals {
    #[serde(rename = "intHP", default, deserialize_with = "flex::opt_int")]
    hp: Option<u32>,
    #[serde(rename = "intHPMax", default, deserialize_with = "flex::opt_int")]
    max_hp: Option<u32>,
    #[serde(rename = "intMP", default, deserialize_with = "flex::opt_int")]
    mp: Option<u32>,
    #[serde(rename = "intMPMax", default, deserialize_with = "flex::opt_int")]
    max_mp: Option<u32>,
    #[serde(rename = "intState", default)]
    state: Option<CombatState>,
}

#[derive(Deserialize)]
struct MonsterUpdate {
    id: MonMapId,
    o: Vitals,
}

#[derive(Deserialize)]
struct PlayerUpdate {
    #[serde(rename = "unm")]
    name: String,
    o: Vitals,
}

#[derive(Deserialize)]
struct Tick {
    #[serde(rename = "m", default)]
    monsters: HashMap<MonMapId, Vitals>,
    #[serde(rename = "p", default)]
    players: HashMap<String, Vitals>,
}

#[derive(Deserialize)]
struct Stats {
    #[serde(default)]
    sta: StatValues,
}

#[derive(Deserialize, Default)]
struct StatValues {
    #[serde(rename = "$tha")]
    haste: Option<f64>,
}

#[derive(Deserialize)]
struct SkillList {
    actions: Actions,
}

#[derive(Deserialize)]
struct Actions {
    #[serde(default)]
    active: Vec<Skill>,
}

fn room_monsters(
    types: Vec<MonsterType>,
    spots: Vec<MonsterSpot>,
    monsters: Vec<MonsterBranch>,
) -> Vec<Monster> {
    let names: HashMap<MonId, String> = types.into_iter().map(|t| (t.id, t.name)).collect();
    let spots: HashMap<MonMapId, MonsterSpot> = spots.into_iter().map(|s| (s.id, s)).collect();
    monsters
        .into_iter()
        .filter_map(|m| {
            let spot = spots.get(&m.id)?;
            Some(Monster {
                id: m.id,
                name: names.get(&m.kind).cloned().unwrap_or_default(),
                cell: spot.cell.clone(),
                hp: m.hp,
                max_hp: m.max_hp,
                state: m.state,
                aggressive: spot.aggressive,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct AreaPlayer {
    #[serde(rename = "entID")]
    eid: EntityId,
    #[serde(rename = "strUsername")]
    name: String,
    #[serde(rename = "strFrame", default)]
    cell: String,
    #[serde(flatten)]
    vitals: Vitals,
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
    #[serde(rename = "intCoins", default, deserialize_with = "flex::int")]
    coins: u64,
    #[serde(rename = "intExp", default, deserialize_with = "flex::int")]
    exp: u64,
    #[serde(rename = "intExpToLevel", default, deserialize_with = "flex::int")]
    exp_to_level: u64,
    #[serde(rename = "iUpgDays", default, deserialize_with = "flex::int")]
    member_days: i64,
    #[serde(rename = "iCP", default, deserialize_with = "flex::int")]
    class_points: u32,
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct QuestUpdate {
    #[serde(rename = "iIndex", deserialize_with = "flex::int")]
    slot: u32,
    #[serde(rename = "iValue", deserialize_with = "flex::int")]
    value: i64,
}

#[derive(Deserialize)]
struct AchievementUpdate {
    field: String,
    #[serde(deserialize_with = "flex::int")]
    index: u32,
    #[serde(deserialize_with = "flex::int")]
    value: u8,
}

#[derive(Deserialize)]
struct LoadInventory {
    #[serde(default)]
    items: Vec<Value>,
    #[serde(default)]
    factions: Vec<Value>,
}

#[derive(Deserialize)]
struct FactionData {
    #[serde(rename = "FactionID")]
    id: FactionId,
    #[serde(rename = "sName")]
    name: String,
    #[serde(rename = "iRep", default, deserialize_with = "flex::int")]
    rep: u32,
}

impl FactionData {
    fn into_faction(self) -> Faction {
        Faction::new(self.id, self.name, self.rep)
    }
}

#[derive(Deserialize)]
struct AddedFaction {
    faction: FactionData,
}

#[derive(Deserialize)]
struct AddedItems {
    items: BTreeMap<String, AddedItem>,
}

#[derive(Deserialize)]
struct AddedItem {
    #[serde(rename = "ItemID", default)]
    id: Option<ItemId>,
    #[serde(rename = "CharItemID", default)]
    char_item_id: Option<CharItemId>,
    #[serde(rename = "sName", default)]
    name: String,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iQtyNow", default, deserialize_with = "flex::opt_int")]
    qty_now: Option<u32>,
    #[serde(rename = "iStk", default, deserialize_with = "flex::int")]
    max_stack: u32,
    #[serde(rename = "sType", default)]
    kind: String,
    #[serde(rename = "sES", default)]
    slot: String,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    coins: bool,
    #[serde(rename = "bUpg", default, deserialize_with = "flex::bool")]
    member: bool,
    #[serde(rename = "bBank", default, deserialize_with = "flex::bool")]
    bank: bool,
    #[serde(rename = "bTemp", default, deserialize_with = "flex::bool")]
    temp: bool,
}

#[derive(Deserialize)]
struct TurnIn {
    #[serde(rename = "sItems", default)]
    items: String,
}

#[derive(Deserialize)]
struct Sold {
    #[serde(rename = "CharItemID")]
    char_item_id: CharItemId,
    #[serde(rename = "iQty", deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "intAmount", default, deserialize_with = "flex::int")]
    amount: u64,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    coins: bool,
}

#[derive(Deserialize)]
struct Reward {
    #[serde(rename = "intGold", default, deserialize_with = "flex::int")]
    gold: u64,
    #[serde(rename = "intExp", default, deserialize_with = "flex::int")]
    exp: u64,
    #[serde(rename = "FactionID", default)]
    faction: Option<FactionId>,
    #[serde(rename = "iRep", default, deserialize_with = "flex::int")]
    rep: i64,
    #[serde(rename = "iCP", default, deserialize_with = "flex::opt_int")]
    class_points: Option<u32>,
}

#[derive(Deserialize)]
struct ClassChange {
    uid: EntityId,
    #[serde(rename = "iCP", deserialize_with = "flex::int")]
    class_points: u32,
}

#[derive(Deserialize)]
struct LevelUp {
    #[serde(rename = "intLevel", deserialize_with = "flex::int")]
    level: u32,
    #[serde(rename = "xp", default, deserialize_with = "flex::int")]
    exp: u64,
    #[serde(rename = "intExpToLevel", default, deserialize_with = "flex::int")]
    exp_to_level: u64,
}

#[derive(Deserialize)]
struct Picked {
    #[serde(rename = "bSuccess", deserialize_with = "flex::bool")]
    ok: bool,
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "CharItemID", default)]
    char_item_id: Option<CharItemId>,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iQtyNow", default, deserialize_with = "flex::opt_int")]
    qty_now: Option<u32>,
    #[serde(rename = "bBank", default, deserialize_with = "flex::bool")]
    bank: bool,
}

#[derive(Deserialize)]
struct Worn {
    uid: EntityId,
    #[serde(rename = "ItemID")]
    id: ItemId,
}

fn take(items: &mut Vec<Item>, at: usize, qty: u32) {
    items[at].qty = items[at].qty.saturating_sub(qty);
    if items[at].qty == 0 || items[at].kind == "Class" {
        items.remove(at);
    }
}

#[cfg(test)]
mod tests;
