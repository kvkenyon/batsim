//! Ancillary services: duration derate, deployment modeling, and
//! performance-scored revenue (spec D.1.3, D.5.1, D.7).
//!
//! AS awards are scenario inputs — the simulator does not clear the DAM.
//! This module computes (a) the duration-based derate bounding how much
//! rated power a battery may sell into a product, and (b) net award revenue
//! given simulated delivered-vs-instructed performance.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::rules::{AsPerformance, ErcotRules};
use crate::types::AsProduct;

/// Awardable fraction of rated discharge power for an AS product (spec D.1.3).
///
/// `min(1, usable_hours / full_duration_hours)` where
/// `usable_hours = usable_energy_kwh / discharge_kw`: a resource with at
/// least `full_duration_hours` of usable energy sells full rated power;
/// shorter durations derate linearly (e.g. a 1-hour battery sells at most
/// half its rated power into ECRS, whose full duration is 2 h).
///
/// Returns 0.0 when the product is not `available_to_ader` (RegUp/RegDown
/// are modeled as out of reach for aggregated residential fleets), when the
/// product has no rule entry, or when `discharge_kw <= 0`.
#[must_use]
pub fn duration_derate(
    usable_energy_kwh: f64,
    discharge_kw: f64,
    product: AsProduct,
    rules: &ErcotRules,
) -> f64 {
    let Some(rule) = rules.as_rule(product) else {
        return 0.0;
    };
    if !rule.available_to_ader || discharge_kw <= 0.0 {
        return 0.0;
    }
    let usable_hours = usable_energy_kwh / discharge_kw;
    (usable_hours / rule.full_duration_hours).clamp(0.0, 1.0)
}

/// One AS deployment event: an aggregate MW instruction over `[start, end)`
/// (spec D.7: scoring starts at `t0 + response_deadline(product)`; the
/// delivered-vs-instructed integration is performed by the Part B engine and
/// reaches settlement as aggregate MWh).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    /// Product being deployed.
    pub product: AsProduct,
    /// Instruction start, UTC (inclusive).
    pub start: OffsetDateTime,
    /// Instruction end, UTC (exclusive).
    pub end: OffsetDateTime,
    /// Instructed aggregate power, MW.
    pub instructed_mw: f64,
}

impl Deployment {
    /// Instruction duration, hours.
    #[must_use]
    pub fn hours(&self) -> f64 {
        (self.end - self.start).as_seconds_f64() / 3600.0
    }

    /// Instructed energy over the full window, MWh.
    #[must_use]
    pub fn instructed_mwh(&self) -> f64 {
        self.instructed_mw * self.hours()
    }
}

/// Outcome of scoring delivered vs instructed energy (spec D.5.1
/// simplification, produced by [`performance_factor`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerformanceOutcome {
    /// Delivered / instructed ratio, clamped to `[0, 1]`.
    pub factor: f64,
    /// Clawback multiplier applied to the shortfall revenue; 0.0 when
    /// `factor >= threshold`.
    pub clawback_usd_multiplier: f64,
}

/// Compute the performance factor for delivered vs instructed energy.
///
/// Documented simplification (spec D.5.1 — exact ERCOT non-performance
/// penalties are protocol-specific and out of scope v1):
///
/// - `factor = delivered_mwh / instructed_mwh`, clamped to `[0, 1]`
///   (over-delivery does not pay extra);
/// - nothing instructed and nothing delivered scores 1.0; any delivery (or
///   consumption) against a zero instruction scores 0.0, mirroring ERCOT's
///   treatment of unrequested energy;
/// - when `factor < rules_perf.threshold`, the rules
///   `[as.performance].clawback_multiplier` applies to the shortfall
///   revenue; otherwise the clawback is 0.
#[must_use]
pub fn performance_factor(
    delivered_mwh: f64,
    instructed_mwh: f64,
    rules_perf: &AsPerformance,
) -> PerformanceOutcome {
    let factor = if instructed_mwh == 0.0 {
        if delivered_mwh == 0.0 { 1.0 } else { 0.0 }
    } else {
        (delivered_mwh / instructed_mwh).clamp(0.0, 1.0)
    };
    let clawback_usd_multiplier = if factor < rules_perf.threshold {
        rules_perf.clawback_multiplier
    } else {
        0.0
    };
    PerformanceOutcome { factor, clawback_usd_multiplier }
}

/// Net AS revenue for an award (spec D.5.1):
/// `awarded_mw x mcpc x hours x factor`, minus the clawback on the
/// shortfall: `gross x (1 - factor) x clawback_multiplier`.
///
/// With the default rules (threshold 0.90, multiplier 2.0) a factor of 0.80
/// nets `gross x (0.80 - 0.20 x 2.0) = gross x 0.40`.
#[must_use]
pub fn as_revenue(
    awarded_mw: f64,
    hours: f64,
    mcpc_usd_per_mw: f64,
    perf: &PerformanceOutcome,
) -> f64 {
    let gross_usd = awarded_mw * mcpc_usd_per_mw * hours;
    net_from_gross(gross_usd, perf)
}

/// Net revenue from a gross amount under a performance outcome. Shared by
/// [`as_revenue`] and the settlement roll-up so both use one formula.
pub(crate) fn net_from_gross(gross_usd: f64, perf: &PerformanceOutcome) -> f64 {
    gross_usd * perf.factor - gross_usd * (1.0 - perf.factor) * perf.clawback_usd_multiplier
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn rules() -> ErcotRules {
        ErcotRules::current().unwrap()
    }

    fn assert_close(actual: f64, expected: f64) {
        let tol = 1e-9 * expected.abs().max(1.0);
        assert!((actual - expected).abs() <= tol, "expected {expected}, got {actual}");
    }

    #[test]
    fn duration_derate_hand_computed() {
        let rules = rules();
        // 13.5 kWh usable / 5 kW = 2.7 h into ECRS (full duration 2 h) -> 1.0.
        assert_eq!(duration_derate(13.5, 5.0, AsProduct::Ecrs, &rules), 1.0);
        // 5 kWh / 5 kW = 1 h -> 0.5.
        assert_eq!(duration_derate(5.0, 5.0, AsProduct::Ecrs, &rules), 0.5);
        // RegUp is not available to aggregated residential fleets -> 0.
        assert_eq!(duration_derate(13.5, 5.0, AsProduct::RegUp, &rules), 0.0);
        // No discharge power -> 0.
        assert_eq!(duration_derate(13.5, 0.0, AsProduct::Ecrs, &rules), 0.0);
    }

    #[test]
    fn performance_scoring_thresholds() {
        let perf = rules().as_performance;
        // 0.93 >= 0.90 threshold -> no clawback.
        let out = performance_factor(29.76, 32.0, &perf);
        assert_eq!(out.factor, 0.93);
        assert_eq!(out.clawback_usd_multiplier, 0.0);
        // 0.80 < 0.90 -> clawback multiplier 2.0.
        let out = performance_factor(25.6, 32.0, &perf);
        assert_eq!(out.factor, 0.80);
        assert_eq!(out.clawback_usd_multiplier, 2.0);
        // Over-delivery caps at 1.0.
        assert_eq!(performance_factor(40.0, 32.0, &perf).factor, 1.0);
        // Nothing instructed, nothing delivered -> 1.0; unrequested
        // delivery against a zero instruction -> 0.0.
        assert_eq!(performance_factor(0.0, 0.0, &perf).factor, 1.0);
        assert_eq!(performance_factor(1.0, 0.0, &perf).factor, 0.0);
    }

    #[test]
    fn as_revenue_hand_computed() {
        // Spec D.6 worked example: 8 MW ECRS x 4 h x $184.20/MW = $5894.40 gross.
        let good = PerformanceOutcome { factor: 0.93, clawback_usd_multiplier: 0.0 };
        assert_close(as_revenue(8.0, 4.0, 184.20, &good), 5481.792);
        // factor 0.80 < 0.90 with clawback x2.0:
        // net = gross x (0.8 - 0.2 x 2.0) = gross x 0.4.
        let bad = PerformanceOutcome { factor: 0.80, clawback_usd_multiplier: 2.0 };
        assert_close(as_revenue(8.0, 4.0, 184.20, &bad), 2357.76);
    }

    #[test]
    fn deployment_energy_helpers() {
        use time::macros::datetime;
        let d = Deployment {
            product: AsProduct::Ecrs,
            start: datetime!(2026-08-14 22:00:00 UTC),
            end: datetime!(2026-08-15 02:00:00 UTC),
            instructed_mw: 8.0,
        };
        assert_eq!(d.hours(), 4.0);
        assert_eq!(d.instructed_mwh(), 32.0);
    }
}
