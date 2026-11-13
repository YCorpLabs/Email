//! Wall-clock helpers for demo dates. Everything is relative to the moment the
//! demo is seeded, in the viewer's UTC offset, so the inbox always reads
//! "today", "yesterday" and "this week".

pub(crate) const MINUTE: i64 = 60_000;
pub(crate) const HOUR: i64 = 60 * MINUTE;
pub(crate) const DAY: i64 = 24 * HOUR;

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const NORWEGIAN_MONTHS: [&str; 12] = [
    "januar",
    "februar",
    "mars",
    "april",
    "mai",
    "juni",
    "juli",
    "august",
    "september",
    "oktober",
    "november",
    "desember",
];
const NORWEGIAN_WEEKDAYS: [&str; 7] = [
    "mandag", "tirsdag", "onsdag", "torsdag", "fredag", "lørdag", "søndag",
];

/// UTC civil date and time for a Unix millisecond timestamp.
pub(crate) fn civil(ms: i64) -> (i64, u32, u32, u32, u32) {
    let days = ms.div_euclid(DAY);
    let rest = ms.rem_euclid(DAY);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        (rest / HOUR) as u32,
        (rest % HOUR / MINUTE) as u32,
    )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Clock {
    pub now: i64,
    offset: i64,
}

impl Clock {
    pub fn new(now_ms: i64, utc_offset_minutes: i32) -> Self {
        let minutes = i64::from(utc_offset_minutes.clamp(-14 * 60, 14 * 60));
        Self {
            now: now_ms,
            offset: minutes * MINUTE,
        }
    }
    /// UTC instant of the viewer's local midnight today.
    fn midnight(&self) -> i64 {
        let local = self.now + self.offset;
        local - local.rem_euclid(DAY) - self.offset
    }
    /// A moment `minutes` before now. Today's mail uses this, so it is never in the future.
    pub fn ago(&self, minutes: i64) -> i64 {
        self.now - minutes * MINUTE
    }
    /// Local wall time `days` days ago. Never later than a minute before now.
    pub fn day(&self, days: i64, hour: i64, minute: i64) -> i64 {
        (self.midnight() - days * DAY + hour * HOUR + minute * MINUTE).min(self.now - MINUTE)
    }
    /// Local wall time `days` days from today.
    pub fn ahead(&self, days: i64, hour: i64, minute: i64) -> i64 {
        self.midnight() + days * DAY + hour * HOUR + minute * MINUTE
    }
    /// Days from today to the next given weekday (0 = Monday), always 1–7.
    pub fn days_until(&self, weekday: i64) -> i64 {
        let today = self.weekday(self.now);
        let delta = (weekday - today).rem_euclid(7);
        if delta == 0 { 7 } else { delta }
    }
    pub fn weekday(&self, ms: i64) -> i64 {
        ((ms + self.offset).div_euclid(DAY) + 3).rem_euclid(7)
    }
    fn local(&self, ms: i64) -> (i64, u32, u32, u32, u32) {
        civil(ms + self.offset)
    }
    pub fn weekday_name(&self, ms: i64) -> &'static str {
        WEEKDAYS[self.weekday(ms) as usize]
    }
    pub fn norwegian_weekday(&self, ms: i64) -> &'static str {
        NORWEGIAN_WEEKDAYS[self.weekday(ms) as usize]
    }
    pub fn month_name(&self, ms: i64) -> &'static str {
        MONTHS[self.local(ms).1 as usize - 1]
    }
    pub fn norwegian_month(&self, ms: i64) -> &'static str {
        NORWEGIAN_MONTHS[self.local(ms).1 as usize - 1]
    }
    pub fn year(&self, ms: i64) -> i64 {
        self.local(ms).0
    }
    pub fn day_of_month(&self, ms: i64) -> u32 {
        self.local(ms).2
    }
    /// "Tue 18 Nov"
    pub fn short_date(&self, ms: i64) -> String {
        format!(
            "{} {} {}",
            &self.weekday_name(ms)[..3],
            self.day_of_month(ms),
            &self.month_name(ms)[..3]
        )
    }
    /// "Tuesday 18 November"
    pub fn long_date(&self, ms: i64) -> String {
        format!(
            "{} {} {}",
            self.weekday_name(ms),
            self.day_of_month(ms),
            self.month_name(ms)
        )
    }
    /// "18.11.2026"
    pub fn norwegian_date(&self, ms: i64) -> String {
        let (y, m, d, _, _) = self.local(ms);
        format!("{d:02}.{m:02}.{y}")
    }
    /// "2026-11-18"
    pub fn iso_date(&self, ms: i64) -> String {
        let (y, m, d, _, _) = self.local(ms);
        format!("{y}-{m:02}-{d:02}")
    }
    /// "10:00"
    pub fn time(&self, ms: i64) -> String {
        let (_, _, _, h, m) = self.local(ms);
        format!("{h:02}:{m:02}")
    }
    /// "GMT+1", as calendar invitation subjects show the zone.
    pub fn zone(&self) -> String {
        let minutes = self.offset / MINUTE;
        let sign = if minutes < 0 { '-' } else { '+' };
        let (h, m) = (minutes.abs() / 60, minutes.abs() % 60);
        if m == 0 {
            format!("GMT{sign}{h}")
        } else {
            format!("GMT{sign}{h}:{m:02}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip_known_instants() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        // 2026-11-12T09:30:00Z
        assert_eq!(civil(1_794_475_800_000), (2026, 11, 12, 9, 30));
        assert_eq!(civil(951_782_400_000), (2000, 2, 29, 0, 0));
    }

    #[test]
    fn local_days_follow_the_offset() {
        // Thursday 2026-11-12 23:30 UTC is already Friday in Oslo (UTC+1).
        let clock = Clock::new(1_794_526_200_000, 60);
        assert_eq!(clock.weekday_name(clock.now), "Friday");
        assert_eq!(clock.time(clock.ahead(1, 9, 0)), "09:00");
        assert!(clock.day(0, 23, 59) < clock.now);
        assert_eq!(clock.days_until(clock.weekday(clock.now)), 7);
        assert_eq!(clock.zone(), "GMT+1");
    }
}
