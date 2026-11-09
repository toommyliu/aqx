use std::task::Poll;
use std::time::Instant;

use aqx_protocol::{ItemId, flex, packet};
use serde::Deserialize;

use super::{ActionError, Limit, Op, Session};
use crate::REPLY_TIMEOUT;
use crate::combat::ITEM_SLOT;
use crate::player::SlotSkill;
use crate::room::CombatState;

const ALWAYS_EQUIPPABLE: [&str; 4] = ["mi", "co", "pe", "am"];

pub(super) enum EquipStage {
    Waiting { wake: Option<Instant> },
    Sent { deadline: Instant, awaits: Ack },
}

pub(super) enum Ack {
    Worn,
    SlotFilled,
}

impl EquipStage {
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            EquipStage::Waiting { wake } => *wake,
            EquipStage::Sent { deadline, .. } => Some(*deadline),
        }
    }
}

enum Plan {
    Done,
    Gear,
    Consumable { link: String, meta: String },
    ClearSlot,
}

#[derive(Deserialize)]
pub(super) struct SlotFilled {
    #[serde(rename = "iRes", default, deserialize_with = "flex::int")]
    res: i64,
    #[serde(default)]
    o: Option<SlotSkill>,
}

impl Session {
    pub(super) fn step_equip(
        &mut self,
        item: ItemId,
        on: bool,
        checked: bool,
        stage: &mut EquipStage,
        now: Instant,
    ) -> Poll<Result<(), ActionError>> {
        if let Err(e) = self.ensure_ready() {
            return Poll::Ready(Err(e));
        }
        if let EquipStage::Sent { deadline, .. } = *stage {
            return if now >= deadline {
                Poll::Ready(Err(ActionError::NoReply))
            } else {
                Poll::Pending
            };
        }
        let plan = match self.plan_equip(item, on, checked) {
            Ok(plan) => plan,
            Err(e) => return Poll::Ready(Err(e)),
        };
        let Some(area) = self.room.as_ref().map(|room| room.area) else {
            return Poll::Ready(Err(ActionError::NotReady));
        };
        let (packet, awaits) = match plan {
            Plan::Done => return Poll::Ready(Ok(())),
            Plan::ClearSlot => {
                if let Some(me) = &mut self.character {
                    me.item_slot = None;
                }
                self.inventory.set_consumable(None);
                return Poll::Ready(Ok(()));
            }
            Plan::Consumable { link, meta } => {
                self.combat.lock_item_slot(now);
                (packet::geia(area, &link, &meta, item), Ack::SlotFilled)
            }
            Plan::Gear => {
                let limit = if on {
                    Limit::EquipItem
                } else {
                    Limit::UnequipItem
                };
                if let Some(until) = self.cooling(limit, now) {
                    *stage = EquipStage::Waiting { wake: Some(until) };
                    return Poll::Pending;
                }
                self.limits.used(limit, now);
                let packet = if on {
                    packet::equip_item(area, item)
                } else {
                    packet::unequip_item(area, item)
                };
                (packet, Ack::Worn)
            }
        };
        self.send(&packet);
        *stage = EquipStage::Sent {
            deadline: now + REPLY_TIMEOUT,
            awaits,
        };
        Poll::Pending
    }

    fn plan_equip(&self, id: ItemId, on: bool, checked: bool) -> Result<Plan, ActionError> {
        let unusable = |why: String| Err(ActionError::Unusable(why));
        let Some(me) = &self.character else {
            return Err(ActionError::NotReady);
        };
        let Some(item) = self.inventory.items.iter().find(|i| i.id == id) else {
            return Err(ActionError::NoTarget(format!(
                "item {id} isn't in the inventory"
            )));
        };
        match me.state {
            CombatState::Dead if checked => return Err(ActionError::Dead),
            CombatState::InCombat if checked => return Err(ActionError::InCombat),
            _ => {}
        }
        let consumable = item.kind.eq_ignore_ascii_case("item");
        let worn = if consumable {
            me.item_slot == Some(id)
        } else {
            item.equipped
        };
        if worn == on {
            return Ok(Plan::Done);
        }
        if !on {
            if consumable {
                return Ok(Plan::ClearSlot);
            }
            if item.slot == "Weapon" || item.slot == "ar" {
                return unusable(format!(
                    "{} can't be unequipped; equip another instead",
                    item.name
                ));
            }
            return Ok(Plan::Gear);
        }
        if checked && item.member && !me.member {
            return unusable(format!("{} is for members", item.name));
        }
        let level = item.enhancement.as_ref().map_or(0, |e| e.level);
        if checked && level > me.level {
            return unusable(format!("{} needs level {level}", item.name));
        }
        if consumable {
            if item.link.is_empty() || item.link.eq_ignore_ascii_case("none") {
                return unusable(format!("{} can't go in the item slot", item.name));
            }
            return Ok(Plan::Consumable {
                link: item.link.clone(),
                meta: item.meta.clone(),
            });
        }
        if checked && !ALWAYS_EQUIPPABLE.contains(&item.slot.as_str()) && item.enhancement.is_none()
        {
            return unusable(format!("{} needs an enhancement first", item.name));
        }
        Ok(Plan::Gear)
    }

    pub(super) fn on_worn(&mut self, item: ItemId, on: bool) {
        let answers = matches!(
            &self.current,
            Some(Op::Equip { item: wanted, on: want, stage: EquipStage::Sent { .. }, .. })
                if *wanted == item && *want == on
        );
        if answers && let Some(Op::Equip { reply, .. }) = self.current.take() {
            self.replies.push((reply, Ok(())));
        }
    }

    pub(super) fn on_slot_filled(&mut self, filled: SlotFilled, now: Instant) {
        self.combat.unlock_item_slot();
        let Some(Op::Equip {
            item,
            stage:
                EquipStage::Sent {
                    awaits: Ack::SlotFilled,
                    ..
                },
            ..
        }) = self.current
        else {
            return;
        };
        let result = match (filled.res, filled.o) {
            (1, Some(skill)) => {
                if let Some(me) = &mut self.character {
                    let cooling = me
                        .skills
                        .iter()
                        .find(|s| s.slot == ITEM_SLOT)
                        .is_some_and(|s| self.combat.cooling(me, s, now));
                    if !cooling {
                        self.combat.restart_clock(ITEM_SLOT);
                    }
                    me.fill_item_slot(item, &skill, cooling);
                }
                self.inventory.set_consumable(Some(item));
                Ok(())
            }
            _ => Err(ActionError::Refused(
                "the server wouldn't put that item in the item slot".into(),
            )),
        };
        if let Some(Op::Equip { reply, .. }) = self.current.take() {
            self.replies.push((reply, result));
        }
    }
}
