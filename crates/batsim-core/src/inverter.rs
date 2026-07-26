//! Inverter conversion model (spec B.3; F6): load-dependent efficiency,
//! clipping, standby draw, and the AC/DC coupling-path conversion stages.
//!
//! The free functions here are the explicit loss points of spec A.3.2/3.3
//! (F16): every conversion stage in every topology routes through
//! [`dc_to_ac`] / [`ac_to_dc`] so telemetry can attribute each loss.
//!
//! All powers are magnitudes (W, non-negative); direction is carried by
//! which function is called. Efficiency curves are evaluated at
//! `x_kw = power / 1000` against the registry `EfficiencyCurve` semantics
//! (linear interpolation, endpoint clamping, spec B.3.1).

use batsim_registry::{EfficiencyCurve, InverterModel};
use serde::{Deserialize, Serialize};

/// Result of one conversion stage.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conversion {
    /// Power leaving the stage (W), after efficiency and clipping.
    pub p_out_w: f64,
    /// Power lost to heat in the stage (W).
    pub loss_w: f64,
    /// Power clipped at the stage rating (W; counted separately, B.3.3).
    pub clipped_w: f64,
}

/// Efficiency floor: at/below this the stage is treated as total loss
/// (guards the (0, 0) zero anchor of B.3.1 curves from NaN/Inf).
const ETA_FLOOR: f64 = 1e-6;

/// DC -> AC conversion (discharge path or PV inversion), B.3.1/B.3.3:
/// `P_ac = P_dc * eta_inv(|P_dc|/P_rated)` hard-clamped to the AC rating;
/// the clamp bounds the OUTPUT, so excess DC input is reported as clipped.
///
/// The clipped share of the DC input is the proportional overhang
/// `p_dc - p_out/eta` (the slice of input power that could not leave the
/// stage); it is counted as clipped, never as heat loss.
#[must_use]
pub fn dc_to_ac(curve: &EfficiencyCurve, rated_ac_w: f64, p_dc_w: f64) -> Conversion {
    let p_dc_w = p_dc_w.max(0.0);
    let eta = curve.eval(p_dc_w / 1000.0);
    if p_dc_w <= 0.0 || eta <= ETA_FLOOR {
        // Zero input, or the (0,0) zero anchor: total loss, no output.
        return Conversion {
            p_out_w: 0.0,
            loss_w: p_dc_w,
            clipped_w: 0.0,
        };
    }
    let p_ac_unclamped = p_dc_w * eta;
    if p_ac_unclamped <= rated_ac_w {
        Conversion {
            p_out_w: p_ac_unclamped,
            loss_w: p_dc_w - p_ac_unclamped,
            clipped_w: 0.0,
        }
    } else {
        let p_out_w = rated_ac_w;
        let clipped_w = p_dc_w - p_out_w / eta;
        Conversion {
            p_out_w,
            loss_w: p_out_w / eta * (1.0 - eta),
            clipped_w,
        }
    }
}

/// AC -> DC conversion (charge path), B.3.1:
/// `P_dc = |P_ac_req| * eta_inv(|P_ac_req|/P_rated)`. The request is
/// clamped to the AC rating first (no clipping counter: the limit is a
/// normal operating bound, not lost energy).
///
/// # Deviation from the B.3.1 formula text
///
/// Spec B.3.1 literally writes `P_dc_draw = |P_ac_req| / eta_inv` for the
/// charge path. That reading violates energy conservation (the DC side
/// would receive MORE power than the AC side draws), so this function
/// implements the conservation-true charge-forward conversion instead:
/// DC delivered = AC requested x eta. The inverse question "AC draw
/// required for a DC target" is the caller's job (`p_ac = p_dc / eta`,
/// one fixed-point step when eta is power-dependent).
#[must_use]
pub fn ac_to_dc(curve: &EfficiencyCurve, rated_ac_w: f64, p_ac_req_w: f64) -> Conversion {
    let p_ac_req_w = p_ac_req_w.max(0.0).min(rated_ac_w);
    let eta = curve.eval(p_ac_req_w / 1000.0).max(0.0);
    let p_out_w = p_ac_req_w * eta;
    Conversion {
        p_out_w,
        loss_w: p_ac_req_w - p_out_w,
        clipped_w: 0.0,
    }
}

/// DC power the source must deliver for an AC output target (the inverse
/// of [`dc_to_ac`]): `P_dc = P_ac_target / eta(P_ac_target)`. The target is
/// clamped to the AC rating first, so a caller never asks its DC source for
/// power the stage could not pass.
#[must_use]
pub fn dc_required_for_ac(curve: &EfficiencyCurve, rated_ac_w: f64, p_ac_target_w: f64) -> f64 {
    let p_ac = p_ac_target_w.max(0.0).min(rated_ac_w.max(0.0));
    let eta = curve.eval(p_ac / 1000.0).max(ETA_FLOOR);
    p_ac / eta
}

/// AC draw required to cover a DC-bus deficit (the inverse of [`ac_to_dc`]
/// evaluated from the DC side): `P_ac = P_dc / eta(P_dc)`, one fixed-point
/// step. Unclamped: the DC has already been absorbed downstream, so the
/// draw must be metered in full (conservation-true, D1 decision).
#[must_use]
pub fn ac_required_for_dc(curve: &EfficiencyCurve, p_dc_target_w: f64) -> f64 {
    let p_dc = p_dc_target_w.max(0.0);
    let eta = curve.eval(p_dc / 1000.0).max(ETA_FLOOR);
    p_dc / eta
}

/// Resolve two candidate AC flows (PV output and battery discharge)
/// sharing one inverter's AC rating (spec B.3.3): the combined flow is
/// capped at `rated_ac_w`; with `pv_priority` PV is admitted first and
/// the battery takes the remainder (the default, matching hybrid
/// firmware); otherwise the battery is admitted first.
/// Returns `(pv_ac_w, batt_ac_w)` admitted through the inverter.
#[must_use]
pub fn resolve_shared_ac_cap(
    rated_ac_w: f64,
    pv_ac_candidate_w: f64,
    batt_ac_candidate_w: f64,
    pv_priority: bool,
) -> (f64, f64) {
    let rated = rated_ac_w.max(0.0);
    let pv = pv_ac_candidate_w.max(0.0);
    let batt = batt_ac_candidate_w.max(0.0);
    if pv_priority {
        let pv_admitted = pv.min(rated);
        (pv_admitted, batt.min(rated - pv_admitted))
    } else {
        let batt_admitted = batt.min(rated);
        (pv.min(rated - batt_admitted), batt_admitted)
    }
}

/// A live inverter instance: registry model plus standby state.
///
/// Only explicitly-declared inverters (hybrid or PV string) exist as
/// `InverterUnit`s; integrated battery inverters are folded into
/// `BatteryUnit` terminal semantics (see `battery` module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InverterUnit {
    /// The registry model driving this unit.
    model: InverterModel,
    /// Number of identical physical units aggregated into this instance.
    quantity: u32,
    /// Aggregate rated continuous AC output (W): catalog kW x quantity.
    rated_ac_w: f64,
    /// Aggregate efficiency curve: the model curve with its x-axis scaled
    /// by `quantity`, so evaluating at fleet power yields the per-unit
    /// efficiency at the equally-shared per-unit power.
    curve: EfficiencyCurve,
    /// Standby draw while energized (W), AC side (B.3.2).
    standby_w: f64,
}

impl InverterUnit {
    /// Build a single-unit instance from its registry model. `standby_w`
    /// is the measured or estimated AC-side standby draw (B.3.2); it is
    /// carried, never invented from the model.
    #[must_use]
    pub fn new(model: &InverterModel, standby_w: f64) -> Self {
        Self::with_quantity(model, 1, standby_w)
    }

    /// Build an instance aggregating `quantity` identical units of the
    /// model: the AC rating and the efficiency-curve x-axis both scale
    /// linearly, so N units share the flow equally (spec A.4.4).
    ///
    /// `quantity` is treated as at least 1.
    #[must_use]
    pub fn with_quantity(model: &InverterModel, quantity: u32, standby_w: f64) -> Self {
        let quantity = quantity.max(1);
        let n = f64::from(quantity);
        let mut curve = model.efficiency_curve.clone();
        for point in &mut curve.points {
            point.x_kw *= n;
        }
        Self {
            rated_ac_w: model.rated_ac_output_kw.value * 1000.0 * n,
            curve,
            quantity,
            standby_w,
            model: model.clone(),
        }
    }

    /// Aggregate rated continuous AC output (W) across all units.
    #[must_use]
    pub fn rated_ac_w(&self) -> f64 {
        self.rated_ac_w
    }

    /// Number of identical physical units this instance aggregates.
    #[must_use]
    pub fn quantity(&self) -> u32 {
        self.quantity
    }

    /// Standby draw while energized (W), AC side (B.3.2).
    #[must_use]
    pub fn standby_w(&self) -> f64 {
        self.standby_w
    }

    /// Conversion efficiency at an AC-side power magnitude, floored away
    /// from zero so callers can divide by it safely.
    #[must_use]
    pub fn eta_at_w(&self, p_w: f64) -> f64 {
        self.curve.eval(p_w.abs() / 1000.0).max(ETA_FLOOR)
    }

    /// DC -> AC through this inverter.
    #[must_use]
    pub fn dc_to_ac(&self, p_dc_w: f64) -> Conversion {
        dc_to_ac(&self.curve, self.rated_ac_w, p_dc_w)
    }

    /// DC -> AC through this inverter against a tighter AC cap than the
    /// nameplate rating (e.g. the array's declared DC/AC ratio, B.7.4).
    #[must_use]
    pub fn dc_to_ac_capped(&self, p_dc_w: f64, cap_ac_w: f64) -> Conversion {
        dc_to_ac(&self.curve, self.rated_ac_w.min(cap_ac_w.max(0.0)), p_dc_w)
    }

    /// AC -> DC through this inverter.
    #[must_use]
    pub fn ac_to_dc(&self, p_ac_req_w: f64) -> Conversion {
        ac_to_dc(&self.curve, self.rated_ac_w, p_ac_req_w)
    }

    /// DC power required from the source for an AC output target.
    #[must_use]
    pub fn dc_required_for_ac(&self, p_ac_target_w: f64) -> f64 {
        dc_required_for_ac(&self.curve, self.rated_ac_w, p_ac_target_w)
    }

    /// AC draw required to cover a DC-bus deficit.
    #[must_use]
    pub fn ac_required_for_dc(&self, p_dc_target_w: f64) -> f64 {
        ac_required_for_dc(&self.curve, p_dc_target_w)
    }

    /// The registry model (unscaled; see [`Self::quantity`]).
    #[must_use]
    pub fn model(&self) -> &InverterModel {
        &self.model
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use batsim_registry::{EfficiencyPoint, Provenance};

    /// B.3.1-conforming curve: (0, 0) anchor, poor 5 % load efficiency,
    /// 0.97 peak in the 0.2-0.5 band, slight rolloff at full load.
    fn cec_curve(rated_kw: f64) -> EfficiencyCurve {
        EfficiencyCurve {
            points: vec![
                EfficiencyPoint {
                    x_kw: 0.0,
                    efficiency: 0.0,
                },
                EfficiencyPoint {
                    x_kw: 0.05 * rated_kw,
                    efficiency: 0.90,
                },
                EfficiencyPoint {
                    x_kw: 0.20 * rated_kw,
                    efficiency: 0.97,
                },
                EfficiencyPoint {
                    x_kw: 0.50 * rated_kw,
                    efficiency: 0.97,
                },
                EfficiencyPoint {
                    x_kw: rated_kw,
                    efficiency: 0.955,
                },
            ],
            provenance: Provenance::Estimated,
        }
    }

    #[test]
    fn dc_to_ac_efficiency_and_clipping() {
        let curve = cec_curve(10.0);
        // Mid-band: 5 kW DC at eta 0.97 -> 4850 W AC, 150 W heat, no clip.
        let c = dc_to_ac(&curve, 10_000.0, 5_000.0);
        assert!((c.p_out_w - 4_850.0).abs() < 1e-9);
        assert!((c.loss_w - 150.0).abs() < 1e-9);
        assert_eq!(c.clipped_w, 0.0);
        // Overdrive: 15 kW DC -> output clamped to 10 kW AC, excess DC
        // input counted as clipped (not heat).
        let c = dc_to_ac(&curve, 10_000.0, 15_000.0);
        assert_eq!(c.p_out_w, 10_000.0);
        let eta = curve.eval(15.0);
        assert!(c.clipped_w > 0.0);
        assert!((c.p_out_w + c.loss_w + c.clipped_w - 15_000.0).abs() < 1e-6);
        assert!((c.loss_w - 10_000.0 / eta * (1.0 - eta)).abs() < 1e-6);
    }

    #[test]
    fn dc_to_ac_low_power_efficiency_is_poor() {
        let curve = cec_curve(10.0);
        // 5 % load: eta 0.90 (B.3.1 MUST-show poor low-load efficiency).
        let c = dc_to_ac(&curve, 10_000.0, 500.0);
        assert!((c.p_out_w - 450.0).abs() < 1e-9);
        // Below 5 % the linear-from-zero anchor drops eta proportionally:
        // at 1 % load eta = 0.18, so 100 W DC yields only 18 W AC.
        let c = dc_to_ac(&curve, 10_000.0, 100.0);
        assert!((c.p_out_w - 18.0).abs() < 1e-9);
        assert!((c.loss_w - 82.0).abs() < 1e-9);
        assert_eq!(c.clipped_w, 0.0);
        // Zero input is exact silence.
        let c = dc_to_ac(&curve, 10_000.0, 0.0);
        assert_eq!((c.p_out_w, c.loss_w, c.clipped_w), (0.0, 0.0, 0.0));
        // A flat-zero curve start is the total-loss guard (no NaN/Inf).
        let dead = EfficiencyCurve {
            points: vec![
                EfficiencyPoint {
                    x_kw: 0.0,
                    efficiency: 0.0,
                },
                EfficiencyPoint {
                    x_kw: 1.0,
                    efficiency: 0.0,
                },
            ],
            provenance: Provenance::Estimated,
        };
        let c = dc_to_ac(&dead, 10_000.0, 500.0);
        assert_eq!((c.p_out_w, c.loss_w, c.clipped_w), (0.0, 500.0, 0.0));
    }

    #[test]
    fn ac_to_dc_multiplies_by_eta_and_clamps_request() {
        let curve = cec_curve(10.0);
        // 5 kW AC request at eta 0.97 -> 4850 W DC delivered, 150 W heat.
        let c = ac_to_dc(&curve, 10_000.0, 5_000.0);
        assert!((c.p_out_w - 4_850.0).abs() < 1e-9);
        assert!((c.loss_w - 150.0).abs() < 1e-9);
        assert_eq!(c.clipped_w, 0.0);
        // Request above the rating clamps to the rating (no clip counter).
        let c = ac_to_dc(&curve, 10_000.0, 20_000.0);
        let eta_at_rated = curve.eval(10.0);
        assert!((c.p_out_w - 10_000.0 * eta_at_rated).abs() < 1e-6);
        // Tiny request under the (0,0) anchor: near-total loss, finite.
        let c = ac_to_dc(&curve, 10_000.0, 1.0);
        assert!(c.p_out_w.is_finite() && c.loss_w.is_finite());
        assert!(c.p_out_w < 1.0, "zero-anchor region loses almost all");
        assert!((c.p_out_w + c.loss_w - 1.0).abs() < 1e-12);
        // Zero request is exact silence.
        let c = ac_to_dc(&curve, 10_000.0, 0.0);
        assert_eq!((c.p_out_w, c.loss_w, c.clipped_w), (0.0, 0.0, 0.0));
    }

    #[test]
    fn dc_required_for_ac_inverts_dc_to_ac() {
        let curve = cec_curve(10.0);
        // Asking for 4850 W AC needs 5 kW DC back through the same curve.
        let p_dc = dc_required_for_ac(&curve, 10_000.0, 4_850.0);
        let back = dc_to_ac(&curve, 10_000.0, p_dc);
        assert!((back.p_out_w - 4_850.0).abs() < 5.0);
        // Targets above the rating clamp: never ask the pack for power the
        // stage could not pass.
        let capped = dc_required_for_ac(&curve, 10_000.0, 50_000.0);
        assert!(capped.is_finite() && capped < 11_000.0);
        assert_eq!(dc_required_for_ac(&curve, 10_000.0, 0.0), 0.0);
    }

    #[test]
    fn ac_required_for_dc_covers_the_bus_deficit_unclamped() {
        let curve = cec_curve(10.0);
        // 4850 W of DC deficit at eta(4.85 kW) = 0.97 costs 5 kW AC.
        let p_ac = ac_required_for_dc(&curve, 4_850.0);
        assert!(p_ac > 4_850.0);
        assert!((p_ac - 4_850.0 / curve.eval(4.85)).abs() < 1e-9);
        // Unclamped: an over-rating deficit is still metered in full.
        assert!(ac_required_for_dc(&curve, 20_000.0) > 20_000.0);
        assert_eq!(ac_required_for_dc(&curve, 0.0), 0.0);
    }

    #[test]
    fn shared_ac_cap_pv_priority_default() {
        // Under the cap: both pass.
        assert_eq!(
            resolve_shared_ac_cap(10_000.0, 3_000.0, 4_000.0, true),
            (3_000.0, 4_000.0)
        );
        // Over the cap with PV priority: PV first, battery gets the rest.
        assert_eq!(
            resolve_shared_ac_cap(10_000.0, 8_000.0, 5_000.0, true),
            (8_000.0, 2_000.0)
        );
        // PV alone saturates: battery fully blocked.
        assert_eq!(
            resolve_shared_ac_cap(10_000.0, 12_000.0, 5_000.0, true),
            (10_000.0, 0.0)
        );
        // Battery priority flips the split.
        assert_eq!(
            resolve_shared_ac_cap(10_000.0, 8_000.0, 5_000.0, false),
            (5_000.0, 5_000.0)
        );
    }
}
