//! Inverter conversion model (spec B.3; F6): load-dependent efficiency,
//! clipping, standby draw, and the AC/DC coupling-path conversion stages.
//!
//! The free functions here are the explicit loss points of spec A.3.2/3.3
//! (F16): every conversion stage in every topology routes through
//! [`dc_to_ac`] / [`ac_to_dc`] so telemetry can attribute each loss.

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

/// DC -> AC conversion (discharge path or PV inversion), B.3.1/B.3.3:
/// `P_ac = P_dc * eta_inv(|P_dc|/P_rated)` hard-clamped to the AC rating;
/// the clamp bounds the OUTPUT, so excess DC input is reported as clipped.
#[must_use]
pub fn dc_to_ac(curve: &EfficiencyCurve, rated_ac_w: f64, p_dc_w: f64) -> Conversion {
    let _ = (curve, rated_ac_w, p_dc_w);
    todo!("implemented by physics task")
}

/// AC -> DC conversion (charge path), B.3.1:
/// `P_dc_draw = |P_ac_req| / eta_inv(|P_ac_req|/P_rated)`. The request is
/// clamped to the AC rating first (no clipping counter: the limit is a
/// normal operating bound, not lost energy).
#[must_use]
pub fn ac_to_dc(curve: &EfficiencyCurve, rated_ac_w: f64, p_ac_req_w: f64) -> Conversion {
    let _ = (curve, rated_ac_w, p_ac_req_w);
    todo!("implemented by physics task")
}

/// Resolve competition for shared hybrid-inverter AC capacity between PV
/// and battery discharge (B.3.3). `pv_priority` (default true) gives PV
/// first claim and curtails the battery second, matching hybrid firmware.
/// Returns `(pv_ac_w, batt_ac_w)` admitted through the inverter.
#[must_use]
pub fn resolve_shared_ac_cap(
    rated_ac_w: f64,
    pv_ac_candidate_w: f64,
    batt_ac_candidate_w: f64,
    pv_priority: bool,
) -> (f64, f64) {
    let _ = (rated_ac_w, pv_ac_candidate_w, batt_ac_candidate_w, pv_priority);
    todo!("implemented by physics task")
}

/// A live inverter instance: registry model plus standby state.
///
/// Only explicitly-declared inverters (hybrid or PV string) exist as
/// `InverterUnit`s; integrated battery inverters are folded into
/// `BatteryUnit` terminal semantics (see `battery` module docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InverterUnit {
    // Implemented by the physics task: model, rated watts, standby watts.
}

impl InverterUnit {
    /// Build from a registry model. `standby_w` comes from the system
    /// composition (B.3.2 defaults when the catalog has no explicit field:
    /// folded into battery self-discharge per Part A §5, so explicit
    /// inverter standby is usually 0 in M1).
    #[must_use]
    pub fn new(model: &InverterModel, standby_w: f64) -> Self {
        let _ = (model, standby_w);
        todo!("implemented by physics task")
    }

    /// Rated continuous AC output (W).
    #[must_use]
    pub fn rated_ac_w(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Standby draw while energized (W), AC side (B.3.2).
    #[must_use]
    pub fn standby_w(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// DC -> AC through this inverter.
    #[must_use]
    pub fn dc_to_ac(&self, p_dc_w: f64) -> Conversion {
        let _ = p_dc_w;
        todo!("implemented by physics task")
    }

    /// AC -> DC through this inverter.
    #[must_use]
    pub fn ac_to_dc(&self, p_ac_req_w: f64) -> Conversion {
        let _ = p_ac_req_w;
        todo!("implemented by physics task")
    }

    /// The registry model.
    #[must_use]
    pub fn model(&self) -> &InverterModel {
        todo!("implemented by physics task")
    }
}
