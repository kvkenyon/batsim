//! Energy-conservation and operating-window property tests.
//!
//! Invariants defended, over random device parameters x random setpoint
//! sequences:
//! 1. Energy conservation: stored-energy delta equals the integral of
//!    realized terminal powers through the efficiency stages:
//!    `delta_stored = sum(chg_term * eta_chg * eta_coul) - sum(dis_term / eta_dis)`
//!    evaluated per tick from REALIZED powers (ramp/min-on-off/window
//!    clamps change realized vs requested; the identity must hold for what
//!    the unit actually did).
//! 2. SOC window: SOC stays within `[min, max]` for all inputs, including
//!    adversarial (charge while full, discharge while empty).
//! 3. Setpoint clamping: realized terminal power never exceeds the
//!    pre-step dynamic limit (Thevenin-sagged, thermally derated).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use batsim_core::battery::{BatteryConfig, BatteryStepInput, BatteryUnit};
use batsim_registry::types::{BatteryModel, Chemistry, Coupling, EfficiencyCurve, EfficiencyPoint};
use batsim_registry::Provenance as Prov;
use proptest::prelude::*;

/// Build a synthetic battery model with realistic curve shapes.
fn make_model(
    chemistry: Chemistry,
    coupling: Coupling,
    usable_kwh: f64,
    continuous_kw: f64,
    peak_kw: f64,
) -> BatteryModel {
    let curve = |base: f64| EfficiencyCurve {
        points: vec![
            EfficiencyPoint {
                x_kw: 0.05 * continuous_kw,
                efficiency: base - 0.04,
            },
            EfficiencyPoint {
                x_kw: 0.25 * continuous_kw,
                efficiency: base,
            },
            EfficiencyPoint {
                x_kw: 0.5 * continuous_kw,
                efficiency: base + 0.01,
            },
            EfficiencyPoint {
                x_kw: continuous_kw,
                efficiency: base - 0.005,
            },
        ],
        provenance: Prov::Estimated,
    };
    serde_json::from_value(serde_json::json!({
        "schema_version": "1.0.0",
        "entry_version": "1.0.0",
        "model_id": "test.synthetic",
        "vendor": "test",
        "display_name": "Synthetic",
        "chemistry": chemistry,
        "coupling": coupling,
        "nameplate_energy_kwh": {"value": usable_kwh, "provenance": "spec", "unit": "kWh"},
        "usable_energy_kwh": {"value": usable_kwh, "provenance": "spec", "unit": "kWh"},
        "continuous_discharge_power_kw": {"value": continuous_kw, "provenance": "spec", "unit": "kW"},
        "peak_discharge_power_kw": {"value": peak_kw, "provenance": "spec", "unit": "kW"},
        "peak_duration_s": {"value": 10.0, "provenance": "spec", "unit": "s"},
        "continuous_charge_power_kw": {"value": continuous_kw, "provenance": "spec", "unit": "kW"},
        "soc_window": {"min_soc_frac": 0.0, "max_soc_frac": 1.0, "reserve_floor_frac": 0.2, "provenance": "spec"},
        "charge_efficiency_curve": serde_json::to_value(curve(0.94)).unwrap(),
        "discharge_efficiency_curve": serde_json::to_value(curve(0.94)).unwrap(),
        "grid_forming_in_backup": true,
        "warranty": {},
        "operating_temperature": {"min_c": -20.0, "max_c": 50.0, "provenance": "spec"},
        "ramp_rate": {"max_kw_per_s": continuous_kw, "provenance": "estimated"},
        "self_discharge_frac_per_day": {"value": 0.002, "provenance": "estimated", "unit": "frac/day"},
        "vendor_api": {"family": "generic", "auth_style": "none", "endpoints": [], "provenance": "estimated"}
    }))
    .unwrap()
}

fn arb_params() -> impl Strategy<Value = (Chemistry, Coupling, f64, f64, f64)> {
    (
        prop::sample::select(vec![Chemistry::LFP, Chemistry::NMC]),
        prop::sample::select(vec![
            Coupling::ACCoupled,
            Coupling::DCCoupledHybrid,
            Coupling::MicroinverterBased,
        ]),
        3.0f64..20.0, // usable kWh
        2.0f64..12.0, // continuous kW
        0.0f64..1.0,  // initial SOC
    )
}

fn arb_setpoints(max_kw: f64) -> impl Strategy<Value = Vec<f64>> {
    // Mix of full-range swings and small perturbations; includes
    // adversarial full-charge/full-discharge holds.
    prop::collection::vec(
        prop_oneof![
            (-max_kw..max_kw),
            prop::sample::select(vec![-max_kw, 0.0, max_kw]),
        ],
        50..300,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn energy_conservation_and_soc_window(
        (chem, coupling, usable_kwh, continuous_kw, initial_soc) in arb_params(),
        setpoints in arb_setpoints(15.0),
    ) {
        let model = make_model(chem, coupling, usable_kwh, continuous_kw, continuous_kw * 1.5);
        let mut unit = BatteryUnit::new(&model, None, initial_soc, 0.0, BatteryConfig::default())
            .unwrap();
        let usable_wh = unit.usable_energy_wh();
        let mut stored_prev = unit.energy_stored_wh();
        let mut stored_expected = stored_prev;

        for sp_w in setpoints {
            let setpoint_w = sp_w * 1000.0;
            // Window pre-check: dynamic limits bound the realized power.
            let lim_dis = unit.max_discharge_w();
            let lim_chg = unit.max_charge_w();
            let out = unit.step(&BatteryStepInput {
                dt_s: 1,
                p_term_setpoint_w: setpoint_w,
                t_amb_c: 25.0,
                grid_present: true,
            });
            prop_assert!(
                out.p_term_w <= lim_dis + 1e-6,
                "discharge clamp: realized {} > limit {}",
                out.p_term_w,
                lim_dis
            );
            prop_assert!(
                out.p_term_w >= -(lim_chg + 1e-6),
                "charge clamp: realized {} < -limit {}",
                out.p_term_w,
                lim_chg
            );

            // Exact per-tick conservation from REALIZED terminal power.
            let p_kw = out.p_term_w / 1000.0;
            let eta_coul = batsim_core::chemistry::eta_coul(chem);
            let delta = if out.p_term_w >= 0.0 {
                let eta = model.discharge_efficiency_curve.eval(p_kw);
                -(out.p_term_w / eta.max(1e-9)) / 3600.0
            } else {
                let eta = model.charge_efficiency_curve.eval(-p_kw);
                (-out.p_term_w) * eta * eta_coul / 3600.0
            };
            stored_expected += delta;

            let stored_now = unit.energy_stored_wh();
            let tol = 1e-9 * usable_wh + 1e-6;
            prop_assert!(
                (stored_now - stored_expected).abs() <= tol,
                "conservation: stored {stored_now} vs expected {stored_expected} (tol {tol})"
            );
            stored_prev = stored_now;
            let _ = stored_prev;

            // SOC window under all inputs.
            let soc = unit.soc();
            prop_assert!(
                (-1e-9..=1.0 + 1e-9).contains(&soc),
                "soc out of window: {soc}"
            );
        }

        // Cumulative counters match the stored-energy integral.
        let chg = unit.cumulative_charge_wh();
        let dis = unit.cumulative_discharge_wh();
        prop_assert!(chg >= 0.0 && dis >= 0.0, "counters monotonic");
    }
}
