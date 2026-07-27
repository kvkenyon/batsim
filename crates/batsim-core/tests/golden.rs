//! Golden SOC traces per device model.
//!
//! For every catalog battery model: a fixed 48-hour scripted scenario
//! (deterministic PV curve, load archetype, diurnal ambient, two dispatch
//! commands) is stepped at dt = 1 s. The golden records per-minute SOC
//! samples, final meter counters, and a SHA-256 of the full per-tick
//! truth series - the hash makes per-tick equivalence exact, and the
//! samples make drift human-reviewable (SOC within 1e-4 of golden,
//! cumulative energy within 1e-6 relative).
//!
//! Regenerate only via `INSTA_UPDATE=always cargo test -p batsim-core
//! --test golden` with a reviewed diff.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use batsim_core::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use batsim_core::engine::SimWorld;
use batsim_registry::Registry;
use sha2::{Digest, Sha256};

const TICKS: u64 = 48 * 3600;
const SAMPLE_EVERY: u64 = 60;
const PW3: &str = "tesla.powerwall_3";

/// Apply the scripted dispatch and step the full scenario, returning the
/// snapshot summary, the per-tick truth hash, and the raw SOC samples.
///
/// Two dispatch commands: at 20:00 UTC (15:00 CDT, battery full from the
/// morning PV charge) switch to manual and hold a 3 kW discharge for the
/// rest of the scenario. The trace then exercises overnight
/// self-consumption discharge, morning PV charge to full, and a commanded
/// discharge back to the reserve floor.
fn run_scenario(mut world: SimWorld, model_label: &str) -> (serde_json::Value, String, Vec<f64>) {
    world
        .dispatch(
            0,
            ScheduledDispatch {
                execute_at_tick: 20 * 3600,
                action: DispatchAction::SetMode(ControlMode::Manual),
                tag: 0,
            },
        )
        .unwrap();
    world
        .dispatch(
            0,
            ScheduledDispatch {
                execute_at_tick: 20 * 3600,
                action: DispatchAction::SetManualSetpoint(3000.0),
                tag: 0,
            },
        )
        .unwrap();

    let mut hasher = Sha256::new();
    let mut soc_samples = Vec::new();
    for _ in 0..TICKS {
        world.step();
        let home = world.home(0).unwrap();
        let tick = world.clock().tick();
        let truth = home.truth().last().unwrap();
        hasher.update(serde_json::to_vec(truth).unwrap());
        if tick % SAMPLE_EVERY == 0 {
            soc_samples.push((truth.soc_mean * 1e6).round() / 1e6);
        }
    }
    let home = world.home(0).unwrap();
    let meters = home.meters();
    let summary = serde_json::json!({
        "model_id": model_label,
        "soc_samples_per_min": soc_samples,
        "final_soc": (home.soc_mean() * 1e6).round() / 1e6,
        "main_import_kwh": (meters.main.import_wh / 1000.0 * 1e6).round() / 1e6,
        "main_export_kwh": (meters.main.export_wh / 1000.0 * 1e6).round() / 1e6,
        "batt_import_kwh": (meters.batt_ac.import_wh / 1000.0 * 1e6).round() / 1e6,
        "batt_export_kwh": (meters.batt_ac.export_wh / 1000.0 * 1e6).round() / 1e6,
        "pv_kwh": (meters.pv_ac.wh / 1000.0 * 1e6).round() / 1e6,
        "standby_kwh": (meters.standby_loss.wh / 1000.0 * 1e6).round() / 1e6,
    });
    (summary, format!("{:x}", hasher.finalize()), soc_samples)
}

/// SOC sample at `hh`:00 UTC (samples land on whole minutes from tick 60).
fn soc_at(samples: &[f64], hh: usize) -> f64 {
    samples[hh * 3600 / SAMPLE_EVERY as usize - 1]
}

#[test]
fn golden_soc_traces() {
    let registry = Registry::embedded().expect("embedded registry");
    for model in registry.batteries() {
        // Expansion packs are not standalone systems.
        if model.continuous_discharge_power_kw.value == 0.0 {
            continue;
        }
        let world = common::build_world(&registry, &model.model_id, 1, 0xB4751, true, true);
        let (summary, hash, _) = run_scenario(world, &model.model_id);
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path("golden");
        settings.set_snapshot_suffix(model.model_id.replace('.', "_"));
        settings.bind(|| {
            insta::assert_json_snapshot!(summary);
            insta::assert_snapshot!(hash);
        });
    }
}

/// Head unit plus one energy-only expansion pack on the same scripted
/// scenario: doubled usable energy behind unchanged power limits. The
/// snapshot pins the full trace; the assertions pin the structural
/// relationships the snapshot alone leaves implicit.
#[test]
fn golden_pw3_head_plus_expansion_pack() {
    let registry = Registry::embedded().expect("embedded registry");
    let pack_spec = common::one_battery_system_with_packs(&registry, PW3, 1, true);
    let pack_world = common::build_world_with(&registry, &pack_spec, 1, 0xB4751, true, true);
    let (summary, hash, pack_soc) = run_scenario(pack_world, "tesla.powerwall_3+1_pack");

    // Head-only reference over the identical scenario: same realized
    // powers, half the usable energy.
    let head_world = common::build_world(&registry, PW3, 1, 0xB4751, true, true);
    let (head_summary, _, head_soc) = run_scenario(head_world, PW3);

    // SOC slope: over the commanded 3 kW discharge while both systems sit
    // clear of their windows (20:00-22:00), the energy drawn is identical,
    // so the head's SOC falls twice as fast as the doubled-window system's.
    let head_drop = soc_at(&head_soc, 20) - soc_at(&head_soc, 22);
    let pack_drop = soc_at(&pack_soc, 20) - soc_at(&pack_soc, 22);
    let ratio = head_drop / pack_drop;
    assert!(
        (1.9..2.1).contains(&ratio),
        "head drop {head_drop} vs pack drop {pack_drop}: ratio {ratio} != 2"
    );

    // Reserve floor: the same reserve fraction now spans twice the
    // energy; the long discharge still parks the pack system exactly on
    // the floor, never below it.
    let final_soc = summary["final_soc"].as_f64().unwrap();
    assert!(
        (final_soc - 0.2).abs() < 1e-6,
        "pack system ended at {final_soc}, expected the 0.2 reserve floor"
    );

    // Energy accounting: every pack kilowatt-hour moves through the head
    // unit's inverter and meter, so the pack system exports strictly more
    // battery energy than the head-only run of the same scenario.
    let pack_export = summary["batt_export_kwh"].as_f64().unwrap();
    let head_export = head_summary["batt_export_kwh"].as_f64().unwrap();
    assert!(
        pack_export > head_export,
        "pack export {pack_export} kWh did not exceed head-only {head_export} kWh"
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("golden");
    settings.set_snapshot_suffix("tesla_powerwall_3_plus_expansion_pack");
    settings.bind(|| {
        insta::assert_json_snapshot!(summary);
        insta::assert_snapshot!(hash);
    });
}
