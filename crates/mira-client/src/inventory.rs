use std::collections::BTreeMap;
use std::sync::Arc;

use mira_protocol::{CharItemId, EntityId, ItemId, flex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rank::rank;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    #[serde(rename(deserialize = "ItemID"))]
    pub id: ItemId,
    #[serde(rename(deserialize = "CharItemID"))]
    pub char_item_id: CharItemId,
    #[serde(rename(deserialize = "sName"))]
    pub name: String,
    #[serde(rename(deserialize = "iQty"), deserialize_with = "flex::int")]
    pub qty: u32,
    #[serde(rename(deserialize = "iStk"), default, deserialize_with = "flex::int")]
    pub max_stack: u32,
    #[serde(rename(deserialize = "sType"), default)]
    pub kind: String,
    #[serde(rename(deserialize = "sES"), default)]
    pub slot: String,
    #[serde(
        rename(deserialize = "bEquip"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub equipped: bool,
    #[serde(
        rename(deserialize = "bCoins"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub coins: bool,
    #[serde(rename(deserialize = "bUpg"), default, deserialize_with = "flex::bool")]
    pub member: bool,
}

impl Item {
    pub fn class_rank(&self) -> Option<u32> {
        (self.kind == "Class").then(|| rank(self.qty))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TempItem {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DropOffer {
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub kind: String,
    pub slot: String,
    pub coins: bool,
    pub member: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Inventory {
    pub(crate) items: Arc<Vec<Item>>,
    pub(crate) temp: Vec<TempItem>,
    pub(crate) drops: Vec<DropOffer>,
}

impl Inventory {
    pub(crate) fn add(&mut self, key: &str, added: AddedItem) {
        let Some(id) = added.id.or_else(|| key.parse().ok().map(ItemId)) else {
            return;
        };
        if added.bank {
            return;
        }
        match added.char_item_id.filter(|_| !added.temp) {
            None => match self.temp.iter_mut().find(|i| i.id == id) {
                Some(held) => held.qty = added.qty_now.unwrap_or(held.qty + added.qty),
                None => self.temp.push(TempItem {
                    id,
                    name: added.name,
                    qty: added.qty_now.unwrap_or(added.qty),
                    max_stack: added.max_stack,
                }),
            },
            Some(char_item_id) => {
                let items = Arc::make_mut(&mut self.items);
                match items.iter_mut().find(|i| i.id == id) {
                    Some(held) => held.qty = added.qty_now.unwrap_or(held.qty + added.qty),
                    None => items.push(Item {
                        id,
                        char_item_id,
                        name: added.name,
                        qty: added.qty_now.unwrap_or(added.qty),
                        max_stack: added.max_stack,
                        kind: added.kind,
                        slot: added.slot,
                        equipped: false,
                        coins: added.coins,
                        member: added.member,
                    }),
                }
            }
        }
    }

    pub(crate) fn offer(&mut self, key: &str, offered: AddedItem) {
        let Some(id) = offered.id.or_else(|| key.parse().ok().map(ItemId)) else {
            return;
        };
        let qty = offered.qty.max(1);
        match self.drops.iter_mut().find(|d| d.id == id) {
            Some(offer) => offer.qty += qty,
            None => self.drops.push(DropOffer {
                id,
                name: offered.name,
                qty,
                max_stack: offered.max_stack,
                kind: offered.kind,
                slot: offered.slot,
                coins: offered.coins,
                member: offered.member,
            }),
        }
    }

    pub(crate) fn pick(&mut self, picked: Picked) {
        if !picked.ok {
            return;
        }
        let offer = self
            .drops
            .iter()
            .position(|d| d.id == picked.id)
            .map(|at| self.drops.remove(at));
        if picked.bank {
            return;
        }
        let items = Arc::make_mut(&mut self.items);
        if let Some(held) = items.iter_mut().find(|i| i.id == picked.id) {
            held.qty = picked.qty_now.unwrap_or(held.qty + picked.qty);
        } else if let (Some(offer), Some(char_item_id)) = (offer, picked.char_item_id) {
            items.push(Item {
                id: offer.id,
                char_item_id,
                name: offer.name,
                qty: picked.qty_now.unwrap_or(picked.qty),
                max_stack: offer.max_stack,
                kind: offer.kind,
                slot: offer.slot,
                equipped: false,
                coins: offer.coins,
                member: offer.member,
            });
        }
    }

    pub(crate) fn turn_in(&mut self, turned_in: &TurnIn) {
        for (id, qty) in turned_in.items.split(',').filter_map(|pair| {
            let (id, qty) = pair.split_once(':')?;
            Some((ItemId(id.parse().ok()?), qty.parse().ok()?))
        }) {
            if let Some(at) = self.items.iter().position(|i| i.id == id) {
                take(Arc::make_mut(&mut self.items), at, qty);
            } else if let Some(at) = self.temp.iter().position(|i| i.id == id) {
                self.temp[at].qty = self.temp[at].qty.saturating_sub(qty);
                if self.temp[at].qty == 0 {
                    self.temp.remove(at);
                }
            }
        }
    }

    pub(crate) fn sell(&mut self, sold: &Sold) {
        if let Some(at) = self
            .items
            .iter()
            .position(|i| i.char_item_id == sold.char_item_id)
        {
            take(Arc::make_mut(&mut self.items), at, sold.qty);
        }
    }

    pub(crate) fn wear(&mut self, id: ItemId, on: bool) {
        let items = Arc::make_mut(&mut self.items);
        let slot = items.iter().find(|i| i.id == id).map(|i| i.slot.clone());
        for item in items.iter_mut() {
            if item.id == id {
                item.equipped = on;
            } else if on && Some(&item.slot) == slot.as_ref() {
                item.equipped = false;
            }
        }
    }

    pub(crate) fn set_class_points(&mut self, points: u32) {
        if let Some(class) = Arc::make_mut(&mut self.items)
            .iter_mut()
            .find(|i| i.slot == "ar" && i.equipped)
        {
            class.qty = points;
        }
    }
}

fn take(items: &mut Vec<Item>, at: usize, qty: u32) {
    items[at].qty = items[at].qty.saturating_sub(qty);
    if items[at].qty == 0 || items[at].kind == "Class" {
        items.remove(at);
    }
}

#[derive(Deserialize)]
pub(crate) struct LoadInventory {
    #[serde(default)]
    pub(crate) items: Vec<Value>,
    #[serde(default)]
    pub(crate) factions: Vec<Value>,
}

#[derive(Deserialize)]
pub(crate) struct AddedItems {
    pub(crate) items: BTreeMap<String, AddedItem>,
}

#[derive(Deserialize)]
pub(crate) struct AddedItem {
    #[serde(rename = "ItemID", default)]
    id: Option<ItemId>,
    #[serde(rename = "CharItemID", default)]
    char_item_id: Option<CharItemId>,
    #[serde(rename = "sName", default)]
    name: String,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iQtyNow", default, deserialize_with = "flex::opt_int")]
    qty_now: Option<u32>,
    #[serde(rename = "iStk", default, deserialize_with = "flex::int")]
    max_stack: u32,
    #[serde(rename = "sType", default)]
    kind: String,
    #[serde(rename = "sES", default)]
    slot: String,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    coins: bool,
    #[serde(rename = "bUpg", default, deserialize_with = "flex::bool")]
    member: bool,
    #[serde(rename = "bBank", default, deserialize_with = "flex::bool")]
    bank: bool,
    #[serde(rename = "bTemp", default, deserialize_with = "flex::bool")]
    temp: bool,
}

#[derive(Deserialize)]
pub(crate) struct TurnIn {
    #[serde(rename = "sItems", default)]
    items: String,
}

#[derive(Deserialize)]
pub(crate) struct Sold {
    #[serde(rename = "CharItemID")]
    char_item_id: CharItemId,
    #[serde(rename = "iQty", deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "intAmount", default, deserialize_with = "flex::int")]
    pub(crate) amount: u64,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    pub(crate) coins: bool,
}

#[derive(Deserialize)]
pub(crate) struct Picked {
    #[serde(rename = "bSuccess", deserialize_with = "flex::bool")]
    ok: bool,
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "CharItemID", default)]
    char_item_id: Option<CharItemId>,
    #[serde(rename = "iQty", default, deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iQtyNow", default, deserialize_with = "flex::opt_int")]
    qty_now: Option<u32>,
    #[serde(rename = "bBank", default, deserialize_with = "flex::bool")]
    bank: bool,
}

#[derive(Deserialize)]
pub(crate) struct Worn {
    pub(crate) uid: EntityId,
    #[serde(rename = "ItemID")]
    pub(crate) id: ItemId,
}
