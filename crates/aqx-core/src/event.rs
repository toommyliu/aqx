use aqx_protocol::{EntityId, QuestId};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Inventory,
    Temp,
    Bank,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    LoggedIn { eid: EntityId },
    LoginRejected { reason: String },
    LoggedInElsewhere,
    Joined { area: String, players: usize },
    InventoryLoaded { items: usize },
    Ready { items: usize },
    Died,
    Respawned,
    Killed { monster: String },
    DropOffered { item: String, qty: u32 },
    ItemChanged { item: String, qty: u32, place: Place },
    Chat { channel: String, sender: String, text: String },
    Progress { label: String, current: u32, total: u32 },
    QuestAccepted { id: QuestId, name: String },
    QuestCompleted { id: QuestId, name: String },
    QuestRejected { id: QuestId, reason: String },
    ServerText { text: String },
    Warning { text: String },
    Unreadable { what: String },
    RecordingFailed { error: String },
}
