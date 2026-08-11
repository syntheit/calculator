//! Calendar date arithmetic — a pure-Rust, dependency-light module.
//!
//! This module provides a small proleptic Gregorian calendar toolkit: a
//! [`Date`] value type plus free functions to validate dates, compute the
//! number of days between two dates, add or subtract days, determine weekdays,
//! and break a span into calendar years/months/days.
//!
//! All day-count arithmetic is built on Howard Hinnant's well-known
//! `days_from_civil` / `civil_from_days` algorithms (see
//! <https://howardhinnant.github.io/date_algorithms.html>), operating on a
//! signed day count relative to the Unix epoch, 1970-01-01. That anchor date
//! has a day count of `0` and was a Thursday.
//!
//! The API is intentionally UI-agnostic: it contains no GTK code and is a
//! plain library module.

// The public API here is exercised by unit tests and is intended for future
// UI wiring; some items are currently unused by the running app.
#![allow(dead_code)]

/// A calendar date in the proleptic Gregorian calendar.
///
/// Fields are public for convenient destructuring, but prefer [`Date::new`] to
/// construct instances so that invalid combinations (e.g. February 30th) are
/// rejected up front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Date {
    /// Proleptic Gregorian year (may be negative; 1 BC is year 0).
    pub year: i32,
    /// Month of the year, in `1..=12`.
    pub month: u32,
    /// Day of the month, in `1..=days_in_month(year, month)`.
    pub day: u32,
}

impl Date {
    /// Construct a validated [`Date`].
    ///
    /// Returns [`DateError::Invalid`] unless `month` is in `1..=12` and `day`
    /// is in `1..=days_in_month(year, month)` (which correctly accounts for
    /// leap years in February).
    pub fn new(year: i32, month: u32, day: u32) -> Result<Date, DateError> {
        if (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month) {
            Ok(Date { year, month, day })
        } else {
            Err(DateError::Invalid)
        }
    }

    /// The day of the week on which this date falls.
    ///
    /// Delegates to the free [`weekday`] function.
    pub fn weekday(self) -> Weekday {
        weekday(self)
    }
}

/// Errors that can arise when constructing or validating a [`Date`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    /// The year/month/day combination is not a real calendar date.
    #[error("Invalid date")]
    Invalid,
    /// A computed date fell outside the representable [`Date`] range (its year
    /// does not fit in `i32`, or the day counter overflowed `i64`).
    #[error("Date out of range")]
    OutOfRange,
}

/// A day of the week, in Monday-first order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl Weekday {
    /// The English name of this weekday, e.g. `"Monday"`.
    pub fn name(self) -> &'static str {
        match self {
            Weekday::Monday => "Monday",
            Weekday::Tuesday => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday => "Thursday",
            Weekday::Friday => "Friday",
            Weekday::Saturday => "Saturday",
            Weekday::Sunday => "Sunday",
        }
    }
}

/// A calendar difference broken into years, months, and days.
///
/// Produced by [`diff_ymd`]. All components share the same sign convention as
/// documented on [`diff_ymd`] (they are the non-negative magnitude of the
/// span).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateDiff {
    /// Whole calendar years in the span.
    pub years: i64,
    /// Whole calendar months beyond the years.
    pub months: i64,
    /// Whole days beyond the years and months.
    pub days: i64,
}

/// Returns `true` iff `year`/`month`/`day` is a real calendar date.
///
/// Delegates to [`Date::new`].
pub fn is_valid(year: i32, month: u32, day: u32) -> bool {
    Date::new(year, month, day).is_ok()
}

/// Returns `true` iff `year` is a Gregorian leap year.
///
/// The rule: divisible by 4, and (not divisible by 100, or divisible by 400).
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in `month` of `year`.
///
/// February accounts for leap years. An out-of-range `month` (0 or > 12)
/// returns `0`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    days_in_month_i64(year as i64, month)
}

/// The number of days in `month` of an `i64` `year`.
///
/// Same rule as [`days_in_month`] but on a widened year, so a borrowed year
/// (e.g. `hi.year - 1` in [`diff_ymd`]) can be computed without an `i32`
/// underflow. The leap test is `year % 4 == 0 && (year % 100 != 0 || year %
/// 400 == 0)`.
fn days_in_month_i64(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Signed difference in days, `b - a`.
///
/// Computed as `days_from_civil(b) - days_from_civil(a)`, so the result is
/// positive when `b` is later than `a` and negative when it is earlier.
pub fn days_between(a: Date, b: Date) -> i64 {
    days_from_civil(b) - days_from_civil(a)
}

/// Return `d` shifted by `n` days (negative `n` moves backwards).
///
/// Returns [`DateError::OutOfRange`] when the shift lands outside the
/// representable [`Date`] range (its year would not fit in `i32`) or when the
/// day counter overflows `i64`.
pub fn add_days(d: Date, n: i64) -> Result<Date, DateError> {
    let days = days_from_civil(d)
        .checked_add(n)
        .ok_or(DateError::OutOfRange)?;
    civil_from_days_checked(days)
}

/// The day of the week on which `d` falls.
///
/// Anchored at the Unix epoch: 1970-01-01 (day count `0`) was a **Thursday**.
/// Taking `days_from_civil(d).rem_euclid(7)` therefore yields
/// `0 => Thursday, 1 => Friday, 2 => Saturday, 3 => Sunday, 4 => Monday,
/// 5 => Tuesday, 6 => Wednesday`, which this function maps to [`Weekday`].
pub fn weekday(d: Date) -> Weekday {
    let dow = days_from_civil(d).rem_euclid(7);
    match dow {
        0 => Weekday::Thursday,
        1 => Weekday::Friday,
        2 => Weekday::Saturday,
        3 => Weekday::Sunday,
        4 => Weekday::Monday,
        5 => Weekday::Tuesday,
        // dow == 6
        _ => Weekday::Wednesday,
    }
}

/// The calendar difference between `a` and `b`, as years/months/days.
///
/// The returned [`DateDiff`] is the **magnitude** of the span: all components
/// are non-negative regardless of the argument order. Internally the pair is
/// ordered so that `lo <= hi` (compared by day count via [`days_from_civil`]),
/// then the difference `hi - lo` is decomposed by borrowing the length of the
/// month preceding `hi`'s month (clamping the low day to that length) and
/// borrowing from the year as needed, so every component is non-negative.
pub fn diff_ymd(a: Date, b: Date) -> DateDiff {
    let (lo, hi) = if days_from_civil(a) <= days_from_civil(b) {
        (a, b)
    } else {
        (b, a)
    };

    let mut years = (hi.year as i64) - (lo.year as i64);
    let mut months = (hi.month as i64) - (lo.month as i64);
    let days;
    if hi.day >= lo.day {
        days = (hi.day - lo.day) as i64;
    } else {
        // The day-of-month went backwards, so borrow a whole month. The days
        // available are the length of the month preceding hi's month; the low
        // day is clamped to that length (a day number beyond the borrowed
        // month — e.g. the 31st borrowing from a 30-day month — counts as that
        // month's last day). This guarantees a non-negative `days` result.
        months -= 1;
        let (by, bm) = if hi.month == 1 {
            ((hi.year as i64) - 1, 12u32)
        } else {
            (hi.year as i64, hi.month - 1)
        };
        let borrow = days_in_month_i64(by, bm) as i64;
        let eff_lo_day = (lo.day as i64).min(borrow);
        days = borrow - eff_lo_day + hi.day as i64;
    }
    if months < 0 {
        years -= 1;
        months += 12;
    }
    DateDiff {
        years,
        months,
        days,
    }
}

/// Days since the Unix epoch (1970-01-01) for `d`.
///
/// Howard Hinnant's `days_from_civil`, operating on `i64` day counts.
fn days_from_civil(d: Date) -> i64 {
    let y = if d.month <= 2 {
        d.year as i64 - 1
    } else {
        d.year as i64
    };
    let m = d.month as i64;
    let day = d.day as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The `(year, month, day)` of the civil date `z` days after the Unix epoch,
/// as `i64` components (year may exceed `i32`).
///
/// Howard Hinnant's `civil_from_days` core. The first line adds `719_468` back
/// to `z` because [`days_from_civil`] subtracts it at the end.
fn civil_ymd_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// The [`Date`] `z` days after the Unix epoch (1970-01-01).
///
/// For internal callers whose input is known in range (its year fits in
/// `i32`); the final year cast is unchecked. Use [`civil_from_days_checked`]
/// for arbitrary day counts.
fn civil_from_days(z: i64) -> Date {
    let (year, month, day) = civil_ymd_from_days(z);
    Date {
        year: year as i32,
        month,
        day,
    }
}

/// Like [`civil_from_days`], but returns [`DateError::OutOfRange`] when the
/// resulting year does not fit in `i32` instead of silently truncating.
fn civil_from_days_checked(z: i64) -> Result<Date, DateError> {
    let (year, month, day) = civil_ymd_from_days(z);
    let year = i32::try_from(year).map_err(|_| DateError::OutOfRange)?;
    Ok(Date { year, month, day })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: build a `Date`, panicking on invalid input.
    fn d(y: i32, m: u32, day: u32) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn leap_years() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2026));
        assert!(!is_leap_year(2100));
        assert!(is_leap_year(2400));
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 1), 31);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 0), 0);
        assert_eq!(days_in_month(2026, 13), 0);
    }

    #[test]
    fn between() {
        assert_eq!(days_between(d(2020, 1, 1), d(2020, 12, 31)), 365);
        assert_eq!(days_between(d(2026, 1, 1), d(2027, 1, 1)), 365);
        let same = d(2026, 8, 11);
        assert_eq!(days_between(same, same), 0);
        assert_eq!(days_between(d(2026, 8, 11), d(2026, 8, 1)), -10);
        assert_eq!(days_between(d(1999, 12, 31), d(2000, 1, 1)), 1);
    }

    #[test]
    fn adding_days() {
        assert_eq!(add_days(d(2026, 8, 11), 30).unwrap(), d(2026, 9, 10));
        assert_eq!(add_days(d(2026, 1, 31), 1).unwrap(), d(2026, 2, 1));
        assert_eq!(add_days(d(2020, 2, 28), 1).unwrap(), d(2020, 2, 29));
        assert_eq!(add_days(d(2026, 3, 1), -1).unwrap(), d(2026, 2, 28));
        assert_eq!(add_days(d(2026, 12, 31), 1).unwrap(), d(2027, 1, 1));
    }

    #[test]
    fn weekdays() {
        assert_eq!(weekday(d(2026, 8, 11)), Weekday::Tuesday);
        assert_eq!(weekday(d(2000, 1, 1)), Weekday::Saturday);
        assert_eq!(weekday(d(2024, 1, 1)), Weekday::Monday);
        assert_eq!(weekday(d(2020, 2, 29)), Weekday::Saturday);
        // Method form delegates to the free function.
        assert_eq!(d(2026, 8, 11).weekday(), Weekday::Tuesday);
    }

    #[test]
    fn weekday_names() {
        assert_eq!(Weekday::Monday.name(), "Monday");
        assert_eq!(Weekday::Sunday.name(), "Sunday");
    }

    #[test]
    fn new_rejects_invalid() {
        assert_eq!(Date::new(2026, 2, 30), Err(DateError::Invalid));
        assert!(Date::new(2026, 13, 1).is_err());
        assert!(Date::new(2026, 0, 1).is_err());
        assert!(Date::new(2026, 4, 31).is_err());
        assert!(Date::new(2026, 1, 0).is_err());
        // Valid cases.
        assert!(Date::new(2024, 2, 29).is_ok());
        assert!(Date::new(2026, 1, 31).is_ok());
    }

    #[test]
    fn validity() {
        assert!(is_valid(2024, 2, 29));
        assert!(is_valid(2026, 12, 31));
        assert!(is_valid(2000, 1, 1));
        assert!(!is_valid(2026, 2, 29));
        assert!(!is_valid(2026, 4, 31));
        assert!(!is_valid(2026, 13, 1));
        assert!(!is_valid(2026, 0, 1));
    }

    #[test]
    fn round_trip() {
        for date in [
            d(2026, 8, 11),
            d(2000, 1, 1),
            d(1970, 1, 1),
            d(1900, 12, 31),
            d(2400, 2, 29),
        ] {
            assert_eq!(add_days(add_days(date, 100).unwrap(), -100).unwrap(), date);
        }
    }

    #[test]
    fn add_days_out_of_range() {
        // Overflowing the day counter in either direction errors, never panics.
        assert_eq!(add_days(d(2020, 1, 1), i64::MAX), Err(DateError::OutOfRange));
        assert_eq!(add_days(d(2020, 1, 1), i64::MIN), Err(DateError::OutOfRange));
        // A normal in-range shift still succeeds.
        assert_eq!(add_days(d(2020, 1, 1), 366).unwrap(), d(2021, 1, 1));
    }

    #[test]
    fn diff_ymd_extreme_years_no_panic() {
        // Extreme but valid dates must not overflow the year subtraction (this
        // test runs with debug overflow checks).
        let lo = d(i32::MIN, 12, 31);
        let hi = d(i32::MAX, 1, 1);
        let _ = days_between(lo, hi);
        let diff = diff_ymd(lo, hi);
        assert!(diff.years > 0);
    }

    #[test]
    fn ymd_difference() {
        assert_eq!(
            diff_ymd(d(2020, 1, 15), d(2023, 3, 20)),
            DateDiff {
                years: 3,
                months: 2,
                days: 5
            }
        );

        // Borrow case: 2020-01-31 -> 2020-03-01. Jan 31 borrows the month
        // preceding March = Feb 2020 (leap, 29 days); the low day 31 clamps to
        // 29, so days = 29 - 29 + 1 = 1. Result { years: 0, months: 1, days: 1 }.
        assert_eq!(
            diff_ymd(d(2020, 1, 31), d(2020, 3, 1)),
            DateDiff {
                years: 0,
                months: 1,
                days: 1
            }
        );

        // Identity: a date against itself is zero.
        let a = d(2026, 8, 11);
        assert_eq!(
            diff_ymd(a, a),
            DateDiff {
                years: 0,
                months: 0,
                days: 0
            }
        );
    }
}
