use std::sync::Arc;
use std::task::Poll;
use std::thread;
use std::time::Instant;

use mira_protocol::{ItemId, flex, packet};
use serde::Deserialize;

use super::{ActionError, Op, Session};
use crate::REPLY_TIMEOUT;
use crate::api;
use crate::event::{Event, Place};
use crate::inventory::Pool;
use crate::room::CombatState;

#[derive(Deserialize)]
pub(super) struct Loaded {
    #[serde(rename = "bitSuccess", deserialize_with = "flex::bool")]
    pub ok: bool,
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
}

#[derive(Clone, Copy)]
pub(super) enum BankMove {
    Deposit(ItemId),
    Withdraw(ItemId),
    Swap { inventory: ItemId, bank: ItemId },
}

pub(super) enum BankStage {
    Idle,
    Fetching,
    Sent { deadline: Instant },
}

pub(super) enum Moved {
    Deposited { id: ItemId, ok: bool, msg: String },
    Withdrew(ItemId),
    Swapped { inventory: ItemId, bank: ItemId },
}

#[derive(Deserialize)]
pub(super) struct Deposited {
    #[serde(rename = "ItemID")]
    id: ItemId,
    #[serde(rename = "bSuccess", default, deserialize_with = "flex::bool")]
    ok: bool,
    #[serde(default)]
    msg: String,
}

#[derive(Deserialize)]
pub(super) struct Withdrew {
    #[serde(rename = "ItemID")]
    id: ItemId,
}

#[derive(Deserialize)]
pub(super) struct Swapped {
    #[serde(rename = "invItemID")]
    inventory: ItemId,
    #[serde(rename = "bankItemID")]
    bank: ItemId,
}

impl From<Deposited> for Moved {
    fn from(d: Deposited) -> Moved {
        Moved::Deposited {
            id: d.id,
            ok: d.ok,
            msg: d.msg,
        }
    }
}

impl From<Withdrew> for Moved {
    fn from(w: Withdrew) -> Moved {
        Moved::Withdrew(w.id)
    }
}

impl From<Swapped> for Moved {
    fn from(s: Swapped) -> Moved {
        Moved::Swapped {
            inventory: s.inventory,
            bank: s.bank,
        }
    }
}

impl Session {
    pub(super) fn step_bank(
        &mut self,
        mv: Option<BankMove>,
        stage: &mut BankStage,
        now: Instant,
    ) -> Poll<Result<(), ActionError>> {
        if let Err(e) = self.ensure_ready() {
            return Poll::Ready(Err(e));
        }
        match *stage {
            BankStage::Fetching => return Poll::Pending,
            BankStage::Sent { deadline } if now >= deadline => {
                return Poll::Ready(Err(ActionError::NoReply));
            }
            BankStage::Sent { .. } => return Poll::Pending,
            BankStage::Idle => {}
        }
        if self.bank.is_none() {
            let Some(me) = &self.character else {
                return Poll::Ready(Err(ActionError::NotReady));
            };
            let (account, char_id) = (self.account.clone(), me.char_id);
            let (results, waker) = (self.bank_out.clone(), Arc::clone(&self.waker));
            let epoch = self.epoch.load(std::sync::atomic::Ordering::SeqCst);
            thread::spawn(move || {
                let _ = results.send((epoch, api::bank(&account, char_id)));
                let _ = waker.wake();
            });
            *stage = BankStage::Fetching;
            return Poll::Pending;
        }
        let Some(mv) = mv else {
            return Poll::Ready(Ok(()));
        };
        let packet = match self.prepare_move(mv) {
            Ok(packet) => packet,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.send(&packet);
        *stage = BankStage::Sent {
            deadline: now + REPLY_TIMEOUT,
        };
        Poll::Pending
    }

    fn prepare_move(&self, mv: BankMove) -> Result<String, ActionError> {
        let unusable = |why: String| Err(ActionError::Unusable(why));
        let (Some(me), Some(room), Some(bank)) = (&self.character, &self.room, &self.bank) else {
            return Err(ActionError::NotReady);
        };
        let carried = |id: ItemId| {
            self.inventory
                .items
                .iter()
                .find(|i| i.id == id)
                .ok_or_else(|| ActionError::NoTarget(format!("item {id} isn't in the inventory")))
        };
        let banked = |id: ItemId| {
            bank.iter()
                .find(|i| i.id == id)
                .ok_or_else(|| ActionError::NoTarget(format!("item {id} isn't in the bank")))
        };
        if me.state == CombatState::InCombat {
            return Err(ActionError::InCombat);
        }
        let bank_full = me.bank_count >= me.bank_slots;
        match mv {
            BankMove::Deposit(id) => {
                let item = carried(id)?;
                if item.pool() == Pool::Class {
                    return unusable("classes can't be banked".into());
                }
                if item.equipped {
                    return unusable(format!("unequip {} before banking it", item.name));
                }
                if !item.coins && bank_full {
                    return unusable("the bank has no room for items bought with gold".into());
                }
                Ok(packet::bank_from_inv(room.area, id, item.char_item_id))
            }
            BankMove::Withdraw(id) => {
                let item = banked(id)?;
                if self.inventory.full_for_item(item, me) {
                    return unusable(format!("the inventory has no room for {}", item.name));
                }
                Ok(packet::bank_to_inv(room.area, id, item.char_item_id))
            }
            BankMove::Swap { inventory, bank } => {
                let (carried, banked) = (carried(inventory)?, banked(bank)?);
                if carried.pool() == Pool::Class {
                    return unusable("classes can't be banked".into());
                }
                if carried.equipped {
                    return unusable(format!("unequip {} before banking it", carried.name));
                }
                if !carried.coins && banked.coins && bank_full {
                    return unusable("the bank has no room for items bought with gold".into());
                }
                let outgrows = |pool: Pool| {
                    banked.pool() == pool
                        && carried.pool() != pool
                        && self.inventory.full_for_item(banked, me)
                };
                if outgrows(Pool::Misc) || outgrows(Pool::Bag) {
                    return unusable(format!("the inventory has no room for {}", banked.name));
                }
                Ok(packet::bank_swap_inv(
                    room.area,
                    inventory,
                    carried.char_item_id,
                    bank,
                    banked.char_item_id,
                ))
            }
        }
    }

    pub(super) fn on_moved(&mut self, moved: Moved) {
        let result = match &moved {
            Moved::Deposited { id, ok: true, .. } => {
                self.deposit(*id);
                Ok(())
            }
            Moved::Deposited { msg, .. } => Err(ActionError::Refused(msg.clone())),
            Moved::Withdrew(id) => {
                self.withdraw(*id);
                Ok(())
            }
            Moved::Swapped { inventory, bank } => {
                self.deposit(*inventory);
                self.withdraw(*bank);
                Ok(())
            }
        };
        let answers = |mv: &BankMove| match (mv, &moved) {
            (BankMove::Deposit(id), Moved::Deposited { id: got, .. }) => id == got,
            (BankMove::Withdraw(id), Moved::Withdrew(got)) => id == got,
            (
                BankMove::Swap { inventory, bank },
                Moved::Swapped {
                    inventory: i,
                    bank: b,
                },
            ) => inventory == i && bank == b,
            _ => false,
        };
        let waiting = matches!(
            &self.current,
            Some(Op::Bank { mv: Some(mv), stage: BankStage::Sent { .. }, .. }) if answers(mv)
        );
        if waiting && let Some(Op::Bank { reply, .. }) = self.current.take() {
            self.replies.push((reply, result));
        }
    }

    fn deposit(&mut self, id: ItemId) {
        let items = Arc::make_mut(&mut self.inventory.items);
        let Some(at) = items.iter().position(|i| i.id == id) else {
            return;
        };
        let item = items.remove(at);
        self.stored(item.coins, 1);
        self.emit(Event::ItemChanged {
            id,
            name: item.name.clone(),
            qty: item.qty,
            place: Place::Bank,
        });
        if let Some(bank) = &mut self.bank {
            Arc::make_mut(bank).push(item);
        }
    }

    fn withdraw(&mut self, id: ItemId) {
        let Some(bank) = &mut self.bank else { return };
        let bank = Arc::make_mut(bank);
        let Some(at) = bank.iter().position(|i| i.id == id) else {
            return;
        };
        let item = bank.remove(at);
        self.stored(item.coins, -1);
        self.emit(Event::ItemChanged {
            id,
            name: item.name.clone(),
            qty: item.qty,
            place: Place::Inventory,
        });
        Arc::make_mut(&mut self.inventory.items).push(item);
    }

    pub(super) fn stored(&mut self, coins: bool, change: i32) {
        if let Some(me) = self.character.as_mut().filter(|_| !coins) {
            me.bank_count = me.bank_count.saturating_add_signed(change);
        }
    }
}
