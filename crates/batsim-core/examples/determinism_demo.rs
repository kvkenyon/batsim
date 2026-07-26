//! Determinism demo: run the same seeded scenario twice and prove the
//! results are bit-identical - then run it a third time with the rayon
//! parallel stepper and prove that matches too.
//!
//! The comparison is a SHA-256 over the serialized world state plus every
//! home's full truth telemetry archive, the same comparison the engine's
//! determinism gate test uses. Determinism comes from keying every
//! random draw by (master seed, entity, tick) instead of carrying RNG
//! state, so scheduling can never perturb results.
//!
//! Run: `cargo run -p batsim-core --example determinism_demo`

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::time::SimClock;
use batsim_core::topology::build_devices;
use batsim_registry::Registry;
use sha2::{Digest, Sha256};

const SEED: u64 = 0xBA75_1DE5;

/// Three homes across device families, six hours at 60 s ticks.
fn run(parallel: bool) -> String {
    let registry = Registry::embedded().expect("embedded registry");
    let models = ["tesla.powerwall_3", "enphase.iq_battery_5p", "sonnen.ecolinx"];
    let mut world = SimWorld::new(
        SimClock::from_rfc3339(support::GOLDEN_EPOCH, 60).expect("valid clock"),
        SEED,
        AmbientFeed::DiurnalSine {
            mean_c: 30.0,
            amplitude_c: 6.0,
        },
    )
    .expect("world builds");
    let cfg = support::build_config(true);
    for (idx, model_id) in models.iter().enumerate() {
        let spec = support::one_battery_system(&registry, model_id, true);
        let devices =
            build_devices(&spec, &registry, &cfg, SEED, idx as u64).expect("devices build");
        world.add_home(Home::new(devices, true));
    }

    for _ in 0..(6 * 60) {
        if parallel {
            world.step_parallel();
        } else {
            world.step();
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&world).expect("world serializes"));
    for idx in 0..models.len() {
        hasher.update(
            serde_json::to_vec(world.home(idx).expect("home exists").truth())
                .expect("truth serializes"),
        );
    }
    format!("{:x}", hasher.finalize())
}

fn main() {
    println!("scenario: 3 homes (PW3, IQ Battery 5P, ecoLinx), 6 h, 60 s ticks, seed {SEED:#x}");
    let a = run(false);
    println!("run 1 (serial):   {a}");
    let b = run(false);
    println!("run 2 (serial):   {b}");
    assert_eq!(a, b, "same-seed serial runs must be bit-identical");
    let c = run(true);
    println!("run 3 (parallel): {c}");
    assert_eq!(a, c, "serial vs parallel must be bit-identical");
    println!();
    println!("bit-identical: same seed twice, and serial vs rayon-parallel");
}
