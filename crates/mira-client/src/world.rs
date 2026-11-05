use std::collections::BTreeMap;
use std::sync::Arc;

use mira_protocol::QuestId;

use crate::faction::Faction;
use crate::inventory::{DropOffer, Item, TempItem};
use crate::player::Me;
use crate::quest::Quest;
use crate::room::Room;
use crate::session::Phase;

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

impl World {
    pub(crate) fn connecting(phase: Phase) -> World {
        World {
            phase,
            room: None,
            me: None,
            inventory: Arc::default(),
            temp: Vec::new(),
            drops: Vec::new(),
            factions: Vec::new(),
            quests: Arc::default(),
        }
    }
}
