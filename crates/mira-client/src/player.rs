use mira_protocol::{EntityId, FactionId, flex};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::quest::QuestProgress;
use crate::room::{CombatState, Vitals};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Me {
    pub level: u32,
    pub exp: u64,
    pub exp_to_level: u64,
    pub gold: u64,
    pub coins: u64,
    pub member: bool,
    pub class_points: u32,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub state: CombatState,
    pub haste: f64,
    pub skills: Vec<Skill>,
    pub quests: QuestProgress,
}

impl Me {
    pub(crate) fn apply(&mut self, patch: &Vitals) {
        self.hp = patch.hp.unwrap_or(self.hp);
        self.max_hp = patch.max_hp.unwrap_or(self.max_hp);
        self.mp = patch.mp.unwrap_or(self.mp);
        self.max_mp = patch.max_mp.unwrap_or(self.max_mp);
        self.state = patch.state.unwrap_or(self.state);
    }

    pub(crate) fn load(&mut self, data: &UserData) {
        self.level = data.level;
        self.gold = data.gold;
        self.coins = data.coins;
        self.member = data.member_days >= 0;
        self.class_points = data.class_points;
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

    pub(crate) fn level_up(&mut self, level: &LevelUp) {
        self.level = level.level;
        self.exp = level.exp;
        self.exp_to_level = level.exp_to_level;
    }

    pub(crate) fn receive(&mut self, amount: u64, coins: bool) {
        let purse = if coins {
            &mut self.coins
        } else {
            &mut self.gold
        };
        *purse += amount;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
pub(crate) struct Stats {
    #[serde(default)]
    pub(crate) sta: StatValues,
}

#[derive(Deserialize, Default)]
pub(crate) struct StatValues {
    #[serde(rename = "$tha")]
    pub(crate) haste: Option<f64>,
}

#[derive(Deserialize)]
pub(crate) struct SkillList {
    actions: Actions,
}

impl SkillList {
    pub(crate) fn into_skills(self) -> Vec<Skill> {
        self.actions.active
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
    gold: u64,
    #[serde(rename = "intExp", default, deserialize_with = "flex::int")]
    exp: u64,
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
