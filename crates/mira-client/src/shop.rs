use mira_protocol::{CharItemId, FactionId, ItemId, ShopId, ShopItemId, flex};
use serde::{Deserialize, Deserializer, Serialize};

use crate::inventory::{Pool, pool};
use crate::player::Me;
use crate::quest::Held;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shop {
    #[serde(rename(deserialize = "ShopID"))]
    pub id: ShopId,
    #[serde(rename(deserialize = "sName"), default)]
    pub name: String,
    #[serde(
        rename(deserialize = "bUpgrd"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub member: bool,
    #[serde(
        rename(deserialize = "bLimited"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub limited: bool,
    #[serde(rename(deserialize = "Location"), default, deserialize_with = "maps")]
    pub maps: Vec<String>,
    #[serde(
        rename(deserialize = "bHouse"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub house: bool,
    #[serde(rename(deserialize = "items"), default)]
    pub items: Vec<ShopItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopItem {
    #[serde(rename(deserialize = "ShopItemID"))]
    pub listing: ShopItemId,
    #[serde(rename(deserialize = "ItemID"))]
    pub id: ItemId,
    #[serde(rename(deserialize = "sName"), default)]
    pub name: String,
    #[serde(rename(deserialize = "iCost"), default, deserialize_with = "flex::int")]
    pub cost: u64,
    #[serde(
        rename(deserialize = "bCoins"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub coins: bool,
    #[serde(rename(deserialize = "bUpg"), default, deserialize_with = "flex::bool")]
    pub member: bool,
    #[serde(rename(deserialize = "iStk"), default, deserialize_with = "flex::int")]
    pub max_stack: u32,
    #[serde(rename(deserialize = "iQty"), default, deserialize_with = "flex::int")]
    pub qty: u32,
    #[serde(
        rename(deserialize = "FactionID"),
        default,
        deserialize_with = "faction"
    )]
    pub faction: Option<FactionId>,
    #[serde(
        rename(deserialize = "iReqRep"),
        default,
        deserialize_with = "flex::int"
    )]
    pub required_rep: u32,
    #[serde(rename(deserialize = "sType"), default)]
    pub kind: String,
    #[serde(rename(deserialize = "sES"), default)]
    pub slot: String,
    #[serde(rename(deserialize = "sMeta"), default)]
    pub meta: String,
    #[serde(rename(deserialize = "sLink"), default)]
    pub link: String,
    #[serde(
        rename(deserialize = "bHouse"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub house: bool,
    #[serde(rename(deserialize = "turnin"), default)]
    pub merge: Vec<MergeCost>,
    #[serde(rename(deserialize = "EnhID"), default, deserialize_with = "flex::int")]
    pub enhancement_id: i64,
    #[serde(rename(deserialize = "TokID"), default, deserialize_with = "flex::int")]
    pub token: i64,
    #[serde(
        rename(deserialize = "TokQty"),
        default,
        deserialize_with = "flex::int"
    )]
    pub token_qty: u32,
    #[serde(
        rename(deserialize = "iQSindex"),
        default = "no_slot",
        deserialize_with = "flex::int"
    )]
    pub quest_slot: i64,
    #[serde(
        rename(deserialize = "iQSvalue"),
        default,
        deserialize_with = "flex::int"
    )]
    pub quest_value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeCost {
    #[serde(rename(deserialize = "ItemID"))]
    pub id: ItemId,
    #[serde(rename(deserialize = "sName"), default)]
    pub name: String,
    #[serde(rename(deserialize = "iQty"), default, deserialize_with = "flex::int")]
    pub qty: u32,
}

const MAX_BUY: u32 = 100_000;

impl Shop {
    pub(crate) fn merges(&self) -> bool {
        !self.house && self.items.iter().any(|item| !item.merge.is_empty())
    }
}

impl ShopItem {
    pub fn pool(&self) -> Pool {
        pool(&self.kind, &self.meta, self.house)
    }

    fn token(&self) -> Option<(ItemId, u32)> {
        (self.token > 0 && self.token_qty > 0).then_some((ItemId(self.token), self.token_qty))
    }

    pub(crate) fn tokens_needed(&self, qty: u32) -> Option<(ItemId, u64)> {
        let (id, per) = self.token()?;
        Some((
            id,
            u64::from(per) * u64::from((qty / self.qty.max(1)).max(1)),
        ))
    }

    pub(crate) fn quest_locked(&self, me: &Me) -> bool {
        self.quest_slot >= 0 && me.quests.client_value(self.quest_slot) < self.quest_value as f64
    }

    pub(crate) fn most_buyable(&self, me: &Me, held: Held) -> u32 {
        if self.slot == "ar" {
            return 1;
        }
        let owned = |id: ItemId| held.items.iter().find(|i| i.id == id).map(|i| i.qty);
        let room = owned(self.id).map_or(self.max_stack, |qty| self.max_stack.saturating_sub(qty));
        let mut most = u64::from(room);
        if most < 1 {
            return 0;
        }
        let funds = if self.coins { me.coins } else { me.gold };
        if let Some(affordable) = funds.checked_div(self.cost) {
            most = most.min(affordable);
        }
        if !self.merge.is_empty() {
            for need in &self.merge {
                let sets = owned(need.id).map_or(0, |qty| qty / need.qty.max(1));
                if sets == 0 {
                    return 0;
                }
                most = most.min(u64::from(sets));
            }
            most *= u64::from(self.qty);
        }
        if let Some((id, per)) = self.token() {
            let have = held.qty(id).unwrap_or(0);
            most = most.min(u64::from(have / per) * u64::from(self.qty));
        }
        most.min(u64::from(MAX_BUY)).min(u64::from(room)) as u32
    }
}

fn no_slot() -> i64 {
    -1
}

#[derive(Deserialize)]
pub(crate) struct Bought {
    #[serde(rename = "bitSuccess", deserialize_with = "flex::bool")]
    pub(crate) ok: bool,
    #[serde(rename = "ItemID", default)]
    pub(crate) id: Option<ItemId>,
    #[serde(rename = "CharItemID", default)]
    pub(crate) char_item_id: Option<CharItemId>,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    pub(crate) qty: u32,
    #[serde(rename = "bBank", default, deserialize_with = "flex::bool")]
    pub(crate) bank: bool,
    #[serde(rename = "strMessage", default)]
    pub(crate) message: Option<String>,
}

fn maps<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let list = String::deserialize(d)?;
    Ok(list
        .split(',')
        .map(str::trim)
        .filter(|map| !map.is_empty())
        .map(str::to_owned)
        .collect())
}

fn faction<'de, D: Deserializer<'de>>(d: D) -> Result<Option<FactionId>, D::Error> {
    let id: i64 = flex::int(d)?;
    Ok((id > 1).then_some(FactionId(id)))
}

#[derive(Deserialize)]
pub(crate) struct LoadedShop {
    pub(crate) shopinfo: Shop,
}
