//! Home load profile synthesis (spec B.6; F9).
//!
//! Per-home, per-tick (stage 1 of B.1.5):
//!
//! ```text
//! P_load(t) = sum_enduses [ S_e(dow, hour, season, zone) * scale_e(archetype) ]
//!           + R_hvac(t)   # thermostat duty cycling, temperature-coupled
//!           + R_app(t)    # marked point process appliance spikes
//!           + R_base(t)   # AR(1) 1-min residual, sigma ~ 60 W
//!           + P_ev(t)     # EV charging session model (load only; V2X out)
//! ```
//!
//! All stochastic draws come from the `LoadNoise` per-tick substream and
//! one-time `LoadPhase` init draws (spec B.1.4). State needed across ticks
//! (AR(1) value, active appliance events, HVAC cycle phase, EV session)
//! lives in the [`LoadModel`] struct, not in RNG state.
//!
//! # Data provenance (spec B.6.2)
//!
//! The shape tables in this module are synthetic engineering estimates
//! calibrated to publicly known Texas residential magnitudes (RECS annual
//! totals, ERCOT seasonal peak timing). They are placeholders for the
//! ResStock/Pecan Street extraction pipeline (B.6.2 SHOULD) and are
//! recorded as such in `assets/DATA_SOURCES.md`. The M1 exit criteria do
//! not include the RECS/KS validation tests; those arrive with the data
//! pipeline.

use serde::{Deserialize, Serialize};

/// HVAC equipment class (spec B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HvacType {
    /// Central vapor-compression AC (+ resistance or gas heat).
    CentralAC,
    /// Heat pump (with aux strips below balance point, B.6.3).
    HeatPump,
    /// Window units.
    WindowUnits,
    /// No mechanical cooling (rare in TX).
    NoHvac,
}

/// Water heating class (spec B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaterHeat {
    /// Electric resistance tank.
    Resistance,
    /// Heat-pump water heater.
    HeatPump,
    /// Gas (no electric load).
    Gas,
}

/// Texas climate zones (spec B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxClimateZone {
    /// Houston: hot-humid.
    GulfCoast,
    /// Austin/San Antonio/Dallas: hot, mixed.
    Central,
    /// Panhandle: colder winters.
    North,
    /// El Paso/Midland: hot-dry.
    West,
}

/// Construction vintage (efficiency factor on HVAC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vintage {
    /// Before 1980.
    Pre1980,
    /// 1980-2000.
    Y1980To2000,
    /// After 2000.
    Post2000,
}

/// EV parameters (spec B.6.1): the EV is a controllable load only; V2X is
/// explicitly out of scope (Part A §5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EvConfig {
    /// EV battery size (kWh).
    pub battery_kwh: f64,
    /// Daily miles driven (weekday average).
    pub daily_miles: f64,
    /// Home charge power (kW), 3.3-11.5.
    pub home_charge_kw: f64,
}

/// Load generator resolution (spec B.6.3). `Min15` disables intra-minute
/// stochastic layers for fast fleet screening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadResolution {
    /// Native 1-min shapes with tick-resolution stochastic layers.
    Min1,
    /// Shape-table values plus scaled noise only.
    Min15,
}

/// Static archetype configuration for one home's load model (B.6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConfig {
    /// Conditioned floor area.
    pub sqft: u32,
    /// HVAC class.
    pub hvac: HvacType,
    /// Water heating class.
    pub water_heat: WaterHeat,
    /// Occupants.
    pub occupancy: u8,
    /// Pool present (pump + seasonal heater).
    pub pool: bool,
    /// EV, if any.
    pub ev: Option<EvConfig>,
    /// Texas climate zone.
    pub climate_zone: TxClimateZone,
    /// Construction vintage.
    pub vintage: Vintage,
    /// Generator resolution.
    pub resolution: LoadResolution,
}

/// Per-home load model: deterministic given `(master_seed, home_entity)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModel {
    // Implemented by the world task: config, phase offsets (from the
    // LoadPhase stream at init), AR(1) residual state, active appliance
    // events, HVAC cycle phase, EV session state, last tick's split.
}

impl LoadModel {
    /// Build from the archetype config. Draws one-time phase/cycle offsets
    /// from `substream(master_seed, home_entity, LoadPhase, 0)` (B.6.3) so
    /// the fleet does not cycle in lockstep.
    #[must_use]
    pub fn new(config: &LoadConfig, master_seed: u64, home_entity: u64) -> Self {
        let _ = (config, master_seed, home_entity);
        todo!("implemented by world task")
    }

    /// Evaluate total home load power (W) for one tick (B.1.5 stage 1).
    ///
    /// `unix_time_s` is the current tick's unix time (drives
    /// dow/hour/season via civil-time math), `tick` the engine tick index
    /// (keys the `LoadNoise` substream), `t_amb_c` the ambient temperature
    /// feed (HVAC coupling).
    pub fn power_w(&mut self, unix_time_s: u64, tick: u64, dt_s: u32, t_amb_c: f64) -> f64 {
        let _ = (unix_time_s, tick, dt_s, t_amb_c);
        todo!("implemented by world task")
    }

    /// The critical-loads share (W) of the most recent [`LoadModel::power_w`]
    /// evaluation (B.6.4): refrigerator, selected lights/plugs, network —
    /// default 25-35 % of average load. Whole-home configs return the full
    /// load. Used by the (M4) islanded balance; computed now so F9's
    /// critical-loads split is real.
    #[must_use]
    pub fn last_critical_w(&self) -> f64 {
        todo!("implemented by world task")
    }
}
