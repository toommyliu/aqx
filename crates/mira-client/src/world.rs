use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mira_protocol::{ItemId, MonMapId, QuestId};

use crate::combat::Readiness;
use crate::faction::Faction;
use crate::inventory::{self, DropOffer, Item, Pool, TempItem};

const ENHANCED_SLOTS: [&str; 4] = ["Weapon", "he", "ar", "ba"];
use crate::player::Me;
use crate::quest::Quest;
use crate::room::Room;
use crate::session::{Aggro, CounterHold, Phase};
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
    pub global_cooldown: Option<Instant>,
    pub skill_timing: BTreeMap<String, SkillTiming>,
    pub action_locks: BTreeMap<&'static str, ActionLock>,
    pub settings: Settings,
}

#[derive(Debug, Clone, Copy)]
pub struct SkillTiming {
    pub last_use: Option<Instant>,
    pub cooldown: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
pub struct ActionLock {
    pub used: Option<Instant>,
    pub cooldown: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub counter_hold: CounterHold,
    pub aggro: Option<Aggro>,
    pub anti_afk: bool,
    pub room: Option<u32>,
    pub auto_relogin: bool,
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
            global_cooldown: None,
            skill_timing: BTreeMap::new(),
            action_locks: BTreeMap::new(),
            settings: Settings::default(),
        }
    }

    pub fn has_room(&self, id: ItemId, pool: Pool) -> bool {
        self.me
            .as_ref()
            .is_some_and(|me| !inventory::no_room(&self.inventory, id, pool, me))
    }

    pub fn enhancement_name(&self, item: &Item) -> Option<String> {
        if !ENHANCED_SLOTS.contains(&item.slot.as_str()) {
            return None;
        }
        item.enhancement.as_ref()?.name(&self.procs)
    }
}
