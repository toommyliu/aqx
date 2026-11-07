use mira_client::{Event, Holder, NoticeKind, Place, World};
use rquickjs::{Ctx, Object, Result};

use crate::views;

pub fn payload<'js>(
    ctx: &Ctx<'js>,
    world: &World,
    event: &Event,
) -> Result<Option<(&'static str, Object<'js>)>> {
    let view = Object::new(ctx.clone())?;
    let name = match event {
        Event::Joined { map, cell, pad, .. } => {
            view.set("map", map.as_str())?;
            view.set("cell", cell.as_str())?;
            view.set("pad", pad.as_str())?;
            "joined"
        }
        Event::Moved { cell, pad } => {
            view.set("cell", cell.as_str())?;
            view.set("pad", pad.as_str())?;
            "moved"
        }
        Event::PlayerEntered { name } => {
            view.set("name", name.as_str())?;
            "playerEntered"
        }
        Event::PlayerLeft { name } => {
            view.set("name", name.as_str())?;
            "playerLeft"
        }
        Event::PlayerJumped { name, cell, pad } => {
            view.set("kind", "jump")?;
            view.set("name", name.as_str())?;
            view.set("cell", cell.as_str())?;
            view.set("pad", pad.as_str())?;
            "playerMoved"
        }
        Event::PlayerWalked { name, cell, x, y } => {
            view.set("kind", "walk")?;
            view.set("name", name.as_str())?;
            view.set("cell", cell.as_str())?;
            view.set("x", *x)?;
            view.set("y", *y)?;
            "playerMoved"
        }
        Event::Chat {
            channel,
            sender,
            text,
        } => {
            view.set("channel", channel.as_str())?;
            view.set("sender", sender.as_str())?;
            view.set("text", text.as_str())?;
            "chat"
        }
        Event::Whisper { sender, text } => {
            view.set("sender", sender.as_str())?;
            view.set("text", text.as_str())?;
            "whisper"
        }
        Event::Notice { kind, text } => {
            let kind = match kind {
                NoticeKind::Server => "server",
                NoticeKind::Notify => "notify",
                NoticeKind::Warning => "warning",
            };
            view.set("kind", kind)?;
            view.set("text", text.as_str())?;
            "notice"
        }
        Event::Killed {
            id,
            name,
            gold,
            exp,
        } => {
            view.set("monMapId", id.0 as f64)?;
            view.set("name", name.as_str())?;
            view.set("gold", *gold as f64)?;
            view.set("exp", *exp as f64)?;
            "killed"
        }
        Event::Died => "died",
        Event::Respawned => "respawned",
        Event::LeveledUp { level } => {
            view.set("level", *level)?;
            "leveledUp"
        }
        Event::DropOffered { id, name, qty } => {
            let offer = world.drops.iter().find(|offer| offer.id == *id);
            let view = match offer {
                Some(offer) => views::drop(ctx, offer)?,
                None => {
                    view.set("id", id.0 as f64)?;
                    view.set("name", name.as_str())?;
                    view.set("qty", *qty)?;
                    view
                }
            };
            return Ok(Some(("dropOffered", view)));
        }
        Event::ItemChanged {
            id,
            name,
            qty,
            place,
        } => {
            view.set("id", id.0 as f64)?;
            view.set("name", name.as_str())?;
            view.set("qty", *qty)?;
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp",
                Place::Bank => "bank",
            };
            view.set("place", place)?;
            "itemChanged"
        }
        Event::QuestAccepted { id } => {
            view.set("id", id.0 as f64)?;
            "questAccepted"
        }
        Event::QuestCompleted { id } => {
            view.set("id", id.0 as f64)?;
            "questCompleted"
        }
        Event::AuraAdded { on, name, stacks } => {
            view.set("on", holder(ctx, *on)?)?;
            view.set("name", name.as_str())?;
            view.set("stacks", *stacks)?;
            "auraAdded"
        }
        Event::AuraRemoved { on, name } => {
            view.set("on", holder(ctx, *on)?)?;
            view.set("name", name.as_str())?;
            "auraRemoved"
        }
        Event::CounterStarted { monster } => {
            view.set("monMapId", monster.0 as f64)?;
            "counterStarted"
        }
        Event::CounterEnded { monster } => {
            view.set("monMapId", monster.0 as f64)?;
            "counterEnded"
        }
        Event::AwayChanged { away } => {
            view.set("away", *away)?;
            "awayChanged"
        }
        Event::Zone { zone } => {
            view.set("zone", views::nullable(ctx, zone.as_deref())?)?;
            "zone"
        }
        Event::BankLoaded
        | Event::ItemPickedUp { .. }
        | Event::ItemReceived { .. }
        | Event::ItemBought { .. }
        | Event::ItemSold { .. }
        | Event::Phase(_)
        | Event::Ready { .. }
        | Event::Unreadable { .. }
        | Event::Packet { .. } => {
            return Ok(None);
        }
    };
    Ok(Some((name, view)))
}

fn holder<'js>(ctx: &Ctx<'js>, holder: Holder) -> Result<Object<'js>> {
    let view = Object::new(ctx.clone())?;
    match holder {
        Holder::Me => view.set("kind", "player")?,
        Holder::Monster(id) => {
            view.set("kind", "monster")?;
            view.set("monMapId", id.0 as f64)?;
        }
    }
    Ok(view)
}
