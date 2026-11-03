use std::time::{Duration, Instant};

use aqx_core::{ActError, Event, Fight, Halt, ItemRef, Monster, Session, Step, Target as Lasting, World};
use aqx_protocol::{ItemId, MonMapId, QuestId, ShopId};
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Array, Ctx, Error, Exception, FromJs, Function, Object, Result, Value};

use crate::bridge::{self, blocking, read, with_session};
use aqx_core::views;

const TICK: Duration = Duration::from_millis(250);

pub struct Native;

macro_rules! natives {
    ($($name:ident),* $(,)?) => {
        pub const NAMES: &[&str] = &[$(stringify!($name)),*];

        impl ModuleDef for Native {
            fn declare(d: &Declarations) -> Result<()> {
                $(d.declare(stringify!($name))?;)*
                Ok(())
            }

            fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
                $(e.export(stringify!($name), Function::new(ctx.clone(), $name)?.with_name(stringify!($name))?)?;)*
                Ok(())
            }
        }
    };
}

natives!(
    init, view, find, bank, quest, shop, join, jump, rest, kill, farm, withdraw, deposit, equip, pickup, accept, complete, buy, sell, wait_until, sleep, status,
    progress, stop, args, respawn, events_queued, listen, listen_packet, aggro, exit, use_skill, use_item, walk, goto, collect, send_packet, server,
    servers, relogin, room,
);

struct Ref(ItemRef);

impl<'js> FromJs<'js> for Ref {
    fn from_js(_: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if let Some(n) = value.as_number().filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= f64::from(u32::MAX)) {
            return Ok(Ref(ItemRef::Id(ItemId(n as u32))));
        }
        match value.as_string() {
            Some(name) => Ok(Ref(ItemRef::Name(name.to_string()?))),
            None => Err(Error::new_from_js_message(value.type_name(), "item", "expected an item name or id")),
        }
    }
}

fn js<'js>(ctx: &Ctx<'js>, value: &serde_json::Value) -> Result<Value<'js>> {
    ctx.json_parse(value.to_string())
}

fn ms(value: Option<f64>) -> Option<Duration> {
    value.filter(|ms| ms.is_finite()).map(|ms| Duration::from_millis(ms.max(0.0) as u64))
}

fn init<'js>(ctx: Ctx<'js>, make_error: Function<'js>, dispatch: Function<'js>) {
    bridge::init(&ctx, make_error, dispatch);
}

fn view<'js>(ctx: Ctx<'js>, what: String) -> Result<Value<'js>> {
    let value = read(&ctx, |world| match what.as_str() {
        "player" => Some(views::player(world)),
        "map" => Some(views::map(world)),
        "inventory" => Some(views::items(&world.inventory)),
        "temp" => Some(views::items(&world.temp)),
        "drops" => Some(views::items(&world.drops)),
        "skills" => Some(views::skills(world)),
        "slots" => Some(views::slots(world)),
        _ => None,
    })?;
    match value {
        Some(value) => js(&ctx, &value),
        None => Err(Exception::throw_message(&ctx, &format!("no view named {what}"))),
    }
}

fn find<'js>(ctx: Ctx<'js>, monsters: Value<'js>) -> Result<Value<'js>> {
    let target = target(&ctx, monsters)?;
    let found: Vec<Monster> = read(&ctx, |w| w.area.iter().flat_map(|a| &a.monsters).filter(|m| target(w, m)).cloned().collect())?;
    js(&ctx, &found.iter().map(views::monster).collect())
}

fn bank<'js>(ctx: Ctx<'js>) -> Result<Value<'js>> {
    if bridge::can_block() && !read(&ctx, |w| w.bank_loaded)? {
        blocking(&ctx, "bank.items", Session::load_bank)?;
    }
    js(&ctx, &read(&ctx, |w| views::items(&w.bank))?)
}

fn quest<'js>(ctx: Ctx<'js>, id: u32) -> Result<Value<'js>> {
    let id = QuestId(id);
    if !read(&ctx, |w| w.quests.contains_key(&id))? {
        if !bridge::can_block() {
            return Err(Exception::throw_message(&ctx, &format!("quest {id} isn't loaded; call quests.get({id}) before using it in a predicate")));
        }
        blocking(&ctx, "quests.get", |s| s.load_quests(&[id]))?;
    }
    js(&ctx, &read(&ctx, |w| w.quests.get(&id).map_or(serde_json::Value::Null, |q| views::quest(w, q)))?)
}

fn shop<'js>(ctx: Ctx<'js>, id: u32) -> Result<Value<'js>> {
    let id = ShopId(id);
    if !read(&ctx, |w| w.shops.contains_key(&id))? {
        blocking(&ctx, "shop.get", |s| s.load_shop(id))?;
    }
    js(&ctx, &read(&ctx, |w| w.shops.get(&id).map_or(serde_json::Value::Null, |s| views::shop(s)))?)
}

fn join(ctx: Ctx<'_>, map: String) -> Result<()> {
    blocking(&ctx, "map.join", |s| s.join(&map))
}

fn jump(ctx: Ctx<'_>, cell: String, pad: String) -> Result<()> {
    blocking(&ctx, "map.jump", |s| s.jump(&cell, &pad))
}

fn rest(ctx: Ctx<'_>) -> Result<()> {
    blocking(&ctx, "player.rest", Session::rest)
}

type Target<'js> = Box<dyn Fn(&World, &Monster) -> bool + 'js>;

enum Wanted {
    Any,
    Name(String),
    Id(MonMapId),
}

fn target<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Target<'js>> {
    if let Some(f) = value.as_function().cloned() {
        let ctx = ctx.clone();
        return Ok(Box::new(move |world, monster| bridge::check(&ctx, world, || f.call((js(&ctx, &views::monster(monster))?,)))));
    }
    Ok(listed(ctx, value)?)
}

fn lasting<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Lasting> {
    match value.as_function().cloned() {
        Some(f) => Ok(bridge::lasting_target(ctx, f)),
        None => listed(ctx, value),
    }
}

fn listed<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Lasting> {
    let mut wanted = Vec::new();
    for entry in Array::from_js(ctx, value)?.iter::<Value>() {
        let entry = entry?;
        wanted.push(match (entry.as_number(), entry.as_string()) {
            (Some(id), _) => Wanted::Id(MonMapId(id as u32)),
            (_, Some(name)) if name.to_string()? == "*" => Wanted::Any,
            (_, Some(name)) => Wanted::Name(name.to_string()?),
            _ => return Err(Error::new_from_js_message(entry.type_name(), "target", "expected a monster name or id")),
        });
    }
    Ok(Box::new(move |_, m| {
        wanted.iter().any(|w| match w {
            Wanted::Any => true,
            Wanted::Name(name) => m.name.eq_ignore_ascii_case(name),
            Wanted::Id(id) => m.map_id == *id,
        })
    }))
}

struct Plan<'js> {
    fight: Fight<'js>,
    cell: Option<(String, String)>,
    timeout: Option<Duration>,
    until: Option<Function<'js>>,
}

impl<'js> Plan<'js> {
    fn from(ctx: &Ctx<'js>, opts: &Object<'js>) -> Result<Self> {
        let mut steps = Vec::new();
        for step in opts.get::<_, Array>("skills")?.iter::<Object>() {
            let step = step?;
            let slot = match step.get::<_, u32>("slot")? {
                slot @ 0..=5 => slot as usize,
                slot => return Err(Exception::throw_type(ctx, &format!("there is no skill slot {slot}; slots are 0 to 5"))),
            };
            let when = step.get::<_, Option<Function>>("when")?.map(|f| bridge::predicate(ctx, f));
            steps.push(Step { slot, when });
        }
        let mut fight = Fight::new(steps);
        fight.strict = opts.get::<_, Option<bool>>("strict")?.unwrap_or(false);
        fight.hold = opts.get::<_, Option<Function>>("hold")?.map(|f| bridge::predicate(ctx, f));
        fight.anti_counter = opts.get::<_, Option<bool>>("antiCounter")?.unwrap_or(true);
        let cell = opts.get::<_, Option<String>>("cell")?;
        let pad = opts.get::<_, Option<String>>("pad")?.unwrap_or_else(|| "Left".into());
        Ok(Plan { fight, cell: cell.map(|c| (c, pad)), timeout: ms(opts.get("timeout")?), until: opts.get("until")? })
    }
}

fn spot(cell: &Option<(String, String)>) -> Option<(&str, &str)> {
    cell.as_ref().map(|(c, p)| (c.as_str(), p.as_str()))
}

fn kill<'js>(ctx: Ctx<'js>, monsters: Value<'js>, opts: Object<'js>) -> Result<()> {
    let target = target(&ctx, monsters)?;
    let Plan { mut fight, cell, timeout, until } = Plan::from(&ctx, &opts)?;
    let until = until.map(|f| bridge::goal(&ctx, f));
    blocking(&ctx, "combat.kill", |s| match until {
        Some(until) => s.kill_until(until, &*target, &mut fight, spot(&cell), timeout),
        None => s.kill(&*target, &mut fight, spot(&cell), timeout),
    })
}

fn farm<'js>(ctx: Ctx<'js>, goals: Array<'js>, monsters: Value<'js>, opts: Object<'js>) -> Result<()> {
    let mut wanted = Vec::new();
    for goal in goals.iter::<Array>() {
        let goal = goal?;
        wanted.push((goal.get::<Ref>(0)?.0, goal.get::<u32>(1)?));
    }
    let target = target(&ctx, monsters)?;
    let Plan { mut fight, cell, timeout, .. } = Plan::from(&ctx, &opts)?;
    blocking(&ctx, "combat.farm", |s| {
        for (item, _) in &wanted {
            if s.world.count_of(item) > 0 {
                continue;
            }
            match withdraw_if_banked(s, item) {
                Err(e @ ActError::Halted(_)) => return Err(e),
                Err(e) => bridge::emit(Event::ScriptLog { level: "warn".into(), text: format!("farming without checking the bank for {item}: {e}") }),
                Ok(()) => {}
            }
        }
        s.farm(&wanted, &*target, &mut fight, spot(&cell), timeout)
    })
}

fn withdraw_if_banked(s: &mut Session, item: &ItemRef) -> std::result::Result<(), ActError> {
    match s.withdraw(item) {
        Err(ActError::NotFound(_)) => Ok(()),
        other => other,
    }
}

fn withdraw(ctx: Ctx<'_>, item: Ref) -> Result<()> {
    blocking(&ctx, "bank.withdraw", |s| s.withdraw(&item.0))
}

fn deposit(ctx: Ctx<'_>, item: Ref) -> Result<()> {
    blocking(&ctx, "bank.deposit", |s| s.deposit(&item.0))
}

fn equip(ctx: Ctx<'_>, item: Ref) -> Result<()> {
    blocking(&ctx, "inventory.equip", |s| {
        withdraw_if_banked(s, &item.0)?;
        s.equip(&item.0)
    })
}

fn pickup(ctx: Ctx<'_>, item: Ref) -> Result<()> {
    blocking(&ctx, "drops.pickup", |s| {
        let Some(name) = s.world.drops.iter().find(|d| item.0.matches(d)).map(|d| d.name.clone()) else {
            return Err(ActError::NotFound(format!("a drop of {}", item.0)));
        };
        s.pickup(&name)
    })
}

fn accept(ctx: Ctx<'_>, id: u32) -> Result<()> {
    blocking(&ctx, "quests.accept", |s| s.accept(QuestId(id)))
}

fn complete(ctx: Ctx<'_>, id: u32, reward: Option<u32>, times: Option<u32>) -> Result<()> {
    blocking(&ctx, "quests.complete", |s| s.complete(QuestId(id), reward.map(ItemId), times.unwrap_or(1)))
}

fn buy(ctx: Ctx<'_>, shop: u32, item: Ref, qty: Option<u32>) -> Result<()> {
    blocking(&ctx, "shop.buy", |s| s.buy(ShopId(shop), &item.0, None, qty.unwrap_or(1)))
}

fn sell(ctx: Ctx<'_>, item: Ref, qty: Option<u32>) -> Result<()> {
    blocking(&ctx, "shop.sell", |s| s.sell(&item.0, qty.unwrap_or(1)))
}

fn wait_until<'js>(ctx: Ctx<'js>, predicate: Function<'js>, timeout: Option<f64>) -> Result<bool> {
    let deadline = ms(timeout).map(|t| Instant::now() + t);
    let mut ready = bridge::predicate(&ctx, predicate);
    blocking(&ctx, "wait.until", |s| {
        loop {
            let tick = Instant::now() + TICK;
            match s.pump_until(Some(deadline.map_or(tick, |d| d.min(tick))), |s| ready(&s.world).then_some(())) {
                Ok(()) => return Ok(true),
                Err(Halt::Timeout) if deadline.is_some_and(|d| Instant::now() >= d) => return Ok(false),
                Err(Halt::Timeout) => {}
                Err(halt) => return Err(halt.into()),
            }
        }
    })
}

fn sleep(ctx: Ctx<'_>, ms: f64) -> Result<()> {
    let until = Instant::now() + self::ms(Some(ms)).unwrap_or_default();
    blocking(&ctx, "wait.ms", |s| match s.pump_until(Some(until), |_| None::<()>) {
        Ok(()) | Err(Halt::Timeout) => Ok(()),
        Err(halt) => Err(halt.into()),
    })
}

fn status(text: String) {
    bridge::emit(Event::Status { text });
}

fn progress(label: String, current: f64, total: Option<f64>) {
    bridge::emit(Event::Progress { label, current: current.max(0.0) as u32, total: total.unwrap_or(0.0).max(0.0) as u32 });
}

fn stop(ctx: Ctx<'_>, reason: Option<String>) -> Result<()> {
    Err(bridge::stop(&ctx, reason))
}

fn args(ctx: Ctx<'_>) -> Result<Object<'_>> {
    let out = Object::new(ctx)?;
    for (key, value) in bridge::args() {
        out.set(key, value)?;
    }
    Ok(out)
}

fn events_queued(ctx: Ctx<'_>) -> Result<f64> {
    with_session(&ctx, "events.once", |s| s.events_queued() as f64)
}

fn respawn(ctx: Ctx<'_>, cell: Option<String>, pad: Option<String>) -> Result<()> {
    let to = cell.map(|cell| (cell, pad.unwrap_or_else(|| "Left".into())));
    with_session(&ctx, "settings.respawn", |s| s.respawn_to = to)
}

fn room() -> Option<u32> {
    bridge::room()
}

fn listen(name: String, on: bool) {
    bridge::listen(name, on);
}

fn listen_packet(ctx: Ctx<'_>, cmd: String, on: bool) -> Result<()> {
    with_session(&ctx, "packet.on", |s| s.listen(&cmd, on))
}

pub(crate) fn alarm(ctx: Ctx<'_>, delay: f64) -> Result<()> {
    let at = (delay >= 0.0).then(|| Instant::now() + ms(Some(delay)).unwrap_or_default());
    with_session(&ctx, "setTimeout", |s| s.set_alarm(at))
}

fn aggro<'js>(ctx: Ctx<'js>, monsters: Value<'js>) -> Result<()> {
    let target = if monsters.is_null() || monsters.is_undefined() { None } else { Some(lasting(&ctx, monsters)?) };
    with_session(&ctx, "combat.aggro", |s| s.set_aggro(target))
}

fn exit(ctx: Ctx<'_>) -> Result<()> {
    blocking(&ctx, "combat.exit", Session::exit_combat)
}

fn use_skill<'js>(ctx: Ctx<'js>, slot: u32, monsters: Value<'js>) -> Result<()> {
    if slot > 5 {
        return Err(Exception::throw_type(&ctx, &format!("there is no skill slot {slot}; slots are 0 to 5")));
    }
    let target = if monsters.is_null() || monsters.is_undefined() { None } else { Some(target(&ctx, monsters)?) };
    blocking(&ctx, "skills.use", |s| s.use_skill(slot as usize, target.as_deref()))
}

fn use_item(ctx: Ctx<'_>, item: Ref) -> Result<()> {
    blocking(&ctx, "inventory.use", |s| s.use_item(&item.0))
}

fn walk(ctx: Ctx<'_>, x: f64, y: f64) -> Result<()> {
    blocking(&ctx, "map.walk", |s| s.walk(x.round() as i32, y.round() as i32))
}

fn goto(ctx: Ctx<'_>, name: String) -> Result<()> {
    blocking(&ctx, "map.goto", |s| s.goto(&name))
}

fn collect(ctx: Ctx<'_>, id: u32, item: Ref, qty: Option<u32>) -> Result<()> {
    blocking(&ctx, "map.collect", |s| s.collect(id, &item.0, qty.unwrap_or(1).max(1)))
}

fn send_packet(ctx: Ctx<'_>, text: String) -> Result<()> {
    blocking(&ctx, "packet.send", |s| s.send_raw(&text))
}

fn server(ctx: Ctx<'_>) -> Result<String> {
    with_session(&ctx, "session.server", |s| s.server().map(|s| s.name.clone()).unwrap_or_default())
}

fn servers(ctx: Ctx<'_>) -> Result<Value<'_>> {
    let list = blocking(&ctx, "session.servers", Session::servers)?;
    js(&ctx, &list.iter().map(views::server).collect())
}

fn relogin(ctx: Ctx<'_>, name: Option<String>) -> Result<()> {
    blocking(&ctx, "session.relogin", |s| s.relogin(name.as_deref()))
}
