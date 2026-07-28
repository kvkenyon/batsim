//! 4CP (Four Coincident Peak) watch and savings attribution (spec D.1.4).
//!
//! ERCOT allocates a large share of transmission cost to load-serving
//! entities by their coincident peak demand during the ERCOT system peak
//! 15-minute interval in each of June-September. A fleet that reduces net
//! load during the actual peak interval directly reduces the retailer's
//! 4CP tag. Because the season's actual peaks are known only retroactively,
//! [`FourCpWatch`] flags *candidate* intervals in real time and settlement
//! (`crate::settlement`) supports retro-confirmation marking.

use time::OffsetDateTime;

use crate::cpt;
use crate::rules::{ErcotRules, FourCp};
use crate::types::SystemSignal;

/// Running 4CP candidate watch for one season (spec D.1.4 (a)).
#[derive(Debug, Clone)]
pub struct FourCpWatch {
    /// Season-to-date system peak, MW (updated only from in-season samples).
    season_peak_mw: f64,
    /// 4CP policy constants (months, candidate window, annual allocation).
    rules: FourCp,
}

impl FourCpWatch {
    /// New watch for a season, seeded from the `[four_cp]` section of
    /// `rules`.
    #[must_use]
    pub fn new(rules: &ErcotRules) -> Self {
        Self { season_peak_mw: 0.0, rules: rules.four_cp.clone() }
    }

    /// Season-to-date peak observed so far, MW.
    #[must_use]
    pub const fn season_peak_mw(&self) -> f64 {
        self.season_peak_mw
    }

    /// Reset for a new season (June 1).
    pub fn reset_season(&mut self) {
        self.season_peak_mw = 0.0;
    }

    /// Observe one system-load sample; returns true when the interval is a
    /// 4CP candidate.
    ///
    /// Candidate rule: the interval's **CPT** month is a 4CP month
    /// (June-September under the default rules) AND system load is at least
    /// `candidate_window_pct_of_peak` of the season-to-date peak INCLUDING
    /// this interval's load (`max(running_peak, load)` — so a new season
    /// high is always a candidate). The running peak is then updated to
    /// that max. Out-of-season samples are never candidates and never move
    /// the season peak.
    pub fn observe(&mut self, signal: &SystemSignal) -> bool {
        let month = u32::from(u8::from(cpt::utc_to_cpt(signal.ts).month()));
        if !self.rules.months.contains(&month) {
            return false;
        }
        let peak = self.season_peak_mw.max(signal.system_load_mw);
        let candidate = signal.system_load_mw >= self.rules.candidate_window_pct_of_peak * peak;
        self.season_peak_mw = peak;
        candidate
    }
}

/// Estimated annual 4CP savings over confirmed coincident-peak intervals
/// (spec D.1.4 / D.5.1).
///
/// Per confirmed interval k:
/// `savings_k = reduction_kw_k x transmission_rate_usd_per_kw_mo x 12 x
/// annual_allocation_per_cp`. Each CP month sets the transmission tag for
/// its `annual_allocation_per_cp` share of the year (rules default 0.25:
/// the four CP months together carry the full annual tag), so with default
/// rules each confirmed interval contributes `reduction_kw x rate x 3`.
/// Example (spec D.6): 38,200 kW at $3.5/kW-mo yields 38200 x 3.5 x 12 x
/// 0.25 = $401,100.
///
/// `confirmed` is `(interval_start_utc, fleet_net_load_reduction_kw)` per
/// confirmed CP interval; entries are applied in slice order for
/// determinism.
#[must_use]
pub fn attribute_savings(
    confirmed: &[(OffsetDateTime, f64)],
    transmission_rate_usd_per_kw_mo: f64,
    rules: &ErcotRules,
) -> f64 {
    let allocation = rules.four_cp.annual_allocation_per_cp;
    confirmed
        .iter()
        .map(|(_, reduction_kw)| reduction_kw * transmission_rate_usd_per_kw_mo * 12.0 * allocation)
        .sum()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use time::macros::datetime;

    fn rules() -> ErcotRules {
        ErcotRules::current().unwrap()
    }

    fn signal(ts: OffsetDateTime, load_mw: f64) -> SystemSignal {
        SystemSignal { ts, system_load_mw: load_mw, reserves_mw: None, fuel_mix: None }
    }

    #[test]
    fn candidate_window_tracks_running_peak() {
        let mut watch = FourCpWatch::new(&rules());
        let at = datetime!(2026-07-15 20:00:00 UTC); // July, 15:00 CDT
        // New season peak is always a candidate.
        assert!(watch.observe(&signal(at, 100.0)));
        // 90 < 95% of 100 -> not a candidate.
        assert!(!watch.observe(&signal(at, 90.0)));
        // Exactly at the window edge (95 >= 0.95 x 100) -> candidate.
        assert!(watch.observe(&signal(at, 95.0)));
        // New peak resets the bar.
        assert!(watch.observe(&signal(at, 120.0)));
        // 110 < 0.95 x 120 = 114 -> not a candidate.
        assert!(!watch.observe(&signal(at, 110.0)));
        assert_eq!(watch.season_peak_mw(), 120.0);
        // Season reset starts a fresh peak.
        watch.reset_season();
        assert_eq!(watch.season_peak_mw(), 0.0);
        assert!(watch.observe(&signal(at, 50.0)));
    }

    #[test]
    fn cpt_month_governs_season_membership() {
        let mut watch = FourCpWatch::new(&rules());
        // 2026-10-01 03:00 UTC = 2026-09-30 22:00 CDT -> CPT month 9, in
        // season (a UTC-month rule would wrongly read October).
        assert!(watch.observe(&signal(datetime!(2026-10-01 03:00:00 UTC), 100.0)));
        // 2026-06-01 04:00 UTC = 2026-05-31 23:00 CDT -> CPT month 5, out of
        // season (a UTC-month rule would wrongly read June).
        assert!(!watch.observe(&signal(datetime!(2026-06-01 04:00:00 UTC), 100.0)));
        // Out-of-season samples never move the season peak.
        assert_eq!(watch.season_peak_mw(), 100.0);
    }

    #[test]
    fn savings_match_spec_d6_example() {
        let rules = rules();
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        // 38,200 kW x 3.5 $/kW-mo x 12 mo x 0.25 = $401,100 (spec D.6).
        assert_eq!(attribute_savings(&[(ts, 38_200.0)], 3.5, &rules), 401_100.0);
        // Two confirmed months add linearly: 1500 x 4.0 x 12 x 0.25 = 18,000.
        assert_eq!(attribute_savings(&[(ts, 1000.0), (ts, 500.0)], 4.0, &rules), 18_000.0);
        // Nothing confirmed -> nothing saved.
        assert_eq!(attribute_savings(&[], 3.5, &rules), 0.0);
    }
}
