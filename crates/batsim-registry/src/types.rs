//! Typed serde targets for the Part A registry JSON schemas (spec §4).
//!
//! Field names and shapes mirror the JSON schema documents verbatim
//! (snake_case, draft 2020-12). Every numeric or categorical catalog value
//! carries a [`Provenance`] marker; unknown values are omitted (`Option`)
//! rather than invented (spec Part A, provenance convention).
//!
//! Units in this layer follow Part A §1.4: energy kWh, power kW,
//! temperature °C, durations seconds, efficiencies fractions in `[0, 1]`.
//! Conversion to the engine's SI watts/watt-hours happens in batsim-core.

use serde::{Deserialize, Serialize};

/// Current schema version accepted for catalog entries (spec §4.2–4.4
/// `schema_version` const).
pub const SCHEMA_VERSION: &str = "1.0.0";

/// Provenance marker required on every catalog value (spec Part A,
/// normative convention). `estimated` values MUST NOT be silently promoted
/// to `spec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Appears in a manufacturer datasheet, warranty, or install manual.
    Spec,
    /// Inferred, rounded, or from secondary sources.
    Estimated,
}

/// A number with provenance, optional unit label, and optional note
/// (`annotatedNumber`, spec §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotatedNumber {
    /// The numeric value.
    pub value: f64,
    /// Provenance marker.
    pub provenance: Provenance,
    /// Unit label, e.g. `kWh`, `kW`, `degC`, `frac`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Free-text assumption / derivation note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AnnotatedNumber {
    /// Construct a spec-grade value with a unit label.
    #[must_use]
    pub fn spec(value: f64, unit: &str) -> Self {
        Self {
            value,
            provenance: Provenance::Spec,
            unit: Some(unit.to_owned()),
            note: None,
        }
    }

    /// Construct an estimated value with a unit label and assumption note.
    #[must_use]
    pub fn estimated(value: f64, unit: &str, note: &str) -> Self {
        Self {
            value,
            provenance: Provenance::Estimated,
            unit: Some(unit.to_owned()),
            note: Some(note.to_owned()),
        }
    }
}

/// One point of a piecewise efficiency curve (`x_kw` ascending, spec §4.1).
///
/// The x-axis is kW at the device's terminal boundary: AC-side for
/// AC-coupled integrated devices, DC-bus-side for DC-coupled hybrid packs
/// (batsim-core documents which boundary each curve kind uses).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EfficiencyPoint {
    /// Power magnitude in kW.
    pub x_kw: f64,
    /// Conversion efficiency fraction in `[0, 1]`.
    pub efficiency: f64,
}

/// Piecewise-linear efficiency curve; linear interpolation between points,
/// clamped (not extrapolated) outside the sampled range (spec §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EfficiencyCurve {
    /// Sample points, minimum 2, `x_kw` monotonically increasing.
    pub points: Vec<EfficiencyPoint>,
    /// Provenance of the whole curve.
    pub provenance: Provenance,
}

impl EfficiencyCurve {
    /// Evaluate the curve at `x_kw` with linear interpolation and endpoint
    /// clamping. An empty curve yields 0.0; callers validate `minItems: 2`
    /// at load time, so this is only a defensive default.
    #[must_use]
    pub fn eval(&self, x_kw: f64) -> f64 {
        let x = x_kw.abs();
        let Some(first) = self.points.first() else {
            return 0.0;
        };
        if x <= first.x_kw {
            return first.efficiency;
        }
        let Some(last) = self.points.last() else {
            return first.efficiency;
        };
        if x >= last.x_kw {
            return last.efficiency;
        }
        for pair in self.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if x <= b.x_kw {
                let span = b.x_kw - a.x_kw;
                if span <= 0.0 {
                    return b.efficiency;
                }
                let t = (x - a.x_kw) / span;
                return a.efficiency + t * (b.efficiency - a.efficiency);
            }
        }
        last.efficiency
    }
}

/// Cell chemistry (spec §4.1). Selects the Part B chemistry behavior module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chemistry {
    /// Lithium iron phosphate: flat OCV, 0 °C charge block, long cycle life.
    LFP,
    /// Nickel manganese cobalt: sloped OCV, high-SOC calendar penalty.
    NMC,
    /// Nickel cobalt aluminum: accepted by schema; no catalog device uses it.
    NCA,
}

/// Coupling topology (spec §4.1). Single-valued per entry; the sonnen
/// Batterie 10 ships as two entries (`_ac` / `_hybrid`) per Part A §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Coupling {
    /// AC-coupled with integrated battery inverter (PW2, ecoLinx, Core+).
    ACCoupled,
    /// DC-coupled hybrid: battery on a shared hybrid inverter DC bus.
    DCCoupledHybrid,
    /// AC-coupled via per-battery microinverters (Enphase IQ Battery).
    MicroinverterBased,
}

/// Operating temperature range in °C (spec §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemperatureRange {
    /// Minimum operating temperature.
    pub min_c: f64,
    /// Maximum operating temperature.
    pub max_c: f64,
    /// Derating behavior note, if the manufacturer publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derating_note: Option<String>,
    /// Provenance marker.
    pub provenance: Provenance,
}

/// Warranty terms; every present field carries provenance (spec §4.1).
/// Telemetry-only in M1: tracked, never enforced (Part A §5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warranty {
    /// Years of coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub years: Option<AnnotatedNumber>,
    /// Cycle-life coverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycles: Option<AnnotatedNumber>,
    /// Aggregate energy throughput cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throughput_mwh: Option<AnnotatedNumber>,
    /// Guaranteed capacity retention at end of warranty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_retention_pct: Option<AnnotatedNumber>,
}

/// Vendor API family to mimic (spec §4.1 `vendorApi.family`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorApiFamily {
    /// Tesla local Gateway LAN API.
    TeslaLocalGateway,
    /// Tesla Fleet (cloud) API.
    TeslaFleetApi,
    /// Enphase Envoy / IQ Gateway local API.
    EnphaseEnvoyLocal,
    /// Enphase Enlighten cloud API.
    EnphaseEnlightenCloud,
    /// SolarEdge monitoring cloud API.
    SolaredgeMonitoringCloud,
    /// SolarEdge local Modbus-TCP.
    SolaredgeModbusTcp,
    /// sonnen local REST API v2.
    SonnenLocalV2,
    /// Non-vendor-specific surface.
    Generic,
}

/// Vendor API authentication style (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStyle {
    /// Local password login returning a bearer token (Tesla Gateway).
    BearerLocalLogin,
    /// OAuth2 (Tesla Fleet API).
    Oauth2,
    /// JWT obtained via the vendor cloud (Enphase local).
    JwtViaCloud,
    /// Static API key (SolarEdge cloud).
    ApiKey,
    /// Token request header (sonnen local v2).
    TokenHeader,
    /// No authentication.
    None,
}

/// Purpose of a mimicked vendor endpoint (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointPurpose {
    /// Telemetry reads.
    Telemetry,
    /// State-of-charge reads.
    Soc,
    /// Setpoint / dispatch writes.
    SetpointDispatch,
    /// Operating-mode configuration.
    ModeConfig,
    /// Device inventory enumeration.
    Inventory,
    /// Authentication handshake.
    Auth,
}

/// One mimicked vendor endpoint declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorEndpoint {
    /// URL path on the real device/cloud.
    pub path: String,
    /// What the endpoint is for.
    pub purpose: EndpointPurpose,
}

/// Vendor-API mimicry metadata (spec §4.1, §4.5). The only Part-A input to
/// the (M2+) vendor adapter layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VendorApi {
    /// API family.
    pub family: VendorApiFamily,
    /// Authentication style.
    pub auth_style: AuthStyle,
    /// Base path hint on the real surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path_hint: Option<String>,
    /// Declared endpoints.
    pub endpoints: Vec<VendorEndpoint>,
    /// Provenance marker.
    pub provenance: Provenance,
}

/// Usable SOC window of a battery (spec §4.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocWindow {
    /// Minimum SOC fraction of usable energy.
    pub min_soc_frac: f64,
    /// Maximum SOC fraction of usable energy.
    pub max_soc_frac: f64,
    /// Simulator-default user backup reserve floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_floor_frac: Option<f64>,
    /// Provenance marker.
    pub provenance: Provenance,
}

/// Ramp-rate declaration (spec §4.2). Rarely published; catalog carries
/// estimated full-swing-in-1s values per Part A §5 default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RampRate {
    /// Maximum power slew in kW per second.
    pub max_kw_per_s: f64,
    /// Provenance marker.
    pub provenance: Provenance,
    /// Assumption note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Expansion-pack metadata (spec §4.2). PW3: up to 3 packs, energy only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expansion {
    /// Maximum battery units (head + packs) sharing one inverter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_units_per_inverter: Option<u32>,
    /// `model_id` of the expansion-pack entry, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion_pack_model_id: Option<String>,
    /// Whether packs add power (false for PW3: energy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packs_add_power: Option<bool>,
}

/// Cooling system type (spec §4.2, B.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cooling {
    /// No fan, no moving parts (Enphase).
    Passive,
    /// Forced air.
    ActiveAir,
    /// Liquid loop (Powerwall).
    ActiveLiquid,
    /// Not published.
    Unknown,
}

/// BatteryModel registry entry (spec §4.2). All power values are AC-side at
/// the device boundary unless the coupling is DC-hybrid, in which case they
/// are DC-bus-side (Part A §1.4, §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryModel {
    /// Schema version; must equal [`SCHEMA_VERSION`].
    pub schema_version: String,
    /// Content revision of this entry (semver).
    pub entry_version: String,
    /// `model_id` of the entry this one replaces, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// Unique `vendor.model` identifier.
    pub model_id: String,
    /// Vendor display name.
    pub vendor: String,
    /// Human-readable model name.
    pub display_name: String,
    /// Cell chemistry.
    pub chemistry: Chemistry,
    /// Coupling topology.
    pub coupling: Coupling,
    /// Nameplate energy.
    pub nameplate_energy_kwh: AnnotatedNumber,
    /// Usable energy (the engine never operates outside this).
    pub usable_energy_kwh: AnnotatedNumber,
    /// Continuous discharge power at the device boundary.
    pub continuous_discharge_power_kw: AnnotatedNumber,
    /// Peak discharge power and its sustain duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_discharge_power_kw: Option<AnnotatedNumber>,
    /// How long peak power may be sustained (spec: hard-timer default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_duration_s: Option<AnnotatedNumber>,
    /// Continuous charge power at the device boundary.
    pub continuous_charge_power_kw: AnnotatedNumber,
    /// Usable SOC window.
    pub soc_window: SocWindow,
    /// Charge-path conversion efficiency vs power.
    pub charge_efficiency_curve: EfficiencyCurve,
    /// Discharge-path conversion efficiency vs power.
    pub discharge_efficiency_curve: EfficiencyCurve,
    /// PV-source round-trip efficiency (single-inversion for DC hybrids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rte_pv_coupled: Option<AnnotatedNumber>,
    /// Grid-source round-trip efficiency (double conversion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rte_ac_coupled: Option<AnnotatedNumber>,
    /// Grid-forming capability when paired with the required controller.
    pub grid_forming_in_backup: bool,
    /// Controller `model_id` required for backup operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_controller_id: Option<String>,
    /// True when the inverter is integrated (PW2/PW3/5P/ecoLinx).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_inverter: Option<bool>,
    /// Microinverter count per unit (Enphase; power scales linearly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microinverter_count: Option<u32>,
    /// Continuous kW per microinverter (0.64 per IQ8D, spec-derived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_per_microinverter_kw: Option<AnnotatedNumber>,
    /// Expansion-pack metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<Expansion>,
    /// Warranty terms (telemetry-only).
    pub warranty: Warranty,
    /// Operating temperature range.
    pub operating_temperature: TemperatureRange,
    /// Cooling system type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling: Option<Cooling>,
    /// Ramp rate (estimated everywhere in the catalog, per Part A §5).
    pub ramp_rate: RampRate,
    /// Self-discharge fraction per day; per Part A §5 this also folds in
    /// idle/standby draw (a later schema revision may split these).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_discharge_frac_per_day: Option<AnnotatedNumber>,
    /// Vendor-API mimicry metadata.
    pub vendor_api: VendorApi,
}

/// Inverter topology classification (spec §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InverterTopology {
    /// Shared hybrid inverter with battery DC bus and PV MPPTs.
    HybridDCCoupled,
    /// PV-only string inverter.
    StringPVOnly,
    /// PV microinverter.
    MicroinverterPV,
    /// Inverter embedded in a battery product.
    BatteryIntegrated,
}

/// InverterModel registry entry (spec §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InverterModel {
    /// Schema version; must equal [`SCHEMA_VERSION`].
    pub schema_version: String,
    /// Content revision of this entry.
    pub entry_version: String,
    /// Unique `vendor.model` identifier.
    pub model_id: String,
    /// Vendor display name.
    pub vendor: String,
    /// Human-readable model name.
    pub display_name: String,
    /// Topology classification.
    pub topology: InverterTopology,
    /// Rated continuous AC output.
    pub rated_ac_output_kw: AnnotatedNumber,
    /// Backup-mode AC output rating, if different.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ac_output_kw_backup: Option<AnnotatedNumber>,
    /// Maximum PV DC input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pv_dc_input_kw: Option<AnnotatedNumber>,
    /// MPPT count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mppt_count: Option<AnnotatedNumber>,
    /// Maximum PV string voltage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pv_voltage_v: Option<AnnotatedNumber>,
    /// DC→AC conversion efficiency vs AC output kW.
    pub efficiency_curve: EfficiencyCurve,
    /// Grid-following when on-grid (all catalog devices: true).
    #[serde(default = "default_true")]
    pub grid_following_on_grid: bool,
    /// Grid-forming in backup when paired with its controller.
    pub grid_forming_in_backup: bool,
    /// Compatible battery `model_id`s (non-empty for hybrids).
    pub compatible_battery_ids: Vec<String>,
    /// Maximum batteries per inverter (SolarEdge: 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_batteries: Option<u32>,
    /// Vendor-API mimicry metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_api: Option<VendorApi>,
}

const fn default_true() -> bool {
    true
}

/// Frequency-shift PV curtailment (Watt-Hz droop) declaration owned by the
/// controller entry (spec §3.4; default span estimated per Part A §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurtailmentCurve {
    /// Island frequency where curtailment begins.
    pub start_hz: f64,
    /// Island frequency where PV is fully curtailed.
    pub full_curtail_hz: f64,
    /// Provenance marker.
    pub provenance: Provenance,
}

/// System controller / gateway / transfer device registry entry.
///
/// The controller owns islanding mechanics (spec §3.4): transfer time,
/// reconnect delay, frequency-shift PV curtailment, generator interlock.
/// The spec's Part A §4 does not publish a controller schema; this shape is
/// the minimal faithful declaration of §3.4/B.3.6 controller behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerModel {
    /// Schema version; must equal [`SCHEMA_VERSION`].
    pub schema_version: String,
    /// Content revision of this entry.
    pub entry_version: String,
    /// Unique `vendor.model` identifier.
    pub model_id: String,
    /// Vendor display name.
    pub vendor: String,
    /// Human-readable model name.
    pub display_name: String,
    /// Whether this controller forms an islanded microgrid.
    pub provides_grid_forming: bool,
    /// Transfer time on grid loss (Part A §5 estimated defaults:
    /// Tesla Gateway 0.1 s, IQ System Controller 1.0 s, SolarEdge 0.5 s).
    pub transfer_time_s: AnnotatedNumber,
    /// Reconnect delay after stable grid returns (default 300 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect_s: Option<AnnotatedNumber>,
    /// Whether a generator input is supported through this controller.
    pub supports_generator_input: bool,
    /// Frequency-shift PV curtailment curve for islanded operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_shift_curtailment: Option<CurtailmentCurve>,
    /// Maximum continuous backup-path power through this controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_backup_power_kw: Option<AnnotatedNumber>,
    /// Whether sunlight (PV-only) black-start is supported (B.8.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pv_blackstart: Option<bool>,
    /// Controller standby draw (gateway/controller vampire load).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standby_power_w: Option<AnnotatedNumber>,
    /// Vendor-API mimicry metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_api: Option<VendorApi>,
}

/// PV preset registry entry: a pre-canned residential array description
/// used when a scenario references a preset instead of inline geometry
/// (spec §1.1 `pv_presets/`, §3.1 PV array row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvPreset {
    /// Schema version; must equal [`SCHEMA_VERSION`].
    pub schema_version: String,
    /// Content revision of this entry.
    pub entry_version: String,
    /// Unique preset identifier (e.g. `residential.south_8kw`).
    pub preset_id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Array DC nameplate.
    pub kw_dc: AnnotatedNumber,
    /// Tilt in degrees (0 = flat, 90 = vertical).
    pub tilt_deg: f64,
    /// Azimuth in degrees (180 = south).
    pub azimuth_deg: f64,
    /// DC/AC ratio vs the associated inverter.
    #[serde(default = "default_dc_ac_ratio")]
    pub dc_ac_ratio: f64,
    /// PV inverter `model_id`; `None` when PV lands on a hybrid inverter's
    /// MPPTs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pv_inverter_model_id: Option<String>,
    /// PV microinverter count, when microinverter-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microinverter_count: Option<u32>,
}

const fn default_dc_ac_ratio() -> f64 {
    1.2
}

/// Entry kinds under the `registry/` tree (spec §1.1 layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// `batteries/*.json`
    Battery,
    /// `inverters/*.json`
    Inverter,
    /// `controllers/*.json`
    Controller,
    /// `pv_presets/*.json`
    PvPreset,
}

impl EntryKind {
    /// All entry kinds, in canonical (sorted) order.
    pub const ALL: [Self; 4] = [Self::Battery, Self::Inverter, Self::Controller, Self::PvPreset];

    /// Subdirectory name within the registry tree.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Self::Battery => "batteries",
            Self::Inverter => "inverters",
            Self::Controller => "controllers",
            Self::PvPreset => "pv_presets",
        }
    }

    /// Resolve a registry-tree directory name to its kind.
    #[must_use]
    pub fn from_dir(dir: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.dir() == dir)
    }
}

/// One entry record in `catalog.json`: identity plus content hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    /// Path relative to the registry root, e.g. `batteries/tesla_powerwall_2.json`.
    pub path: String,
    /// Entry kind (matches the directory).
    pub kind: EntryKindSerde,
    /// `model_id` (or `preset_id`) declared inside the entry file.
    pub model_id: String,
    /// `entry_version` declared inside the entry file.
    pub entry_version: String,
    /// Lowercase hex SHA-256 of the entry file's UTF-8 bytes.
    pub sha256: String,
}

/// Serde-facing mirror of [`EntryKind`] (snake_case strings in JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKindSerde {
    /// Battery entry.
    Battery,
    /// Inverter entry.
    Inverter,
    /// Controller entry.
    Controller,
    /// PV preset entry.
    PvPreset,
}

impl From<EntryKindSerde> for EntryKind {
    fn from(k: EntryKindSerde) -> Self {
        match k {
            EntryKindSerde::Battery => Self::Battery,
            EntryKindSerde::Inverter => Self::Inverter,
            EntryKindSerde::Controller => Self::Controller,
            EntryKindSerde::PvPreset => Self::PvPreset,
        }
    }
}

impl From<EntryKind> for EntryKindSerde {
    fn from(k: EntryKind) -> Self {
        match k {
            EntryKind::Battery => Self::Battery,
            EntryKind::Inverter => Self::Inverter,
            EntryKind::Controller => Self::Controller,
            EntryKind::PvPreset => Self::PvPreset,
        }
    }
}

/// `catalog.json` manifest (spec §1.1–1.2, §4.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifest {
    /// Single semantic version of the whole registry, bumped on any change.
    pub registry_version: String,
    /// Schema version the entries conform to.
    pub schema_version: String,
    /// Entry index with per-file content hashes.
    pub entries: Vec<CatalogEntry>,
    /// Integrity hash: SHA-256 over the concatenation (in lexicographic
    /// path order) of each entry file's SHA-256 digest bytes, hashed again
    /// (spec Part A §5, normative default).
    pub catalog_sha256: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn efficiency_curve_interpolates_and_clamps() {
        let curve = EfficiencyCurve {
            points: vec![
                EfficiencyPoint { x_kw: 0.5, efficiency: 0.90 },
                EfficiencyPoint { x_kw: 2.0, efficiency: 0.95 },
                EfficiencyPoint { x_kw: 5.0, efficiency: 0.93 },
            ],
            provenance: Provenance::Estimated,
        };
        assert!((curve.eval(0.0) - 0.90).abs() < 1e-12, "clamp below first point");
        assert!((curve.eval(99.0) - 0.93).abs() < 1e-12, "clamp above last point");
        let mid = curve.eval(1.25);
        assert!((mid - 0.925).abs() < 1e-12, "midpoint interp: {mid}");
        // Negative input evaluates by magnitude (charge/discharge symmetric).
        assert!((curve.eval(-1.25) - mid).abs() < 1e-12);
    }

    #[test]
    fn battery_model_roundtrips_spec_example() {
        // Minimal but schema-shaped PW3-flavored entry.
        let json = serde_json::json!({
            "schema_version": "1.0.0",
            "entry_version": "1.0.0",
            "model_id": "tesla.powerwall_3",
            "vendor": "Tesla",
            "display_name": "Tesla Powerwall 3",
            "chemistry": "LFP",
            "coupling": "DCCoupledHybrid",
            "nameplate_energy_kwh": {"value": 13.5, "provenance": "spec", "unit": "kWh"},
            "usable_energy_kwh": {"value": 13.5, "provenance": "spec", "unit": "kWh"},
            "continuous_discharge_power_kw": {"value": 11.5, "provenance": "spec", "unit": "kW"},
            "continuous_charge_power_kw": {"value": 11.5, "provenance": "estimated", "unit": "kW"},
            "soc_window": {"min_soc_frac": 0.0, "max_soc_frac": 1.0, "reserve_floor_frac": 0.2, "provenance": "spec"},
            "charge_efficiency_curve": {"points": [{"x_kw": 0.5, "efficiency": 0.9}, {"x_kw": 11.5, "efficiency": 0.93}], "provenance": "estimated"},
            "discharge_efficiency_curve": {"points": [{"x_kw": 0.5, "efficiency": 0.9}, {"x_kw": 11.5, "efficiency": 0.93}], "provenance": "estimated"},
            "grid_forming_in_backup": true,
            "warranty": {},
            "operating_temperature": {"min_c": -20.0, "max_c": 50.0, "provenance": "spec"},
            "ramp_rate": {"max_kw_per_s": 11.5, "provenance": "estimated"},
            "vendor_api": {"family": "tesla_local_gateway", "auth_style": "bearer_local_login", "endpoints": [], "provenance": "spec"}
        });
        let model: BatteryModel = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(model.model_id, "tesla.powerwall_3");
        assert_eq!(model.chemistry, Chemistry::LFP);
        let back = serde_json::to_value(&model).unwrap();
        assert_eq!(back["model_id"], json["model_id"]);
    }
}
