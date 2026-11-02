use std::fmt::Display;
use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

use aqx_core::{Event, Place, World};
use serde_json::{Value, json};

static START: OnceLock<Instant> = OnceLock::new();
static JSON: OnceLock<bool> = OnceLock::new();

pub fn init(json: bool) {
    START.get_or_init(Instant::now);
    JSON.get_or_init(|| json);
}

pub fn say(source: &str, text: impl Display) {
    write(json!({ "event": "log", "source": source, "text": text.to_string() }), source, &text);
}

pub fn event(event: &Event) {
    let line = match event {
        Event::LoggedIn { eid } => Some(("aqx", format!("logged in, entity {eid}"))),
        Event::Joined { area, players: 1 } => Some(("aqx", format!("joined {area}, alone"))),
        Event::Joined { area, players } => Some(("aqx", format!("joined {area}, {players} players"))),
        Event::Ready { items } => Some(("aqx", format!("ready, {items} items. Ctrl-C logs out"))),
        Event::Died => Some(("aqx", "died".into())),
        Event::Respawned => Some(("aqx", "respawned".into())),
        Event::Killed { monster } => Some(("aqx", format!("killed {monster}"))),
        Event::DropOffered { item, qty } => Some(("aqx", format!("drop offered: {item} x{qty}"))),
        Event::ItemChanged { item, qty, place } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp inventory",
                Place::Bank => "bank",
            };
            Some(("aqx", format!("{item}: {qty} in the {place}")))
        }
        Event::Chat { channel, sender, text } => Some(("server", format!("[{channel}] {sender}: {text}"))),
        Event::ServerText { text } | Event::Warning { text } => Some(("server", text.clone())),
        Event::Unreadable { what } => Some(("aqx", format!("unreadable message: {what}"))),
        Event::RecordingFailed { error } => Some(("aqx", format!("recording stopped: {error}"))),
        Event::LoginRejected { .. } | Event::LoggedInElsewhere | Event::InventoryLoaded { .. } => None,
    };
    if *JSON.get().unwrap_or(&false) {
        write(serde_json::to_value(event).unwrap_or(Value::Null), "", &"");
    } else if let Some((source, text)) = line {
        write(Value::Null, source, &text);
    }
}

pub fn status(world: &World) {
    let Some(me) = &world.me else { return };
    let area = world.area.as_ref().map_or("", |a| a.name.as_str());
    let players = world.area.as_ref().map_or(0, |a| a.players.len()) + 1;
    let v = me.vitals;
    let text = format!(
        "{area} {}/{}, hp {}/{}, mp {}/{}, {} items, {players} players",
        me.cell,
        me.pad,
        v.hp,
        v.max_hp,
        v.mp,
        v.max_mp,
        world.inventory.len()
    );
    let structured = json!({
        "event": "status", "area": area, "cell": me.cell, "pad": me.pad,
        "hp": v.hp, "max_hp": v.max_hp, "mp": v.mp, "max_mp": v.max_mp,
        "items": world.inventory.len(), "players": players,
    });
    write(structured, "aqx", &text);
}

fn write(structured: Value, source: &str, text: &dyn Display) {
    let ms = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let mut out = std::io::stdout().lock();
    let _ = if *JSON.get().unwrap_or(&false) {
        let mut structured = structured;
        if let Value::Object(fields) = &mut structured {
            fields.insert("t".into(), ms.into());
        }
        writeln!(out, "{structured}")
    } else {
        writeln!(out, "{}  {source:<6}  {text}", clock(ms))
    };
}

pub fn clock(ms: u64) -> String {
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h}:{m:02}:{s:02}.{ms:03}")
}
