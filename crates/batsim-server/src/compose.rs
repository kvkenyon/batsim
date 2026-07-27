//! Home composition: API-level home descriptions become validated
//! registry `HomeSystem` documents, then engine `Home` instances.
//!
//! Also holds the household archetype table and the deterministic fleet
//! expansion sampler.

use batsim_core::battery::BatteryConfig;
use batsim_core::home::Home;
use batsim_core::load::{
    EvConfig, HvacType, LoadConfig, LoadResolution, TxClimateZone, Vintage, WaterHeat,
};
use batsim_core::topology::{build_devices, HomeBuildConfig, PvSiteConfig};
use batsim_registry::types::Coupling;
use batsim_registry::{HomeSystem, Registry};
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use xxhash_rust::xxh3::xxh3_64;

use crate::model::{
    ArchetypeEntry, BatterySpec, FleetManifest, HomeTemplate, InverterSpec, LoadSpec,
    LocationSpec, PvSpec,
};
use crate::problem::{ApiResult, Problem};

/// A fully resolved home description ready for composition.
#[derive(Debug, Clone)]
pub struct HomePlan {
    /// Battery selection.
    pub battery: BatterySpec,
    /// Explicit inverter, if any.
    pub inverter: Option<InverterSpec>,
    /// Resolved PV peak in kW, if PV present.
    pub pv_peak_kw: Option<f64>,
    /// PV azimuth (deg).
    pub pv_azimuth_deg: f64,
    /// PV tilt (deg).
    pub pv_tilt_deg: f64,
    /// Load selection.
    pub load: LoadSpec,
    /// Location.
    pub location: LocationSpec,
    /// Initial SOC fraction.
    pub initial_soc: f64,
}

/// Household archetype presets.
fn archetype_load(name: &str) -> Option<LoadConfig> {
    let base = LoadConfig {
        sqft: 2400,
        hvac: HvacType::CentralAC,
        water_heat: WaterHeat::Resistance,
        occupancy: 3,
        pool: false,
        ev: None,
        climate_zone: TxClimateZone::Central,
        vintage: Vintage::Post2000,
        resolution: LoadResolution::Min1,
    };
    Some(match name {
        "sfh_family" => base,
        "sfh_family_ev" => LoadConfig {
            ev: Some(EvConfig {
                battery_kwh: 60.0,
                daily_miles: 35.0,
                home_charge_kw: 7.7,
            }),
            ..base
        },
        "sfh_empty_nester" => LoadConfig {
            sqft: 1900,
            occupancy: 2,
            ..base
        },
        "sfh_pool" => LoadConfig {
            sqft: 2600,
            pool: true,
            occupancy: 4,
            ..base
        },
        "townhome" => LoadConfig {
            sqft: 1500,
            occupancy: 2,
            ..base
        },
        "apartment" => LoadConfig {
            sqft: 950,
            hvac: HvacType::WindowUnits,
            occupancy: 1,
            ..base
        },
        _ => return None,
    })
}

/// Known archetype names (validation error messages).
pub const ARCHETYPES: &[&str] = &[
    "sfh_family",
    "sfh_family_ev",
    "sfh_empty_nester",
    "sfh_pool",
    "townhome",
    "apartment",
];

/// Resolve a climate-zone string.
fn climate_zone(s: Option<&str>) -> ApiResult<TxClimateZone> {
    match s.unwrap_or("central").to_ascii_lowercase().as_str() {
        "2a" | "gulf_coast" | "gulfcoast" => Ok(TxClimateZone::GulfCoast),
        "3a" | "central" => Ok(TxClimateZone::Central),
        "3b" | "west" => Ok(TxClimateZone::West),
        "4a" | "north" => Ok(TxClimateZone::North),
        other => Err(Problem::validation(format!(
            "unknown climate zone `{other}` (expected 2A, 3A, 3B, 4A, or a Texas zone name)"
        ))),
    }
}

/// Approximate site coordinates per ERCOT load zone.
fn zone_lat_lon(zone: &str) -> Option<(f64, f64)> {
    Some(match zone {
        "LZ_NORTH" | "LZ_NORTH_C" => (32.78, -96.80),
        "LZ_HOUSTON" => (29.76, -95.37),
        "LZ_COAST" => (27.80, -97.40),
        "LZ_SOUTH" | "LZ_SOUTH_C" | "LZ_SOUTHERN" => (29.42, -98.49),
        "LZ_AUSTIN" => (30.27, -97.74),
        "LZ_WEST" => (31.99, -102.10),
        "LZ_FAR_WEST" => (31.76, -106.49),
        "LZ_EAST" => (32.35, -95.30),
        _ => return None,
    })
}

/// The catalog string PV inverter used for AC-coupled arrays.
const STRING_PV_INVERTER: &str = "generic.string_pv_8kw";

/// Validate a plan and compose the engine home.
///
/// # Errors
/// [`Problem`] with a 400/422 code describing every violated rule.
pub fn compose_home(
    registry: &Registry,
    plan: &HomePlan,
    home_id: &str,
    master_seed: u64,
    home_idx: u64,
) -> ApiResult<Home> {
    if plan.battery.count == 0 {
        return Err(Problem::validation("battery.count must be >= 1"));
    }
    if !(0.0..=1.0).contains(&plan.initial_soc) {
        return Err(Problem::validation("initial_soc must be within 0..=1"));
    }
    let doc = system_doc(registry, plan, home_id)?;
    let system = HomeSystem::from_json(&doc.to_string())
        .map_err(|e| Problem::unprocessable(format!("system document invalid: {e}")))?;
    let spec = system.validate(registry).map_err(|e| {
        Problem::unprocessable(format!("system composition failed validation: {e}"))
    })?;

    let mut load = archetype_load(&plan.load.archetype).ok_or_else(|| {
        Problem::validation(format!(
            "unknown load archetype `{}` (expected one of: {})",
            plan.load.archetype,
            ARCHETYPES.join(", ")
        ))
    })?;
    load.climate_zone = climate_zone(plan.location.climate_zone.as_deref())?;
    if let Some(annual) = plan.load.annual_kwh {
        if !(annual.is_finite() && annual > 0.0) {
            return Err(Problem::validation("annual_kwh must be positive"));
        }
        // Heuristic: ~6 kWh per sqft per year for Texas housing stock.
        let scaled = (annual / 6.0).round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sqft = (scaled as u32).clamp(800, 6000);
        load.sqft = sqft;
    }

    let (lat, lon) = zone_lat_lon(&plan.location.ercot_load_zone).ok_or_else(|| {
        Problem::validation(format!(
            "unknown ERCOT load zone `{}`",
            plan.location.ercot_load_zone
        ))
    })?;

    let cfg = HomeBuildConfig {
        load,
        pv_site: plan.pv_peak_kw.map(|_| PvSiteConfig {
            latitude_deg: lat,
            longitude_deg: lon,
            shading_factor: 0.0,
            cloud_noise: true,
        }),
        battery: BatteryConfig::default(),
        pv_priority: true,
    };
    let devices = build_devices(&spec, registry, &cfg, master_seed, home_idx)
        .map_err(|e| Problem::unprocessable(format!("device construction failed: {e}")))?;
    Ok(Home::new(devices, true))
}

/// Build the registry composition document for a home plan.
fn system_doc(registry: &Registry, plan: &HomePlan, home_id: &str) -> ApiResult<serde_json::Value> {
    let model = registry
        .battery(&plan.battery.model_id)
        .ok_or_else(|| {
            Problem::validation(format!(
                "unknown battery model `{}`; see /v1/registry/batteries",
                plan.battery.model_id
            ))
        })?;

    // Inverters: explicit selection, else the vendor default for
    // DC-coupled hybrids without an integrated inverter.
    let mut inverters = Vec::new();
    if let Some(inv) = &plan.inverter {
        if registry.inverter(&inv.model_id).is_none() {
            return Err(Problem::validation(format!(
                "unknown inverter model `{}`",
                inv.model_id
            )));
        }
        inverters.push(serde_json::json!({"model_id": inv.model_id, "quantity": inv.quantity}));
    } else if matches!(model.coupling, Coupling::DCCoupledHybrid) {
        let compatible = registry
            .inverters()
            .find(|i| i.compatible_battery_ids.iter().any(|b| b == &plan.battery.model_id));
        if let Some(inv) = compatible {
            inverters.push(serde_json::json!({
                "model_id": inv.model_id,
                "quantity": plan.battery.count,
            }));
        }
    }

    // Controllers required by the battery vendor for backup.
    let mut controllers = Vec::new();
    if let Some(ctrl) = &model.requires_controller_id {
        controllers.push(serde_json::json!({"model_id": ctrl, "quantity": 1}));
    }

    let pv = match plan.pv_peak_kw {
        Some(kw) => Some(pv_doc(registry, plan, kw, &mut inverters)?),
        None => None,
    };

    Ok(serde_json::json!({
        "schema_version": "1.0.0",
        "system_id": home_id,
        "batteries": [{
            "model_id": plan.battery.model_id,
            "quantity": plan.battery.count,
            "initial_soc_frac": plan.initial_soc,
            "reserve_frac": 0.2,
        }],
        "inverters": inverters,
        "controllers": controllers,
        "pv": pv,
        "main_panel": {"service_rating_a": 200.0},
        "backup_capable": !controllers.is_empty(),
        "grid_meter": {"esiid": format!("1008900{:013}", xxh3_64(home_id.as_bytes()) % 10_000_000_000_000_u64)},
    }))
}

/// Build the PV section, adding a string inverter for AC-coupled
/// systems (DC-coupled arrays land on the hybrid's MPPTs).
fn pv_doc(
    registry: &Registry,
    plan: &HomePlan,
    kw: f64,
    inverters: &mut Vec<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    if !(kw.is_finite() && kw > 0.0 && kw <= 100.0) {
        return Err(Problem::validation(
            "pv peak_kw must be finite and within (0, 100]",
        ));
    }
    let model = registry
        .battery(&plan.battery.model_id)
        .ok_or_else(|| Problem::validation("unknown battery model"))?;
    let inv_id = if matches!(model.coupling, Coupling::DCCoupledHybrid) {
        serde_json::Value::Null
    } else {
        if registry.inverter(STRING_PV_INVERTER).is_none() {
            return Err(Problem::unprocessable(format!(
                "catalog lacks the string PV inverter `{STRING_PV_INVERTER}`"
            )));
        }
        let ac_kw = kw / 1.2;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let quantity = ((ac_kw / 8.0).ceil() as u32).max(1);
        inverters.push(serde_json::json!({
            "model_id": STRING_PV_INVERTER,
            "quantity": quantity,
        }));
        serde_json::json!(STRING_PV_INVERTER)
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let azimuth = (plan.pv_azimuth_deg.round().max(0.0) as u32) % 360;
    Ok(serde_json::json!({
        "kw_dc": kw,
        "orientation": azimuth,
        "tilt_deg": plan.pv_tilt_deg,
        "dc_ac_ratio": 1.2,
        "pv_inverter_model_id": inv_id,
    }))
}

/// Per-home deterministic RNG stream for fleet expansion draws.
#[must_use]
pub fn expansion_rng(seed: u64, ordinal: u64) -> ChaCha8Rng {
    let mut key = [0u8; 24];
    key[..8].copy_from_slice(&seed.to_le_bytes());
    key[8..16].copy_from_slice(&ordinal.to_le_bytes());
    key[16..24].copy_from_slice(&0x6578_7061_6e64_u64.to_le_bytes());
    ChaCha8Rng::seed_from_u64(xxh3_64(&key))
}

/// A fully expanded set of home plans for a manifest (deterministic).
///
/// # Errors
/// [`Problem::validation`] on malformed manifests (bad weights, empty
/// archetypes, zero count).
pub fn expand_manifest(manifest: &FleetManifest, ordinal_base: u64) -> ApiResult<Vec<HomePlan>> {
    if manifest.count == 0 {
        return Err(Problem::validation("count must be >= 1"));
    }
    if manifest.count > 100_000 {
        return Err(Problem::validation("count must be <= 100000 per request"));
    }
    if manifest.archetypes.is_empty() {
        return Err(Problem::validation("archetypes must not be empty"));
    }
    let mut weights = Vec::with_capacity(manifest.archetypes.len());
    for a in &manifest.archetypes {
        if !(a.weight.is_finite() && a.weight > 0.0) {
            return Err(Problem::validation("archetype weights must be positive"));
        }
        weights.push(a.weight);
    }
    let total_w: f64 = weights.iter().sum();

    let zones: Vec<(String, f64)> = match &manifest.geo {
        None => vec![("LZ_NORTH".to_owned(), 1.0)],
        Some(g) => {
            if g.ercot_load_zones.is_empty() {
                return Err(Problem::validation("ercot_load_zones must not be empty"));
            }
            let mut v = Vec::with_capacity(g.ercot_load_zones.len());
            for (z, w) in &g.ercot_load_zones {
                if zone_lat_lon(z).is_none() {
                    return Err(Problem::validation(format!("unknown ERCOT load zone `{z}`")));
                }
                if !(w.is_finite() && *w > 0.0) {
                    return Err(Problem::validation("load-zone weights must be positive"));
                }
                v.push((z.clone(), *w));
            }
            v
        }
    };
    let total_z: f64 = zones.iter().map(|(_, w)| w).sum();

    let mut plans = Vec::with_capacity(manifest.count as usize);
    for i in 0..u64::from(manifest.count) {
        let mut rng = expansion_rng(manifest.seed, ordinal_base + i);
        let pick: f64 = rng.gen_range(0.0..total_w);
        let mut acc = 0.0;
        let mut chosen: &ArchetypeEntry = &manifest.archetypes[0];
        for (entry, w) in manifest.archetypes.iter().zip(&weights) {
            acc += w;
            if pick < acc {
                chosen = entry;
                break;
            }
        }
        let zpick: f64 = rng.gen_range(0.0..total_z);
        let mut zacc = 0.0;
        let mut zone = &zones[0].0;
        for (z, w) in &zones {
            zacc += w;
            if zpick < zacc {
                zone = z;
                break;
            }
        }
        plans.push(plan_from_template(&chosen.template, zone, &mut rng));
    }
    Ok(plans)
}

fn plan_from_template(t: &HomeTemplate, zone: &str, rng: &mut ChaCha8Rng) -> HomePlan {
    let (peak, az, tilt) = t.pv.as_ref().map_or((None, 180.0, 25.0), |pv| {
        (
            Some(pv.peak_kw.resolve(rng)),
            pv.azimuth_deg.unwrap_or(180.0),
            pv.tilt_deg.unwrap_or(25.0),
        )
    });
    HomePlan {
        battery: t.battery.clone(),
        inverter: t.inverter.clone(),
        pv_peak_kw: peak,
        pv_azimuth_deg: az,
        pv_tilt_deg: tilt,
        load: t.load.clone(),
        location: LocationSpec {
            ercot_load_zone: zone.to_owned(),
            climate_zone: None,
        },
        initial_soc: t.initial_soc.unwrap_or(0.5),
    }
}

/// Resolve a single-home PV spec (ranges are fleet-only).
///
/// # Errors
/// [`Problem::validation`] when `peak_kw` is a range.
pub fn fixed_pv(pv: &PvSpec) -> ApiResult<(f64, f64, f64)> {
    let kw = pv.peak_kw.fixed().ok_or_else(|| {
        Problem::validation("pv.peak_kw must be a fixed value for single-home creation")
    })?;
    Ok((kw, pv.azimuth_deg.unwrap_or(180.0), pv.tilt_deg.unwrap_or(25.0)))
}

/// Content hash of a manifest expansion (canonical JSON of manifest +
/// ordinal base), as `sha256:<hex>`.
#[must_use]
pub fn expansion_hash(manifest: &FleetManifest, ordinal_base: u64) -> String {
    use sha2::Digest;
    let canonical = serde_json::json!({
        "manifest": manifest,
        "ordinal_base": ordinal_base,
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let hash = sha2::Sha256::digest(&bytes);
    let hex = hash.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    format!("sha256:{hex}")
}
