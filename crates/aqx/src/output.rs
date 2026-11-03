use std::sync::OnceLock;
use std::time::Instant;

use aqx_core::{Event, Place};

static START: OnceLock<Instant> = OnceLock::new();

pub fn start() {
    START.get_or_init(Instant::now);
}

pub fn now() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn describe(event: &Event) -> Option<(&'static str, String)> {
    Some(match event {
        Event::Connecting { server, version } => ("aqx", format!("connecting to {server}, client version {version}")),
        Event::LoggedIn { eid } => ("aqx", format!("logged in, entity {eid}")),
        Event::Joined { area, players: 1 } => ("aqx", format!("joined {area}, alone")),
        Event::Joined { area, players } => ("aqx", format!("joined {area}, {players} players")),
        Event::Ready { items } => ("aqx", format!("ready, {items} items")),
        Event::BankLoaded { items } => ("aqx", format!("bank loaded, {items} items")),
        Event::Died => ("aqx", "died".into()),
        Event::Respawned => ("aqx", "respawned".into()),
        Event::Moved { cell, pad } => ("game", format!("moved to {cell}/{pad}")),
        Event::Afk { afk: true } => ("aqx", "marked away, answering".into()),
        Event::Killed { monster, .. } => ("game", format!("killed {monster}")),
        Event::DropOffered { item, qty, .. } => ("game", format!("drop offered: {item} x{qty}")),
        Event::ItemChanged { item, qty, place } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp inventory",
                Place::Bank => "bank",
            };
            ("game", format!("{item}: {qty} in the {place}"))
        }
        Event::Chat { channel, sender, text } => ("server", format!("[{channel}] {sender}: {text}")),
        Event::Progress { label, current, total } => ("script", format!("{label} {current}/{total}")),
        Event::Status { text } => ("script", text.clone()),
        Event::ScriptLog { level, text } if level == "log" || level == "info" => ("script", text.clone()),
        Event::ScriptLog { level, text } => ("script", format!("{level}: {text}")),
        Event::QuestAccepted { name, .. } => ("game", format!("accepted {name}")),
        Event::QuestCompleted { name, .. } => ("game", format!("completed {name}")),
        Event::QuestRejected { id, reason } => ("aqx", format!("quest {id} refused: {reason}")),
        Event::Refused { action, reason } => ("aqx", format!("{action} refused: {reason}")),
        Event::ServerText { text } | Event::Warning { text } => ("server", text.clone()),
        Event::Unreadable { what } => ("aqx", format!("unreadable message: {what}")),
        Event::RecordingFailed { error } => ("aqx", format!("recording stopped: {error}")),
        Event::LoginRejected { .. }
        | Event::PlayerEntered { .. }
        | Event::LoggedInElsewhere
        | Event::InventoryLoaded { .. }
        | Event::Afk { afk: false }
        | Event::AuraChanged { .. }
        | Event::Countering { .. }
        | Event::Packet { .. }
        | Event::Alarm => return None,
    })
}

pub fn clock(ms: u64) -> String {
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h}:{m:02}:{s:02}.{ms:03}")
}

pub fn span(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}m", secs / 3600, secs / 60 % 60),
    }
}
