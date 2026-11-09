use aqx_client::{
    ActionError, ActionId, Aim, EntityId, Handle, ItemId, MapItemId, MonMapId, QuestId, ShopId,
    ShopItemId, World,
};
use rquickjs::{Array, Object, Value};

const DEFAULT_PAD: &str = "Left";

pub enum Act {
    Rest,
    Join {
        map: String,
        spot: Option<(String, String)>,
    },
    Jump {
        cell: String,
        pad: String,
    },
    Walk {
        x: i32,
        y: i32,
    },
    Goto(String),
    Collect(i64),
    Kill(Ref, Option<Vec<&'static str>>),
    Attack(Ref),
    Skill {
        slot: &'static str,
        aim: Option<AimAt>,
    },
    Exit,
    Equip(Ref, bool),
    Use(Ref),
    LoadBank,
    Deposit(Ref),
    Withdraw(Ref),
    Swap(Ref, Ref),
    Pickup(Ref),
    LoadQuests(Vec<i64>),
    Accept(i64),
    Complete {
        id: i64,
        reward: Option<Ref>,
        times: u32,
    },
    LoadShop(i64),
    Buy {
        item: Listing,
        qty: Option<u32>,
    },
    Sell(Ref, u32),
    Send(String),
}

pub enum Ref {
    Id(i64),
    Name(String),
}

pub enum AimAt {
    Monster(Ref),
    Player(Ref),
}

pub enum Listing {
    Of(Ref),
    Id(i64),
}

#[derive(Clone, Copy)]
pub enum Answer {
    Nothing,
    Joined,
    Killed,
    Arrived,
    Picked,
    View(&'static str),
}

pub enum Started {
    Sent(ActionId, Answer),
    Answered(&'static str),
}

pub enum Reply {
    Nothing,
    Text(&'static str),
    View(&'static str),
}

impl Answer {
    pub fn reply(self, result: Result<(), ActionError>) -> Result<Reply, String> {
        match (self, result) {
            (Answer::Nothing, Ok(())) => Ok(Reply::Nothing),
            (Answer::Joined, Ok(())) => Ok(Reply::Text("joined")),
            (Answer::Joined, Err(ActionError::Refused(_))) => Ok(Reply::Text("refused")),
            (Answer::Killed, Ok(())) => Ok(Reply::Text("killed")),
            (Answer::Killed, Err(ActionError::Dead)) => Ok(Reply::Text("died")),
            (Answer::Arrived, Ok(())) => Ok(Reply::Text("arrived")),
            (Answer::Arrived, Err(ActionError::NoTarget(_))) => Ok(Reply::Text("not_found")),
            (Answer::Arrived, Err(ActionError::Refused(_))) => Ok(Reply::Text("refused")),
            (Answer::Picked, Ok(())) => Ok(Reply::Text("picked")),
            (Answer::Picked, Err(ActionError::Unusable(_))) => Ok(Reply::Text("no_room")),
            (Answer::Picked, Err(ActionError::NoTarget(_))) => Ok(Reply::Text("gone")),
            (Answer::View(view), Ok(())) => Ok(Reply::View(view)),
            (_, Err(e)) => Err(e.to_string()),
        }
    }
}

pub fn parse(op: &Object<'_>) -> Option<Act> {
    let text = |key: &str| op.get::<_, Option<String>>(key).ok();
    let reference = |key: &str| {
        op.get::<_, Option<Value>>(key)
            .ok()
            .map(|v| v.and_then(ref_of))
    };
    let item = || reference("item")?;
    match text("name")??.as_str() {
        "rest" => Some(Act::Rest),
        "join" => {
            let pad = text("pad")?.unwrap_or_else(|| DEFAULT_PAD.to_owned());
            Some(Act::Join {
                map: text("map")??,
                spot: text("cell")?.map(|cell| (cell, pad)),
            })
        }
        "jump" => Some(Act::Jump {
            cell: text("cell")??,
            pad: text("pad")?.unwrap_or_else(|| DEFAULT_PAD.to_owned()),
        }),
        "walk" => Some(Act::Walk {
            x: op.get("x").ok()?,
            y: op.get("y").ok()?,
        }),
        "goto" => Some(Act::Goto(text("player")??)),
        "collect" => Some(Act::Collect(op.get("item").ok()?)),
        "kill" => {
            let skills = match op.get::<_, Option<Array>>("skills").ok()? {
                Some(skills) => Some(
                    skills
                        .iter::<u32>()
                        .map(|slot| slot.ok().and_then(slot_name))
                        .collect::<Option<Vec<_>>>()?,
                ),
                None => None,
            };
            Some(Act::Kill(reference("target")??, skills))
        }
        "attack" => Some(Act::Attack(reference("target")??)),
        "skill" => {
            let slot = slot_name(op.get("slot").ok()?)?;
            let aim = match (reference("monster")?, reference("player")?) {
                (None, None) => None,
                (Some(monster), None) => Some(AimAt::Monster(monster)),
                (None, Some(player)) => Some(AimAt::Player(player)),
                (Some(_), Some(_)) => return None,
            };
            Some(Act::Skill { slot, aim })
        }
        "exit" => Some(Act::Exit),
        "equip" => Some(Act::Equip(item()?, true)),
        "unequip" => Some(Act::Equip(item()?, false)),
        "use" => Some(Act::Use(item()?)),
        "loadBank" => Some(Act::LoadBank),
        "deposit" => Some(Act::Deposit(item()?)),
        "withdraw" => Some(Act::Withdraw(item()?)),
        "swap" => Some(Act::Swap(item()?, reference("bankItem")??)),
        "pickup" => Some(Act::Pickup(item()?)),
        "loadQuests" => {
            let ids: Array = op.get("ids").ok()?;
            Some(Act::LoadQuests(
                ids.iter().collect::<rquickjs::Result<_>>().ok()?,
            ))
        }
        "accept" => Some(Act::Accept(op.get("id").ok()?)),
        "complete" => Some(Act::Complete {
            id: op.get("id").ok()?,
            reward: reference("reward")?,
            times: op.get("times").ok()?,
        }),
        "loadShop" => Some(Act::LoadShop(op.get("id").ok()?)),
        "buy" => {
            let listing = op.get::<_, Option<i64>>("listingId").ok()?;
            let item = match listing {
                Some(id) => Listing::Id(id),
                None => Listing::Of(item()?),
            };
            Some(Act::Buy {
                item,
                qty: op.get("qty").ok()?,
            })
        }
        "sell" => Some(Act::Sell(item()?, op.get("qty").ok()?)),
        "send" => Some(Act::Send(text("text")??)),
        _ => None,
    }
}

fn slot_name(slot: u32) -> Option<&'static str> {
    match slot {
        1 => Some("a1"),
        2 => Some("a2"),
        3 => Some("a3"),
        4 => Some("a4"),
        5 => Some("i1"),
        _ => None,
    }
}

fn ref_of(value: Value<'_>) -> Option<Ref> {
    if let Some(name) = value.as_string() {
        return name.to_string().ok().map(Ref::Name);
    }
    value.as_number().map(|id| Ref::Id(id as i64))
}

impl Act {
    pub fn start(self, session: &Handle, world: &World) -> Result<Started, String> {
        let sent = |pending: aqx_client::Pending, answer| Ok(Started::Sent(pending.id(), answer));
        let held = |item| held(world, item);
        match self {
            Act::Rest => sent(session.rest(), Answer::Nothing),
            Act::Join { map, spot: None } => sent(session.join(&map), Answer::Joined),
            Act::Join {
                map,
                spot: Some((cell, pad)),
            } => sent(session.join_at(&map, &cell, &pad), Answer::Joined),
            Act::Jump { cell, pad } => sent(session.move_to_cell(&cell, &pad), Answer::Nothing),
            Act::Walk { x, y } => sent(session.walk(x, y), Answer::Nothing),
            Act::Goto(player) => sent(session.goto(&player), Answer::Arrived),
            Act::Collect(item) => sent(session.get_map_item(MapItemId(item)), Answer::Nothing),
            Act::Kill(target, skills) => match (live_in_cell(world, &target), skills) {
                (Some(id), None) => sent(session.kill(id), Answer::Killed),
                (Some(id), Some(slots)) => sent(session.kill_with(id, &slots), Answer::Killed),
                (None, _) => Ok(Started::Answered("none")),
            },
            Act::Attack(target) => sent(session.attack(monster(world, target)?), Answer::Nothing),
            Act::Skill { slot, aim } => {
                let aim = match aim {
                    None => None,
                    Some(AimAt::Monster(target)) => Some(Aim::Monster(monster(world, target)?)),
                    Some(AimAt::Player(target)) => Some(Aim::Player(player(world, target)?)),
                };
                sent(session.use_skill(slot, aim), Answer::Nothing)
            }
            Act::Exit => sent(session.exit_combat(), Answer::Nothing),
            Act::Equip(item, true) => sent(session.equip(held(item)?), Answer::Nothing),
            Act::Equip(item, false) => sent(session.unequip(held(item)?), Answer::Nothing),
            Act::Use(item) => sent(session.use_item(held(item)?), Answer::Nothing),
            Act::LoadBank => sent(session.load_bank(), Answer::View("bankItems")),
            Act::Deposit(item) => sent(session.deposit(held(item)?), Answer::Nothing),
            Act::Withdraw(item) => sent(session.withdraw(banked(world, item)?), Answer::Nothing),
            Act::Swap(item, bank) => sent(
                session.swap(held(item)?, banked(world, bank)?),
                Answer::Nothing,
            ),
            Act::Pickup(item) => {
                let offer = world.drops.iter().find(|offer| match &item {
                    Ref::Id(id) => offer.id.0 == *id,
                    Ref::Name(name) => offer.name.eq_ignore_ascii_case(name),
                });
                match offer {
                    Some(offer) => sent(session.pick_drop(offer.id), Answer::Picked),
                    None => Ok(Started::Answered("gone")),
                }
            }
            Act::LoadQuests(ids) => {
                let ids: Vec<QuestId> = ids.into_iter().map(QuestId).collect();
                sent(session.load_quests(&ids), Answer::Nothing)
            }
            Act::Accept(id) => sent(session.accept_quest(QuestId(id)), Answer::Nothing),
            Act::Complete { id, reward, times } => {
                let reward = reward
                    .map(|reward| quest_reward(world, id, reward))
                    .transpose()?;
                sent(
                    session.complete_quest(QuestId(id), reward, times),
                    Answer::Nothing,
                )
            }
            Act::LoadShop(id) => sent(session.load_shop(ShopId(id)), Answer::View("currentShop")),
            Act::Buy { item, qty } => {
                let (listing, units) = listing(world, item)?;
                sent(session.buy(listing, qty.unwrap_or(units)), Answer::Nothing)
            }
            Act::Sell(item, qty) => sent(session.sell(held(item)?, qty), Answer::Nothing),
            Act::Send(text) => sent(session.send_raw(&text), Answer::Nothing),
        }
    }
}

fn live_in_cell(world: &World, target: &Ref) -> Option<MonMapId> {
    let room = world.room.as_ref()?;
    room.monsters
        .iter()
        .filter(|m| m.cell == room.cell && m.alive())
        .filter(|m| match target {
            Ref::Id(id) => m.id.0 == *id,
            Ref::Name(name) => m.name.eq_ignore_ascii_case(name),
        })
        .min_by_key(|m| m.hp)
        .map(|m| m.id)
}

fn monster(world: &World, target: Ref) -> Result<MonMapId, String> {
    match target {
        Ref::Id(id) => Ok(MonMapId(id)),
        Ref::Name(name) => live_in_cell(world, &Ref::Name(name.clone()))
            .ok_or_else(|| format!("no live {name} in this cell")),
    }
}

fn player(world: &World, target: Ref) -> Result<EntityId, String> {
    let name = match target {
        Ref::Id(id) => return Ok(EntityId(id)),
        Ref::Name(name) => name,
    };
    if let Some(me) = world
        .me
        .as_ref()
        .filter(|me| me.name.eq_ignore_ascii_case(&name))
    {
        return Ok(me.id);
    }
    world
        .room
        .iter()
        .flat_map(|room| &room.players)
        .find(|p| p.name.eq_ignore_ascii_case(&name))
        .map(|p| p.eid)
        .ok_or_else(|| format!("no player {name} in this map"))
}

fn held(world: &World, item: Ref) -> Result<ItemId, String> {
    match item {
        Ref::Id(id) => Ok(ItemId(id)),
        Ref::Name(name) => world
            .inventory
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(&name))
            .map(|i| i.id)
            .ok_or_else(|| format!("no {name} in the inventory")),
    }
}

fn banked(world: &World, item: Ref) -> Result<ItemId, String> {
    match item {
        Ref::Id(id) => Ok(ItemId(id)),
        Ref::Name(name) => {
            let Some(bank) = &world.bank else {
                return Err(format!("load the bank before naming {name} in it"));
            };
            bank.iter()
                .find(|i| i.name.eq_ignore_ascii_case(&name))
                .map(|i| i.id)
                .ok_or_else(|| format!("no {name} in the bank"))
        }
    }
}

fn quest_reward(world: &World, quest: i64, reward: Ref) -> Result<ItemId, String> {
    match reward {
        Ref::Id(id) => Ok(ItemId(id)),
        Ref::Name(name) => world
            .quests
            .get(&QuestId(quest))
            .and_then(|q| {
                q.rewards
                    .iter()
                    .find(|r| r.name.eq_ignore_ascii_case(&name))
            })
            .map(|r| r.id)
            .ok_or_else(|| format!("quest {quest} has no reward {name}")),
    }
}

fn listing(world: &World, item: Listing) -> Result<(ShopItemId, u32), String> {
    let Some(shop) = &world.shop else {
        return Err("load a shop before buying".into());
    };
    let wanted = match item {
        Listing::Id(id) => {
            return shop
                .items
                .iter()
                .find(|listing| listing.listing.0 == id)
                .map(|listing| (listing.listing, listing.qty))
                .ok_or_else(|| format!("{} has no listing {id}", shop.name));
        }
        Listing::Of(wanted) => wanted,
    };
    let mut matches = shop.items.iter().filter(|listing| match &wanted {
        Ref::Id(id) => listing.id.0 == *id,
        Ref::Name(name) => listing.name.eq_ignore_ascii_case(name),
    });
    let shown = || match &wanted {
        Ref::Id(id) => format!("item {id}"),
        Ref::Name(name) => name.clone(),
    };
    match (matches.next(), matches.next()) {
        (Some(listing), None) => Ok((listing.listing, listing.qty)),
        (None, _) => Err(format!("{} doesn't sell {}", shop.name, shown())),
        (Some(_), Some(_)) => Err(format!(
            "more than one listing sells {}; buy it by {{ listingId }}",
            shown()
        )),
    }
}
