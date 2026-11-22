use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use aqx_protocol::ItemId;
use serde::{Deserialize, Serialize};

use crate::REPLY_TIMEOUT;
use crate::combat::ITEM_SLOT;
use crate::inventory::Item;
use crate::room::CombatState;
use crate::session::{ActionError, Handle, Pending};

const STOP_CHECK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gear {
    pub class: Option<String>,
    pub weapon: Option<String>,
    pub helm: Option<String>,
    pub armor: Option<String>,
    pub cape: Option<String>,
    pub pet: Option<String>,
    pub consumable: Option<String>,
    #[serde(rename = "use")]
    pub use_items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Missing {
    pub item: String,
    pub why: String,
}

impl Handle {
    pub fn equip_gear(
        &self,
        gear: &Gear,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Vec<Missing>, ActionError> {
        let mut wearing = Wearing {
            session: self,
            stopped,
            missing: Vec::new(),
            bank_failed: None,
        };
        let fighting = self
            .snapshot()
            .me
            .as_ref()
            .is_some_and(|me| me.state == CombatState::InCombat);
        if fighting {
            wearing.run(Handle::exit_combat)?;
        }
        let worn = [
            &gear.class,
            &gear.weapon,
            &gear.helm,
            &gear.armor,
            &gear.cape,
            &gear.pet,
        ];
        for name in worn.into_iter().flatten() {
            wearing.equip(name)?;
        }
        for name in &gear.use_items {
            wearing.consume(name)?;
        }
        if let Some(name) = &gear.consumable {
            wearing.slot(name)?;
        }
        Ok(wearing.missing)
    }
}

struct Wearing<'a> {
    session: &'a Handle,
    stopped: &'a dyn Fn() -> bool,
    missing: Vec<Missing>,
    bank_failed: Option<ActionError>,
}

impl Wearing<'_> {
    fn equip(&mut self, name: &str) -> Result<(), ActionError> {
        let Some(item) = self.held(name)? else {
            return Ok(());
        };
        let equipped = self.run(|s| s.equip(item.id));
        self.settle(name, equipped)
    }

    fn slot(&mut self, name: &str) -> Result<(), ActionError> {
        let Some(item) = self.held(name)? else {
            return Ok(());
        };
        if !item.kind.eq_ignore_ascii_case("Item") {
            self.missing.push(Missing {
                item: name.to_owned(),
                why: format!("{} can't go in the consumable slot", item.name),
            });
            return Ok(());
        }
        let slotted = self.run(|s| s.equip(item.id));
        self.settle(name, slotted)
    }

    fn consume(&mut self, name: &str) -> Result<(), ActionError> {
        let Some(item) = self.held(name)? else {
            return Ok(());
        };
        let server_use = item.kind.eq_ignore_ascii_case("ServerUse");
        if !server_use && !item.kind.eq_ignore_ascii_case("Item") {
            self.missing.push(Missing {
                item: name.to_owned(),
                why: format!("{} can't be used", item.name),
            });
            return Ok(());
        }
        let updates = self.session.updates();
        let used = if server_use {
            self.run(|s| s.use_item(item.id))
        } else {
            self.run(|s| s.equip(item.id))
                .and_then(|()| self.run(|s| s.use_skill(ITEM_SLOT, None)))
        };
        if used.is_err() {
            return self.settle(name, used);
        }
        let deadline = Instant::now() + REPLY_TIMEOUT;
        while held_qty(self.session, item.id) >= item.qty {
            if (self.stopped)() {
                return Err(ActionError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match updates.recv_timeout(left.min(STOP_CHECK)) {
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) if !left.is_zero() => {}
                Err(RecvTimeoutError::Timeout) => {
                    self.missing.push(Missing {
                        item: name.to_owned(),
                        why: "the server didn't use it".into(),
                    });
                    return Ok(());
                }
                Err(RecvTimeoutError::Disconnected) => return Err(ActionError::Stopped),
            }
        }
        Ok(())
    }

    fn run(&self, act: impl FnOnce(&Handle) -> Pending) -> Result<(), ActionError> {
        if (self.stopped)() {
            return Err(ActionError::Cancelled);
        }
        let pending = act(self.session);
        loop {
            if let Some(result) = pending.wait_for(STOP_CHECK) {
                return result;
            }
            if (self.stopped)() {
                self.session.cancel(pending.id());
                return Err(ActionError::Cancelled);
            }
        }
    }

    fn held(&mut self, name: &str) -> Result<Option<Item>, ActionError> {
        if (self.stopped)() {
            return Err(ActionError::Cancelled);
        }
        if let Some(item) = carried(self.session, name) {
            return Ok(Some(item));
        }
        if self.session.snapshot().bank.is_none() {
            let loaded = match self.bank_failed.clone() {
                Some(failed) => Err(failed),
                None => self.run(Handle::load_bank),
            };
            if let Err(e) = loaded {
                self.bank_failed = Some(e.clone());
                self.settle(name, Err(e))?;
                return Ok(None);
            }
        }
        let banked = self.session.snapshot().bank.as_ref().and_then(|bank| {
            bank.iter()
                .find(|i| i.name.eq_ignore_ascii_case(name))
                .map(|i| i.id)
        });
        let Some(id) = banked else {
            self.missing.push(Missing {
                item: name.to_owned(),
                why: "not in the inventory or bank".into(),
            });
            return Ok(None);
        };
        let withdrew = self.run(|s| s.withdraw(id));
        self.settle(name, withdrew)?;
        Ok(carried(self.session, name))
    }

    fn settle(&mut self, name: &str, result: Result<(), ActionError>) -> Result<(), ActionError> {
        match result {
            Ok(()) => Ok(()),
            Err(
                e @ (ActionError::NotReady
                | ActionError::Dead
                | ActionError::Closed(_)
                | ActionError::Stopped
                | ActionError::Cancelled),
            ) => Err(e),
            Err(e) => {
                self.missing.push(Missing {
                    item: name.to_owned(),
                    why: e.to_string(),
                });
                Ok(())
            }
        }
    }
}

fn held_qty(session: &Handle, id: ItemId) -> u32 {
    session
        .snapshot()
        .inventory
        .iter()
        .find(|i| i.id == id)
        .map_or(0, |i| i.qty)
}

fn carried(session: &Handle, name: &str) -> Option<Item> {
    session
        .snapshot()
        .inventory
        .iter()
        .find(|i| i.name.eq_ignore_ascii_case(name))
        .cloned()
}
