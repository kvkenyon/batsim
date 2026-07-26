//! Build a small fleet spanning every device family in the catalog -
//! Tesla, Enphase, SolarEdge, and sonnen - simulate 24 hours, and report
//! per-home and fleet-wide energy totals.
//!
//! Each home pairs one battery with an 8 kW PV array and the vendor-
//! required controller/inverter, all at the same Austin site on the same
//! summer day, so the numbers are directly comparable across topologies
//! (AC-coupled, microinverter-based, and DC-coupled hybrid).
//!
//! Run: `cargo run -p batsim-core --example fleet_energy`

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::time::SimClock;
use batsim_core::topology::build_devices;
use batsim_registry::Registry;

/// One standalone battery from each device family.
const FLEET: [&str; 4] = [
    "tesla.powerwall_3",
    "enphase.iq_battery_10",
    "solaredge.home_battery_400v",
    "sonnen.sonnenbatterie_10_hybrid",
];

fn main() {
    let registry = Registry::embedded().expect("embedded registry");
    let seed = 7;
    let mut world = SimWorld::new(
        SimClock::from_rfc3339(support::GOLDEN_EPOCH, 60).expect("valid clock"),
        seed,
        AmbientFeed::DiurnalSine {
            mean_c: 30.0,
            amplitude_c: 6.0,
        },
    )
    .expect("world builds");
    let cfg = support::build_config(true);
    for (idx, model_id) in FLEET.iter().enumerate() {
        let spec = support::one_battery_system(&registry, model_id, true);
        let devices =
            build_devices(&spec, &registry, &cfg, seed, idx as u64).expect("devices build");
        world.add_home(Home::new(devices, false));
    }

    world.step_n(24 * 60); // 24 h at 60 s ticks

    println!("24 h fleet run, 60 s ticks, self-consumption, Austin TX 2025-06-15");
    println!();
    println!(
        "{:36} {:>8} {:>8} {:>8} {:>8} {:>7}",
        "home", "grid_in", "grid_out", "PV", "batt_out", "SOC"
    );
    println!(
        "{:36} {:>8} {:>8} {:>8} {:>8} {:>7}",
        "(kWh in/out)", "kWh", "kWh", "kWh", "kWh", "%"
    );
    let (mut fleet_in, mut fleet_out, mut fleet_pv) = (0.0, 0.0, 0.0);
    for (idx, model_id) in FLEET.iter().enumerate() {
        let home = world.home(idx).expect("home exists");
        let m = home.meters();
        fleet_in += m.main.import_wh;
        fleet_out += m.main.export_wh;
        fleet_pv += m.pv_ac.wh;
        println!(
            "{model_id:36} {:8.2} {:8.2} {:8.2} {:8.2} {:7.1}",
            m.main.import_wh / 1000.0,
            m.main.export_wh / 1000.0,
            m.pv_ac.wh / 1000.0,
            m.batt_ac.export_wh / 1000.0,
            home.soc_mean() * 100.0,
        );
    }
    println!();
    println!(
        "fleet totals: grid import {:.2} kWh, grid export {:.2} kWh, PV {:.2} kWh",
        fleet_in / 1000.0,
        fleet_out / 1000.0,
        fleet_pv / 1000.0,
    );
}
