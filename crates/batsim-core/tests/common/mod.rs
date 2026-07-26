//! Shared helpers for batsim-core integration tests.

#![allow(dead_code)]

use batsim_core::battery::BatteryConfig;
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::load::{
    HvacType, LoadConfig, LoadResolution, TxClimateZone, Vintage, WaterHeat,
};
use batsim_core::time::SimClock;
use batsim_core::topology::{build_devices, HomeBuildConfig, PvSiteConfig};
use batsim_registry::{HomeSystem, Registry, SystemSpec};

/// Austin, TX site (TX_Central climate zone).
pub const AUSTIN_LAT: f64 = 30.27;
/// Austin longitude.
pub const AUSTIN_LON: f64 = -97.74;
/// 2025-06-15T00:00:00Z (5-minute aligned).
pub const GOLDEN_EPOCH: &str = "2025-06-15T00:00:00Z";

/// Standard test load archetype: 2400 sqft Central AC family home.
pub fn std_load_config() -> LoadConfig {
    LoadConfig {
        sqft: 2400,
        hvac: HvacType::CentralAC,
        water_heat: WaterHeat::Resistance,
        occupancy: 3,
        pool: false,
        ev: None,
        climate_zone: TxClimateZone::Central,
        vintage: Vintage::Post2000,
        resolution: LoadResolution::Min1,
    }
}

/// Standard PV site: no cloud noise (deterministic smooth feed).
pub fn std_pv_site() -> PvSiteConfig {
    PvSiteConfig {
        latitude_deg: AUSTIN_LAT,
        longitude_deg: AUSTIN_LON,
        shading_factor: 0.0,
        cloud_noise: false,
    }
}

/// Compose a one-battery HomeSystem document for a catalog model, with the
/// vendor-required inverter/controller present.
pub fn one_battery_system(
    registry: &Registry,
    model_id: &str,
    with_pv: bool,
) -> SystemSpec {
    let model = registry.battery(model_id).expect("catalog battery");
    let mut inverters = Vec::new();
    let mut controllers = Vec::new();
    if matches!(
        model.coupling,
        batsim_registry::Coupling::DCCoupledHybrid
    ) {
        let inv = registry
            .inverters()
            .find(|i| i.compatible_battery_ids.iter().any(|b| b == model_id))
            .expect("compatible hybrid inverter in catalog");
        inverters.push(serde_json::json!({"model_id": inv.model_id, "quantity": 1}));
    }
    if let Some(ctrl) = &model.requires_controller_id {
        controllers.push(serde_json::json!({"model_id": ctrl, "quantity": 1}));
    }
    let pv = with_pv.then(|| {
        serde_json::json!({
            "kw_dc": 8.0,
            "orientation": "S",
            "tilt_deg": 25.0,
            "dc_ac_ratio": 1.2,
            "pv_inverter_model_id": null
        })
    });
    let mut doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-000000000001",
        "batteries": [{"model_id": model_id, "quantity": 1, "initial_soc_frac": 0.5, "reserve_frac": 0.2}],
        "inverters": inverters,
        "controllers": controllers,
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": true,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    if let Some(pv) = pv {
        doc["pv"] = pv;
    }
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    system.validate(registry).expect("system validates")
}

/// Build a deterministic world of `n` identical homes for a model.
pub fn build_world(
    registry: &Registry,
    model_id: &str,
    n: usize,
    seed: u64,
    with_pv: bool,
    record_truth: bool,
) -> SimWorld {
    let mut world = SimWorld::new(
        SimClock::from_rfc3339(GOLDEN_EPOCH, 1).unwrap(),
        seed,
        AmbientFeed::DiurnalSine {
            mean_c: 30.0,
            amplitude_c: 6.0,
        },
    )
    .unwrap();
    let spec = one_battery_system(registry, model_id, with_pv);
    let cfg = HomeBuildConfig {
        load: std_load_config(),
        pv_site: with_pv.then(std_pv_site),
        battery: BatteryConfig::default(),
        pv_priority: true,
    };
    for idx in 0..n {
        let devices = build_devices(&spec, registry, &cfg, seed, idx as u64).unwrap();
        world.add_home(Home::new(devices, record_truth));
    }
    world
}
