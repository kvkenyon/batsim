//! Simulate one home with a Tesla Powerwall 3 and rooftop PV for 24
//! hours in self-consumption mode, printing an hourly state-of-charge
//! trace plus the day's energy totals.
//!
//! The home runs on a 60 s tick at an Austin, TX site on 2025-06-15 with
//! a smooth clear-sky PV feed and a diurnal temperature swing. The
//! battery starts at 50 % SOC with a 20 % backup reserve; the engine's
//! self-consumption controller charges on PV surplus and discharges to
//! cover the load.
//!
//! Run: `cargo run -p batsim-core --example single_home_trace`

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::time::SimClock;
use batsim_core::topology::build_devices;
use batsim_registry::Registry;

fn main() {
    let registry = Registry::embedded().expect("embedded registry");
    let seed = 42;
    let spec = support::one_battery_system(&registry, "tesla.powerwall_3", true);
    let cfg = support::build_config(true);
    let devices = build_devices(&spec, &registry, &cfg, seed, 0).expect("devices build");

    let mut world = SimWorld::new(
        SimClock::from_rfc3339(support::GOLDEN_EPOCH, 60).expect("valid clock"),
        seed,
        AmbientFeed::DiurnalSine {
            mean_c: 30.0,
            amplitude_c: 6.0,
        },
    )
    .expect("world builds");
    world.add_home(Home::new(devices, true));

    println!("tesla.powerwall_3 + 8 kW PV, Austin TX, 2025-06-15, self-consumption");
    println!("sign conventions: battery + = discharging, grid + = importing");
    println!();
    println!(" clock   SOC     load    PV(AC)  battery  grid");
    println!("         %       kW      kW      kW       kW");
    for _ in 0..24 {
        world.step_n(60); // one hour at 60 s ticks
        let home = world.home(0).expect("home 0");
        let t = home.truth().last().expect("truth recorded");
        println!(
            " {:5}  {:5.1}  {:7.2}  {:7.2}  {:+7.2}  {:+7.2}",
            world.clock().tick() / 60,
            t.soc_mean * 100.0,
            t.p_load_w / 1000.0,
            t.p_pv_ac_w / 1000.0,
            t.p_batt_ac_w / 1000.0,
            t.p_grid_w / 1000.0,
        );
    }

    let home = world.home(0).expect("home 0");
    let m = home.meters();
    println!();
    println!("day totals (kWh):");
    println!("  grid import       {:8.2}", m.main.import_wh / 1000.0);
    println!("  grid export       {:8.2}", m.main.export_wh / 1000.0);
    println!("  PV production     {:8.2}", m.pv_ac.wh / 1000.0);
    println!("  battery charged   {:8.2}", m.batt_ac.import_wh / 1000.0);
    println!("  battery discharged{:8.2}", m.batt_ac.export_wh / 1000.0);
    println!("  standby losses    {:8.2}", m.standby_loss.wh / 1000.0);
    println!("  final SOC         {:8.1} %", home.soc_mean() * 100.0);
}
