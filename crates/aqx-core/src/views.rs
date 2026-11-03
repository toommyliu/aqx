use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{Aura, Event, Item, Monster, Place, Player, Quest, QuestStatus, RewardKind, Server, Shop, SkillTarget, State, World};
use aqx_protocol::Message;
use serde_json::{Value, json};

pub fn player(world: &World) -> Value {
    let Some(me) = &world.me else { return Value::Null };
    let stat = |key: &str| me.stats.get(key).copied().unwrap_or(0.0);
    json!({
        "name": me.user,
        "level": me.level,
        "hp": me.vitals.hp,
        "maxHp": me.vitals.max_hp,
        "mp": me.vitals.mp,
        "maxMp": me.vitals.max_mp,
        "state": state(me.vitals.state),
        "cell": me.cell,
        "pad": me.pad,
        "x": me.x,
        "y": me.y,
        "afk": me.afk,
        "target": world.target.and_then(|id| world.area.as_ref()?.monsters.iter().find(|m| m.map_id == id)).map_or(Value::Null, monster),
        "gold": me.gold,
        "member": me.member,
        "class": me.class,
        "stats": {
            "str": stat("$STR"),
            "int": stat("$INT"),
            "dex": stat("$DEX"),
            "wis": stat("$WIS"),
            "end": stat("$END"),
            "luk": stat("$LCK"),
            "haste": me.haste(),
            "crit": stat("$tcr"),
            "hit": stat("$thi"),
            "dodge": stat("$tdo"),
        },
        "auras": auras(&me.auras),
        "factions": world.factions.iter().map(|f| json!({ "id": f.id, "name": f.name, "rep": f.rep, "rank": f.rank(), "repInRank": f.rep_in_rank(), "repForNextRank": f.rep_for_next_rank() })).collect::<Vec<_>>(),
    })
}

pub fn map(world: &World) -> Value {
    let Some(area) = &world.area else { return json!({ "name": "", "room": 0, "areaId": 0, "monsters": [], "players": [] }) };
    let room = area.name.rsplit_once('-').and_then(|(_, room)| room.parse::<u32>().ok()).unwrap_or(0);
    json!({
        "name": area.map,
        "room": room,
        "areaId": area.id,
        "monsters": area.monsters.iter().map(monster).collect::<Vec<_>>(),
        "players": area.players.iter().map(other).collect::<Vec<_>>(),
    })
}

pub fn monster(m: &Monster) -> Value {
    json!({
        "mapId": m.map_id,
        "monId": m.mon_id,
        "name": m.name,
        "cell": m.cell,
        "hp": m.vitals.hp,
        "maxHp": m.vitals.max_hp,
        "alive": m.alive(),
        "auras": auras(&m.auras),
    })
}

fn other(p: &Player) -> Value {
    json!({
        "name": p.name,
        "level": p.level,
        "cell": p.cell,
        "pad": p.pad,
        "hp": p.vitals.hp,
        "maxHp": p.vitals.max_hp,
        "state": state(p.vitals.state),
    })
}

pub fn items(items: &[Item]) -> Value {
    items.iter().map(item).collect()
}

fn item(i: &Item) -> Value {
    json!({
        "id": i.id,
        "name": i.name,
        "qty": i.qty,
        "maxStack": i.max_stack,
        "type": i.kind,
        "equipped": i.equipped,
        "ac": i.ac,
        "member": i.member,
    })
}

pub fn skills(world: &World) -> Value {
    let Some(me) = &world.me else { return json!([]) };
    me.skills
        .iter()
        .enumerate()
        .map(|(slot, s)| {
            let kind = match s.target {
                SkillTarget::Hostile => "hostile",
                SkillTarget::Friendly => "friendly",
                SkillTarget::Myself => "self",
            };
            json!({
                "slot": slot,
                "name": s.name,
                "mana": s.mana,
                "cooldown": me.cooldown(s).as_millis() as u64,
                "kind": kind,
                "maxTargets": s.max_targets,
            })
        })
        .collect()
}

pub fn slots(world: &World) -> Value {
    let (bag, bank) = world.me.as_ref().map_or((0, 0), |m| (m.bag_slots, m.bank_slots));
    json!({ "bag": bag, "bagUsed": world.inventory.len(), "bank": bank, "bankUsed": world.bank_count })
}

pub fn quest(world: &World, quest: &Quest) -> Value {
    let status = match world.quest_status(quest.id) {
        Some(QuestStatus::Available) => "available",
        Some(QuestStatus::Active) => "active",
        Some(QuestStatus::Completable) => "completable",
        Some(QuestStatus::Done) => "done",
        Some(QuestStatus::Locked) | None => "locked",
    };
    let rewards: Vec<Value> = quest
        .rewards
        .iter()
        .map(|r| {
            let kind = match r.kind {
                RewardKind::Fixed => "fixed",
                RewardKind::Choice => "choice",
                RewardKind::Roll => "roll",
                RewardKind::Random => "random",
            };
            json!({ "item": r.item, "name": r.name, "qty": r.qty, "kind": kind })
        })
        .collect();
    json!({
        "id": quest.id,
        "name": quest.name,
        "status": status,
        "requirements": quest.turn_in.iter().map(|n| json!({ "item": n.item, "name": n.name, "qty": n.qty })).collect::<Vec<_>>(),
        "rewards": rewards,
    })
}

pub fn shop(shop: &Shop) -> Value {
    let items: Vec<Value> = shop
        .items
        .iter()
        .map(|s| json!({ "id": s.item.id, "name": s.item.name, "cost": s.cost, "ac": s.item.ac, "member": s.item.member, "stack": s.item.max_stack }))
        .collect();
    json!({ "id": shop.id, "name": shop.name, "items": items })
}

pub fn event_name(event: &Event) -> Option<&'static str> {
    Some(match event {
        Event::Died => "death",
        Event::Respawned => "respawn",
        Event::Joined { .. } => "map",
        Event::Moved { .. } => "cell",
        Event::Killed { .. } => "kill",
        Event::DropOffered { .. } => "drop",
        Event::ItemChanged { .. } => "item",
        Event::QuestAccepted { .. } | Event::QuestCompleted { .. } => "quest",
        Event::Chat { .. } => "chat",
        Event::Warning { .. } => "warning",
        Event::AuraChanged { .. } => "aura",
        Event::Countering { .. } => "counter",
        Event::Packet { .. } => "packet",
        _ => return None,
    })
}

pub fn event(world: &World, event: &Event) -> Value {
    match event {
        Event::Joined { area, .. } => {
            let (name, room) = area.rsplit_once('-').map_or((area.as_str(), 0), |(name, room)| (name, room.parse::<u32>().unwrap_or(0)));
            json!({ "name": name, "room": room })
        }
        Event::Moved { cell, pad } => json!({ "cell": cell, "pad": pad }),
        Event::Killed { id, monster: name } => {
            let found = world.area.as_ref().and_then(|a| a.monsters.iter().find(|m| m.map_id == *id));
            let mut killed = found.map_or_else(|| json!({ "mapId": id, "monId": 0, "name": name, "cell": "", "maxHp": 0, "auras": [] }), monster);
            killed["alive"] = json!(false);
            killed["hp"] = json!(0);
            killed
        }
        Event::DropOffered { id, item: name, qty } => {
            let ac = world.drops.iter().chain(&world.inventory).find(|i| i.id == *id).is_some_and(|i| i.ac);
            json!({ "id": id, "name": name, "qty": qty, "ac": ac })
        }
        Event::ItemChanged { item, qty, place } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp",
                Place::Bank => "bank",
            };
            json!({ "name": item, "qty": qty, "where": place })
        }
        Event::QuestAccepted { id, .. } => json!({ "id": id, "change": "accepted" }),
        Event::QuestCompleted { id, .. } => json!({ "id": id, "change": "completed" }),
        Event::Chat { channel, sender, text } => json!({ "from": sender, "text": text, "channel": channel }),
        Event::Warning { text } => json!({ "text": text }),
        Event::AuraChanged { monster, name, stacks, added } => {
            json!({ "name": name, "stacks": stacks, "change": if *added { "added" } else { "removed" }, "monster": monster })
        }
        Event::Countering { id, active } => json!({ "monster": id, "active": active }),
        Event::Packet { text } => packet(text),
        _ => Value::Null,
    }
}

pub fn packet(text: &str) -> Value {
    match Message::parse(text) {
        Ok(Message::Json(json)) => {
            json!({ "cmd": json.cmd, "data": serde_json::from_str::<Value>(json.body.get()).unwrap_or(Value::Null), "args": [], "raw": text })
        }
        Ok(Message::Xt(xt)) => json!({ "cmd": xt.cmd, "data": null, "args": xt.args().collect::<Vec<_>>(), "raw": text }),
        Ok(Message::Xml(xml)) => json!({ "cmd": xml.action.unwrap_or_default(), "data": null, "args": [], "raw": text }),
        Err(_) => json!({ "cmd": "", "data": null, "args": [], "raw": text }),
    }
}

pub fn server(server: &Server) -> Value {
    json!({ "name": server.name, "online": server.online, "members": server.members_only, "players": server.players, "capacity": server.capacity })
}

fn auras(auras: &[Aura]) -> Value {
    let (now, wall) = (Instant::now(), SystemTime::now());
    auras
        .iter()
        .map(|a| {
            let expires = a.until.map(|until| (wall + until.saturating_duration_since(now)).duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64));
            json!({ "name": a.name, "stacks": a.stacks, "expiresAt": expires })
        })
        .collect()
}

fn state(state: State) -> &'static str {
    match state {
        State::Dead => "dead",
        State::Idle => "idle",
        State::Combat => "combat",
    }
}
