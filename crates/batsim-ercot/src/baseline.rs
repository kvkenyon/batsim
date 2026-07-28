//! Pluggable counterfactual-load baselines (spec D.2).
//!
//! A baseline answers: "what would this home's average net load (kW) have
//! been in this interval without dispatch?" Delivered MW is then
//! `baseline - metered`. The methodology is pluggable and the chosen
//! method's [`BaselineMethod::label`] is recorded in every settlement
//! report so P&L is auditable against it.
//!
//! History contract: the `history` accessor returns the metered average kW
//! for the interval STARTING at the queried UTC timestamp, or `None` when
//! no meter data exists. Missing intervals are skipped, never treated as
//! zero. For [`LastNDaysAverage`] with `exclude_event_days` set, callers
//! going through the [`Baseline`] trait must return `None` for event-day
//! intervals (the engine-side convention); callers holding a separate event
//! calendar use [`LastNDaysAverage::baseline_kw_with_events`] instead.

use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime};

use crate::cpt;

/// Pluggable baseline methodology (spec D.2).
pub trait Baseline: Send + Sync {
    /// Human-readable method label recorded in settlement reports.
    fn name(&self) -> String;

    /// Counterfactual average net load (kW) for `home_id` in the interval
    /// starting at `interval_start` (UTC), or `None` when no usable history
    /// exists.
    fn baseline_kw(
        &self,
        home_id: &str,
        interval_start: OffsetDateTime,
        history: &dyn Fn(&str, OffsetDateTime) -> Option<f64>,
    ) -> Option<f64>;
}

/// Mean of the same-interval average kW over the `n` previous CPT operating
/// days (spec D.2 `LastNDaysAverage{n, exclusion_rules}`).
///
/// "Same interval" means the same CPT wall-clock interval start on each
/// prior operating day (DST-correct, via [`same_wallclock_on_day`]): a day
/// on which that wall-clock time does not exist (the spring-forward gap) is
/// skipped, and the ambiguous fall-back hour resolves to its first (CDT)
/// occurrence. Days with no meter data are skipped; the result is `None`
/// when no eligible day has data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastNDaysAverage {
    /// Number of previous CPT operating days to average.
    pub n: u32,
    /// Whether event days are excluded (see module docs for the two ways
    /// the event-day signal reaches this type).
    pub exclude_event_days: bool,
}

impl LastNDaysAverage {
    /// Baseline with an explicit event-day predicate keyed by CPT operating
    /// date. The predicate is consulted only when `exclude_event_days` is
    /// set.
    pub fn baseline_kw_with_events(
        &self,
        home_id: &str,
        interval_start: OffsetDateTime,
        history: &dyn Fn(&str, OffsetDateTime) -> Option<f64>,
        is_event_day: &dyn Fn(Date) -> bool,
    ) -> Option<f64> {
        let today = cpt::operating_day(interval_start);
        let mut sum = 0.0_f64;
        let mut count = 0_u32;
        for k in 1..=self.n {
            let Some(day) = today.checked_sub(Duration::days(i64::from(k))) else {
                continue;
            };
            if self.exclude_event_days && is_event_day(day) {
                continue;
            }
            let Some(ts) = same_wallclock_on_day(interval_start, day) else {
                continue;
            };
            if let Some(kw) = history(home_id, ts) {
                sum += kw;
                count += 1;
            }
        }
        if count == 0 { None } else { Some(sum / f64::from(count)) }
    }
}

impl Baseline for LastNDaysAverage {
    fn name(&self) -> String {
        let exclusion = if self.exclude_event_days { "event_days" } else { "none" };
        format!("LastNDaysAverage{{n:{}, exclusion: {exclusion}}}", self.n)
    }

    fn baseline_kw(
        &self,
        home_id: &str,
        interval_start: OffsetDateTime,
        history: &dyn Fn(&str, OffsetDateTime) -> Option<f64>,
    ) -> Option<f64> {
        // Trait-level contract: with `exclude_event_days` set the caller's
        // history accessor returns `None` on event days (module docs).
        self.baseline_kw_with_events(home_id, interval_start, history, &|_| false)
    }
}

/// Mean of the `pre_event_intervals` metered intervals immediately before
/// the event start (spec D.2 `MeteredBeforeAfter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeteredBeforeAfter {
    /// Number of intervals before the event start to average.
    pub pre_event_intervals: u32,
    /// Cadence of the history grid, seconds (the run's settlement
    /// interval; serde configs default to 900, matching
    /// `rules.settlement.default_interval_secs`).
    pub interval_secs: u32,
}

impl Baseline for MeteredBeforeAfter {
    fn name(&self) -> String {
        format!(
            "MeteredBeforeAfter{{pre_event_intervals:{}, interval_secs:{}}}",
            self.pre_event_intervals, self.interval_secs
        )
    }

    fn baseline_kw(
        &self,
        home_id: &str,
        interval_start: OffsetDateTime,
        history: &dyn Fn(&str, OffsetDateTime) -> Option<f64>,
    ) -> Option<f64> {
        let mut sum = 0.0_f64;
        let mut count = 0_u32;
        for k in 1..=self.pre_event_intervals {
            let ts =
                interval_start - Duration::seconds(i64::from(self.interval_secs) * i64::from(k));
            if let Some(kw) = history(home_id, ts) {
                sum += kw;
                count += 1;
            }
        }
        if count == 0 { None } else { Some(sum / f64::from(count)) }
    }
}

/// Serde-friendly configuration form of the baseline methods (spec D.2:
/// "baseline as a pluggable component ... record the chosen baseline method
/// in settlement output"). [`BaselineMethod::label`] is the recorded string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineMethod {
    /// See [`LastNDaysAverage`].
    LastNDaysAverage {
        /// Number of previous CPT operating days to average.
        n: u32,
        /// Exclude event days from the average (see module docs).
        exclude_event_days: bool,
    },
    /// See [`MeteredBeforeAfter`].
    MeteredBeforeAfter {
        /// Number of pre-event intervals to average.
        pre_event_intervals: u32,
        /// Cadence of the history grid, seconds (default 900).
        #[serde(default = "default_interval_secs")]
        interval_secs: u32,
    },
}

const fn default_interval_secs() -> u32 {
    900
}

impl BaselineMethod {
    /// Report label, e.g. `LastNDaysAverage{n:10, exclusion: event_days}`.
    #[must_use]
    pub fn label(&self) -> String {
        self.name()
    }
}

impl Baseline for BaselineMethod {
    fn name(&self) -> String {
        match *self {
            Self::LastNDaysAverage { n, exclude_event_days } =>
                LastNDaysAverage { n, exclude_event_days }.name(),
            Self::MeteredBeforeAfter { pre_event_intervals, interval_secs } =>
                MeteredBeforeAfter { pre_event_intervals, interval_secs }.name(),
        }
    }

    fn baseline_kw(
        &self,
        home_id: &str,
        interval_start: OffsetDateTime,
        history: &dyn Fn(&str, OffsetDateTime) -> Option<f64>,
    ) -> Option<f64> {
        match *self {
            Self::LastNDaysAverage { n, exclude_event_days } =>
                LastNDaysAverage { n, exclude_event_days }
                    .baseline_kw(home_id, interval_start, history),
            Self::MeteredBeforeAfter { pre_event_intervals, interval_secs } =>
                MeteredBeforeAfter { pre_event_intervals, interval_secs }
                    .baseline_kw(home_id, interval_start, history),
        }
    }
}

/// UTC instant of the same CPT wall-clock time as `ts_utc` on operating day
/// `day`, or `None` when that wall-clock time does not exist on `day` (the
/// spring-forward gap). The ambiguous fall-back hour resolves to its first
/// (CDT) occurrence, deterministically.
fn same_wallclock_on_day(ts_utc: OffsetDateTime, day: Date) -> Option<OffsetDateTime> {
    let local = cpt::utc_to_cpt(ts_utc);
    let target = PrimitiveDateTime::new(day, local.time());
    let daylight_guess = target.assume_offset(cpt::CDT);
    if cpt::utc_to_cpt(daylight_guess) == target {
        return Some(daylight_guess);
    }
    let standard_guess = target.assume_offset(cpt::CST);
    if cpt::utc_to_cpt(standard_guess) == target {
        return Some(standard_guess);
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use time::Month;
    use time::macros::datetime;

    fn aug(day: u8) -> Date {
        Date::from_calendar_date(2026, Month::August, day).unwrap()
    }

    #[test]
    fn last_n_days_average_means_prior_operating_days() {
        let method = LastNDaysAverage { n: 3, exclude_event_days: false };
        // 2026-08-10 22:00 UTC = 17:00 CPT; prior days same wall clock.
        let start = datetime!(2026-08-10 22:00:00 UTC);
        let history = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-09 22:00:00 UTC) => Some(4.0),
                t if t == datetime!(2026-08-08 22:00:00 UTC) => Some(6.0),
                t if t == datetime!(2026-08-07 22:00:00 UTC) => Some(5.0),
                _ => None,
            }
        };
        // (4 + 6 + 5) / 3 = 5.
        assert_eq!(method.baseline_kw("h1", start, &history), Some(5.0));
        assert_eq!(method.name(), "LastNDaysAverage{n:3, exclusion: none}");
    }

    #[test]
    fn last_n_days_average_skips_missing_days() {
        let method = LastNDaysAverage { n: 3, exclude_event_days: false };
        let start = datetime!(2026-08-10 22:00:00 UTC);
        // Aug 8 missing -> mean(4, 5) = 4.5.
        let history = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-09 22:00:00 UTC) => Some(4.0),
                t if t == datetime!(2026-08-07 22:00:00 UTC) => Some(5.0),
                _ => None,
            }
        };
        assert_eq!(method.baseline_kw("h1", start, &history), Some(4.5));
        // No data at all -> None.
        assert_eq!(method.baseline_kw("h1", start, &|_, _| None), None);
    }

    #[test]
    fn last_n_days_average_event_day_predicate() {
        let start = datetime!(2026-08-10 22:00:00 UTC);
        let history = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-09 22:00:00 UTC) => Some(100.0), // event day
                t if t == datetime!(2026-08-08 22:00:00 UTC) => Some(4.0),
                _ => None,
            }
        };
        let is_event = |d: Date| d == aug(9);
        let excluding = LastNDaysAverage { n: 2, exclude_event_days: true };
        assert_eq!(
            excluding.baseline_kw_with_events("h1", start, &history, &is_event),
            Some(4.0)
        );
        // Flag off: predicate ignored, both days averaged.
        let including = LastNDaysAverage { n: 2, exclude_event_days: false };
        assert_eq!(
            including.baseline_kw_with_events("h1", start, &history, &is_event),
            Some(52.0)
        );
        // Trait-level path with exclusion set relies on history returning
        // None on event days (module-doc contract).
        let history_no_event = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-08 22:00:00 UTC) => Some(4.0),
                _ => None,
            }
        };
        assert_eq!(excluding.baseline_kw("h1", start, &history_no_event), Some(4.0));
    }

    #[test]
    fn same_wallclock_crosses_dst_boundaries() {
        // 2026-03-09 09:30 CDT (14:30 UTC) -> same wall clock on 2026-03-08
        // is also 09:30 CDT (14:30 UTC): 09:30 is after the 02:00 spring
        // transition on Mar 8, so both days observe CDT at that hour.
        let ts = datetime!(2026-03-09 14:30:00 UTC);
        assert_eq!(
            same_wallclock_on_day(ts, Date::from_calendar_date(2026, Month::March, 8).unwrap()),
            Some(datetime!(2026-03-08 14:30:00 UTC))
        );
        // Before the transition the shift shows: 2026-03-09 01:30 CDT
        // (06:30 UTC) -> 2026-03-08 01:30 CST (07:30 UTC), 25 h earlier.
        let pre = datetime!(2026-03-09 06:30:00 UTC);
        assert_eq!(
            same_wallclock_on_day(pre, Date::from_calendar_date(2026, Month::March, 8).unwrap()),
            Some(datetime!(2026-03-08 07:30:00 UTC))
        );
        // 02:30 does not exist on the spring-forward day (2026-03-08).
        let gap_ts = datetime!(2026-03-09 07:30:00 UTC); // 02:30 CDT
        assert_eq!(
            same_wallclock_on_day(
                gap_ts,
                Date::from_calendar_date(2026, Month::March, 8).unwrap()
            ),
            None
        );
        // Ambiguous fall-back hour resolves to the first (CDT) occurrence:
        // 2026-11-02 01:30 CST -> 2026-11-01 01:30 CDT (06:30 UTC).
        let fb = datetime!(2026-11-02 07:30:00 UTC);
        assert_eq!(
            same_wallclock_on_day(
                fb,
                Date::from_calendar_date(2026, Month::November, 1).unwrap()
            ),
            Some(datetime!(2026-11-01 06:30:00 UTC))
        );
        // End to end through the baseline across the spring boundary.
        let method = LastNDaysAverage { n: 1, exclude_event_days: false };
        let history = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            (ts == datetime!(2026-03-08 14:30:00 UTC)).then_some(3.0)
        };
        assert_eq!(method.baseline_kw("h1", ts, &history), Some(3.0));
    }

    #[test]
    fn metered_before_after_means_pre_event_intervals() {
        let method = MeteredBeforeAfter { pre_event_intervals: 3, interval_secs: 900 };
        let start = datetime!(2026-08-14 22:00:00 UTC);
        let history = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-14 21:45:00 UTC) => Some(1.0),
                t if t == datetime!(2026-08-14 21:30:00 UTC) => Some(2.0),
                t if t == datetime!(2026-08-14 21:15:00 UTC) => Some(3.0),
                _ => None,
            }
        };
        // (1 + 2 + 3) / 3 = 2.
        assert_eq!(method.baseline_kw("h1", start, &history), Some(2.0));
        // Partial history: only the two nearest intervals present.
        let partial = |_home: &str, ts: OffsetDateTime| -> Option<f64> {
            match ts {
                t if t == datetime!(2026-08-14 21:45:00 UTC) => Some(1.0),
                t if t == datetime!(2026-08-14 21:30:00 UTC) => Some(2.0),
                _ => None,
            }
        };
        assert_eq!(method.baseline_kw("h1", start, &partial), Some(1.5));
        assert_eq!(method.baseline_kw("h1", start, &|_, _| None), None);
        assert_eq!(
            method.name(),
            "MeteredBeforeAfter{pre_event_intervals:3, interval_secs:900}"
        );
    }

    #[test]
    fn labels_match_report_format() {
        assert_eq!(
            BaselineMethod::LastNDaysAverage { n: 10, exclude_event_days: true }.label(),
            "LastNDaysAverage{n:10, exclusion: event_days}"
        );
        assert_eq!(
            BaselineMethod::LastNDaysAverage { n: 10, exclude_event_days: false }.label(),
            "LastNDaysAverage{n:10, exclusion: none}"
        );
        assert_eq!(
            BaselineMethod::MeteredBeforeAfter { pre_event_intervals: 4, interval_secs: 900 }
                .label(),
            "MeteredBeforeAfter{pre_event_intervals:4, interval_secs:900}"
        );
    }

    #[test]
    fn baseline_method_serde_round_trip_with_default_cadence() {
        let method =
            BaselineMethod::MeteredBeforeAfter { pre_event_intervals: 4, interval_secs: 900 };
        let json = serde_json::to_string(&method).unwrap();
        let back: BaselineMethod = serde_json::from_str(&json).unwrap();
        assert_eq!(method, back);
        // interval_secs defaults to 900 when omitted from config JSON.
        let parsed: BaselineMethod =
            serde_json::from_str(r#"{"metered_before_after":{"pre_event_intervals":4}}"#).unwrap();
        assert_eq!(parsed, method);
        let avg = BaselineMethod::LastNDaysAverage { n: 10, exclude_event_days: true };
        let back: BaselineMethod =
            serde_json::from_str(&serde_json::to_string(&avg).unwrap()).unwrap();
        assert_eq!(avg, back);
    }
}
