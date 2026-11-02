use std::collections::HashMap;
use std::time::{Duration, Instant};

const GCD: Duration = Duration::from_millis(1500);
const STEP: Duration = Duration::from_millis(300);
const MAX_BACKOFF: Duration = Duration::from_millis(2400);
const QUIET: Duration = Duration::from_secs(60);
const RATE: f64 = 8.0;
const BURST: f64 = 4.0;

const LOCKS: [(&str, u64); 22] = [
    ("loadShop", 3000),
    ("loadEnhShop", 3000),
    ("loadHairShop", 3000),
    ("equipItem", 1500),
    ("unequipItem", 1500),
    ("buyItem", 1000),
    ("sellItem", 1000),
    ("getMapItem", 1000),
    ("tryQuestComplete", 1250),
    ("acceptQuest", 1000),
    ("doIA", 1000),
    ("rest", 1900),
    ("who", 3000),
    ("tfer", 3000),
    ("wearItem", 1500),
    ("unwearItem", 1500),
    ("addLoadout", 1500),
    ("removeLoadout", 1500),
    ("wearLoadout", 3000),
    ("equipLoadout", 3000),
    ("dungeonQueue", 5000),
    ("dungeonRejoin", 5000),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    ClassSkill,
    AutoAttack,
    Locked(&'static str),
    Free,
}

pub struct Pacer {
    last: HashMap<&'static str, Instant>,
    last_class: Option<Instant>,
    last_any: Option<Instant>,
    tokens: f64,
    tokens_at: Instant,
    backoff: Duration,
    backoff_at: Instant,
}

pub fn kind_for(action: &str) -> Kind {
    match action {
        "gar" => Kind::ClassSkill,
        _ => LOCKS.iter().find(|(name, _)| *name == action).map_or(Kind::Free, |(name, _)| Kind::Locked(name)),
    }
}

impl Pacer {
    pub fn new(now: Instant) -> Self {
        Pacer { last: HashMap::new(), last_class: None, last_any: None, tokens: BURST, tokens_at: now, backoff: Duration::ZERO, backoff_at: now }
    }

    pub fn ready_at(&self, kind: Kind, now: Instant) -> Instant {
        let backoff = self.backoff(now);
        let gap_since = |last: Option<Instant>, gap: Duration| last.map_or(now, |t| t + gap + backoff);
        let lock = match kind {
            Kind::ClassSkill => gap_since(self.last_class, GCD),
            Kind::Locked(name) => {
                let ms = LOCKS.iter().find(|(n, _)| *n == name).map_or(0, |(_, ms)| *ms);
                gap_since(self.last.get(name).copied(), Duration::from_millis(ms))
            }
            Kind::AutoAttack | Kind::Free => now,
        };
        let spacing = if backoff.is_zero() { now } else { gap_since(self.last_any, Duration::ZERO) };
        let tokens = self.tokens_now(now);
        let bucket = if tokens >= 1.0 { now } else { now + Duration::from_secs_f64((1.0 - tokens) / RATE) };
        lock.max(spacing).max(bucket).max(now)
    }

    pub fn sent(&mut self, kind: Kind, now: Instant) {
        self.tokens = self.tokens_now(now) - 1.0;
        self.tokens_at = now;
        self.last_any = Some(now);
        match kind {
            Kind::ClassSkill => self.last_class = Some(now),
            Kind::Locked(name) => {
                self.last.insert(name, now);
            }
            Kind::AutoAttack | Kind::Free => {}
        }
    }

    pub fn warned(&mut self, now: Instant) {
        self.backoff = (self.backoff(now) + STEP).min(MAX_BACKOFF);
        self.backoff_at = now;
    }

    fn backoff(&self, now: Instant) -> Duration {
        let quiet_minutes = (now.saturating_duration_since(self.backoff_at).as_secs() / QUIET.as_secs()) as u32;
        self.backoff.saturating_sub(STEP * quiet_minutes)
    }

    fn tokens_now(&self, now: Instant) -> f64 {
        (self.tokens + now.saturating_duration_since(self.tokens_at).as_secs_f64() * RATE).min(BURST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn class_skills_wait_out_the_global_cooldown_but_auto_attacks_do_not() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        pacer.sent(Kind::ClassSkill, t0);
        assert_eq!(pacer.ready_at(Kind::ClassSkill, t0), t0 + ms(1500));
        assert_eq!(pacer.ready_at(Kind::AutoAttack, t0), t0);
    }

    #[test]
    fn locked_actions_follow_the_client_table() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        pacer.sent(Kind::Locked("tfer"), t0);
        assert_eq!(pacer.ready_at(Kind::Locked("tfer"), t0 + ms(1000)), t0 + ms(3000));
        assert_eq!(pacer.ready_at(Kind::Locked("acceptQuest"), t0 + ms(1000)), t0 + ms(1000));
    }

    #[test]
    fn the_bucket_allows_a_burst_of_four_then_eight_a_second() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        for _ in 0..4 {
            assert_eq!(pacer.ready_at(Kind::Free, t0), t0);
            pacer.sent(Kind::Free, t0);
        }
        assert_eq!(pacer.ready_at(Kind::Free, t0), t0 + ms(125));
    }

    #[test]
    fn after_a_warning_every_packet_keeps_its_distance() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        pacer.sent(Kind::AutoAttack, t0);
        assert_eq!(pacer.ready_at(Kind::Free, t0), t0);
        pacer.warned(t0);
        assert_eq!(pacer.ready_at(Kind::Free, t0), t0 + ms(300));
        assert_eq!(pacer.ready_at(Kind::AutoAttack, t0), t0 + ms(300));
    }

    #[test]
    fn warnings_stretch_gaps_and_wear_off_with_quiet_minutes() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new(t0);
        pacer.sent(Kind::ClassSkill, t0);
        pacer.warned(t0);
        pacer.warned(t0);
        assert_eq!(pacer.ready_at(Kind::ClassSkill, t0), t0 + ms(2100));
        for _ in 0..10 {
            pacer.warned(t0);
        }
        assert_eq!(pacer.ready_at(Kind::ClassSkill, t0), t0 + ms(1500 + 2400));
        let later = t0 + Duration::from_secs(120);
        pacer.sent(Kind::ClassSkill, later);
        assert_eq!(pacer.ready_at(Kind::ClassSkill, later), later + ms(1500 + 1800));
    }
}
