//! Browse the embedded OEM device catalog programmatically: list every
//! battery, inverter, controller, and PV preset with its key ratings and
//! the provenance marker each value carries.
//!
//! The catalog ships as JSON under `registry/` and is compiled into the
//! binary; `Registry::embedded()` verifies per-file content hashes and
//! the whole-catalog integrity hash before handing out typed models.
//!
//! Run: `cargo run -p batsim-core --example catalog_browser`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use batsim_registry::Registry;

fn main() {
    let registry = Registry::embedded().expect("embedded registry");
    let manifest = registry.manifest();
    println!(
        "catalog: {} batteries, {} inverters, {} controllers, {} PV presets",
        registry.batteries().count(),
        registry.inverters().count(),
        registry.controllers().count(),
        registry.pv_presets().count(),
    );
    println!(
        "registry version {}, catalog sha256 {}...",
        manifest.registry_version,
        &manifest.catalog_sha256[..16],
    );

    println!();
    println!("BATTERIES");
    for b in registry.batteries() {
        let rte = b
            .rte_ac_coupled
            .as_ref()
            .map_or("n/a (expansion pack)".to_owned(), |r| {
                format!("{:.1} % ({:?})", r.value * 100.0, r.provenance)
            });
        println!(
            "  {:36} {:30} {:>6.1} kWh usable, {:>5.1} kW, {:?}/{:?}",
            b.model_id,
            b.display_name,
            b.usable_energy_kwh.value,
            b.continuous_discharge_power_kw.value,
            b.chemistry,
            b.coupling,
        );
        println!("  {:36} AC round-trip efficiency: {rte}", "");
    }

    println!();
    println!("INVERTERS");
    for i in registry.inverters() {
        println!(
            "  {:36} {:30} {:>5.1} kW AC, {:?}",
            i.model_id, i.display_name, i.rated_ac_output_kw.value, i.topology,
        );
    }

    println!();
    println!("CONTROLLERS");
    for c in registry.controllers() {
        println!(
            "  {:36} {:30} grid-forming={}, transfer {:.1} s",
            c.model_id, c.display_name, c.provides_grid_forming, c.transfer_time_s.value,
        );
    }

    println!();
    println!("PV PRESETS");
    for p in registry.pv_presets() {
        println!(
            "  {:36} {:30} {:.1} kW DC, tilt {:.0} deg, azimuth {:.0} deg",
            p.preset_id, p.display_name, p.kw_dc.value, p.tilt_deg, p.azimuth_deg,
        );
    }
}
