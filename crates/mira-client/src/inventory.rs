use std::collections::BTreeMap;
use std::sync::Arc;

use mira_protocol::{CharItemId, EntityId, ItemId, flex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::enhancement::{Enhancement, EnhancementFields};
use crate::event::{Event, Place};
use crate::player::Me;
use crate::rank::rank;
use crate::shop::{Bought, ShopItem};

const GEAR_OF_DOOM: ItemId = ItemId(45739);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "WireItem")]
pub struct Item {
    pub id: ItemId,
    pub char_item_id: CharItemId,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub kind: String,
    pub slot: String,
    pub equipped: bool,
    pub coins: bool,
    pub member: bool,
    pub meta: String,
    pub link: String,
    pub house: bool,
    pub enhancement: Option<Enhancement>,
}

#[derive(Deserialize)]
struct WireItem {
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "CharItemID")]
    char_item_id: CharItemId,
    #[serde(rename = "sName")]
    name: String,
    #[serde(rename = "iQty", deserialize_with = "flex::int")]
    qty: u32,
    #[serde(rename = "iStk", default, deserialize_with = "flex::int")]
    max_stack: u32,
    #[serde(rename = "sType", default)]
    kind: String,
    #[serde(rename = "sES", default)]
    slot: String,
    #[serde(rename = "bEquip", default, deserialize_with = "flex::bool")]
    equipped: bool,
    #[serde(rename = "bCoins", default, deserialize_with = "flex::bool")]
    coins: bool,
    #[serde(rename = "bUpg", default, deserialize_with = "flex::bool")]
    member: bool,
    #[serde(rename = "sMeta", default)]
    meta: String,
    #[serde(rename = "sLink", default)]
    link: String,
    #[serde(rename = "bHouse", default, deserialize_with = "flex::bool")]
    house: bool,
    #[serde(flatten)]
    enhancement: EnhancementFields,
}

impl From<WireItem> for Item {
    fn from(wire: WireItem) -> Item {
        Item {
            id: wire.id,
            char_item_id: wire.char_item_id,
            name: wire.name,
            qty: wire.qty,
            max_stack: wire.max_stack,
            kind: wire.kind,
            slot: wire.slot,
            equipped: wire.equipped,
            coins: wire.coins,
            member: wire.member,
            meta: wire.meta,
            link: wire.link,
            house: wire.house,
            enhancement: wire.enhancement.enhancement(),
        }
    }
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

pub(crate) fn pool(kind: &str, meta: &str, house: bool) -> Pool {
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
    pub link: String,
    pub house: bool,
    #[serde(serialize_with = "computed_enhancement")]
    pub(crate) enhancement: EnhancementFields,
}

fn computed_enhancement<S: serde::Serializer>(
    fields: &EnhancementFields,
    s: S,
) -> Result<S::Ok, S::Error> {
    fields.enhancement().serialize(s)
}

impl DropOffer {
    pub fn pool(&self) -> Pool {
        pool(&self.kind, &self.meta, self.house)
    }

    pub fn enhancement(&self) -> Option<Enhancement> {
        self.enhancement.enhancement()
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
                            link: added.link,
                            house: added.house,
                            enhancement: added.enhancement.enhancement(),
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
                link: offered.link,
                house: offered.house,
                enhancement: offered.enhancement,
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
                link: offer.link,
                house: offer.house,
                enhancement: picked.enhancement.or(offer.enhancement).enhancement(),
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
                    link: listed.link.clone(),
                    house: listed.house,
                    enhancement: EnhancementFields::from_id(listed.enhancement_id).enhancement(),
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

    pub(crate) fn set_consumable(&mut self, slotted: Option<ItemId>) {
        for item in Arc::make_mut(&mut self.items) {
            if item.kind.eq_ignore_ascii_case("item") && !item.link.eq_ignore_ascii_case("none") {
                item.equipped = Some(item.id) == slotted;
            }
        }
    }

    pub(crate) fn full_for(&self, offer: &DropOffer, me: &Me) -> bool {
        self.no_room(offer.id, offer.pool(), me)
    }

    pub(crate) fn full_for_item(&self, item: &Item, me: &Me) -> bool {
        self.no_room(item.id, item.pool(), me)
    }

    pub(crate) fn full_for_purchase(&self, listing: &ShopItem, qty: u32, me: &Me) -> bool {
        let used = |pool| self.items.iter().filter(|i| i.pool() == pool).count();
        let owned = self.items.iter().find(|i| i.id == listing.id);
        let pool = listing.pool();
        if pool == Pool::House {
            return used(Pool::House) >= me.house_slots as usize;
        }
        if pool == Pool::Class || owned.is_some_and(|i| i.qty < i.max_stack) {
            return false;
        }
        let room = me.bag_slots as usize + self.freed_by(listing, qty, pool);
        match pool {
            Pool::Misc => owned.is_none() && used(Pool::Misc) >= room,
            _ => used(Pool::Bag) >= room,
        }
    }

    fn freed_by(&self, listing: &ShopItem, qty: u32, pool: Pool) -> usize {
        let held = |id: ItemId| self.items.iter().find(|i| i.id == id && i.pool() == pool);
        if self.items.iter().any(|i| i.id == listing.id) {
            return 0;
        }
        let sets = u64::from((qty / listing.qty.max(1)).max(1));
        let token = listing.tokens_needed(qty);
        let mut token_counted = false;
        let mut freed = 0;
        for cost in &listing.merge {
            let mut need = u64::from(cost.qty) * sets;
            if let Some((id, more)) = token
                && id == cost.id
            {
                token_counted = true;
                need += more;
            }
            let used_up =
                held(cost.id).is_some_and(|item| item.slot == "ar" || u64::from(item.qty) <= need);
            freed += usize::from(cost.id != GEAR_OF_DOOM && used_up);
        }
        if let Some((id, need)) = token
            && !token_counted
            && id != GEAR_OF_DOOM
            && held(id).is_some_and(|item| u64::from(item.qty) <= need)
        {
            freed += 1;
        }
        freed
    }

    fn no_room(&self, id: ItemId, pool: Pool, me: &Me) -> bool {
        no_room(&self.items, id, pool, me)
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
    #[serde(default)]
    pub(crate) hitems: Vec<Value>,
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
    #[serde(rename = "sLink", default)]
    link: String,
    #[serde(rename = "bHouse", default, deserialize_with = "flex::bool")]
    house: bool,
    #[serde(flatten)]
    enhancement: EnhancementFields,
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
    #[serde(flatten)]
    enhancement: EnhancementFields,
}

#[derive(Deserialize)]
pub(crate) struct Worn {
    pub(crate) uid: EntityId,
    #[serde(rename = "ItemID")]
    pub(crate) id: ItemId,
}

pub(crate) fn no_room(items: &[Item], id: ItemId, pool: Pool, me: &Me) -> bool {
    let used = |pool| items.iter().filter(|i| i.pool() == pool).count();
    let owned = items.iter().find(|i| i.id == id);
    let stacks = owned.is_some_and(|i| i.qty < i.max_stack);
    let bag = me.bag_slots as usize;
    match pool {
        Pool::Class => false,
        Pool::House => used(Pool::House) >= me.house_slots as usize,
        Pool::Misc => owned.is_none() && used(Pool::Misc) >= bag,
        Pool::Bag => !stacks && used(Pool::Bag) >= bag,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn inventory(items: Value) -> Inventory {
        Inventory {
            items: Arc::new(serde_json::from_value(items).unwrap()),
            ..Inventory::default()
        }
    }

    fn listing(item: Value) -> ShopItem {
        let mut base =
            json!({"ShopItemID": 1, "ItemID": 500, "sName": "Buy", "iQty": 1, "iStk": 1});
        base.as_object_mut()
            .unwrap()
            .extend(item.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn me(bag: u32, house: u32) -> Me {
        Me {
            bag_slots: bag,
            house_slots: house,
            ..Me::default()
        }
    }

    #[test]
    fn a_merge_purchase_counts_the_slots_its_inputs_free() {
        let held = |id: i64, kind: &str, qty: u32, slot: &str| {
            json!({"ItemID": id, "CharItemID": id, "sName": "Held", "iQty": qty, "iStk": 99,
                   "sType": kind, "sES": slot})
        };
        let full_bag = inventory(json!([
            held(1, "Axe", 1, "Weapon"),
            held(2, "Axe", 1, "Weapon")
        ]));
        let axe = listing(json!({"sType": "Axe"}));
        assert!(full_bag.full_for_purchase(&axe, 1, &me(2, 0)));
        assert!(!full_bag.full_for_purchase(&listing(json!({"sType": "Class"})), 1, &me(2, 0)));
        let stacking = listing(json!({"ItemID": 1, "sType": "Axe"}));
        assert!(!full_bag.full_for_purchase(&stacking, 1, &me(2, 0)));
        let consumes = |id: i64, qty: u32| {
            listing(json!({"sType": "Axe", "turnin": [{"ItemID": id, "iQty": qty}]}))
        };
        assert!(!full_bag.full_for_purchase(&consumes(1, 1), 1, &me(2, 0)));
        let kept = inventory(json!([
            held(45739, "Axe", 1, "Weapon"),
            held(2, "Axe", 1, "Weapon")
        ]));
        assert!(kept.full_for_purchase(&consumes(45739, 1), 1, &me(2, 0)));
        let worn = inventory(json!([
            held(1, "Axe", 5, "ar"),
            held(2, "Axe", 1, "Weapon")
        ]));
        assert!(!worn.full_for_purchase(&consumes(1, 1), 1, &me(2, 0)));
        let misc = inventory(json!([held(3, "Item", 1, "None")]));
        let gem = listing(json!({"sType": "Item"}));
        assert!(misc.full_for_purchase(&gem, 1, &me(1, 0)));
        let house = inventory(json!([held(4, "Wall Item", 1, "hi")]));
        let rug = listing(json!({"ItemID": 4, "sType": "Floor Item"}));
        assert!(house.full_for_purchase(&rug, 1, &me(5, 1)));
        assert!(!house.full_for_purchase(&rug, 1, &me(5, 2)));
    }
}
