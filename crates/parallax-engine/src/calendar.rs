//! New York Stock Exchange trading days, and trading time for chart axes.
//!
//! The rules cover full-day closures from 2000 onward: the regular holidays with
//! NYSE's weekend observance, plus the unscheduled closures listed in
//! [`SPECIAL_CLOSURES`]. Early closes are not modelled.
//!
//! [`TradingCalendar`] places dates and instants on a trading-time axis, where
//! closed days and the hours outside the regular session take no space, so a
//! chart of trading days draws them evenly.

use std::sync::OnceLock;

use jiff::Timestamp;
use jiff::civil::{Date, Time, Weekday, date, time};
use jiff::tz::TimeZone;

/// Full-day closures that no holiday rule produces.
pub const SPECIAL_CLOSURES: [Date; 10] = [
    date(2001, 9, 11), // September 11 attacks
    date(2001, 9, 12),
    date(2001, 9, 13),
    date(2001, 9, 14),
    date(2004, 6, 11),  // President Reagan's funeral
    date(2007, 1, 2),   // President Ford's funeral
    date(2012, 10, 29), // Hurricane Sandy
    date(2012, 10, 30),
    date(2018, 12, 5), // President George H. W. Bush's funeral
    date(2025, 1, 9),  // President Carter's funeral
];

/// The Unix epoch as a civil date.
pub const EPOCH: Date = date(1970, 1, 1);

/// Returns the number of days from the Unix epoch, the representation of Arrow `Date32`.
///
/// # Panics
///
/// Panics only for dates outside jiff's supported range, which cannot be constructed.
#[must_use]
pub fn epoch_days(day: Date) -> i32 {
    day.since(EPOCH)
        .expect("span between civil dates")
        .get_days()
}

/// Returns the civil date `days` after the Unix epoch.
///
/// # Panics
///
/// Panics if the result is outside jiff's supported range.
#[must_use]
pub fn from_epoch_days(days: i32) -> Date {
    EPOCH
        .checked_add(jiff::Span::new().days(i64::from(days)))
        .expect("date within jiff's range")
}

/// Returns Easter Sunday for `year` (anonymous Gregorian algorithm).
#[must_use]
pub fn easter_sunday(year: i16) -> Date {
    let y = i32::from(year);
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    date(year, month as i8, day as i8)
}

fn nth_weekday(year: i16, month: i8, nth: i8, weekday: Weekday) -> Date {
    date(year, month, 1)
        .nth_weekday_of_month(nth, weekday)
        .expect("weekday exists in month")
}

/// Moves a fixed-date holiday that falls on a weekend to the Friday before or the
/// Monday after, as NYSE observes it.
fn observed(day: Date) -> Date {
    match day.weekday() {
        Weekday::Saturday => day.yesterday().expect("in range"),
        Weekday::Sunday => day.tomorrow().expect("in range"),
        _ => day,
    }
}

/// Returns the NYSE holidays for `year`, sorted, excluding special closures.
#[must_use]
pub fn holidays(year: i16) -> Vec<Date> {
    let mut days = Vec::with_capacity(10);
    // New Year's Day moves to Monday when it falls on Sunday. When it falls on
    // Saturday NYSE does not close the Friday before, because that Friday ends the
    // previous year's books.
    let new_year = date(year, 1, 1);
    match new_year.weekday() {
        Weekday::Saturday => {}
        Weekday::Sunday => days.push(date(year, 1, 2)),
        _ => days.push(new_year),
    }
    days.push(nth_weekday(year, 1, 3, Weekday::Monday)); // Martin Luther King Jr. Day
    days.push(nth_weekday(year, 2, 3, Weekday::Monday)); // Washington's Birthday
    days.push(
        easter_sunday(year)
            .checked_sub(jiff::Span::new().days(2))
            .expect("in range"),
    ); // Good Friday
    days.push(nth_weekday(year, 5, -1, Weekday::Monday)); // Memorial Day
    if year >= 2022 {
        days.push(observed(date(year, 6, 19))); // Juneteenth
    }
    days.push(observed(date(year, 7, 4))); // Independence Day
    days.push(nth_weekday(year, 9, 1, Weekday::Monday)); // Labor Day
    days.push(nth_weekday(year, 11, 4, Weekday::Thursday)); // Thanksgiving
    days.push(observed(date(year, 12, 25))); // Christmas
    days.sort();
    days
}

/// Returns `true` if NYSE is open for the full or a partial day on `day`.
#[must_use]
pub fn is_trading_day(day: Date) -> bool {
    !matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday)
        && !SPECIAL_CLOSURES.contains(&day)
        && !holidays(day.year()).contains(&day)
}

/// Returns the first trading day after `day`.
///
/// # Panics
///
/// Panics if no trading day exists before the end of jiff's date range.
#[must_use]
pub fn next_trading_day(day: Date) -> Date {
    let mut next = day.tomorrow().expect("in range");
    while !is_trading_day(next) {
        next = next.tomorrow().expect("in range");
    }
    next
}

/// Returns the trading days in `start..=end`, in order.
#[must_use]
pub fn trading_days(start: Date, end: Date) -> Vec<Date> {
    let mut days = Vec::new();
    let mut day = start;
    while day <= end {
        if is_trading_day(day) {
            days.push(day);
        }
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    days
}

/// The years whose closures the trading-time axis knows: the holiday rules hold
/// from 2000. Outside them, only weekends are closed.
const CLOSURE_YEARS: std::ops::RangeInclusive<i16> = 2000..=2100;

/// NYSE's full-day closures on weekdays, as days since the epoch, sorted.
fn weekday_closures() -> &'static [i32] {
    static CLOSURES: OnceLock<Vec<i32>> = OnceLock::new();
    CLOSURES.get_or_init(|| {
        let mut days: Vec<i32> = CLOSURE_YEARS
            .flat_map(holidays)
            .chain(SPECIAL_CLOSURES)
            .filter(|d| !matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday))
            .map(epoch_days)
            .collect();
        days.sort_unstable();
        days.dedup();
        days
    })
}

/// Weekdays in `0..days`, counted from the epoch, or minus the weekdays in
/// `days..0` for days before it.
fn weekdays_before(days: i32) -> i64 {
    // The epoch was a Thursday: Monday is day -3. Count from that Monday.
    let from_monday = i64::from(days) + 3;
    let weeks = from_monday.div_euclid(7);
    let rest = from_monday.rem_euclid(7);
    5 * weeks + rest.min(5) - 3
}

/// A market's trading days and regular session, for placing dates and instants
/// on a trading-time axis.
#[derive(Clone, Debug)]
pub struct TradingCalendar {
    zone: TimeZone,
    open: Time,
    close: Time,
}

impl TradingCalendar {
    /// NYSE: weekdays except its closures, 09:30 to 16:00 New York time.
    ///
    /// # Panics
    ///
    /// Panics if the bundled time-zone database lacks New York, which it does not.
    #[must_use]
    pub fn nyse() -> Self {
        Self {
            zone: TimeZone::get("America/New_York").expect("bundled zone"),
            open: time(9, 30, 0, 0),
            close: time(16, 0, 0, 0),
        }
    }

    /// The length of the regular session in microseconds.
    #[must_use]
    pub fn session_micros(&self) -> i64 {
        i64::try_from(self.open.duration_until(self.close).as_micros()).unwrap_or(0)
    }

    /// Trading days from the epoch to `days`, not counting `days`: consecutive
    /// trading days are one apart, and a closed day has the number of the next
    /// trading day.
    #[must_use]
    pub fn day_index(&self, days: i32) -> i64 {
        let closures = weekday_closures();
        // Every closure the calendar knows comes after the epoch.
        let closed = closures.partition_point(|&c| c < days) as i64;
        weekdays_before(days) - closed
    }

    /// Whether the market opens on `days`.
    #[must_use]
    pub fn is_open(&self, days: i32) -> bool {
        self.day_index(days + 1) > self.day_index(days)
    }

    /// The trading day numbered `index` by [`TradingCalendar::day_index`].
    #[must_use]
    pub fn day_at(&self, index: i64) -> i32 {
        // The trading day is the last day whose number is at most `index`.
        let (mut low, mut high) = (-1_000_000i32, 1_000_000i32);
        while low < high {
            let mid = low + (high - low + 1) / 2;
            if self.day_index(mid) <= index {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        low
    }

    /// An instant's place in trading time, in microseconds of open market since
    /// the epoch. Instants outside the session take its nearest edge, so the
    /// close of one day and the open of the next share a place.
    ///
    /// # Panics
    ///
    /// Panics only for instants outside jiff's range, which a table cannot hold.
    #[must_use]
    pub fn instant_position(&self, micros: i64) -> f64 {
        let instant = Timestamp::from_microsecond(micros).expect("instant in range");
        let local = instant.to_zoned(self.zone.clone()).datetime();
        let day = epoch_days(local.date());
        let session = self.session_micros();
        let start = self.day_index(day) * session;
        let offset = if self.is_open(day) {
            let since_open = self.open.duration_until(local.time()).as_micros();
            i64::try_from(since_open).unwrap_or(0).clamp(0, session)
        } else {
            0
        };
        (start + offset) as f64
    }

    /// The instant at a place in trading time, in microseconds since the epoch.
    ///
    /// # Panics
    ///
    /// Panics only for places outside jiff's range.
    #[must_use]
    pub fn instant_at(&self, position: f64) -> i64 {
        let session = self.session_micros();
        let at = position.round() as i64;
        let index = at.div_euclid(session);
        let offset = at.rem_euclid(session);
        let day = from_epoch_days(self.day_at(index));
        let open = day
            .to_datetime(self.open)
            .to_zoned(self.zone.clone())
            .expect("session open exists in its zone");
        open.timestamp().as_microsecond() + offset
    }

    /// A date's place in trading time, in trading days since the epoch.
    #[must_use]
    pub fn date_position(&self, days: i32) -> f64 {
        self.day_index(days) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trading_day_numbers_agree_with_the_holiday_rules() {
        let calendar = TradingCalendar::nyse();
        let days = trading_days(date(2024, 1, 1), date(2026, 12, 31));
        for pair in days.windows(2) {
            let (a, b) = (epoch_days(pair[0]), epoch_days(pair[1]));
            assert_eq!(
                calendar.day_index(b),
                calendar.day_index(a) + 1,
                "{}",
                pair[1]
            );
            assert_eq!(calendar.day_at(calendar.day_index(a)), a);
            // Every closed day between them has the later day's number.
            for closed in a + 1..b {
                assert!(!calendar.is_open(closed));
                assert_eq!(calendar.day_index(closed), calendar.day_index(b));
            }
        }
        for day in epoch_days(date(2000, 1, 1))..epoch_days(date(2030, 1, 1)) {
            assert_eq!(
                calendar.is_open(day),
                is_trading_day(from_epoch_days(day)),
                "{}",
                from_epoch_days(day)
            );
        }
    }

    #[test]
    fn before_2000_only_weekends_close() {
        let calendar = TradingCalendar::nyse();
        // Martin Luther King Jr. Day 1990 was a trading day.
        assert!(calendar.is_open(epoch_days(date(1990, 1, 15))));
        assert!(!calendar.is_open(epoch_days(date(1990, 1, 13))));
    }

    #[test]
    fn days_before_the_epoch_count_backwards() {
        let calendar = TradingCalendar::nyse();
        assert_eq!(calendar.day_index(0), 0);
        // 1969-12-31 was a Wednesday, 1969-12-29 a Monday.
        assert_eq!(calendar.day_index(-1), -1);
        assert_eq!(calendar.day_index(-3), -3);
        assert_eq!(calendar.day_index(-4), -3, "Sunday takes Monday's number");
    }

    #[test]
    fn instants_take_their_place_in_the_session() {
        let calendar = TradingCalendar::nyse();
        let at = |s: &str| {
            s.parse::<jiff::Zoned>()
                .unwrap()
                .timestamp()
                .as_microsecond()
        };
        let session = calendar.session_micros();
        assert_eq!(session, 23_400_000_000);
        let monday_open = calendar.instant_position(at("2026-11-02T09:30:00[America/New_York]"));
        assert_eq!(
            calendar.instant_position(at("2026-11-02T10:30:00[America/New_York]")),
            monday_open + 3_600_000_000.0
        );
        // Before the open and after the close take the session's edges.
        assert_eq!(
            calendar.instant_position(at("2026-11-02T07:00:00[America/New_York]")),
            monday_open
        );
        let close = calendar.instant_position(at("2026-11-02T16:00:00[America/New_York]"));
        assert_eq!(
            calendar.instant_position(at("2026-11-02T20:00:00[America/New_York]")),
            close
        );
        assert_eq!(
            close,
            calendar.instant_position(at("2026-11-03T09:30:00[America/New_York]"))
        );
        // The weekend takes no time: Friday's close is Monday's open.
        let friday_close = calendar.instant_position(at("2026-10-30T16:00:00[America/New_York]"));
        assert_eq!(friday_close, monday_open);
        assert_eq!(
            calendar.instant_at(monday_open + 3_600_000_000.0),
            at("2026-11-02T10:30:00[America/New_York]")
        );
    }

    // Published by NYSE: https://www.nyse.com/markets/hours-calendars
    #[test]
    fn holidays_2025_match_nyse() {
        let mut expected = vec![
            date(2025, 1, 1),
            date(2025, 1, 20),
            date(2025, 2, 17),
            date(2025, 4, 18),
            date(2025, 5, 26),
            date(2025, 6, 19),
            date(2025, 7, 4),
            date(2025, 9, 1),
            date(2025, 11, 27),
            date(2025, 12, 25),
        ];
        assert_eq!(holidays(2025), expected);
        expected.push(date(2025, 1, 9));
        expected.sort();
        let closed: Vec<Date> = expected
            .iter()
            .copied()
            .filter(|d| !is_trading_day(*d))
            .collect();
        assert_eq!(closed, expected);
    }

    #[test]
    fn holidays_2026_observe_independence_day_on_friday() {
        assert_eq!(
            holidays(2026),
            vec![
                date(2026, 1, 1),
                date(2026, 1, 19),
                date(2026, 2, 16),
                date(2026, 4, 3),
                date(2026, 5, 25),
                date(2026, 6, 19),
                date(2026, 7, 3),
                date(2026, 9, 7),
                date(2026, 11, 26),
                date(2026, 12, 25),
            ]
        );
    }

    #[test]
    fn saturday_new_year_is_not_moved() {
        // 2022-01-01 was a Saturday and NYSE opened on Friday 2021-12-31.
        assert!(is_trading_day(date(2021, 12, 31)));
        assert!(!holidays(2022).contains(&date(2021, 12, 31)));
    }

    #[test]
    fn counts_trading_days_per_year() {
        assert_eq!(
            trading_days(date(2025, 1, 1), date(2025, 12, 31)).len(),
            250
        );
        assert_eq!(
            trading_days(date(2026, 1, 1), date(2026, 12, 31)).len(),
            251
        );
    }

    #[test]
    fn next_trading_day_skips_weekends_and_holidays() {
        assert_eq!(next_trading_day(date(2026, 7, 2)), date(2026, 7, 6));
        assert_eq!(next_trading_day(date(2026, 10, 30)), date(2026, 11, 2));
    }

    #[test]
    fn epoch_days_round_trip() {
        assert_eq!(epoch_days(EPOCH), 0);
        assert_eq!(epoch_days(date(2026, 11, 2)), 20_759);
        assert_eq!(from_epoch_days(20_759), date(2026, 11, 2));
    }

    #[test]
    fn easter_dates() {
        assert_eq!(easter_sunday(2024), date(2024, 3, 31));
        assert_eq!(easter_sunday(2025), date(2025, 4, 20));
        assert_eq!(easter_sunday(2026), date(2026, 4, 5));
    }
}
