use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aqx_protocol::{FactionId, ItemId, QuestId, flex};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::faction::Faction;
use crate::inventory::{Item, TempItem};
use crate::player::Me;
use crate::world::World;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quest {
    pub id: QuestId,
    pub name: String,
    pub description: String,
    pub end_text: String,
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
    pub max_stack: u32,
    pub temp: bool,
    pub description: String,
    pub category: String,
    pub member: bool,
    pub coins: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestReward {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub temp: bool,
    pub description: String,
    pub category: String,
    pub member: bool,
    pub coins: bool,
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
        let held = Held {
            items: &self.inventory,
            temp: &self.temp,
        };
        Some(status(
            self.quests.get(&id)?,
            self.me.as_ref()?,
            held,
            &self.factions,
        ))
    }

    pub fn turn_in_limit(&self, id: QuestId) -> Option<Result<u32, String>> {
        let held = Held {
            items: &self.inventory,
            temp: &self.temp,
        };
        let accepted = self.accepted_quests.contains(&id);
        Some(turn_in_limit(self.quests.get(&id)?, accepted, held))
    }

    pub fn holds_turn_in(&self, id: QuestId) -> Option<bool> {
        let held = Held {
            items: &self.inventory,
            temp: &self.temp,
        };
        let quest = self.quests.get(&id)?;
        Some(
            quest
                .turn_in
                .iter()
                .all(|need| held.qty(need.id).is_some_and(|qty| qty >= need.qty)),
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Held<'a> {
    pub(crate) items: &'a [Item],
    pub(crate) temp: &'a [TempItem],
}

impl Held<'_> {
    pub(crate) fn qty(self, id: ItemId) -> Option<u32> {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.qty)
            .or_else(|| self.temp.iter().find(|i| i.id == id).map(|i| i.qty))
    }
}

pub(crate) fn status(quest: &Quest, me: &Me, held: Held, factions: &[Faction]) -> QuestStatus {
    let value = |slot| me.quests.client_value(slot);
    if quest.once && (quest.slot < 0 || value(quest.slot) >= quest.value as f64) {
        return QuestStatus::Done;
    }
    let period_used = quest.period.as_ref().is_some_and(|period| {
        let bit = u32::try_from(period.index).ok();
        bit.and_then(|bit| me.quests.achievement(&period.field, bit)) != Some(false)
    });
    if period_used {
        return QuestStatus::DoneThisPeriod;
    }
    let locked = (quest.member && !me.member)
        || (quest.slot >= 0 && value(quest.slot) < (quest.value - 1) as f64)
        || quest.level > i64::from(me.level)
        || quest
            .class
            .is_some_and(|class| held.qty(class).map_or(-1, i64::from) < quest.required_cp)
        || quest.faction.is_some_and(|faction| {
            let rep = factions
                .iter()
                .find(|f| f.id == faction)
                .map_or(0, |f| f.rep);
            i64::from(rep) < quest.required_rep
        })
        || quest
            .required
            .iter()
            .any(|need| held.qty(need.id).is_none_or(|qty| qty < need.qty));
    if locked {
        QuestStatus::Locked
    } else {
        QuestStatus::Available
    }
}

pub(crate) fn turn_in_limit(quest: &Quest, accepted: bool, held: Held) -> Result<u32, String> {
    if !accepted {
        return Err(format!("{} hasn't been accepted", quest.name));
    }
    let mut most = MAX_TURN_INS;
    for need in &quest.turn_in {
        let qty = held.qty(need.id).unwrap_or(0);
        if qty < need.qty {
            return Err(format!(
                "{} needs {} {}, and {qty} are held",
                quest.name, need.qty, need.name
            ));
        }
        if held.items.iter().any(|i| i.id == need.id && i.equipped) {
            return Err(format!("{} is equipped and can't be turned in", need.name));
        }
        most = most.min(qty / need.qty.max(1));
    }
    let repeatable = !quest.once && quest.period.is_none() && !quest.turn_in.is_empty();
    Ok(if repeatable { most } else { 1 })
}

const MAX_TURN_INS: u32 = 250;

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

impl QuestList {
    pub(crate) fn ids(&self) -> Vec<QuestId> {
        self.quests
            .keys()
            .filter_map(|key| key.parse().ok())
            .map(QuestId)
            .collect()
    }
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
    let mut metadata = HashMap::new();
    for items in [
        &data.turn_in_items,
        &data.required_items,
        &data.reward_items,
    ] {
        collect_items(items, &mut metadata);
    }
    let unknown = QuestItemData::default();
    let meta = |id: ItemId| metadata.get(&id).unwrap_or(&unknown);
    let item = |need: Need| {
        let meta = meta(need.id);
        QuestItem {
            id: need.id,
            name: meta.name.clone(),
            qty: need.qty,
            max_stack: meta.max_stack,
            temp: meta.temp,
            description: meta.description.clone(),
            category: meta.category.clone(),
            member: meta.member,
            coins: meta.coins,
        }
    };
    Ok(Quest {
        id,
        name: data.name,
        description: data.description,
        end_text: data.end_text,
        turn_in: data.turnin.into_iter().map(item).collect(),
        required: data.reqd.into_iter().map(item).collect(),
        rewards: data
            .reward
            .into_iter()
            .map(|reward| {
                let meta = meta(reward.id);
                QuestReward {
                    id: reward.id,
                    name: meta.name.clone(),
                    qty: reward.qty,
                    max_stack: meta.max_stack,
                    temp: meta.temp,
                    description: meta.description.clone(),
                    category: meta.category.clone(),
                    member: meta.member,
                    coins: meta.coins,
                    chance: reward.rate,
                    kind: reward.kind,
                }
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

#[derive(Deserialize, Default)]
struct QuestItemData {
    #[serde(rename = "sName", default)]
    name: String,
    #[serde(rename = "iStk", default, deserialize_with = "flex::int")]
    max_stack: u32,
    #[serde(rename = "bTemp", default, deserialize_with = "flex::bool")]
    temp: bool,
    #[serde(rename = "sDesc", default)]
    description: String,
    #[serde(rename = "sType", default)]
    category: String,
    #[serde(rename = "bUpg", default, deserialize_with = "flex::bool")]
    member: bool,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    coins: bool,
}

fn collect_items(value: &Value, items: &mut HashMap<ItemId, QuestItemData>) {
    let Value::Object(map) = value else { return };
    if let Some(id) = map.get("ItemID") {
        if let (Ok(id), Ok(item)) = (
            serde_json::from_value::<ItemId>(id.clone()),
            serde_json::from_value::<QuestItemData>(value.clone()),
        ) {
            items.insert(id, item);
        }
    } else {
        for nested in map.values() {
            collect_items(nested, items);
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
    #[serde(rename = "sDesc", default)]
    description: String,
    #[serde(rename = "sEndText", default)]
    end_text: String,
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
