use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};
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
    EntityId, Framer, ItemId, MapItemId, Message, MonMapId, QuestId, ShopId, ShopItemId, XmlUser,
    packet,
};
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::REPLY_TIMEOUT;
use crate::api::{Account, ApiError, Server, Token};
use crate::aura::{self, AuraChange, AuraDelta, Target};
use crate::combat::{self, AUTO_ATTACK, Combat};
use crate::event::{Direction, Event, Holder};
use crate::faction::Faction;
use crate::inventory::{Inventory, Item, Pool};
use crate::map::MapFile;
use crate::player::{BoostKind, Me};
use crate::quest::Quest;
use crate::room::{CombatState, Monster, Player, Room, Spot, UserEntry, Vitals, names_room};
use crate::shop::Shop;
use crate::world::{Settings, World};
use bank::{BankMove, BankStage};
use equip::EquipStage;
use fight::Fight;
use quests::{QuestAction, QuestCall};
use reconnect::{Recovery, Step};
use shops::{Trade, TradeStage};

mod bank;
mod equip;
mod fight;
mod messages;
mod quests;
mod reconnect;
mod shops;

pub const DEFAULT_CLIENT_VERSION: &str = "5.32";

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
    pub fn retryable(&self) -> bool {
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

#[derive(Debug, Clone, thiserror::Error)]
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
    #[error("cancelled")]
    Cancelled,
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

fn random_room() -> u32 {
    let bits = RandomState::new().build_hasher().finish();
    1000 + (bits % 99_000) as u32
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
    RestPacket,
    Walk,
}

impl Limit {
    const COUNT: usize = 13;

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
            Limit::RestPacket => 1200,
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

    fn snapshot(&self) -> BTreeMap<&'static str, crate::ActionLock> {
        [
            ("tfer", Some(Limit::Transfer), 3000),
            ("loadShop", Some(Limit::LoadShop), 3000),
            ("loadEnhShop", None, 3000),
            ("loadHairShop", None, 3000),
            ("equipItem", Some(Limit::EquipItem), 1500),
            ("unequipItem", Some(Limit::UnequipItem), 1500),
            ("buyItem", Some(Limit::BuyItem), 1000),
            ("sellItem", Some(Limit::SellItem), 1000),
            ("getMapItem", Some(Limit::MapItem), 1000),
            ("tryQuestComplete", Some(Limit::TurnIn), 1250),
            ("acceptQuest", Some(Limit::AcceptQuest), 1000),
            ("rest", Some(Limit::Rest), 1900),
            ("doIA", None, 1000),
            ("who", None, 3000),
        ]
        .into_iter()
        .map(|(name, limit, millis)| {
            (
                name,
                crate::ActionLock {
                    used: limit.and_then(|limit| self.0[limit as usize]),
                    cooldown: limit.map_or(Duration::from_millis(millis), Limit::gap),
                },
            )
        })
        .collect()
    }

    fn used(&mut self, limit: Limit, now: Instant) {
        self.0[limit as usize] = Some(now);
    }
}

type Slot = Arc<Mutex<Arc<World>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionId(u64);

struct Reply {
    id: ActionId,
    tx: Option<mpsc::SyncSender<Result<(), ActionError>>>,
}

#[derive(Debug, Clone)]
pub enum Update {
    State,
    Event(Event),
    Done {
        id: ActionId,
        result: Result<(), ActionError>,
    },
    Packet(Direction, String),
}

enum Outgoing {
    Event(Event),
    Packet(Direction, String),
}

enum Subscriber {
    Events(mpsc::Sender<Event>),
    Updates(mpsc::Sender<Update>),
}

impl Subscriber {
    fn event(&self, event: &Event) -> bool {
        match self {
            Subscriber::Events(tx) => tx.send(event.clone()).is_ok(),
            Subscriber::Updates(tx) => tx.send(Update::Event(event.clone())).is_ok(),
        }
    }

    fn packet(&self, direction: Direction, text: &str) -> bool {
        match self {
            Subscriber::Events(_) => true,
            Subscriber::Updates(tx) => tx.send(Update::Packet(direction, text.to_owned())).is_ok(),
        }
    }

    fn done(&self, id: ActionId, result: &Result<(), ActionError>) -> bool {
        match self {
            Subscriber::Events(_) => true,
            Subscriber::Updates(tx) => tx
                .send(Update::Done {
                    id,
                    result: result.clone(),
                })
                .is_ok(),
        }
    }
}

#[derive(Clone, Copy)]
enum Cancel {
    One(ActionId),
    All,
}

impl Cancel {
    fn covers(self, reply: &Reply) -> bool {
        match self {
            Cancel::One(id) => reply.id == id,
            Cancel::All => true,
        }
    }
}

fn player_not_found(warning: &str) -> bool {
    warning.starts_with("Player '") && warning.ends_with("' could not be found.")
}

fn latest(slot: &Slot) -> Arc<World> {
    Arc::clone(&slot.lock().unwrap_or_else(PoisonError::into_inner))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Jump {
    pub client_only: bool,
    pub correct_pad: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aggro {
    All,
    Cell,
    Only(HashSet<MonMapId>),
}

impl Aggro {
    fn covers(&self, id: MonMapId) -> bool {
        match self {
            Aggro::All | Aggro::Cell => true,
            Aggro::Only(ids) => ids.contains(&id),
        }
    }
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
    SkillLastUse(String, Option<Instant>),
    Aggro(Option<Aggro>),
    SpawnPoint(String, String),
    AntiAfk(bool),
    Room(Option<u32>),
}

enum Request {
    Act(Action, Reply, u64),
    Set(Setting, u64),
    Cancel(Cancel),
    Untarget,
    Inject(String),
    Subscribe(Subscriber),
    Logout,
}

enum Action {
    Ready,
    LoadCells,
    Join {
        map: String,
        spot: Option<(String, String)>,
    },
    MoveToCell {
        cell: String,
        pad: String,
        jump: Jump,
    },
    Attack {
        target: MonMapId,
    },
    UseSkill {
        slot: String,
        target: Option<Aim>,
    },
    Kill {
        target: MonMapId,
        slots: Option<Vec<String>>,
    },
    ExitCombat,
    Quest(QuestAction),
    AbandonQuest(QuestId),
    PickDrop(ItemId),
    RequestDrop(ItemId),
    LoadShop(ShopId),
    Trade(Trade),
    Bank(Option<BankMove>),
    Equip {
        item: ItemId,
        on: bool,
    },
    MapItem(MapItemId),
    Rest,
    RequestRest,
    RequestRestPacket,
    UseItem(ItemId),
    Goto(String),
    Walk {
        x: i32,
        y: i32,
    },
    Send(String),
}

impl Action {
    fn runs_during_fight(&self) -> bool {
        matches!(
            self,
            Action::Ready
                | Action::Quest(_)
                | Action::AbandonQuest(_)
                | Action::PickDrop(_)
                | Action::RequestDrop(_)
                | Action::MapItem(_)
        )
    }
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
        target: Option<Aim>,
        stage: CastStage,
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
        mode: RestMode,
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

    fn reply(&self) -> Option<&Reply> {
        match self {
            Op::Login { .. } => None,
            Op::Join { reply, .. }
            | Op::Cast { reply, .. }
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

    fn reply_mut(&mut self) -> Option<&mut Reply> {
        match self {
            Op::Login { .. } => None,
            Op::Join { reply, .. }
            | Op::Cast { reply, .. }
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

    fn awaiting(&self) -> bool {
        match self {
            Op::Join { stage, .. } => matches!(stage, JoinStage::Sent { .. }),
            Op::Cast { stage, .. } => matches!(stage, CastStage::Sent { .. }),
            Op::Quest { call, .. } => call.stage.awaiting(),
            Op::Pick { .. } => true,
            Op::Shop { stage, .. } => matches!(stage, ShopStage::Sent { .. }),
            Op::Trade { stage, .. } => !matches!(stage, TradeStage::Waiting { .. }),
            Op::Bank { stage, .. } => !matches!(stage, BankStage::Idle),
            Op::Equip { stage, .. } => matches!(stage, EquipStage::Sent { .. }),
            Op::Login { .. }
            | Op::Leave { .. }
            | Op::MapItem { .. }
            | Op::Rest { .. }
            | Op::Walk { .. } => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RestMode {
    Full,
    Start,
    Packet,
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
    Leaving {
        to: JoinTarget,
        cell: String,
        deadline: Instant,
    },
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
    server_host: Arc<str>,
    requests: mpsc::Sender<Request>,
    waker: Arc<Waker>,
    world: Slot,
    epoch: Arc<AtomicU64>,
    next_id: Arc<AtomicU64>,
}

impl Handle {
    pub fn server_host(&self) -> &str {
        &self.server_host
    }

    pub fn ready(&self) -> Pending {
        self.act(Action::Ready)
    }

    pub fn load_cells(&self) -> Pending {
        self.act(Action::LoadCells)
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
        self.jump(cell, pad, Jump::default())
    }

    pub fn jump(&self, cell: &str, pad: &str, jump: Jump) -> Pending {
        self.act(Action::MoveToCell {
            cell: cell.to_owned(),
            pad: pad.to_owned(),
            jump,
        })
    }

    pub fn attack(&self, target: MonMapId) -> Pending {
        self.act(Action::Attack { target })
    }

    pub fn use_skill(&self, slot: &str, target: Option<Aim>) -> Pending {
        self.act(Action::UseSkill {
            slot: slot.to_owned(),
            target,
        })
    }

    pub fn kill(&self, target: MonMapId) -> Pending {
        self.act(Action::Kill {
            target,
            slots: None,
        })
    }

    pub fn kill_with(&self, target: MonMapId, slots: &[&str]) -> Pending {
        self.act(Action::Kill {
            target,
            slots: Some(slots.iter().map(|slot| (*slot).to_owned()).collect()),
        })
    }

    pub fn exit_combat(&self) -> Pending {
        self.act(Action::ExitCombat)
    }

    pub fn load_quests(&self, ids: &[QuestId]) -> Pending {
        self.act(Action::Quest(QuestAction::Load(ids.to_vec())))
    }

    pub fn abandon_quest(&self, id: QuestId) -> Pending {
        self.act(Action::AbandonQuest(id))
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

    pub fn send_raw(&self, text: &str) -> Pending {
        self.act(Action::Send(text.to_owned()))
    }

    pub fn request_rest_packet(&self) -> Pending {
        self.act(Action::RequestRestPacket)
    }

    pub fn request_rest(&self) -> Pending {
        self.act(Action::RequestRest)
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

    pub fn request_drop(&self, item: ItemId) -> Pending {
        self.act(Action::RequestDrop(item))
    }

    pub fn pick_drop(&self, item: ItemId) -> Pending {
        self.act(Action::PickDrop(item))
    }

    pub fn complete_quest(&self, id: QuestId, reward: Option<ItemId>, times: u32) -> Pending {
        self.complete_quest_special(id, reward, times, false)
    }

    pub fn complete_quest_special(
        &self,
        id: QuestId,
        reward: Option<ItemId>,
        times: u32,
        special: bool,
    ) -> Pending {
        self.act(Action::Quest(QuestAction::TurnIn {
            id,
            reward,
            times,
            special,
        }))
    }

    pub fn set_skill_last_use(&self, slot: &str, at: Option<Instant>) {
        self.set(Setting::SkillLastUse(slot.to_owned(), at));
    }

    pub fn set_counter_hold(&self, hold: CounterHold) {
        self.set(Setting::CounterHold(hold));
    }

    pub fn set_aggro(&self, aggro: Option<Aggro>) {
        self.set(Setting::Aggro(aggro));
    }

    pub fn set_spawn_point(&self, cell: &str, pad: &str) {
        self.set(Setting::SpawnPoint(cell.to_owned(), pad.to_owned()));
    }

    pub fn set_anti_afk(&self, on: bool) {
        self.set(Setting::AntiAfk(on));
    }

    pub fn set_room(&self, room: Option<u32>) {
        self.set(Setting::Room(room));
    }

    fn set(&self, setting: Setting) {
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.request(Request::Set(setting, epoch));
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (subscriber, events) = mpsc::channel();
        self.request(Request::Subscribe(Subscriber::Events(subscriber)));
        events
    }

    pub fn updates(&self) -> mpsc::Receiver<Update> {
        let (subscriber, updates) = mpsc::channel();
        self.request(Request::Subscribe(Subscriber::Updates(subscriber)));
        updates
    }

    pub fn logout(&self) {
        self.request(Request::Logout);
    }

    pub fn snapshot(&self) -> Arc<World> {
        latest(&self.world)
    }

    pub fn same_session(&self, other: &Handle) -> bool {
        Arc::ptr_eq(&self.world, &other.world)
    }

    pub fn cancel(&self, id: ActionId) {
        self.request(Request::Cancel(Cancel::One(id)));
    }

    pub fn cancel_all(&self) {
        self.request(Request::Cancel(Cancel::All));
    }

    pub fn untarget(&self) {
        self.request(Request::Untarget);
    }

    pub fn inject(&self, text: &str) {
        self.request(Request::Inject(text.to_owned()));
    }

    fn act(&self, action: Action) -> Pending {
        let (tx, result) = mpsc::sync_channel(1);
        let id = ActionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.request(Request::Act(action, Reply { id, tx: Some(tx) }, epoch));
        Pending {
            id,
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
    id: ActionId,
    result: mpsc::Receiver<Result<(), ActionError>>,
    world: Slot,
}

impl Pending {
    pub fn id(&self) -> ActionId {
        self.id
    }

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
    let (map_out, map_results) = mpsc::channel();
    let (relogin_out, relogin_results) = mpsc::channel();
    let server_host = Arc::from(server.host.as_str());
    let session = Session {
        account: account.clone(),
        waker: Arc::clone(&waker),
        bank_out,
        bank_results,
        map_out,
        map_results,
        map_cells: HashMap::new(),
        load_cells_pending: false,
        relogin_out,
        relogin_results,
        server: server.clone(),
        epoch: Arc::clone(&epoch),
        stale: None,
        ready_once: false,
        reconnects: 0,
        recovery: None,
        bank: None,
        procs: Arc::default(),
        username: account.username.clone(),
        token: account.token.clone(),
        options,
        version: String::new(),
        poll,
        ready: Events::with_capacity(16),
        requests,
        subscribers: vec![Subscriber::Events(events)],
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
        join_room: None,
        counter_hold: CounterHold::Off,
        aggro: None,
        anti_afk: false,
        afk_cleared: None,
        boost_check: None,
        combat: Combat::default(),
        fight: None,
        cancelled: None,
        closing: None,
        logout_deadline: None,
    };
    thread::Builder::new()
        .name("mira-session".into())
        .spawn(move || session.run(server, version))?;
    let handle = Handle {
        server_host,
        requests: requests_out,
        waker,
        world,
        epoch,
        next_id: Arc::default(),
    };
    Ok((handle, events_in))
}

enum MapCells {
    Loading(Vec<Reply>),
    Loaded(MapFile),
}

type MapResult = (String, Result<MapFile, ActionError>);

const MAP_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

struct Session {
    account: Account,
    waker: Arc<Waker>,
    bank_out: mpsc::Sender<BankResult>,
    bank_results: mpsc::Receiver<BankResult>,
    map_out: mpsc::Sender<MapResult>,
    map_results: mpsc::Receiver<MapResult>,
    map_cells: HashMap<String, MapCells>,
    load_cells_pending: bool,
    relogin_out: mpsc::Sender<Result<Account, ApiError>>,
    relogin_results: mpsc::Receiver<Result<Account, ApiError>>,
    server: Server,
    epoch: Arc<AtomicU64>,
    stale: Option<Disconnect>,
    ready_once: bool,
    reconnects: u32,
    recovery: Option<Recovery>,
    bank: Option<Arc<Vec<Item>>>,
    procs: Arc<HashMap<u32, String>>,
    username: String,
    token: Token,
    options: Options,
    version: String,
    poll: Poll,
    ready: Events,
    requests: mpsc::Receiver<Request>,
    subscribers: Vec<Subscriber>,
    world: Slot,
    outgoing: Vec<Outgoing>,
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
    join_room: Option<u32>,
    counter_hold: CounterHold,
    aggro: Option<(Aggro, Instant)>,
    anti_afk: bool,
    afk_cleared: Option<Instant>,
    boost_check: Option<Instant>,
    combat: Combat,
    fight: Option<Fight>,
    cancelled: Option<ActionId>,
    closing: Option<Disconnect>,
    logout_deadline: Option<Instant>,
}

impl Session {
    fn run(mut self, server: Server, version: String) {
        self.current = Some(self.start_login(server, version, 1));
        loop {
            self.take_requests();
            self.take_map_results();
            self.take_bank_results();
            self.take_relogin_results();
            self.advance(Instant::now());
            if matches!(self.phase, Phase::Closed { .. }) && self.current.is_none() {
                self.take_requests();
                self.advance(Instant::now());
                return self.publish();
            }
            self.flush();
            if self.load_cells_pending && self.outbox.is_empty() {
                self.load_cells_pending = false;
                self.load_map_cells(None);
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
        self.recovery.is_some() || epoch != self.epoch.load(Ordering::SeqCst)
    }

    fn cancel(&mut self, which: Cancel) {
        let (dropped, kept): (Vec<_>, Vec<_>) = self
            .queue
            .drain(..)
            .partition(|(_, reply)| which.covers(reply));
        self.queue = kept.into();
        for (_, reply) in dropped {
            self.replies.push((reply, Err(ActionError::Cancelled)));
        }
        for entry in self.map_cells.values_mut() {
            if let MapCells::Loading(waiters) = entry {
                let (cancelled, kept): (Vec<_>, Vec<_>) =
                    waiters.drain(..).partition(|r| which.covers(r));
                *waiters = kept;
                self.replies.extend(
                    cancelled
                        .into_iter()
                        .map(|r| (r, Err(ActionError::Cancelled))),
                );
            }
        }
        if self.fight.as_ref().is_some_and(|f| which.covers(&f.reply)) {
            self.untarget();
        }
        if let Some(reply) = self.current.as_mut().and_then(Op::reply_mut)
            && which.covers(reply)
        {
            let answered = Reply {
                id: reply.id,
                tx: reply.tx.take(),
            };
            let id = answered.id;
            self.replies.push((answered, Err(ActionError::Cancelled)));
            self.cancelled = Some(id);
        }
    }

    fn untarget(&mut self) {
        if let Some(fight) = self.fight.take() {
            self.replies
                .push((fight.reply, Err(ActionError::Cancelled)));
        }
        self.combat.disengage();
    }

    fn apply(&mut self, setting: Setting) {
        match setting {
            Setting::CounterHold(hold) => self.counter_hold = hold,
            Setting::SkillLastUse(slot, at) => self.combat.set_last_use(&slot, at),
            Setting::Aggro(aggro) => self.aggro = aggro.map(|aggro| (aggro, Instant::now())),
            Setting::SpawnPoint(cell, pad) => {
                if let Some(room) = &mut self.room {
                    room.spawn_point = (cell, pad);
                }
            }
            Setting::AntiAfk(on) => self.anti_afk = on,
            Setting::Room(room) => self.join_room = room,
        }
    }

    fn take_requests(&mut self) {
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Request::Act(_, reply, epoch) if self.stale_epoch(epoch) => {
                    let why = self.recovery.as_ref().map(|r| r.why.clone());
                    let why = why.or_else(|| self.stale.clone());
                    let why = why.unwrap_or(Disconnect::Ended);
                    self.replies.push((reply, Err(ActionError::Closed(why))));
                }
                Request::Cancel(which) => self.cancel(which),
                Request::Untarget => self.untarget(),
                Request::Inject(text) => self.receive_text(text),
                Request::Subscribe(subscriber) => self.subscribers.push(subscriber),
                Request::Logout => self.begin_logout(),
                Request::Set(_, epoch) if self.stale_epoch(epoch) => {}
                Request::Set(setting, _) => self.apply(setting),
                Request::Act(Action::LoadCells, reply, _) => self.load_map_cells(Some(reply)),
                Request::Act(Action::ExitCombat, reply, _) => {
                    if let Some(fight) = self.fight.take() {
                        self.replies
                            .push((fight.reply, Err(ActionError::Interrupted)));
                    }
                    if matches!(self.current, Some(Op::Cast { .. }))
                        && let Some(Op::Cast { reply, .. }) = self.current.take()
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
        if let Some(recovery) = &mut self.recovery
            && matches!(recovery.step, Step::Waiting(at) if now >= at)
        {
            recovery.step = Step::Relogging;
            self.relogin();
        }
        if let Some(me) = &self.character {
            self.combat.expire(me, now);
        }
        self.step_fight(now);
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
                Some(op)
                    if !op.awaiting()
                        && op.reply().is_some_and(|r| Some(r.id) == self.cancelled) => {}
                Some(op) => {
                    self.current = self.step(op, now);
                    if self.current.is_some() {
                        return;
                    }
                }
                None => {
                    let held = self.fight.is_some()
                        && self
                            .queue
                            .front()
                            .is_some_and(|(action, _)| !action.runs_during_fight());
                    if held {
                        return;
                    }
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
            Action::LoadCells => {
                self.load_map_cells(Some(reply));
                return None;
            }
            Action::Ready => self.ensure_ready(),
            Action::AbandonQuest(id) => self.ensure_ready().map(|()| {
                self.accepted_quests.remove(&id);
            }),
            Action::MoveToCell { cell, pad, jump } => {
                let pad = match self.loaded_map() {
                    Some(map) if jump.correct_pad => map.pad_in(&cell, &pad).to_owned(),
                    _ => pad,
                };
                self.move_to_cell_with(&cell, &pad, jump.client_only)
            }
            Action::UseItem(item) => self.use_item(item),
            Action::Send(text) => self.ensure_ready().map(|()| self.send(&text)),
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
            Action::RequestDrop(item) => self.start_pick(item, now).map(|_| ()),
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
            Action::Rest | Action::RequestRest | Action::RequestRestPacket => {
                let mode = match action {
                    Action::Rest => RestMode::Full,
                    Action::RequestRest => RestMode::Start,
                    _ => RestMode::Packet,
                };
                let stage = RestStage::Cooling { wake: None };
                return self.step(Op::Rest { reply, stage, mode }, now);
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
            Action::Kill { target, slots } => {
                self.fight = Some(Fight {
                    reply,
                    target,
                    slots,
                    next: 0,
                    wake: None,
                });
                self.step_fight(now);
                return None;
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
                stage: JoinStage::Leaving { to, cell, deadline },
            } => match self.step_leaving(&cell, deadline, now) {
                task::Poll::Pending => Some(Op::Join {
                    reply,
                    stage: JoinStage::Leaving { to, cell, deadline },
                }),
                task::Poll::Ready(Ok(())) => {
                    let until = self.cooling(Limit::Transfer, now).unwrap_or(now);
                    let stage = JoinStage::Cooling { to, until };
                    self.step(Op::Join { reply, stage }, now)
                }
                task::Poll::Ready(Err(e)) => {
                    self.replies.push((reply, Err(e)));
                    None
                }
            },
            Op::Join {
                reply,
                stage: JoinStage::Cooling { to, .. },
            } => {
                let state = self.character.as_ref().map(|me| me.state);
                let in_room =
                    matches!(&to, JoinTarget::Player(name) if self.player_in_room(name).is_some());
                let refused = match (self.ensure_ready(), state) {
                    (Err(e), _) => Some(e),
                    (Ok(()), Some(CombatState::Dead)) => Some(ActionError::Dead),
                    (Ok(()), Some(CombatState::InCombat)) if !in_room => {
                        Some(ActionError::InCombat)
                    }
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
                    Err(if goto && player_not_found(&why) {
                        ActionError::NoTarget(why)
                    } else {
                        ActionError::Refused(why)
                    })
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
            Op::Leave {
                reply,
                cell,
                deadline,
            } => match self.step_leaving(&cell, deadline, now) {
                task::Poll::Pending => Some(Op::Leave {
                    reply,
                    cell,
                    deadline,
                }),
                task::Poll::Ready(result) => {
                    self.replies.push((reply, result));
                    None
                }
            },
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
            Op::Rest { reply, stage, mode } => {
                let vitals = self.character.as_ref().map(|me| {
                    let hurt = me.hp < me.max_hp || me.mp < me.max_mp;
                    (me.state, hurt)
                });
                let result = match (self.ensure_ready(), vitals, stage) {
                    (Err(e), ..) => Err(e),
                    (Ok(()), None, _) => Err(ActionError::NotReady),
                    (Ok(()), Some((state, _)), _)
                        if mode != RestMode::Full && state != CombatState::Idle =>
                    {
                        Ok(())
                    }
                    (Ok(()), Some((CombatState::Dead, _)), _) => Err(ActionError::Dead),
                    (Ok(()), Some((CombatState::InCombat, _)), _) => Err(ActionError::InCombat),
                    (Ok(()), Some((_, false)), _) if mode == RestMode::Full => Ok(()),
                    (Ok(()), _, RestStage::Sent { deadline }) if now >= deadline => {
                        Err(ActionError::NoReply)
                    }
                    (Ok(()), _, stage @ RestStage::Sent { .. }) => {
                        return Some(Op::Rest { reply, stage, mode });
                    }
                    (Ok(()), _, RestStage::Cooling { .. }) => {
                        let limit = if mode == RestMode::Packet {
                            Limit::RestPacket
                        } else {
                            Limit::Rest
                        };
                        if let Some(until) = self.cooling(limit, now) {
                            let stage = RestStage::Cooling { wake: Some(until) };
                            return Some(Op::Rest { reply, stage, mode });
                        }
                        self.limits.used(limit, now);
                        self.send(&packet::rest_request());
                        if mode != RestMode::Packet {
                            self.send(&packet::emote("rest"));
                            self.clear_afk();
                        }
                        match self.ensure_ready() {
                            Ok(()) if mode != RestMode::Full => Ok(()),
                            Ok(()) => {
                                let deadline = now + REPLY_TIMEOUT;
                                let stage = RestStage::Sent { deadline };
                                return Some(Op::Rest { reply, stage, mode });
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
                stage: JoinStage::Sent { deadline, .. } | JoinStage::Leaving { deadline, .. },
                ..
            }) => Some(*deadline),
            Some(Op::Join {
                stage: JoinStage::Cooling { until, .. },
                ..
            }) => Some(*until),
            Some(Op::Cast {
                stage: CastStage::Waiting { wake },
                ..
            }) => *wake,
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
            .chain(self.fight.as_ref().and_then(|fight| fight.wake))
            .chain(counters)
            .chain(self.logout_deadline)
            .chain(self.recovery.as_ref().and_then(|r| match r.step {
                Step::Waiting(at) => Some(at),
                Step::Relogging | Step::Rejoining => None,
            }))
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
            readiness: self.readiness(Instant::now()),
            global_cooldown: self
                .character
                .as_ref()
                .and_then(|me| self.combat.global_cooldown(me)),
            skill_timing: self
                .character
                .iter()
                .flat_map(|me| {
                    me.skills.iter().map(|skill| {
                        (
                            skill.slot.clone(),
                            crate::SkillTiming {
                                last_use: self.combat.last_use(&skill.slot),
                                cooldown: self
                                    .combat
                                    .cooldown(me, skill)
                                    .into_iter()
                                    .chain(self.combat.global_cooldown(me))
                                    .max(),
                            },
                        )
                    })
                })
                .collect(),
            action_locks: self.limits.snapshot(),
            settings: Settings {
                counter_hold: self.counter_hold,
                aggro: self.aggro.as_ref().map(|(aggro, _)| aggro.clone()),
                anti_afk: self.anti_afk,
                room: self.join_room,
            },
        });
        self.subscribers.retain(|subscriber| match subscriber {
            Subscriber::Events(_) => true,
            Subscriber::Updates(tx) => tx.send(Update::State).is_ok(),
        });
        for outgoing in self.outgoing.drain(..) {
            self.subscribers.retain(|subscriber| match &outgoing {
                Outgoing::Event(event) => subscriber.event(event),
                Outgoing::Packet(direction, text) => subscriber.packet(*direction, text),
            });
        }
        for (reply, result) in self.replies.drain(..) {
            let Some(tx) = reply.tx else { continue };
            self.subscribers
                .retain(|subscriber| subscriber.done(reply.id, &result));
            let _ = tx.send(result);
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
        let map = match self.join_room {
            Some(number) if !names_room(&map) => format!("{map}-{number}"),
            _ => map,
        };
        if self.room.as_ref().is_some_and(|room| room.is_in(&map)) {
            if let Some((cell, pad)) = spot {
                self.move_to_cell(&cell, &pad)?;
            }
            return Ok(None);
        }
        let map = if names_room(&map) {
            map
        } else {
            format!("{map}-{}", random_room())
        };
        self.start_transfer(JoinTarget::Map { map, spot }, now)
    }

    fn start_goto(&mut self, name: String, now: Instant) -> Result<Option<JoinStage>, ActionError> {
        self.ensure_ready()?;
        if self
            .character
            .as_ref()
            .is_some_and(|me| me.state == CombatState::Dead)
        {
            return Err(ActionError::Dead);
        }
        if name.eq_ignore_ascii_case(&self.username) {
            return Err(ActionError::Unusable("that's this player".into()));
        }
        if let Some(result) = self.goto_in_room(&name) {
            return result.map(|()| None);
        }
        self.start_transfer(JoinTarget::Player(name), now)
    }

    fn start_transfer(
        &mut self,
        to: JoinTarget,
        now: Instant,
    ) -> Result<Option<JoinStage>, ActionError> {
        let in_combat = self
            .character
            .as_ref()
            .is_some_and(|me| me.state == CombatState::InCombat);
        if in_combat && let Some((cell, deadline)) = self.start_leaving(now)? {
            return Ok(Some(JoinStage::Leaving { to, cell, deadline }));
        }
        let until = self.cooling(Limit::Transfer, now).unwrap_or(now);
        Ok(Some(JoinStage::Cooling { to, until }))
    }

    fn step_leaving(
        &self,
        cell: &str,
        deadline: Instant,
        now: Instant,
    ) -> task::Poll<Result<(), ActionError>> {
        let state = self.character.as_ref().map(|me| me.state);
        task::Poll::Ready(match (self.ensure_ready(), state) {
            (Err(e), _) => Err(e),
            (Ok(()), Some(CombatState::Dead)) => Err(ActionError::Dead),
            (Ok(()), Some(CombatState::InCombat)) if now < deadline => return task::Poll::Pending,
            (Ok(()), Some(CombatState::InCombat)) => {
                Err(ActionError::StillInCombat(cell.to_owned()))
            }
            (Ok(()), _) => Ok(()),
        })
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

    fn player_in_room(&self, name: &str) -> Option<&Player> {
        self.room
            .as_ref()?
            .players
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    fn goto_in_room(&mut self, name: &str) -> Option<Result<(), ActionError>> {
        let player = self.player_in_room(name)?;
        let room = self.room.as_ref()?;
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

    fn loaded_map(&self) -> Option<&MapFile> {
        match self.map_cells.get(&self.room.as_ref()?.file)? {
            MapCells::Loaded(map) => Some(map),
            MapCells::Loading(_) => None,
        }
    }

    fn move_to_cell(&mut self, cell: &str, pad: &str) -> Result<(), ActionError> {
        self.move_to_cell_with(cell, pad, false)
    }

    fn move_to_cell_with(
        &mut self,
        cell: &str,
        pad: &str,
        client_only: bool,
    ) -> Result<(), ActionError> {
        self.ensure_ready()?;
        if self.room.as_ref().is_some_and(|room| room.locked(cell)) {
            return Ok(());
        }
        let arrival = self
            .loaded_map()
            .map_or((0, 0), |map| map.arrival(cell, pad));
        let Some(room) = &mut self.room else {
            return Err(ActionError::NotReady);
        };
        room.cell = cell.to_owned();
        room.pad = pad.to_owned();
        (room.x, room.y) = arrival;
        let area = room.area;
        self.combat.disengage();
        if !client_only {
            self.send_action(&packet::move_to_cell(area, cell, pad));
            self.fetch_cellmates();
            self.aggro_cell();
        }
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
        for entry in self.map_cells.values_mut() {
            if let MapCells::Loading(waiters) = entry {
                self.replies.extend(
                    waiters
                        .drain(..)
                        .map(|r| (r, Err(ActionError::Closed(why.clone())))),
                );
            }
        }
        let plan =
            self.options.reconnect.as_ref().filter(|plan| {
                self.ready_once && why.retryable() && self.reconnects < plan.attempts
            });
        let Some(wait) = plan.map(|plan| plan.wait(self.reconnects, why.lockout())) else {
            self.recovery = None;
            return self.set_phase(Phase::Closed { reason: why });
        };
        self.reconnects += 1;
        let at = Instant::now() + wait;
        let dropped = self.current.take().and_then(Op::into_reply);
        let fighting = self.fight.take().map(|fight| fight.reply);
        for reply in dropped
            .into_iter()
            .chain(fighting)
            .chain(self.queue.drain(..).map(|(_, r)| r))
        {
            self.replies
                .push((reply, Err(ActionError::Closed(why.clone()))));
        }
        self.recovery = Some(Recovery {
            why: why.clone(),
            step: Step::Waiting(at),
        });
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
        self.outgoing.push(Outgoing::Event(event));
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
        if self.watched() {
            let text = packet.replace(self.token.as_str(), "<token>");
            self.outgoing.push(Outgoing::Packet(Direction::Out, text));
        }
        self.outbox.extend_from_slice(packet.as_bytes());
        self.outbox.push(0);
        self.flush();
    }

    fn watched(&self) -> bool {
        self.subscribers
            .iter()
            .any(|s| matches!(s, Subscriber::Updates(_)))
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
        match mira_protocol::unpack(frame) {
            Ok(text) => self.receive_text(text),
            Err(e) => self.emit(Event::Unreadable {
                error: e.to_string(),
            }),
        }
    }

    fn receive_text(&mut self, text: String) {
        if self.options.trace_packets {
            self.emit(Event::Packet {
                direction: Direction::In,
                text: redact(&text),
            });
        }
        let handled = match mira_protocol::parse(&text) {
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
        if self.watched() {
            self.outgoing
                .push(Outgoing::Packet(Direction::In, redact(&text)));
        }
    }

    fn load_map_cells(&mut self, reply: Option<Reply>) {
        let file = match self.room.as_ref() {
            Some(room) if !room.file.is_empty() && !matches!(self.phase, Phase::Closed { .. }) => {
                room.file.clone()
            }
            _ => {
                if let Some(reply) = reply {
                    self.replies.push((reply, Err(ActionError::NotReady)));
                }
                return;
            }
        };
        match self.map_cells.get_mut(&file) {
            Some(MapCells::Loaded(_)) => {
                if let Some(reply) = reply {
                    self.replies.push((reply, Ok(())));
                }
                return;
            }
            Some(MapCells::Loading(waiters)) => {
                waiters.extend(reply);
                return;
            }
            None => {}
        }
        self.map_cells
            .insert(file.clone(), MapCells::Loading(reply.into_iter().collect()));
        let account = self.account.clone();
        let tx = self.map_out.clone();
        let waker = self.waker.clone();
        thread::spawn(move || {
            let mut delays = MAP_RETRY_DELAYS.iter();
            let map = loop {
                match crate::api::map_file(&account, &file) {
                    Ok(bytes) => {
                        break crate::map::read(&bytes)
                            .map_err(|e| ActionError::Http(format!("unreadable map file: {e}")));
                    }
                    Err(e) => match delays.next() {
                        Some(delay) => thread::sleep(*delay),
                        None => break Err(ActionError::Http(e.to_string())),
                    },
                }
            };
            let _ = tx.send((file, map));
            let _ = waker.wake();
        });
    }

    fn take_map_results(&mut self) {
        while let Ok((file, map)) = self.map_results.try_recv() {
            let Some(MapCells::Loading(waiters)) = self.map_cells.remove(&file) else {
                continue;
            };
            let result = match map {
                Ok(map) => {
                    let current = self.room.as_mut().filter(|r| r.file == file);
                    let result = if let Some(room) = current {
                        room.load_map(&map);
                        Ok(())
                    } else {
                        Err(ActionError::Unusable(
                            "the map changed while loading cells".into(),
                        ))
                    };
                    self.map_cells.insert(file, MapCells::Loaded(map));
                    result
                }
                Err(e) => Err(e),
            };
            self.replies
                .extend(waiters.into_iter().map(|reply| (reply, result.clone())));
        }
    }

    fn enter(&mut self, mut room: Room) {
        if let Some(MapCells::Loaded(map)) = self.map_cells.get(&room.file) {
            room.load_map(map);
        }
        let Some(me) = self.me else { return };
        let first = self.phase
            == (Phase::Loading {
                step: Load::Joining,
            });
        let move_to_cell = packet::move_to_cell(room.area, &room.cell, &room.pad);
        let user_data = packet::retrieve_user_data(room.area, me);
        let joined = Event::Joined {
            map: room.name.clone(),
            cell: room.cell.clone(),
            pad: room.pad.clone(),
            players: room.players.len(),
        };
        self.room = Some(room);
        self.joins += 1;
        self.combat.disengage();
        self.emit(joined);
        if first {
            self.set_phase(Phase::Loading {
                step: Load::Character,
            });
        }
        self.send_action(&move_to_cell);
        if first {
            self.send(&user_data);
        }
        self.fetch_cellmates();
        self.aggro_cell();
        self.load_cells_pending = true;
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
            && !maps.iter().any(|map| map == "Menu" || room.is_in(map))
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
        if let Some(moved) = self
            .room
            .as_mut()
            .and_then(|room| room.move_player(name, fields))
        {
            self.emit(moved);
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
            Some(Target::Player(eid)) => {
                if let Some(player) = self
                    .room
                    .as_mut()
                    .and_then(|room| room.players.iter_mut().find(|p| p.eid == eid))
                {
                    player.auras.apply(change, now);
                }
                return;
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
            self.patch_me(patch);
        } else if let Some(player) = self.room.as_mut().and_then(|r| r.player_named(name)) {
            player.apply(patch);
        }
    }

    fn patch_me(&mut self, patch: &Vitals) {
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
