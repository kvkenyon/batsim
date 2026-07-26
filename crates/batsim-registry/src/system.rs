//! HomeSystem composition (spec §3.1, §4.4): the declarative system
//! document, per-vendor validation rules, and the resolved [`SystemSpec`]
//! the engine consumes at simulation-init.
//!
//! Split of responsibilities: this module validates and computes; it never
//! constructs engine types. batsim-core turns a [`SystemSpec`] into live
//! `Home` state.

use serde::{Deserialize, Serialize};

use crate::error::{RegistryError, Violation};
use crate::load::Registry;

/// A battery line item in a HomeSystem document (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryRef {
    /// Battery `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
    /// Expansion packs per head unit (PW3; 0 for all other models).
    #[serde(default)]
    pub expansion_packs_per_unit: u32,
    /// Initial SOC fraction of usable energy.
    #[serde(default = "default_initial_soc")]
    pub initial_soc_frac: f64,
    /// User backup reserve floor.
    #[serde(default = "default_reserve_frac")]
    pub reserve_frac: f64,
}

const fn default_initial_soc() -> f64 {
    0.5
}

const fn default_reserve_frac() -> f64 {
    0.2
}

/// An inverter line item (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InverterRef {
    /// Inverter `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// A controller line item (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerRef {
    /// Controller `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// Array orientation: named compass point or explicit azimuth degrees
/// (spec §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Orientation {
    /// Named orientation.
    Named(NamedOrientation),
    /// Azimuth degrees, 0..359 (180 = south).
    Azimuth(u32),
}

/// Named array orientations (spec §4.4 enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedOrientation {
    /// North.
    N,
    /// North-east.
    NE,
    /// East.
    E,
    /// South-east.
    SE,
    /// South.
    S,
    /// South-south-west.
    SSW,
    /// South-west.
    SW,
    /// West-south-west.
    WSW,
    /// West.
    W,
    /// North-west.
    NW,
    /// Flat (horizontal).
    FLAT,
}

impl Orientation {
    /// Resolve to azimuth degrees (180 = south, 90 = east, 270 = west).
    /// `FLAT` resolves to 180 (tilt carries the flatness).
    #[must_use]
    pub const fn azimuth_deg(self) -> u32 {
        match self {
            Self::Azimuth(a) => a,
            Self::Named(n) => match n {
                NamedOrientation::N => 0,
                NamedOrientation::NE => 45,
                NamedOrientation::E => 90,
                NamedOrientation::SE => 135,
                NamedOrientation::S | NamedOrientation::FLAT => 180,
                NamedOrientation::SSW => 202,
                NamedOrientation::SW => 225,
                NamedOrientation::WSW => 247,
                NamedOrientation::W => 270,
                NamedOrientation::NW => 315,
            },
        }
    }
}

/// PV array configuration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
    /// Array DC nameplate.
    pub kw_dc: f64,
    /// Array orientation.
    pub orientation: Orientation,
    /// Tilt in degrees.
    #[serde(default = "default_tilt")]
    pub tilt_deg: f64,
    /// DC/AC ratio.
    #[serde(default = "default_dc_ac_ratio")]
    pub dc_ac_ratio: f64,
    /// PV inverter `model_id`; `None` iff PV lands on a hybrid inverter's
    /// MPPTs.
    #[serde(default)]
    pub pv_inverter_model_id: Option<String>,
}

const fn default_tilt() -> f64 {
    25.0
}

const fn default_dc_ac_ratio() -> f64 {
    1.2
}

/// Main service panel (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainPanel {
    /// Service rating in amps (240 V split-phase assumed).
    #[serde(default = "default_service_rating")]
    pub service_rating_a: f64,
    /// Utility-imposed export cap; `None` = none.
    #[serde(default)]
    pub interconnection_limit_kw: Option<f64>,
}

const fn default_service_rating() -> f64 {
    200.0
}

/// Backup sub-panel declaration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPanel {
    /// Peak critical-load power.
    #[serde(default = "default_critical_peak")]
    pub critical_loads_peak_kw: f64,
    /// Whole-home backup topology (no critical-loads split).
    #[serde(default)]
    pub whole_home: bool,
}

const fn default_critical_peak() -> f64 {
    5.0
}

/// Generator input declaration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorConfig {
    /// Rated power.
    pub rated_kw: f64,
    /// Auto-transfer-switch flag.
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

const fn default_true() -> bool {
    true
}

/// EV charger declaration: load-only (V1G), V2X out of scope (Part A §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvChargerConfig {
    /// Rated charge power (default 11.5 kW L2).
    #[serde(default = "default_ev_kw")]
    pub rated_kw: f64,
    /// V1G controllable load.
    #[serde(default = "default_true")]
    pub controllable: bool,
    /// Whether the charger sits on the backup sub-panel.
    #[serde(default)]
    pub on_backup_panel: bool,
}

const fn default_ev_kw() -> f64 {
    11.5
}

/// Grid meter point: the ERCOT ESIID binding (spec §4.4; consumed by
/// Part D in M3+).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridMeter {
    /// ERCOT ESI ID.
    pub esiid: String,
    /// TDSP name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdsp: Option<String>,
}

/// The HomeSystem composition document (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSystem {
    /// Schema version; must equal [`crate::types::SCHEMA_VERSION`].
    pub schema_version: String,
    /// System UUID (server-assigned in M2+; any string accepted here).
    pub system_id: String,
    /// Human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Battery line items.
    pub batteries: Vec<BatteryRef>,
    /// Inverter line items.
    pub inverters: Vec<InverterRef>,
    /// Controller line items.
    #[serde(default)]
    pub controllers: Vec<ControllerRef>,
    /// PV array, if any.
    #[serde(default)]
    pub pv: Option<PvConfig>,
    /// Main service panel.
    pub main_panel: MainPanel,
    /// Whether the system asserts backup capability.
    pub backup_capable: bool,
    /// Backup sub-panel, if any.
    #[serde(default)]
    pub backup_panel: Option<BackupPanel>,
    /// Generator input, if any.
    #[serde(default)]
    pub generator: Option<GeneratorConfig>,
    /// EV chargers.
    #[serde(default)]
    pub ev_chargers: Vec<EvChargerConfig>,
    /// Grid meter point.
    pub grid_meter: GridMeter,
}

/// A resolved, validated system: the composition-time output consumed by
/// batsim-core at simulation-init (spec §3.1).
#[derive(Debug, Clone)]
pub struct SystemSpec {
    /// The (validated) source document.
    pub system: HomeSystem,
    /// Total usable battery energy across all units and packs, kWh.
    pub total_usable_energy_kwh: f64,
    /// Total continuous discharge power at device boundaries, kW.
    pub total_discharge_power_kw: f64,
    /// Total continuous charge power, kW.
    pub total_charge_power_kw: f64,
    /// Computed backup-path continuous power:
    /// `min(total battery continuous, total inverter backup rating)`
    /// (spec §3.1). `None` when not backup-capable.
    pub backup_path_power_kw: Option<f64>,
}

impl HomeSystem {
    /// Validate this composition against the registry per spec §3.1 rules
    /// and §4.6 cross-reference checks; on success return the resolved
    /// [`SystemSpec`]. All violations are enumerated, never fail-fast.
    ///
    /// Enforced rules (spec §3.1):
    /// - Every referenced `model_id` resolves to an entry of matching kind.
    /// - `backup_capable` requires exactly one grid-forming controller, or
    ///   every battery having `grid_forming_in_backup` with the controller
    ///   named by its `requires_controller_id` present.
    /// - DC-coupled batteries reference a compatible hybrid inverter.
    /// - SolarEdge: battery count <= 3 x Home Hub count.
    /// - PW3: expansion packs <= 3 per head unit; packs add no power.
    /// - Enphase: continuous power == 0.64 kW x total IQ8D count.
    /// - Generator only when a present controller supports generator input.
    ///
    /// # Errors
    /// [`RegistryError::Validation`] enumerating every violation.
    pub fn validate(&self, registry: &Registry) -> Result<SystemSpec, RegistryError> {
        let _ = registry;
        todo!("implemented by composer task")
    }

    /// Parse a HomeSystem JSON document.
    ///
    /// # Errors
    /// [`RegistryError::Parse`] on malformed JSON or schema-shape mismatch.
    pub fn from_json(json: &str) -> Result<Self, RegistryError> {
        serde_json::from_str(json).map_err(|source| RegistryError::Parse {
            path: "<home_system>".to_owned(),
            source,
        })
    }
}

/// Violation constructor helper for composition checks.
#[must_use]
pub fn violation(field: &str, message: impl Into<String>) -> Violation {
    Violation {
        path: "<home_system>".to_owned(),
        field: field.to_owned(),
        message: message.into(),
    }
}
