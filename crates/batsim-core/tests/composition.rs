//! Device-composition edge cases at the home boundary: integrated-inverter
//! systems, inverter multiplicity, and out-of-order dispatch scheduling.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use batsim_core::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use batsim_core::home::Home;
use batsim_core::topology::{build_devices, HomeBuildConfig};
use batsim_registry::{HomeSystem, Registry, SystemSpec};

/// A PW3-only document: the battery's hybrid inverter is integrated, so
/// `inverters[]` is empty (validation exempts it).
fn pw3_only_system(registry: &Registry, inverters: &serde_json::Value) -> SystemSpec {
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-000000000009",
        "batteries": [{
            "model_id": "tesla.powerwall_3", "quantity": 1,
            "initial_soc_frac": 0.8, "reserve_frac": 0.2
        }],
        "inverters": inverters,
        "controllers": [],
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    system.validate(registry).expect("system validates")
}

fn build_config() -> HomeBuildConfig {
    HomeBuildConfig {
        load: common::std_load_config(),
        pv_site: None,
        battery: batsim_core::battery::BatteryConfig::default(),
        pv_priority: true,
    }
}

#[test]
fn integrated_inverter_battery_gets_a_synthesized_ac_path() {
    let registry = Registry::embedded().unwrap();
    let spec = pw3_only_system(&registry, &serde_json::json!([]));
    let devices = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap();

    let inv = devices
        .hybrid_inverter
        .as_ref()
        .expect("integrated hybrid synthesized from the catalog");
    assert_eq!(inv.model().model_id, "tesla.pw3_integrated_hybrid");

    // A discharge command must show up at the AC boundary, not vanish on
    // the DC bus: the truth record carries a positive battery AC power and
    // the pack drains.
    let mut home = Home::new(devices, true);
    home.set_mode(ControlMode::Manual);
    home.set_manual_setpoint_w(3_000.0);
    let soc_before = home.soc_mean();
    for tick in 0..12 {
        home.step(tick, 1_750_000_000 + tick * 60, 60, 25.0);
    }
    let last = home.truth().last().unwrap();
    assert!(
        last.p_batt_ac_w > 1_000.0,
        "DC-coupled discharge must reach the AC panel, got {}",
        last.p_batt_ac_w
    );
    assert!(home.soc_mean() < soc_before);
    // Truth now carries the unit's Thevenin voltage and conversion heat.
    let unit = last.units.first().unwrap();
    assert!(unit.v_term_v > 0.0);
    assert!(unit.heat_w > 0.0);
}

#[test]
fn inverter_quantity_aggregates_rated_ac() {
    let registry = Registry::embedded().unwrap();
    let single = pw3_only_system(
        &registry,
        &serde_json::json!([{"model_id": "tesla.pw3_integrated_hybrid", "quantity": 1}]),
    );
    let one = build_devices(&single, &registry, &build_config(), 42, 0).unwrap();
    let rated_one = one.hybrid_inverter.as_ref().unwrap().rated_ac_w();

    let double = pw3_only_system(
        &registry,
        &serde_json::json!([{"model_id": "tesla.pw3_integrated_hybrid", "quantity": 2}]),
    );
    let two = build_devices(&double, &registry, &build_config(), 42, 0).unwrap();
    let inv = two.hybrid_inverter.as_ref().unwrap();
    assert_eq!(inv.quantity(), 2);
    assert!((inv.rated_ac_w() - 2.0 * rated_one).abs() < 1e-9);
    // Two units at fleet power P run at the per-unit efficiency of P/2.
    assert!(
        (inv.eta_at_w(2.0 * 3_000.0) - one.hybrid_inverter.as_ref().unwrap().eta_at_w(3_000.0))
            .abs()
            < 1e-12
    );
}

#[test]
fn duplicate_same_topology_inverter_entries_are_rejected() {
    let registry = Registry::embedded().unwrap();
    let spec = pw3_only_system(
        &registry,
        &serde_json::json!([
            {"model_id": "tesla.pw3_integrated_hybrid", "quantity": 1},
            {"model_id": "tesla.pw3_integrated_hybrid", "quantity": 1}
        ]),
    );
    let err = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap_err();
    assert!(
        err.to_string().contains("same topology"),
        "unexpected error: {err}"
    );
}

#[test]
fn dispatch_scheduled_out_of_order_still_fires_on_time() {
    let registry = Registry::embedded().unwrap();
    let spec = pw3_only_system(&registry, &serde_json::json!([]));
    let devices = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap();
    let mut home = Home::new(devices, true);

    // Submitted late-tick-first: the tick-2 command must still land at 2.
    home.schedule(ScheduledDispatch {
        execute_at_tick: 8,
        action: DispatchAction::SetManualSetpoint(1_000.0),
    });
    home.schedule(ScheduledDispatch {
        execute_at_tick: 2,
        action: DispatchAction::SetMode(ControlMode::Manual),
    });
    home.schedule(ScheduledDispatch {
        execute_at_tick: 2,
        action: DispatchAction::SetManualSetpoint(2_500.0),
    });

    for tick in 0..4 {
        home.step(tick, 1_750_000_000 + tick * 60, 60, 25.0);
    }
    let at_tick_3 = home.truth().last().unwrap().p_batt_ac_w;
    assert!(
        at_tick_3 > 0.0,
        "the tick-2 manual discharge must be active by tick 3, got {at_tick_3}"
    );
}
