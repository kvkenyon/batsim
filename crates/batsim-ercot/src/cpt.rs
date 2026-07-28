//! Central Prevailing Time (CPT) handling for ERCOT reports (spec D.3.3).
//!
//! ERCOT publishes in CPT (`America/Chicago`) using hour-ENDING conventions
//! and a 25-hour day on fall-back. Conversion is implemented directly from
//! the post-2007 US DST rules (2nd Sunday of March 02:00 local springs
//! forward; 1st Sunday of November 02:00 local falls back) so no tz database
//! is needed and results are identical on every platform.
//!
//! Repeated-hour convention: on the fall-back day the 01:00-02:00 local hour
//! occurs twice; ERCOT flags the SECOND occurrence `repeated_hour = true`
//! (column `Repeated Hour Flag` = "Y"). The first occurrence is CDT (UTC-5),
//! the repeat is CST (UTC-6).

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};

use crate::error::{ErcotError, Result};

/// UTC offset during Central Daylight Time.
pub const CDT: UtcOffset = time::macros::offset!(-5);
/// UTC offset during Central Standard Time.
pub const CST: UtcOffset = time::macros::offset!(-6);

/// Nth weekday of a month (e.g. 2nd Sunday of March).
fn nth_weekday(year: i32, month: Month, weekday: Weekday, n: u8) -> Date {
    let first = Date::from_calendar_date(year, month, 1).expect("valid date");
    let shift = (7 + weekday as u8 - first.weekday() as u8) % 7;
    #[allow(clippy::cast_possible_wrap)]
    let day = 1 + shift + 7 * (n - 1);
    Date::from_calendar_date(year, month, day).expect("valid date")
}

/// UTC instant of the spring-forward transition (02:00 CST -> 03:00 CDT).
fn spring_forward_utc(year: i32) -> OffsetDateTime {
    // 02:00 local CST = 08:00 UTC.
    nth_weekday(year, Month::March, Weekday::Sunday, 2)
        .with_time(Time::from_hms(8, 0, 0).expect("valid time"))
        .assume_utc()
}

/// UTC instant of the fall-back transition (02:00 CDT -> 01:00 CST).
fn fall_back_utc(year: i32) -> OffsetDateTime {
    // 02:00 local CDT = 07:00 UTC.
    nth_weekday(year, Month::November, Weekday::Sunday, 1)
        .with_time(Time::from_hms(7, 0, 0).expect("valid time"))
        .assume_utc()
}

/// CPT offset in force at a UTC instant.
#[must_use]
pub fn offset_at_utc(ts: OffsetDateTime) -> UtcOffset {
    let year = ts.year();
    if ts >= spring_forward_utc(year) && ts < fall_back_utc(year) {
        CDT
    } else {
        CST
    }
}

/// Convert a UTC instant to CPT civil date/time.
#[must_use]
pub fn utc_to_cpt(ts: OffsetDateTime) -> PrimitiveDateTime {
    ts.to_offset(offset_at_utc(ts)).date().with_time(ts.to_offset(offset_at_utc(ts)).time())
}

/// CPT operating day (civil date) containing a UTC instant.
#[must_use]
pub fn operating_day(ts: OffsetDateTime) -> Date {
    utc_to_cpt(ts).date()
}

/// Convert an ERCOT report row to the UTC interval start.
///
/// - `date`: CPT delivery date.
/// - `hour_ending`: 1-24 (ERCOT hour-ending convention).
/// - `interval_index`: 1-based index of the sub-hourly interval
///   (1 for hourly data).
/// - `intervals_per_hour`: 1 (hourly), 4 (15-min), or 12 (5-min).
/// - `repeated_hour`: ERCOT `Repeated Hour Flag` ("Y"), true only for the
///   second occurrence of the ambiguous fall-back hour.
///
/// # Errors
/// Returns `ErcotError::Time` for out-of-range inputs, non-divisible
/// cadences, or an interval that falls in the spring-forward gap.
pub fn cpt_interval_to_utc(
    date: Date,
    hour_ending: u8,
    interval_index: u8,
    intervals_per_hour: u8,
    repeated_hour: bool,
) -> Result<OffsetDateTime> {
    if !(1..=24).contains(&hour_ending) {
        return Err(ErcotError::Time(format!("hour_ending {hour_ending} out of range")));
    }
    if intervals_per_hour == 0 || 60 % intervals_per_hour != 0 {
        return Err(ErcotError::Time(format!(
            "intervals_per_hour {intervals_per_hour} does not divide 60"
        )));
    }
    if interval_index == 0 || interval_index > intervals_per_hour {
        return Err(ErcotError::Time(format!(
            "interval_index {interval_index} out of range for {intervals_per_hour}/hour"
        )));
    }
    let step_min = 60 / u32::from(intervals_per_hour);
    // Local interval-start minutes since local midnight (hour 24 wraps to
    // the next civil day).
    let start_min = (u32::from(hour_ending) - 1) * 60 + u32::from(interval_index - 1) * step_min;
    let (day_offset, min_of_day) = (start_min / 1440, start_min % 1440);
    let local_date = date
        .checked_add(time::Duration::days(i64::from(day_offset)))
        .ok_or_else(|| ErcotError::Time("date overflow".to_string()))?;
    let local_time =
        Time::from_hms(0, 0, 0).expect("midnight") + time::Duration::minutes(i64::from(min_of_day));
    let naive = PrimitiveDateTime::new(local_date, local_time);

    let year = local_date.year();
    let spring = spring_forward_utc(year);
    let fall = fall_back_utc(year);

    // Candidate UTC instants under both offsets.
    let as_cdt = naive.assume_offset(CDT);
    let as_cst = naive.assume_offset(CST);

    // Ambiguous window: the fall-back day's [01:00, 02:00) local hour.
    let ambiguous = naive >= PrimitiveDateTime::new(
        fall_back_utc(year).date(),
        Time::from_hms(1, 0, 0).expect("valid time"),
    ) && naive
        < PrimitiveDateTime::new(
            fall_back_utc(year).date(),
            Time::from_hms(2, 0, 0).expect("valid time"),
        );
    // Gap window: the spring-forward day's [02:00, 03:00) local hour.
    let gap = naive >= PrimitiveDateTime::new(
        spring_forward_utc(year).date(),
        Time::from_hms(2, 0, 0).expect("valid time"),
    ) && naive
        < PrimitiveDateTime::new(
            spring_forward_utc(year).date(),
            Time::from_hms(3, 0, 0).expect("valid time"),
        );

    if ambiguous {
        // First occurrence is CDT; the flagged repeat is CST.
        return Ok(if repeated_hour { as_cst } else { as_cdt });
    }
    if gap {
        return Err(ErcotError::Time(format!(
            "{naive} falls in the spring-forward gap (CPT)"
        )));
    }
    let _ = (spring, fall);
    Ok(if offset_at_local(naive) == CDT { as_cdt } else { as_cst })
}

/// CPT offset for an unambiguous local civil time.
fn offset_at_local(naive: PrimitiveDateTime) -> UtcOffset {
    let year = naive.year();
    // Local-view bounds: CDT runs from 2nd Sun Mar 03:00 local (exclusive
    // of the gap) to 1st Sun Nov 02:00 local (exclusive; the ambiguous hour
    // is handled by the caller).
    let cdt_start = PrimitiveDateTime::new(
        nth_weekday(year, Month::March, Weekday::Sunday, 2),
        Time::from_hms(3, 0, 0).expect("valid time"),
    );
    let cdt_end = PrimitiveDateTime::new(
        nth_weekday(year, Month::November, Weekday::Sunday, 1),
        Time::from_hms(2, 0, 0).expect("valid time"),
    );
    if naive >= cdt_start && naive < cdt_end {
        CDT
    } else {
        CST
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    #[test]
    fn summer_is_cdt() {
        // 2023-08-17 hour-ending 18, interval 1 => 17:00 CPT = 22:00 UTC.
        let ts = cpt_interval_to_utc(d(2023, 8, 17), 18, 1, 4, false).unwrap();
        assert_eq!(ts, datetime!(2023-08-17 22:00:00 UTC));
    }

    #[test]
    fn winter_is_cst() {
        // 2023-01-15 hour-ending 1 => 00:00 CST = 06:00 UTC.
        let ts = cpt_interval_to_utc(d(2023, 1, 15), 1, 1, 4, false).unwrap();
        assert_eq!(ts, datetime!(2023-01-15 06:00:00 UTC));
    }

    #[test]
    fn hour_24_is_next_day_midnight() {
        let ts = cpt_interval_to_utc(d(2023, 8, 17), 24, 4, 4, false).unwrap();
        // 23:45 CPT = next day 04:45 UTC.
        assert_eq!(ts, datetime!(2023-08-18 04:45:00 UTC));
    }

    #[test]
    fn fall_back_repeated_hour_is_preserved() {
        // 2023-11-05: hour-ending 2 occurs twice.
        let first = cpt_interval_to_utc(d(2023, 11, 5), 2, 1, 4, false).unwrap();
        let repeat = cpt_interval_to_utc(d(2023, 11, 5), 2, 1, 4, true).unwrap();
        assert_eq!(first, datetime!(2023-11-05 06:00:00 UTC)); // 01:00 CDT
        assert_eq!(repeat, datetime!(2023-11-05 07:00:00 UTC)); // 01:00 CST
        assert_eq!(repeat - first, time::Duration::hours(1));
    }

    #[test]
    fn fall_back_day_has_25_hours() {
        let mut stamps = Vec::new();
        for hour in 1..=24u8 {
            let passes = if hour == 2 { 2 } else { 1 };
            for pass in 0..passes {
                for idx in 1..=4u8 {
                    stamps.push(
                        cpt_interval_to_utc(d(2023, 11, 5), hour, idx, 4, pass == 1).unwrap(),
                    );
                }
            }
        }
        assert_eq!(stamps.len(), 100);
        // Strictly increasing and contiguous at 15-minute steps.
        for w in stamps.windows(2) {
            assert_eq!(w[1] - w[0], time::Duration::minutes(15));
        }
    }

    #[test]
    fn spring_forward_day_has_23_hours() {
        let mut stamps = Vec::new();
        for hour in 1..=24u8 {
            for idx in 1..=4u8 {
                if let Ok(ts) = cpt_interval_to_utc(d(2023, 3, 12), hour, idx, 4, false) {
                    stamps.push(ts);
                }
            }
        }
        assert_eq!(stamps.len(), 92);
        for w in stamps.windows(2) {
            assert_eq!(w[1] - w[0], time::Duration::minutes(15));
        }
    }

    #[test]
    fn utc_round_trip() {
        let ts = datetime!(2023-08-17 22:00:00 UTC);
        let cpt = utc_to_cpt(ts);
        assert_eq!(cpt, PrimitiveDateTime::new(d(2023, 8, 17), Time::from_hms(17, 0, 0).unwrap()));
        assert_eq!(operating_day(ts), d(2023, 8, 17));
        // CST winter round trip.
        let ts = datetime!(2023-01-15 06:00:00 UTC);
        assert_eq!(utc_to_cpt(ts).time(), Time::from_hms(0, 0, 0).unwrap());
    }
}
