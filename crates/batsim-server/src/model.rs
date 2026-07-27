//! Request/response documents for the HTTP API.
//!
//! Every type here appears in the generated OpenAPI document; field
//! names on the wire are snake_case and stable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::price::PriceSourceSpec;

// ---------- shared ----------

/// Cursor pagination envelope.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PageInfo {
    /// Opaque cursor for the next page; null when exhausted.
    pub next_cursor: Option<String>,
    /// Whether more results exist.
    pub has_more: bool,
}

/// Pagination query parameters shared by list endpoints.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct PageParams {
    /// Page size (1..=1000, default 100).
    #[param(minimum = 1, maximum = 1000)]
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page.
    #[param(pattern = "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2,3})?$")]
    pub cursor: Option<String>,
}

impl PageParams {
    /// Validated limit.
    ///
    /// # Errors
    /// [`crate::problem::Problem::validation`] when the limit is outside
    /// `1..=1000`.
    pub fn limit(&self) -> crate::problem::ApiResult<usize> {
        match self.limit {
            None => Ok(100),
            Some(n) if (1..=1000).contains(&n) => Ok(n as usize),
            Some(n) => Err(crate::problem::Problem::validation(format!(
                "limit must be within 1..=1000, got {n}"
            ))),
        }
    }
}

/// Battery selection for a home.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BatterySpec {
    /// Battery model id from the device catalog.
    pub model_id: String,
    /// Number of units (>= 1).
    #[schema(minimum = 1, maximum = 16)]
    pub count: u32,
}

/// Inverter selection; omitted means "compose the vendor default".
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InverterSpec {
    /// Inverter model id from the device catalog.
    pub model_id: String,
    /// Number of units (>= 1).
    #[schema(minimum = 1, maximum = 16)]
    #[serde(default = "one")]
    pub quantity: u32,
}

const fn one() -> u32 {
    1
}

/// A numeric draw: a fixed value or a uniform range (fleet templates).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum KwDraw {
    /// Fixed value in kW.
    Fixed(f64),
    /// Uniform draw over `[min, max]` kW.
    Range {
        /// Inclusive range bounds.
        #[schema(min_items = 2, max_items = 2)]
        uniform: [f64; 2],
    },
}

impl KwDraw {
    /// Resolve to a fixed value, drawing in the range if needed.
    /// Ranges are validated before drawing; this never panics.
    #[must_use]
    pub fn resolve(&self, rng: &mut rand_chacha::ChaCha8Rng) -> f64 {
        use rand::Rng;
        match self {
            Self::Fixed(v) => *v,
            Self::Range { uniform } => {
                let width = uniform[1] - uniform[0];
                if width.is_finite() && width > 0.0 {
                    rng.gen_range(uniform[0]..=uniform[1])
                } else {
                    uniform[0]
                }
            }
        }
    }

    /// Fixed value when not a range (single-home requests reject ranges).
    #[must_use]
    pub const fn fixed(&self) -> Option<f64> {
        match self {
            Self::Fixed(v) => Some(*v),
            Self::Range { .. } => None,
        }
    }
}

/// PV array selection.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PvSpec {
    /// Array DC nameplate in kW (a fixed value, or a uniform range in
    /// fleet templates).
    pub peak_kw: KwDraw,
    /// Azimuth in degrees (180 = south); default 180.
    pub azimuth_deg: Option<f64>,
    /// Tilt in degrees; default 25.
    pub tilt_deg: Option<f64>,
}

/// Household load selection.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadSpec {
    /// Home archetype name: `sfh_family`, `sfh_family_ev`,
    /// `sfh_empty_nester`, `sfh_pool`, `townhome`, `apartment`.
    pub archetype: String,
    /// Target annual consumption in kWh; scales the archetype's floor
    /// area heuristically when present.
    #[schema(exclusive_minimum = 0.0)]
    pub annual_kwh: Option<f64>,
}

/// Home siting.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LocationSpec {
    /// ERCOT load zone, e.g. `LZ_NORTH`, `LZ_HOUSTON`, `LZ_WEST`.
    pub ercot_load_zone: String,
    /// Climate zone: IECC-style (`2A`, `3A`, `3B`, `4A`) or a Texas zone
    /// name (`gulf_coast`, `central`, `north`, `west`). Default `central`.
    pub climate_zone: Option<String>,
}

/// Operating modes a home can be commanded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OperatingMode {
    /// Net-zero grid exchange: charge on PV surplus, discharge to cover
    /// load above the reserve.
    SelfConsumption,
    /// Hold state of charge at or above the reserve floor.
    BackupOnly,
    /// Follows self-consumption until tariff schedules are modeled.
    TimeOfUse,
    /// Follow externally supplied setpoints (manual setpoint channel).
    GridServices,
}

// ---------- homes ----------

/// Create-home request.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateHomeRequest {
    /// Fleet to attach the home to (must exist when given).
    pub fleet_id: Option<String>,
    /// Battery selection.
    pub battery: BatterySpec,
    /// Inverter selection (optional; vendor default composed when
    /// omitted).
    pub inverter: Option<InverterSpec>,
    /// PV array (optional).
    pub pv: Option<PvSpec>,
    /// Household load.
    pub load: LoadSpec,
    /// Siting.
    pub location: LocationSpec,
    /// Initial state of charge (fraction of usable; default 0.5).
    #[schema(minimum = 0.0, maximum = 1.0)]
    pub initial_soc: Option<f64>,
}

/// Echo of a home's configuration as validated and defaulted.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HomeConfigDoc {
    /// Fleet membership, if any.
    pub fleet_id: Option<String>,
    /// Battery selection.
    pub battery: BatterySpec,
    /// Composed inverter model id, when any.
    pub inverter_model_id: Option<String>,
    /// Composed controller model id, when any.
    pub controller_model_id: Option<String>,
    /// Resolved PV peak in kW, when PV is present.
    pub pv_peak_kw: Option<f64>,
    /// Load archetype.
    pub load_archetype: String,
    /// ERCOT load zone.
    pub ercot_load_zone: String,
}

/// Live home state.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomeStateDoc {
    /// Mean state of charge across battery units.
    pub soc: f64,
    /// Active operating mode.
    pub mode: OperatingMode,
    /// Battery-system AC power (kW; + discharge).
    pub battery_power_kw: f64,
    /// PV AC power (kW).
    pub pv_power_kw: f64,
    /// Load power (kW).
    pub load_power_kw: f64,
    /// Grid exchange (kW; + import).
    pub grid_power_kw: f64,
    /// Simulation time of the state.
    pub sim_time: String,
    /// Active PV curtailment fraction (0..1).
    pub pv_curtail_frac: f64,
    /// Manual-mode setpoint (kW; + discharge).
    pub manual_setpoint_kw: f64,
}

/// Home document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomeDoc {
    /// Home id.
    pub id: String,
    /// Validated configuration.
    pub config: HomeConfigDoc,
    /// Live state.
    pub state: HomeStateDoc,
    /// Wall-clock creation time.
    pub created_at: String,
}

/// Page of homes.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HomesPage {
    /// Homes.
    pub data: Vec<HomeDoc>,
    /// Pagination.
    pub page: PageInfo,
}

/// Mutable home configuration.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchHomeRequest {
    /// New operating mode.
    pub mode: Option<OperatingMode>,
    /// New backup reserve floor (fraction of usable).
    #[schema(minimum = 0.0, maximum = 1.0)]
    pub reserve_soc: Option<f64>,
}

/// Home list filters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HomeListParams {
    /// Page size.
    #[param(minimum = 1, maximum = 1000)]
    pub limit: Option<u32>,
    /// Continuation cursor.
    #[param(pattern = "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2,3})?$")]
    pub cursor: Option<String>,
    /// Restrict to a fleet.
    pub fleet_id: Option<String>,
    /// Restrict to an operating mode.
    pub mode: Option<OperatingMode>,
    /// Restrict to a load zone.
    pub load_zone: Option<String>,
    /// Restrict to a battery model.
    pub battery_model: Option<String>,
}

// ---------- fleets ----------

/// One weighted archetype entry in a fleet manifest.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchetypeEntry {
    /// Relative weight (need not sum to 1).
    #[schema(exclusive_minimum = 0.0)]
    pub weight: f64,
    /// Home template for this archetype.
    pub template: HomeTemplate,
}

/// A fleet-manifest home template.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HomeTemplate {
    /// Battery selection.
    pub battery: BatterySpec,
    /// Inverter selection (optional).
    pub inverter: Option<InverterSpec>,
    /// PV array (optional; `peak_kw` may be a uniform range).
    pub pv: Option<PvSpec>,
    /// Household load.
    pub load: LoadSpec,
    /// Initial state of charge (default 0.5).
    #[schema(minimum = 0.0, maximum = 1.0)]
    pub initial_soc: Option<f64>,
}

/// Geographic distribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GeoSpec {
    /// Load-zone weights, e.g. `{ "LZ_NORTH": 0.6, "LZ_HOUSTON": 0.4 }`.
    pub ercot_load_zones: BTreeMap<String, f64>,
}

/// Fleet manifest: archetype x count x load-zone distribution.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetManifest {
    /// Fleet name.
    pub name: String,
    /// Expansion seed; re-applying the same manifest yields identical
    /// homes.
    pub seed: u64,
    /// Weighted home templates.
    #[schema(min_items = 1)]
    pub archetypes: Vec<ArchetypeEntry>,
    /// Load-zone distribution (default: all `LZ_NORTH`).
    pub geo: Option<GeoSpec>,
    /// Number of homes to create.
    #[schema(minimum = 1, maximum = 10000)]
    pub count: u32,
}

/// Fleet document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FleetDoc {
    /// Fleet id.
    pub id: String,
    /// Fleet name.
    pub name: String,
    /// Wall-clock creation time.
    pub created_at: String,
    /// Current number of homes.
    pub home_count: usize,
    /// Content hash of the expansion (verification of determinism).
    pub expansion_hash: String,
    /// The manifest the fleet was created from.
    pub manifest: FleetManifest,
}

/// Page of fleets.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FleetsPage {
    /// Fleets.
    pub data: Vec<FleetDoc>,
    /// Pagination.
    pub page: PageInfo,
}

/// Fleet expansion request.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpandFleetRequest {
    /// Additional homes to create from the fleet's manifest.
    #[schema(minimum = 1, maximum = 10000)]
    pub count: u32,
}

// ---------- scenarios ----------

/// Scenario time binding.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioTime {
    /// Run start (RFC 3339 UTC; must be 5-minute aligned).
    #[schema(format = "date-time")]
    pub start: String,
    /// Run end (RFC 3339 UTC).
    #[schema(format = "date-time")]
    pub end: String,
    /// Tick length in seconds (default 1).
    pub tick_seconds: Option<u32>,
}

/// Ambient temperature feed selection.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AmbientSpec {
    /// Constant temperature.
    Constant {
        /// Temperature in deg C.
        c: f64,
    },
    /// Diurnal sinusoid peaking mid-afternoon.
    Diurnal {
        /// Daily mean temperature in deg C.
        mean_c: f64,
        /// Half peak-to-peak swing in deg C.
        amplitude_c: f64,
    },
}

/// Weather binding.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum WeatherSpec {
    /// Deterministic synthetic ambient feed.
    Synthetic {
        /// The ambient feed.
        ambient: AmbientSpec,
    },
    /// Historical weather replay. Not available yet.
    Replay {
        /// Dataset name (reserved).
        dataset: Option<String>,
    },
}

/// A scheduled outage window. Recorded with the scenario; outage physics
/// (grid loss, islanding) is applied once the resilience milestone ships.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutageSpec {
    /// Outage start (RFC 3339 UTC).
    #[schema(format = "date-time")]
    pub start: String,
    /// Outage end (RFC 3339 UTC).
    #[schema(format = "date-time")]
    pub end: String,
    /// Affected load zones (empty = everywhere).
    #[serde(default)]
    pub load_zones: Vec<String>,
    /// Per-home probability of being affected.
    #[serde(default = "one_f")]
    pub probability: f64,
}

const fn one_f() -> f64 {
    1.0
}

/// Scenario creation request (and stored document).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScenarioRequest {
    /// Scenario name.
    pub name: String,
    /// Time binding.
    pub time: ScenarioTime,
    /// Price binding.
    pub prices: PriceSourceSpec,
    /// Weather binding (default: synthetic 25 C constant).
    pub weather: Option<WeatherSpec>,
    /// Scheduled outages.
    #[serde(default)]
    pub outages: Vec<OutageSpec>,
    /// Scenario seed; governs every random draw during the run.
    pub seed: u64,
}

/// Scenario document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ScenarioDoc {
    /// Scenario id.
    pub id: String,
    /// The binding.
    #[serde(flatten)]
    pub request: ScenarioRequest,
    /// Wall-clock creation time.
    pub created_at: String,
    /// Whether this scenario is the active one.
    pub active: bool,
}

/// Page of scenarios.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ScenariosPage {
    /// Scenarios.
    pub data: Vec<ScenarioDoc>,
    /// Pagination.
    pub page: PageInfo,
}

// ---------- sim ----------

/// Simulation run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimState {
    /// Not ticking.
    Stopped,
    /// Ticking at the configured speed.
    Running,
    /// Frozen at a tick boundary.
    Paused,
}

/// Simulation status.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SimStatusDoc {
    /// Run state.
    pub state: SimState,
    /// Current simulation time (RFC 3339 UTC).
    pub sim_time: String,
    /// Current tick index.
    pub tick: u64,
    /// Configured speed multiplier (0 = as fast as possible).
    pub speed: f64,
    /// Measured speed over the recent window.
    pub achieved_speed: f64,
    /// Ticks the scheduler is behind (must return to 0 after bursts).
    pub lag_ticks: u64,
    /// Commands waiting in per-home queues.
    pub queued_commands: usize,
    /// Id of the active scenario, if any.
    pub active_scenario: Option<String>,
}

/// Synchronous step request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StepRequest {
    /// Ticks to advance (at most 86400 unless `allow_large=true`).
    #[schema(minimum = 1)]
    pub ticks: u64,
}

/// Synchronous step / run-until response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StepResponse {
    /// Simulation time reached.
    pub sim_time: String,
    /// Tick index reached.
    pub tick: u64,
    /// Ticks executed.
    pub ticks_executed: u64,
    /// Wall-clock milliseconds taken.
    pub wall_ms: u64,
}

/// Run-until request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunUntilRequest {
    /// Target simulation time (RFC 3339 UTC).
    #[schema(format = "date-time")]
    pub until: String,
}

/// Speed change request.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SpeedRequest {
    /// Sim-seconds per wall-second; 0 = as fast as possible.
    #[schema(minimum = 0.0)]
    pub multiplier: f64,
}

// ---------- dispatch ----------

/// Latency model for per-device execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum LatencySpec {
    /// Fixed latency in milliseconds.
    Fixed(u64),
    /// Uniform draw over `[min, max]` milliseconds.
    Range {
        /// Inclusive bounds.
        #[schema(min_items = 2, max_items = 2)]
        uniform: [u64; 2],
    },
}

/// Target filter within a fleet.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TargetFilter {
    /// Only homes in one of these modes.
    pub mode: Option<Vec<OperatingMode>>,
    /// Only homes with SOC strictly above this fraction.
    #[schema(minimum = 0.0, maximum = 1.0)]
    pub soc_gt: Option<f64>,
}

/// Dispatch target set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TargetSpec {
    /// Target every home in a fleet.
    pub fleet_id: Option<String>,
    /// Target specific homes.
    pub home_ids: Option<Vec<String>>,
    /// Filter within the resolved set.
    pub filter: Option<TargetFilter>,
    /// Deterministic random sub-sample percentage (0..100].
    #[schema(exclusive_minimum = 0.0, maximum = 100.0)]
    pub sample_pct: Option<f64>,
}

/// The dispatch action. Closed union, discriminated by `type`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSpec {
    /// Charge at a fixed power.
    ChargeTo {
        /// Charge power in kW (> 0).
        #[schema(exclusive_minimum = 0.0)]
        kw: f64,
        /// Hold duration in seconds; the home returns to
        /// self-consumption afterwards. Omit to hold indefinitely.
        duration_s: Option<u64>,
    },
    /// Discharge at a fixed power.
    DischargeTo {
        /// Discharge power in kW (> 0).
        #[schema(exclusive_minimum = 0.0)]
        kw: f64,
        /// Hold duration in seconds; the home returns to
        /// self-consumption afterwards. Omit to hold indefinitely.
        duration_s: Option<u64>,
    },
    /// Set the backup reserve floor.
    SetReserveSoc {
        /// Reserve as a fraction of usable energy (0..1).
        #[schema(minimum = 0.0, maximum = 1.0)]
        soc: f64,
    },
    /// Switch operating mode.
    SetMode {
        /// The new mode.
        mode: OperatingMode,
    },
    /// Curtail PV output.
    CurtailPv {
        /// Curtailed fraction of output in percent (0..100).
        #[schema(minimum = 0.0, maximum = 100.0)]
        pct: f64,
    },
    /// Clear all overrides: self-consumption mode, no setpoint, no
    /// curtailment.
    ClearOverride {},
}

/// Execution shaping for a command.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    /// Per-device execution latency; each home draws its own delay,
    /// mimicking a vendor cloud API. Default uniform 250..2000 ms.
    pub latency_ms: Option<LatencySpec>,
    /// Per-device timeout in seconds (default 30).
    pub timeout_s: Option<u64>,
    /// Ramp style; only `immediate` is supported.
    pub ramp: Option<RampStyle>,
}

/// Ramp styles for power commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RampStyle {
    /// Apply the full setpoint at the execution tick.
    Immediate,
}

/// Dispatch request.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequest {
    /// Client-supplied command id (ULID/uuid). Retries with the same id
    /// are deduplicated before enqueue. Server-assigned when omitted.
    pub command_id: Option<String>,
    /// Target set.
    pub target: TargetSpec,
    /// Action.
    pub action: ActionSpec,
    /// Execution shaping.
    pub execution: Option<ExecutionSpec>,
}

/// Command rollup status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Enqueued; no device has executed yet.
    Queued,
    /// At least one device has executed; more pending.
    InFlight,
    /// Every device applied the command.
    Completed,
    /// Finished with at least one partial/rejected/timed-out target.
    CompletedWithErrors,
    /// Cancelled before full execution.
    Cancelled,
}

/// Dispatch acceptance response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DispatchResponse {
    /// The command id (echoed or server-assigned).
    pub command_id: String,
    /// Always true for a 202 response.
    pub accepted: bool,
    /// Number of homes targeted.
    pub targets: usize,
    /// Rollup status at response time.
    pub status: CommandStatus,
    /// Poll URL for execution detail.
    pub status_url: String,
}

/// Per-target execution record.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TargetExecution {
    /// Home id.
    pub home_id: String,
    /// Execution status; null while still queued.
    pub status: Option<TargetStatus>,
    /// Requested power in kW for power actions.
    pub requested_kw: Option<f64>,
    /// Applied power in kW measured at execution.
    pub applied_kw: Option<f64>,
    /// Simulation time of execution.
    pub executed_at_sim_time: Option<String>,
    /// Assigned execution latency in milliseconds.
    pub latency_ms: u64,
}

/// Per-target status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    /// Command fully applied.
    Applied,
    /// Command partially applied (physics limited).
    Partial,
    /// Command rejected by the device.
    Rejected,
    /// Device did not execute within the timeout.
    Timeout,
    /// Cancelled before execution.
    Cancelled,
}

/// Audit-log command record.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CommandDoc {
    /// Command id.
    pub command_id: String,
    /// Rollup status.
    pub status: CommandStatus,
    /// Wall-clock acceptance time.
    pub created_at: String,
    /// Requesting principal.
    pub principal: String,
    /// Idempotency key, when supplied.
    pub idempotency_key: Option<String>,
    /// SHA-256 of the canonical request body.
    pub request_hash: String,
    /// The original request.
    pub request: DispatchRequest,
    /// Per-target execution detail.
    pub targets: Vec<TargetExecution>,
}

/// Page of command records.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CommandsPage {
    /// Commands.
    pub data: Vec<CommandDoc>,
    /// Pagination.
    pub page: PageInfo,
}

/// Command audit-log filters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct CommandListParams {
    /// Page size.
    #[param(minimum = 1, maximum = 1000)]
    pub limit: Option<u32>,
    /// Continuation cursor.
    #[param(pattern = "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2,3})?$")]
    pub cursor: Option<String>,
    /// Filter by target home id.
    pub target: Option<String>,
    /// Filter by rollup status.
    pub status: Option<CommandStatus>,
    /// Only commands accepted at or after this RFC 3339 time.
    #[param(format = "date-time")]
    pub since: Option<String>,
}

// ---------- telemetry ----------

/// Telemetry field allow-list.
pub const TELEMETRY_FIELDS: &[&str] = &[
    "soc",
    "battery_power_kw",
    "pv_power_kw",
    "load_power_kw",
    "grid_power_kw",
    "price_rtm",
];

/// Series resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// Raw ticks.
    #[serde(rename = "1s")]
    S1,
    /// One-minute buckets.
    #[serde(rename = "1m")]
    M1,
    /// Five-minute settlement-interval buckets.
    #[serde(rename = "5m")]
    M5,
    /// Fifteen-minute buckets.
    #[serde(rename = "15m")]
    M15,
    /// One-hour buckets.
    #[serde(rename = "1h")]
    H1,
}

impl Resolution {
    /// Bucket length in seconds.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        match self {
            Self::S1 => 1,
            Self::M1 => 60,
            Self::M5 => 300,
            Self::M15 => 900,
            Self::H1 => 3600,
        }
    }
}

/// Fleet aggregation across homes per bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FleetAgg {
    /// Sum across homes.
    Sum,
    /// Mean across homes.
    Mean,
    /// 95th percentile across homes.
    P95,
}

/// Series query parameters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct SeriesParams {
    /// Comma-separated field list (see the field allow-list).
    pub fields: Option<String>,
    /// Range start, sim time (RFC 3339). Default: earliest retained.
    #[param(format = "date-time")]
    pub from: Option<String>,
    /// Range end, sim time (RFC 3339). Default: latest retained.
    #[param(format = "date-time")]
    pub to: Option<String>,
    /// Bucket resolution (default 1m).
    pub resolution: Option<Resolution>,
    /// Fleet bucket aggregation (fleet series only; default sum).
    pub agg: Option<FleetAgg>,
}

/// Columnar series response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SeriesResponse {
    /// Home id (home series).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_id: Option<String>,
    /// Fleet id (fleet series).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fleet_id: Option<String>,
    /// Bucket resolution.
    pub resolution: Resolution,
    /// Column names.
    pub fields: Vec<String>,
    /// Bucket start timestamps (RFC 3339 UTC).
    pub t: Vec<String>,
    /// Values: `v[row][column]`, aligned with `t` and `fields`.
    pub v: Vec<Vec<f64>>,
}

/// Live-stream query parameters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct StreamParams {
    /// Restrict to a fleet. Mutually exclusive with `home_ids`.
    pub fleet_id: Option<String>,
    /// Restrict to specific homes (comma-separated; at most 500; raw
    /// streams only). Mutually exclusive with `fleet_id`.
    pub home_ids: Option<String>,
    /// `aggregate` for fleet rollups, `raw` for per-home vectors.
    pub fields: Option<String>,
    /// Stream at most one event per N ticks (default 1).
    pub downsample: Option<u64>,
}

// ---------- registry ----------

/// Battery catalog summary.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BatterySummary {
    /// Model id.
    pub model_id: String,
    /// Vendor name.
    pub vendor: String,
    /// Display name.
    pub display_name: String,
    /// Cell chemistry.
    pub chemistry: String,
    /// Coupling topology.
    pub coupling: String,
    /// Usable energy in kWh.
    pub usable_energy_kwh: f64,
    /// Continuous charge power in kW.
    pub continuous_charge_power_kw: f64,
    /// Continuous discharge power in kW.
    pub continuous_discharge_power_kw: f64,
}

/// Inverter catalog summary.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InverterSummary {
    /// Model id.
    pub model_id: String,
    /// Vendor name.
    pub vendor: String,
    /// Display name.
    pub display_name: String,
    /// Topology classification.
    pub topology: String,
    /// Rated continuous AC output in kW.
    pub rated_ac_output_kw: f64,
}

/// Battery list filters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct BatteryListParams {
    /// Filter by vendor substring.
    pub vendor: Option<String>,
    /// Minimum usable capacity (kWh).
    pub min_capacity_kwh: Option<f64>,
    /// Maximum usable capacity (kWh).
    pub max_capacity_kwh: Option<f64>,
    /// Filter by chemistry (`LFP`, `NMC`).
    pub chemistry: Option<String>,
}

/// Inverter list filters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct InverterListParams {
    /// Filter by vendor substring.
    pub vendor: Option<String>,
    /// Minimum rated power (kW).
    pub min_power_kw: Option<f64>,
}

/// Battery list response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BatteryList {
    /// Entries.
    pub data: Vec<BatterySummary>,
}

/// Inverter list response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InverterList {
    /// Entries.
    pub data: Vec<InverterSummary>,
}

/// Catalog version document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegistryVersionDoc {
    /// Registry semantic version.
    pub registry_version: String,
    /// Registry JSON schema version.
    pub schema_version: String,
    /// Catalog integrity hash.
    pub catalog_sha256: String,
    /// Number of battery entries.
    pub batteries: usize,
    /// Number of inverter entries.
    pub inverters: usize,
}

// ---------- system ----------

/// Health document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HealthDoc {
    /// `ok` when the server is ready.
    pub status: String,
    /// Simulation run state.
    pub sim_state: SimState,
    /// Wall-clock uptime in seconds.
    pub uptime_s: u64,
    /// Server version.
    pub version: String,
}

/// Version document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct VersionDoc {
    /// Server version.
    pub version: String,
    /// Build git SHA (when available).
    pub git_sha: String,
    /// Registry version in use.
    pub registry_version: String,
    /// OpenAPI version served.
    pub openapi_version: String,
}

/// Empty success body for action endpoints with nothing to return.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OkDoc {
    /// Always true.
    pub ok: bool,
}
