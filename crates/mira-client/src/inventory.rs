use std::collections::BTreeMap;
use std::sync::Arc;

use mira_protocol::{CharItemId, EntityId, ItemId, flex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Event, Place};
use crate::rank::rank;
use crate::shop::{Bought, ShopItem};

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
    #[serde(rename(deserialize = "sMeta"), default)]
    pub meta: String,
    #[serde(
        rename(deserialize = "bHouse"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub house: bool,
}

impl Item {
    pub fn class_rank(&self) -> Option<u32> {
        (self.kind == "Class").then(|| rank(self.qty))
    }

    pub fn pool(&self) -> Pool {
        pool(&self.kind, &self.meta, self.house)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pool {
    Bag,
    Misc,
    Class,
    House,
}

fn pool(kind: &str, meta: &str, house: bool) -> Pool {
    let meta = meta.trim();
    let numeric_meta = !meta.is_empty() && meta.bytes().all(|b| b.is_ascii_digit());
    match kind {
        "Class" => Pool::Class,
        "House" | "Wall Item" | "Floor Item" | "Guild" => Pool::House,
        _ if house => Pool::House,
        "Item" if numeric_meta => Pool::Bag,
        "Item" | "Note" | "Quest Item" | "Resource" => Pool::Misc,
        _ => Pool::Bag,
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
    pub meta: String,
    pub house: bool,
}

impl DropOffer {
    pub fn pool(&self) -> Pool {
        pool(&self.kind, &self.meta, self.house)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Inventory {
    pub(crate) items: Arc<Vec<Item>>,
    pub(crate) temp: Vec<TempItem>,
    pub(crate) drops: Vec<DropOffer>,
}

impl Inventory {
    pub(crate) fn add(&mut self, key: &str, added: AddedItem) -> Option<Event> {
        let id = added.id.or_else(|| key.parse().ok().map(ItemId))?;
        if added.bank {
            return None;
        }
        let (name, qty, place) = match added.char_item_id.filter(|_| !added.temp) {
            None => match self.temp.iter_mut().find(|i| i.id == id) {
                Some(held) => {
                    held.qty = added.qty_now.unwrap_or(held.qty + added.qty);
                    (held.name.clone(), held.qty, Place::Temp)
                }
                None => {
                    let qty = added.qty_now.unwrap_or(added.qty);
                    self.temp.push(TempItem {
                        id,
                        name: added.name.clone(),
                        qty,
                        max_stack: added.max_stack,
                    });
                    (added.name, qty, Place::Temp)
                }
            },
            Some(char_item_id) => {
                let items = Arc::make_mut(&mut self.items);
                match items.iter_mut().find(|i| i.id == id) {
                    Some(held) => {
                        held.qty = added.qty_now.unwrap_or(held.qty + added.qty);
                        (held.name.clone(), held.qty, Place::Inventory)
                    }
                    None => {
                        let qty = added.qty_now.unwrap_or(added.qty);
                        items.push(Item {
                            id,
                            char_item_id,
                            name: added.name.clone(),
                            qty,
                            max_stack: added.max_stack,
                            kind: added.kind,
                            slot: added.slot,
                            equipped: false,
                            coins: added.coins,
                            member: added.member,
                            meta: added.meta,
                            house: added.house,
                        });
                        (added.name, qty, Place::Inventory)
                    }
                }
            }
        };
        Some(Event::ItemChanged {
            id,
            name,
            qty,
            place,
        })
    }

    pub(crate) fn offer(&mut self, key: &str, offered: AddedItem) -> Option<Event> {
        let id = offered.id.or_else(|| key.parse().ok().map(ItemId))?;
        let qty = offered.qty.max(1);
        let event = Event::DropOffered {
            id,
            name: offered.name.clone(),
            qty,
        };
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
                meta: offered.meta,
                house: offered.house,
            }),
        }
        Some(event)
    }

    pub(crate) fn pick(&mut self, picked: Picked) -> Option<Event> {
        if !picked.ok {
            return None;
        }
        let offer = self
            .drops
            .iter()
            .position(|d| d.id == picked.id)
            .map(|at| self.drops.remove(at));
        if picked.bank {
            return None;
        }
        let items = Arc::make_mut(&mut self.items);
        let (name, qty) = if let Some(held) = items.iter_mut().find(|i| i.id == picked.id) {
            held.qty = picked.qty_now.unwrap_or(held.qty + picked.qty);
            (held.name.clone(), held.qty)
        } else if let (Some(offer), Some(char_item_id)) = (offer, picked.char_item_id) {
            let qty = picked.qty_now.unwrap_or(picked.qty);
            items.push(Item {
                id: offer.id,
                char_item_id,
                name: offer.name.clone(),
                qty,
                max_stack: offer.max_stack,
                kind: offer.kind,
                slot: offer.slot,
                equipped: false,
                coins: offer.coins,
                member: offer.member,
                meta: offer.meta,
                house: offer.house,
            });
            (offer.name, qty)
        } else {
            return None;
        };
        Some(Event::ItemChanged {
            id: picked.id,
            name,
            qty,
            place: Place::Inventory,
        })
    }

    pub(crate) fn turn_in(&mut self, turned_in: &TurnIn) -> Vec<Event> {
        let mut events = Vec::new();
        for (id, qty) in turned_in.items.split(',').filter_map(|pair| {
            let (id, qty) = pair.split_once(':')?;
            Some((ItemId(id.parse().ok()?), qty.parse().ok()?))
        }) {
            if let Some(at) = self.items.iter().position(|i| i.id == id) {
                events.push(take(Arc::make_mut(&mut self.items), at, qty));
            } else if let Some(at) = self.temp.iter().position(|i| i.id == id) {
                let held = &mut self.temp[at];
                held.qty = held.qty.saturating_sub(qty);
                events.push(Event::ItemChanged {
                    id,
                    name: held.name.clone(),
                    qty: held.qty,
                    place: Place::Temp,
                });
                if held.qty == 0 {
                    self.temp.remove(at);
                }
            }
        }
        events
    }

    pub(crate) fn buy(&mut self, listed: &ShopItem, bought: &Bought) -> Option<Event> {
        if bought.bank {
            return None;
        }
        let items = Arc::make_mut(&mut self.items);
        let qty = match items.iter_mut().find(|i| i.id == listed.id) {
            Some(held) => {
                held.qty += bought.qty;
                held.qty
            }
            None => {
                items.push(Item {
                    id: listed.id,
                    char_item_id: bought.char_item_id?,
                    name: listed.name.clone(),
                    qty: bought.qty,
                    max_stack: listed.max_stack,
                    kind: listed.kind.clone(),
                    slot: listed.slot.clone(),
                    equipped: false,
                    coins: listed.coins,
                    member: listed.member,
                    meta: listed.meta.clone(),
                    house: listed.house,
                });
                bought.qty
            }
        };
        Some(Event::ItemChanged {
            id: listed.id,
            name: listed.name.clone(),
            qty,
            place: Place::Inventory,
        })
    }

    pub(crate) fn full_for(&self, offer: &DropOffer, slots: u32) -> bool {
        self.no_room(offer.id, offer.pool(), slots)
    }

    pub(crate) fn full_for_item(&self, item: &Item, slots: u32) -> bool {
        self.no_room(item.id, item.pool(), slots)
    }

    fn no_room(&self, id: ItemId, pool: Pool, slots: u32) -> bool {
        let owned = self.items.iter().find(|i| i.id == id);
        if owned.is_some_and(|i| i.qty < i.max_stack) {
            return false;
        }
        let used = |pool| self.items.iter().filter(|i| i.pool() == pool).count();
        let slots = slots as usize;
        match pool {
            Pool::Class | Pool::House => false,
            Pool::Misc => owned.is_none() && used(Pool::Misc) >= slots,
            Pool::Bag => used(Pool::Bag) >= slots,
        }
    }

    pub(crate) fn sell(&mut self, sold: &Sold) -> Option<Event> {
        let at = self
            .items
            .iter()
            .position(|i| i.char_item_id == sold.char_item_id)?;
        Some(take(Arc::make_mut(&mut self.items), at, sold.qty))
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

fn take(items: &mut Vec<Item>, at: usize, qty: u32) -> Event {
    let item = &mut items[at];
    item.qty = item.qty.saturating_sub(qty);
    let gone = item.qty == 0 || item.kind == "Class";
    let event = Event::ItemChanged {
        id: item.id,
        name: item.name.clone(),
        qty: if gone { 0 } else { item.qty },
        place: Place::Inventory,
    };
    if gone {
        items.remove(at);
    }
    event
}

#[derive(Deserialize)]
pub(crate) struct LoadInventory {
    #[serde(default)]
    pub(crate) items: Vec<Value>,
    #[serde(rename = "bankCount", default, deserialize_with = "flex::int")]
    pub(crate) bank_count: u32,
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
    #[serde(rename = "sMeta", default)]
    meta: String,
    #[serde(rename = "bHouse", default, deserialize_with = "flex::bool")]
    house: bool,
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
    pub(crate) char_item_id: CharItemId,
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
    pub(crate) ok: bool,
    #[serde(rename = "ItemID")]
    pub(crate) id: ItemId,
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
