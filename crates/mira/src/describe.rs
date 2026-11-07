use std::time::Duration;

use mira_client::{Direction, Event, Load, NoticeKind, Phase, Place};

pub fn describe(event: &Event) -> Option<String> {
    Some(match event {
        Event::Phase(phase) => match phase {
            Phase::Connecting { server, version } => {
                format!("connecting to {server} as client {version}")
            }
            Phase::Handshaking => "handshaking".into(),
            Phase::LoggingIn => "logging in".into(),
            Phase::Loading {
                step: Load::Joining,
            } => "joining the first map".into(),
            Phase::Loading {
                step: Load::Character,
            } => "loading the character".into(),
            Phase::Loading {
                step: Load::Inventory,
            } => "loading the inventory".into(),
            Phase::Ready => return None,
            Phase::Reconnecting {
                reason,
                attempt,
                wait_ms,
            } => format!(
                "disconnected: {reason}; reconnecting in {:.0} s (attempt {attempt})",
                Duration::from_millis(*wait_ms).as_secs_f64()
            ),
            Phase::Closed { reason } => format!("disconnected: {reason}"),
        },
        Event::Joined {
            map,
            cell,
            pad,
            players,
        } => {
            format!("joined {map} at {cell}/{pad}, {players} other players")
        }
        Event::Moved { cell, pad } => format!("moved to {cell}/{pad}"),
        Event::PlayerEntered { name } => format!("{name} entered"),
        Event::PlayerLeft { name } => format!("{name} left"),
        Event::PlayerJumped { .. } | Event::PlayerWalked { .. } => return None,
        Event::Zone { zone: Some(zone) } => format!("the boss marked floor zone {zone}"),
        Event::Zone { zone: None } => "the boss cleared the floor zone".into(),
        Event::Ready { level, gold, items } => {
            format!("ready: level {level}, {gold} gold, {items} items")
        }
        Event::Chat {
            channel,
            sender,
            text,
        } => format!("[{channel}] {sender}: {text}"),
        Event::Whisper { sender, text } => format!("[whisper] {sender}: {text}"),
        Event::Notice { kind, text } => {
            let kind = match kind {
                NoticeKind::Server => "server",
                NoticeKind::Notify => "notice",
                NoticeKind::Warning => "warning",
            };
            format!("{kind}: {text}")
        }
        Event::Killed {
            name, gold, exp, ..
        } => format!("killed {name}, +{gold} gold, +{exp} xp"),
        Event::Died => "died".into(),
        Event::Respawned => "respawned".into(),
        Event::LeveledUp { level } => format!("reached level {level}"),
        Event::DropOffered { name, qty, .. } => format!("drop offered: {name} x{qty}"),
        Event::ItemChanged {
            name, qty, place, ..
        } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp",
                Place::Bank => "the bank",
            };
            format!("{name}: {qty} in {place}")
        }
        Event::QuestAccepted { id } => format!("accepted quest {id}"),
        Event::QuestCompleted { id } => format!("completed quest {id}"),
        Event::CounterStarted { monster } => format!("monster #{monster} is countering"),
        Event::CounterEnded { monster } => format!("monster #{monster} stopped countering"),
        Event::BankLoaded
        | Event::ItemPickedUp { .. }
        | Event::ItemReceived { .. }
        | Event::ItemBought { .. }
        | Event::ItemSold { .. }
        | Event::AuraAdded { .. }
        | Event::AuraRemoved { .. }
        | Event::AwayChanged { .. } => {
            return None;
        }
        Event::Unreadable { error } => format!("unreadable message: {error}"),
        Event::Packet { direction, text } => {
            let arrow = match direction {
                Direction::In => "<",
                Direction::Out => ">",
            };
            format!("{arrow} {text}")
        }
    })
}
