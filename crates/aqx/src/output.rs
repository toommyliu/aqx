use std::time::{SystemTime, UNIX_EPOCH};

use aqx_core::{Event, Place};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn time_of_day(ms: u64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut local) }.is_null() {
        return clock(ms % 86_400_000);
    }
    format!("{:02}:{:02}:{:02}.{:03}", local.tm_hour, local.tm_min, local.tm_sec, ms % 1000)
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
                Place::House => "house",
                Place::Temp => "temp inventory",
                Place::Bank => "bank",
            };
            ("game", format!("{item}: {qty} in the {place}"))
        }
        Event::RepGained { faction, rep } => ("game", format!("{faction}: +{rep} reputation")),
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
