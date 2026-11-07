use mira_client::{
    ActionError, EntityId, ItemId, MapItemId, MonMapId, QuestId, ShopId, ShopItemId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    #[serde(flatten)]
    pub call: Call,
}
fn enter() -> String {
    "Enter".into()
}
fn spawn() -> String {
    "Spawn".into()
}
fn login_timeout() -> u64 {
    30_000
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum Call {
    Join {
        map: String,
        #[serde(default = "enter")]
        cell: String,
        #[serde(default = "spawn")]
        pad: String,
    },
    Jump {
        cell: String,
        pad: String,
        #[serde(default)]
        client_only: bool,
    },
    Walk {
        x: i32,
        y: i32,
    },
    Goto {
        player: String,
    },
    Rest,
    RestPacket,
    ExitCombat,
    Attack {
        target: MonMapId,
    },
    Untarget,
    SetSkillLastUse {
        slot: String,
        at: i64,
    },
    UseSkill {
        slot: String,
        target: Option<Target>,
    },
    LoadQuests {
        ids: Vec<QuestId>,
    },
    Accept {
        quest: QuestId,
    },
    Complete {
        quest: QuestId,
        reward: Option<ItemId>,
        times: i64,
        #[serde(default)]
        special: bool,
    },
    Abandon {
        quest: QuestId,
    },
    ForgetQuests,
    Pickup {
        item: ItemId,
    },
    GetMapItem {
        item: MapItemId,
    },
    LoadBank,
    LoadCells,
    Deposit {
        item: ItemId,
    },
    Withdraw {
        item: ItemId,
    },
    Swap {
        inventory: ItemId,
        bank: ItemId,
    },
    LoadShop {
        shop: ShopId,
    },
    Buy {
        listing: ShopItemId,
        quantity: u32,
    },
    Sell {
        item: ItemId,
        quantity: u32,
    },
    Equip {
        item: ItemId,
    },
    Unequip {
        item: ItemId,
    },
    UseItem {
        item: ItemId,
    },
    Send {
        text: String,
    },
    Inject {
        text: String,
    },
    SetAggro {
        mode: Aggro,
    },
    SetSpawnPoint {
        cell: String,
        pad: String,
    },
    SetCounterHold {
        mode: CounterHold,
    },
    Listen {
        direction: Direction,
        enabled: bool,
    },
    Log {
        level: Level,
        text: String,
    },
    Status {
        text: String,
    },
    Servers,
    SetLoginInfo {
        username: String,
        password: String,
    },
    CancelRelogin {
        request: RequestId,
    },
    Logout,
    Relogin {
        server: Option<String>,
        #[serde(default = "login_timeout")]
        timeout_ms: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Monster(MonMapId),
    Player(EntityId),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Log,
    Warn,
    Error,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggro {
    Off,
    All,
    Cell,
    Only(Vec<MonMapId>),
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterHold {
    Off,
    All,
    Attacks,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Worker {
    Request(Request),
    Done { done: Done },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Done {
    pub outcome: Outcome,
    pub detail: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Finished,
    Stopped,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Ok { reply: RequestId, ok: Value },
    Error { reply: RequestId, error: Error },
    State { state: Section },
    Event { event: Value },
    Packet { packet: Packet },
    Stop { stop: Empty },
    Disconnected { disconnected: bool },
    Reset { reset: Empty },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Empty {}
#[derive(Debug, Serialize, Deserialize)]
pub struct Section {
    pub section: SectionName,
    pub value: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionName {
    Session,
    Me,
    Room,
    Inventory,
    Temp,
    Drops,
    Bank,
    Quests,
    Factions,
    Shop,
    Combat,
    Settings,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Packet {
    pub direction: Direction,
    pub text: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotReady,
    Refused,
    NoReply,
    NoTarget,
    Unusable,
    Dead,
    InCombat,
    Interrupted,
    StillInCombat,
    Http,
    Closed,
    Stopped,
    Cancelled,
}
impl From<ActionError> for Error {
    fn from(error: ActionError) -> Self {
        let kind = match &error {
            ActionError::NotReady => ErrorKind::NotReady,
            ActionError::Refused(_) => ErrorKind::Refused,
            ActionError::NoReply => ErrorKind::NoReply,
            ActionError::NoTarget(_) => ErrorKind::NoTarget,
            ActionError::Unusable(_) => ErrorKind::Unusable,
            ActionError::Dead => ErrorKind::Dead,
            ActionError::InCombat => ErrorKind::InCombat,
            ActionError::Interrupted => ErrorKind::Interrupted,
            ActionError::StillInCombat(_) => ErrorKind::StillInCombat,
            ActionError::Http(_) => ErrorKind::Http,
            ActionError::Closed(_) => ErrorKind::Closed,
            ActionError::Stopped => ErrorKind::Stopped,
            ActionError::Cancelled => ErrorKind::Cancelled,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}
