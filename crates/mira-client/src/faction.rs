use mira_protocol::{FactionId, flex};
use serde::{Deserialize, Serialize};

const RANK_THRESHOLDS: [u32; 9] = [
    900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Faction {
    pub id: FactionId,
    pub name: String,
    pub rep: u32,
    pub rank: u32,
}

impl Faction {
    fn set_rep(&mut self, rep: u32) {
        self.rep = rep;
        self.rank = 1 + RANK_THRESHOLDS
            .iter()
            .filter(|&&floor| rep >= floor)
            .count() as u32;
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
        };
        faction.set_rep(data.rep);
        faction
    }
}

#[derive(Deserialize)]
pub(crate) struct AddedFaction {
    pub(crate) faction: FactionData,
}
