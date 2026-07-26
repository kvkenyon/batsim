//! Round-trip efficiency conformance (spec B.2.2, B.11 `rte_conformance`).
//!
//! Standard profile per device: charge at 0.5 C for 2 h, rest 10 min,
//! discharge at 0.5 C to cutoff. The measured AC-path round-trip
//! efficiency MUST reproduce the entry's `rte_ac_coupled` within
//! +/-0.5 percentage points (mandatory conformance).
//!
//! Path construction honors coupling (F16): AC-coupled units are measured
//! at their terminal (= AC); DC-coupled hybrid units are measured through
//! their compatible hybrid inverter (grid charge is a double conversion,
//! A.3.3). Standby/tare draw is excluded from the integral — it models
//! gateway self-consumption (B.3.2), not the conversion path.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use batsim_core::battery::{BatteryConfig, BatteryStepInput, BatteryUnit};
use batsim_registry::{BatteryModel, Coupling, Registry};

/// Measured AC-AC round-trip efficiency for one catalog battery.
fn measure_rte(registry: &Registry, model: &BatteryModel) -> f64 {
    let usable_kwh = model.usable_energy_kwh.value;
    let p_half_c_kw = 0.5 * usable_kwh;
    let p_chg_kw = p_half_c_kw.min(model.continuous_charge_power_kw.value);
    let p_dis_kw = p_half_c_kw.min(model.continuous_discharge_power_kw.value);
    // The hybrid inverter stage for DC-coupled entries (F16).
    let hybrid = if matches!(model.coupling, Coupling::DCCoupledHybrid) {
        registry
            .inverters()
            .find(|i| i.compatible_battery_ids.iter().any(|b| b == &model.model_id))
            .map(|m| batsim_core::inverter::InverterUnit::new(m, 0.0))
    } else {
        None
    };

    let mut unit = BatteryUnit::new(model, None, 0.0, 0.0, BatteryConfig::default()).unwrap();
    let mut e_chg_ac = 0.0f64;
    let mut e_dis_ac = 0.0f64;

    let mut drive = |unit: &mut BatteryUnit, p_ac_kw: f64, seconds: u64, charge: bool| {
        for _ in 0..seconds {
            // Translate the AC-side request to the unit's terminal.
            let p_term_req = match (&hybrid, charge) {
                (Some(inv), true) => -inv.ac_to_dc(p_ac_kw * 1000.0).p_out_w,
                (Some(inv), false) => {
                    let eta = inv.dc_to_ac(1000.0).p_out_w / 1000.0;
                    p_ac_kw * 1000.0 / eta.max(1e-6)
                }
                (None, true) => -p_ac_kw * 1000.0,
                (None, false) => p_ac_kw * 1000.0,
            };
            let out = unit.step(&BatteryStepInput {
                dt_s: 1,
                p_term_setpoint_w: p_term_req,
                t_amb_c: 25.0,
                grid_present: true,
            });
            // Meter the AC side of the path.
            let p_ac_realized = match &hybrid {
                Some(inv) if out.p_term_w >= 0.0 => inv.dc_to_ac(out.p_term_w).p_out_w,
                Some(_) => {
                    let eta = inv_eta_for_charge(&hybrid);
                    out.p_term_w / eta
                }
                None => out.p_term_w,
            };
            if p_ac_realized >= 0.0 {
                e_dis_ac += p_ac_realized / 3600.0;
            } else {
                e_chg_ac += -p_ac_realized / 3600.0;
            }
        }
    };

    // Charge 2 h at 0.5 C (clamped to rating), rest 10 min, then
    // discharge at 0.5 C to cutoff (guarded against non-termination).
    drive(&mut unit, p_chg_kw, 2 * 3600, true);
    drive(&mut unit, 0.0, 600, true);
    let mut guard = 0u64;
    while unit.soc() > model.soc_window.min_soc_frac + 1e-9 {
        drive(&mut unit, p_dis_kw, 1, false);
        guard += 1;
        if guard > 12 * 3600 {
            break;
        }
    }
    drop(drive);
    e_dis_ac / e_chg_ac
}

fn inv_eta_for_charge(hybrid: &Option<batsim_core::inverter::InverterUnit>) -> f64 {
    hybrid
        .as_ref()
        .map_or(1.0, |inv| inv.ac_to_dc(1000.0).p_out_w / 1000.0)
        .max(1e-6)
}

#[test]
fn rte_conformance() {
    let registry = Registry::embedded().expect("embedded registry");
    let mut failures = Vec::new();
    for model in registry.batteries() {
        if model.continuous_discharge_power_kw.value == 0.0 {
            continue; // expansion pack: not a standalone system
        }
        let Some(target) = &model.rte_ac_coupled else {
            continue;
        };
        let measured = measure_rte(&registry, model);
        let err_pp = (measured - target.value).abs() * 100.0;
        if err_pp > 0.5 {
            failures.push(format!(
                "{}: measured {:.4} vs spec {:.4} ({:.2} pp)",
                model.model_id, measured, target.value, err_pp
            ));
        }
    }
    assert!(failures.is_empty(), "RTE conformance failures:\n{}", failures.join("\n"));
}

/// Diagnostic (not a conformance gate): print measured RTE per device for
/// catalog calibration. Run: `cargo test -p batsim-core --test
/// rte_conformance rte_report -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic"]
fn rte_report() {
    let registry = Registry::embedded().expect("embedded registry");
    for model in registry.batteries() {
        if model.continuous_discharge_power_kw.value == 0.0 {
            continue;
        }
        let measured = measure_rte(&registry, model);
        let target = model.rte_ac_coupled.as_ref().map_or(f64::NAN, |t| t.value);
        println!(
            "{:35} measured {:.4}  target {:.4}  err {:+.2} pp",
            model.model_id,
            measured,
            target,
            (measured - target) * 100.0
        );
    }
}
