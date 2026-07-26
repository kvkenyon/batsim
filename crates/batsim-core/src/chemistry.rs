//! Chemistry modules (spec B.2.4, B.2.5; F5): per-chemistry OCV tables,
//! internal-resistance modifiers, cold-temperature charge rules, coulombic
//! efficiency, and the shared thermal-derate curve.
//!
//! LFP vs NMC differences are data + small pure functions here; the battery
//! unit consumes them. All interpolation that touches LFP's flat region
//! uses Fritsch-Carlson monotone cubic (B.2.4: linear interpolation at the
//! knees is explicitly not acceptable).
//!
//! # Pack scaling
//!
//! The tables below are cell voltages scaled to a nominal pack voltage of
//! 400 V (spec B.2.4): the per-chemistry column is
//! `v_cell(soc) * (400 / v_cell_nominal)` with `v_cell_nominal = 3.2 V`
//! (LFP) and `3.6 V` (NMC), so both tables sit on the same 400 V pack
//! scale and chemistry differences live in the *shape* only.
//!
//! The SOC axis is the *usable* window fraction (what the unit reports).
//! Both tables therefore start slightly ABOVE the pack cutoff voltage
//! (`v_min = 340 V`, see [`base_internal_resistance`]): the bottom of a
//! vendor's usable window is a firmware floor, not absolute empty. This
//! keeps the Thevenin voltage-cutoff model from stranding usable energy
//! near SOC 0 (the steep LFP knee discharges the last percent quickly).
//! NMC's floor sits higher still (366 V): its shallow slope would
//! otherwise leave the cutoff regime spanning the last ~15 % of usable
//! energy even at room temperature.

use batsim_registry::Chemistry;

/// Nominal pack voltage the OCV tables are scaled to (V, spec B.2.4).
pub const NOMINAL_PACK_V: f64 = 400.0;

/// SOC abscissa of the 17-point OCV tables: 0, 6.25, 12.5, ..., 100 %
/// (fractions 0.0, 0.0625, ..., 1.0).
pub const OCV_SOC_POINTS: usize = 17;

/// LFP pack OCV table (V) at the 17 SOC abscissa points, 400 V pack scale.
///
/// Shape requirements (spec B.2.4): a flat mid-range (<= 3 % swing over
/// SOC 15-90 %, realized here as ~2.8 %) with distinct knees below 10 %
/// and above 95 % SOC.
pub const LFP_OCV_V: [f64; OCV_SOC_POINTS] = [
    341.00, 393.75, 412.50, 414.38, 415.94, 417.19, 418.13, 418.91, 419.53, 420.16, 420.78, 421.41,
    422.03, 422.66, 423.28, 424.69, 462.50,
];

/// NMC pack OCV table (V) at the 17 SOC abscissa points, 400 V pack scale.
///
/// Shape requirement (spec B.2.4): near-linear with a 15-20 % swing
/// across the window (realized as ~16.9 %). The 0 % point sits at 366 V
/// (3.29 V/cell): NMC vendors float the usable floor well above the pack
/// cutoff, which keeps the Thevenin model from throttling the last
/// ~10 % of usable energy at 25 degC (B.11 rte_conformance).
pub const NMC_OCV_V: [f64; OCV_SOC_POINTS] = [
    366.00, 369.70, 373.50, 377.20, 380.90, 384.50, 388.00, 391.40, 394.80, 398.10, 401.30, 404.60,
    407.90, 411.30, 415.00, 421.00, 428.00,
];

/// The 17 SOC abscissa points (fractions in `[0, 1]`).
const OCV_SOC_FRAC: [f64; OCV_SOC_POINTS] = [
    0.0, 0.0625, 0.125, 0.1875, 0.25, 0.3125, 0.375, 0.4375, 0.5, 0.5625, 0.625, 0.6875, 0.75,
    0.8125, 0.875, 0.9375, 1.0,
];

/// OCV table (pack volts) for a chemistry; NCA shares the NMC table.
#[must_use]
pub const fn ocv_table(chemistry: Chemistry) -> &'static [f64; OCV_SOC_POINTS] {
    match chemistry {
        Chemistry::LFP => &LFP_OCV_V,
        Chemistry::NMC | Chemistry::NCA => &NMC_OCV_V,
    }
}

/// Fritsch-Carlson monotone cubic (PCHIP) evaluation on the OCV table.
///
/// Classic PCHIP: secant slopes `d_i`, tangents `m_i` set to zero at local
/// extrema and harmonic-mean limited elsewhere, then the cubic Hermite
/// basis per interval. Monotonic input data yield a monotone interpolant;
/// the zero-tangent rule prevents the overshoot linear interpolation
/// would hide at LFP's knees.
fn fritsch_carlson_eval(xs: &[f64], ys: &[f64], x_eval: f64) -> f64 {
    let n_pts = xs.len();
    debug_assert!(n_pts >= 2 && ys.len() == n_pts);
    // Clamp outside the sampled range.
    if x_eval <= xs[0] {
        return ys[0];
    }
    if x_eval >= xs[n_pts - 1] {
        return ys[n_pts - 1];
    }
    // Secant slopes.
    let mut secant = [0.0_f64; OCV_SOC_POINTS - 1];
    for idx in 0..n_pts - 1 {
        secant[idx] = (ys[idx + 1] - ys[idx]) / (xs[idx + 1] - xs[idx]);
    }
    // Tangents (Fritsch-Carlson / PCHIP limiting).
    let mut tangent = [0.0_f64; OCV_SOC_POINTS];
    tangent[0] = secant[0];
    tangent[n_pts - 1] = secant[n_pts - 2];
    for idx in 1..n_pts - 1 {
        if secant[idx - 1] * secant[idx] <= 0.0 {
            tangent[idx] = 0.0;
        } else {
            let w1 = 2.0 * (xs[idx + 1] - xs[idx]) + (xs[idx] - xs[idx - 1]);
            let w2 = (xs[idx + 1] - xs[idx]) + 2.0 * (xs[idx] - xs[idx - 1]);
            tangent[idx] = (w1 + w2) / (w1 / secant[idx - 1] + w2 / secant[idx]);
        }
    }
    // Locate interval.
    let mut idx = 0;
    while idx < n_pts - 2 && x_eval > xs[idx + 1] {
        idx += 1;
    }
    // Cubic Hermite basis.
    let width = xs[idx + 1] - xs[idx];
    let frac = (x_eval - xs[idx]) / width;
    let t2 = frac * frac;
    let t3 = t2 * frac;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + frac;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * ys[idx] + h10 * width * tangent[idx] + h01 * ys[idx + 1] + h11 * width * tangent[idx + 1]
}

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
    let soc = soc.clamp(0.0, 1.0);
    fritsch_carlson_eval(&OCV_SOC_FRAC, ocv_table(chemistry), soc)
}

/// Base internal resistance (ohm) of the nominal 400 V pack, derived from
/// an estimated I^2R loss at the rated continuous pack power
/// (spec B.2.4 "estimated where not published").
///
/// # Calibration (chosen fraction, voltage-cutoff model)
///
/// * **Loss fraction**: `R_base` dissipates 7.7 % of `p_rated_w` as I^2R
///   heat at the rated continuous current, `I_rated = p_rated_w /
///   NOMINAL_PACK_V`, giving `R_base = 0.077 * p_rated_w / I_rated^2`.
///   7.7 % is an estimated effective pack resistance (cells + contactors
///   + harness), calibrated so the B.11 `thevenin_sag` anchor lands near
///   the middle of its window: for the LFP reference device
///   (`tesla.powerwall_3`, 11.5 kW continuous) at 5 % SOC and -5 degC the
///   deliverable power is ~54 % of nameplate continuous (required:
///   40-60 %).
/// * **Voltage-cutoff model**: [`thevenin_max_discharge_w`] enforces the
///   terminal-voltage cutoff `V_term >= v_min` with `v_min =
///   V_MIN_CUTOFF_FRAC * NOMINAL_PACK_V` (0.85 * 400 = 340 V). Power
///   requested below the cutoff is unavailable; the clamp reports the
///   power at which the terminal would sit exactly at `v_min`.
#[must_use]
pub fn base_internal_resistance(p_rated_w: f64) -> f64 {
    /// Estimated effective I^2R loss fraction at rated continuous power;
    /// see the rustdoc above for calibration against B.11 `thevenin_sag`.
    const IR_LOSS_FRAC_AT_RATED: f64 = 0.077;
    if p_rated_w <= 0.0 {
        return 0.0;
    }
    let i_rated = p_rated_w / NOMINAL_PACK_V;
    IR_LOSS_FRAC_AT_RATED * p_rated_w / (i_rated * i_rated)
}

/// Terminal-voltage cutoff as a fraction of the nominal pack voltage
/// (spec B.2.4 estimated model; see [`base_internal_resistance`]).
pub const V_MIN_CUTOFF_FRAC: f64 = 0.85;

/// Internal resistance with SOC/temperature/aging modifiers (spec B.2.4):
/// low-SOC rise `k_soc_low = 1.5`, high-SOC rise `k_soc_hi = 0.3`, cold
/// rise `k_T_r = 0.06` per 10 degC below 25 degC, hot rise 0.5 %/degC
/// above 35 degC, and aging growth `r_growth` (M1: always 0, F8 is M4).
///
/// Exact form:
///
/// ```text
/// R_int = R_base
///     * (1 + k_soc_low * max(0, 0.15 - soc) / 0.15)
///     * (1 + k_soc_hi  * max(0, soc - 0.95) / 0.05)
///     * (1 + k_T_r     * max(0, 25 - T_cell) / 10)
///     * (1 + 0.005     * max(0, T_cell - 35))
///     * (1 + r_growth)
/// ```
#[must_use]
pub fn r_int(r_base_ohm: f64, soc: f64, r_growth: f64, cell_c: f64) -> f64 {
    /// Low-SOC resistance-rise coefficient (spec B.2.4).
    const K_SOC_LOW: f64 = 1.5;
    /// High-SOC resistance-rise coefficient (spec B.2.4).
    const K_SOC_HI: f64 = 0.3;
    /// Cold resistance rise per 10 degC below 25 degC (spec B.2.4).
    const K_T_R: f64 = 0.06;
    /// Hot resistance rise per degC above 35 degC (spec B.2.4).
    const K_T_HOT_PER_C: f64 = 0.005;
    let soc_term = 1.0 + K_SOC_LOW * (0.15 - soc).max(0.0) / 0.15;
    let soc_hi_term = 1.0 + K_SOC_HI * (soc - 0.95).max(0.0) / 0.05;
    let cold_term = 1.0 + K_T_R * (25.0 - cell_c).max(0.0) / 10.0;
    let hot_term = 1.0 + K_T_HOT_PER_C * (cell_c - 35.0).max(0.0);
    r_base_ohm * soc_term * soc_hi_term * cold_term * hot_term * (1.0 + r_growth)
}

/// Solve the Thevenin discharge current for a requested pack power
/// (spec B.2.4). On negative discriminant the request is infeasible:
/// returns the maximum-power-point current `v_oc / (2 r_int)` and sets
/// `limited = true`.
///
/// The quadratic `V_oc*I - I^2*R = P_req` has roots
/// `I = (V_oc +/- sqrt(V_oc^2 - 4 R P_req)) / (2 R)`; the smaller root is
/// the physical (low-current) operating point. Degenerate `r_int <= 0`
/// collapses to the lossless `I = P_req / V_oc`.
#[must_use]
pub fn thevenin_current_discharge(v_oc: f64, r_int: f64, p_req_w: f64) -> (f64, bool) {
    if p_req_w <= 0.0 {
        return (0.0, false);
    }
    if r_int <= 0.0 || v_oc <= 0.0 {
        return (p_req_w / v_oc.max(1.0), false);
    }
    let disc = v_oc.mul_add(v_oc, -4.0 * r_int * p_req_w);
    if disc < 0.0 {
        // Infeasible: maximum-power-point current.
        return (v_oc / (2.0 * r_int), true);
    }
    ((v_oc - disc.sqrt()) / (2.0 * r_int), false)
}

/// Maximum deliverable pack power (W) under terminal-voltage cutoff:
/// solves for the current where `V_term` hits `v_min_frac * v_nominal`,
/// calibrated with [`base_internal_resistance`] so the B.11 `thevenin_sag`
/// anchor holds.
///
/// `V_term = V_oc - I*R >= v_min` gives `I_max = (V_oc - v_min) / R` and
/// `P_max = I_max * v_min`; the quadratic voltage-drop bound is the active
/// one under sag, so the resistive maximum-power point is never the
/// limiting constraint.
#[must_use]
pub fn thevenin_max_discharge_w(v_oc: f64, r_int: f64, v_min: f64) -> f64 {
    if v_oc <= 0.0 || v_min <= 0.0 {
        return 0.0;
    }
    if r_int <= 0.0 {
        // No sag: the cutoff only binds when v_oc itself is below v_min.
        return if v_oc >= v_min { f64::INFINITY } else { 0.0 };
    }
    if v_oc <= v_min {
        return 0.0;
    }
    let i_max = (v_oc - v_min) / r_int;
    i_max * v_min
}

/// Charge-acceptance limit factor vs cell temperature (spec B.2.5).
///
/// LFP: charge is prohibited below 0 degC, ramping linearly 0 -> full
/// over 0-10 degC. NMC/NCA: derated to a 0.1 C floor at -10 degC, ramping
/// linearly to full at 10 degC; no hard prohibition.
#[must_use]
pub fn cold_charge_factor(chemistry: Chemistry, cell_c: f64) -> f64 {
    match chemistry {
        Chemistry::LFP => {
            if cell_c < 0.0 {
                0.0
            } else if cell_c < 10.0 {
                cell_c / 10.0
            } else {
                1.0
            }
        }
        Chemistry::NMC | Chemistry::NCA => {
            if cell_c < -10.0 {
                0.1
            } else if cell_c < 10.0 {
                (cell_c + 10.0).mul_add(0.9 / 20.0, 0.1)
            } else {
                1.0
            }
        }
    }
}

/// Hard discharge cutoff temperature (degC), if the chemistry has one
/// (spec B.2.5): NMC/NCA discharge must stop below -20 degC; LFP relies
/// on the thermal derate reaching zero instead (it already hits 0 at
/// -20 degC, B.4.4).
#[must_use]
pub const fn discharge_cutoff_c(chemistry: Chemistry) -> Option<f64> {
    match chemistry {
        Chemistry::LFP => None,
        Chemistry::NMC | Chemistry::NCA => Some(-20.0),
    }
}

/// Shared thermal derate factor `d_T` vs cell temperature (spec B.4.4
/// piecewise curve, exact):
///
/// ```text
/// 0.0                        T_cell < -20
/// linear 0.5 -> 1.0     -20 <= T_cell < 0
/// 1.0                        0 <= T_cell <= 40
/// linear 1.0 -> 0.6      40 <  T_cell <= 55
/// 0.6 -> 0.0 (linear)    55 <  T_cell <= 65
/// 0.0 (trip)                  T_cell > 65
/// ```
///
/// The overtemp latch / `ThermalTrip` event pair is state-machine business
/// of the unit (M4), not this pure curve; M1 clamps continuously.
#[must_use]
pub fn thermal_derate(cell_c: f64) -> f64 {
    if cell_c < -20.0 {
        0.0
    } else if cell_c < 0.0 {
        (cell_c + 20.0).mul_add(0.5 / 20.0, 0.5)
    } else if cell_c <= 40.0 {
        1.0
    } else if cell_c <= 55.0 {
        (cell_c - 40.0).mul_add(-0.4 / 15.0, 1.0)
    } else if cell_c <= 65.0 {
        0.6 * (65.0 - cell_c) / 10.0
    } else {
        0.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
    }

    #[test]
    fn ocv_tables_match_shape_requirements() {
        // LFP: <= 3 % swing over SOC 15-90 % of the mid-range level.
        let v15 = v_oc(Chemistry::LFP, 0.15);
        let v90 = v_oc(Chemistry::LFP, 0.90);
        assert!(
            (v90 - v15) / v15 <= 0.03,
            "LFP mid swing {}",
            (v90 - v15) / v15
        );
        // Distinct knees: a fast voltage move below 10 % and above 95 %
        // against a barely-sloped mid-range.
        let knee_lo = v_oc(Chemistry::LFP, 0.10) - v_oc(Chemistry::LFP, 0.0);
        assert!(knee_lo > 25.0, "LFP low knee {knee_lo} V over 10 %");
        assert!(knee_lo > 10.0 * (v_oc(Chemistry::LFP, 0.20) - v_oc(Chemistry::LFP, 0.15)));
        let knee_hi = v_oc(Chemistry::LFP, 1.0) - v_oc(Chemistry::LFP, 0.95);
        assert!(knee_hi > 25.0, "LFP high knee {knee_hi} V over 5 %");
        assert!(knee_hi > 10.0 * (v_oc(Chemistry::LFP, 0.90) - v_oc(Chemistry::LFP, 0.85)));
        // NMC: near-linear, 15-20 % swing across the window.
        let swing =
            (v_oc(Chemistry::NMC, 1.0) - v_oc(Chemistry::NMC, 0.0)) / v_oc(Chemistry::NMC, 0.0);
        assert!((0.15..=0.20).contains(&swing), "NMC swing {swing}");
        // NCA shares the NMC table.
        approx(v_oc(Chemistry::NCA, 0.5), v_oc(Chemistry::NMC, 0.5), 0.0);
    }

    #[test]
    fn ocv_interpolation_is_monotone_and_hits_nodes() {
        for chem in [Chemistry::LFP, Chemistry::NMC] {
            let table = ocv_table(chem);
            for (i, &v) in table.iter().enumerate() {
                let soc = f64::from(i as u32) * 0.0625;
                approx(v_oc(chem, soc), v, 1e-9);
            }
            let mut prev = v_oc(chem, 0.0);
            for k in 1..=1000 {
                let v = v_oc(chem, f64::from(k) / 1000.0);
                assert!(v >= prev, "non-monotone at {k}: {prev} -> {v}");
                prev = v;
            }
        }
    }

    #[test]
    fn r_int_modifiers_match_b_2_4() {
        let r = base_internal_resistance(11_500.0);
        // Unity at soc 0.5, 25 degC, no growth.
        approx(r_int(r, 0.5, 0.0, 25.0), r, 1e-12);
        // Low-SOC: 1.5 * (0.15 - soc)/0.15 at soc <= 0.15.
        approx(r_int(r, 0.05, 0.0, 25.0), r * 2.0, 1e-12);
        approx(r_int(r, 0.15, 0.0, 25.0), r, 1e-12);
        // High-SOC: 0.3 * (soc - 0.95)/0.05 above 0.95.
        approx(r_int(r, 1.0, 0.0, 25.0), r * 1.3, 1e-12);
        // Cold: 0.06 per 10 degC below 25.
        approx(r_int(r, 0.5, 0.0, 15.0), r * 1.06, 1e-12);
        approx(r_int(r, 0.5, 0.0, -5.0), r * 1.18, 1e-12);
        // Hot: +0.5 %/degC above 35.
        approx(r_int(r, 0.5, 0.0, 45.0), r * 1.05, 1e-12);
        // Growth multiplies.
        approx(r_int(r, 0.5, 0.1, 25.0), r * 1.1, 1e-12);
    }

    #[test]
    fn thevenin_sag_anchor_b11() {
        // PW3-shaped LFP unit: 11.5 kW continuous, 5 % SOC, -5 degC must
        // deliver 40-60 % of nameplate continuous (spec B.11 thevenin_sag).
        let p_cont_w = 11_500.0;
        let r_base = base_internal_resistance(p_cont_w);
        let r = r_int(r_base, 0.05, 0.0, -5.0);
        let v = v_oc(Chemistry::LFP, 0.05);
        let v_min = V_MIN_CUTOFF_FRAC * NOMINAL_PACK_V;
        let p_max = thevenin_max_discharge_w(v, r, v_min);
        let frac = p_max / p_cont_w;
        assert!(
            (0.40..=0.60).contains(&frac),
            "thevenin sag fraction {frac} (p_max {p_max} W) outside 40-60 %"
        );
        // Warm, mid-SOC: Thevenin leaves the full rating available.
        let p_warm = thevenin_max_discharge_w(
            v_oc(Chemistry::LFP, 0.5),
            r_int(r_base, 0.5, 0.0, 25.0),
            v_min,
        );
        assert!(p_warm > 2.0 * p_cont_w, "warm sag {p_warm}");
    }

    #[test]
    fn thevenin_current_solves_quadratic_with_clamp() {
        let (i, limited) = thevenin_current_discharge(400.0, 1.0, 10_000.0);
        assert!(!limited);
        // V_oc*I - I^2 R = P_req.
        approx(400.0_f64.mul_add(i, -(i * i)), 10_000.0, 1e-6);
        // Infeasible request -> max-power-point current, limited.
        let (i_max, limited) = thevenin_current_discharge(400.0, 1.0, 1e9);
        assert!(limited);
        approx(i_max, 200.0, 1e-12);
        // Zero request and zero resistance degenerate sanely.
        assert_eq!(thevenin_current_discharge(400.0, 1.0, 0.0), (0.0, false));
        let (i, limited) = thevenin_current_discharge(400.0, 0.0, 8_000.0);
        assert!(!limited);
        approx(i, 20.0, 1e-12);
    }

    #[test]
    fn lfp_cold_charge_block_and_nmc_derate_b11() {
        // LFP: charge power = 0 below 0 degC (spec B.11 lfp_cold_charge_block).
        assert_eq!(cold_charge_factor(Chemistry::LFP, -5.0), 0.0);
        assert_eq!(cold_charge_factor(Chemistry::LFP, -0.001), 0.0);
        // Linear recovery 0 -> 10 degC.
        assert_eq!(cold_charge_factor(Chemistry::LFP, 0.0), 0.0);
        approx(cold_charge_factor(Chemistry::LFP, 5.0), 0.5, 1e-12);
        assert_eq!(cold_charge_factor(Chemistry::LFP, 10.0), 1.0);
        assert_eq!(cold_charge_factor(Chemistry::LFP, 30.0), 1.0);
        // NMC: derated only (never blocked), 0.1 C at -10 degC.
        approx(cold_charge_factor(Chemistry::NMC, -10.0), 0.1, 1e-12);
        approx(cold_charge_factor(Chemistry::NMC, -30.0), 0.1, 1e-12);
        approx(cold_charge_factor(Chemistry::NMC, 0.0), 0.55, 1e-12);
        assert_eq!(cold_charge_factor(Chemistry::NMC, 10.0), 1.0);
        // NMC hard discharge cutoff at -20 degC; LFP has none.
        assert_eq!(discharge_cutoff_c(Chemistry::NMC), Some(-20.0));
        assert_eq!(discharge_cutoff_c(Chemistry::NCA), Some(-20.0));
        assert_eq!(discharge_cutoff_c(Chemistry::LFP), None);
    }

    #[test]
    fn thermal_derate_matches_b_4_4_piecewise() {
        assert_eq!(thermal_derate(-21.0), 0.0);
        assert_eq!(thermal_derate(-20.0), 0.5);
        approx(thermal_derate(-10.0), 0.75, 1e-12);
        assert_eq!(thermal_derate(0.0), 1.0);
        assert_eq!(thermal_derate(25.0), 1.0);
        assert_eq!(thermal_derate(40.0), 1.0);
        approx(thermal_derate(47.5), 0.8, 1e-12);
        assert_eq!(thermal_derate(55.0), 0.6);
        approx(thermal_derate(60.0), 0.3, 1e-12);
        assert_eq!(thermal_derate(65.0), 0.0);
        assert_eq!(thermal_derate(70.0), 0.0);
    }

    #[test]
    fn eta_coul_values() {
        assert_eq!(eta_coul(Chemistry::LFP), 0.99);
        assert_eq!(eta_coul(Chemistry::NMC), 0.98);
        assert_eq!(eta_coul(Chemistry::NCA), 0.98);
    }
}
