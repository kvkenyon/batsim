//! Golden SOC traces per device model (spec C.7.1; M1 exit criterion 1).
//!
//! For every catalog battery model: a fixed 48-hour scripted scenario
//! (deterministic PV curve, load archetype, diurnal ambient, two dispatch
//! commands) is stepped at dt = 1 s. The golden records per-minute SOC
//! samples, final meter counters, and a SHA-256 of the full per-tick
//! truth series — the hash makes per-tick equivalence exact, and the
//! samples make drift human-reviewable (spec: SOC within 1e-4 of golden,
//! cumulative energy within 1e-6 relative).
//!
//! Regenerate only via `INSTA_UPDATE=always cargo test -p batsim-core
//! --test golden` with a reviewed diff.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use batsim_core::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use batsim_registry::Registry;
use sha2::{Digest, Sha256};

const TICKS: u64 = 48 * 3600;
const SAMPLE_EVERY: u64 = 60;

fn golden_for(model_id: &str) -> (serde_json::Value, String) {
    let registry = Registry::embedded().expect("embedded registry");
    let mut world = common::build_world(&registry, model_id, 1, 0xB4751, true, true);
    // Two dispatch commands (C.7.1): switch to manual at 11:00 and hold a
    // 3 kW discharge from the battery for the rest of the scenario.
    world
        .dispatch(
            0,
            ScheduledDispatch {
                execute_at_tick: 11 * 3600,
                action: DispatchAction::SetMode(ControlMode::Manual),
            },
        )
        .unwrap();
    world
        .dispatch(
            0,
            ScheduledDispatch {
                execute_at_tick: 11 * 3600,
                action: DispatchAction::SetManualSetpoint(3000.0),
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
        "model_id": model_id,
        "soc_samples_per_min": soc_samples,
        "final_soc": (home.soc_mean() * 1e6).round() / 1e6,
        "main_import_kwh": (meters.main.import_wh / 1000.0 * 1e6).round() / 1e6,
        "main_export_kwh": (meters.main.export_wh / 1000.0 * 1e6).round() / 1e6,
        "batt_import_kwh": (meters.batt_ac.import_wh / 1000.0 * 1e6).round() / 1e6,
        "batt_export_kwh": (meters.batt_ac.export_wh / 1000.0 * 1e6).round() / 1e6,
        "pv_kwh": (meters.pv_ac.wh / 1000.0 * 1e6).round() / 1e6,
        "standby_kwh": (meters.standby_loss.wh / 1000.0 * 1e6).round() / 1e6,
    });
    (summary, format!("{:x}", hasher.finalize()))
}

#[test]
fn golden_soc_traces() {
    let registry = Registry::embedded().expect("embedded registry");
    for model in registry.batteries() {
        // Expansion packs are not standalone systems.
        if model.continuous_discharge_power_kw.value == 0.0 {
            continue;
        }
        let (summary, hash) = golden_for(&model.model_id);
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path("golden");
        settings.set_snapshot_suffix(model.model_id.replace('.', "_"));
        settings.bind(|| {
            insta::assert_json_snapshot!(summary);
            insta::assert_snapshot!(hash);
        });
    }
}
