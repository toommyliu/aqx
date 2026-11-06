use std::time::Instant;

use mira_client::{
    Aura, BoostKind, CombatState, DropOffer, Item, Me, Monster, Player, Pool, Quest, QuestId,
    QuestItem, QuestStatus, Readiness, RewardKind, Room, Shop, SkillTarget, TempItem, World,
};
use rquickjs::{Array, Ctx, IntoJs, Object, Result, Value};

pub fn view<'js>(ctx: &Ctx<'js>, world: &World, name: &str) -> Result<Value<'js>> {
    let now = Instant::now();
    let view = match name {
        "player" => world
            .me
            .as_ref()
            .map(|me| player(ctx, world, me, now))
            .transpose()?,
        "map" => world
            .room
            .as_ref()
            .map(|room| map(ctx, room, now))
            .transpose()?,
        "combat" => world
            .me
            .as_ref()
            .map(|me| combat(ctx, world, me, now))
            .transpose()?,
        "inventory" => world
            .me
            .as_ref()
            .map(|me| inventory(ctx, world, me))
            .transpose()?,
        "bank" => world
            .me
            .as_ref()
            .map(|me| bank(ctx, world, me))
            .transpose()?,
        "drops" => {
            let view = Object::new(ctx.clone())?;
            view.set("pending", list(ctx, &world.drops, |d| drop(ctx, d))?)?;
            Some(view)
        }
        "shop" => {
            let view = Object::new(ctx.clone())?;
            let current = world.shop.as_ref().map(|s| shop(ctx, s)).transpose()?;
            view.set("current", nullable(ctx, current)?)?;
            Some(view)
        }
        "currentShop" => world.shop.as_ref().map(|s| shop(ctx, s)).transpose()?,
        "bankItems" => {
            return match &world.bank {
                Some(items) => Ok(list(ctx, items, |i| item(ctx, world, i))?.into_value()),
                None => Ok(Value::new_null(ctx.clone())),
            };
        }
        _ => None,
    };
    Ok(view.map_or_else(|| Value::new_null(ctx.clone()), Object::into_value))
}

fn state(state: CombatState) -> &'static str {
    match state {
        CombatState::Idle => "idle",
        CombatState::InCombat => "combat",
        CombatState::Dead => "dead",
    }
}

fn list<'js, T>(
    ctx: &Ctx<'js>,
    items: &[T],
    mut each: impl FnMut(&T) -> Result<Object<'js>>,
) -> Result<Array<'js>> {
    let array = Array::new(ctx.clone())?;
    for (i, item) in items.iter().enumerate() {
        array.set(i, each(item)?)?;
    }
    Ok(array)
}

pub fn nullable<'js, T: IntoJs<'js>>(ctx: &Ctx<'js>, value: Option<T>) -> Result<Value<'js>> {
    match value {
        Some(value) => value.into_js(ctx),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_millis() as f64
}

fn auras<'js>(ctx: &Ctx<'js>, auras: &[Aura], now: Instant) -> Result<Array<'js>> {
    list(ctx, auras, |aura| {
        let view = Object::new(ctx.clone())?;
        view.set("name", aura.name.as_str())?;
        view.set("stacks", aura.stacks)?;
        view.set(
            "remainingMs",
            nullable(
                ctx,
                aura.expires.map(|at| ms(at.saturating_duration_since(now))),
            )?,
        )?;
        Ok(view)
    })
}

fn player<'js>(ctx: &Ctx<'js>, world: &World, me: &Me, now: Instant) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("entityId", me.id.0 as f64)?;
    view.set("charId", me.char_id as f64)?;
    view.set("name", me.name.as_str())?;
    view.set("level", me.level)?;
    view.set("exp", me.exp as f64)?;
    view.set("maxExp", me.exp_to_level as f64)?;
    view.set("hp", me.hp)?;
    view.set("maxHp", me.max_hp)?;
    view.set("mp", me.mp)?;
    view.set("maxMp", me.max_mp)?;
    view.set("state", state(me.state))?;
    view.set("away", me.away)?;
    view.set("gold", me.gold as f64)?;
    view.set("coins", me.coins as f64)?;
    view.set("member", me.member)?;
    view.set("class", me.class_name.as_str())?;
    view.set("classRank", me.class_rank())?;

    let stats = Object::new(ctx.clone())?;
    let s = &me.stats;
    for (key, value) in [
        ("strength", s.strength),
        ("endurance", s.endurance),
        ("dexterity", s.dexterity),
        ("intellect", s.intellect),
        ("wisdom", s.wisdom),
        ("luck", s.luck),
        ("hit", s.hit),
        ("evasion", s.evasion),
        ("crit", s.crit),
        ("haste", s.haste),
    ] {
        stats.set(key, value)?;
    }
    view.set("stats", stats)?;
    view.set("auras", auras(ctx, &me.auras.active, now)?)?;
    view.set(
        "factions",
        list(ctx, &world.factions, |faction| {
            let view = Object::new(ctx.clone())?;
            view.set("id", faction.id.0 as f64)?;
            view.set("name", faction.name.as_str())?;
            view.set("rep", faction.rep)?;
            view.set("rank", faction.rank)?;
            Ok(view)
        })?,
    )?;
    let boosts = Object::new(ctx.clone())?;
    for (key, kind) in [
        ("xp", BoostKind::Xp),
        ("gold", BoostKind::Gold),
        ("rep", BoostKind::Rep),
        ("classPoints", BoostKind::ClassPoints),
    ] {
        boosts.set(key, ms(me.boosts.remaining(kind, now)))?;
    }
    view.set("boosts", boosts)?;
    Ok(view)
}

fn map<'js>(ctx: &Ctx<'js>, room: &Room, now: Instant) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("name", room.name.as_str())?;
    view.set("areaId", room.area.0 as f64)?;
    view.set("cell", room.cell.as_str())?;
    view.set("pad", room.pad.as_str())?;
    view.set("x", room.x)?;
    view.set("y", room.y)?;
    view.set("zone", nullable(ctx, room.zone.as_deref())?)?;
    view.set(
        "players",
        list(ctx, &room.players, |p| other_player(ctx, p))?,
    )?;
    view.set(
        "monsters",
        list(ctx, &room.monsters, |m| monster(ctx, m, now))?,
    )?;
    Ok(view)
}

fn other_player<'js>(ctx: &Ctx<'js>, player: &Player) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("entityId", player.eid.0 as f64)?;
    view.set("name", player.name.as_str())?;
    view.set("level", nullable(ctx, player.level)?)?;
    view.set("class", nullable(ctx, player.class_name.as_deref())?)?;
    view.set("cell", nullable(ctx, player.cell.as_deref())?)?;
    view.set("pad", nullable(ctx, player.pad.as_deref())?)?;
    view.set("x", player.x)?;
    view.set("y", player.y)?;
    view.set("hp", player.hp)?;
    view.set("maxHp", player.max_hp)?;
    view.set("mp", player.mp)?;
    view.set("maxMp", player.max_mp)?;
    view.set("state", state(player.state))?;
    view.set("away", player.away)?;
    Ok(view)
}

pub fn monster<'js>(ctx: &Ctx<'js>, monster: &Monster, now: Instant) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("monMapId", monster.id.0 as f64)?;
    view.set("monId", monster.mon_id.0 as f64)?;
    view.set("name", monster.name.as_str())?;
    view.set("cell", monster.cell.as_str())?;
    view.set("hp", monster.hp)?;
    view.set("maxHp", monster.max_hp)?;
    view.set("state", state(monster.state))?;
    view.set("auras", auras(ctx, &monster.auras.active, now)?)?;
    Ok(view)
}

fn slot_number(slot: &str) -> Option<u32> {
    match slot {
        "aa" => Some(0),
        "a1" => Some(1),
        "a2" => Some(2),
        "a3" => Some(3),
        "a4" => Some(4),
        "i1" => Some(5),
        _ => None,
    }
}

fn combat<'js>(ctx: &Ctx<'js>, world: &World, me: &Me, now: Instant) -> Result<Object<'js>> {
    let skills = Array::new(ctx.clone())?;
    for skill in &me.skills {
        let Some(slot) = slot_number(&skill.slot) else {
            continue;
        };
        let view = Object::new(ctx.clone())?;
        view.set("slot", slot)?;
        view.set("name", skill.name.as_str())?;
        view.set("mana", skill.mana)?;
        view.set("cooldownMs", skill.cooldown_ms)?;
        view.set(
            "target",
            match skill.target {
                SkillTarget::Hostile => "hostile",
                SkillTarget::Friendly => "friendly",
                SkillTarget::Myself => "self",
            },
        )?;
        view.set("maxTargets", skill.max_targets)?;
        let status = Object::new(ctx.clone())?;
        let kind = match world.readiness.get(&skill.slot) {
            None | Some(Readiness::Ready) => "ready",
            Some(Readiness::At(at)) => {
                status.set("readyInMs", ms(at.saturating_duration_since(now)))?;
                "cooling"
            }
            Some(Readiness::Casting) => "casting",
            Some(Readiness::Disabled) => "disabled",
            Some(Readiness::Locked) => "locked",
            Some(Readiness::NoMana) => "no_mana",
        };
        status.set("kind", kind)?;
        view.set("status", status)?;
        skills.set(skills.len(), view)?;
    }
    let target = world.target.and_then(|id| {
        world
            .room
            .as_ref()
            .and_then(|room| room.monsters.iter().find(|m| m.id == id))
    });
    let view = Object::new(ctx.clone())?;
    view.set("skills", skills)?;
    let target = target.map(|m| monster(ctx, m, now)).transpose()?;
    view.set("target", nullable(ctx, target)?)?;
    Ok(view)
}

pub fn pool(pool: Pool) -> &'static str {
    match pool {
        Pool::Bag => "bag",
        Pool::Misc => "misc",
        Pool::Class => "class",
        Pool::House => "house",
    }
}

pub fn parse_pool(name: &str) -> Option<Pool> {
    match name {
        "bag" => Some(Pool::Bag),
        "misc" => Some(Pool::Misc),
        "class" => Some(Pool::Class),
        "house" => Some(Pool::House),
        _ => None,
    }
}

fn stack<'js>(ctx: &Ctx<'js>, id: i64, name: &str, qty: u32) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("id", id as f64)?;
    view.set("name", name)?;
    view.set("qty", qty)?;
    Ok(view)
}

fn temp_item<'js>(ctx: &Ctx<'js>, temp: &TempItem) -> Result<Object<'js>> {
    let view = stack(ctx, temp.id.0, &temp.name, temp.qty)?;
    view.set("maxStack", temp.max_stack)?;
    Ok(view)
}

fn item<'js>(ctx: &Ctx<'js>, world: &World, item: &Item) -> Result<Object<'js>> {
    let view = stack(ctx, item.id.0, &item.name, item.qty)?;
    view.set("maxStack", item.max_stack)?;
    view.set("charItemId", item.char_item_id.0 as f64)?;
    view.set("type", item.kind.as_str())?;
    view.set("slot", item.slot.as_str())?;
    view.set("pool", pool(item.pool()))?;
    view.set("meta", item.meta.as_str())?;
    view.set("equipped", item.equipped)?;
    view.set("coins", item.coins)?;
    view.set("member", item.member)?;
    view.set("enhancement", nullable(ctx, world.enhancement_name(item))?)?;
    Ok(view)
}

pub fn drop<'js>(ctx: &Ctx<'js>, offer: &DropOffer) -> Result<Object<'js>> {
    let view = stack(ctx, offer.id.0, &offer.name, offer.qty)?;
    view.set("maxStack", offer.max_stack)?;
    view.set("type", offer.kind.as_str())?;
    view.set("pool", pool(offer.pool()))?;
    view.set("coins", offer.coins)?;
    view.set("member", offer.member)?;
    Ok(view)
}

fn inventory<'js>(ctx: &Ctx<'js>, world: &World, me: &Me) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set(
        "items",
        list(ctx, &world.inventory, |i| item(ctx, world, i))?,
    )?;
    view.set("temp", list(ctx, &world.temp, |t| temp_item(ctx, t))?)?;
    view.set("bagSlots", me.bag_slots)?;
    view.set("houseSlots", me.house_slots)?;
    Ok(view)
}

fn bank<'js>(ctx: &Ctx<'js>, world: &World, me: &Me) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    let items = world
        .bank
        .as_ref()
        .map(|items| list(ctx, items, |i| item(ctx, world, i)))
        .transpose()?;
    view.set("items", nullable(ctx, items)?)?;
    view.set("slots", me.bank_slots)?;
    Ok(view)
}

fn stacks<'js>(ctx: &Ctx<'js>, items: &[QuestItem]) -> Result<Array<'js>> {
    list(ctx, items, |i| stack(ctx, i.id.0, &i.name, i.qty))
}

pub fn quest<'js>(ctx: &Ctx<'js>, world: &World, id: QuestId) -> Result<Value<'js>> {
    let Some(quest) = world.quests.get(&id) else {
        return Ok(Value::new_null(ctx.clone()));
    };
    quest_view(ctx, world, quest).map(Object::into_value)
}

fn quest_view<'js>(ctx: &Ctx<'js>, world: &World, quest: &Quest) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("id", quest.id.0 as f64)?;
    view.set("name", quest.name.as_str())?;
    let status = match world.quest_status(quest.id) {
        Some(QuestStatus::Done) => "done",
        Some(QuestStatus::DoneThisPeriod) => "done_this_period",
        Some(QuestStatus::Locked) => "locked",
        Some(QuestStatus::Available) | None => "available",
    };
    view.set("status", status)?;
    view.set("accepted", world.accepted_quests.contains(&quest.id))?;
    let turn_ins = match world.turn_in_limit(quest.id) {
        Some(Ok(times)) => times,
        Some(Err(_)) | None => 0,
    };
    view.set("turnIns", turn_ins)?;
    view.set("required", stacks(ctx, &quest.required)?)?;
    view.set("turnIn", stacks(ctx, &quest.turn_in)?)?;
    let class = quest
        .class
        .map(|id| -> Result<Object<'js>> {
            let class = Object::new(ctx.clone())?;
            class.set("id", id.0 as f64)?;
            class.set("points", quest.required_cp as f64)?;
            Ok(class)
        })
        .transpose()?;
    view.set("requiredClass", nullable(ctx, class)?)?;
    view.set("gold", quest.gold as f64)?;
    view.set("exp", quest.exp as f64)?;
    view.set(
        "rewards",
        list(ctx, &quest.rewards, |reward| {
            let view = stack(ctx, reward.id.0, &reward.name, reward.qty)?;
            let kind = match reward.kind {
                RewardKind::Always => "always",
                RewardKind::Rolled => "rolled",
                RewardKind::Chosen => "chosen",
                RewardKind::Random => "random",
            };
            view.set("kind", kind)?;
            Ok(view)
        })?,
    )?;
    Ok(view)
}

fn shop<'js>(ctx: &Ctx<'js>, shop: &Shop) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    view.set("id", shop.id.0 as f64)?;
    view.set("name", shop.name.as_str())?;
    view.set(
        "items",
        list(ctx, &shop.items, |listing| {
            let view = Object::new(ctx.clone())?;
            view.set("listingId", listing.listing.0 as f64)?;
            view.set("id", listing.id.0 as f64)?;
            view.set("name", listing.name.as_str())?;
            view.set("qty", listing.qty)?;
            view.set("cost", listing.cost as f64)?;
            view.set("coins", listing.coins)?;
            view.set("member", listing.member)?;
            view.set(
                "merge",
                list(ctx, &listing.merge, |cost| {
                    stack(ctx, cost.id.0, &cost.name, cost.qty)
                })?,
            )?;
            let token = (listing.token > 0 && listing.token_qty > 0)
                .then(|| -> Result<Object<'js>> {
                    let token = Object::new(ctx.clone())?;
                    token.set("id", listing.token as f64)?;
                    token.set("qty", listing.token_qty)?;
                    Ok(token)
                })
                .transpose()?;
            view.set("token", nullable(ctx, token)?)?;
            Ok(view)
        })?,
    )?;
    Ok(view)
}
