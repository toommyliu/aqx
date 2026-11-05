use std::task::Poll;
use std::time::Instant;

use mira_protocol::{ItemId, packet};

use super::{ActionError, Limit, Op, Session};
use crate::REPLY_TIMEOUT;
use crate::room::CombatState;

const ALWAYS_EQUIPPABLE: [&str; 4] = ["mi", "co", "pe", "am"];

pub(super) enum EquipStage {
    Waiting { wake: Option<Instant> },
    Sent { deadline: Instant },
}

impl EquipStage {
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            EquipStage::Waiting { wake } => *wake,
            EquipStage::Sent { deadline } => Some(*deadline),
        }
    }
}

impl Session {
    pub(super) fn step_equip(
        &mut self,
        item: ItemId,
        on: bool,
        stage: &mut EquipStage,
        now: Instant,
    ) -> Poll<Result<(), ActionError>> {
        if let Err(e) = self.ensure_ready() {
            return Poll::Ready(Err(e));
        }
        if let EquipStage::Sent { deadline } = *stage {
            return if now >= deadline {
                Poll::Ready(Err(ActionError::NoReply))
            } else {
                Poll::Pending
            };
        }
        let worn = match self.check_equip(item, on) {
            Ok(worn) => worn,
            Err(e) => return Poll::Ready(Err(e)),
        };
        if worn == on {
            return Poll::Ready(Ok(()));
        }
        let limit = if on {
            Limit::EquipItem
        } else {
            Limit::UnequipItem
        };
        if let Some(until) = self.cooling(limit, now) {
            *stage = EquipStage::Waiting { wake: Some(until) };
            return Poll::Pending;
        }
        let Some(room) = &self.room else {
            return Poll::Ready(Err(ActionError::NotReady));
        };
        let packet = if on {
            packet::equip_item(room.area, item)
        } else {
            packet::unequip_item(room.area, item)
        };
        self.limits.used(limit, now);
        self.send(&packet);
        *stage = EquipStage::Sent {
            deadline: now + REPLY_TIMEOUT,
        };
        Poll::Pending
    }

    fn check_equip(&self, id: ItemId, on: bool) -> Result<bool, ActionError> {
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
            CombatState::Dead => return Err(ActionError::Dead),
            CombatState::InCombat => return Err(ActionError::InCombat),
            CombatState::Idle => {}
        }
        if item.equipped == on {
            return Ok(on);
        }
        let consumable = item.kind.eq_ignore_ascii_case("item");
        if consumable {
            return unusable(format!(
                "{} goes in the item slot, not on the character",
                item.name
            ));
        }
        if !on {
            if item.slot == "Weapon" || item.slot == "ar" {
                return unusable(format!(
                    "{} can't be unequipped; equip another instead",
                    item.name
                ));
            }
            return Ok(true);
        }
        if item.member && !me.member {
            return unusable(format!("{} is for members", item.name));
        }
        if item.enhancement_level > me.level {
            return unusable(format!(
                "{} needs level {}",
                item.name, item.enhancement_level
            ));
        }
        if !ALWAYS_EQUIPPABLE.contains(&item.slot.as_str()) && item.enhancement_id <= 0 {
            return unusable(format!("{} needs an enhancement first", item.name));
        }
        Ok(false)
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
}
