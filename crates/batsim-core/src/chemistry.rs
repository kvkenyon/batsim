//! Chemistry modules (spec B.2.4, B.2.5; F5): per-chemistry OCV tables,
//! internal-resistance modifiers, cold-temperature charge rules, coulombic
//! efficiency, and the shared thermal-derate curve.
//!
//! LFP vs NMC differences are data + small pure functions here; the battery
//! unit consumes them. All interpolation that touches LFP's flat region
//! uses Fritsch-Carlson monotone cubic (B.2.4: linear interpolation at the
//! knees is explicitly not acceptable).

use batsim_registry::Chemistry;

/// Coulombic (charge) efficiency by chemistry (spec B.2.2 defaults:
/// LFP 0.99, NMC 0.98).
#[must_use]
pub const fn eta_coul(chemistry: Chemistry) -> f64 {
    match chemistry {
        Chemistry::LFP => 0.99,
        Chemistry::NMC | Chemistry::NCA => 0.98,
    }
}

/// Open-circuit voltage at a SOC fraction and cell temperature, from the
/// per-chemistry 17-point table with Fritsch-Carlson monotone cubic
/// interpolation (spec B.2.4). Temperature shifts are applied by the
/// caller through `R_int`, not here.
#[must_use]
pub fn v_oc(chemistry: Chemistry, soc: f64) -> f64 {
    let _ = (chemistry, soc);
    todo!("implemented by physics task")
}

/// Internal resistance with SOC/temperature/aging modifiers (spec B.2.4):
/// low-SOC rise `k_soc_low = 1.5`, high-SOC rise `k_soc_hi = 0.3`, cold
/// rise `k_T_r = 0.06` per 10 degC below 25 degC, hot-side +0.5 %/degC
/// above 35 degC, and the (M4) aging factor `(1 + r_growth)`.
#[must_use]
pub fn r_int(r_base: f64, soc: f64, t_cell_c: f64, r_growth: f64) -> f64 {
    let _ = (r_base, soc, t_cell_c, r_growth);
    todo!("implemented by physics task")
}

/// Base internal resistance (ohm) for a device. Not published by any
/// catalog vendor (Part A has no field); derived from a per-chemistry
/// estimated I2R-loss-at-rated assumption, calibrated so the B.11
/// `thevenin_sag` anchor (40-60 % of nameplate deliverable at 5 % SOC,
/// -5 degC, LFP reference device) holds. Document the chosen loss fraction
/// and the calibration here.
#[must_use]
pub fn base_internal_resistance(
    chemistry: Chemistry,
    continuous_discharge_w: f64,
    nominal_pack_v: f64,
) -> f64 {
    let _ = (chemistry, continuous_discharge_w, nominal_pack_v);
    todo!("implemented by physics task")
}

/// Nominal pack voltage anchor for the OCV tables (V). Estimated per
/// chemistry class (residential LFP/NMC packs are 350-450 V class); used
/// with [`v_oc`] to scale table values.
#[must_use]
pub const fn nominal_pack_v(chemistry: Chemistry) -> f64 {
    match chemistry {
        // 400 V-class packs for both chemistries in the M1 catalog.
        Chemistry::LFP | Chemistry::NMC | Chemistry::NCA => 400.0,
    }
}

/// Charge C-rate limit factor in [0, 1] from cold-temperature chemistry
/// rules (spec B.2.5): LFP prohibits charging below 0 degC cell temp with
/// linear recovery to full at 10 degC; NMC scales from 0.1 C at -10 degC
/// to full at 10 degC. Returns the factor on the rated charge C-rate.
#[must_use]
pub fn cold_charge_factor(chemistry: Chemistry, t_cell_c: f64) -> f64 {
    let _ = (chemistry, t_cell_c);
    todo!("implemented by physics task")
}

/// Whether discharge is hard-cutoff at this temperature (NMC at -20 degC,
/// B.2.5; both chemistries dead below the B.4.4 derate floor).
#[must_use]
pub fn discharge_cutoff(chemistry: Chemistry, t_cell_c: f64) -> bool {
    let _ = (chemistry, t_cell_c);
    todo!("implemented by physics task")
}

/// Thermal derate factor on continuous power limits (spec B.4.4 piecewise
/// curve; M1 uses ambient as cell temperature).
#[must_use]
pub fn thermal_derate(t_cell_c: f64) -> f64 {
    let _ = t_cell_c;
    todo!("implemented by physics task")
}

/// Solve the Thevenin discharge current for a requested pack power
/// (spec B.2.4). On negative discriminant the request is infeasible:
/// returns the maximum-power-point current `v_oc / (2 r_int)` and sets
/// `limited = true`.
#[must_use]
pub fn thevenin_current_discharge(
    v_oc: f64,
    r_int: f64,
    p_req_w: f64,
) -> (f64, bool) {
    let _ = (v_oc, r_int, p_req_w);
    todo!("implemented by physics task")
}

/// Maximum deliverable pack power (W) under terminal-voltage cutoff:
/// solves for the current where `V_term` hits `v_min_frac * v_nominal`,
/// calibrated with [`base_internal_resistance`] so the B.11 `thevenin_sag`
/// anchor holds.
#[must_use]
pub fn thevenin_max_discharge_w(v_oc: f64, r_int: f64, v_min: f64) -> f64 {
    let _ = (v_oc, r_int, v_min);
    todo!("implemented by physics task")
}
