//! New York Stock Exchange trading days.
//!
//! The rules cover full-day closures from 2000 onward: the regular holidays with
//! NYSE's weekend observance, plus the unscheduled closures listed in
//! [`SPECIAL_CLOSURES`]. Early closes are not modelled.

use jiff::civil::{Date, Weekday, date};

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

#[cfg(test)]
mod tests {
    use super::*;

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
