use std::collections::HashMap;

use mira_protocol::{AreaId, EntityId, MonId, MonMapId, XmlUser, flex};
use serde::{Deserialize, Deserializer, Serialize};

const SPAWN: (&str, &str) = ("Enter", "Spawn");

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Room {
    #[serde(skip)]
    pub(crate) area: AreaId,
    pub name: String,
    pub cell: String,
    pub pad: String,
    pub players: Vec<Player>,
    pub monsters: Vec<Monster>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Player {
    #[serde(skip)]
    eid: EntityId,
    pub name: String,
    pub cell: Option<String>,
    #[serde(skip)]
    fetched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Monster {
    pub id: MonMapId,
    pub name: String,
    pub cell: String,
    pub hp: u32,
    pub max_hp: u32,
    pub state: CombatState,
    pub aggressive: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatState {
    Dead,
    #[default]
    Idle,
    InCombat,
}

impl<'de> Deserialize<'de> for CombatState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match flex::int::<_, i64>(d)? {
            0 => Ok(CombatState::Dead),
            1 => Ok(CombatState::Idle),
            2 => Ok(CombatState::InCombat),
            n => Err(serde::de::Error::custom(format!(
                "unknown combat state {n}"
            ))),
        }
    }
}

impl Room {
    pub(crate) fn arriving(area: MoveToArea, me: Option<EntityId>) -> (Room, Option<Vitals>) {
        let (mine, others): (Vec<AreaPlayer>, Vec<AreaPlayer>) =
            area.players.into_iter().partition(|p| Some(p.eid) == me);
        let (cell, pad) = SPAWN;
        let room = Room {
            area: area.area,
            name: area.name,
            cell: cell.to_owned(),
            pad: pad.to_owned(),
            players: others
                .into_iter()
                .map(|p| Player {
                    eid: p.eid,
                    name: p.name,
                    cell: Some(p.cell).filter(|cell| !cell.is_empty()),
                    fetched: false,
                })
                .collect(),
            monsters: monsters(area.monster_types, area.monster_spots, area.monsters),
        };
        (room, mine.into_iter().next().map(|p| p.vitals))
    }

    pub(crate) fn is_in(&self, map: &str) -> bool {
        let base = self
            .name
            .rsplit_once('-')
            .map_or(self.name.as_str(), |(base, _)| base);
        self.name.eq_ignore_ascii_case(map) || base.eq_ignore_ascii_case(map)
    }

    pub(crate) fn unfetched_cellmates(&mut self) -> Vec<EntityId> {
        let cell = &self.cell;
        self.players
            .iter_mut()
            .filter(|p| !p.fetched && p.cell.as_ref() == Some(cell))
            .map(|p| {
                p.fetched = true;
                p.eid
            })
            .collect()
    }

    pub(crate) fn add_player(&mut self, user: &XmlUser) -> Option<String> {
        let name = user.name.as_ref()?;
        if self.players.iter().any(|p| p.eid == user.id) {
            return None;
        }
        self.players.push(Player {
            eid: user.id,
            name: name.clone(),
            cell: None,
            fetched: true,
        });
        Some(name.clone())
    }

    pub(crate) fn remove_player(&mut self, eid: EntityId) -> Option<String> {
        let at = self.players.iter().position(|p| p.eid == eid)?;
        Some(self.players.remove(at).name)
    }

    pub(crate) fn move_player(&mut self, name: &str, fields: &str) {
        let Some(player) = self
            .players
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        let cell = fields
            .split(',')
            .filter_map(|pair| pair.split_once(':'))
            .find_map(|(key, value)| (key == "strFrame").then_some(value));
        if let Some(cell) = cell {
            player.cell = Some(cell.to_owned());
        }
    }

    pub(crate) fn patch_monster(&mut self, id: MonMapId, patch: &Vitals) {
        if let Some(monster) = self.monsters.iter_mut().find(|m| m.id == id) {
            monster.hp = patch.hp.unwrap_or(monster.hp);
            monster.max_hp = patch.max_hp.unwrap_or(monster.max_hp);
            monster.state = patch.state.unwrap_or(monster.state);
        }
    }
}

fn monsters(
    types: Vec<MonsterType>,
    spots: Vec<MonsterSpot>,
    monsters: Vec<MonsterBranch>,
) -> Vec<Monster> {
    let names: HashMap<MonId, String> = types.into_iter().map(|t| (t.id, t.name)).collect();
    let spots: HashMap<MonMapId, MonsterSpot> = spots.into_iter().map(|s| (s.id, s)).collect();
    monsters
        .into_iter()
        .filter_map(|m| {
            let spot = spots.get(&m.id)?;
            Some(Monster {
                id: m.id,
                name: names.get(&m.kind).cloned().unwrap_or_default(),
                cell: spot.cell.clone(),
                hp: m.hp,
                max_hp: m.max_hp,
                state: m.state,
                aggressive: spot.aggressive,
            })
        })
        .collect()
}

#[derive(Deserialize)]
pub(crate) struct Vitals {
    #[serde(rename = "intHP", default, deserialize_with = "flex::opt_int")]
    pub(crate) hp: Option<u32>,
    #[serde(rename = "intHPMax", default, deserialize_with = "flex::opt_int")]
    pub(crate) max_hp: Option<u32>,
    #[serde(rename = "intMP", default, deserialize_with = "flex::opt_int")]
    pub(crate) mp: Option<u32>,
    #[serde(rename = "intMPMax", default, deserialize_with = "flex::opt_int")]
    pub(crate) max_mp: Option<u32>,
    #[serde(rename = "intState", default)]
    pub(crate) state: Option<CombatState>,
}

#[derive(Deserialize)]
pub(crate) struct MoveToArea {
    #[serde(rename = "areaId")]
    area: AreaId,
    #[serde(rename = "areaName")]
    name: String,
    #[serde(rename = "uoBranch", default)]
    players: Vec<AreaPlayer>,
    #[serde(rename = "mondef", default)]
    monster_types: Vec<MonsterType>,
    #[serde(rename = "monmap", default)]
    monster_spots: Vec<MonsterSpot>,
    #[serde(rename = "monBranch", default)]
    monsters: Vec<MonsterBranch>,
}

#[derive(Deserialize)]
struct AreaPlayer {
    #[serde(rename = "entID")]
    eid: EntityId,
    #[serde(rename = "strUsername")]
    name: String,
    #[serde(rename = "strFrame", default)]
    cell: String,
    #[serde(flatten)]
    vitals: Vitals,
}

#[derive(Deserialize)]
struct MonsterType {
    #[serde(rename = "MonID")]
    id: MonId,
    #[serde(rename = "strMonName")]
    name: String,
}

#[derive(Deserialize)]
struct MonsterSpot {
    #[serde(rename = "MonMapID")]
    id: MonMapId,
    #[serde(rename = "strFrame", default)]
    cell: String,
    #[serde(rename = "bRed", default, deserialize_with = "flex::bool")]
    aggressive: bool,
}

#[derive(Deserialize)]
struct MonsterBranch {
    #[serde(rename = "MonMapID")]
    id: MonMapId,
    #[serde(rename = "MonID")]
    kind: MonId,
    #[serde(rename = "intHP", deserialize_with = "flex::int")]
    hp: u32,
    #[serde(rename = "intHPMax", deserialize_with = "flex::int")]
    max_hp: u32,
    #[serde(rename = "intState")]
    state: CombatState,
}

#[derive(Deserialize)]
pub(crate) struct MonsterUpdate {
    pub(crate) id: MonMapId,
    pub(crate) o: Vitals,
}

#[derive(Deserialize)]
pub(crate) struct PlayerUpdate {
    #[serde(rename = "unm")]
    pub(crate) name: String,
    pub(crate) o: Vitals,
}

#[derive(Deserialize)]
pub(crate) struct Tick {
    #[serde(rename = "m", default)]
    pub(crate) monsters: HashMap<MonMapId, Vitals>,
    #[serde(rename = "p", default)]
    pub(crate) players: HashMap<String, Vitals>,
}
