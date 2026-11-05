use std::collections::HashMap;
use std::iter;
use std::time::{Duration, Instant};

use mira_protocol::packet::Aim;
use mira_protocol::{EntityId, MonMapId, flex};
use serde::Deserialize;

use crate::REPLY_TIMEOUT;
use crate::player::{Me, Skill, SkillTarget};
use crate::room::{CombatState, Room};

pub(crate) const AUTO_ATTACK: &str = "aa";
pub(crate) const ITEM_SLOT: &str = "i1";
const RESPAWN_DELAY: Duration = Duration::from_secs(10);
const GLOBAL_COOLDOWN_MS: u32 = 1500;
const SKILL_GAP: Duration = Duration::from_millis(700);
const AUTO_GAP: Duration = Duration::from_millis(300);
const MIN_ONE_WAY: Duration = Duration::from_millis(20);
const AUTO_SLACK: Duration = Duration::from_millis(100);
const DISABLED_RETRY: Duration = Duration::from_millis(500);
const BACKOFF_STEP: Duration = Duration::from_millis(300);
const MAX_BACKOFF: Duration = Duration::from_millis(2400);
const BACKOFF_QUIET: Duration = Duration::from_secs(60);
const LAST_ACT: u8 = 250;
const DISABLING: [&str; 5] = ["stun", "stone", "paralyze", "disable", "disabled"];
const SLOW_DOWN: [&str; 3] = ["slow down", "too soon", "spamming"];

pub(crate) fn slow_down(warning: &str) -> bool {
    let warning = warning.to_ascii_lowercase();
    SLOW_DOWN.iter().any(|phrase| warning.contains(phrase))
}

#[derive(Default)]
pub(crate) struct Combat {
    next_act: u8,
    last_packet: Option<Instant>,
    global_from: Option<Instant>,
    clocks: HashMap<String, Clock>,
    auto: Option<Auto>,
    auto_rest: Option<Instant>,
    auto_last: bool,
    backoff: Backoff,
    respawn_at: Option<Instant>,
}

#[derive(Default)]
struct Clock {
    from: Option<Instant>,
    before: Option<Instant>,
    flight: Option<Flight>,
    settled: Option<(u8, Outcome)>,
}

#[derive(Clone, Copy)]
struct Flight {
    act: u8,
    sent: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Landed,
    Refused,
    Lost,
}

struct Auto {
    target: MonMapId,
    due: Instant,
}

pub(crate) enum Gate {
    Open,
    At(Instant),
    Held,
    Locked,
    NoMana,
}

#[derive(Default)]
struct Backoff {
    extra: Duration,
    since: Option<Instant>,
}

impl Backoff {
    fn now(&self, now: Instant) -> Duration {
        let Some(since) = self.since else {
            return Duration::ZERO;
        };
        let quiet = now.saturating_duration_since(since).as_secs() / BACKOFF_QUIET.as_secs();
        let quiet = u32::try_from(quiet).unwrap_or(u32::MAX);
        self.extra
            .saturating_sub(BACKOFF_STEP.saturating_mul(quiet))
    }

    fn bump(&mut self, now: Instant) {
        self.extra = (self.now(now) + BACKOFF_STEP).min(MAX_BACKOFF);
        self.since = Some(now);
    }
}

#[derive(Deserialize)]
pub(crate) struct ActionResult {
    #[serde(
        rename = "actID",
        default = "unmatched",
        deserialize_with = "flex::int"
    )]
    act: i64,
    #[serde(rename = "cInf", default)]
    caster: String,
    #[serde(rename = "iRes", default, deserialize_with = "flex::int")]
    result: i64,
}

fn unmatched() -> i64 {
    -1
}

impl Combat {
    pub(crate) fn sent(&mut self, now: Instant) {
        self.last_packet = Some(now);
    }

    pub(crate) fn gate(&self, me: &Me, skill: &Skill, now: Instant) -> Gate {
        self.gate_paced(me, skill, now, true)
    }

    pub(crate) fn ready_but_for_gap(&self, me: &Me, skill: &Skill, now: Instant) -> bool {
        matches!(self.gate_paced(me, skill, now, false), Gate::Open)
    }

    fn gate_paced(&self, me: &Me, skill: &Skill, now: Instant, paced: bool) -> Gate {
        if !skill.unlocked {
            return Gate::Locked;
        }
        if !affordable(me, skill) {
            return Gate::NoMana;
        }
        let clock = self.clocks.get(&skill.slot);
        if disabled(me) || clock.is_some_and(|c| c.flight.is_some()) {
            return Gate::Held;
        }
        let own = clock
            .and_then(|c| c.from)
            .map(|from| from + scaled(skill.cooldown_ms, me));
        let global = self
            .global_from
            .map(|from| from + scaled(GLOBAL_COOLDOWN_MS, me));
        let gap = self.gap(SKILL_GAP, now).filter(|_| paced);
        match [own, global, gap].into_iter().flatten().max() {
            Some(at) if at > now => Gate::At(at),
            _ => Gate::Open,
        }
    }

    fn gap(&self, gap: Duration, now: Instant) -> Option<Instant> {
        self.last_packet.map(|at| at + gap + self.backoff.now(now))
    }

    pub(crate) fn cast(&mut self, slot: &str, now: Instant) -> u8 {
        let act = self.next_act;
        self.next_act = if act >= LAST_ACT { 0 } else { act + 1 };
        let clock = self.clocks.entry(slot.to_owned()).or_default();
        clock.before = clock.from;
        clock.from = Some(now);
        clock.flight = Some(Flight { act, sent: now });
        clock.settled = None;
        self.auto_last = slot == AUTO_ATTACK;
        if !self.auto_last {
            self.global_from = Some(now);
        }
        act
    }

    pub(crate) fn settle(&mut self, result: &ActionResult, eid: EntityId, me: &Me, now: Instant) {
        if result.caster != format!("p:{eid}") {
            return;
        }
        let Some(slot) = self.clocks.iter().find_map(|(slot, clock)| {
            clock
                .flight
                .filter(|f| i64::from(f.act) == result.act)
                .map(|_| slot.clone())
        }) else {
            return;
        };
        let outcome = if result.result == 0 {
            Outcome::Refused
        } else {
            Outcome::Landed
        };
        self.finish(&slot, outcome, me, now);
    }

    fn finish(&mut self, slot: &str, outcome: Outcome, me: &Me, now: Instant) {
        let Some(clock) = self.clocks.get_mut(slot) else {
            return;
        };
        let Some(flight) = clock.flight.take() else {
            return;
        };
        let trip = now.saturating_duration_since(flight.sent);
        match outcome {
            Outcome::Landed => clock.from = Some(flight.sent + (trip / 2).max(MIN_ONE_WAY)),
            Outcome::Refused => clock.from = clock.before,
            Outcome::Lost => {}
        }
        clock.settled = Some((flight.act, outcome));
        if slot == AUTO_ATTACK {
            let cooldown = me
                .skills
                .iter()
                .find(|s| s.slot == AUTO_ATTACK)
                .map_or(Duration::ZERO, |s| scaled(s.cooldown_ms, me));
            let due = now + cooldown.saturating_sub(trip.min(AUTO_SLACK));
            self.auto_rest = Some(due);
            if let Some(auto) = &mut self.auto {
                auto.due = due;
            }
        }
    }

    pub(crate) fn expire(&mut self, me: &Me, now: Instant) {
        let lost: Vec<String> = self
            .clocks
            .iter()
            .filter(|(_, c)| c.flight.is_some_and(|f| now >= f.sent + REPLY_TIMEOUT))
            .map(|(slot, _)| slot.clone())
            .collect();
        for slot in lost {
            self.finish(&slot, Outcome::Lost, me, now);
        }
    }

    pub(crate) fn outcome(&self, slot: &str, act: u8) -> Option<Outcome> {
        self.clocks
            .get(slot)?
            .settled
            .filter(|(settled, _)| *settled == act)
            .map(|(_, outcome)| outcome)
    }

    pub(crate) fn engage(&mut self, target: MonMapId, now: Instant) {
        match &mut self.auto {
            Some(auto) => auto.target = target,
            None => {
                let due = self.auto_rest.map_or(now, |rest| rest.max(now));
                self.auto = Some(Auto { target, due });
            }
        }
    }

    pub(crate) fn auto_went_last(&self) -> bool {
        self.auto_last
    }

    pub(crate) fn defer_auto(&mut self, until: Instant) {
        if let Some(auto) = &mut self.auto {
            auto.due = auto.due.max(until);
        }
    }

    pub(crate) fn disengage(&mut self) {
        self.auto = None;
    }

    pub(crate) fn auto_target(&self) -> Option<MonMapId> {
        self.auto.as_ref().map(|auto| auto.target)
    }

    fn auto_due(&self, now: Instant) -> Option<Instant> {
        let auto = self.auto.as_ref()?;
        if self
            .clocks
            .get(AUTO_ATTACK)
            .is_some_and(|c| c.flight.is_some())
        {
            return None;
        }
        Some(
            self.gap(AUTO_GAP, now)
                .map_or(auto.due, |gap| gap.max(auto.due)),
        )
    }

    pub(crate) fn auto_ready(&mut self, me: &Me, now: Instant) -> bool {
        if self.auto_due(now).is_none_or(|due| due > now) {
            return false;
        }
        if disabled(me) {
            if let Some(auto) = &mut self.auto {
                auto.due = now + DISABLED_RETRY;
            }
            return false;
        }
        true
    }

    pub(crate) fn died(&mut self, now: Instant) {
        self.auto = None;
        self.respawn_at = Some(now + RESPAWN_DELAY);
    }

    pub(crate) fn revived(&mut self) {
        self.respawn_at = None;
    }

    pub(crate) fn respawn_due(&mut self, now: Instant) -> bool {
        if self.respawn_at.is_some_and(|at| now >= at) {
            self.respawn_at = None;
            return true;
        }
        false
    }

    pub(crate) fn warned(&mut self, text: &str, now: Instant) {
        if slow_down(text) {
            self.backoff.bump(now);
        }
    }

    pub(crate) fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let expiries = self
            .clocks
            .values()
            .filter_map(|c| c.flight.map(|f| f.sent + REPLY_TIMEOUT));
        expiries
            .chain(self.auto_due(now))
            .chain(self.respawn_at)
            .min()
    }
}

fn scaled(ms: u32, me: &Me) -> Duration {
    let haste = me.haste.clamp(-1.0, 0.5);
    Duration::from_millis((f64::from(ms) * (1.0 - haste)).round() as u64)
}

fn affordable(me: &Me, skill: &Skill) -> bool {
    me.mana_rate
        .is_none_or(|rate| (f64::from(skill.mana) * rate).round() <= f64::from(me.mp))
}

fn disabled(me: &Me) -> bool {
    me.auras.active.iter().any(|aura| {
        aura.category.as_deref().is_some_and(|category| {
            let category: String = category.split_whitespace().collect();
            DISABLING.contains(&category.to_lowercase().as_str())
        })
    })
}

pub(crate) fn aims(
    skill: &Skill,
    primary: Option<MonMapId>,
    eid: EntityId,
    room: &Room,
) -> Vec<Aim> {
    let most = usize::try_from(skill.max_targets.max(1)).unwrap_or(usize::MAX);
    match skill.target {
        SkillTarget::Hostile => {
            let Some(primary) = primary else {
                return Vec::new();
            };
            let mut others: Vec<_> = room
                .monsters
                .iter()
                .filter(|m| m.id != primary && m.cell == room.cell && m.alive())
                .collect();
            others.sort_by_key(|m| m.hp);
            iter::once(primary)
                .chain(others.into_iter().map(|m| m.id))
                .take(most)
                .map(Aim::Monster)
                .collect()
        }
        SkillTarget::Friendly => {
            let mut others: Vec<_> = room
                .players
                .iter()
                .filter(|p| p.cell.as_ref() == Some(&room.cell) && p.state != CombatState::Dead)
                .collect();
            others.sort_by_key(|p| p.hp);
            iter::once(eid)
                .chain(others.into_iter().map(|p| p.eid))
                .take(most)
                .map(Aim::Player)
                .collect()
        }
        SkillTarget::Myself => vec![Aim::Player(eid)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn skill(slot: &str, cooldown_ms: u32) -> Skill {
        Skill {
            slot: slot.into(),
            name: slot.into(),
            cooldown_ms,
            mana: 0,
            target: SkillTarget::Hostile,
            max_targets: 1,
            unlocked: true,
        }
    }

    fn fighter() -> Me {
        Me {
            haste: 0.84,
            skills: vec![skill("aa", 2000), skill("a1", 4000), skill("a2", 1000)],
            ..Me::default()
        }
    }

    fn landed(act: u8) -> ActionResult {
        ActionResult {
            act: act.into(),
            caster: "p:7".into(),
            result: 1,
        }
    }

    fn gate_at(combat: &Combat, me: &Me, slot: &str, now: Instant) -> Option<Instant> {
        let skill = me.skills.iter().find(|s| s.slot == slot).unwrap();
        match combat.gate(me, skill, now) {
            Gate::Open => Some(now),
            Gate::At(at) => Some(at),
            _ => None,
        }
    }

    #[test]
    fn the_action_counter_wraps_after_250() {
        let mut combat = Combat::default();
        let now = Instant::now();
        let acts: Vec<u8> = (0..252).map(|_| combat.cast("a1", now)).collect();
        assert_eq!(acts[..3], [0, 1, 2]);
        assert_eq!(acts[250..], [250, 0]);
    }

    #[test]
    fn a_cast_that_reuses_an_action_id_waits_for_its_own_result() {
        let (me, t0) = (fighter(), Instant::now());
        let mut combat = Combat::default();
        let first = combat.cast("a1", t0);
        combat.settle(&landed(first), EntityId(7), &me, t0);
        assert_eq!(combat.outcome("a1", first), Some(Outcome::Landed));
        for _ in 0..LAST_ACT {
            combat.cast("a2", t0);
        }
        assert_eq!(combat.cast("a1", t0), first);
        assert_eq!(combat.outcome("a1", first), None);
    }

    #[test]
    fn cooldowns_scale_with_haste_capped_at_half() {
        let (me, t0) = (fighter(), Instant::now());
        let mut combat = Combat::default();
        combat.cast("a1", t0);
        combat.settle(&landed(0), EntityId(7), &me, t0);
        let later = t0 + ms(10);
        assert_eq!(
            gate_at(&combat, &me, "a1", later),
            Some(t0 + ms(20) + ms(2000))
        );
        assert_eq!(gate_at(&combat, &me, "a2", later), Some(t0 + ms(750)));
    }

    #[test]
    fn the_auto_attack_repeats_a_cooldown_after_each_result_less_the_round_trip() {
        let (me, t0) = (fighter(), Instant::now());
        let mut combat = Combat::default();
        combat.engage(MonMapId(1), t0);
        assert!(combat.auto_ready(&me, t0));
        combat.cast("aa", t0);
        assert!(!combat.auto_ready(&me, t0 + ms(5000)));
        combat.settle(&landed(0), EntityId(7), &me, t0 + ms(40));
        assert_eq!(combat.next_deadline(t0), Some(t0 + ms(1000)));
        combat.cast("aa", t0 + ms(1000));
        combat.settle(&landed(1), EntityId(7), &me, t0 + ms(1400));
        assert_eq!(combat.next_deadline(t0), Some(t0 + ms(2300)));
        combat.disengage();
        combat.engage(MonMapId(2), t0 + ms(1500));
        assert_eq!(combat.next_deadline(t0), Some(t0 + ms(2300)));
    }

    #[test]
    fn slow_down_warnings_widen_the_gap_until_a_quiet_minute() {
        let (me, t0) = (fighter(), Instant::now());
        let mut combat = Combat::default();
        combat.sent(t0);
        combat.warned("Last action was too soon!", t0);
        combat.warned("Please slow down", t0);
        combat.warned("You can't do that here.", t0);
        assert_eq!(gate_at(&combat, &me, "a2", t0), Some(t0 + ms(700 + 600)));
        let quiet = t0 + Duration::from_secs(60);
        combat.sent(quiet);
        assert_eq!(
            gate_at(&combat, &me, "a2", quiet),
            Some(quiet + ms(700 + 300))
        );
    }

    #[test]
    fn a_death_respawns_after_ten_seconds_unless_revived_first() {
        let t0 = Instant::now();
        let mut combat = Combat::default();
        combat.engage(MonMapId(1), t0);
        combat.died(t0);
        assert_eq!(combat.auto_target(), None);
        assert!(!combat.respawn_due(t0 + ms(9999)));
        assert!(combat.respawn_due(t0 + ms(10_000)));
        assert!(!combat.respawn_due(t0 + ms(20_000)));
        combat.died(t0);
        combat.revived();
        assert!(!combat.respawn_due(t0 + ms(20_000)));
    }
}
