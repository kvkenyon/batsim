//! Shared helpers for the batsim-core examples: a standard load
//! archetype, an Austin PV site, and a one-battery HomeSystem composer
//! that adds the vendor-required inverter and controller for any catalog
//! battery model.

#![allow(dead_code)]

use batsim_core::battery::BatteryConfig;
use batsim_core::load::{HvacType, LoadConfig, LoadResolution, TxClimateZone, Vintage, WaterHeat};
use batsim_core::topology::{HomeBuildConfig, PvSiteConfig};
use batsim_registry::{Coupling, HomeSystem, Registry, SystemSpec};

/// Austin, TX latitude.
pub const AUSTIN_LAT: f64 = 30.27;
/// Austin, TX longitude.
pub const AUSTIN_LON: f64 = -97.74;
/// 2025-06-15T00:00:00Z, a 5-minute-aligned summer epoch.
pub const GOLDEN_EPOCH: &str = "2025-06-15T00:00:00Z";

/// Reference home: 2400 sqft, central AC, 3 occupants, central Texas.
pub fn load_config() -> LoadConfig {
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

/// Austin site with a smooth clear-sky feed (no cloud noise).
pub fn pv_site() -> PvSiteConfig {
    PvSiteConfig {
        latitude_deg: AUSTIN_LAT,
        longitude_deg: AUSTIN_LON,
        shading_factor: 0.0,
        cloud_noise: false,
    }
}

/// Build config pairing the reference load with an optional PV site.
pub fn build_config(with_pv: bool) -> HomeBuildConfig {
    HomeBuildConfig {
        load: load_config(),
        pv_site: with_pv.then(pv_site),
        battery: BatteryConfig::default(),
        pv_priority: true,
    }
}

/// Compose a one-battery HomeSystem document for a catalog model, adding
/// the hybrid inverter and controller the vendor topology requires, then
/// validate it against the registry.
pub fn one_battery_system(registry: &Registry, model_id: &str, with_pv: bool) -> SystemSpec {
    let model = registry.battery(model_id).expect("catalog battery");
    let mut inverters = Vec::new();
    let mut controllers = Vec::new();
    if matches!(model.coupling, Coupling::DCCoupledHybrid) {
        let inv = registry
            .inverters()
            .find(|i| i.compatible_battery_ids.iter().any(|b| b == model_id))
            .expect("compatible hybrid inverter in catalog");
        inverters.push(serde_json::json!({"model_id": inv.model_id, "quantity": 1}));
    }
    if let Some(ctrl) = &model.requires_controller_id {
        controllers.push(serde_json::json!({"model_id": ctrl, "quantity": 1}));
    }
    let backup_capable = !controllers.is_empty();
    let pv = with_pv.then(|| {
        // DC-coupled systems land PV on the hybrid inverter's MPPTs
        // (null); AC-coupled systems need a dedicated string inverter.
        let inv_id = if matches!(model.coupling, Coupling::DCCoupledHybrid) {
            serde_json::Value::Null
        } else {
            serde_json::json!("generic.string_pv_8kw")
        };
        serde_json::json!({
            "kw_dc": 8.0,
            "orientation": "S",
            "tilt_deg": 25.0,
            "dc_ac_ratio": 1.2,
            "pv_inverter_model_id": inv_id
        })
    });
    let mut doc = serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": "00000000-0000-0000-0000-000000000001",
        "batteries": [{"model_id": model_id, "quantity": 1, "initial_soc_frac": 0.5, "reserve_frac": 0.2}],
        "inverters": inverters,
        "controllers": controllers,
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": backup_capable,
        "grid_meter": {"esiid": "1008900000000000000001"}
    });
    if let Some(pv) = pv {
        doc["pv"] = pv;
    }
    let system = HomeSystem::from_json(&serde_json::to_string(&doc).unwrap()).unwrap();
    system.validate(registry).expect("system validates")
}
