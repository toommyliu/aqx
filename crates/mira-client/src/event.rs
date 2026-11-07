use mira_protocol::{CharItemId, ItemId, MonMapId, QuestId};
use serde::Serialize;

use crate::session::Phase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Holder {
    Me,
    Monster(MonMapId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Phase(Phase),
    Joined {
        map: String,
        cell: String,
        pad: String,
        players: usize,
    },
    Moved {
        cell: String,
        pad: String,
    },
    PlayerEntered {
        name: String,
    },
    PlayerLeft {
        name: String,
    },
    PlayerJumped {
        name: String,
        cell: String,
        pad: String,
    },
    PlayerWalked {
        name: String,
        cell: String,
        x: i32,
        y: i32,
    },
    Ready {
        level: u32,
        gold: u64,
        items: usize,
    },
    Chat {
        channel: String,
        sender: String,
        text: String,
    },
    Whisper {
        sender: String,
        text: String,
    },
    Notice {
        kind: NoticeKind,
        text: String,
    },
    Killed {
        id: MonMapId,
        name: String,
        gold: u64,
        exp: u64,
    },
    Died,
    Respawned,
    LeveledUp {
        level: u32,
    },
    DropOffered {
        id: ItemId,
        name: String,
        qty: u32,
    },
    ItemChanged {
        id: ItemId,
        name: String,
        qty: u32,
        place: Place,
    },
    BankLoaded,
    ItemPickedUp {
        id: ItemId,
    },
    ItemReceived {
        id: ItemId,
        qty: u32,
        place: Place,
    },
    ItemBought {
        char_item_id: CharItemId,
    },
    ItemSold {
        char_item_id: CharItemId,
        qty: u32,
        qty_now: u32,
        amount: u64,
        coins: bool,
    },
    QuestAccepted {
        id: QuestId,
    },
    QuestCompleted {
        id: QuestId,
    },
    AuraAdded {
        on: Holder,
        name: String,
        stacks: u32,
    },
    AuraRemoved {
        on: Holder,
        name: String,
    },
    CounterStarted {
        monster: MonMapId,
    },
    CounterEnded {
        monster: MonMapId,
    },
    AwayChanged {
        away: bool,
    },
    Zone {
        zone: Option<String>,
    },
    Unreadable {
        error: String,
    },
    Packet {
        direction: Direction,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Server,
    Notify,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Inventory,
    Temp,
    Bank,
}
