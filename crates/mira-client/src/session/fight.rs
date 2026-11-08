use std::collections::BTreeMap;
use std::time::Instant;

use mira_protocol::packet::{self, Aim};
use mira_protocol::{EntityId, ItemId, MonMapId};

use super::{
    AGGRO_EVERY, ActionError, CELL_PAD, CastStage, CounterHold, EXIT_TIMEOUT, JoinStage, Op,
    Prepared, Reply, Session,
};
use crate::combat::{self, AUTO_ATTACK, ActionResult, ITEM_SLOT, Outcome, Readiness};
use crate::player::{Skill, SkillTarget};
use crate::room::{CombatState, Monster, SPAWN};

pub(super) struct Fight {
    pub(super) reply: Reply,
    pub(super) target: MonMapId,
    pub(super) slots: Option<Vec<String>>,
    pub(super) next: usize,
    pub(super) wake: Option<Instant>,
}

pub(super) struct PendingCast {
    pub(super) reply: Reply,
    pub(super) slot: String,
    pub(super) target: Option<Aim>,
    pub(super) wake: Option<Instant>,
}

impl Session {
    pub(super) fn readiness(&self, now: Instant) -> BTreeMap<String, Readiness> {
        let Some(me) = &self.character else {
            return BTreeMap::new();
        };
        me.skills
            .iter()
            .map(|s| (s.slot.clone(), self.combat.readiness(me, s, now)))
            .collect()
    }

    pub(super) fn step_fight(&mut self, now: Instant) {
        let Some(fight) = self.fight.take() else {
            return;
        };
        match self.step_kill(fight.target, fight.next, fight.slots.as_deref(), now) {
            Ok(Some((wake, next))) => {
                self.fight = Some(Fight {
                    wake,
                    next,
                    ..fight
                })
            }
            Ok(None) => self.replies.push((fight.reply, Ok(()))),
            Err(e) => self.replies.push((fight.reply, Err(e))),
        }
    }

    pub(super) fn step_pending_cast(&mut self, now: Instant) {
        let Some(cast) = self.pending_cast.take() else {
            return;
        };
        let moving = matches!(self.current, Some(Op::Join { .. } | Op::Leave { .. }));
        let prepared = match self.ensure_ready() {
            Err(e) => Err(e),
            Ok(()) if moving => Ok(Prepared::Wait(None)),
            Ok(()) => self.prepare(&cast.slot, cast.target, now),
        };
        match prepared {
            Ok(Prepared::Wait(wake)) => self.pending_cast = Some(PendingCast { wake, ..cast }),
            Ok(Prepared::Fire { aims, hostile }) => {
                self.fire(&cast.slot, &aims, hostile, now);
                self.replies.push((cast.reply, Ok(())));
            }
            Err(e) => self.replies.push((cast.reply, Err(e))),
        }
    }

    pub(super) fn tick_aggro(&mut self, now: Instant) {
        let Some((mode, due)) = &mut self.aggro else {
            return;
        };
        if now < *due {
            return;
        }
        *due = now
            + if matches!(mode, super::Aggro::All) {
                std::time::Duration::from_millis(250)
            } else {
                AGGRO_EVERY
            };
        let joining = matches!(
            self.current,
            Some(Op::Join {
                stage: JoinStage::Sent { .. },
                ..
            })
        );
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        if joining || me.state == CombatState::Dead || self.ensure_ready().is_err() {
            return;
        }
        let ids: Vec<MonMapId> = room
            .monsters
            .iter()
            .filter(|m| {
                matches!(self.aggro, Some((super::Aggro::All, _)))
                    || (m.cell == room.cell && m.alive())
            })
            .map(|m| m.id)
            .filter(|id| {
                self.aggro
                    .as_ref()
                    .is_some_and(|(aggro, _)| aggro.covers(*id))
            })
            .collect();
        if !ids.is_empty() {
            let area = room.area;
            self.send_unpaced(&packet::aggro_mon(area, &ids));
        }
    }

    pub(super) fn aggro_cell(&mut self) {
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        if me.state == CombatState::Dead {
            return;
        }
        let ids: Vec<MonMapId> = room
            .monsters
            .iter()
            .filter(|m| m.aggressive && m.cell == room.cell && m.alive())
            .map(|m| m.id)
            .collect();
        if !ids.is_empty() {
            let area = room.area;
            self.send_unpaced(&packet::aggro_mon(area, &ids));
        }
    }

    pub(super) fn aggro_respawned(&mut self, id: MonMapId) {
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        let here = room
            .monsters
            .iter()
            .any(|m| m.id == id && m.aggressive && m.cell == room.cell);
        if here && me.state != CombatState::Dead {
            let area = room.area;
            self.send_unpaced(&packet::aggro_mon(area, &[id]));
        }
    }

    pub(super) fn attack(&mut self, target: MonMapId, now: Instant) -> Result<(), ActionError> {
        self.ensure_ready()?;
        let target = self.pick_target(Some(target))?;
        if self
            .character
            .as_ref()
            .is_some_and(|me| me.state == CombatState::Dead)
        {
            self.combat.hold(target, now);
        } else {
            self.combat.engage(target, now);
        }
        Ok(())
    }

    pub(super) fn start_leaving(
        &mut self,
        now: Instant,
    ) -> Result<Option<(String, Instant)>, ActionError> {
        self.ensure_ready()?;
        self.combat.disengage();
        if self
            .character
            .as_ref()
            .is_none_or(|me| me.state != CombatState::InCombat)
        {
            return Ok(None);
        }
        let Some((cell, pad)) = self.calm_cell() else {
            return Err(ActionError::Unusable(
                "every known cell in this map has aggressive monsters".into(),
            ));
        };
        self.move_to_cell(&cell, &pad)?;
        Ok(Some((cell, now + EXIT_TIMEOUT)))
    }

    pub(super) fn calm_cell(&self) -> Option<(String, String)> {
        let room = self.room.as_ref()?;
        let calm = |cell: &str| {
            !room
                .monsters
                .iter()
                .any(|m| m.cell == cell && m.aggressive && m.alive())
        };
        if calm(&room.cell) {
            return Some((room.cell.clone(), room.pad.clone()));
        }
        let cell = std::iter::once(SPAWN.0)
            .chain(room.monsters.iter().map(|m| m.cell.as_str()))
            .find(|cell| calm(cell) && !room.locked(cell))?;
        let pad = if cell == SPAWN.0 { SPAWN.1 } else { CELL_PAD };
        Some((cell.to_owned(), pad.to_owned()))
    }

    pub(super) fn pick_target(&self, wanted: Option<MonMapId>) -> Result<MonMapId, ActionError> {
        let Some(room) = &self.room else {
            return Err(ActionError::NotReady);
        };
        let Some(id) = wanted else {
            let alive_here = |m: &&Monster| m.cell == room.cell && m.alive();
            if let Some(id) = self
                .combat
                .auto_target()
                .filter(|id| room.monsters.iter().filter(alive_here).any(|m| m.id == *id))
            {
                return Ok(id);
            }
            return room
                .monsters
                .iter()
                .filter(alive_here)
                .min_by_key(|m| m.hp)
                .map(|m| m.id)
                .ok_or_else(|| ActionError::NoTarget("no live monster in this cell".into()));
        };
        let Some(monster) = room.monsters.iter().find(|m| m.id == id) else {
            return Err(ActionError::NoTarget(format!(
                "no monster #{id} in this map"
            )));
        };
        if monster.cell != room.cell {
            return Err(ActionError::NoTarget(format!(
                "{} #{id} is in {}, not this cell",
                monster.name, monster.cell
            )));
        }
        if !monster.alive() {
            return Err(ActionError::NoTarget(format!(
                "{} #{id} is dead",
                monster.name
            )));
        }
        Ok(id)
    }

    fn pick_friend(&self, eid: EntityId) -> Result<EntityId, ActionError> {
        let Some(room) = &self.room else {
            return Err(ActionError::NotReady);
        };
        if Some(eid) == self.me {
            return Ok(eid);
        }
        let Some(player) = room.players.iter().find(|p| p.eid == eid) else {
            return Err(ActionError::NoTarget(format!(
                "no player #{eid} in this map"
            )));
        };
        if player.cell.as_ref() != Some(&room.cell) {
            return Err(ActionError::NoTarget(format!(
                "{} isn't in this cell",
                player.name
            )));
        }
        if player.state == CombatState::Dead {
            return Err(ActionError::NoTarget(format!("{} is dead", player.name)));
        }
        Ok(eid)
    }

    pub(super) fn step_cast(
        &mut self,
        slot: &str,
        target: Option<Aim>,
        stage: CastStage,
        now: Instant,
    ) -> Result<Option<CastStage>, ActionError> {
        self.ensure_ready()?;
        if let CastStage::Sent { act } = stage {
            return match self.combat.outcome(slot, act) {
                None => Ok(Some(stage)),
                Some(Outcome::Landed) => Ok(None),
                Some(Outcome::Refused) => Err(ActionError::Refused(format!(
                    "the server refused {}",
                    self.skill_name(slot)
                ))),
                Some(Outcome::Lost) => Err(ActionError::NoReply),
            };
        }
        match self.prepare(slot, target, now)? {
            Prepared::Wait(wake) => Ok(Some(CastStage::Waiting { wake })),
            Prepared::Fire { aims, hostile } => {
                let act = self.fire(slot, &aims, hostile, now);
                Ok(Some(CastStage::Sent { act }))
            }
        }
    }

    pub(super) fn skill_name(&self, slot: &str) -> String {
        self.character
            .as_ref()
            .and_then(|me| me.skills.iter().find(|s| s.slot == slot))
            .map_or_else(|| slot.to_owned(), |s| s.name.clone())
    }

    pub(super) fn prepare(
        &self,
        slot: &str,
        target: Option<Aim>,
        now: Instant,
    ) -> Result<Prepared, ActionError> {
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        if me.state == CombatState::Dead {
            return Err(ActionError::Dead);
        }
        if slot == AUTO_ATTACK {
            return Err(ActionError::Unusable(
                "the auto attack repeats on its own once it starts; attack a monster instead"
                    .into(),
            ));
        }
        if slot == ITEM_SLOT && self.slotted_item().is_none() {
            return Err(ActionError::Unusable(
                "nothing is equipped in the item slot".into(),
            ));
        }
        let Some(skill) = me.skills.iter().find(|s| s.slot == slot) else {
            return Err(ActionError::Unusable(format!("no skill in slot {slot}")));
        };
        let primary = match (skill.target, target) {
            (SkillTarget::Hostile, Some(Aim::Player(_))) => {
                return Err(ActionError::NoTarget(format!(
                    "{} can't target a player",
                    skill.name
                )));
            }
            (SkillTarget::Hostile, Some(Aim::Monster(id))) => {
                Some(Aim::Monster(self.pick_target(Some(id))?))
            }
            (SkillTarget::Hostile, None) => Some(Aim::Monster(self.pick_target(None)?)),
            (SkillTarget::Friendly, Some(Aim::Player(eid))) => {
                Some(Aim::Player(self.pick_friend(eid)?))
            }
            _ => None,
        };
        let hostile = match primary {
            Some(Aim::Monster(id)) => Some(id),
            _ => None,
        };
        if let Some(until) = self.held_by_counter(skill, hostile, now) {
            return Ok(Prepared::Wait(until));
        }
        match self.combat.gate(me, skill, now) {
            Readiness::Locked => Err(ActionError::Unusable(format!(
                "{} is locked at this class rank",
                skill.name
            ))),
            Readiness::NoMana => Err(ActionError::Unusable(format!(
                "not enough mana for {}",
                skill.name
            ))),
            Readiness::Casting | Readiness::Disabled => Ok(Prepared::Wait(None)),
            Readiness::At(at) => Ok(Prepared::Wait(Some(at))),
            Readiness::Ready => Ok(Prepared::Fire {
                aims: combat::aims(skill, primary, me, room),
                hostile,
            }),
        }
    }

    pub(super) fn slotted_item(&self) -> Option<ItemId> {
        let item = self.character.as_ref()?.item_slot?;
        let held = self
            .inventory
            .items
            .iter()
            .any(|i| i.id == item && i.qty > 0);
        held.then_some(item)
    }

    pub(super) fn fire(
        &mut self,
        slot: &str,
        aims: &[Aim],
        hostile: Option<MonMapId>,
        now: Instant,
    ) -> u8 {
        let act = self.combat.cast(slot, now);
        match self.slotted_item().filter(|_| slot == ITEM_SLOT) {
            Some(item) => {
                self.send_action(&packet::gar_item(act, slot, aims, item));
                if let Some(me) = &mut self.character
                    && let Some(skill) = me.skills.iter_mut().find(|s| s.slot == slot)
                {
                    skill.old_cooldown_ms = None;
                }
            }
            None => self.send_action(&packet::gar(act, slot, aims)),
        }
        if let Some(target) = hostile {
            self.combat.engage(target, now);
        }
        act
    }

    pub(super) fn step_kill(
        &mut self,
        target: MonMapId,
        mut next: usize,
        slots: Option<&[String]>,
        now: Instant,
    ) -> Result<Option<(Option<Instant>, usize)>, ActionError> {
        self.ensure_ready()?;
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        if me.state == CombatState::Dead {
            return Err(ActionError::Dead);
        }
        if room.monsters.iter().any(|m| m.id == target && !m.alive()) {
            return Ok(None);
        }
        let target = self.pick_target(Some(target))?;
        if let (Some((at, slot)), _) = self.rotation(next, now, target, slots)
            && let Prepared::Fire { aims, hostile } =
                self.prepare(&slot, Some(Aim::Monster(target)), now)?
        {
            self.fire(&slot, &aims, hostile, now);
            next = at + 1;
        }
        if self.combat.attacking() != Some(target) && !self.hostile_coming(slots, now) {
            self.combat.engage(target, now);
        }
        Ok(Some((self.rotation(next, now, target, slots).1, next)))
    }

    pub(super) fn rotation_skills(&self, slots: Option<&[String]>) -> Vec<&Skill> {
        let skills = self.character.iter().flat_map(|me| &me.skills);
        match slots {
            Some(slots) => slots
                .iter()
                .filter_map(|slot| skills.clone().find(|s| s.slot == *slot))
                .collect(),
            None => skills
                .filter(|s| s.slot != AUTO_ATTACK && s.slot != ITEM_SLOT)
                .collect(),
        }
    }

    pub(super) fn rotation(
        &self,
        next: usize,
        now: Instant,
        target: MonMapId,
        slots: Option<&[String]>,
    ) -> (Option<(usize, String)>, Option<Instant>) {
        let Some(me) = &self.character else {
            return (None, None);
        };
        let skills = self.rotation_skills(slots);
        let mut wake = None;
        for at in (0..skills.len()).map(|i| (next + i) % skills.len()) {
            if let Some(until) = self.held_by_counter(skills[at], Some(target), now) {
                wake = wake.into_iter().chain(until).min();
                continue;
            }
            match self.combat.gate(me, skills[at], now) {
                Readiness::Ready => return (Some((at, skills[at].slot.clone())), None),
                Readiness::At(ready) => wake = wake.into_iter().chain(Some(ready)).min(),
                Readiness::Casting
                | Readiness::Disabled
                | Readiness::Locked
                | Readiness::NoMana => {}
            }
        }
        (None, wake)
    }

    pub(super) fn class_skill_due(&self, now: Instant) -> Option<Instant> {
        let me = self.character.as_ref()?;
        let waiting: Vec<&Skill> = match (&self.fight, &self.pending_cast, &self.current) {
            (Some(fight), ..) => self.rotation_skills(fight.slots.as_deref()),
            (None, Some(PendingCast { slot, .. }), _)
            | (
                None,
                None,
                Some(Op::Cast {
                    slot,
                    stage: CastStage::Waiting { .. },
                    ..
                }),
            ) => me.skills.iter().filter(|s| s.slot == *slot).collect(),
            _ => Vec::new(),
        };
        waiting
            .into_iter()
            .filter(|s| self.combat.readiness(me, s, now) == Readiness::Ready)
            .filter_map(|s| match self.combat.gate(me, s, now) {
                Readiness::At(at) => Some(at),
                _ => None,
            })
            .min()
    }

    pub(super) fn hostile_coming(&self, slots: Option<&[String]>, now: Instant) -> bool {
        let Some(me) = &self.character else {
            return false;
        };
        self.rotation_skills(slots)
            .into_iter()
            .filter(|s| s.target == SkillTarget::Hostile)
            .any(|s| {
                matches!(
                    self.combat.gate(me, s, now),
                    Readiness::Ready | Readiness::At(_)
                )
            })
    }

    pub(super) fn tick_combat(&mut self, now: Instant) {
        if self.ensure_ready().is_err() {
            return;
        }
        if self.combat.respawn_due(now)
            && let (Some(room), Some(eid)) = (&self.room, self.me)
        {
            let area = room.area;
            self.send_action(&packet::res_player_timed(area, eid));
        }
        let Some(target) = self.combat.auto_target() else {
            return;
        };
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return;
        };
        let fighting = room
            .monsters
            .iter()
            .any(|m| m.id == target && m.cell == room.cell && m.alive());
        let skill = me.skills.iter().find(|s| s.slot == AUTO_ATTACK);
        let Some(skill) = skill.filter(|_| fighting) else {
            return self.combat.disengage();
        };
        let aims = combat::aims(skill, Some(Aim::Monster(target)), me, room);
        if self.combat.auto_went_last()
            && let Some(due) = self.class_skill_due(now)
        {
            return self.combat.defer_auto(due);
        }
        if self.combat.auto_ready(me, now)
            && self.held_by_counter(skill, Some(target), now).is_none()
        {
            self.fire(AUTO_ATTACK, &aims, None, now);
        }
    }

    pub(super) fn held_by_counter(
        &self,
        skill: &Skill,
        target: Option<MonMapId>,
        now: Instant,
    ) -> Option<Option<Instant>> {
        let room = self.room.as_ref()?;
        let hit: Vec<MonMapId> = match self.counter_hold {
            CounterHold::Off => return None,
            CounterHold::All => room
                .monsters
                .iter()
                .filter(|m| m.cell == room.cell)
                .map(|m| m.id)
                .collect(),
            CounterHold::Attacks => combat::aims(
                skill,
                target.map(Aim::Monster),
                self.character.as_ref()?,
                room,
            )
            .into_iter()
            .filter_map(|aim| match aim {
                Aim::Monster(id) => Some(id),
                Aim::Player(_) => None,
            })
            .collect(),
        };
        let countering: Vec<&Monster> = room
            .monsters
            .iter()
            .filter(|m| hit.contains(&m.id) && m.countering(now))
            .collect();
        if countering.is_empty() {
            return None;
        }
        Some(countering.iter().filter_map(|m| m.counter_deadline()).min())
    }

    pub(super) fn settle(&mut self, result: &ActionResult) {
        if let (Some(eid), Some(me)) = (self.me, &self.character) {
            self.combat.settle(result, eid, me, Instant::now());
        }
    }
}
