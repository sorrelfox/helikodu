// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar dates and ages as times in seconds since the Unix epoch.
//!
//! Dates are in the local time zone, given as one offset from UTC: around a change to
//! or from summer time a boundary can be off by an hour.

use helikodu_model::query::{Age, AgeUnit, CalendarDate};

use super::QueryContext;

const DAY: i64 = 86_400;

/// The days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a number of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let next = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    next - days_from_civil(year, month, 1)
}

/// Local midnight at the start of a date, in seconds since the epoch.
fn local_midnight(context: &QueryContext, year: i64, month: i64, day: i64) -> i64 {
    days_from_civil(year, month, day) * DAY - context.utc_offset
}

/// The times from the start (inclusive) to the end (exclusive) of `date`'s year,
/// month or day.
pub(super) fn period(context: &QueryContext, date: CalendarDate) -> (i64, i64) {
    let year = i64::from(date.year_number());
    match (date.month_number(), date.day_number()) {
        (Some(month), Some(day)) => {
            let start = local_midnight(context, year, month.into(), day.into());
            (start, start + DAY)
        }
        (Some(month), None) => {
            let month = i64::from(month);
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            (
                local_midnight(context, year, month, 1),
                local_midnight(context, next_year, next_month, 1),
            )
        }
        _ => (
            local_midnight(context, year, 1, 1),
            local_midnight(context, year + 1, 1, 1),
        ),
    }
}

/// The time `age` before now. Months and years are calendar months and years in
/// local time; a day past the end of the month is the month's last day.
pub(super) fn age_bound(context: &QueryContext, age: Age) -> i64 {
    let amount = i64::from(age.amount);
    let seconds = match age.unit {
        AgeUnit::Hours => amount * 3600,
        AgeUnit::Days => amount * DAY,
        AgeUnit::Weeks => amount * 7 * DAY,
        AgeUnit::Months | AgeUnit::Years => {
            let months = if age.unit == AgeUnit::Years {
                amount * 12
            } else {
                amount
            };
            let local = context.now + context.utc_offset;
            let (days, time_of_day) = (local.div_euclid(DAY), local.rem_euclid(DAY));
            let (year, month, day) = civil_from_days(days);
            let total = year * 12 + (month - 1) - months;
            let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
            let day = day.min(days_in_month(year, month));
            let then = days_from_civil(year, month, day) * DAY + time_of_day - context.utc_offset;
            return then;
        }
    };
    context.now - seconds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(now: i64, utc_offset: i64) -> QueryContext {
        QueryContext {
            now,
            utc_offset,
            seed: 0,
        }
    }

    #[test]
    fn civil_dates_convert_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for days in [-800_000, -1, 0, 1, 11_016, 19_000, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2024, 12), 31);
    }

    #[test]
    fn periods_start_at_local_midnight() {
        // 2024-05-01 00:00 UTC.
        let may_first = 1_714_521_600;
        let utc = context(0, 0);
        let day = CalendarDate::day(2024, 5, 1).unwrap();
        assert_eq!(period(&utc, day), (may_first, may_first + DAY));
        let tallinn = context(0, 3 * 3600);
        assert_eq!(period(&tallinn, day).0, may_first - 3 * 3600);
        let month = CalendarDate::month(2024, 12).unwrap();
        let (start, end) = period(&utc, month);
        assert_eq!(end - start, 31 * DAY);
        let year = CalendarDate::year(2024).unwrap();
        let (start, end) = period(&utc, year);
        assert_eq!(end - start, 366 * DAY);
    }

    #[test]
    fn ages_count_back_from_now() {
        // 2024-03-31 12:00 UTC.
        let now = days_from_civil(2024, 3, 31) * DAY + 12 * 3600;
        let utc = context(now, 0);
        let age = |amount, unit| age_bound(&utc, Age { amount, unit });
        assert_eq!(age(30, AgeUnit::Days), now - 30 * DAY);
        assert_eq!(age(2, AgeUnit::Weeks), now - 14 * DAY);
        assert_eq!(age(5, AgeUnit::Hours), now - 5 * 3600);
        // A month before 31 March is the last day of February.
        assert_eq!(
            age(1, AgeUnit::Months),
            days_from_civil(2024, 2, 29) * DAY + 12 * 3600
        );
        assert_eq!(
            age(1, AgeUnit::Years),
            days_from_civil(2023, 3, 31) * DAY + 12 * 3600
        );
        assert_eq!(
            age(15, AgeUnit::Months),
            days_from_civil(2022, 12, 31) * DAY + 12 * 3600
        );
    }
}
