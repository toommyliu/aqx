use std::fmt::Display;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use aqx_core::{Event, Place, World};
use serde_json::{Value, json};

pub enum Mode {
    Plain,
    Json,
    Tui(SyncSender<Update>),
}

pub enum Update {
    Say { t: u64, source: &'static str, text: String },
    Event { t: u64, event: Event },
}

static START: OnceLock<Instant> = OnceLock::new();
static MODE: OnceLock<Mode> = OnceLock::new();
static DROPPED: AtomicUsize = AtomicUsize::new(0);

pub fn init(mode: Mode) {
    START.get_or_init(Instant::now);
    let _ = MODE.set(mode);
}

pub fn now() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn dropped() -> usize {
    DROPPED.load(Ordering::Relaxed)
}

fn send(tx: &SyncSender<Update>, update: Update) {
    if tx.try_send(update).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn say(source: &'static str, text: impl Display) {
    match MODE.get() {
        Some(Mode::Tui(tx)) => send(tx, Update::Say { t: now(), source, text: text.to_string() }),
        Some(Mode::Json) => json_line(json!({ "event": "log", "source": source, "text": text.to_string() })),
        _ => plain_line(source, &text),
    }
}

pub fn event(event: &Event) {
    match MODE.get() {
        Some(Mode::Tui(tx)) => send(tx, Update::Event { t: now(), event: event.clone() }),
        Some(Mode::Json) => json_line(serde_json::to_value(event).unwrap_or(Value::Null)),
        _ => {
            if let Some((source, text)) = describe(event) {
                plain_line(source, &text);
            }
        }
    }
}

pub fn status(world: &World) {
    let Some(me) = &world.me else { return };
    let area = world.area.as_ref().map_or("", |a| a.name.as_str());
    let players = world.area.as_ref().map_or(0, |a| a.players.len()) + 1;
    let v = me.vitals;
    match MODE.get() {
        Some(Mode::Json) => json_line(json!({
            "event": "status", "area": area, "cell": me.cell, "pad": me.pad,
            "hp": v.hp, "max_hp": v.max_hp, "mp": v.mp, "max_mp": v.max_mp,
            "items": world.inventory.len(), "players": players,
        })),
        Some(Mode::Tui(_)) => {}
        _ => plain_line(
            "aqx",
            &format_args!(
                "{area} {}/{}, hp {}/{}, mp {}/{}, {} items, {players} players",
                me.cell,
                me.pad,
                v.hp,
                v.max_hp,
                v.mp,
                v.max_mp,
                world.inventory.len()
            ),
        ),
    }
}

pub fn describe(event: &Event) -> Option<(&'static str, String)> {
    Some(match event {
        Event::LoggedIn { eid } => ("aqx", format!("logged in, entity {eid}")),
        Event::Joined { area, players: 1 } => ("aqx", format!("joined {area}, alone")),
        Event::Joined { area, players } => ("aqx", format!("joined {area}, {players} players")),
        Event::Ready { items } => ("aqx", format!("ready, {items} items")),
        Event::Died => ("aqx", "died".into()),
        Event::Respawned => ("aqx", "respawned".into()),
        Event::Killed { monster } => ("aqx", format!("killed {monster}")),
        Event::DropOffered { item, qty } => ("aqx", format!("drop offered: {item} x{qty}")),
        Event::ItemChanged { item, qty, place } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp inventory",
                Place::Bank => "bank",
            };
            ("aqx", format!("{item}: {qty} in the {place}"))
        }
        Event::Chat { channel, sender, text } => ("server", format!("[{channel}] {sender}: {text}")),
        Event::Progress { label, current, total } => ("aqx", format!("{label} {current}/{total}")),
        Event::QuestAccepted { name, .. } => ("aqx", format!("accepted {name}")),
        Event::QuestCompleted { name, .. } => ("aqx", format!("completed {name}")),
        Event::QuestRejected { id, reason } => ("aqx", format!("quest {id} refused: {reason}")),
        Event::ServerText { text } | Event::Warning { text } => ("server", text.clone()),
        Event::Unreadable { what } => ("aqx", format!("unreadable message: {what}")),
        Event::RecordingFailed { error } => ("aqx", format!("recording stopped: {error}")),
        Event::LoginRejected { .. } | Event::LoggedInElsewhere | Event::InventoryLoaded { .. } => return None,
    })
}

fn plain_line(source: &str, text: &dyn Display) {
    let _ = writeln!(std::io::stdout().lock(), "{}  {source:<6}  {text}", clock(now()));
}

fn json_line(mut value: Value) {
    if let Value::Object(fields) = &mut value {
        fields.insert("t".into(), now().into());
    }
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

pub fn clock(ms: u64) -> String {
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h}:{m:02}:{s:02}.{ms:03}")
}
