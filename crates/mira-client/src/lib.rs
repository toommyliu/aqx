use std::time::Duration;

pub mod api;
mod aura;
mod combat;
mod enhancement;
mod event;
mod faction;
mod inventory;
mod map;
mod player;
mod quest;
mod rank;
mod room;
mod session;
mod shop;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod world;

pub use api::{Account, ApiError, DeviceId, Server};
pub use aura::{Aura, Auras, Caster, StatEffect};
pub use combat::Readiness;
pub use enhancement::Enhancement;
pub use event::{Direction, Event, Holder, NoticeKind, Place};
pub use faction::Faction;
pub use inventory::{DropOffer, Item, ItemDetails, Pool, TempItem};
pub use mira_protocol::packet::Aim;
pub use mira_protocol::{EntityId, ItemId, MapItemId, MonMapId, QuestId, ShopId, ShopItemId};
pub use player::{BoostKind, Boosts, Me, Skill, SkillDetails, SkillTarget, Stats};
pub use quest::{
    Quest, QuestItem, QuestPeriod, QuestProgress, QuestReward, QuestStatus, RewardKind,
};
pub use room::{CombatState, Monster, Player, Room};
pub use session::{
    ActionError, ActionId, Aggro, CounterHold, DEFAULT_CLIENT_VERSION, DEFAULT_RECONNECT_ATTEMPTS,
    Disconnect, Handle, Load, Options, Pending, Phase, Reconnect, Update, spawn,
};
pub use shop::{MergeCost, Shop, ShopItem};
pub use world::{ActionLock, Settings, SkillTiming, World};

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
