use std::time::Duration;

pub mod api;
mod aura;
mod combat;
mod event;
mod faction;
mod inventory;
mod player;
mod quest;
mod rank;
mod room;
mod session;
mod shop;
mod world;

pub use api::{Account, ApiError, DeviceId, Server};
pub use aura::{Aura, Auras, Caster, StatEffect};
pub use event::{Direction, Event, NoticeKind, Place};
pub use faction::Faction;
pub use inventory::{DropOffer, Item, Pool, TempItem};
pub use mira_protocol::{ItemId, MonMapId, QuestId, ShopId, ShopItemId};
pub use player::{BoostKind, Boosts, Me, Skill, SkillTarget};
pub use quest::{
    Quest, QuestItem, QuestPeriod, QuestProgress, QuestReward, QuestStatus, RewardKind,
};
pub use room::{CombatState, Monster, Player, Room};
pub use session::{
    ActionError, DEFAULT_CLIENT_VERSION, Disconnect, Handle, Load, Options, Pending, Phase, spawn,
};
pub use shop::{MergeCost, Shop, ShopItem};
pub use world::World;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
