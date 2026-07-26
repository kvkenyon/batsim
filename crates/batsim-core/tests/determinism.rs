//! Determinism gate (spec B.1.4 `determinism_check`; F2; release gate).
//!
//! A seeded 24 h scenario MUST produce bit-identical outputs: twice in the
//! same process, and single-threaded vs rayon-parallel stepping. The
//! comparison is a SHA-256 over the serialized world state plus the full
//! truth telemetry archive of every home.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use batsim_core::engine::SimWorld;
use batsim_registry::Registry;
use sha2::{Digest, Sha256};

const HOMES: usize = 10;
const TICKS_24H: u64 = 86_400;
const SEED: u64 = 0xDEAD_BEEF;

fn run_24h(parallel: bool) -> String {
    let registry = Registry::embedded().expect("embedded registry");
    // Mixed fleet: one home per distinct catalog battery (cycled).
    let models: Vec<String> = registry
        .batteries()
        .filter(|m| m.continuous_discharge_power_kw.value > 0.0)
        .map(|m| m.model_id.clone())
        .collect();
    assert!(!models.is_empty(), "catalog has batteries");
    let mut world = SimWorld::new(
        batsim_core::time::SimClock::from_rfc3339(common::GOLDEN_EPOCH, 1).unwrap(),
        SEED,
        batsim_core::engine::AmbientFeed::DiurnalSine {
            mean_c: 30.0,
            amplitude_c: 6.0,
        },
    )
    .unwrap();
    for idx in 0..HOMES {
        let model_id = &models[idx % models.len()];
        let spec = common::one_battery_system(&registry, model_id, true);
        let cfg = batsim_core::topology::HomeBuildConfig {
            load: common::std_load_config(),
            pv_site: Some(common::std_pv_site()),
            battery: batsim_core::battery::BatteryConfig::default(),
            pv_priority: true,
        };
        let devices =
            batsim_core::topology::build_devices(&spec, &registry, &cfg, SEED, idx as u64).unwrap();
        world.add_home(batsim_core::home::Home::new(devices, true));
    }

    let mut hasher = Sha256::new();
    for _ in 0..TICKS_24H {
        if parallel {
            world.step_parallel();
        } else {
            world.step();
        }
    }
    hasher.update(serde_json::to_vec(&world).unwrap());
    for idx in 0..HOMES {
        hasher.update(serde_json::to_vec(&world.home(idx).unwrap().truth()).unwrap());
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn determinism_check() {
    let a = run_24h(false);
    let b = run_24h(false);
    assert_eq!(a, b, "same-seed runs must be bit-identical");
    let c = run_24h(true);
    assert_eq!(a, c, "single-threaded vs parallel must be bit-identical");
}
