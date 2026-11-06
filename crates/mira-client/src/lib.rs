use std::time::Duration;

pub mod api;
mod aura;
mod combat;
mod enhancement;
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
pub use combat::Readiness;
pub use enhancement::Enhancement;
pub use event::{Direction, Event, Holder, NoticeKind, Place};
pub use faction::Faction;
pub use inventory::{DropOffer, Item, Pool, TempItem};
pub use mira_protocol::{ItemId, MapItemId, MonMapId, QuestId, ShopId, ShopItemId};
pub use player::{BoostKind, Boosts, Me, Skill, SkillTarget, Stats};
pub use quest::{
    Quest, QuestItem, QuestPeriod, QuestProgress, QuestReward, QuestStatus, RewardKind,
};
pub use room::{CombatState, Monster, Player, Room};
pub use session::{
    ActionError, ActionId, Aggro, CounterHold, DEFAULT_CLIENT_VERSION, DEFAULT_RECONNECT_ATTEMPTS,
    Disconnect, Handle, Load, Options, Pending, Phase, Reconnect, spawn,
};
pub use shop::{MergeCost, Shop, ShopItem};
pub use world::World;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
