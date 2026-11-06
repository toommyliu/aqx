use mira_protocol::{FactionId, flex};
use serde::{Deserialize, Serialize};

use crate::rank::{progress, rank};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Faction {
    pub id: FactionId,
    pub name: String,
    pub rep: u32,
    pub rank: u32,
    pub rank_progress: u32,
    pub rank_progress_max: Option<u32>,
}

impl Faction {
    fn set_rep(&mut self, rep: u32) {
        self.rep = rep;
        self.rank = rank(rep);
        (self.rank_progress, self.rank_progress_max) = progress(rep);
    }
}

pub(crate) fn add_rep(factions: &mut [Faction], id: FactionId, gained: i64) {
    if let Some(faction) = factions.iter_mut().find(|f| f.id == id) {
        let rep = (i64::from(faction.rep) + gained).clamp(0, i64::from(u32::MAX));
        faction.set_rep(rep as u32);
    }
}

pub(crate) fn learn(factions: &mut Vec<Faction>, faction: Faction) {
    match factions.iter_mut().find(|f| f.id == faction.id) {
        Some(known) => *known = faction,
        None => factions.push(faction),
    }
}

#[derive(Deserialize)]
pub(crate) struct FactionData {
    #[serde(rename = "FactionID")]
    id: FactionId,
    #[serde(rename = "sName")]
    name: String,
    #[serde(rename = "iRep", default, deserialize_with = "flex::int")]
    rep: u32,
}

impl From<FactionData> for Faction {
    fn from(data: FactionData) -> Faction {
        let mut faction = Faction {
            id: data.id,
            name: data.name,
            rep: 0,
            rank: 1,
            rank_progress: 0,
            rank_progress_max: None,
        };
        faction.set_rep(data.rep);
        faction
    }
}

#[derive(Deserialize)]
pub(crate) struct AddedFaction {
    pub(crate) faction: FactionData,
}
