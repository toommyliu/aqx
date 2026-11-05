use std::collections::HashMap;

use mira_protocol::{AreaId, EntityId, MonId, MonMapId, XmlUser, flex};
use serde::{Deserialize, Deserializer, Serialize};

use crate::aura::{AuraChange, Auras};
use crate::combat::ActionResult;

pub(crate) const SPAWN: (&str, &str) = ("Enter", "Spawn");

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Room {
    #[serde(skip)]
    pub(crate) area: AreaId,
    pub name: String,
    pub cell: String,
    pub pad: String,
    pub x: i32,
    pub y: i32,
    pub players: Vec<Player>,
    pub monsters: Vec<Monster>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Player {
    #[serde(skip)]
    pub(crate) eid: EntityId,
    pub name: String,
    pub cell: Option<String>,
    pub pad: Option<String>,
    pub x: i32,
    pub y: i32,
    pub level: Option<u32>,
    pub class_name: Option<String>,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub state: CombatState,
    #[serde(skip)]
    fetched: bool,
}

impl Player {
    fn new(eid: EntityId, name: String, cell: Option<String>, fetched: bool) -> Player {
        Player {
            eid,
            name,
            cell,
            pad: None,
            x: 0,
            y: 0,
            level: None,
            class_name: None,
            hp: 0,
            max_hp: 0,
            mp: 0,
            max_mp: 0,
            state: CombatState::default(),
            fetched,
        }
    }

    pub(crate) fn apply(&mut self, patch: &Vitals) {
        self.hp = patch.hp.unwrap_or(self.hp);
        self.max_hp = patch.max_hp.unwrap_or(self.max_hp);
        self.mp = patch.mp.unwrap_or(self.mp);
        self.max_mp = patch.max_mp.unwrap_or(self.max_mp);
        self.state = patch.state.unwrap_or(self.state);
    }

    pub(crate) fn learn(&mut self, data: &PlayerData) {
        self.level = data.level.or(self.level);
        if let Some(class) = &data.class_name {
            self.class_name = Some(class.clone());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Monster {
    pub id: MonMapId,
    pub name: String,
    pub cell: String,
    pub hp: u32,
    pub max_hp: u32,
    pub state: CombatState,
    pub aggressive: bool,
    pub auras: Auras,
}

impl Monster {
    pub fn alive(&self) -> bool {
        self.hp > 0 && self.state != CombatState::Dead
    }
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

#[derive(Debug, Clone)]
pub(crate) struct Spot {
    pub(crate) map: String,
    pub(crate) cell: String,
    pub(crate) pad: String,
}

fn names_map(area: &str, map: &str) -> bool {
    let base = area.rsplit_once('-').map_or(area, |(base, _)| base);
    area.eq_ignore_ascii_case(map) || base.eq_ignore_ascii_case(map)
}

impl MoveToArea {
    pub(crate) fn is_for(&self, map: &str) -> bool {
        names_map(&self.name, map)
    }
}

impl Room {
    pub(crate) fn arriving(
        area: MoveToArea,
        me: Option<EntityId>,
        requested: Option<Spot>,
    ) -> (Room, Option<Vitals>) {
        let (mine, others): (Vec<AreaPlayer>, Vec<AreaPlayer>) =
            area.players.into_iter().partition(|p| Some(p.eid) == me);
        let (x, y) = mine.first().map_or((0, 0), |p| (p.tx, p.ty));
        let placed = mine.first().filter(|p| !p.cell.is_empty()).map(|p| {
            let pad = if p.pad.is_empty() { SPAWN.1 } else { &p.pad };
            (p.cell.clone(), pad.to_owned())
        });
        let (cell, pad) = requested
            .filter(|spot| area.kind != 0 && names_map(&area.name, &spot.map))
            .map(|spot| (spot.cell, spot.pad))
            .or(placed)
            .unwrap_or_else(|| (SPAWN.0.to_owned(), SPAWN.1.to_owned()));
        let room = Room {
            area: area.area,
            name: area.name,
            cell,
            pad,
            x,
            y,
            players: others
                .into_iter()
                .map(|p| {
                    let cell = Some(p.cell).filter(|cell| !cell.is_empty());
                    let mut player = Player::new(p.eid, p.name, cell, false);
                    player.pad = Some(p.pad).filter(|pad| !pad.is_empty());
                    (player.x, player.y) = (p.tx, p.ty);
                    player.level = p.level;
                    player.apply(&p.vitals);
                    player
                })
                .collect(),
            monsters: monsters(area.monster_types, area.monster_spots, area.monsters),
        };
        (room, mine.into_iter().next().map(|p| p.vitals))
    }

    pub(crate) fn is_in(&self, map: &str) -> bool {
        names_map(&self.name, map)
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
        self.players
            .push(Player::new(user.id, name.clone(), None, true));
        Some(name.clone())
    }

    pub(crate) fn player(&mut self, eid: EntityId) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.eid == eid)
    }

    pub(crate) fn player_named(&mut self, name: &str) -> Option<&mut Player> {
        self.players
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub(crate) fn remove_player(&mut self, eid: EntityId) -> Option<String> {
        let at = self.players.iter().position(|p| p.eid == eid)?;
        Some(self.players.remove(at).name)
    }

    pub(crate) fn move_player(&mut self, name: &str, fields: &str) {
        let field = |wanted: &str| {
            fields
                .split(',')
                .filter_map(|pair| pair.split_once(':'))
                .find_map(|(key, value)| (key == wanted).then(|| value.to_owned()))
        };
        self.place_player(name, field("strFrame"), field("strPad"));
        let at = |key| field(key).and_then(|value| value.parse().ok());
        if let (Some(x), Some(y)) = (at("tx"), at("ty"))
            && let Some(player) = self
                .players
                .iter_mut()
                .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            (player.x, player.y) = (x, y);
        }
    }

    pub(crate) fn place_player(&mut self, name: &str, cell: Option<String>, pad: Option<String>) {
        let Some(player) = self
            .players
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        if cell.is_some() {
            player.cell = cell;
            (player.x, player.y) = (0, 0);
        }
        if pad.is_some() {
            player.pad = pad;
        }
    }

    pub(crate) fn patch_monster(&mut self, id: MonMapId, patch: &Vitals) {
        let Some(monster) = self.monsters.iter_mut().find(|m| m.id == id) else {
            return;
        };
        monster.max_hp = patch.max_hp.unwrap_or(monster.max_hp);
        if let Some(hp) = patch.hp {
            monster.hp = hp;
            if hp == 0 && monster.cell == self.cell {
                monster.auras.clear();
            }
        }
        if let Some(state) = patch.state {
            monster.state = state;
            if state != CombatState::InCombat {
                monster.auras.clear();
            }
        }
    }

    pub(crate) fn monster_auras(&mut self, id: MonMapId) -> Option<&mut Auras> {
        self.monsters
            .iter_mut()
            .find(|m| m.id == id)
            .map(|m| &mut m.auras)
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
                auras: Auras::default(),
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
    #[serde(rename = "intType", default, deserialize_with = "flex::int")]
    kind: i64,
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
    #[serde(rename = "strPad", default)]
    pad: String,
    #[serde(default, deserialize_with = "flex::int")]
    tx: i32,
    #[serde(default, deserialize_with = "flex::int")]
    ty: i32,
    #[serde(rename = "intLevel", default, deserialize_with = "flex::opt_int")]
    level: Option<u32>,
    #[serde(flatten)]
    vitals: Vitals,
}

#[derive(Deserialize)]
pub(crate) struct PlayerData {
    #[serde(rename = "intLevel", default, deserialize_with = "flex::opt_int")]
    level: Option<u32>,
    #[serde(rename = "strClassName", default)]
    class_name: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UserEntry {
    pub(crate) uid: EntityId,
    pub(crate) data: PlayerData,
}

#[derive(Deserialize)]
pub(crate) struct UserList {
    #[serde(default)]
    pub(crate) a: Vec<UserEntry>,
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
    pub(crate) o: PlayerPatch,
}

#[derive(Deserialize)]
pub(crate) struct PlayerPatch {
    #[serde(flatten)]
    pub(crate) vitals: Vitals,
    #[serde(rename = "strFrame", default)]
    pub(crate) cell: Option<String>,
    #[serde(rename = "strPad", default)]
    pub(crate) pad: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Tick {
    #[serde(rename = "m", default)]
    pub(crate) monsters: HashMap<MonMapId, Vitals>,
    #[serde(rename = "p", default)]
    pub(crate) players: HashMap<String, Vitals>,
    #[serde(rename = "a", default)]
    pub(crate) auras: Vec<AuraChange>,
    #[serde(default)]
    pub(crate) sarsa: Vec<ActionResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requested_cell_applies_only_to_its_own_map() {
        let area = |name: &str| -> MoveToArea {
            serde_json::from_value(serde_json::json!({
                "areaId": 1, "areaName": name, "intType": "1",
                "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "r3", "strPad": "Right"}],
            }))
            .unwrap()
        };
        let spot = || Spot {
            map: "xantown".into(),
            cell: "r7".into(),
            pad: "Left".into(),
        };
        let me = Some(EntityId(7));
        let (room, _) = Room::arriving(area("tavern-1"), me, Some(spot()));
        assert_eq!((room.cell.as_str(), room.pad.as_str()), ("r3", "Right"));
        let (room, _) = Room::arriving(area("xantown-2"), me, Some(spot()));
        assert_eq!((room.cell.as_str(), room.pad.as_str()), ("r7", "Left"));
    }
}
