use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use mira_client::{Auras, BoostKind, Readiness, World};
use serde_json::{Value, json};

use super::protocol::{Message, Section, SectionName as S};

pub struct Projection {
    instant: Instant,
    unix: u64,
    previous: Option<Arc<World>>,
    sent: HashMap<S, String>,
}

impl Projection {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            previous: None,
            sent: HashMap::new(),
        }
    }
    pub fn reset(&mut self) {
        self.previous = None;
        self.sent.clear();
    }
    fn millis(&self, at: Instant) -> u64 {
        if at >= self.instant {
            self.unix
                .saturating_add(at.duration_since(self.instant).as_millis() as u64)
        } else {
            self.unix
                .saturating_sub(self.instant.duration_since(at).as_millis() as u64)
        }
    }
    pub fn instant_at(&self, millis: i64) -> Option<Instant> {
        let millis = u64::try_from(millis).ok().filter(|n| *n > 0)?;
        if millis >= self.unix {
            self.instant
                .checked_add(std::time::Duration::from_millis(millis - self.unix))
        } else {
            self.instant
                .checked_sub(std::time::Duration::from_millis(self.unix - millis))
        }
    }

    fn auras(&self, auras: &Auras) -> Value {
        let list = |auras: &[mira_client::Aura]| {
            auras
                .iter()
                .map(|aura| {
                    let mut value = serde_json::to_value(aura).expect("auras serialize");
                    value["expires"] = json!(aura.expires.map(|at| self.millis(at)));
                    value["applied"] = json!(self.millis(aura.applied));
                    value
                })
                .collect::<Vec<_>>()
        };
        let hud: Vec<Value> = auras
            .hud
            .iter()
            .map(|hud| {
                let mut value = serde_json::to_value(hud).expect("auras serialize");
                value["expires"] = json!(hud.expires.map(|at| self.millis(at)));
                value
            })
            .collect();
        json!({"active": list(&auras.active), "hud": hud})
    }
    pub fn changed(&mut self, world: Arc<World>, server: &str, host: &str) -> Vec<Message> {
        let me = world.me.as_ref().map(|me| {
            let mut value = serde_json::to_value(me).expect("players serialize");
            value["class_rank"] = json!(me.class_rank());
            value["boosts"] = [
                (BoostKind::Xp, "xp"),
                (BoostKind::Gold, "gold"),
                (BoostKind::Rep, "rep"),
                (BoostKind::ClassPoints, "class_points"),
            ]
            .into_iter()
            .filter_map(|(kind, name)| {
                let remaining = me.boosts.remaining(kind, self.instant);
                (!remaining.is_zero())
                    .then(|| (name.into(), json!(self.millis(self.instant + remaining))))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
            value["auras"] = self.auras(&me.auras);
            value
        });
        let room = world.room.as_ref().map(|room| {
            let mut value = serde_json::to_value(room).expect("rooms serialize");
            value["room_id"] = json!(room.area);
            for (monster, projected) in room
                .monsters
                .iter()
                .zip(value["monsters"].as_array_mut().unwrap())
            {
                projected["auras"] = self.auras(&monster.auras);
            }
            for (player, projected) in room
                .players
                .iter()
                .zip(value["players"].as_array_mut().unwrap())
            {
                projected["auras"] = self.auras(&player.auras);
            }
            if let Some(me) = &world.me {
                value["players"].as_array_mut().unwrap().push(json!({
                    "eid": me.id, "name": me.name, "cell": room.cell, "pad": room.pad,
                    "x": room.x, "y": room.y, "level": me.level, "class_name": me.class_name,
                    "hp": me.hp, "max_hp": me.max_hp, "mp": me.mp, "max_mp": me.max_mp,
                    "state": me.state, "away": me.away,
                }));
            }
            value
        });
        let mut values = vec![
            (
                S::Session,
                json!({"phase": world.phase, "server": server, "host": host}),
            ),
            (S::Me, json!(me)),
            (S::Room, json!(room)),
            (S::Temp, json!(world.temp)),
            (S::Drops, json!(world.drops)),
            (S::Factions, json!(world.factions)),
        ];
        let old = self.previous.as_ref();
        if old.is_none_or(|old| !Arc::ptr_eq(&old.inventory, &world.inventory)) {
            values.push((S::Inventory, json!(world.inventory)));
        }
        if old.is_none_or(|old| match (&old.bank, &world.bank) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        }) {
            values.push((S::Bank, json!(world.bank)));
        }
        if old.is_none_or(|old| match (&old.shop, &world.shop) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        }) {
            values.push((S::Shop, json!(world.shop)));
        }
        let statuses: std::collections::BTreeMap<_, _> = world
            .quests
            .keys()
            .map(|id| {
                let status = if world.accepted_quests.contains(id) {
                    json!(if world.holds_turn_in(*id) == Some(true) {
                        "completable"
                    } else {
                        "active"
                    })
                } else {
                    json!(world.quest_status(*id))
                };
                (id.to_string(), status)
            })
            .collect();
        values.push((
            S::Quests,
            json!({
                "definitions": world.quests, "accepted": world.accepted_quests,
                "progress": world.me.as_ref().map(|me| &me.quests), "status": statuses,
            }),
        ));
        let readiness: serde_json::Map<_, _> = world
            .readiness
            .iter()
            .map(|(slot, readiness)| {
                let value = match readiness {
                    Readiness::At(at) => json!({"at": self.millis(*at)}),
                    Readiness::Ready => json!("ready"),
                    Readiness::Casting => json!("casting"),
                    Readiness::Disabled => json!("disabled"),
                    Readiness::Locked => json!("locked"),
                    Readiness::NoMana => json!("no_mana"),
                };
                (slot.clone(), value)
            })
            .collect();
        values.push((
            S::Combat,
            json!({
                "target": world.target, "readiness": readiness,
                "global_cooldown": world.global_cooldown.map(|at| self.millis(at)),
                "skill_timing": world.skill_timing.iter().map(|(slot, timing)| (slot.clone(), json!({
                    "last_use": timing.last_use.map(|at| self.millis(at)),
                    "cooldown": timing.cooldown.map(|at| self.millis(at)),
                }))).collect::<serde_json::Map<_, _>>(),
                "action_locks": world.action_locks.iter().map(|(name, lock)| ((*name).to_owned(), json!({
                    "used": lock.used.map(|at| self.millis(at)),
                    "cooldown": lock.cooldown.as_millis() as u64,
                }))).collect::<serde_json::Map<_, _>>(),
            }),
        ));
        let hold = match world.settings.counter_hold {
            mira_client::CounterHold::Off => "off",
            mira_client::CounterHold::All => "all",
            mira_client::CounterHold::Attacks => "attacks",
        };
        let aggro = match &world.settings.aggro {
            None => json!("off"),
            Some(mira_client::Aggro::All) => json!("all"),
            Some(mira_client::Aggro::Cell) => json!("cell"),
            Some(mira_client::Aggro::Only(ids)) => {
                let mut ids: Vec<_> = ids.iter().collect();
                ids.sort_by_key(|id| id.0);
                json!({"only": ids})
            }
        };
        values.push((
            S::Settings,
            json!({
                "counter_hold": hold, "aggro": aggro,
                "anti_afk": world.settings.anti_afk, "room": world.settings.room,
            }),
        ));
        let mut sections = Vec::new();
        for (section, value) in values {
            let text = value.to_string();
            if self.sent.get(&section) != Some(&text) {
                self.sent.insert(section, text);
                sections.push(Message::State {
                    state: Section { section, value },
                });
            }
        }
        self.previous = Some(world);
        sections
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use mira_client::{
        Options,
        testing::{Harness, Peer},
    };
    #[test]
    fn unchanged_deadlines_do_not_push_sections_again() {
        let harness = Harness::start(Options::default(), |listener| {
            let mut peer = Peer::accept(&listener);
            peer.serve_to_ready();
            peer.expect_logout();
        });
        harness.session.ready().wait().unwrap();
        let mut world = (*harness.session.snapshot()).clone();
        world
            .me
            .as_mut()
            .unwrap()
            .auras
            .active
            .push(mira_client::Aura {
                name: "Example".into(),
                icon: String::new(),
                description: String::new(),
                duration: 5.0,
                stacks: 1,
                value: None,
                category: None,
                persist: false,
                caster: None,
                effects: vec![],
                expires: Some(Instant::now() + std::time::Duration::from_secs(5)),
                applied: Instant::now(),
            });
        world.readiness.insert(
            "a1".into(),
            Readiness::At(Instant::now() + std::time::Duration::from_secs(1)),
        );
        let mut projection = Projection::new();
        assert_eq!(
            projection
                .changed(Arc::new(world.clone()), "Test", "localhost")
                .len(),
            12
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(
            projection
                .changed(Arc::new(world), "Test", "localhost")
                .is_empty()
        );
        harness.session.logout();
        harness.finish();
    }
}
