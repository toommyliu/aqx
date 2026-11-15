use std::time::Duration;

use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, Zoned};

use crate::config::{AfterRun, Script, WhenBusy};

const GRACE: SignedDuration = SignedDuration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub name: String,
    pub account: String,
    pub scripts: Vec<Script>,
    pub days: Days,
    pub at: Time,
    pub length: Option<Length>,
    pub zone: Option<TimeZone>,
    pub when_busy: WhenBusy,
    pub after: AfterRun,
    pub catch_up: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    Until(Time),
    For(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days(u8);

impl Days {
    pub const EVERY: Days = Days(0x7f);
    pub const NONE: Days = Days(0);

    pub fn with(self, day: Weekday) -> Days {
        Days(self.0 | 1 << day.to_monday_zero_offset())
    }

    fn has(self, day: Weekday) -> bool {
        self.0 & 1 << day.to_monday_zero_offset() != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deadline {
    zone: TimeZone,
    end: End,
}

#[derive(Debug, Clone, PartialEq)]
enum End {
    Wall(DateTime),
    Real(Timestamp),
}

impl Deadline {
    pub fn reached(&self, now: Timestamp) -> bool {
        match self.end {
            End::Wall(at) => now.to_zoned(self.zone.clone()).datetime() >= at,
            End::Real(at) => now >= at,
        }
    }

    pub fn label(&self) -> String {
        let time = match self.end {
            End::Wall(at) => at.time(),
            End::Real(at) => at.to_zoned(self.zone.clone()).time(),
        };
        clock(time)
    }
}

pub fn clock(time: Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    No,
    Start,
    Reload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    Fire(Option<Deadline>),
    Missed,
}

impl Schedule {
    fn zone(&self) -> TimeZone {
        self.zone.clone().unwrap_or_else(TimeZone::system)
    }

    fn same_times(&self, other: &Schedule) -> bool {
        (self.at, self.days, &self.zone) == (other.at, other.days, &other.zone)
    }

    fn check(&self, handled: &mut Option<Date>, now: &Zoned, fresh: Fresh) -> Option<Due> {
        let wall = now.datetime();
        let mut day = wall.date();
        while !self.days.has(day.weekday()) || DateTime::from_parts(day, self.at) > wall {
            day = day.yesterday().ok()?;
        }
        if handled.is_some_and(|done| done >= day) {
            return None;
        }
        *handled = Some(day);
        let zone = now.time_zone();
        let start = DateTime::from_parts(day, self.at)
            .to_zoned(zone.clone())
            .ok()?
            .timestamp();
        if now.timestamp().duration_since(start) <= GRACE {
            return Some(Due::Fire(self.deadline(
                day,
                start.min(now.timestamp()),
                zone,
            )));
        }
        let end = self.deadline(day, start, zone);
        let inside = end
            .as_ref()
            .is_some_and(|end| !end.reached(now.timestamp()));
        match fresh {
            _ if inside && self.catch_up => Some(Due::Fire(end)),
            Fresh::No => Some(Due::Missed),
            Fresh::Start if inside => Some(Due::Missed),
            Fresh::Start | Fresh::Reload => None,
        }
    }

    fn deadline(&self, day: Date, start: Timestamp, zone: &TimeZone) -> Option<Deadline> {
        let end = match self.length? {
            Length::Until(until) if until > self.at => End::Wall(DateTime::from_parts(day, until)),
            Length::Until(until) => End::Wall(DateTime::from_parts(day.tomorrow().ok()?, until)),
            Length::For(length) => End::Real(start.checked_add(length).ok()?),
        };
        Some(Deadline {
            zone: zone.clone(),
            end,
        })
    }
}

struct Tracked {
    schedule: Schedule,
    handled: Option<Date>,
    fresh: Fresh,
}

#[derive(Default)]
pub struct Schedules {
    tracked: Vec<Tracked>,
}

impl Schedules {
    pub fn set(&mut self, schedules: Vec<Schedule>, fresh: Fresh) {
        let mut old = std::mem::take(&mut self.tracked);
        self.tracked = schedules
            .into_iter()
            .map(|schedule| {
                let kept = old
                    .iter()
                    .position(|t| t.schedule.name.eq_ignore_ascii_case(&schedule.name))
                    .map(|at| old.swap_remove(at))
                    .filter(|t| t.schedule.same_times(&schedule));
                match kept {
                    Some(t) => Tracked {
                        handled: t.handled,
                        fresh: match t.schedule.enabled {
                            false if schedule.enabled => Fresh::Reload,
                            _ => t.fresh,
                        },
                        schedule,
                    },
                    None => Tracked {
                        schedule,
                        handled: None,
                        fresh,
                    },
                }
            })
            .collect();
    }

    pub fn waiting(&self) -> bool {
        self.tracked.iter().any(|t| t.schedule.enabled)
    }

    pub fn due(&mut self, now: Timestamp) -> Vec<(Schedule, Due)> {
        self.tracked
            .iter_mut()
            .filter(|t| t.schedule.enabled)
            .filter_map(|t| {
                let now = now.to_zoned(t.schedule.zone());
                let fresh = std::mem::replace(&mut t.fresh, Fresh::No);
                let due = t.schedule.check(&mut t.handled, &now, fresh)?;
                Some((t.schedule.clone(), due))
            })
            .collect()
    }
}

#[cfg(test)]
impl Schedule {
    pub fn for_tests(account: &str, at: Time) -> Schedule {
        Schedule {
            name: "morning farm".into(),
            account: account.into(),
            scripts: vec![Script {
                path: "/srv/farm.ts".into(),
                args: Vec::new(),
            }],
            days: Days::EVERY,
            at,
            length: None,
            zone: Some(TimeZone::UTC),
            when_busy: WhenBusy::Skip,
            after: AfterRun::Stay,
            catch_up: false,
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::*;

    fn eastern() -> TimeZone {
        TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap()
    }

    fn at(zone: &TimeZone, wall: DateTime) -> Zoned {
        wall.to_zoned(zone.clone()).unwrap()
    }

    fn tick(
        schedule: &Schedule,
        handled: &mut Option<Date>,
        wall: DateTime,
        fresh: Fresh,
    ) -> Option<Due> {
        let zone = schedule.zone.clone().unwrap();
        schedule.check(handled, &at(&zone, wall), fresh)
    }

    fn fires(due: &Option<Due>) -> bool {
        matches!(due, Some(Due::Fire(_)))
    }

    fn running_since(schedule: &Schedule, wall: DateTime) -> Option<Date> {
        let mut handled = None;
        tick(schedule, &mut handled, wall, Fresh::Start);
        handled
    }

    fn ts(wall: DateTime) -> Timestamp {
        wall.to_zoned(TimeZone::UTC).unwrap().timestamp()
    }

    #[test]
    fn a_schedule_fires_once_when_its_time_comes_on_its_days() {
        let monday = date(2026, 11, 16);
        let schedule = Schedule {
            days: Days::NONE.with(Weekday::Monday).with(Weekday::Wednesday),
            ..Schedule::for_tests("player1", time(9, 30, 0, 0))
        };
        let mut handled = running_since(&schedule, monday.at(0, 0, 0, 0));
        let mut at = |wall: DateTime| tick(&schedule, &mut handled, wall, Fresh::No);
        assert_eq!(at(monday.at(9, 29, 59, 0)), None);
        assert!(fires(&at(monday.at(9, 30, 0, 0))));
        assert_eq!(at(monday.at(9, 30, 0, 250_000_000)), None, "once a day");
        assert_eq!(at(monday.at(18, 0, 0, 0)), None, "once a day");
        assert_eq!(
            at(date(2026, 11, 17).at(9, 30, 0, 0)),
            None,
            "not on its days"
        );
        assert!(fires(&at(date(2026, 11, 18).at(9, 30, 1, 0))));
    }

    #[test]
    fn a_late_tick_past_the_grace_is_a_miss_and_within_it_still_fires() {
        let schedule = Schedule::for_tests("player1", time(9, 30, 0, 0));
        let day = date(2026, 11, 16);
        let midnight = day.at(0, 0, 0, 0);
        let mut handled = running_since(&schedule, midnight);
        assert!(fires(&tick(
            &schedule,
            &mut handled,
            day.at(9, 31, 0, 0),
            Fresh::No
        )));
        let mut handled = running_since(&schedule, midnight);
        assert_eq!(
            tick(&schedule, &mut handled, day.at(9, 31, 1, 0), Fresh::No),
            Some(Due::Missed),
            "a wake from sleep after the grace misses"
        );
        assert_eq!(
            tick(&schedule, &mut handled, day.at(9, 32, 0, 0), Fresh::No),
            None
        );
        let mut handled = running_since(&schedule, midnight);
        assert_eq!(
            tick(
                &schedule,
                &mut handled,
                date(2026, 11, 19).at(12, 0, 0, 0),
                Fresh::No
            ),
            Some(Due::Missed),
            "a long sleep misses once"
        );
        assert_eq!(handled, Some(date(2026, 11, 19)));
    }

    #[test]
    fn a_session_starting_after_the_time_fires_only_inside_the_grace() {
        let schedule = Schedule::for_tests("player1", time(9, 30, 0, 0));
        let day = date(2026, 11, 16);
        let mut handled = None;
        assert!(fires(&tick(
            &schedule,
            &mut handled,
            day.at(9, 30, 30, 0),
            Fresh::Start
        )));
        for fresh in [Fresh::Start, Fresh::Reload] {
            let mut handled = None;
            assert_eq!(
                tick(&schedule, &mut handled, day.at(10, 0, 0, 0), fresh),
                None,
                "without a window there's nothing to miss"
            );
            assert_eq!(handled, Some(day));
        }
    }

    #[test]
    fn a_backward_clock_jump_never_fires_the_same_day_twice() {
        let schedule = Schedule::for_tests("player1", time(9, 30, 0, 0));
        let day = date(2026, 11, 16);
        let mut handled = running_since(&schedule, day.at(0, 0, 0, 0));
        let mut at = |wall: DateTime| tick(&schedule, &mut handled, wall, Fresh::No);
        assert!(fires(&at(day.at(9, 30, 0, 0))));
        assert_eq!(at(day.at(8, 0, 0, 0)), None);
        assert_eq!(at(day.at(9, 30, 0, 0)), None);
        assert_eq!(at(date(2026, 11, 15).at(9, 30, 10, 0)), None);
    }

    #[test]
    fn a_time_skipped_by_daylight_saving_fires_at_the_jump_and_a_repeated_one_fires_once() {
        let zone = eastern();
        let gap = Schedule {
            zone: Some(zone.clone()),
            ..Schedule::for_tests("player1", time(2, 30, 0, 0))
        };
        let spring = date(2026, 3, 8);
        let mut handled = running_since(&gap, spring.at(0, 0, 0, 0));
        let before = at(&zone, spring.at(1, 59, 59, 900_000_000));
        assert_eq!(gap.check(&mut handled, &before, Fresh::No), None);
        let jump = before
            .timestamp()
            .checked_add(SignedDuration::from_millis(250))
            .unwrap()
            .to_zoned(zone.clone());
        assert_eq!(jump.time(), time(3, 0, 0, 150_000_000), "the clock jumped");
        assert!(fires(&gap.check(&mut handled, &jump, Fresh::No)));

        let repeat = Schedule {
            zone: Some(zone.clone()),
            ..Schedule::for_tests("player1", time(1, 30, 0, 0))
        };
        let fall = date(2026, 11, 1);
        let first = at(&zone, fall.at(1, 30, 0, 0));
        let mut handled = running_since(&repeat, fall.at(0, 0, 0, 0));
        assert!(fires(&repeat.check(&mut handled, &first, Fresh::No)));
        let second = first
            .timestamp()
            .checked_add(SignedDuration::from_hours(1))
            .unwrap()
            .to_zoned(zone);
        assert_eq!(
            second.datetime(),
            fall.at(1, 30, 0, 0),
            "the same wall time again"
        );
        assert_eq!(repeat.check(&mut handled, &second, Fresh::No), None);
    }

    #[test]
    fn times_are_read_in_the_schedules_own_zone() {
        let schedule = Schedule {
            zone: Some(eastern()),
            days: Days::NONE.with(Weekday::Monday),
            ..Schedule::for_tests("player1", time(9, 30, 0, 0))
        };
        let monday = date(2026, 11, 16);
        let mut handled = running_since(&schedule, monday.at(0, 0, 0, 0));
        let utc = |h, m| ts(monday.at(h, m, 0, 0)).to_zoned(eastern());
        assert_eq!(schedule.check(&mut handled, &utc(9, 30), Fresh::No), None);
        assert!(fires(&schedule.check(
            &mut handled,
            &utc(14, 30),
            Fresh::No
        )));
    }

    #[test]
    fn until_stops_at_its_wall_time_and_an_earlier_until_is_the_next_day() {
        let day = date(2026, 11, 16);
        let next = date(2026, 11, 17);
        for (until, before, reached) in [
            (
                time(23, 0, 0, 0),
                day.at(22, 59, 59, 0),
                day.at(23, 0, 0, 0),
            ),
            (time(6, 0, 0, 0), next.at(5, 59, 59, 0), next.at(6, 0, 0, 0)),
        ] {
            let schedule = Schedule {
                length: Some(Length::Until(until)),
                ..Schedule::for_tests("player1", time(22, 0, 0, 0))
            };
            let mut handled = running_since(&schedule, day.at(0, 0, 0, 0));
            let Some(Due::Fire(Some(end))) =
                tick(&schedule, &mut handled, day.at(22, 0, 0, 0), Fresh::No)
            else {
                panic!("a fire with a deadline");
            };
            assert!(!end.reached(ts(before)) && end.reached(ts(reached)));
            assert_eq!(end.label(), clock(until));
        }
    }

    #[test]
    fn catch_up_fires_inside_the_window_for_what_is_left_of_it() {
        let day = date(2026, 11, 16);
        let windowed = |length, catch_up| Schedule {
            length: Some(length),
            catch_up,
            ..Schedule::for_tests("player1", time(9, 0, 0, 0))
        };
        let two_hours = Length::For(Duration::from_secs(7200));
        let schedule = windowed(two_hours, true);
        for fresh in [Fresh::No, Fresh::Start, Fresh::Reload] {
            let mut handled = None;
            let Some(Due::Fire(Some(end))) =
                tick(&schedule, &mut handled, day.at(10, 0, 0, 0), fresh)
            else {
                panic!("catch_up fires inside the window ({fresh:?})");
            };
            assert!(
                end.reached(ts(day.at(11, 0, 0, 0))),
                "for runs only for the rest of the window"
            );
            assert_eq!(end.label(), "11:00");
        }
        let mut handled = None;
        assert_eq!(
            tick(&schedule, &mut handled, day.at(11, 0, 0, 0), Fresh::Start),
            None,
            "after the window there's nothing to catch up"
        );
        let schedule = windowed(Length::Until(time(12, 0, 0, 0)), false);
        for (fresh, said) in [
            (Fresh::Start, Some(Due::Missed)),
            (Fresh::Reload, None),
            (Fresh::No, Some(Due::Missed)),
        ] {
            let mut handled = None;
            assert_eq!(
                tick(&schedule, &mut handled, day.at(10, 0, 0, 0), fresh),
                said,
                "without catch_up, a window that started is missed ({fresh:?})"
            );
        }
    }

    #[test]
    fn a_reload_keeps_what_fired_unless_the_times_change() {
        let day = date(2026, 11, 16);
        let now = |h, m, s| ts(day.at(h, m, s, 0));
        let schedule = Schedule::for_tests("player1", time(9, 30, 0, 0));
        let mut schedules = Schedules::default();
        schedules.set(vec![schedule.clone()], Fresh::Start);
        assert!(schedules.waiting());
        assert_eq!(schedules.due(now(9, 0, 0)), []);
        assert_eq!(schedules.due(now(9, 30, 0)).len(), 1);
        let edited = Schedule {
            after: AfterRun::Logout,
            ..schedule
        };
        schedules.set(vec![edited.clone()], Fresh::Reload);
        assert_eq!(
            schedules.due(now(9, 30, 10)),
            [],
            "an edit doesn't fire it again"
        );
        let moved = Schedule {
            at: time(9, 45, 0, 0),
            ..edited.clone()
        };
        schedules.set(vec![moved], Fresh::Reload);
        assert_eq!(schedules.due(now(9, 31, 0)), []);
        assert_eq!(
            schedules.due(now(9, 45, 0)).len(),
            1,
            "a new time fires at it"
        );
        let off = Schedule {
            enabled: false,
            ..edited
        };
        schedules.set(vec![off], Fresh::Reload);
        assert!(
            !schedules.waiting(),
            "a disabled schedule keeps nothing alive"
        );
        assert_eq!(schedules.due(now(9, 45, 30)), []);
    }
}
