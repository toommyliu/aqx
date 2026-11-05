use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task;
use std::thread;
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Waker};
use mira_protocol::packet::Aim;
use mira_protocol::{
    AreaId, EntityId, Framer, ItemId, JsonMessage, MapItemId, Message, MonMapId, QuestId, ShopId,
    ShopItemId, XmlMessage, XmlUser, XtMessage, packet,
};
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::REPLY_TIMEOUT;
use crate::api::{Account, ApiError, Server, Token};
use crate::aura::{self, AuraChange, AuraDelta, Target};
use crate::combat::{self, AUTO_ATTACK, Combat, ITEM_SLOT};
use crate::event::{Direction, Event, Holder, NoticeKind};
use crate::faction::{self, AddedFaction, Faction, FactionData};
use crate::inventory::{
    AddedItems, Inventory, Item, LoadInventory, Picked, Pool, Sold, TurnIn, Worn,
};
use crate::player::{
    BoostKind, BoostUpdate, ClassChange, InitUserData, LevelUp, Me, Reward, SkillList, StatUpdate,
};
use crate::quest::{self, AchievementUpdate, Quest, QuestList, QuestUpdate};
use crate::room::{
    self, CombatState, Monster, MonsterUpdate, MoveToArea, PlayerUpdate, Room, SPAWN, Spot, Tick,
    UserEntry, UserList, Vitals,
};
use crate::shop::{Bought, LoadedShop, Shop};
use crate::world::World;
use bank::{BankMove, BankStage};
use equip::EquipStage;
use quests::{Answer, QuestAction, QuestCall, Verdict};
use shops::{Trade, TradeStage};

mod bank;
mod equip;
mod fight;
mod quests;
mod reconnect;
mod shops;

pub const DEFAULT_CLIENT_VERSION: &str = "5.31";

const SOCKET: mio::Token = mio::Token(0);
const WAKER: mio::Token = mio::Token(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const CELL_PAD: &str = "Left";
const VERSION_RETRIES: usize = 3;
const WALK_SPEED: u32 = 8;
const AGGRO_EVERY: Duration = Duration::from_millis(500);
const AFK_EVERY: Duration = Duration::from_millis(500);
const BOOST_CHECK: Duration = Duration::from_millis(2167);
pub const DEFAULT_RECONNECT_ATTEMPTS: u32 = 5;
const MAX_RECONNECT_WAIT: Duration = Duration::from_secs(300);
const STAGE_WIDTH: i32 = 960;
const STAGE_HEIGHT: i32 = 500;
const REDACTED_KEYS: [&str; 2] = ["strEmail", "strIP"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Connecting {
        server: String,
        version: String,
    },
    Handshaking,
    LoggingIn,
    Loading {
        step: Load,
    },
    Ready,
    Reconnecting {
        reason: Disconnect,
        attempt: u32,
        wait_ms: u64,
    },
    Closed {
        reason: Disconnect,
    },
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
    Kicked { secs: u64 },
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
            Disconnect::Kicked { secs } => write!(
                f,
                "the server kicked the account; it can log in again in {secs} s"
            ),
            Disconnect::InvalidSession => f.write_str("the server says the session is invalid"),
            Disconnect::Ended => f.write_str("the server ended the session"),
            Disconnect::ConnectionLost(e) => write!(f, "connection lost: {e}"),
            Disconnect::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

impl Disconnect {
    fn retryable(&self) -> bool {
        matches!(
            self,
            Disconnect::ConnectionLost(_)
                | Disconnect::Ended
                | Disconnect::InvalidSession
                | Disconnect::Kicked { .. }
        )
    }

    fn lockout(&self) -> Duration {
        match self {
            Disconnect::Kicked { secs } => Duration::from_secs(*secs),
            _ => Duration::ZERO,
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
    Http(String),
    #[error("{0}")]
    Closed(Disconnect),
    #[error("the session stopped without closing")]
    Stopped,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub client_version: Option<String>,
    pub trace_packets: bool,
    pub reconnect: Option<Reconnect>,
}

#[derive(Clone)]
pub struct Reconnect {
    pub password: String,
    pub delay: Duration,
    pub attempts: u32,
}

impl Reconnect {
    pub fn new(password: String, delay: Duration) -> Reconnect {
        Reconnect {
            password,
            delay,
            attempts: DEFAULT_RECONNECT_ATTEMPTS,
        }
    }

    fn wait(&self, made: u32, lockout: Duration) -> Duration {
        let doubled = self.delay.saturating_mul(2u32.saturating_pow(made));
        let capped = doubled.min(MAX_RECONNECT_WAIT.max(self.delay));
        jitter(capped).max(lockout)
    }
}

impl fmt::Debug for Reconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reconnect")
            .field("delay", &self.delay)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

fn jitter(wait: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    wait.mul_f64(0.8 + f64::from(nanos % 1000) / 1000.0 * 0.4)
}

#[derive(Debug, Clone, Copy)]
enum Limit {
    Transfer,
    LoadQuests,
    AcceptQuest,
    TurnIn,
    LoadShop,
    BuyItem,
    SellItem,
    EquipItem,
    UnequipItem,
    MapItem,
    Rest,
    Walk,
}

impl Limit {
    const COUNT: usize = 12;

    fn gap(self) -> Duration {
        Duration::from_millis(match self {
            Limit::Transfer => 3000,
            Limit::LoadQuests => 1000,
            Limit::AcceptQuest => 1000,
            Limit::TurnIn => 1250,
            Limit::LoadShop => 3000,
            Limit::BuyItem | Limit::SellItem => 1000,
            Limit::EquipItem | Limit::UnequipItem => 1500,
            Limit::MapItem => 1000,
            Limit::Rest => 2000,
            Limit::Walk => 500,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aggro {
    Cell,
    Only(HashSet<MonMapId>),
}

impl Aggro {
    fn covers(&self, id: MonMapId) -> bool {
        match self {
            Aggro::Cell => true,
            Aggro::Only(ids) => ids.contains(&id),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Respawn {
    #[default]
    Spawn,
    WhereDied,
    At {
        cell: String,
        pad: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CounterHold {
    #[default]
    Off,
    All,
    Attacks,
}

enum Setting {
    CounterHold(CounterHold),
    Aggro(Option<Aggro>),
    Respawn(Respawn),
    AntiAfk(bool),
}

enum Request {
    Act(Action, Reply, u64),
    Set(Setting, u64),
    Subscribe(mpsc::Sender<Event>),
    Logout,
}

enum Action {
    Ready,
    Join {
        map: String,
        spot: Option<(String, String)>,
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
    LoadShop(ShopId),
    Trade(Trade),
    Bank(Option<BankMove>),
    Equip {
        item: ItemId,
        on: bool,
    },
    MapItem(MapItemId),
    Rest,
    UseItem(ItemId),
    Goto(String),
    Walk {
        x: i32,
        y: i32,
    },
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
    Shop {
        reply: Reply,
        id: ShopId,
        stage: ShopStage,
    },
    Trade {
        reply: Reply,
        trade: Trade,
        stage: TradeStage,
    },
    Bank {
        reply: Reply,
        mv: Option<BankMove>,
        stage: BankStage,
    },
    Equip {
        reply: Reply,
        item: ItemId,
        on: bool,
        stage: EquipStage,
    },
    MapItem {
        reply: Reply,
        item: MapItemId,
        wake: Option<Instant>,
    },
    Rest {
        reply: Reply,
        stage: RestStage,
    },
    Walk {
        reply: Reply,
        x: i32,
        y: i32,
        wake: Option<Instant>,
    },
}

impl Op {
    fn into_reply(self) -> Option<Reply> {
        match self {
            Op::Login { .. } => None,
            Op::Join { reply, .. }
            | Op::Cast { reply, .. }
            | Op::Kill { reply, .. }
            | Op::Leave { reply, .. }
            | Op::Quest { reply, .. }
            | Op::Pick { reply, .. }
            | Op::Shop { reply, .. }
            | Op::Trade { reply, .. }
            | Op::Bank { reply, .. }
            | Op::Equip { reply, .. }
            | Op::MapItem { reply, .. }
            | Op::Rest { reply, .. }
            | Op::Walk { reply, .. } => Some(reply),
        }
    }
}

enum RestStage {
    Cooling { wake: Option<Instant> },
    Sent { deadline: Instant },
}

type BankResult = (u64, Result<Vec<Value>, ApiError>);

enum ShopStage {
    Waiting { wake: Option<Instant> },
    Sent { deadline: Instant },
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

enum JoinTarget {
    Map {
        map: String,
        spot: Option<(String, String)>,
    },
    Player(String),
}

enum JoinStage {
    Cooling {
        to: JoinTarget,
        until: Instant,
    },
    Sent {
        joins: u64,
        goto: bool,
        deadline: Instant,
    },
}

#[derive(Clone)]
pub struct Handle {
    requests: mpsc::Sender<Request>,
    waker: Arc<Waker>,
    world: Slot,
    epoch: Arc<AtomicU64>,
}

impl Handle {
    pub fn ready(&self) -> Pending {
        self.act(Action::Ready)
    }

    pub fn join(&self, map: &str) -> Pending {
        self.act(Action::Join {
            spot: None,
            map: map.to_owned(),
        })
    }

    pub fn join_at(&self, map: &str, cell: &str, pad: &str) -> Pending {
        self.act(Action::Join {
            map: map.to_owned(),
            spot: Some((cell.to_owned(), pad.to_owned())),
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

    pub fn load_shop(&self, id: ShopId) -> Pending {
        self.act(Action::LoadShop(id))
    }

    pub fn buy(&self, listing: ShopItemId, qty: u32) -> Pending {
        self.act(Action::Trade(Trade::Buy { listing, qty }))
    }

    pub fn sell(&self, item: ItemId, qty: u32) -> Pending {
        self.act(Action::Trade(Trade::Sell { item, qty }))
    }

    pub fn load_bank(&self) -> Pending {
        self.act(Action::Bank(None))
    }

    pub fn walk(&self, x: i32, y: i32) -> Pending {
        self.act(Action::Walk { x, y })
    }

    pub fn goto(&self, player: &str) -> Pending {
        self.act(Action::Goto(player.to_owned()))
    }

    pub fn rest(&self) -> Pending {
        self.act(Action::Rest)
    }

    pub fn use_item(&self, item: ItemId) -> Pending {
        self.act(Action::UseItem(item))
    }

    pub fn get_map_item(&self, item: MapItemId) -> Pending {
        self.act(Action::MapItem(item))
    }

    pub fn equip(&self, item: ItemId) -> Pending {
        self.act(Action::Equip { item, on: true })
    }

    pub fn unequip(&self, item: ItemId) -> Pending {
        self.act(Action::Equip { item, on: false })
    }

    pub fn deposit(&self, item: ItemId) -> Pending {
        self.act(Action::Bank(Some(BankMove::Deposit(item))))
    }

    pub fn withdraw(&self, item: ItemId) -> Pending {
        self.act(Action::Bank(Some(BankMove::Withdraw(item))))
    }

    pub fn swap(&self, inventory: ItemId, bank: ItemId) -> Pending {
        self.act(Action::Bank(Some(BankMove::Swap { inventory, bank })))
    }

    pub fn pick_drop(&self, item: ItemId) -> Pending {
        self.act(Action::PickDrop(item))
    }

    pub fn complete_quest(&self, id: QuestId, reward: Option<ItemId>, times: u32) -> Pending {
        self.act(Action::Quest(QuestAction::TurnIn { id, reward, times }))
    }

    pub fn set_counter_hold(&self, hold: CounterHold) {
        self.set(Setting::CounterHold(hold));
    }

    pub fn set_aggro(&self, aggro: Option<Aggro>) {
        self.set(Setting::Aggro(aggro));
    }

    pub fn set_respawn(&self, respawn: Respawn) {
        self.set(Setting::Respawn(respawn));
    }

    pub fn set_anti_afk(&self, on: bool) {
        self.set(Setting::AntiAfk(on));
    }

    fn set(&self, setting: Setting) {
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.request(Request::Set(setting, epoch));
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (subscriber, events) = mpsc::channel();
        self.request(Request::Subscribe(subscriber));
        events
    }

    pub fn logout(&self) {
        self.request(Request::Logout);
    }

    pub fn snapshot(&self) -> Arc<World> {
        latest(&self.world)
    }

    fn act(&self, action: Action) -> Pending {
        let (reply, result) = mpsc::sync_channel(1);
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.request(Request::Act(action, reply, epoch));
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
    let epoch = Arc::new(AtomicU64::new(0));
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
    let (bank_out, bank_results) = mpsc::channel();
    let (relogin_out, relogin_results) = mpsc::channel();
    let session = Session {
        account: account.clone(),
        waker: Arc::clone(&waker),
        bank_out,
        bank_results,
        relogin_out,
        relogin_results,
        server: server.clone(),
        epoch: Arc::clone(&epoch),
        stale: None,
        ready_once: false,
        reconnects: 0,
        reconnect_at: None,
        lost: None,
        bank: None,
        procs: Arc::default(),
        username: account.username.clone(),
        token: account.token.clone(),
        options,
        version: String::new(),
        poll,
        ready: Events::with_capacity(16),
        requests,
        events: vec![events],
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
        shop: None,
        shop_maps: HashMap::new(),
        requested_spot: None,
        counter_hold: CounterHold::Off,
        aggro: None,
        respawn: Respawn::Spawn,
        died_in: None,
        anti_afk: false,
        afk_cleared: None,
        boost_check: None,
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
        epoch,
    };
    Ok((handle, events_in))
}

struct Session {
    account: Account,
    waker: Arc<Waker>,
    bank_out: mpsc::Sender<BankResult>,
    bank_results: mpsc::Receiver<BankResult>,
    relogin_out: mpsc::Sender<Result<Account, ApiError>>,
    relogin_results: mpsc::Receiver<Result<Account, ApiError>>,
    server: Server,
    epoch: Arc<AtomicU64>,
    stale: Option<Disconnect>,
    ready_once: bool,
    reconnects: u32,
    reconnect_at: Option<Instant>,
    lost: Option<Disconnect>,
    bank: Option<Arc<Vec<Item>>>,
    procs: Arc<HashMap<u32, String>>,
    username: String,
    token: Token,
    options: Options,
    version: String,
    poll: Poll,
    ready: Events,
    requests: mpsc::Receiver<Request>,
    events: Vec<mpsc::Sender<Event>>,
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
    shop: Option<Arc<Shop>>,
    shop_maps: HashMap<ShopId, Vec<String>>,
    requested_spot: Option<Spot>,
    counter_hold: CounterHold,
    aggro: Option<(Aggro, Instant)>,
    respawn: Respawn,
    died_in: Option<(AreaId, String, String)>,
    anti_afk: bool,
    afk_cleared: Option<Instant>,
    boost_check: Option<Instant>,
    combat: Combat,
    closing: Option<Disconnect>,
    logout_deadline: Option<Instant>,
}

impl Session {
    fn run(mut self, server: Server, version: String) {
        self.current = Some(self.start_login(server, version, 1));
        loop {
            self.take_requests();
            self.take_bank_results();
            self.take_relogin_results();
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

    fn take_bank_results(&mut self) {
        while let Ok((epoch, result)) = self.bank_results.try_recv() {
            if epoch != self.epoch.load(Ordering::SeqCst) {
                continue;
            }
            let fetching = matches!(
                self.current,
                Some(Op::Bank {
                    stage: BankStage::Fetching,
                    ..
                })
            );
            if !fetching {
                continue;
            }
            let loaded = self.ensure_ready().and_then(|()| {
                let values = result.map_err(|e| ActionError::Http(e.to_string()))?;
                self.bank = Some(Arc::new(self.parse_each("bank item", values)));
                Ok(())
            });
            match (loaded, &mut self.current) {
                (Ok(()), Some(Op::Bank { stage, .. })) => *stage = BankStage::Idle,
                (Err(e), _) => {
                    if let Some(Op::Bank { reply, .. }) = self.current.take() {
                        self.replies.push((reply, Err(e)));
                    }
                }
                _ => {}
            }
        }
    }

    fn stale_epoch(&self, epoch: u64) -> bool {
        self.lost.is_some() || epoch != self.epoch.load(Ordering::SeqCst)
    }

    fn apply(&mut self, setting: Setting) {
        match setting {
            Setting::CounterHold(hold) => self.counter_hold = hold,
            Setting::Aggro(aggro) => self.aggro = aggro.map(|aggro| (aggro, Instant::now())),
            Setting::Respawn(respawn) => self.respawn = respawn,
            Setting::AntiAfk(on) => self.anti_afk = on,
        }
    }

    fn take_requests(&mut self) {
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Request::Act(_, reply, epoch) if self.stale_epoch(epoch) => {
                    let why = self.lost.clone().or_else(|| self.stale.clone());
                    let why = why.unwrap_or(Disconnect::Ended);
                    self.replies.push((reply, Err(ActionError::Closed(why))));
                }
                Request::Subscribe(subscriber) => self.events.push(subscriber),
                Request::Logout => self.begin_logout(),
                Request::Set(_, epoch) if self.stale_epoch(epoch) => {}
                Request::Set(setting, _) => self.apply(setting),
                Request::Act(Action::ExitCombat, reply, _) => {
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
                Request::Act(action, reply, _) => self.queue.push_back((action, reply)),
            }
        }
    }

    fn advance(&mut self, now: Instant) {
        if self.logout_deadline.is_some_and(|d| now >= d) {
            self.close(Disconnect::LoggedOut);
        }
        if self.reconnect_at.is_some_and(|at| now >= at) {
            self.reconnect_at = None;
            self.relogin();
        }
        if let Some(me) = &self.character {
            self.combat.expire(me, now);
        }
        self.run_ops(now);
        self.tick_combat(now);
        self.tick_aggro(now);
        self.end_boosts(now);
    }

    fn end_boosts(&mut self, now: Instant) {
        let Some(check) = self.boost_check.filter(|check| now >= *check) else {
            return;
        };
        if self.ensure_ready().is_err() {
            return;
        }
        let missed = (now - check).as_nanos() / BOOST_CHECK.as_nanos();
        let skip = u32::try_from(missed + 1).unwrap_or(u32::MAX);
        self.boost_check = Some(check + BOOST_CHECK.saturating_mul(skip));
        let Some(me) = &self.character else {
            return;
        };
        let due: Vec<BoostKind> = me
            .boosts
            .ending()
            .filter(|&(_, at)| now >= at)
            .map(|(kind, _)| kind)
            .collect();
        for kind in due {
            self.send(&packet::end_boost(kind.cmd()));
        }
    }

    fn use_item(&mut self, id: ItemId) -> Result<(), ActionError> {
        self.ensure_ready()?;
        let Some(item) = self.inventory.items.iter().find(|i| i.id == id) else {
            return Err(ActionError::NoTarget(format!(
                "item {id} isn't in the inventory"
            )));
        };
        if !item.kind.eq_ignore_ascii_case("ServerUse") {
            return Err(ActionError::Unusable(format!(
                "{} can't be used",
                item.name
            )));
        }
        self.send(&packet::use_item(id));
        Ok(())
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
            Action::UseItem(item) => self.use_item(item),
            Action::Walk { x, y } => {
                let op = Op::Walk {
                    reply,
                    x,
                    y,
                    wake: None,
                };
                return self.step(op, now);
            }
            Action::Goto(name) => match self.start_goto(name, now) {
                Ok(Some(stage)) => return Some(Op::Join { reply, stage }),
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
            Action::Join { map, spot } => match self.start_join(map, spot, now) {
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
            Action::Rest => {
                let stage = RestStage::Cooling { wake: None };
                return self.step(Op::Rest { reply, stage }, now);
            }
            Action::MapItem(item) => {
                let op = Op::MapItem {
                    reply,
                    item,
                    wake: None,
                };
                return self.step(op, now);
            }
            Action::Equip { item, on } => {
                let stage = EquipStage::Waiting { wake: None };
                return self.step(
                    Op::Equip {
                        reply,
                        item,
                        on,
                        stage,
                    },
                    now,
                );
            }
            Action::Bank(mv) => {
                let stage = BankStage::Idle;
                return self.step(Op::Bank { reply, mv, stage }, now);
            }
            Action::Trade(trade) => {
                let stage = TradeStage::Waiting { wake: None };
                return self.step(
                    Op::Trade {
                        reply,
                        trade,
                        stage,
                    },
                    now,
                );
            }
            Action::LoadShop(id) => {
                let stage = ShopStage::Waiting { wake: None };
                let op = Op::Shop { reply, id, stage };
                return self.step(op, now);
            }
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
                Phase::Closed { .. } | Phase::Reconnecting { .. } => None,
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
                stage: JoinStage::Cooling { to, .. },
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
                        stage: JoinStage::Cooling { to, until },
                    });
                }
                if let JoinTarget::Player(name) = &to
                    && let Some(result) = self.goto_in_room(name)
                {
                    self.replies.push((reply, result));
                    return None;
                }
                let joins = self.joins;
                self.refusal = None;
                let area = self.room.as_ref().map(|room| room.area);
                let transfer = match (&to, area) {
                    (JoinTarget::Player(name), _) => packet::goto(&name.to_lowercase()),
                    (
                        JoinTarget::Map {
                            map,
                            spot: Some((cell, pad)),
                        },
                        Some(area),
                    ) => packet::transfer_to(area, &self.username, map, cell, pad),
                    (JoinTarget::Map { map, .. }, _) => packet::transfer(&self.username, map),
                };
                let goto = matches!(to, JoinTarget::Player(_));
                self.requested_spot = match to {
                    JoinTarget::Map {
                        map,
                        spot: Some((cell, pad)),
                    } => Some(Spot { map, cell, pad }),
                    _ => None,
                };
                self.send(&transfer);
                self.limits.used(Limit::Transfer, now);
                Some(Op::Join {
                    reply,
                    stage: JoinStage::Sent {
                        joins,
                        goto,
                        deadline: now + REPLY_TIMEOUT,
                    },
                })
            }
            Op::Join {
                reply,
                stage:
                    JoinStage::Sent {
                        joins,
                        goto,
                        deadline,
                    },
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
                        stage: JoinStage::Sent {
                            joins,
                            goto,
                            deadline,
                        },
                    });
                };
                if result.is_err() {
                    self.requested_spot = None;
                }
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
            Op::Shop {
                reply,
                id,
                mut stage,
            } => match self.step_shop(id, &mut stage, now) {
                task::Poll::Pending => Some(Op::Shop { reply, id, stage }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
            Op::Walk { reply, x, y, .. } => match self.step_walk(x, y, now) {
                Ok(Some(wake)) => Some(Op::Walk {
                    reply,
                    x,
                    y,
                    wake: Some(wake),
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
            Op::Rest { reply, stage } => {
                let vitals = self.character.as_ref().map(|me| {
                    let hurt = me.hp < me.max_hp || me.mp < me.max_mp;
                    (me.state, hurt)
                });
                let result = match (self.ensure_ready(), vitals, stage) {
                    (Err(e), ..) => Err(e),
                    (Ok(()), None, _) => Err(ActionError::NotReady),
                    (Ok(()), Some((CombatState::Dead, _)), _) => Err(ActionError::Dead),
                    (Ok(()), Some((CombatState::InCombat, _)), _) => Err(ActionError::InCombat),
                    (Ok(()), Some((_, false)), _) => Ok(()),
                    (Ok(()), _, RestStage::Sent { deadline }) if now >= deadline => {
                        Err(ActionError::NoReply)
                    }
                    (Ok(()), _, stage @ RestStage::Sent { .. }) => {
                        return Some(Op::Rest { reply, stage });
                    }
                    (Ok(()), _, RestStage::Cooling { .. }) => {
                        if let Some(until) = self.cooling(Limit::Rest, now) {
                            let stage = RestStage::Cooling { wake: Some(until) };
                            return Some(Op::Rest { reply, stage });
                        }
                        self.limits.used(Limit::Rest, now);
                        self.send(&packet::rest_request());
                        self.send(&packet::emote("rest"));
                        self.clear_afk();
                        match self.ensure_ready() {
                            Ok(()) => {
                                let deadline = now + REPLY_TIMEOUT;
                                let stage = RestStage::Sent { deadline };
                                return Some(Op::Rest { reply, stage });
                            }
                            Err(e) => Err(e),
                        }
                    }
                };
                self.replies.push((reply, result));
                None
            }
            Op::MapItem { reply, item, .. } => {
                let area = self.room.as_ref().map(|room| room.area);
                let result = match (self.ensure_ready(), area) {
                    (Err(e), _) => Err(e),
                    (Ok(()), None) => Err(ActionError::NotReady),
                    (Ok(()), Some(area)) => match self.cooling(Limit::MapItem, now) {
                        Some(until) => {
                            let wake = Some(until);
                            return Some(Op::MapItem { reply, item, wake });
                        }
                        None => {
                            self.limits.used(Limit::MapItem, now);
                            self.send(&packet::get_map_item(area, item));
                            self.ensure_ready()
                        }
                    },
                };
                self.replies.push((reply, result));
                None
            }
            Op::Equip {
                reply,
                item,
                on,
                mut stage,
            } => match self.step_equip(item, on, &mut stage, now) {
                task::Poll::Pending => Some(Op::Equip {
                    reply,
                    item,
                    on,
                    stage,
                }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
            Op::Bank {
                reply,
                mv,
                mut stage,
            } => match self.step_bank(mv, &mut stage, now) {
                task::Poll::Pending => Some(Op::Bank { reply, mv, stage }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
            Op::Trade {
                reply,
                trade,
                mut stage,
            } => match self.step_trade(&trade, &mut stage, now) {
                task::Poll::Pending => Some(Op::Trade {
                    reply,
                    trade,
                    stage,
                }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
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
            Some(Op::Trade { stage, .. }) => stage.deadline(),
            Some(Op::Bank {
                stage: BankStage::Sent { deadline },
                ..
            }) => Some(*deadline),
            Some(Op::Bank { .. }) => None,
            Some(Op::Equip { stage, .. }) => stage.deadline(),
            Some(Op::MapItem { wake, .. })
            | Some(Op::Walk { wake, .. })
            | Some(Op::Rest {
                stage: RestStage::Cooling { wake },
                ..
            }) => *wake,
            Some(Op::Rest {
                stage: RestStage::Sent { deadline },
                ..
            }) => Some(*deadline),
            Some(Op::Shop {
                stage: ShopStage::Waiting { wake },
                ..
            }) => *wake,
            Some(Op::Shop {
                stage: ShopStage::Sent { deadline },
                ..
            }) => Some(*deadline),
            Some(Op::Cast {
                stage: CastStage::Sent { .. },
                ..
            })
            | None => None,
        };
        let counters = self
            .room
            .iter()
            .filter(|_| self.counter_hold != CounterHold::Off)
            .flat_map(|room| &room.monsters)
            .filter_map(Monster::counter_deadline)
            .filter(|until| Instant::now() < *until);
        op.into_iter()
            .chain(counters)
            .chain(self.logout_deadline)
            .chain(self.reconnect_at)
            .chain(self.boost_check.filter(|_| {
                self.ensure_ready().is_ok()
                    && self
                        .character
                        .as_ref()
                        .is_some_and(|me| me.boosts.ending().next().is_some())
            }))
            .chain(self.aggro.as_ref().map(|(_, due)| *due))
            .chain(self.ensure_ready().ok().and_then(|()| {
                let now = Instant::now();
                let auto_held = self
                    .character
                    .iter()
                    .flat_map(|me| &me.skills)
                    .find(|s| s.slot == AUTO_ATTACK)
                    .zip(self.combat.auto_target())
                    .is_some_and(|(skill, target)| {
                        self.held_by_counter(skill, Some(target), now).is_some()
                    });
                self.combat.next_deadline(now, !auto_held)
            }))
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
            shop: self.shop.clone(),
            bank: self.bank.clone(),
            procs: Arc::clone(&self.procs),
            target: self.combat.auto_target(),
        });
        for event in self.outgoing.drain(..) {
            self.events
                .retain(|subscriber| subscriber.send(event.clone()).is_ok());
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

    fn start_join(
        &mut self,
        map: String,
        spot: Option<(String, String)>,
        now: Instant,
    ) -> Result<Option<JoinStage>, ActionError> {
        self.ensure_ready()?;
        if self.room.as_ref().is_some_and(|room| room.is_in(&map)) {
            if let Some((cell, pad)) = spot {
                self.move_to_cell(&cell, &pad)?;
            }
            return Ok(None);
        }
        let until = self.cooling(Limit::Transfer, now).unwrap_or(now);
        let to = JoinTarget::Map { map, spot };
        Ok(Some(JoinStage::Cooling { to, until }))
    }

    fn start_goto(&mut self, name: String, now: Instant) -> Result<Option<JoinStage>, ActionError> {
        self.ensure_ready()?;
        match self.character.as_ref().map(|me| me.state) {
            Some(CombatState::Dead) => return Err(ActionError::Dead),
            Some(CombatState::InCombat) => return Err(ActionError::InCombat),
            _ => {}
        }
        if name.eq_ignore_ascii_case(&self.username) {
            return Err(ActionError::NoTarget("that's this player".into()));
        }
        if let Some(result) = self.goto_in_room(&name) {
            return result.map(|()| None);
        }
        let until = self.cooling(Limit::Transfer, now).unwrap_or(now);
        let to = JoinTarget::Player(name);
        Ok(Some(JoinStage::Cooling { to, until }))
    }

    fn step_walk(&mut self, x: i32, y: i32, now: Instant) -> Result<Option<Instant>, ActionError> {
        self.ensure_ready()?;
        let Some(me) = &self.character else {
            return Err(ActionError::NotReady);
        };
        if me.state == CombatState::Dead {
            return Err(ActionError::Dead);
        }
        if combat::disabled(me) {
            return Err(ActionError::Unusable(
                "the player can't move right now".into(),
            ));
        }
        if !(0..=STAGE_WIDTH).contains(&x) || !(0..=STAGE_HEIGHT).contains(&y) {
            return Err(ActionError::Unusable(format!(
                "({x}, {y}) is off the {STAGE_WIDTH}x{STAGE_HEIGHT} stage"
            )));
        }
        if let Some(until) = self.cooling(Limit::Walk, now) {
            return Ok(Some(until));
        }
        let Some(room) = &mut self.room else {
            return Err(ActionError::NotReady);
        };
        (room.x, room.y) = (x, y);
        let area = room.area;
        self.limits.used(Limit::Walk, now);
        self.send_action(&packet::walk(area, x, y, WALK_SPEED));
        self.ensure_ready().map(|()| None)
    }

    fn goto_in_room(&mut self, name: &str) -> Option<Result<(), ActionError>> {
        let room = self.room.as_ref()?;
        let player = room
            .players
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))?;
        let Some(cell) = player.cell.clone() else {
            return Some(Err(ActionError::Unusable(format!(
                "{}'s cell isn't known yet",
                player.name
            ))));
        };
        if cell == room.cell {
            return Some(Ok(()));
        }
        let pad = player.pad.clone().unwrap_or_else(|| CELL_PAD.to_owned());
        Some(self.move_to_cell(&cell, &pad))
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
        (room.x, room.y) = (0, 0);
        let area = room.area;
        self.combat.disengage();
        self.send_action(&packet::move_to_cell(area, cell, pad));
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
            Phase::Closed { reason } | Phase::Reconnecting { reason, .. } => {
                Err(ActionError::Closed(reason.clone()))
            }
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
        self.shop = None;
        self.bank = None;
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
        if matches!(self.phase, Phase::Closed { .. }) {
            return;
        }
        let why = self.closing.take().unwrap_or(why);
        let plan =
            self.options.reconnect.as_ref().filter(|plan| {
                self.ready_once && why.retryable() && self.reconnects < plan.attempts
            });
        let Some(wait) = plan.map(|plan| plan.wait(self.reconnects, why.lockout())) else {
            self.reconnect_at = None;
            self.lost = None;
            return self.set_phase(Phase::Closed { reason: why });
        };
        self.reconnects += 1;
        self.reconnect_at = Some(Instant::now() + wait);
        let dropped = self.current.take().and_then(Op::into_reply);
        for reply in dropped
            .into_iter()
            .chain(self.queue.drain(..).map(|(_, r)| r))
        {
            self.replies
                .push((reply, Err(ActionError::Closed(why.clone()))));
        }
        self.lost = Some(why.clone());
        self.forget_session();
        self.set_phase(Phase::Reconnecting {
            reason: why,
            attempt: self.reconnects,
            wait_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
        });
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase.clone();
        self.emit(Event::Phase(phase));
    }

    fn emit(&mut self, event: Event) {
        self.outgoing.push(event);
    }

    fn send_action(&mut self, packet: &str) {
        self.clear_afk();
        self.send(packet);
    }

    fn clear_afk(&mut self) {
        let now = Instant::now();
        let away = self.character.as_ref().is_some_and(|me| me.away);
        if self.anti_afk && away && self.afk_cleared.is_none_or(|at| now >= at + AFK_EVERY) {
            self.afk_cleared = Some(now);
            self.send(&packet::afk(false));
        }
    }

    fn send(&mut self, packet: &str) {
        if self.stream.is_some() {
            self.combat.sent(Instant::now());
        }
        self.send_unpaced(packet);
    }

    fn send_unpaced(&mut self, packet: &str) {
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
                let goto_refused = text.to_ascii_lowercase().contains("goto")
                    && matches!(
                        self.current,
                        Some(Op::Join {
                            stage: JoinStage::Sent { goto: true, .. },
                            ..
                        })
                    );
                if goto_refused {
                    self.refusal = Some(text.clone());
                }
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
                if matches!(
                    self.current,
                    Some(Op::Join {
                        stage: JoinStage::Sent { .. },
                        ..
                    })
                ) {
                    self.requested_spot = None;
                }
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
            "logoutWarning" => {
                let secs = xt.field(5).and_then(|secs| secs.parse().ok()).unwrap_or(0);
                self.closing = Some(Disconnect::Kicked { secs });
            }
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
                let died_in = self.died_in.take();
                let room = self.room.as_ref();
                let back = match &self.respawn {
                    Respawn::Spawn => None,
                    Respawn::WhereDied => died_in
                        .filter(|(area, ..)| room.is_some_and(|room| room.area == *area))
                        .map(|(_, cell, pad)| (cell, pad)),
                    Respawn::At { cell, pad } => Some((cell.clone(), pad.clone())),
                };
                if let Some((cell, pad)) =
                    back.filter(|(cell, _)| room.is_some_and(|room| room.cell != *cell))
                {
                    let _ = self.move_to_cell(&cell, &pad);
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
                self.character = Some(Me {
                    id: EntityId(eid),
                    name: self.username.clone(),
                    ..Me::default()
                });
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
        if let Some(Value::Object(procs)) = json.data.get("procs") {
            let named = procs.iter().filter_map(|(id, proc)| {
                Some((id.parse().ok()?, proc.get("nam")?.as_str()?.to_owned()))
            });
            Arc::make_mut(&mut self.procs).extend(named);
        }
        match json.cmd.as_str() {
            "moveToArea" => {
                let area: MoveToArea = read(&json)?;
                let requested = self.requested_spot.take_if(|spot| area.is_for(&spot.map));
                let (room, mine) = Room::arriving(area, self.me, requested);
                if let (Some(mine), Some(character)) = (mine, &mut self.character) {
                    character.apply(&mine.vitals);
                    character.name = mine.name;
                    if character.away != mine.away {
                        character.away = mine.away;
                        self.emit(Event::AwayChanged { away: mine.away });
                    }
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
                for anim in &tick.anims {
                    let telegraph = anim.msg.as_deref().is_some_and(room::counter_warning);
                    if let (true, Some(Target::Monster(id))) =
                        (telegraph, anim.caster.as_deref().and_then(aura::target))
                        && let Some(monster) = self.room.as_mut().and_then(|r| r.monster(id))
                    {
                        let now = Instant::now();
                        let was = monster.countering(now);
                        monster.warn_counter(now);
                        if !was && monster.countering(now) {
                            self.emit(Event::CounterStarted { monster: id });
                        }
                    }
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
                self.patch_player(&update.name, &update.o.vitals);
                if let Some(room) = &mut self.room {
                    room.place_player(&update.name, update.o.cell, update.o.pad);
                }
            }
            "stu" => {
                let update: StatUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.apply_stats(update);
                }
            }
            "seia" => self.on_slot_filled(read(&json)?, Instant::now()),
            "sAct" => {
                let skills: SkillList = read(&json)?;
                if let Some(character) = &mut self.character
                    && character.learn_skills(skills)
                {
                    self.combat.restart_clock(ITEM_SLOT);
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
                if let Some(character) = &mut self.character {
                    character.bank_count = loaded.bank_count;
                }
                let mut items = self.parse_each("inventory item", loaded.items);
                items.extend(self.parse_each("house item", loaded.hitems));
                self.inventory.items = Arc::new(items);
                self.factions = self
                    .parse_each::<FactionData>("faction", loaded.factions)
                    .into_iter()
                    .map(Faction::from)
                    .collect();
                self.ready_once = true;
                self.reconnects = 0;
                self.boost_check = Some(Instant::now() + BOOST_CHECK);
                if let Some(why) = self.lost.take() {
                    self.epoch.fetch_add(1, Ordering::SeqCst);
                    self.stale = Some(why);
                }
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
                self.on_sold(sold.char_item_id);
            }
            "buyItem" => self.on_bought(read::<Bought>(&json)?),
            "bankFromInv" => self.on_moved(read::<bank::Deposited>(&json)?.into()),
            "bankToInv" => self.on_moved(read::<bank::Withdrew>(&json)?.into()),
            "bankSwapInv" => self.on_moved(read::<bank::Swapped>(&json)?.into()),
            "equipItem" | "unequipItem" => {
                let worn: Worn = read(&json)?;
                if Some(worn.uid) == self.me {
                    let on = json.cmd == "equipItem";
                    if on && self.inventory.items.iter().any(|i| i.id == worn.id) {
                        self.clear_afk();
                    }
                    self.inventory.wear(worn.id, on);
                    self.on_worn(worn.id, on);
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
            "loadShop" => {
                let shop = Arc::new(read::<LoadedShop>(&json)?.shopinfo);
                let id = shop.id;
                self.shop_maps.insert(id, shop.maps.clone());
                self.shop = Some(shop);
                if matches!(self.current, Some(Op::Shop { id: wanted, stage: ShopStage::Sent { .. }, .. }) if wanted == id)
                    && let Some(Op::Shop { reply, .. }) = self.current.take()
                {
                    self.replies.push((reply, Ok(())));
                }
            }
            "acceptQuest" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok && self.accepted_quests.insert(verdict.id) {
                    self.emit(Event::QuestAccepted { id: verdict.id });
                }
                self.answer(Answer::Accepted(verdict));
            }
            "ccqr" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok {
                    self.accepted_quests.remove(&verdict.id);
                    self.emit(Event::QuestCompleted { id: verdict.id });
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
        self.send_action(&packet::move_to_cell(room.area, &room.cell, &room.pad));
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

    fn step_shop(
        &mut self,
        id: ShopId,
        stage: &mut ShopStage,
        now: Instant,
    ) -> task::Poll<Result<(), ActionError>> {
        use task::Poll::{Pending, Ready};
        if let Err(e) = self.ensure_ready() {
            return Ready(Err(e));
        }
        if let ShopStage::Sent { deadline } = *stage {
            return if now >= deadline {
                Ready(Err(ActionError::NoReply))
            } else {
                Pending
            };
        }
        if self
            .shop
            .as_ref()
            .is_some_and(|shop| shop.id == id && !shop.limited)
        {
            return Ready(Ok(()));
        }
        let Some(room) = &self.room else {
            return Ready(Err(ActionError::NotReady));
        };
        if let Some(maps) = self.shop_maps.get(&id)
            && !maps.iter().any(|map| room.is_in(map))
        {
            return Ready(Err(ActionError::Unusable(format!(
                "shop {id} isn't available in this map"
            ))));
        }
        if let Some(until) = self.cooling(Limit::LoadShop, now) {
            *stage = ShopStage::Waiting { wake: Some(until) };
            return Pending;
        }
        let Some(area) = self.room.as_ref().map(|room| room.area) else {
            return Ready(Err(ActionError::NotReady));
        };
        self.limits.used(Limit::LoadShop, now);
        self.send(&packet::load_shop(area, id));
        *stage = ShopStage::Sent {
            deadline: now + REPLY_TIMEOUT,
        };
        Pending
    }

    fn start_pick(&mut self, item: ItemId, now: Instant) -> Result<Instant, ActionError> {
        self.ensure_ready()?;
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        let Some(offer) = self.inventory.drops.iter().find(|d| d.id == item) else {
            return Err(ActionError::NoTarget(format!("item {item} isn't on offer")));
        };
        if self.inventory.full_for(offer, me) {
            let pool = match offer.pool() {
                Pool::Misc => "misc inventory",
                Pool::House => "house inventory",
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
        let away = fields
            .split(',')
            .find_map(|pair| pair.strip_prefix("afk:"))
            .map(|value| value == "true");
        if let (Some(away), Some(me)) = (away, &mut self.character)
            && name.eq_ignore_ascii_case(&self.username)
            && me.away != away
        {
            me.away = away;
            self.emit(Event::AwayChanged { away });
        }
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
        let now = Instant::now();
        let (on, deltas, counter) = match change.target.as_deref().and_then(aura::target) {
            Some(Target::Player(eid)) if Some(eid) == self.me => {
                let Some(character) = &mut self.character else {
                    return;
                };
                (Holder::Me, character.auras.apply(change, now), None)
            }
            Some(Target::Monster(id)) => {
                let room = self.arriving.as_mut().or(self.room.as_mut());
                let Some(monster) = room.and_then(|room| room.monster(id)) else {
                    return;
                };
                let was = monster.countering(now);
                let deltas = monster.apply_aura(change, now);
                let counter = (was != monster.countering(now)).then_some((id, !was));
                (Holder::Monster(id), deltas, counter)
            }
            _ => return,
        };
        for delta in deltas {
            self.emit(match delta {
                AuraDelta::Added { name, stacks } => Event::AuraAdded { on, name, stacks },
                AuraDelta::Removed { name } => Event::AuraRemoved { on, name },
            });
        }
        if let Some((monster, started)) = counter {
            self.emit(if started {
                Event::CounterStarted { monster }
            } else {
                Event::CounterEnded { monster }
            });
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
                    self.died_in = self
                        .room
                        .as_ref()
                        .map(|room| (room.area, room.cell.clone(), room.pad.clone()));
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
