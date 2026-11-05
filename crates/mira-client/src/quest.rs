use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mira_protocol::{FactionId, ItemId, QuestId, flex};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quest {
    pub id: QuestId,
    pub name: String,
    pub turn_in: Vec<QuestItem>,
    pub required: Vec<QuestItem>,
    pub rewards: Vec<QuestReward>,
    pub once: bool,
    pub slot: i64,
    pub value: i64,
    pub period: Option<QuestPeriod>,
    pub member: bool,
    pub level: i64,
    pub class: Option<ItemId>,
    pub required_cp: i64,
    pub faction: Option<FactionId>,
    pub required_rep: i64,
    pub gold: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestItem {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestReward {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub chance: f64,
    pub kind: RewardKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardKind {
    #[default]
    Always,
    Rolled,
    Chosen,
    Random,
}

impl<'de> Deserialize<'de> for RewardKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match flex::int::<_, i64>(d)? {
            0 => Ok(RewardKind::Always),
            1 => Ok(RewardKind::Rolled),
            2 => Ok(RewardKind::Chosen),
            3 => Ok(RewardKind::Random),
            n => Err(serde::de::Error::custom(format!("unknown reward type {n}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestPeriod {
    pub field: String,
    pub index: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestStatus {
    Done,
    DoneThisPeriod,
    Locked,
    Available,
}

impl World {
    pub fn quest_status(&self, id: QuestId) -> Option<QuestStatus> {
        let quest = self.quests.get(&id)?;
        let me = self.me.as_ref()?;
        let value = |slot| me.quests.client_value(slot);
        if quest.once && (quest.slot < 0 || value(quest.slot) >= quest.value as f64) {
            return Some(QuestStatus::Done);
        }
        let period_used = quest.period.as_ref().is_some_and(|period| {
            let bit = u32::try_from(period.index).ok();
            bit.and_then(|bit| me.quests.achievement(&period.field, bit)) != Some(false)
        });
        if period_used {
            return Some(QuestStatus::DoneThisPeriod);
        }
        let held = |id: ItemId| self.inventory.iter().find(|i| i.id == id).map(|i| i.qty);
        let locked = (quest.member && !me.member)
            || (quest.slot >= 0 && value(quest.slot) < (quest.value - 1) as f64)
            || quest.level > i64::from(me.level)
            || quest
                .class
                .is_some_and(|class| held(class).map_or(-1, i64::from) < quest.required_cp)
            || quest.faction.is_some_and(|faction| {
                let rep = self
                    .factions
                    .iter()
                    .find(|f| f.id == faction)
                    .map_or(0, |f| f.rep);
                i64::from(rep) < quest.required_rep
            })
            || quest
                .required
                .iter()
                .any(|need| held(need.id).is_none_or(|qty| qty < need.qty));
        Some(if locked {
            QuestStatus::Locked
        } else {
            QuestStatus::Available
        })
    }
}

const QUEST_STRINGS: usize = 8;
const ACHIEVEMENT_FIELDS: [&str; 13] = [
    "ia0", "ia1", "iq0", "id0", "id1", "id2", "id3", "id4", "id5", "id6", "id7", "iw0", "im0",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QuestProgress {
    strings: [Option<String>; QUEST_STRINGS],
    achievements: BTreeMap<String, i64>,
}

impl QuestProgress {
    pub fn value(&self, slot: u32) -> Option<u32> {
        let string = self.strings.get((slot / 100) as usize)?.as_ref()?;
        string.chars().nth((slot % 100) as usize)?.to_digit(36)
    }

    pub(crate) fn client_value(&self, slot: i64) -> f64 {
        let Ok(slot) = usize::try_from(slot) else {
            return -1.0;
        };
        match self.strings.get(slot / 100) {
            Some(Some(string)) => string
                .chars()
                .nth(slot % 100)
                .and_then(|c| c.to_digit(36))
                .map_or(f64::NAN, f64::from),
            _ => -1.0,
        }
    }

    pub fn achievement(&self, field: &str, bit: u32) -> Option<bool> {
        if bit > 31 {
            return None;
        }
        Some(self.achievements.get(field)? & (1 << bit) != 0)
    }

    pub(crate) fn from_user_data(data: &serde_json::Map<String, Value>) -> QuestProgress {
        let int = |value: &Value| flex::int::<_, i64>(value.clone()).ok();
        QuestProgress {
            strings: std::array::from_fn(|i| {
                let key = match i {
                    0 => "strQuests".to_owned(),
                    n => format!("strQuests{}", n + 1),
                };
                data.get(&key).and_then(Value::as_str).map(str::to_owned)
            }),
            achievements: ACHIEVEMENT_FIELDS
                .iter()
                .filter_map(|&field| Some((field.to_owned(), int(data.get(field)?)?)))
                .collect(),
        }
    }

    pub(crate) fn update(&mut self, update: &QuestUpdate) {
        let Some(Some(string)) = self.strings.get_mut((update.slot / 100) as usize) else {
            return;
        };
        let digit = u32::try_from(update.value)
            .ok()
            .and_then(|value| char::from_digit(value, 36))
            .map_or('0', |c| c.to_ascii_uppercase());
        let at = (update.slot % 100) as usize;
        let mut chars: Vec<char> = string.chars().collect();
        match chars.get_mut(at) {
            Some(c) => *c = digit,
            None => chars.push(digit),
        }
        *string = chars.into_iter().collect();
    }

    pub(crate) fn set_achievement(&mut self, update: &AchievementUpdate) {
        let (bit, value) = (update.index, update.value);
        if bit > 31 || value > 1 {
            return;
        }
        let flags = self.achievements.entry(update.field.clone()).or_insert(0);
        if value == 1 {
            *flags |= 1 << bit;
        } else {
            *flags &= !(1 << bit);
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct QuestUpdate {
    #[serde(rename = "iIndex", deserialize_with = "flex::int")]
    slot: u32,
    #[serde(rename = "iValue", deserialize_with = "flex::int")]
    value: i64,
}

#[derive(Deserialize)]
pub(crate) struct AchievementUpdate {
    field: String,
    #[serde(deserialize_with = "flex::int")]
    index: u32,
    #[serde(deserialize_with = "flex::int")]
    value: u8,
}

#[derive(Deserialize)]
pub(crate) struct QuestList {
    #[serde(default)]
    quests: BTreeMap<String, Value>,
}

pub(crate) fn learn(quests: &mut Arc<BTreeMap<QuestId, Quest>>, list: QuestList) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, value) in list.quests {
        match parse(&key, value) {
            Ok(quest) if !quests.contains_key(&quest.id) => {
                Arc::make_mut(quests).insert(quest.id, quest);
            }
            Ok(_) => {}
            Err(e) => errors.push(format!("quest {key}: {e}")),
        }
    }
    errors
}

fn parse(key: &str, value: Value) -> Result<Quest, serde_json::Error> {
    let data: QuestData = serde_json::from_value(value)?;
    let id = match data.id {
        Some(id) => id,
        None => QuestId(
            key.parse()
                .map_err(|_| serde::de::Error::custom(format!("quest {key:?} has no QuestID")))?,
        ),
    };
    let mut names = HashMap::new();
    for items in [
        &data.turn_in_items,
        &data.required_items,
        &data.reward_items,
    ] {
        collect_names(items, &mut names);
    }
    let name = |id: ItemId| names.get(&id).cloned().unwrap_or_default();
    let item = |need: Need| QuestItem {
        id: need.id,
        name: name(need.id),
        qty: need.qty,
    };
    Ok(Quest {
        id,
        name: data.name,
        turn_in: data.turnin.into_iter().map(item).collect(),
        required: data.reqd.into_iter().map(item).collect(),
        rewards: data
            .reward
            .into_iter()
            .map(|reward| QuestReward {
                id: reward.id,
                name: name(reward.id),
                qty: reward.qty,
                chance: reward.rate,
                kind: reward.kind,
            })
            .collect(),
        once: data.once,
        slot: data.slot,
        value: data.value,
        period: data.field.map(|field| QuestPeriod {
            field,
            index: data.index,
        }),
        member: data.member,
        level: data.level,
        class: (data.class > 0).then_some(ItemId(data.class)),
        required_cp: data.required_cp,
        faction: (data.faction > 1).then_some(FactionId(data.faction)),
        required_rep: data.required_rep,
        gold: data.gold,
        exp: data.exp,
    })
}

fn collect_names(items: &Value, names: &mut HashMap<ItemId, String>) {
    let Value::Object(map) = items else { return };
    let id = map
        .get("ItemID")
        .and_then(|id| ItemId::deserialize(id.clone()).ok());
    match (id, map.get("sName").and_then(Value::as_str)) {
        (Some(id), Some(name)) => {
            names.insert(id, name.to_owned());
        }
        _ => {
            for nested in map.values() {
                collect_names(nested, names);
            }
        }
    }
}

fn minus_one() -> i64 {
    -1
}

#[derive(Deserialize)]
struct QuestData {
    #[serde(rename = "QuestID", default)]
    id: Option<QuestId>,
    #[serde(rename = "sName", default)]
    name: String,
    #[serde(default)]
    turnin: Vec<Need>,
    #[serde(default)]
    reqd: Vec<Need>,
    #[serde(rename = "oItems", default)]
    turn_in_items: Value,
    #[serde(rename = "oReqd", default)]
    required_items: Value,
    #[serde(rename = "oRewards", default)]
    reward_items: Value,
    #[serde(default)]
    reward: Vec<RewardData>,
    #[serde(rename = "bOnce", default, deserialize_with = "flex::bool")]
    once: bool,
    #[serde(
        rename = "iSlot",
        default = "minus_one",
        deserialize_with = "flex::int"
    )]
    slot: i64,
    #[serde(rename = "iValue", default, deserialize_with = "flex::int")]
    value: i64,
    #[serde(rename = "sField", default)]
    field: Option<String>,
    #[serde(rename = "iIndex", default, deserialize_with = "flex::int")]
    index: i64,
    #[serde(rename = "bUpg", default, deserialize_with = "flex::bool")]
    member: bool,
    #[serde(rename = "iLvl", default, deserialize_with = "flex::int")]
    level: i64,
    #[serde(rename = "iClass", default, deserialize_with = "flex::int")]
    class: i64,
    #[serde(rename = "iReqCP", default, deserialize_with = "flex::int")]
    required_cp: i64,
    #[serde(rename = "FactionID", default, deserialize_with = "flex::int")]
    faction: i64,
    #[serde(rename = "iReqRep", default, deserialize_with = "flex::int")]
    required_rep: i64,
    #[serde(rename = "iGold", default, deserialize_with = "flex::int")]
    gold: i64,
    #[serde(rename = "iExp", default, deserialize_with = "flex::int")]
    exp: i64,
}

#[derive(Deserialize)]
struct Need {
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
}

#[derive(Deserialize)]
struct RewardData {
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iRate", default, deserialize_with = "flex::float")]
    rate: f64,
    #[serde(rename = "iType", default)]
    kind: RewardKind,
}
