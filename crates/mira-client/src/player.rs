use std::time::{Duration, Instant};

const BOOST_LEAD: Duration = Duration::from_secs(1);

use mira_protocol::{EntityId, FactionId, ItemId, MonMapId, flex};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::aura::Auras;
use crate::combat::ITEM_SLOT;
use crate::quest::QuestProgress;
use crate::rank::rank;
use crate::room::{CombatState, Vitals};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Me {
    pub id: EntityId,
    pub name: String,
    pub level: u32,
    pub exp: u64,
    pub exp_to_level: u64,
    pub gold: u64,
    pub coins: u64,
    pub member: bool,
    pub member_days: i64,
    pub class_name: String,
    pub class_points: u32,
    pub char_id: i64,
    pub aq: u32,
    pub email_verified: bool,
    pub bag_slots: u32,
    pub house_slots: u32,
    pub bank_slots: u32,
    pub bank_count: u32,
    pub boosts: Boosts,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub state: CombatState,
    pub away: bool,
    pub stats: Stats,
    #[serde(skip)]
    pub(crate) mana_rate: Option<f64>,
    pub skills: Vec<Skill>,
    pub item_slot: Option<ItemId>,
    pub quests: QuestProgress,
    pub auras: Auras,
}

impl Me {
    pub fn class_rank(&self) -> u32 {
        rank(self.class_points)
    }

    pub(crate) fn apply(&mut self, patch: &Vitals) {
        self.hp = patch.hp.unwrap_or(self.hp);
        self.max_hp = patch.max_hp.unwrap_or(self.max_hp);
        self.mp = patch.mp.unwrap_or(self.mp);
        self.max_mp = patch.max_mp.unwrap_or(self.max_mp);
        self.state = patch.state.unwrap_or(self.state);
    }

    pub(crate) fn load(&mut self, data: &UserData, now: Instant) {
        for (kind, secs) in [
            (BoostKind::Xp, data.boost_xp),
            (BoostKind::Gold, data.boost_gold),
            (BoostKind::Rep, data.boost_rep),
            (BoostKind::ClassPoints, data.boost_class_points),
        ] {
            self.boosts.set(kind, secs, now);
        }
        self.level = data.level;
        self.gold = data.gold;
        self.coins = data.coins;
        self.member = data.member_days >= 0;
        self.member_days = data.member_days;
        self.class_points = data.class_points;
        self.char_id = data.char_id;
        self.aq = data.aq;
        self.email_verified = data.activation_flag == 5;
        self.bag_slots = data.bag_slots;
        self.house_slots = data.house_slots;
        self.bank_slots = data.bank_slots;
        self.exp = data.exp;
        self.exp_to_level = data.exp_to_level;
        self.quests = QuestProgress::from_user_data(&data.rest);
    }

    pub(crate) fn gain(&mut self, reward: &Reward) -> Option<u32> {
        self.gold += reward.gold;
        self.exp += reward.exp;
        let gained = reward.class_points?;
        self.class_points = self.class_points.saturating_add(gained);
        Some(self.class_points)
    }

    pub(crate) fn level_up(&mut self, level: &LevelUp) -> u32 {
        self.level = level.level;
        self.exp = level.exp;
        self.exp_to_level = level.exp_to_level;
        self.level
    }

    pub(crate) fn receive(&mut self, amount: u64, coins: bool) {
        let purse = if coins {
            &mut self.coins
        } else {
            &mut self.gold
        };
        *purse += amount;
    }

    pub(crate) fn spend(&mut self, amount: u64, coins: bool) {
        let purse = if coins {
            &mut self.coins
        } else {
            &mut self.gold
        };
        *purse = purse.saturating_sub(amount);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoostKind {
    Xp = 0,
    Gold = 1,
    Rep = 2,
    ClassPoints = 3,
}

impl BoostKind {
    const ALL: [BoostKind; 4] = [
        BoostKind::Xp,
        BoostKind::Gold,
        BoostKind::Rep,
        BoostKind::ClassPoints,
    ];

    fn from_cmd(cmd: &str) -> Option<BoostKind> {
        BoostKind::ALL.into_iter().find(|kind| kind.cmd() == cmd)
    }

    pub(crate) fn cmd(self) -> &'static str {
        match self {
            BoostKind::Xp => "xpboost",
            BoostKind::Gold => "gboost",
            BoostKind::Rep => "repboost",
            BoostKind::ClassPoints => "cpboost",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Boosts {
    ends: [Option<Instant>; 4],
}

impl Boosts {
    pub fn remaining(&self, kind: BoostKind, now: Instant) -> Duration {
        self.ends[kind as usize].map_or(Duration::ZERO, |end| end.saturating_duration_since(now))
    }

    pub(crate) fn ending(&self) -> impl Iterator<Item = (BoostKind, Instant)> + '_ {
        BoostKind::ALL
            .into_iter()
            .filter_map(|kind| Some((kind, self.ends[kind as usize]?.checked_sub(BOOST_LEAD)?)))
    }

    fn set(&mut self, kind: BoostKind, secs: u64, now: Instant) {
        self.ends[kind as usize] = (secs > 0).then(|| now + Duration::from_secs(secs));
    }

    pub(crate) fn apply(&mut self, cmd: &str, update: &BoostUpdate, now: Instant) {
        let Some(kind) = BoostKind::from_cmd(cmd) else {
            return;
        };
        let secs = if update.op == "+" { update.secs } else { 0 };
        self.set(kind, secs, now);
    }
}

impl Serialize for Boosts {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let now = Instant::now();
        let mut map = s.serialize_map(None)?;
        for kind in BoostKind::ALL {
            let left = self.remaining(kind, now);
            if !left.is_zero() {
                map.serialize_entry(&kind, &left.as_secs())?;
            }
        }
        map.end()
    }
}

#[derive(Deserialize)]
pub(crate) struct BoostUpdate {
    #[serde(default)]
    op: String,
    #[serde(rename = "iSecsLeft", default, deserialize_with = "flex::int")]
    secs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    #[serde(rename(deserialize = "ref"))]
    pub slot: String,
    #[serde(rename(deserialize = "nam"))]
    pub name: String,
    #[serde(rename(deserialize = "cd"), default, deserialize_with = "flex::int")]
    pub cooldown_ms: u32,
    #[serde(rename(deserialize = "mp"), default, deserialize_with = "flex::int")]
    pub mana: i32,
    #[serde(rename(deserialize = "tgt"))]
    pub target: SkillTarget,
    #[serde(
        rename(deserialize = "tgtMax"),
        default = "one",
        deserialize_with = "max_targets"
    )]
    pub max_targets: u32,
    #[serde(
        rename(deserialize = "isOK"),
        default = "yes",
        deserialize_with = "flex::bool"
    )]
    pub unlocked: bool,
    #[serde(skip)]
    pub(crate) old_cooldown_ms: Option<u32>,
    #[serde(flatten)]
    pub details: SkillDetails,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillDetails {
    #[serde(default, deserialize_with = "flex::int")]
    pub id: i64,
    #[serde(rename(deserialize = "actID"), default, deserialize_with = "flex::int")]
    pub act_id: i64,
    #[serde(default, deserialize_with = "flex::float")]
    pub damage: f64,
    #[serde(default)]
    pub icon: String,
    #[serde(rename(deserialize = "desc"), default)]
    pub description: String,
    #[serde(default, deserialize_with = "flex::int")]
    pub range: u32,
    #[serde(rename(deserialize = "typ"), default)]
    pub kind: String,
    #[serde(rename(deserialize = "anim"), default)]
    pub animation: String,
    #[serde(
        rename(deserialize = "tgtMin"),
        default,
        deserialize_with = "flex::int"
    )]
    pub min_targets: u32,
    #[serde(
        rename(deserialize = "skillLock"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub skill_lock: bool,
    #[serde(rename(deserialize = "lock"), default, deserialize_with = "flex::bool")]
    pub locked: bool,
}

#[derive(Deserialize)]
pub(crate) struct SlotSkill {
    #[serde(default, deserialize_with = "flex::opt_int")]
    cd: Option<u32>,
    #[serde(default, deserialize_with = "flex::opt_int")]
    mp: Option<i32>,
    #[serde(default)]
    tgt: Option<SkillTarget>,
    #[serde(rename = "tgtMax", default, deserialize_with = "flex::opt_int")]
    max_targets: Option<u32>,
}

fn one() -> u32 {
    1
}

fn max_targets<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    #[derive(Deserialize)]
    struct Count(#[serde(deserialize_with = "flex::int")] u32);
    Ok(Option::<Count>::deserialize(d)?.map_or(1, |count| count.0))
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillTarget {
    #[serde(rename(serialize = "hostile", deserialize = "h"))]
    Hostile,
    #[serde(rename(serialize = "friendly", deserialize = "f"))]
    Friendly,
    #[serde(rename(serialize = "self", deserialize = "s"))]
    Myself,
}

#[derive(Deserialize)]
pub(crate) struct InitUserData {
    pub(crate) uid: EntityId,
    pub(crate) data: UserData,
}

#[derive(Deserialize)]
pub(crate) struct UserData {
    #[serde(rename = "intLevel", deserialize_with = "flex::int")]
    level: u32,
    #[serde(rename = "intGold", deserialize_with = "flex::int")]
    gold: u64,
    #[serde(rename = "intCoins", default, deserialize_with = "flex::int")]
    coins: u64,
    #[serde(rename = "intExp", default, deserialize_with = "flex::int")]
    exp: u64,
    #[serde(rename = "intExpToLevel", default, deserialize_with = "flex::int")]
    exp_to_level: u64,
    #[serde(rename = "iUpgDays", default, deserialize_with = "flex::int")]
    member_days: i64,
    #[serde(rename = "iCP", default, deserialize_with = "flex::int")]
    class_points: u32,
    #[serde(rename = "CharID", default, deserialize_with = "flex::int")]
    char_id: i64,
    #[serde(rename = "intAQ", default, deserialize_with = "flex::int")]
    aq: u32,
    #[serde(rename = "intActivationFlag", default, deserialize_with = "flex::int")]
    activation_flag: u32,
    #[serde(rename = "iBagSlots", default, deserialize_with = "flex::int")]
    bag_slots: u32,
    #[serde(rename = "iHouseSlots", default, deserialize_with = "flex::int")]
    house_slots: u32,
    #[serde(rename = "iBankSlots", default, deserialize_with = "flex::int")]
    bank_slots: u32,
    #[serde(rename = "iBoostXP", default, deserialize_with = "flex::int")]
    boost_xp: u64,
    #[serde(rename = "iBoostG", default, deserialize_with = "flex::int")]
    boost_gold: u64,
    #[serde(rename = "iBoostRep", default, deserialize_with = "flex::int")]
    boost_rep: u64,
    #[serde(rename = "iBoostCP", default, deserialize_with = "flex::int")]
    boost_class_points: u64,
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub strength: f64,
    pub endurance: f64,
    pub dexterity: f64,
    pub intellect: f64,
    pub wisdom: f64,
    pub luck: f64,
    pub hit: f64,
    pub evasion: f64,
    pub crit: f64,
    pub haste: f64,
    pub attack_power: f64,
    pub spell_power: f64,
    pub critical_multiplier: f64,
}

#[derive(Deserialize)]
pub(crate) struct StatUpdate {
    #[serde(default)]
    sta: StatValues,
}

#[derive(Deserialize, Default)]
struct StatValues {
    #[serde(rename = "$STR")]
    strength: Option<f64>,
    #[serde(rename = "$END")]
    endurance: Option<f64>,
    #[serde(rename = "$DEX")]
    dexterity: Option<f64>,
    #[serde(rename = "$INT")]
    intellect: Option<f64>,
    #[serde(rename = "$WIS")]
    wisdom: Option<f64>,
    #[serde(rename = "$LCK")]
    luck: Option<f64>,
    #[serde(rename = "$thi")]
    hit: Option<f64>,
    #[serde(rename = "$tdo")]
    evasion: Option<f64>,
    #[serde(rename = "$tcr")]
    crit: Option<f64>,
    #[serde(rename = "$tha")]
    haste: Option<f64>,
    #[serde(rename = "$ap")]
    attack_power: Option<f64>,
    #[serde(rename = "$sp")]
    spell_power: Option<f64>,
    #[serde(rename = "$scm")]
    critical_multiplier: Option<f64>,
    #[serde(rename = "$cmc")]
    mana_rate: Option<f64>,
}

impl Me {
    pub(crate) fn apply_stats(&mut self, update: StatUpdate) {
        let sta = update.sta;
        let stats = &mut self.stats;
        for (to, from) in [
            (&mut stats.strength, sta.strength),
            (&mut stats.endurance, sta.endurance),
            (&mut stats.dexterity, sta.dexterity),
            (&mut stats.intellect, sta.intellect),
            (&mut stats.wisdom, sta.wisdom),
            (&mut stats.luck, sta.luck),
            (&mut stats.hit, sta.hit),
            (&mut stats.evasion, sta.evasion),
            (&mut stats.crit, sta.crit),
            (&mut stats.haste, sta.haste),
            (&mut stats.attack_power, sta.attack_power),
            (&mut stats.spell_power, sta.spell_power),
            (&mut stats.critical_multiplier, sta.critical_multiplier),
        ] {
            *to = from.unwrap_or(*to);
        }
        self.mana_rate = sta.mana_rate.or(self.mana_rate);
    }
}

#[derive(Deserialize)]
pub(crate) struct SkillList {
    actions: Actions,
}

impl Me {
    pub(crate) fn learn_skills(&mut self, list: SkillList) -> bool {
        let slot = self.skills.iter().find(|s| s.slot == ITEM_SLOT).cloned();
        self.skills = list.actions.active;
        let Some(slot) = slot else {
            return false;
        };
        match self.skills.iter_mut().find(|s| s.slot == ITEM_SLOT) {
            Some(incoming) => *incoming = slot,
            None => self.skills.push(slot),
        }
        true
    }

    pub(crate) fn fill_item_slot(&mut self, item: ItemId, update: &SlotSkill, cooling: bool) {
        self.item_slot = Some(item);
        let Some(skill) = self.skills.iter_mut().find(|s| s.slot == ITEM_SLOT) else {
            return;
        };
        if cooling && skill.old_cooldown_ms.is_none() {
            skill.old_cooldown_ms = Some(skill.cooldown_ms);
        }
        skill.cooldown_ms = update.cd.unwrap_or(skill.cooldown_ms);
        skill.mana = update.mp.unwrap_or(skill.mana);
        skill.target = update.tgt.unwrap_or(skill.target);
        skill.max_targets = update.max_targets.unwrap_or(1);
    }

    pub(crate) fn change_class(&mut self, class: &ClassChange) {
        self.class_name.clone_from(&class.class_name);
        self.class_points = class.class_points;
        self.auras.clear_passives();
    }
}

#[derive(Deserialize)]
struct Actions {
    #[serde(default)]
    active: Vec<Skill>,
}

#[derive(Deserialize)]
pub(crate) struct Reward {
    #[serde(rename = "intGold", default, deserialize_with = "flex::int")]
    pub(crate) gold: u64,
    #[serde(rename = "intExp", default, deserialize_with = "flex::int")]
    pub(crate) exp: u64,
    #[serde(rename = "typ", default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) id: Option<MonMapId>,
    #[serde(rename = "FactionID", default)]
    pub(crate) faction: Option<FactionId>,
    #[serde(rename = "iRep", default, deserialize_with = "flex::int")]
    pub(crate) rep: i64,
    #[serde(rename = "iCP", default, deserialize_with = "flex::opt_int")]
    class_points: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct ClassChange {
    pub(crate) uid: EntityId,
    #[serde(rename = "sClassName", default)]
    pub(crate) class_name: String,
    #[serde(rename = "iCP", deserialize_with = "flex::int")]
    pub(crate) class_points: u32,
}

#[derive(Deserialize)]
pub(crate) struct LevelUp {
    #[serde(rename = "intLevel", deserialize_with = "flex::int")]
    level: u32,
    #[serde(rename = "xp", default, deserialize_with = "flex::int")]
    exp: u64,
    #[serde(rename = "intExpToLevel", default, deserialize_with = "flex::int")]
    exp_to_level: u64,
}

#[derive(Deserialize)]
pub(crate) struct Balance {
    #[serde(rename = "intGold", default, deserialize_with = "flex::opt_int")]
    gold: Option<u64>,
    #[serde(rename = "intCoins", default, deserialize_with = "flex::opt_int")]
    coins: Option<u64>,
    #[serde(rename = "intExp", default, deserialize_with = "flex::opt_int")]
    exp: Option<u64>,
}
impl Me {
    pub(crate) fn balance(&mut self, balance: Balance) {
        self.gold = balance.gold.unwrap_or(self.gold);
        self.coins = balance.coins.unwrap_or(self.coins);
        self.exp = balance.exp.unwrap_or(self.exp);
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_link_and_verification_follow_user_data() {
        for (activation, verified) in [(0, false), (4, false), (5, true)] {
            let data: UserData = serde_json::from_value(
                json!({"intLevel": 1, "intGold": 0, "intAQ": "1", "intActivationFlag": activation}),
            )
            .unwrap();
            let mut me = Me::default();
            me.load(&data, Instant::now());
            let value = serde_json::to_value(me).unwrap();
            assert_eq!(value["aq"], 1);
            assert_eq!(value["email_verified"], verified);
        }
    }
}

#[cfg(test)]
mod skill_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_metadata_keeps_the_action_type_separate_from_the_target() {
        let skill: Skill = serde_json::from_value(json!({
            "ref":"a1", "nam":"Strike", "tgt":"h", "typ":"s", "id":"7", "actID":9,
            "damage":"1.5", "icon":"strike", "desc":"Strike description", "range":301,
            "anim":"Cast,Attack", "tgtMin":1, "skillLock":true
        }))
        .unwrap();
        let value = serde_json::to_value(skill).unwrap();
        assert_eq!(value["kind"], "s");
        assert_eq!(value["id"], 7);
        assert_eq!(value["damage"], 1.5);
        assert_eq!(value["skill_lock"], true);
    }
}
