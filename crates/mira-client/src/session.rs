use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task;
use std::thread;
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Waker};
use mira_protocol::packet::Aim;
use mira_protocol::{
    EntityId, Framer, ItemId, JsonMessage, Message, MonMapId, QuestId, XmlMessage, XmlUser,
    XtMessage, packet,
};
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::REPLY_TIMEOUT;
use crate::api::{Account, Server, Token};
use crate::aura::{self, AuraChange, Target};
use crate::combat::{self, AUTO_ATTACK, ActionResult, Combat, Gate, ITEM_SLOT, Outcome};
use crate::event::{Direction, Event, NoticeKind};
use crate::faction::{self, AddedFaction, Faction, FactionData};
use crate::inventory::{AddedItems, Inventory, LoadInventory, Picked, Pool, Sold, TurnIn, Worn};
use crate::player::{
    BoostUpdate, ClassChange, InitUserData, LevelUp, Me, Reward, Skill, SkillList, SkillTarget,
    Stats,
};
use crate::quest::{self, AchievementUpdate, Quest, QuestList, QuestUpdate};
use crate::room::{
    CombatState, Monster, MonsterUpdate, MoveToArea, PlayerUpdate, Room, SPAWN, Tick, UserEntry,
    UserList, Vitals,
};
use crate::world::World;
use quests::{Answer, QuestAction, QuestCall, Verdict};

mod quests;

pub const DEFAULT_CLIENT_VERSION: &str = "5.31";

const SOCKET: mio::Token = mio::Token(0);
const WAKER: mio::Token = mio::Token(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const CELL_PAD: &str = "Left";
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
    #[error("the server did not answer")]
    NoReply,
    #[error("{0}")]
    NoTarget(String),
    #[error("{0}")]
    Unusable(String),
    #[error("the player is dead")]
    Dead,
    #[error("the server ignores this during combat; leave combat first")]
    InCombat,
    #[error("stopped to leave combat")]
    Interrupted,
    #[error("still in combat {secs} s after moving to {0}", secs = EXIT_TIMEOUT.as_secs())]
    StillInCombat(String),
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

#[derive(Debug, Clone, Copy)]
enum Limit {
    Transfer,
    LoadQuests,
    AcceptQuest,
    TurnIn,
}

impl Limit {
    const COUNT: usize = 4;

    fn gap(self) -> Duration {
        Duration::from_millis(match self {
            Limit::Transfer => 3000,
            Limit::LoadQuests => 1000,
            Limit::AcceptQuest => 1000,
            Limit::TurnIn => 1250,
        })
    }
}

#[derive(Default)]
struct Limits([Option<Instant>; Limit::COUNT]);

impl Limits {
    fn until(&self, limit: Limit) -> Option<Instant> {
        self.0[limit as usize].map(|at| at + limit.gap())
    }

    fn used(&mut self, limit: Limit, now: Instant) {
        self.0[limit as usize] = Some(now);
    }
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
    Join {
        map: String,
    },
    MoveToCell {
        cell: String,
        pad: String,
    },
    Attack {
        target: MonMapId,
    },
    UseSkill {
        slot: String,
        target: Option<MonMapId>,
    },
    Kill {
        target: MonMapId,
    },
    ExitCombat,
    Quest(QuestAction),
    PickDrop(ItemId),
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
    Cast {
        reply: Reply,
        slot: String,
        target: Option<MonMapId>,
        stage: CastStage,
    },
    Kill {
        reply: Reply,
        target: MonMapId,
        wake: Option<Instant>,
        next: usize,
    },
    Leave {
        reply: Reply,
        cell: String,
        deadline: Instant,
    },
    Quest {
        reply: Reply,
        call: QuestCall,
    },
    Pick {
        reply: Reply,
        item: ItemId,
        deadline: Instant,
    },
}

enum CastStage {
    Waiting { wake: Option<Instant> },
    Sent { act: u8 },
}

enum Prepared {
    Wait(Option<Instant>),
    Fire {
        aims: Vec<Aim>,
        hostile: Option<MonMapId>,
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

    pub fn attack(&self, target: MonMapId) -> Pending {
        self.act(Action::Attack { target })
    }

    pub fn use_skill(&self, slot: &str, target: Option<MonMapId>) -> Pending {
        self.act(Action::UseSkill {
            slot: slot.to_owned(),
            target,
        })
    }

    pub fn kill(&self, target: MonMapId) -> Pending {
        self.act(Action::Kill { target })
    }

    pub fn exit_combat(&self) -> Pending {
        self.act(Action::ExitCombat)
    }

    pub fn load_quests(&self, ids: &[QuestId]) -> Pending {
        self.act(Action::Quest(QuestAction::Load(ids.to_vec())))
    }

    pub fn accept_quest(&self, id: QuestId) -> Pending {
        self.act(Action::Quest(QuestAction::Accept(id)))
    }

    pub fn pick_drop(&self, item: ItemId) -> Pending {
        self.act(Action::PickDrop(item))
    }

    pub fn complete_quest(&self, id: QuestId, reward: Option<ItemId>, times: u32) -> Pending {
        self.act(Action::Quest(QuestAction::TurnIn { id, reward, times }))
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
        limits: Limits::default(),
        refusal: None,
        character: None,
        inventory: Inventory::default(),
        factions: Vec::new(),
        quests: Arc::default(),
        accepted_quests: BTreeSet::new(),
        combat: Combat::default(),
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
    limits: Limits,
    refusal: Option<String>,
    character: Option<Me>,
    inventory: Inventory,
    factions: Vec<Faction>,
    quests: Arc<BTreeMap<QuestId, Quest>>,
    accepted_quests: BTreeSet<QuestId>,
    combat: Combat,
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
                Request::Act(Action::ExitCombat, reply) => {
                    if matches!(self.current, Some(Op::Kill { .. } | Op::Cast { .. }))
                        && let Some(Op::Kill { reply, .. } | Op::Cast { reply, .. }) =
                            self.current.take()
                    {
                        self.replies.push((reply, Err(ActionError::Interrupted)));
                    }
                    let (fights, rest): (Vec<_>, Vec<_>) =
                        self.queue.drain(..).partition(|(action, _)| {
                            matches!(
                                action,
                                Action::Attack { .. }
                                    | Action::UseSkill { .. }
                                    | Action::Kill { .. }
                            )
                        });
                    for (_, reply) in fights {
                        self.replies.push((reply, Err(ActionError::Interrupted)));
                    }
                    self.queue = rest.into();
                    self.queue.push_front((Action::ExitCombat, reply));
                }
                Request::Act(action, reply) => self.queue.push_back((action, reply)),
            }
        }
    }

    fn advance(&mut self, now: Instant) {
        if self.logout_deadline.is_some_and(|d| now >= d) {
            self.close(Disconnect::LoggedOut);
        }
        if let Some(me) = &self.character {
            self.combat.expire(me, now);
        }
        self.run_ops(now);
        self.tick_combat(now);
    }

    fn run_ops(&mut self, now: Instant) {
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
            Action::Attack { target } => self.attack(target, now),
            Action::ExitCombat => match self.start_leaving(now) {
                Ok(Some((cell, deadline))) => {
                    return Some(Op::Leave {
                        reply,
                        cell,
                        deadline,
                    });
                }
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
            Action::PickDrop(item) => match self.start_pick(item, now) {
                Ok(deadline) => {
                    return Some(Op::Pick {
                        reply,
                        item,
                        deadline,
                    });
                }
                Err(e) => Err(e),
            },
            Action::Quest(action) => {
                let call = self.start_quest(action);
                return self.step(Op::Quest { reply, call }, now);
            }
            Action::UseSkill { slot, target } => {
                let stage = CastStage::Waiting { wake: None };
                return self.step(
                    Op::Cast {
                        reply,
                        slot,
                        target,
                        stage,
                    },
                    now,
                );
            }
            Action::Kill { target } => {
                let (wake, next) = (None, 0);
                return self.step(
                    Op::Kill {
                        reply,
                        target,
                        wake,
                        next,
                    },
                    now,
                );
            }
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
                stage: JoinStage::Cooling { map, .. },
            } => {
                let state = self.character.as_ref().map(|me| me.state);
                let refused = match (self.ensure_ready(), state) {
                    (Err(e), _) => Some(e),
                    (Ok(()), Some(CombatState::Dead)) => Some(ActionError::Dead),
                    (Ok(()), Some(CombatState::InCombat)) => Some(ActionError::InCombat),
                    (Ok(()), _) => None,
                };
                if let Some(e) = refused {
                    self.replies.push((reply, Err(e)));
                    return None;
                }
                if let Some(until) = self.cooling(Limit::Transfer, now) {
                    return Some(Op::Join {
                        reply,
                        stage: JoinStage::Cooling { map, until },
                    });
                }
                let joins = self.joins;
                self.refusal = None;
                self.send(&packet::transfer(&self.username, &map));
                self.limits.used(Limit::Transfer, now);
                Some(Op::Join {
                    reply,
                    stage: JoinStage::Sent {
                        joins,
                        deadline: now + REPLY_TIMEOUT,
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
            Op::Cast {
                reply,
                slot,
                target,
                stage,
            } => match self.step_cast(&slot, target, stage, now) {
                Ok(Some(stage)) => Some(Op::Cast {
                    reply,
                    slot,
                    target,
                    stage,
                }),
                Ok(None) => {
                    self.replies.push((reply, Ok(())));
                    None
                }
                Err(e) => {
                    self.replies.push((reply, Err(e)));
                    None
                }
            },
            Op::Kill {
                reply,
                target,
                next,
                ..
            } => match self.step_kill(target, next, now) {
                Ok(Some((wake, next))) => Some(Op::Kill {
                    reply,
                    target,
                    wake,
                    next,
                }),
                Ok(None) => {
                    self.replies.push((reply, Ok(())));
                    None
                }
                Err(e) => {
                    self.replies.push((reply, Err(e)));
                    None
                }
            },
            Op::Leave {
                reply,
                cell,
                deadline,
            } => {
                let state = self.character.as_ref().map(|me| me.state);
                let result = match (self.ensure_ready(), state) {
                    (Err(e), _) => Err(e),
                    (Ok(()), Some(CombatState::Dead)) => Err(ActionError::Dead),
                    (Ok(()), Some(CombatState::InCombat)) if now < deadline => {
                        return Some(Op::Leave {
                            reply,
                            cell,
                            deadline,
                        });
                    }
                    (Ok(()), Some(CombatState::InCombat)) => Err(ActionError::StillInCombat(cell)),
                    (Ok(()), _) => Ok(()),
                };
                self.replies.push((reply, result));
                None
            }
            Op::Pick {
                reply,
                item,
                deadline,
            } => {
                let result = match self.ensure_ready() {
                    Err(e) => Err(e),
                    Ok(()) if now >= deadline => Err(ActionError::NoReply),
                    Ok(()) => {
                        return Some(Op::Pick {
                            reply,
                            item,
                            deadline,
                        });
                    }
                };
                self.replies.push((reply, result));
                None
            }
            Op::Quest { reply, mut call } => match self.step_quest(&mut call, now) {
                task::Poll::Pending => Some(Op::Quest { reply, call }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let op = match &self.current {
            Some(Op::Login { deadline, .. })
            | Some(Op::Leave { deadline, .. })
            | Some(Op::Pick { deadline, .. })
            | Some(Op::Join {
                stage: JoinStage::Sent { deadline, .. },
                ..
            }) => Some(*deadline),
            Some(Op::Join {
                stage: JoinStage::Cooling { until, .. },
                ..
            }) => Some(*until),
            Some(Op::Cast {
                stage: CastStage::Waiting { wake },
                ..
            })
            | Some(Op::Kill { wake, .. }) => *wake,
            Some(Op::Quest { call, .. }) => call.stage.deadline(),
            Some(Op::Cast {
                stage: CastStage::Sent { .. },
                ..
            })
            | None => None,
        };
        op.into_iter()
            .chain(self.logout_deadline)
            .chain(
                self.ensure_ready()
                    .ok()
                    .and_then(|()| self.combat.next_deadline(Instant::now())),
            )
            .min()
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
            accepted_quests: self.accepted_quests.clone(),
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
        let until = self.cooling(Limit::Transfer, now).unwrap_or(now);
        Ok(Some(JoinStage::Cooling { map, until }))
    }

    fn cooling(&self, limit: Limit, now: Instant) -> Option<Instant> {
        let until = self.limits.until(limit)? + self.combat.backoff(now);
        (now < until).then_some(until)
    }

    fn move_to_cell(&mut self, cell: &str, pad: &str) -> Result<(), ActionError> {
        self.ensure_ready()?;
        let Some(room) = &mut self.room else {
            return Err(ActionError::NotReady);
        };
        room.cell = cell.to_owned();
        room.pad = pad.to_owned();
        let area = room.area;
        self.combat.disengage();
        self.send(&packet::move_to_cell(area, cell, pad));
        self.fetch_cellmates();
        self.aggro_cell();
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
        self.limits = Limits::default();
        self.refusal = None;
        self.character = None;
        self.inventory = Inventory::default();
        self.factions.clear();
        self.quests = Arc::default();
        self.accepted_quests.clear();
        self.combat = Combat::default();
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
        self.combat.sent(Instant::now());
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
                self.combat.warned(&text, Instant::now());
                if combat::slow_down(&text) {
                    self.answer(Answer::SlowedDown);
                }
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
            "zm" if xt.field(3) == Some("tryQuestComplete") => self.answer(Answer::TurnInEchoed),
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
            "respawnMon" => {
                for id in text().split(',').filter_map(|id| id.parse().ok()) {
                    self.aggro_respawned(MonMapId(id));
                }
            }
            "resTimed" => {
                let (cell, pad) = match (xt.field(4), xt.field(5)) {
                    (Some(cell), Some(pad)) if !cell.is_empty() && !pad.is_empty() => {
                        (cell.to_owned(), pad.to_owned())
                    }
                    _ => (SPAWN.0.to_owned(), SPAWN.1.to_owned()),
                };
                let _ = self.move_to_cell(&cell, &pad);
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
            "initUserData" => {
                let entry: UserEntry = read(&json)?;
                if Some(entry.uid) != self.me {
                    self.learn_player(&entry);
                    return Ok(());
                }
                if self.phase
                    != (Phase::Loading {
                        step: Load::Character,
                    })
                {
                    return Ok(());
                }
                let user: InitUserData = read(&json)?;
                let (Some(me), Some(room)) = (self.me, &self.room) else {
                    return Ok(());
                };
                if user.uid == me {
                    let room = room.area;
                    if let Some(character) = &mut self.character {
                        character.load(&user.data, Instant::now());
                    }
                    self.send(&packet::retrieve_inventory(room, me));
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                }
            }
            "initUserDatas" => {
                for entry in read::<UserList>(&json)?.a {
                    self.learn_player(&entry);
                }
            }
            "xpboost" | "gboost" | "repboost" | "cpboost" => {
                let update: BoostUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.boosts.apply(&json.cmd, &update, Instant::now());
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
                for result in &tick.sarsa {
                    self.settle(result);
                }
            }
            "sars" => self.settle(&read(&json)?),
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
                if let Some(character) = &mut self.character {
                    character.haste = stats.sta.haste.unwrap_or(character.haste);
                    character.mana_rate = stats.sta.mana_rate.or(character.mana_rate);
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
                    if let Some(event) = self.inventory.add(&key, item) {
                        self.emit(event);
                    }
                }
            }
            "dropItem" => {
                for (key, item) in read::<AddedItems>(&json)?.items {
                    if let Some(event) = self.inventory.offer(&key, item) {
                        self.emit(event);
                    }
                }
            }
            "getDrop" => {
                let picked: Picked = read(&json)?;
                let (id, ok) = (picked.id, picked.ok);
                if let Some(event) = self.inventory.pick(picked) {
                    self.emit(event);
                }
                self.picked(id, ok);
            }
            "turnIn" => {
                for event in self.inventory.turn_in(&read::<TurnIn>(&json)?) {
                    self.emit(event);
                }
            }
            "sellItem" => {
                let sold: Sold = read(&json)?;
                if let Some(event) = self.inventory.sell(&sold) {
                    self.emit(event);
                }
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
                if let (Some(id), "m") = (reward.id, reward.source.as_str()) {
                    let name = self
                        .arriving
                        .as_ref()
                        .or(self.room.as_ref())
                        .and_then(|room| room.monsters.iter().find(|m| m.id == id))
                        .map(|m| m.name.clone())
                        .unwrap_or_default();
                    self.emit(Event::Killed {
                        id,
                        name,
                        gold: reward.gold,
                        exp: reward.exp,
                    });
                }
            }
            "levelUp" => {
                let level: LevelUp = read(&json)?;
                if let Some(character) = &mut self.character {
                    let level = character.level_up(&level);
                    self.emit(Event::LeveledUp { level });
                }
            }
            "updateClass" => {
                let class: ClassChange = read(&json)?;
                if Some(class.uid) == self.me {
                    if let Some(character) = &mut self.character {
                        character.change_class(&class);
                    }
                } else if let Some(player) = self.room.as_mut().and_then(|r| r.player(class.uid)) {
                    player.class_name = Some(class.class_name);
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
                self.answer(Answer::Quests);
            }
            "acceptQuest" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok {
                    self.accepted_quests.insert(verdict.id);
                }
                self.answer(Answer::Accepted(verdict));
            }
            "ccqr" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok {
                    self.accepted_quests.remove(&verdict.id);
                }
                self.answer(Answer::TurnedIn(verdict));
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
        self.combat.disengage();
        self.fetch_cellmates();
        self.aggro_cell();
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

    fn aggro_cell(&mut self) {
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        if me.state == CombatState::Dead {
            return;
        }
        let ids: Vec<MonMapId> = room
            .monsters
            .iter()
            .filter(|m| m.aggressive && m.cell == room.cell && m.alive())
            .map(|m| m.id)
            .collect();
        if !ids.is_empty() {
            let area = room.area;
            self.send(&packet::aggro_mon(area, &ids));
        }
    }

    fn aggro_respawned(&mut self, id: MonMapId) {
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        let here = room
            .monsters
            .iter()
            .any(|m| m.id == id && m.aggressive && m.cell == room.cell);
        if here && me.state != CombatState::Dead {
            let area = room.area;
            self.send(&packet::aggro_mon(area, &[id]));
        }
    }

    fn attack(&mut self, target: MonMapId, now: Instant) -> Result<(), ActionError> {
        self.ensure_ready()?;
        if self
            .character
            .as_ref()
            .is_some_and(|me| me.state == CombatState::Dead)
        {
            return Err(ActionError::Dead);
        }
        let target = self.pick_target(Some(target))?;
        self.combat.engage(target, now);
        Ok(())
    }

    fn start_pick(&mut self, item: ItemId, now: Instant) -> Result<Instant, ActionError> {
        self.ensure_ready()?;
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        let Some(offer) = self.inventory.drops.iter().find(|d| d.id == item) else {
            return Err(ActionError::NoTarget(format!("item {item} isn't on offer")));
        };
        if self.inventory.full_for(offer, me.bag_slots) {
            let pool = match offer.pool() {
                Pool::Misc => "misc inventory",
                _ => "inventory",
            };
            return Err(ActionError::Unusable(format!(
                "the {pool} has no room for {}",
                offer.name
            )));
        }
        let area = room.area;
        self.send(&packet::get_drop(area, item));
        Ok(now + REPLY_TIMEOUT)
    }

    fn picked(&mut self, id: ItemId, ok: bool) {
        if !matches!(&self.current, Some(Op::Pick { item, .. }) if *item == id) {
            return;
        }
        let Some(Op::Pick { reply, .. }) = self.current.take() else {
            return;
        };
        let result = if ok {
            Ok(())
        } else {
            Err(ActionError::Refused(
                "the item couldn't be added; the inventory may be full or already hold it".into(),
            ))
        };
        self.replies.push((reply, result));
    }

    fn start_leaving(&mut self, now: Instant) -> Result<Option<(String, Instant)>, ActionError> {
        self.ensure_ready()?;
        self.combat.disengage();
        if self
            .character
            .as_ref()
            .is_none_or(|me| me.state != CombatState::InCombat)
        {
            return Ok(None);
        }
        let Some((cell, pad)) = self.calm_cell() else {
            return Err(ActionError::Unusable(
                "every known cell in this map has aggressive monsters".into(),
            ));
        };
        self.move_to_cell(&cell, &pad)?;
        Ok(Some((cell, now + EXIT_TIMEOUT)))
    }

    fn calm_cell(&self) -> Option<(String, String)> {
        let room = self.room.as_ref()?;
        let calm = |cell: &str| {
            !room
                .monsters
                .iter()
                .any(|m| m.cell == cell && m.aggressive && m.alive())
        };
        if calm(&room.cell) {
            return Some((room.cell.clone(), room.pad.clone()));
        }
        let cell = std::iter::once(SPAWN.0)
            .chain(room.monsters.iter().map(|m| m.cell.as_str()))
            .find(|cell| calm(cell))?;
        let pad = if cell == SPAWN.0 { SPAWN.1 } else { CELL_PAD };
        Some((cell.to_owned(), pad.to_owned()))
    }

    fn pick_target(&self, wanted: Option<MonMapId>) -> Result<MonMapId, ActionError> {
        let Some(room) = &self.room else {
            return Err(ActionError::NotReady);
        };
        let Some(id) = wanted else {
            let alive_here = |m: &&Monster| m.cell == room.cell && m.alive();
            if let Some(id) = self
                .combat
                .auto_target()
                .filter(|id| room.monsters.iter().filter(alive_here).any(|m| m.id == *id))
            {
                return Ok(id);
            }
            return room
                .monsters
                .iter()
                .filter(alive_here)
                .min_by_key(|m| m.hp)
                .map(|m| m.id)
                .ok_or_else(|| ActionError::NoTarget("no live monster in this cell".into()));
        };
        let Some(monster) = room.monsters.iter().find(|m| m.id == id) else {
            return Err(ActionError::NoTarget(format!(
                "no monster #{id} in this map"
            )));
        };
        if monster.cell != room.cell {
            return Err(ActionError::NoTarget(format!(
                "{} #{id} is in {}, not this cell",
                monster.name, monster.cell
            )));
        }
        if !monster.alive() {
            return Err(ActionError::NoTarget(format!(
                "{} #{id} is dead",
                monster.name
            )));
        }
        Ok(id)
    }

    fn step_cast(
        &mut self,
        slot: &str,
        target: Option<MonMapId>,
        stage: CastStage,
        now: Instant,
    ) -> Result<Option<CastStage>, ActionError> {
        self.ensure_ready()?;
        if let CastStage::Sent { act } = stage {
            return match self.combat.outcome(slot, act) {
                None => Ok(Some(stage)),
                Some(Outcome::Landed) => Ok(None),
                Some(Outcome::Refused) => Err(ActionError::Refused(format!(
                    "the server refused {}",
                    self.skill_name(slot)
                ))),
                Some(Outcome::Lost) => Err(ActionError::NoReply),
            };
        }
        match self.prepare(slot, target, now)? {
            Prepared::Wait(wake) => Ok(Some(CastStage::Waiting { wake })),
            Prepared::Fire { aims, hostile } => {
                let act = self.fire(slot, &aims, hostile, now);
                Ok(Some(CastStage::Sent { act }))
            }
        }
    }

    fn skill_name(&self, slot: &str) -> String {
        self.character
            .as_ref()
            .and_then(|me| me.skills.iter().find(|s| s.slot == slot))
            .map_or_else(|| slot.to_owned(), |s| s.name.clone())
    }

    fn prepare(
        &self,
        slot: &str,
        target: Option<MonMapId>,
        now: Instant,
    ) -> Result<Prepared, ActionError> {
        let (Some(me), Some(room), Some(eid)) = (&self.character, &self.room, self.me) else {
            return Err(ActionError::NotReady);
        };
        if me.state == CombatState::Dead {
            return Err(ActionError::Dead);
        }
        if slot == AUTO_ATTACK {
            return Err(ActionError::Unusable(
                "the auto attack repeats on its own once it starts; attack a monster instead"
                    .into(),
            ));
        }
        if slot == ITEM_SLOT {
            return Err(ActionError::Unusable(
                "nothing is equipped in the item slot".into(),
            ));
        }
        let Some(skill) = me.skills.iter().find(|s| s.slot == slot) else {
            return Err(ActionError::Unusable(format!("no skill in slot {slot}")));
        };
        let hostile = match skill.target {
            SkillTarget::Hostile => Some(self.pick_target(target)?),
            _ => None,
        };
        match self.combat.gate(me, skill, now) {
            Gate::Locked => Err(ActionError::Unusable(format!(
                "{} is locked at this class rank",
                skill.name
            ))),
            Gate::NoMana => Err(ActionError::Unusable(format!(
                "not enough mana for {}",
                skill.name
            ))),
            Gate::Held => Ok(Prepared::Wait(None)),
            Gate::At(at) => Ok(Prepared::Wait(Some(at))),
            Gate::Open => Ok(Prepared::Fire {
                aims: combat::aims(skill, hostile, eid, room),
                hostile,
            }),
        }
    }

    fn fire(&mut self, slot: &str, aims: &[Aim], hostile: Option<MonMapId>, now: Instant) -> u8 {
        let act = self.combat.cast(slot, now);
        self.send(&packet::gar(act, slot, aims));
        if let Some(target) = hostile {
            self.combat.engage(target, now);
        }
        act
    }

    fn step_kill(
        &mut self,
        target: MonMapId,
        mut next: usize,
        now: Instant,
    ) -> Result<Option<(Option<Instant>, usize)>, ActionError> {
        self.ensure_ready()?;
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        if me.state == CombatState::Dead {
            return Err(ActionError::Dead);
        }
        if room.monsters.iter().any(|m| m.id == target && !m.alive()) {
            return Ok(None);
        }
        let target = self.pick_target(Some(target))?;
        if let (Some((at, slot)), _) = self.rotation(next, now)
            && let Prepared::Fire { aims, hostile } = self.prepare(&slot, Some(target), now)?
        {
            self.fire(&slot, &aims, hostile, now);
            next = at + 1;
        }
        if self.combat.auto_target() != Some(target) && !self.hostile_coming(now) {
            self.combat.engage(target, now);
        }
        Ok(Some((self.rotation(next, now).1, next)))
    }

    fn rotation_skills(&self) -> impl Iterator<Item = &Skill> {
        self.character
            .iter()
            .flat_map(|me| &me.skills)
            .filter(|s| s.slot != AUTO_ATTACK && s.slot != ITEM_SLOT)
    }

    fn rotation(&self, next: usize, now: Instant) -> (Option<(usize, String)>, Option<Instant>) {
        let Some(me) = &self.character else {
            return (None, None);
        };
        let skills: Vec<&Skill> = self.rotation_skills().collect();
        let mut wake = None;
        for at in (0..skills.len()).map(|i| (next + i) % skills.len()) {
            match self.combat.gate(me, skills[at], now) {
                Gate::Open => return (Some((at, skills[at].slot.clone())), None),
                Gate::At(ready) => wake = Some(wake.map_or(ready, |w: Instant| w.min(ready))),
                Gate::Held | Gate::Locked | Gate::NoMana => {}
            }
        }
        (None, wake)
    }

    fn class_skill_due(&self, now: Instant) -> Option<Instant> {
        let me = self.character.as_ref()?;
        let waiting: Vec<&Skill> = match &self.current {
            Some(Op::Kill { .. }) => self.rotation_skills().collect(),
            Some(Op::Cast {
                slot,
                stage: CastStage::Waiting { .. },
                ..
            }) => me.skills.iter().filter(|s| s.slot == *slot).collect(),
            _ => Vec::new(),
        };
        waiting
            .into_iter()
            .filter(|s| self.combat.ready_but_for_gap(me, s, now))
            .filter_map(|s| match self.combat.gate(me, s, now) {
                Gate::At(at) => Some(at),
                _ => None,
            })
            .min()
    }

    fn hostile_coming(&self, now: Instant) -> bool {
        let Some(me) = &self.character else {
            return false;
        };
        self.rotation_skills()
            .filter(|s| s.target == SkillTarget::Hostile)
            .any(|s| matches!(self.combat.gate(me, s, now), Gate::Open | Gate::At(_)))
    }

    fn tick_combat(&mut self, now: Instant) {
        if self.ensure_ready().is_err() {
            return;
        }
        if self.combat.respawn_due(now)
            && let (Some(room), Some(eid)) = (&self.room, self.me)
        {
            let area = room.area;
            self.send(&packet::res_player_timed(area, eid));
        }
        let Some(target) = self.combat.auto_target() else {
            return;
        };
        let (Some(me), Some(room), Some(eid)) = (&self.character, &self.room, self.me) else {
            return;
        };
        let fighting = me.state != CombatState::Dead
            && room
                .monsters
                .iter()
                .any(|m| m.id == target && m.cell == room.cell && m.alive());
        let skill = me.skills.iter().find(|s| s.slot == AUTO_ATTACK);
        let Some(skill) = skill.filter(|_| fighting) else {
            return self.combat.disengage();
        };
        let aims = combat::aims(skill, Some(target), eid, room);
        if self.combat.auto_went_last()
            && let Some(due) = self.class_skill_due(now)
        {
            return self.combat.defer_auto(due);
        }
        if self.combat.auto_ready(me, now) {
            self.fire(AUTO_ATTACK, &aims, None, now);
        }
    }

    fn settle(&mut self, result: &ActionResult) {
        if let (Some(eid), Some(me)) = (self.me, &self.character) {
            self.combat.settle(result, eid, me, Instant::now());
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
        if name.eq_ignore_ascii_case(&self.username) {
            let Some(character) = &mut self.character else {
                return;
            };
            let was = character.state;
            character.apply(patch);
            match (was, character.state) {
                (CombatState::Dead, CombatState::Dead) => {}
                (_, CombatState::Dead) => {
                    self.combat.died(Instant::now());
                    self.emit(Event::Died);
                }
                (CombatState::Dead, _) => {
                    self.combat.revived();
                    self.emit(Event::Respawned);
                }
                _ => {}
            }
        } else if let Some(player) = self.room.as_mut().and_then(|r| r.player_named(name)) {
            player.apply(patch);
        }
    }

    fn learn_player(&mut self, entry: &UserEntry) {
        if let Some(player) = self.room.as_mut().and_then(|r| r.player(entry.uid)) {
            player.learn(&entry.data);
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
