use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use mira_protocol::{MonMapId, QuestId};

use crate::combat::Readiness;
use crate::faction::Faction;
use crate::inventory::{DropOffer, Item, TempItem};

const ENHANCED_SLOTS: [&str; 4] = ["Weapon", "he", "ar", "ba"];
use crate::player::Me;
use crate::quest::Quest;
use crate::room::Room;
use crate::session::Phase;
use crate::shop::Shop;

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
    pub accepted_quests: BTreeSet<QuestId>,
    pub shop: Option<Arc<Shop>>,
    pub bank: Option<Arc<Vec<Item>>>,
    pub procs: Arc<HashMap<u32, String>>,
    pub target: Option<MonMapId>,
    pub readiness: BTreeMap<String, Readiness>,
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
            accepted_quests: BTreeSet::new(),
            shop: None,
            bank: None,
            procs: Arc::default(),
            target: None,
            readiness: BTreeMap::new(),
        }
    }

    pub fn enhancement_name(&self, item: &Item) -> Option<String> {
        if !ENHANCED_SLOTS.contains(&item.slot.as_str()) {
            return None;
        }
        item.enhancement.as_ref()?.name(&self.procs)
    }
}
