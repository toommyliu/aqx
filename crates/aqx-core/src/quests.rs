use std::collections::HashMap;

use aqx_protocol::de::{flag, int, text};
use aqx_protocol::{FactionId, ItemId, QuestId};
use serde::Deserialize;
use serde_json::Value;

use crate::world::{Me, World};

#[derive(Clone, Debug)]
pub struct Quest {
    pub id: QuestId,
    pub name: String,
    pub turn_in: Vec<Need>,
    pub hold: Vec<Need>,
    pub rewards: Vec<Reward>,
    pub once: bool,
    pub slot: i32,
    pub value: i32,
    pub period: Option<(String, u32)>,
    pub member: bool,
    pub level: u32,
    pub reputation: Option<(FactionId, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub item: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reward {
    pub item: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub kind: RewardKind,
    pub chance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewardKind {
    Fixed,
    Roll,
    Choice,
    Random,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestStatus {
    Locked,
    Available,
    Active,
    Completable,
    Done,
}

impl World {
    pub fn count(&self, item: ItemId) -> u32 {
        self.count_of(&crate::world::ItemRef::Id(item))
    }

    pub fn count_of(&self, item: &crate::world::ItemRef) -> u32 {
        self.carried().chain(&self.temp).filter(|i| item.matches(i)).map(|i| i.qty).sum()
    }

    pub fn name_of(&self, item: &crate::world::ItemRef) -> String {
        let crate::world::ItemRef::Id(id) = item else { return item.to_string() };
        let held = self.carried().chain(&self.temp).chain(&self.drops).chain(self.bank.iter()).find(|i| i.id == *id).map(|i| i.name.clone());
        held.or_else(|| self.quests.values().flat_map(|q| q.turn_in.iter().chain(&q.hold)).find(|n| n.item == *id).map(|n| n.name.clone()))
            .unwrap_or_else(|| item.to_string())
    }

    pub fn quest_status(&self, id: QuestId) -> Option<QuestStatus> {
        let quest = self.quests.get(&id)?;
        let me = self.me.as_ref()?;
        if self.active.contains(&id) {
            let ready = quest.turn_in.iter().all(|n| self.count(n.item) >= n.qty);
            return Some(if ready { QuestStatus::Completable } else { QuestStatus::Active });
        }
        let done_once = quest.once && (quest.slot < 0 || quest_value(me, quest.slot) >= quest.value);
        let done_for_now = quest.period.as_ref().is_some_and(|(field, bit)| achievement(me, field, *bit));
        if done_once || done_for_now {
            return Some(QuestStatus::Done);
        }
        let below_rep = quest.reputation.is_some_and(|(faction, rep)| self.factions.iter().find(|f| f.id == faction).is_none_or(|f| f.rep < rep));
        let locked = (quest.member && !me.member)
            || (quest.slot >= 0 && quest_value(me, quest.slot) < quest.value - 1)
            || quest.hold.iter().any(|n| self.count(n.item) < n.qty)
            || me.level < quest.level
            || below_rep;
        Some(if locked { QuestStatus::Locked } else { QuestStatus::Available })
    }

    pub fn quest_value(&self, slot: i32) -> i32 {
        self.me.as_ref().map_or(-1, |me| quest_value(me, slot))
    }

    pub fn achievement(&self, field: &str, bit: u32) -> bool {
        self.me.as_ref().is_some_and(|me| achievement(me, field, bit))
    }
}

pub(crate) fn quest_value(me: &Me, slot: i32) -> i32 {
    let (string, at) = ((slot / 100) as usize, (slot % 100) as usize);
    me.quest_slots.get(string).and_then(|s| s.chars().nth(at)).and_then(|c| c.to_digit(36)).map_or(-1, |d| d as i32)
}

pub(crate) fn set_quest_value(me: &mut Me, slot: i32, value: u32) {
    let (string, at) = ((slot / 100) as usize, (slot % 100) as usize);
    let Some(digit) = char::from_digit(value, 36) else { return };
    if me.quest_slots.len() <= string {
        me.quest_slots.resize(string + 1, String::new());
    }
    let mut chars: Vec<char> = me.quest_slots[string].chars().collect();
    if chars.len() <= at {
        chars.resize(at + 1, '0');
    }
    chars[at] = digit;
    me.quest_slots[string] = chars.into_iter().collect();
}

fn achievement(me: &Me, field: &str, bit: u32) -> bool {
    me.achievements.get(field).is_some_and(|v| bit < 64 && v >> bit & 1 == 1)
}

#[derive(Deserialize)]
pub(crate) struct QuestData {
    #[serde(rename = "QuestID")]
    id: QuestId,
    #[serde(rename = "sName", default, deserialize_with = "text")]
    name: String,
    #[serde(default)]
    turnin: Vec<NeedData>,
    #[serde(default)]
    reqd: Vec<NeedData>,
    #[serde(rename = "oItems", default)]
    turn_in_items: Value,
    #[serde(rename = "oReqd", default)]
    hold_items: Value,
    #[serde(rename = "oRewards", default)]
    reward_items: Value,
    #[serde(default)]
    reward: Vec<RewardData>,
    #[serde(rename = "bOnce", default, deserialize_with = "flag")]
    once: bool,
    #[serde(rename = "iSlot", default, deserialize_with = "int")]
    slot: i64,
    #[serde(rename = "iValue", default, deserialize_with = "int")]
    value: i64,
    #[serde(rename = "sField", default, deserialize_with = "text")]
    field: String,
    #[serde(rename = "iIndex", default, deserialize_with = "int")]
    index: i64,
    #[serde(rename = "bUpg", default, deserialize_with = "flag")]
    member: bool,
    #[serde(rename = "iLvl", default, deserialize_with = "int")]
    level: i64,
    #[serde(rename = "FactionID", default, deserialize_with = "int")]
    faction: i64,
    #[serde(rename = "iReqRep", default, deserialize_with = "int")]
    rep: i64,
}

#[derive(Deserialize)]
struct NeedData {
    #[serde(rename = "ItemID")]
    item: ItemId,
    #[serde(rename = "iQty", default, deserialize_with = "int")]
    qty: i64,
}

#[derive(Deserialize)]
struct RewardData {
    #[serde(rename = "ItemID")]
    item: ItemId,
    #[serde(rename = "iQty", default, deserialize_with = "int")]
    qty: i64,
    #[serde(rename = "iRate", default)]
    rate: f64,
    #[serde(rename = "iType", default, deserialize_with = "int")]
    kind: i64,
}

fn names(items: &Value, into: &mut HashMap<u32, (String, u32)>) {
    let Value::Object(map) = items else { return };
    for item in map.values() {
        if let (Some(id), Some(name)) = (item.get("ItemID").and_then(Value::as_u64), item.get("sName").and_then(Value::as_str)) {
            let max_stack = item.get("iStk").and_then(|n| n.as_u64().or_else(|| n.as_str()?.parse().ok())).unwrap_or(1);
            into.insert(id as u32, (name.to_string(), max_stack.min(u32::MAX as u64) as u32));
        } else {
            names(item, into);
        }
    }
}

impl QuestData {
    pub(crate) fn into_quest(self) -> Quest {
        let mut known = HashMap::new();
        for items in [&self.turn_in_items, &self.hold_items, &self.reward_items] {
            names(items, &mut known);
        }
        let name_of = |id: ItemId| known.get(&id.0).map_or_else(|| format!("item {id}"), |(name, _)| name.clone());
        let stack_of = |id: ItemId| known.get(&id.0).map_or(1, |(_, max_stack)| *max_stack);
        let need = |n: NeedData| Need { item: n.item, name: name_of(n.item), qty: n.qty.clamp(0, u32::MAX as i64) as u32, max_stack: stack_of(n.item) };
        let rewards = self
            .reward
            .into_iter()
            .map(|r| Reward {
                item: r.item,
                name: name_of(r.item),
                qty: r.qty.clamp(0, u32::MAX as i64) as u32,
                max_stack: stack_of(r.item),
                kind: match r.kind {
                    1 => RewardKind::Roll,
                    2 => RewardKind::Choice,
                    3 => RewardKind::Random,
                    _ => RewardKind::Fixed,
                },
                chance: r.rate,
            })
            .collect();
        Quest {
            id: self.id,
            name: self.name,
            turn_in: self.turnin.into_iter().map(need).collect(),
            hold: self.reqd.into_iter().map(need).collect(),
            rewards,
            once: self.once,
            slot: self.slot as i32,
            value: self.value as i32,
            period: (!self.field.is_empty()).then(|| (self.field, self.index.max(0) as u32)),
            member: self.member,
            level: self.level.max(0) as u32,
            reputation: (self.faction > 1 && self.rep > 0).then_some((FactionId(self.faction as u32), self.rep as u32)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use aqx_protocol::Message;

    use super::*;
    use crate::event::Event;
    use crate::world::{ItemRef, reduce};

    fn json(o: &str) -> String {
        format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#)
    }

    fn feed(world: &mut World, text: &str) -> Vec<Event> {
        reduce(world, &Message::parse(text).unwrap(), Instant::now())
    }

    fn hero(daily_bits: u32, once_value: char) -> World {
        let mut world = World::default();
        feed(&mut world, "%xt%loginResponse%-1%true%1001%hero%Welcome!%");
        let slots = format!("{}{once_value}", "0".repeat(89));
        let slots5 = format!("{}7", "0".repeat(10));
        feed(
            &mut world,
            &json(&format!(
                r#"{{"cmd":"initUserData","uid":1001,"data":{{"intLevel":30,"iUpg":0,"strQuests":"{slots}","strQuests5":"{slots5}","id4":"{daily_bits}"}}}}"#
            )),
        );
        feed(
            &mut world,
            &json(
                r#"{"cmd":"getQuests","quests":{
                "11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"FactionID":1,"iReqRep":0,
                      "turnin":[{"ItemID":87,"iQty":1}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch"}},
                      "oRewards":{"itemsS":{"0":{"ItemID":1027,"sName":"Twilly Twig"}}},"reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":0}]},
                "445":{"QuestID":445,"sName":"Tipping the Scales","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":true,"iLvl":0,
                      "turnin":[{"ItemID":2568,"iQty":1}],
                      "oRewards":{"itemsC":{"0":{"ItemID":2476,"sName":"Shark Bait's Armor"},"1":{"ItemID":2481,"sName":"Shark Sabre"}}},
                      "reward":[{"ItemID":2476,"iQty":1,"iRate":100,"iType":2},{"ItemID":2481,"iQty":1,"iRate":100,"iType":2}]},
                "2365":{"QuestID":2365,"sName":"Waking History","bOnce":true,"iSlot":89,"iValue":1,"bUpg":false,"iLvl":0,"turnin":[],"oRewards":{}},
                "8152":{"QuestID":8152,"sName":"Ultra Ezrajal","bOnce":false,"iSlot":410,"iValue":7,"sField":"id4","iIndex":4,"bUpg":false,"iLvl":0,"turnin":[]}}}"#,
            ),
        );
        world
    }

    #[test]
    fn reads_needs_and_rewards_by_kind() {
        let world = hero(0, '0');
        let twilly = &world.quests[&QuestId(11)];
        assert_eq!(twilly.turn_in, [Need { item: ItemId(87), name: "Treeant Branch".into(), qty: 1, max_stack: 1 }]);
        assert_eq!((twilly.rewards[0].name.as_str(), twilly.rewards[0].kind), ("Twilly Twig", RewardKind::Fixed));
        let scales = &world.quests[&QuestId(445)];
        assert_eq!(scales.rewards.iter().map(|r| (r.name.as_str(), r.kind)).collect::<Vec<_>>(), [
            ("Shark Bait's Armor", RewardKind::Choice),
            ("Shark Sabre", RewardKind::Choice)
        ]);
    }

    #[test]
    fn item_ids_are_named_from_what_the_world_has_seen() {
        let world = hero(0, '0');
        assert_eq!(world.name_of(&ItemRef::Id(ItemId(87))), "Treeant Branch");
        assert_eq!(world.name_of(&ItemRef::Id(ItemId(5))), "item 5");
        assert_eq!(world.name_of(&ItemRef::Name("Bone Dust".into())), "Bone Dust");
    }

    #[test]
    fn a_quest_goes_from_available_to_active_to_completable_and_back() {
        let mut world = hero(0, '0');
        let status = |w: &World| w.quest_status(QuestId(11));
        assert_eq!(status(&world), Some(QuestStatus::Available));
        feed(&mut world, &json(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#));
        assert_eq!(status(&world), Some(QuestStatus::Active));
        feed(&mut world, &json(r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#));
        assert_eq!(status(&world), Some(QuestStatus::Completable));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":11,"sName":"Twilly's New Staff","rewardObj":{"intGold":100}}"#)),
            [Event::QuestCompleted { id: QuestId(11), name: "Twilly's New Staff".into() }]
        );
        assert_eq!(status(&world), Some(QuestStatus::Available));
    }

    #[test]
    fn membership_one_time_slots_and_daily_flags_decide_the_rest() {
        let fresh = hero(0, '0');
        assert_eq!(fresh.quest_status(QuestId(445)), Some(QuestStatus::Locked));
        assert_eq!(fresh.quest_status(QuestId(2365)), Some(QuestStatus::Available));
        assert_eq!(fresh.quest_status(QuestId(8152)), Some(QuestStatus::Available));

        let mut veteran = hero(1 << 4, '1');
        assert_eq!(veteran.quest_status(QuestId(2365)), Some(QuestStatus::Done));
        assert_eq!(veteran.quest_status(QuestId(8152)), Some(QuestStatus::Done));
        feed(&mut veteran, &json(r#"{"cmd":"setAchievement","field":"id4","index":4,"value":0}"#));
        assert_eq!(veteran.quest_status(QuestId(8152)), Some(QuestStatus::Available));
        feed(&mut veteran, &json(r#"{"cmd":"updateQuest","iIndex":410,"iValue":5}"#));
        assert_eq!(veteran.quest_status(QuestId(8152)), Some(QuestStatus::Locked));
    }

    #[test]
    fn a_refused_turn_in_is_a_rejection() {
        let mut world = hero(0, '0');
        assert_eq!(
            feed(&mut world, "%xt%zm%tryQuestComplete%167964%11%-1%false%1%wvz%"),
            [Event::QuestRejected { id: QuestId(11), reason: "the server refused it".into() }]
        );
    }
}
