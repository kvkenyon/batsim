//! Coupling-aware device construction and energy-path routing (spec A.3,
//! B.3.4; F16).
//!
//! Turns a validated [`SystemSpec`] into the live device set of a [`Home`].
//! The routing rules the home tick executes (explicit loss points):
//!
//! - **AC-coupled** (A.3.2): PV DC -> PV inverter (L1) -> AC panel; battery
//!   charge AC -> battery inverter (L2) -> pack; discharge pack -> battery
//!   inverter (L3) -> AC. PV and battery reach the panel over parallel
//!   paths; there is no shared inverter bottleneck.
//! - **DC-coupled hybrid** (A.3.3): PV DC -> MPPT -> hybrid DC bus;
//!   PV->battery via the battery's DC-DC curve (L2', single inversion);
//!   one DC->AC inversion (L3') at the hybrid inverter whose AC rating caps
//!   PV + battery discharge combined (PV priority, B.3.3); grid charging
//!   remains a double conversion (AC -> hybrid -> DC-DC -> pack).

use batsim_registry::{Coupling, Registry, SystemSpec};

use crate::battery::{BatteryConfig, BatteryUnit};
use crate::error::CoreError;
use crate::inverter::InverterUnit;
use crate::load::{LoadConfig, LoadModel};
use crate::pv::{PvArray, PvConfig, SubArray};
use crate::rng;

/// RNG slot assignments within a home (spec B.1.4 entity mapping; fixed,
/// never reuse). Battery units occupy slots `1..=64`.
pub const SLOT_BATTERY_BASE: u64 = 1;
/// PV array stream slot.
pub const SLOT_PV: u64 = 0x100;
/// Load model stream slot.
pub const SLOT_LOAD: u64 = 0x101;

/// Site-level PV parameters that come from the scenario rather than the
/// HomeSystem document (B.7.1: array geometry is home-scenario data).
#[derive(Debug, Clone, Copy)]
pub struct PvSiteConfig {
    /// Site latitude (deg).
    pub latitude_deg: f64,
    /// Site longitude (deg).
    pub longitude_deg: f64,
    /// Fixed shading derate in [0, 0.3].
    pub shading_factor: f64,
    /// Seeded cloud-variability overlay (B.7.5).
    pub cloud_noise: bool,
}

/// Static construction inputs for one home.
#[derive(Debug, Clone)]
pub struct HomeBuildConfig {
    /// Load archetype.
    pub load: LoadConfig,
    /// PV site parameters (required when the system has PV).
    pub pv_site: Option<PvSiteConfig>,
    /// Battery behavior config.
    pub battery: BatteryConfig,
    /// PV-first vs battery-first at a shared hybrid inverter (B.3.3).
    pub pv_priority: bool,
}

/// The constructed device set of one home.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HomeDevices {
    /// Battery units, one per physical unit (B.2.1), in declaration order.
    pub batteries: Vec<BatteryUnit>,
    /// The shared hybrid inverter (DC-coupled systems; also carries PV when
    /// `pv_inverter_model_id` is null).
    pub hybrid_inverter: Option<InverterUnit>,
    /// A dedicated PV string inverter (AC-coupled PV only).
    pub pv_inverter: Option<InverterUnit>,
    /// The PV array, if the system has one.
    pub pv: Option<PvArray>,
    /// The load model.
    pub load: LoadModel,
    /// Summed controller standby draw (W; B.3.2).
    pub controller_standby_w: f64,
    /// PV-priority at the shared inverter.
    pub pv_priority: bool,
    /// AC-side cap on the PV path from the array's declared DC/AC ratio
    /// (`kw_dc / dc_ac_ratio`, W); `None` when the system has no PV.
    pub pv_ac_cap_w: Option<f64>,
}

/// Build the device set for one home from a validated system spec.
///
/// `home_idx` is the home's stable index in the world arena; it keys all
/// per-home RNG substreams (B.1.4).
///
/// # Errors
/// [`CoreError::InvalidSystem`] when the spec references models the
/// registry lacks (composition validation should have caught these;
/// re-checked here), or PV site config is missing for a system with PV.
pub fn build_devices(
    spec: &SystemSpec,
    registry: &Registry,
    config: &HomeBuildConfig,
    master_seed: u64,
    home_idx: u64,
) -> Result<HomeDevices, CoreError> {
    let sys = &spec.system;
    let batteries = build_batteries(sys, registry, config)?;
    let mut hybrid_inverter = None;
    let mut pv_inverter = None;
    for inv_ref in &sys.inverters {
        let model = registry.inverter(&inv_ref.model_id).ok_or_else(|| {
            CoreError::InvalidSystem(format!("unknown inverter `{}`", inv_ref.model_id))
        })?;
        // Multiple entries of one topology would silently overwrite each
        // other and understate the composed AC capacity; multiplicity
        // within one entry is expressed by `quantity`.
        let slot = match model.topology {
            batsim_registry::types::InverterTopology::HybridDCCoupled => &mut hybrid_inverter,
            batsim_registry::types::InverterTopology::StringPVOnly
            | batsim_registry::types::InverterTopology::MicroinverterPV => &mut pv_inverter,
            batsim_registry::types::InverterTopology::BatteryIntegrated => {
                // Folded into BatteryUnit terminal semantics (A.3.1).
                continue;
            }
        };
        if slot.is_some() {
            return Err(CoreError::InvalidSystem(format!(
                "multiple inverter entries of the same topology (`{}`); \
                 use one entry with `quantity`",
                inv_ref.model_id
            )));
        }
        *slot = Some(InverterUnit::with_quantity(model, inv_ref.quantity, 0.0));
    }
    // An integrated hybrid comes with each head unit, so the AC path must
    // scale with them whether it was declared or synthesized.
    let integrated_units = integrated_hybrid_unit_count(sys, registry);
    match &hybrid_inverter {
        None => hybrid_inverter = synthesize_integrated_hybrid(sys, registry, integrated_units)?,
        Some(inv) if integrated_units > 0 && inv.quantity() < integrated_units => {
            return Err(CoreError::InvalidSystem(format!(
                "`{}` declares quantity {} but the system has {integrated_units} \
                 integrated-inverter head units",
                inv.model().model_id,
                inv.quantity()
            )));
        }
        Some(_) => {}
    }

    let mut pv_ac_cap_w = None;
    let pv = match &sys.pv {
        Some(pv_cfg) => {
            let site = config.pv_site.ok_or_else(|| {
                CoreError::InvalidSystem("system has PV but no PvSiteConfig".to_owned())
            })?;
            let sub = SubArray {
                kw_dc: pv_cfg.kw_dc,
                tilt_deg: pv_cfg.tilt_deg,
                azimuth_deg: f64::from(pv_cfg.orientation.azimuth_deg()),
            };
            let pv = PvArray::new(
                &PvConfig {
                    sub_arrays: vec![sub],
                    latitude_deg: site.latitude_deg,
                    longitude_deg: site.longitude_deg,
                    shading_factor: site.shading_factor,
                    cloud_noise: site.cloud_noise,
                },
                master_seed,
                rng::entity_device(home_idx, SLOT_PV),
            );
            // The array's declared DC/AC ratio caps the PV path's AC
            // output independently of the inverter nameplate (B.7.4).
            pv_ac_cap_w = Some(pv_cfg.kw_dc / pv_cfg.dc_ac_ratio * 1000.0);
            // PV lands on the hybrid inverter's MPPTs when no explicit PV
            // inverter is named (A.4.4); a named PV inverter must exist and
            // wins over an `inverters[]`-declared PV-topology entry.
            if let Some(inv_id) = &pv_cfg.pv_inverter_model_id {
                let named_already_built = pv_inverter
                    .as_ref()
                    .is_some_and(|inv| inv.model().model_id == *inv_id);
                if !named_already_built {
                    let model = registry.inverter(inv_id).ok_or_else(|| {
                        CoreError::InvalidSystem(format!("unknown PV inverter `{inv_id}`"))
                    })?;
                    let units = pv_inverter_unit_count(model, pv_cfg.kw_dc)?;
                    pv_inverter = Some(InverterUnit::with_quantity(model, units, 0.0));
                }
            }
            Some(pv)
        }
        None => None,
    };

    let controller_standby_w = sys
        .controllers
        .iter()
        .filter_map(|c| registry.controller(&c.model_id))
        .filter_map(|m| m.standby_power_w.as_ref())
        .map(|s| s.value)
        .sum();

    let load = LoadModel::new(
        &config.load,
        master_seed,
        rng::entity_device(home_idx, SLOT_LOAD),
    );

    Ok(HomeDevices {
        batteries,
        hybrid_inverter,
        pv_inverter,
        pv,
        load,
        controller_standby_w,
        pv_priority: config.pv_priority,
        pv_ac_cap_w,
    })
}

/// Find the catalog hybrid inverter that is physically integrated into a
/// declared DC-coupled battery, for systems whose `inverters[]` names none
/// (validation exempts integrated-inverter batteries; the DC power still
/// needs an AC path, otherwise stage 6 would silently drop it).
///
/// Returns `None` when no declared battery is a DC-coupled integrated
/// unit; errors when one is but the catalog has no compatible hybrid.
/// How many units a named PV inverter needs to carry the whole array.
///
/// A string inverter is one box for the array; a per-module unit (the
/// microinverter and battery-integrated ratings are per module, e.g. 0.64 kW
/// for `enphase.iq8d_micro`) is deployed one per module group, so it scales
/// to the array's DC nameplate instead of capping it at a single unit.
///
/// # Errors
/// [`CoreError::InvalidSystem`] when the named model is a hybrid inverter:
/// a hybrid is the shared battery/PV inverter, never a PV string inverter.
fn pv_inverter_unit_count(
    model: &batsim_registry::InverterModel,
    array_kw_dc: f64,
) -> Result<u32, CoreError> {
    use batsim_registry::types::InverterTopology;
    match model.topology {
        InverterTopology::StringPVOnly => Ok(1),
        InverterTopology::MicroinverterPV | InverterTopology::BatteryIntegrated => {
            let per_unit_kw = model.rated_ac_output_kw.value;
            if per_unit_kw <= 0.0 {
                return Err(CoreError::InvalidSystem(format!(
                    "PV inverter `{}` has a non-positive AC rating",
                    model.model_id
                )));
            }
            Ok((array_kw_dc / per_unit_kw).ceil().max(1.0) as u32)
        }
        InverterTopology::HybridDCCoupled => Err(CoreError::InvalidSystem(format!(
            "`{}` is a hybrid inverter and cannot serve as the PV string \
             inverter; leave `pv_inverter_model_id` null to land PV on its MPPTs",
            model.model_id
        ))),
    }
}

/// Total DC-coupled integrated-inverter head units the system declares.
fn integrated_hybrid_unit_count(
    sys: &batsim_registry::system::HomeSystem,
    registry: &Registry,
) -> u32 {
    sys.batteries
        .iter()
        .filter(|b| {
            registry.battery(&b.model_id).is_some_and(|m| {
                m.coupling == Coupling::DCCoupledHybrid && m.integrated_inverter == Some(true)
            })
        })
        .map(|b| b.quantity)
        .sum()
}

fn synthesize_integrated_hybrid(
    sys: &batsim_registry::system::HomeSystem,
    registry: &Registry,
    integrated_units: u32,
) -> Result<Option<InverterUnit>, CoreError> {
    for bat_ref in &sys.batteries {
        let Some(model) = registry.battery(&bat_ref.model_id) else {
            continue;
        };
        if model.coupling != Coupling::DCCoupledHybrid || model.integrated_inverter != Some(true) {
            continue;
        }
        // Registry iteration is sorted by `model_id`, so the pick is stable.
        let inv = registry
            .inverters()
            .find(|i| {
                matches!(
                    i.topology,
                    batsim_registry::types::InverterTopology::HybridDCCoupled
                ) && i.compatible_battery_ids.contains(&bat_ref.model_id)
            })
            .ok_or_else(|| {
                CoreError::InvalidSystem(format!(
                    "`{}` has an integrated hybrid inverter but the catalog \
                     has no compatible hybrid inverter entry",
                    bat_ref.model_id
                ))
            })?;
        return Ok(Some(InverterUnit::with_quantity(
            inv,
            integrated_units,
            0.0,
        )));
    }
    Ok(None)
}

/// Build all battery units declared by the system document.
fn build_batteries(
    sys: &batsim_registry::system::HomeSystem,
    registry: &Registry,
    config: &HomeBuildConfig,
) -> Result<Vec<BatteryUnit>, CoreError> {
    let mut batteries = Vec::new();
    for bat_ref in &sys.batteries {
        let model = registry.battery(&bat_ref.model_id).ok_or_else(|| {
            CoreError::InvalidSystem(format!("unknown battery `{}`", bat_ref.model_id))
        })?;
        let pack = if bat_ref.expansion_packs_per_unit > 0 {
            let pack_id = model
                .expansion
                .as_ref()
                .and_then(|e| e.expansion_pack_model_id.as_deref())
                .ok_or_else(|| {
                    CoreError::InvalidSystem(format!(
                        "`{}` does not accept expansion packs",
                        bat_ref.model_id
                    ))
                })?;
            let pack_model = registry.battery(pack_id).ok_or_else(|| {
                CoreError::InvalidSystem(format!("unknown expansion pack `{pack_id}`"))
            })?;
            Some((pack_model, bat_ref.expansion_packs_per_unit))
        } else {
            None
        };
        for _unit_idx in 0..bat_ref.quantity {
            // Identical units; per-unit serials are an M2 concern.
            batteries.push(BatteryUnit::new(
                model,
                pack,
                bat_ref.initial_soc_frac,
                bat_ref.reserve_frac,
                config.battery,
            )?);
        }
    }
    Ok(batteries)
}

/// Whether a unit's terminal boundary is the AC panel (vs the hybrid DC
/// bus). Drives which stage handles its conversion (B.3.4).
#[must_use]
pub const fn is_ac_terminal(coupling: Coupling) -> bool {
    matches!(coupling, Coupling::ACCoupled | Coupling::MicroinverterBased)
}
