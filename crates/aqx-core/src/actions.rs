use std::fmt;
use std::time::{Duration, Instant};

use aqx_protocol::{Aim, MonMapId, Out};

use crate::pacer::Kind;
use crate::session::{Halt, Phase, Session};
use crate::world::{Monster, SkillTarget, State, World};

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const REST_TIMEOUT: Duration = Duration::from_secs(30);
const PICKUP_TIMEOUT: Duration = Duration::from_secs(5);
const JUMP_SETTLE: Duration = Duration::from_millis(500);
const SERVER_MARGIN: Duration = Duration::from_millis(200);
const HOLD_RECHECK: Duration = Duration::from_millis(200);
const HASTE_CAP: f64 = 0.5;
const AUTO_ATTACK: usize = 0;

#[derive(Debug)]
pub enum ActError {
    Halted(Halt),
    Timeout(String),
    NotFound(String),
}

impl From<Halt> for ActError {
    fn from(halt: Halt) -> Self {
        ActError::Halted(halt)
    }
}

impl fmt::Display for ActError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActError::Halted(Halt::Stopped) => write!(f, "stopped"),
            ActError::Halted(Halt::Timeout) => write!(f, "timed out"),
            ActError::Halted(Halt::Closed(why)) => write!(f, "{why}"),
            ActError::Timeout(what) => write!(f, "timed out {what}"),
            ActError::NotFound(what) => write!(f, "{what} not found"),
        }
    }
}

pub type Predicate<'a> = Box<dyn FnMut(&World) -> bool + 'a>;

pub struct Step<'a> {
    pub slot: usize,
    pub when: Option<Predicate<'a>>,
}

pub struct Fight<'a> {
    pub steps: Vec<Step<'a>>,
    pub strict: bool,
    pub hold: Option<Predicate<'a>>,
    pub anti_counter: bool,
    next: usize,
}

impl Default for Fight<'_> {
    fn default() -> Self {
        Fight::new((1..=4).map(|slot| Step { slot, when: None }).collect())
    }
}

impl<'a> Fight<'a> {
    pub fn new(steps: Vec<Step<'a>>) -> Self {
        Fight { steps, strict: false, hold: None, anti_counter: true, next: 0 }
    }
}

enum Choice {
    Cast(usize),
    Wait(Instant),
}

impl Session {
    fn ready(&mut self, deadline: Option<Instant>) -> Result<(), ActError> {
        self.wait_for(deadline, "waiting to be alive and ready", |s| {
            s.phase == Phase::Ready && s.world.me.as_ref().is_some_and(|m| m.vitals.state != State::Dead)
        })
    }

    fn wait_for(&mut self, deadline: Option<Instant>, what: &str, mut done: impl FnMut(&Session) -> bool) -> Result<(), ActError> {
        match self.pump_until(deadline, |s| done(s).then_some(())) {
            Err(Halt::Timeout) => Err(ActError::Timeout(what.into())),
            other => Ok(other?),
        }
    }

    pub fn join(&mut self, map: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let base = map.split('-').next().unwrap_or(map);
        let here = |s: &Session| s.world.area.as_ref().is_some_and(|a| a.map.eq_ignore_ascii_case(base) && s.joined == Some(a.id));
        let same_room = self.world.area.as_ref().is_some_and(|a| a.name.eq_ignore_ascii_case(map));
        if here(self) && (same_room || !map.contains('-')) {
            return Ok(());
        }
        let user = self.world.me.as_ref().map(|m| m.user.clone()).unwrap_or_default();
        let before = self.joined;
        self.send_paced(Out::Join { user: &user, map }, Kind::Locked("tfer"))?;
        self.wait_for(Some(Instant::now() + JOIN_TIMEOUT), &format!("joining {map}"), |s| here(s) && s.joined != before)
    }

    pub fn jump(&mut self, cell: &str, pad: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Err(ActError::NotFound("a map".into())) };
        if me.cell == cell && me.pad == pad {
            return Ok(());
        }
        let room = area.id;
        self.send_paced(Out::MoveToCell { room, cell, pad }, Kind::Free)?;
        self.moved_to(cell, pad);
        self.idle_until(Instant::now() + JUMP_SETTLE)?;
        Ok(())
    }

    pub fn rest(&mut self) -> Result<(), ActError> {
        self.ready(None)?;
        let full = |s: &Session| s.world.me.as_ref().is_some_and(|m| m.vitals.hp >= m.vitals.max_hp && m.vitals.mp >= m.vitals.max_mp);
        if full(self) {
            return Ok(());
        }
        self.send_paced(Out::Rest, Kind::Locked("rest"))?;
        self.send(Out::Emote("rest"));
        self.wait_for(Some(Instant::now() + REST_TIMEOUT), "resting", full)
    }

    pub fn pickup(&mut self, name: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(item) = self.world.drops.iter().find(|d| d.name.eq_ignore_ascii_case(name)).map(|d| d.id) else {
            return Err(ActError::NotFound(format!("a drop named {name}")));
        };
        let Some(room) = self.world.area.as_ref().map(|a| a.id) else { return Err(ActError::NotFound("a map".into())) };
        self.send_paced(Out::PickUp { room, item }, Kind::Free)?;
        self.wait_for(Some(Instant::now() + PICKUP_TIMEOUT), &format!("picking up {name}"), |s| s.world.drops.iter().all(|d| d.id != item))
    }

    pub fn kill(
        &mut self,
        target: &dyn Fn(&Monster) -> bool,
        fight: &mut Fight,
        cell: Option<(&str, &str)>,
        timeout: Option<Duration>,
    ) -> Result<(), ActError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            self.ready(deadline)?;
            if self.world.me.as_ref().is_none_or(|m| m.skills.is_empty()) {
                return Err(ActError::NotFound("a skill list (the server never sent sAct)".into()));
            }
            if let Some((cell, pad)) = cell {
                self.jump(cell, pad)?;
            }
            let Some(id) = self.target_in_cell(target) else {
                if cell.is_none()
                    && let Some(best) = self.best_cell(target)
                    && self.world.me.as_ref().is_some_and(|m| m.cell != best)
                {
                    self.jump(&best, "Left")?;
                    continue;
                }
                let hunting = cell.is_none();
                self.wait_for(deadline, "waiting for a target", |s| {
                    s.target_in_cell(target).is_some() || s.dead() || (hunting && s.live_target_anywhere(target))
                })?;
                continue;
            };
            if self.fight(id, fight, deadline)? {
                return Ok(());
            }
        }
    }

    fn dead(&self) -> bool {
        self.world.me.as_ref().is_none_or(|m| m.vitals.state == State::Dead)
    }

    fn fight(&mut self, id: MonMapId, fight: &mut Fight, deadline: Option<Instant>) -> Result<bool, ActError> {
        loop {
            let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Ok(false) };
            let Some(monster) = area.monsters.iter().find(|m| m.map_id == id) else { return Ok(false) };
            if !monster.alive() {
                return Ok(true);
            }
            if me.vitals.state == State::Dead || me.cell != monster.cell {
                return Ok(false);
            }
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return Err(ActError::Timeout("killing".into()));
            }
            let countering = fight.anti_counter && area.monsters.iter().any(|m| m.cell == me.cell && m.countering(now));
            let holding = fight.hold.as_mut().is_some_and(|hold| hold(&self.world));
            if countering || holding {
                self.idle_until(cap(now + HOLD_RECHECK, deadline))?;
                continue;
            }
            match self.choose(fight, now) {
                Choice::Cast(slot) => self.cast(slot, id)?,
                Choice::Wait(at) => self.idle_until(cap(at, deadline))?,
            }
        }
    }

    fn choose(&self, fight: &mut Fight, now: Instant) -> Choice {
        let mut earliest = now + HOLD_RECHECK;
        let order: Vec<usize> = if fight.strict && !fight.steps.is_empty() { vec![fight.next % fight.steps.len()] } else { (0..fight.steps.len()).collect() };
        for i in order {
            let step = &mut fight.steps[i];
            let Some(at) = self.castable_at(step.slot, now) else { continue };
            if at > now {
                earliest = earliest.min(at);
                continue;
            }
            if step.when.as_mut().is_none_or(|when| when(&self.world)) {
                fight.next = i + 1;
                return Choice::Cast(step.slot);
            }
        }
        match self.castable_at(AUTO_ATTACK, now) {
            Some(at) if at <= now => Choice::Cast(AUTO_ATTACK),
            Some(at) => Choice::Wait(earliest.min(at)),
            None => Choice::Wait(earliest),
        }
    }

    fn castable_at(&self, slot: usize, now: Instant) -> Option<Instant> {
        let me = self.world.me.as_ref()?;
        let skill = me.skills.get(slot).filter(|s| !s.slot.starts_with('i'))?;
        let stat = |name: &str, default: f64| me.stats.get(name).copied().unwrap_or(default);
        if f64::from(skill.mana) * stat("$cmc", 1.0) > f64::from(me.vitals.mp) {
            return None;
        }
        let cooldown = skill.cooldown.mul_f64(1.0 - stat("$tha", 0.0).clamp(-1.0, HASTE_CAP));
        let own = self.casts.get(&skill.slot).map_or(now, |last| *last + cooldown + SERVER_MARGIN);
        let kind = if slot == AUTO_ATTACK { Kind::AutoAttack } else { Kind::ClassSkill };
        Some(own.max(self.pacer_ready_at(kind, now)))
    }

    fn cast(&mut self, slot: usize, id: MonMapId) -> Result<(), ActError> {
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Ok(()) };
        let Some(skill) = me.skills.get(slot) else { return Ok(()) };
        let aims: Vec<Aim> = match skill.target {
            SkillTarget::Hostile => {
                let mut others: Vec<&Monster> = area.monsters.iter().filter(|m| m.map_id != id && m.cell == me.cell && m.alive()).collect();
                others.sort_by_key(|m| m.vitals.hp);
                std::iter::once(id).chain(others.iter().map(|m| m.map_id)).take(skill.max_targets as usize).map(Aim::Monster).collect()
            }
            SkillTarget::Friendly | SkillTarget::Myself => vec![Aim::Player(me.eid)],
        };
        let name = skill.slot.clone();
        let kind = if slot == AUTO_ATTACK { Kind::AutoAttack } else { Kind::ClassSkill };
        let counter = self.next_counter();
        self.send_paced(Out::Skill { counter, slot: &name, targets: &aims }, kind)?;
        self.casts.insert(name, Instant::now());
        Ok(())
    }

    fn live_target_anywhere(&self, target: &dyn Fn(&Monster) -> bool) -> bool {
        self.world.area.as_ref().is_some_and(|a| a.monsters.iter().any(|m| m.alive() && target(m)))
    }

    fn target_in_cell(&self, target: &dyn Fn(&Monster) -> bool) -> Option<MonMapId> {
        let (me, area) = (self.world.me.as_ref()?, self.world.area.as_ref()?);
        area.monsters.iter().filter(|m| m.cell == me.cell && m.alive() && target(m)).min_by_key(|m| m.vitals.hp).map(|m| m.map_id)
    }

    fn best_cell(&self, target: &dyn Fn(&Monster) -> bool) -> Option<String> {
        let area = self.world.area.as_ref()?;
        let mut cells: Vec<(&str, usize, usize)> = Vec::new();
        for m in area.monsters.iter().filter(|m| target(m)) {
            match cells.iter_mut().find(|(c, _, _)| *c == m.cell) {
                Some((_, alive, total)) => {
                    *alive += usize::from(m.alive());
                    *total += 1;
                }
                None => cells.push((&m.cell, usize::from(m.alive()), 1)),
            }
        }
        cells.into_iter().max_by_key(|(_, alive, total)| (*alive, *total)).map(|(c, _, _)| c.to_string())
    }
}

fn cap(at: Instant, deadline: Option<Instant>) -> Instant {
    deadline.map_or(at, |d| at.min(d))
}
