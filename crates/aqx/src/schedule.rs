use jiff::Timestamp;
use jiff::civil::{Date, Time, Weekday};
use jiff::tz::TimeZone;

use crate::config::Script;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub at: Time,
    pub days: Days,
    pub tz: Option<TimeZone>,
    pub script: Option<Script>,
    pub relogin: bool,
}

impl Schedule {
    pub fn label(&self) -> String {
        self.script
            .as_ref()
            .map_or_else(|| "relogin".into(), |s| s.path.display().to_string())
    }

    pub fn next_after(&self, after: Timestamp) -> Option<Timestamp> {
        let tz = self.tz.clone().unwrap_or_else(TimeZone::system);
        let mut date = after.to_zoned(tz.clone()).date();
        for _ in 0..8 {
            if self.days.has(date.weekday()) {
                let at = date.to_datetime(self.at).to_zoned(tz.clone()).ok()?;
                if at.timestamp() > after {
                    return Some(at.timestamp());
                }
            }
            date = date.tomorrow().ok()?;
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days(u8);

impl Days {
    pub const EVERY: Days = Days(0x7f);

    pub fn of(days: &[Weekday]) -> Days {
        Days(days.iter().fold(0, |bits, day| bits | bit(*day)))
    }

    fn has(self, day: Weekday) -> bool {
        self.0 & bit(day) != 0
    }
}

fn bit(day: Weekday) -> u8 {
    1 << day.to_monday_zero_offset()
}

pub fn parse_time(text: &str) -> Option<Time> {
    let text = text.trim().to_ascii_lowercase();
    let (clock, half) = match text.strip_suffix("am").or_else(|| text.strip_suffix("pm")) {
        Some(clock) => (clock.trim_end(), Some(text.ends_with("pm"))),
        None => (text.as_str(), None),
    };
    let (hour, minute) = match clock.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => (hour, minute.parse::<i8>().ok()?),
        Some(_) => return None,
        None if half.is_some() => (clock, 0),
        None => return None,
    };
    if hour.is_empty() || hour.len() > 2 || !hour.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour: i8 = hour.parse().ok()?;
    let hour = match half {
        None if hour < 24 => hour,
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        _ => return None,
    };
    Time::new(hour, minute, 0, 0).ok()
}

pub fn parse_day(text: &str) -> Option<Weekday> {
    let text = text.trim().to_ascii_lowercase();
    let days = [
        ("mon", "monday", Weekday::Monday),
        ("tue", "tuesday", Weekday::Tuesday),
        ("wed", "wednesday", Weekday::Wednesday),
        ("thu", "thursday", Weekday::Thursday),
        ("fri", "friday", Weekday::Friday),
        ("sat", "saturday", Weekday::Saturday),
        ("sun", "sunday", Weekday::Sunday),
    ];
    days.iter()
        .find(|(short, long, _)| text == *short || text == *long)
        .map(|(_, _, day)| *day)
}

pub fn when(at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    let at = at.to_zoned(tz.clone());
    let today: Date = now.to_zoned(tz.clone()).date();
    let format = if at.date() == today {
        "%H:%M"
    } else {
        "%a %H:%M"
    };
    at.strftime(format).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn eastern() -> Option<TimeZone> {
        Some(TimeZone::get("America/New_York").unwrap())
    }

    fn schedule(at: &str, days: Days) -> Schedule {
        Schedule {
            at: parse_time(at).unwrap(),
            days,
            tz: eastern(),
            script: Some(Script {
                path: "/srv/dailies.ts".into(),
                args: Vec::new(),
            }),
            relogin: false,
        }
    }

    fn eastern_at(y: i16, m: i8, d: i8, h: i8, min: i8) -> Timestamp {
        date(y, m, d)
            .at(h, min, 0, 0)
            .to_zoned(eastern().unwrap())
            .unwrap()
            .timestamp()
    }

    #[test]
    fn times_read_as_a_24_hour_clock_or_with_am_and_pm() {
        let at = |text: &str| parse_time(text).map(|t| (t.hour(), t.minute()));
        assert_eq!(at("21:00"), Some((21, 0)));
        assert_eq!(at("0:05"), Some((0, 5)));
        assert_eq!(at("9pm"), Some((21, 0)));
        assert_eq!(at("9:30 PM"), Some((21, 30)));
        assert_eq!(at("12am"), Some((0, 0)));
        assert_eq!(at("12pm"), Some((12, 0)));
        for bad in [
            "", "21", "24:00", "9:5", "13pm", "0am", "9:60", "noon", "-1:00", "+9:00",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn days_read_as_short_or_full_names() {
        assert_eq!(parse_day("Fri"), Some(Weekday::Friday));
        assert_eq!(parse_day("thursday"), Some(Weekday::Thursday));
        assert_eq!(parse_day("thurs"), None);
    }

    #[test]
    fn the_next_fire_is_the_first_matching_time_after_now() {
        let daily = schedule("00:00", Days::EVERY);
        assert_eq!(
            daily.next_after(eastern_at(2026, 11, 15, 21, 10)),
            Some(eastern_at(2026, 11, 16, 0, 0))
        );
        assert_eq!(
            daily.next_after(eastern_at(2026, 11, 16, 0, 0)),
            Some(eastern_at(2026, 11, 17, 0, 0)),
            "a schedule doesn't fire twice for the same minute"
        );
        let weekly = schedule("9am", Days::of(&[Weekday::Friday]));
        assert_eq!(
            weekly.next_after(eastern_at(2026, 11, 20, 9, 0)),
            Some(eastern_at(2026, 11, 27, 9, 0))
        );
        assert_eq!(
            weekly.next_after(eastern_at(2026, 11, 16, 12, 0)),
            Some(eastern_at(2026, 11, 20, 9, 0))
        );
        assert_eq!(
            schedule("9am", Days::of(&[])).next_after(Timestamp::UNIX_EPOCH),
            None
        );
    }

    #[test]
    fn a_time_follows_the_zone_across_daylight_saving_changes() {
        let daily = schedule("00:00", Days::EVERY);
        let before = eastern_at(2026, 11, 1, 0, 0);
        let after = daily.next_after(before).unwrap();
        assert_eq!(after, eastern_at(2026, 11, 2, 0, 0));
        assert_eq!(after.duration_since(before).as_hours(), 25);
        let skipped = schedule("2:30", Days::EVERY);
        assert_eq!(
            skipped.next_after(eastern_at(2026, 3, 7, 12, 0)),
            Some(eastern_at(2026, 3, 8, 3, 30)),
            "a time the clocks skip fires an hour later"
        );
    }
}
