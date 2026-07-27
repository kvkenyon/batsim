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

/// Solar noon in Austin on the golden day (2025-06-15T18:30:00Z).
const NOON_UNIX_S: u64 = 1_750_012_200;

/// A PW3 + PV document with an explicit array size and DC/AC ratio; PV lands
/// on the integrated hybrid's MPPTs.
fn pw3_pv_system(registry: &Registry, kw_dc: f64, dc_ac_ratio: f64) -> SystemSpec {
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-00000000000a",
        "batteries": [{
            "model_id": "tesla.powerwall_3", "quantity": 1,
            "initial_soc_frac": 0.8, "reserve_frac": 0.2
        }],
        "inverters": [],
        "controllers": [],
        "pv": {
            "kw_dc": kw_dc,
            "orientation": "S",
            "tilt_deg": 25.0,
            "dc_ac_ratio": dc_ac_ratio,
            "pv_inverter_model_id": serde_json::Value::Null
        },
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    system.validate(registry).expect("system validates")
}

/// A PV home discharging at `setpoint_w` across solar noon.
fn run_pv_home(registry: &Registry, kw_dc: f64, dc_ac_ratio: f64, setpoint_w: f64) -> Home {
    let spec = pw3_pv_system(registry, kw_dc, dc_ac_ratio);
    let cfg = HomeBuildConfig {
        pv_site: Some(common::std_pv_site()),
        ..build_config()
    };
    let devices = build_devices(&spec, registry, &cfg, 42, 0).unwrap();
    let mut home = Home::new(devices, true);
    home.set_mode(ControlMode::Manual);
    home.set_manual_setpoint_w(setpoint_w);
    for tick in 0..20 {
        home.step(tick, NOON_UNIX_S + tick * 60, 60, 30.0);
    }
    home
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
fn dc_ac_ratio_caps_the_pv_path_and_books_the_overhang_as_pv_clipped() {
    let registry = Registry::embedded().unwrap();
    // 14 kW array behind a 4 kW AC cap (ratio 3.5): the cap binds well
    // below both the array's DC peak and the hybrid's 11.5 kW nameplate.
    let capped = run_pv_home(&registry, 14.0, 3.5, 0.0);
    let cap_w = 14.0 / 3.5 * 1000.0;
    let peak_ac = capped
        .truth()
        .iter()
        .fold(0.0_f64, |m, t| m.max(t.p_pv_ac_w));
    assert!(
        peak_ac <= cap_w + 1e-6,
        "PV AC must not exceed kw_dc / dc_ac_ratio = {cap_w}, got {peak_ac}"
    );
    assert!(peak_ac > 0.9 * cap_w, "the cap should be binding, not idle");
    assert!(
        capped.meters().pv_clipped.wh > 0.0,
        "the curtailed overhang must land on pv_clipped"
    );

    // The same array with a 1.0 ratio (14 kW cap) is not curtailed by the
    // ratio, so it delivers strictly more AC.
    let uncapped = run_pv_home(&registry, 14.0, 1.0, 0.0);
    let peak_uncapped = uncapped
        .truth()
        .iter()
        .fold(0.0_f64, |m, t| m.max(t.p_pv_ac_w));
    assert!(peak_uncapped > peak_ac + 1_000.0);
}

#[test]
fn shared_cap_curtails_the_command_not_the_pack() {
    let registry = Registry::embedded().unwrap();
    // 14 kW array at solar noon saturates the PW3's 11.5 kW hybrid, so a
    // 5 kW discharge command cannot fit alongside PV.
    let home = run_pv_home(&registry, 14.0, 1.0, 5_000.0);
    let rated_w = home
        .devices()
        .hybrid_inverter
        .as_ref()
        .unwrap()
        .rated_ac_w();

    for t in home.truth() {
        let batt_dc: f64 = t.units.iter().map(|u| u.p_term_w).sum();
        assert!(
            t.p_pv_ac_w + t.p_batt_ac_w <= rated_w + 1e-6,
            "tick {}: {} + {} exceeds the shared rating {rated_w}",
            t.tick,
            t.p_pv_ac_w,
            t.p_batt_ac_w
        );
        // Every watt that left the pack must appear at the AC boundary:
        // curtailment happens before integration, never after.
        if batt_dc > 0.0 {
            assert!(
                t.p_batt_ac_w >= 0.9 * batt_dc,
                "tick {}: pack delivered {batt_dc} W DC but only {} W reached AC",
                t.tick,
                t.p_batt_ac_w
            );
        }
    }
    // The command really was curtailed (PV owns the headroom), and
    // that curtailment - not lost pack energy - is what batt_clipped holds.
    assert!(home.meters().batt_clipped.wh > 0.0);
    assert!(
        home.truth().iter().any(|t| t.p_batt_ac_w < 4_000.0),
        "a 5 kW command behind a saturated hybrid must be curtailed"
    );
}

#[test]
fn hybrid_quantity_below_integrated_head_units_is_rejected() {
    let registry = Registry::embedded().unwrap();
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-00000000000b",
        "batteries": [{
            "model_id": "tesla.powerwall_3", "quantity": 2,
            "initial_soc_frac": 0.5, "reserve_frac": 0.2
        }],
        "inverters": [{"model_id": "tesla.pw3_integrated_hybrid", "quantity": 1}],
        "controllers": [],
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let spec = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap())
        .unwrap()
        .validate(&registry)
        .expect("system validates");
    let err = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap_err();
    assert!(
        err.to_string().contains("integrated-inverter head units"),
        "unexpected error: {err}"
    );
}

#[test]
fn per_module_pv_inverter_scales_to_the_array() {
    let registry = Registry::embedded().unwrap();
    // `enphase.iq8d_micro` is rated per module (0.64 kW); naming it must not
    // cap an 8 kW array at one microinverter.
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-00000000000c",
        "batteries": [{
            "model_id": "enphase.iq_battery_5p", "quantity": 1,
            "initial_soc_frac": 0.5, "reserve_frac": 0.2
        }],
        "inverters": [],
        "controllers": [],
        "pv": {
            "kw_dc": 8.0, "orientation": "S", "tilt_deg": 25.0,
            "dc_ac_ratio": 1.2, "pv_inverter_model_id": "enphase.iq8d_micro"
        },
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let spec = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap())
        .unwrap()
        .validate(&registry)
        .expect("system validates");
    let cfg = HomeBuildConfig {
        pv_site: Some(common::std_pv_site()),
        ..build_config()
    };
    let devices = build_devices(&spec, &registry, &cfg, 42, 0).unwrap();
    let inv = devices.pv_inverter.as_ref().expect("named PV inverter");
    assert_eq!(inv.quantity(), 13, "ceil(8.0 / 0.64) microinverters");
    assert!(inv.rated_ac_w() >= 8_000.0);
}

#[test]
fn dispatch_scheduled_out_of_order_still_fires_on_time() {
    let registry = Registry::embedded().unwrap();
    let spec = pw3_only_system(&registry, &serde_json::json!([]));
    let devices = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap();
    let mut home = Home::new(devices, true);
    // Idle until the manual command lands, so "not yet fired" is exactly
    // zero battery power rather than the self-consumption baseline.
    home.set_mode(ControlMode::Idle);

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
    let truth = home.truth();
    assert!(
        truth[1].p_batt_ac_w.abs() < 1e-9,
        "the tick-2 command must not leak into tick 1, got {}",
        truth[1].p_batt_ac_w
    );
    assert!(
        (truth[2].p_batt_ac_w - 2_500.0).abs() <= 125.0,
        "the 2.5 kW manual discharge must be realized at tick 2, got {}",
        truth[2].p_batt_ac_w
    );
    assert!(
        truth[3].p_batt_ac_w > 0.0,
        "the manual discharge must hold past its arrival tick, got {}",
        truth[3].p_batt_ac_w
    );
}

/// Mixed topology: PW3 (integrated hybrid) plus PV on a DEDICATED string
/// inverter (`pv_inverter_model_id` non-null). Stage 2 converts the array
/// at the string inverter; the hybrid bus must not convert and meter the
/// same DC a second time (review finding pv-double-counted-with-both-
/// inverters). Physical bound: AC out can never exceed DC in.
#[test]
fn mixed_string_pv_and_hybrid_never_converts_the_array_twice() {
    let registry = Registry::embedded().unwrap();
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-00000000000b",
        "batteries": [{
            "model_id": "tesla.powerwall_3", "quantity": 1,
            "initial_soc_frac": 0.8, "reserve_frac": 0.2
        }],
        "inverters": [],
        "controllers": [],
        "pv": {
            "kw_dc": 8.0,
            "orientation": "S",
            "tilt_deg": 25.0,
            "dc_ac_ratio": 1.2,
            "pv_inverter_model_id": "generic.string_pv_8kw"
        },
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    let spec = system.validate(&registry).expect("system validates");
    let cfg = HomeBuildConfig {
        pv_site: Some(common::std_pv_site()),
        ..build_config()
    };
    let devices = build_devices(&spec, &registry, &cfg, 42, 0).unwrap();
    let mut home = Home::new(devices, true);
    home.set_mode(ControlMode::Idle);

    let mut checked = 0u32;
    for tick in 0..20 {
        home.step(tick, NOON_UNIX_S + tick * 60, 60, 30.0);
        let rec = home.truth().last().unwrap();
        if rec.p_pv_dc_w > 100.0 {
            checked += 1;
            assert!(
                rec.p_pv_ac_w <= rec.p_pv_dc_w,
                "array converted twice: pv_ac {} > pv_dc {}",
                rec.p_pv_ac_w,
                rec.p_pv_dc_w
            );
        }
    }
    assert!(checked > 0, "expected PV production at solar noon");
    // Sanity: the string inverter really did convert (PV was not dropped
    // instead of double-counted).
    let pv_wh: f64 = home
        .truth()
        .iter()
        .map(|r| r.p_pv_ac_w * 60.0 / 3600.0)
        .sum();
    assert!(
        pv_wh > 1_000.0,
        "PV should deliver real energy, got {pv_wh} Wh"
    );
}

/// Mixed coupling: a PW2 (AC-terminal) plus a PW3 (DC-terminal hybrid) in
/// one home must apply the AC-boundary setpoint as a single pro-rata split
/// across all units: the fleet never realizes more than the setpoint (review
/// finding mixed-coupling-double-dispatch, where each terminal class
/// received the full setpoint and the pair exported past an 8 kW target).
#[test]
fn mixed_coupling_home_splits_the_setpoint_once() {
    let registry = Registry::embedded().unwrap();
    let doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-00000000000d",
        "batteries": [
            {"model_id": "tesla.powerwall_2", "quantity": 1,
             "initial_soc_frac": 0.8, "reserve_frac": 0.2},
            {"model_id": "tesla.powerwall_3", "quantity": 1,
             "initial_soc_frac": 0.8, "reserve_frac": 0.2}
        ],
        "inverters": [],
        "controllers": [{"model_id": "tesla.gateway_2", "quantity": 1}],
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": false,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    let spec = system.validate(&registry).expect("system validates");
    let devices = build_devices(&spec, &registry, &build_config(), 42, 0).unwrap();
    let mut home = Home::new(devices, true);
    home.set_mode(ControlMode::Manual);
    home.set_manual_setpoint_w(8_000.0);

    for tick in 0..6 {
        home.step(tick, NOON_UNIX_S + tick * 60, 60, 25.0);
        let realized = home.truth().last().unwrap().p_batt_ac_w;
        assert!(
            (7_000.0..=8_100.0).contains(&realized),
            "8 kW setpoint must realize ~8 kW across both couplings, got {realized}"
        );
    }
    // Both terminal classes carry a share (no class is starved either).
    let last = home.truth().last().unwrap();
    assert!(
        last.units.iter().all(|u| u.p_term_w > 500.0),
        "both units should discharge a share: {:?}",
        last.units.iter().map(|u| u.p_term_w).collect::<Vec<_>>()
    );
}
