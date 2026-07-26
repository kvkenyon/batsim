//! Home load profile synthesis (spec B.6; F9).
//!
//! Per-home, per-tick (stage 1 of B.1.5):
//!
//! ```text
//! P_load(t) = sum_enduses [ S_e(dow, hour, season, zone) * scale_e(archetype) ]
//! + R_hvac(t) # thermostat duty cycling, temperature-coupled
//! + R_app(t) # marked point process appliance spikes
//! + R_base(t) # AR(1) 1-min residual, sigma ~ 60 W
//! + P_ev(t) # EV charging session model (load only; V2X out)
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
//! not include the RECS/KS validation tests (M2+).
//!
//! # M1 model choices (documented estimates)
//!
//! - **Shape tables**: average kW of a reference home (2400 sqft, 2.8
//!   occupants, CentralAC, Post2000, TX_Central) per end use, indexed by
//!   day-type x season x hour-of-day, linearly interpolated within the
//!   hour and held constant within each 1-min block (B.6.3 native 1-min
//!   resolution). End uses: hvac, water_heat, plug (background
//!   appliances/standby — discrete spikes live in `R_app`), lighting,
//!   pool (schedule window), plus the EV session model. Zone enters
//!   through the HVAC climate factor only (M1 simplification, recorded
//!   in DATA_SOURCES.md).
//! - **Scaling laws** (B.6.3 exactly): HVAC ∝ sqft x climate factor x
//!   vintage factor; water heat ∝ occupancy; plug/lighting ∝
//!   sqft^0.7 x occupancy^0.5.
//! - **Civil time**: pure integer math at fixed UTC-6 (no DST, no chrono,
//!   no wall clock — B.1.1). America/Chicago DST is ignored; Texas CDT ~=
//!   CST for load-shape purposes (documented simplification).
//! - **RNG discipline**: every `Min1`-mode tick draws exactly five values
//!   from the per-tick substream in fixed order (arrival, signature,
//!   duration, two Box-Muller uniforms), independent of config and state,
//!   so homes differing only in one config knob keep aligned streams and
//!   differences isolate that knob's contribution. EV sessions draw from
//!   a separate per-day-keyed substream (stateless; reproducible no
//!   matter when the scenario starts).

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::math;
use crate::pv::{civil_local, normal_from_uniforms, Season};
use crate::rng::{self, RngPurpose};

/// HVAC equipment class (B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HvacType {
    /// Central air conditioner (resistance or gas backup heat in winter;
    /// the M1 winter shape covers whatever electric share exists).
    CentralAC,
    /// Heat pump (aux strip heat below the ~2 C balance point, B.6.3).
    HeatPump,
    /// Window units (partial-home coverage factor).
    WindowUnits,
    /// No electric HVAC (rare in TX, B.6.1).
    None,
}

/// Water heater class (B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaterHeat {
    /// Electric resistance tank.
    Resistance,
    /// Heat-pump water heater (~0.45x energy factor).
    HeatPump,
    /// Gas (electric load is controls/standby only).
    Gas,
}

/// Texas climate zones (B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxClimateZone {
    /// Houston — hot-humid.
    GulfCoast,
    /// Austin / San Antonio / Dallas — hot, mixed.
    Central,
    /// Panhandle — colder winters.
    North,
    /// El Paso / Midland — hot-dry.
    West,
}

/// Construction vintage (B.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vintage {
    /// Pre-1980 (leaky envelope, old equipment).
    Pre1980,
    /// 1980-2000.
    Y1980_2000,
    /// Post-2000 (code envelope, SEER 14+).
    Post2000,
}

/// EV configuration (B.6.1): the EV is a controllable load only; V2X is
/// explicitly out of scope (Part A §5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EvConfig {
    /// Battery capacity (kWh; bounds the session energy implicitly).
    pub battery_kwh: f64,
    /// Miles driven per day (session energy = miles x 0.28 kWh/mile,
    /// B.6.3).
    pub daily_miles: f64,
    /// Home charge power (kW, 3.3-11.5 per B.6.3).
    pub home_charge_kw: f64,
}

/// Load generator resolution (spec B.6.3). `Min15` disables intra-minute
/// stochastic layers for fast fleet screening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadResolution {
    /// Native 1-min shapes with tick-resolution stochastic layers.
    Min1,
    /// Shape tables plus one scaled noise draw per 15-min block.
    Min15,
}

/// Static archetype configuration for one home's load model (B.6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConfig {
    /// Conditioned floor area (sqft; 800..6000 per B.6.1).
    pub sqft: u32,
    /// HVAC equipment class.
    pub hvac: HvacType,
    /// Water heater class.
    pub water_heat: WaterHeat,
    /// Occupants.
    pub occupancy: u8,
    /// Pool present (pump schedule end use).
    pub pool: bool,
    /// EV config, if an EV charges at home.
    pub ev: Option<EvConfig>,
    /// Texas climate zone (weather + HVAC calibration).
    pub climate_zone: TxClimateZone,
    /// Construction vintage.
    pub vintage: Vintage,
    /// Generator resolution (B.6.3).
    pub resolution: LoadResolution,
}

/// Reference-home floor area for the shape tables (sqft).
const REF_SQFT: f64 = 2400.0;
/// Reference-home occupancy for the shape tables (persons).
const REF_OCC: f64 = 2.8;

/// HVAC climate factors `(cooling, heating)` per zone (B.6.3 climate
/// factor; fitted-order estimates: Gulf humidity raises cooling and
/// milds winters, Panhandle reverses, West dust-belt is hot-dry with
/// cold desert nights).
fn climate_factors(zone: TxClimateZone) -> (f64, f64) {
    match zone {
        TxClimateZone::GulfCoast => (1.10, 0.80),
        TxClimateZone::Central => (1.00, 1.00),
        TxClimateZone::North => (0.80, 1.45),
        TxClimateZone::West => (1.05, 1.10),
    }
}

/// HVAC vintage efficiency factor (B.6.3): envelope + equipment.
fn vintage_factor(vintage: Vintage) -> f64 {
    match vintage {
        Vintage::Pre1980 => 1.25,
        Vintage::Y1980_2000 => 1.00,
        Vintage::Post2000 => 0.85,
    }
}

/// HVAC type factor on the shape-table power (coverage/efficiency):
/// heat pumps cool slightly more efficiently, window units cover a
/// fraction of the home.
fn hvac_type_factor(hvac: HvacType) -> f64 {
    match hvac {
        HvacType::CentralAC => 1.00,
        HvacType::HeatPump => 0.90,
        HvacType::WindowUnits => 0.60,
        HvacType::None => 0.0,
    }
}

/// Compressor rated power (kW) of the reference 2400-sqft home by HVAC
/// type; the duty cycle converts shape-table average power into
/// on/off cycling at this rating (B.6.3 hysteresis cycling).
fn hvac_rated_kw(hvac: HvacType) -> f64 {
    match hvac {
        HvacType::CentralAC => 3.4,
        HvacType::HeatPump => 3.0,
        HvacType::WindowUnits => 1.5,
        HvacType::None => 0.0,
    }
}

/// Water-heater energy factor by class.
fn water_factor(water_heat: WaterHeat) -> f64 {
    match water_heat {
        WaterHeat::Resistance => 1.00,
        WaterHeat::HeatPump => 0.45,
        WaterHeat::Gas => 0.05,
    }
}

/// Multiply all 24 hourly entries by a constant (table authoring at
/// const-eval time).
const fn mul24(mut a: [f64; 24], f: f64) -> [f64; 24] {
    let mut i = 0;
    while i < 24 {
        a[i] *= f;
        i += 1;
    }
    a
}

/// Shape-table set for one end use: average kW of the reference home,
/// indexed [season][day_type][hour]. Seasons: 0 summer, 1 winter,
/// 2 shoulder (see [`Season`]); day types: 0 weekday, 1 weekend.
struct ShapeSet {
    /// Weekday rows per season.
    weekday: [[f64; 24]; 3],
    /// Weekend rows per season.
    weekend: [[f64; 24]; 3],
}

impl ShapeSet {
    /// Interpolated table value (kW) at (season, weekend?, hour, minute);
    /// linear between adjacent hourly nodes (B.6.3 15-min-resolution
    /// lookup with linear interpolation; hour-of-day authoring grid).
    fn value(&self, season: Season, weekend: bool, hour: usize, minute: usize) -> f64 {
        let rows = if weekend {
            &self.weekend
        } else {
            &self.weekday
        };
        let row = &rows[match season {
            Season::Summer => 0,
            Season::Winter => 1,
            Season::Shoulder => 2,
        }];
        let frac = minute as f64 / 60.0;
        let a = row[hour];
        let b = row[(hour + 1) % 24];
        a + (b - a) * frac
    }
}

/// HVAC shape (kW avg incl. duty cycling): summer Central-Texas cooling
/// runs through the night (27 C+ overnight lows), peaks 16:00 with the
/// ERCOT 4CP window (B.6.3 peak-coincidence target); winter has the
/// morning resistance-heat ramp; shoulder is mild.
const HVAC_SHAPES: ShapeSet = ShapeSet {
    weekday: [
        // summer weekday
        [
            1.45, 1.38, 1.32, 1.28, 1.25, 1.22, 1.25, 1.30, 1.36, 1.42, 1.52, 1.64, 1.75, 1.85,
            1.92, 1.98, 2.00, 1.96, 1.88, 1.78, 1.68, 1.60, 1.53, 1.48,
        ],
        // winter weekday
        [
            0.85, 0.80, 0.76, 0.74, 0.78, 0.95, 1.25, 1.45, 1.30, 0.95, 0.72, 0.60, 0.56, 0.54,
            0.56, 0.62, 0.78, 0.95, 1.12, 1.18, 1.10, 1.00, 0.92, 0.88,
        ],
        // shoulder weekday
        [
            0.30, 0.28, 0.26, 0.25, 0.26, 0.30, 0.38, 0.44, 0.46, 0.44, 0.42, 0.44, 0.48, 0.52,
            0.55, 0.57, 0.55, 0.52, 0.48, 0.45, 0.42, 0.38, 0.35, 0.32,
        ],
    ],
    weekend: [
        // summer weekend (later ramp, occupied midday)
        [
            1.45, 1.38, 1.32, 1.28, 1.25, 1.22, 1.24, 1.28, 1.36, 1.46, 1.58, 1.70, 1.80, 1.90,
            1.97, 2.02, 2.04, 2.00, 1.92, 1.82, 1.72, 1.63, 1.55, 1.49,
        ],
        // winter weekend
        [
            0.85, 0.80, 0.76, 0.74, 0.76, 0.88, 1.10, 1.30, 1.35, 1.15, 0.90, 0.75, 0.68, 0.65,
            0.66, 0.70, 0.85, 1.00, 1.15, 1.20, 1.12, 1.02, 0.94, 0.89,
        ],
        // shoulder weekend
        mul24(
            [
                0.30, 0.28, 0.26, 0.25, 0.26, 0.30, 0.38, 0.44, 0.46, 0.44, 0.42, 0.44, 0.48, 0.52,
                0.55, 0.57, 0.55, 0.52, 0.48, 0.45, 0.42, 0.38, 0.35, 0.32,
            ],
            1.05,
        ),
    ],
};

/// Water-heat shape (kW): morning and evening hot-water draws on a
/// standby baseline; seasonal rows scale the whole day (colder winter
/// inlet water).
const WATER_SHAPES: ShapeSet = ShapeSet {
    weekday: [
        mul24(WATER_WEEKDAY, 0.92),
        mul24(WATER_WEEKDAY, 1.15),
        WATER_WEEKDAY,
    ],
    weekend: [
        mul24(WATER_WEEKEND, 0.92),
        mul24(WATER_WEEKEND, 1.15),
        WATER_WEEKEND,
    ],
};

/// Water-heat weekday base row (shoulder season).
const WATER_WEEKDAY: [f64; 24] = [
    0.16, 0.14, 0.12, 0.12, 0.15, 0.28, 0.50, 0.58, 0.48, 0.32, 0.25, 0.22, 0.21, 0.20, 0.20, 0.21,
    0.24, 0.30, 0.38, 0.48, 0.52, 0.42, 0.30, 0.20,
];

/// Water-heat weekend base row (morning peak shifted later).
const WATER_WEEKEND: [f64; 24] = [
    0.16, 0.14, 0.12, 0.12, 0.14, 0.22, 0.38, 0.52, 0.58, 0.52, 0.40, 0.30, 0.26, 0.24, 0.23, 0.24,
    0.27, 0.33, 0.40, 0.48, 0.52, 0.44, 0.32, 0.22,
];

/// Plug/background shape (kW): fridge cycling, electronics, standby —
/// the smooth part of appliance load; discrete spikes are `R_app`'s job.
const PLUG_SHAPES: ShapeSet = ShapeSet {
    weekday: [PLUG_WEEKDAY, PLUG_WEEKDAY, PLUG_WEEKDAY],
    weekend: [
        mul24(PLUG_WEEKDAY, 1.10),
        mul24(PLUG_WEEKDAY, 1.10),
        mul24(PLUG_WEEKDAY, 1.10),
    ],
};

/// Plug weekday base row (season-independent in the M1 estimates).
const PLUG_WEEKDAY: [f64; 24] = [
    0.130, 0.120, 0.115, 0.110, 0.110, 0.120, 0.145, 0.170, 0.170, 0.150, 0.140, 0.145, 0.150,
    0.150, 0.150, 0.155, 0.170, 0.185, 0.210, 0.230, 0.230, 0.205, 0.175, 0.150,
];

/// Lighting shape (kW): morning and evening occupancy peaks, seasonal
/// rows scale with day length (winter evenings are long).
const LIGHT_SHAPES: ShapeSet = ShapeSet {
    weekday: [
        mul24(LIGHT_WEEKDAY, 0.90),
        mul24(LIGHT_WEEKDAY, 1.25),
        LIGHT_WEEKDAY,
    ],
    weekend: [
        mul24(LIGHT_WEEKDAY, 0.945),
        mul24(LIGHT_WEEKDAY, 1.3125),
        mul24(LIGHT_WEEKDAY, 1.05),
    ],
};

/// Lighting weekday base row.
const LIGHT_WEEKDAY: [f64; 24] = [
    0.030, 0.020, 0.020, 0.020, 0.030, 0.065, 0.095, 0.085, 0.050, 0.040, 0.040, 0.040, 0.040,
    0.040, 0.040, 0.045, 0.070, 0.120, 0.180, 0.220, 0.220, 0.170, 0.105, 0.055,
];

/// Pool-pump schedule shape (kW): summer runs the filter 08:00-16:00
/// (fractional edge hours absorb the per-home offset), shoulder halves
/// the window, winter keeps a short freeze-protection run.
const POOL_SHAPES: ShapeSet = ShapeSet {
    weekday: [POOL_SUMMER, POOL_WINTER, POOL_SHOULDER],
    weekend: [POOL_SUMMER, POOL_WINTER, POOL_SHOULDER],
};

/// Pool summer row (8 h window at 1.2 kW pump).
const POOL_SUMMER: [f64; 24] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2, 0.3, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0,
];

/// Pool shoulder row (halved window).
const POOL_SHOULDER: [f64; 24] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6, 0.6, 0.6, 0.6, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0,
];

/// Pool winter row (freeze-protection run).
const POOL_WINTER: [f64; 24] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0,
];

/// Poisson arrival rate (events/hour at reference occupancy) for the
/// appliance point process `R_app` (B.6.3): evening-cooking peak,
/// morning routine bump, quiet overnight. Weekend row scales up 15 %
/// (occupants home all day).
const ARRIVAL_WEEKDAY: [f64; 24] = [
    0.09, 0.07, 0.06, 0.06, 0.07, 0.18, 0.40, 0.51, 0.44, 0.31, 0.26, 0.31, 0.35, 0.33, 0.31, 0.33,
    0.40, 0.55, 0.70, 0.77, 0.73, 0.59, 0.40, 0.20,
];

/// Appliance signature table (B.6.3: fixed (power, duration) signatures
/// fitted to Pecan Street circuit data in M2; M1 synthetic estimates):
/// `(power_w, duration_min_lo, duration_min_hi, weight)`.
const SIGNATURES: [(f64, f64, f64, f64); 12] = [
    (1200.0, 1.0, 4.0, 20.0),  // microwave
    (1500.0, 2.0, 6.0, 8.0),   // kettle / coffee maker
    (1500.0, 3.0, 10.0, 6.0),  // hair dryer
    (1000.0, 5.0, 20.0, 6.0),  // vacuum / iron
    (2500.0, 20.0, 60.0, 5.0), // oven
    (2000.0, 10.0, 30.0, 7.0), // range / cooktop
    (1200.0, 30.0, 90.0, 5.0), // dishwasher
    (500.0, 25.0, 50.0, 6.0),  // clothes washer
    (3000.0, 30.0, 60.0, 5.0), // clothes dryer
    (300.0, 20.0, 90.0, 12.0), // TV / PC / hobby
    (1000.0, 2.0, 5.0, 5.0),   // toaster
    (4500.0, 10.0, 25.0, 3.0), // oven broil + range (big cook)
];

/// Nominal HVAC cycle period (s); the per-home draw jitters it +/-10 %
/// (B.6.3). 18 min is a plausible Texas compressor cycle at moderate
/// duty.
const HVAC_PERIOD_S: f64 = 1080.0;
/// AR(1) base-residual sigma (B.6.3: ~60 W, fitted to Pecan Street
/// residuals in M2).
const BASE_SIGMA_W: f64 = 60.0;
/// AR(1) base-residual correlation time (B.6.3: 5 min).
const BASE_TAU_S: f64 = 300.0;
/// Vampire floor: total load never drops below this (B.6.3, 0.05 kW).
const VAMPIRE_FLOOR_W: f64 = 50.0;
/// Min15 scaled-noise sigma (W): a single normal draw per 15-min block
/// standing in for the aggregate of `R_app` + `R_base` at 15-min scale
/// (B.6.3 "shape-table values plus scaled noise").
const MIN15_SIGMA_W: f64 = 120.0;
/// Cap on simultaneously active appliance events (B.10.3: fixed-capacity,
/// allocation-free per tick). Arrivals beyond the cap are dropped.
const MAX_EVENTS: usize = 32;
/// EV session energy per daily mile (Wh/mile; B.6.3: 0.28 kWh/mile).
const EV_WH_PER_MILE: f64 = 280.0;
/// Substream key namespace offsets: per-day EV schedule draws and per-
/// block Min15 noise draws share the `LoadNoise` purpose tag but must not
/// collide with real tick indices (`2^40` ticks = 35 Myr at dt = 1 s).
const DAY_KEY_OFFSET: u64 = 1 << 40;
/// See [`DAY_KEY_OFFSET`].
const MIN15_KEY_OFFSET: u64 = 1 << 41;

/// One active appliance event (preallocated storage in [`LoadModel`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ApplianceEvent {
    /// Tick at which the event ends (exclusive).
    end_tick: u64,
    /// Draw power (W).
    power_w: f64,
}

/// Per-home load model: deterministic given `(master_seed, home_entity)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModel {
    config: LoadConfig,
    master_seed: u64,
    home_entity: u64,
    /// HVAC cycle period after the one-time +/-10 % LoadPhase jitter (s).
    hvac_period_s: f64,
    /// HVAC cycle phase offset (+/-5 min, LoadPhase; B.6.3 fleet
    /// desynchronization).
    hvac_phase_s: f64,
    /// Critical-loads share of total load (B.6.4 default 25-35 %,
    /// per-home LoadPhase draw).
    critical_share: f64,
    /// Heat-pump aux-strip power (W; +3-5 kW step below the 2 C balance
    /// point, per-home LoadPhase draw, B.6.3).
    aux_heat_w: f64,
    /// Pool-schedule offset (+/-30 min, LoadPhase).
    pool_offset_s: f64,
    /// Precomputed HVAC scale: sqft x climate x vintage x type factors.
    hvac_scale: f64,
    /// Precomputed water-heat scale (occupancy x heater factor).
    water_scale: f64,
    /// Precomputed plug/lighting scale: sqft^0.7 x occupancy^0.5.
    plug_scale: f64,
    /// AR(1) base-residual state (W).
    ar1_base_w: f64,
    /// Active appliance events (preallocated to [`MAX_EVENTS`]).
    events: Vec<ApplianceEvent>,
    /// Total load of the last [`LoadModel::power_w`] evaluation (W).
    last_total_w: f64,
    /// Critical-loads power of the last evaluation (W).
    last_critical_w: f64,
}

impl LoadModel {
    /// Construct from static config, applying one-time seeded per-home
    /// draws from the `LoadPhase` stream (B.6.3: HVAC cycle-length jitter
    /// and phase offset; B.6.4: critical-share draw).
    #[must_use]
    pub fn new(config: &LoadConfig, master_seed: u64, home_entity: u64) -> Self {
        let mut phase = rng::substream(master_seed, home_entity, RngPurpose::LoadPhase, 0);
        // Fixed draw order (documented in module docs).
        let hvac_period_s = HVAC_PERIOD_S * (1.0 + 0.2 * (phase.gen::<f64>() - 0.5));
        let hvac_phase_s = 600.0 * (phase.gen::<f64>() - 0.5);
        let critical_share = 0.25 + 0.10 * phase.gen::<f64>();
        let aux_heat_w = 3000.0 + 2000.0 * phase.gen::<f64>();
        let pool_offset_s = 3600.0 * (phase.gen::<f64>() - 0.5);
        // The season-dependent climate factor is applied per tick
        // (`hvac_avg_scale`); the stored scale carries sqft x vintage x
        // type factors only.
        let hvac_scale = f64::from(config.sqft.max(1)) / REF_SQFT
            * vintage_factor(config.vintage)
            * hvac_type_factor(config.hvac);
        let water_scale =
            f64::from(config.occupancy.max(1)) / REF_OCC * water_factor(config.water_heat);
        let plug_scale = math::powf(f64::from(config.sqft.max(1)) / REF_SQFT, 0.7)
            * math::powf(f64::from(config.occupancy.max(1)) / REF_OCC, 0.5);
        let events = Vec::with_capacity(MAX_EVENTS);
        Self {
            config: config.clone(),
            master_seed,
            home_entity,
            hvac_period_s,
            hvac_phase_s,
            critical_share,
            aux_heat_w,
            pool_offset_s,
            hvac_scale,
            water_scale,
            plug_scale,
            ar1_base_w: 0.0,
            events,
            last_total_w: 0.0,
            last_critical_w: 0.0,
        }
    }

    /// Total home load power (W, >= 0) for one tick (stage 1 of B.1.5).
    ///
    /// `t_amb_c` is the scenario ambient feed at this tick (drives the
    /// HVAC duty multiplier and heat-pump aux mode). The evaluation is
    /// allocation-free per tick; every stochastic layer draws from the
    /// `LoadNoise` per-tick substream with a fixed draw count (module
    /// docs).
    #[must_use]
    pub fn power_w(&mut self, unix_time_s: u64, tick: u64, dt_s: u32, t_amb_c: f64) -> f64 {
        let civil = civil_local(unix_time_s);
        let season = Season::of_month(civil.month);
        let weekend = civil.day_of_week == 0 || civil.day_of_week == 6;
        let hour = civil.hour as usize;
        let minute = civil.minute as usize;

        // --- Archetypal shapes (kW -> W at the end) ---
        let hvac_avg_kw =
            HVAC_SHAPES.value(season, weekend, hour, minute) * self.hvac_avg_scale(season);
        let water_w = 1000.0 * WATER_SHAPES.value(season, weekend, hour, minute) * self.water_scale;
        let plug_w = 1000.0 * PLUG_SHAPES.value(season, weekend, hour, minute) * self.plug_scale;
        let light_w = 1000.0 * LIGHT_SHAPES.value(season, weekend, hour, minute) * self.plug_scale;
        let pool_w = if self.config.pool {
            // Per-home offset shifts the schedule window (evaluated at
            // the offset local time).
            let shifted = civil_local(unix_time_s.wrapping_add_signed(self.pool_offset_s as i64));
            1000.0
                * POOL_SHAPES.value(
                    Season::of_month(shifted.month),
                    shifted.day_of_week == 0 || shifted.day_of_week == 6,
                    shifted.hour as usize,
                    shifted.minute as usize,
                )
        } else {
            0.0
        };

        // --- HVAC: duty-cycled or duty-mean, temperature-coupled ---
        let hvac_w = self.hvac_power_w(hvac_avg_kw * 1000.0, season, t_amb_c, unix_time_s);

        // --- EV session (stateless per-day draws) ---
        let ev_w = self.ev_power_w(&civil);

        // --- Stochastic layers by resolution ---
        let (app_w, base_w) = match self.config.resolution {
            LoadResolution::Min1 => self.stochastic_layers(tick, dt_s, hour, weekend),
            LoadResolution::Min15 => (0.0, self.min15_noise_w(unix_time_s)),
        };

        let hvac_or_mean = if self.config.resolution == LoadResolution::Min15 {
            // Min15: duty-mean HVAC (no intra-minute cycling), with the
            // same temperature multiplier and aux mode.
            self.hvac_duty_mean_w(hvac_avg_kw * 1000.0, season, t_amb_c)
        } else {
            hvac_w
        };

        let total = (water_w + plug_w + light_w + pool_w + hvac_or_mean + ev_w + app_w + base_w)
            .max(VAMPIRE_FLOOR_W);
        self.last_total_w = total;
        // B.6.4 M1 model: constant per-home share of the last evaluation
        // (documented; the scenario end-use share table is M2+).
        self.last_critical_w = total * self.critical_share;
        total
    }

    /// Critical-loads power (W) at the last evaluation (B.6.4).
    #[must_use]
    pub fn last_critical_w(&self) -> f64 {
        self.last_critical_w
    }

    /// HVAC shape scale with the season-resolved climate factor
    /// (cooling factor in summer, heating in winter, their average in
    /// shoulder).
    fn hvac_avg_scale(&self, season: Season) -> f64 {
        let (cool, heat) = climate_factors(self.config.climate_zone);
        let climate = match season {
            Season::Summer => cool,
            Season::Winter => heat,
            Season::Shoulder => 0.5 * (cool + heat),
        };
        self.hvac_scale * climate
    }

    /// Temperature multiplier on HVAC duty (documented estimates):
    /// cooling gain 10 %/C above 30 C summer reference, heating gain
    /// 8 %/C below 8 C winter reference, mild shoulder coupling.
    fn temp_multiplier(season: Season, t_amb_c: f64) -> f64 {
        match season {
            Season::Summer => (1.0 + 0.10 * (t_amb_c - 30.0)).clamp(0.0, 1.8),
            Season::Winter => (1.0 + 0.08 * (8.0 - t_amb_c)).clamp(0.0, 1.8),
            Season::Shoulder => (1.0 + 0.06 * (t_amb_c - 20.0)).clamp(0.5, 1.5),
        }
    }

    /// Duty fraction demanded by shape x temperature (and the heat-pump
    /// aux mode, B.6.3: below the 2 C balance point the compressor runs
    /// continuous and the aux strip steps in).
    fn hvac_duty(&self, hvac_avg_w: f64, season: Season, t_amb_c: f64) -> f64 {
        let rated_w = hvac_rated_kw(self.config.hvac) * 1000.0 * self.hvac_scale;
        if rated_w <= 0.0 {
            return 0.0;
        }
        let cold_snap = self.config.hvac == HvacType::HeatPump && t_amb_c < 2.0;
        let duty = hvac_avg_w * Self::temp_multiplier(season, t_amb_c) / rated_w;
        if cold_snap {
            1.0
        } else {
            duty.clamp(0.0, 1.0)
        }
    }

    /// HVAC power with thermostat duty cycling (B.6.3): a fixed per-home
    /// period grid (jittered +/-10 %, phase +/-5 min from LoadPhase) with
    /// on-time = duty x period models the hysteresis on/off pattern while
    /// keeping the cycle-average equal to the shape x temperature demand.
    fn hvac_power_w(&self, hvac_avg_w: f64, season: Season, t_amb_c: f64, unix_time_s: u64) -> f64 {
        let rated_w = hvac_rated_kw(self.config.hvac) * 1000.0 * self.hvac_scale;
        if rated_w <= 0.0 {
            return 0.0;
        }
        let duty = self.hvac_duty(hvac_avg_w, season, t_amb_c);
        let on_time = duty * self.hvac_period_s;
        let cycle_pos = (unix_time_s as f64 + self.hvac_phase_s).rem_euclid(self.hvac_period_s);
        let mut p = if cycle_pos < on_time { rated_w } else { 0.0 };
        if self.config.hvac == HvacType::HeatPump && t_amb_c < 2.0 {
            p += self.aux_heat_w;
        }
        p
    }

    /// Duty-mean HVAC power (Min15 mode; same demand, no cycling).
    fn hvac_duty_mean_w(&self, hvac_avg_w: f64, season: Season, t_amb_c: f64) -> f64 {
        let rated_w = hvac_rated_kw(self.config.hvac) * 1000.0 * self.hvac_scale;
        if rated_w <= 0.0 {
            return 0.0;
        }
        let duty = self.hvac_duty(hvac_avg_w, season, t_amb_c);
        let mut p = duty * rated_w;
        if self.config.hvac == HvacType::HeatPump && t_amb_c < 2.0 {
            p += self.aux_heat_w;
        }
        p
    }

    /// `R_app` + `R_base` for one `Min1` tick: five fixed draws from the
    /// per-tick substream (module docs), Poisson appliance arrivals over
    /// the signature table, AR(1) base residual.
    fn stochastic_layers(
        &mut self,
        tick: u64,
        dt_s: u32,
        hour: usize,
        weekend: bool,
    ) -> (f64, f64) {
        let mut stream = rng::substream(
            self.master_seed,
            self.home_entity,
            RngPurpose::LoadNoise,
            tick,
        );
        let u_arrival: f64 = stream.gen();
        let u_sig: f64 = stream.gen();
        let u_dur: f64 = stream.gen();
        let bm1: f64 = stream.gen();
        let bm2: f64 = stream.gen();

        // Expire finished events (in-place; no allocation).
        self.events.retain(|e| e.end_tick > tick);

        // Poisson arrival: lambda(hour, occupancy) over this tick.
        let base_rate = if weekend {
            ARRIVAL_WEEKDAY[hour] * 1.15
        } else {
            ARRIVAL_WEEKDAY[hour]
        };
        let lambda_h = base_rate * f64::from(self.config.occupancy.max(1)) / REF_OCC;
        let p_arrive = lambda_h * f64::from(dt_s) / 3600.0;
        if u_arrival < p_arrive && self.events.len() < MAX_EVENTS {
            let (power_w, dur_lo, dur_hi) = pick_signature(u_sig);
            let dur_min = dur_lo + u_dur * (dur_hi - dur_lo);
            let dur_ticks = ((dur_min * 60.0) / f64::from(dt_s)).max(1.0) as u64;
            self.events.push(ApplianceEvent {
                end_tick: tick + dur_ticks,
                power_w,
            });
        }
        let app_w: f64 = self.events.iter().map(|e| e.power_w).sum();

        // AR(1) base residual, sigma 60 W, 5-min correlation.
        let phi = math::exp(-f64::from(dt_s) / BASE_TAU_S);
        let eps = normal_from_uniforms(bm1, bm2);
        self.ar1_base_w = phi * self.ar1_base_w + BASE_SIGMA_W * (1.0 - phi * phi).sqrt() * eps;

        (app_w, self.ar1_base_w)
    }

    /// Min15 scaled noise: one normal draw per 15-min block, held
    /// constant within the block (B.6.3; stateless block-keyed stream).
    fn min15_noise_w(&self, unix_time_s: u64) -> f64 {
        let block = unix_time_s / 900;
        let mut stream = rng::substream(
            self.master_seed,
            self.home_entity,
            RngPurpose::LoadNoise,
            MIN15_KEY_OFFSET + block,
        );
        let bm1: f64 = stream.gen();
        let bm2: f64 = stream.gen();
        MIN15_SIGMA_W * normal_from_uniforms(bm1, bm2)
    }

    /// EV charging power (W): evening plug-in session per day (B.6.3).
    /// Stateless: the day's schedule draws from a day-keyed substream, so
    /// the evaluation is identical no matter when the scenario starts.
    /// Sessions spill past midnight when the energy requires it (the
    /// previous day's session is evaluated alongside today's).
    fn ev_power_w(&self, civil: &crate::pv::CivilLocal) -> f64 {
        let Some(ev) = self.config.ev else {
            return 0.0;
        };
        let charge_w = ev.home_charge_kw * 1000.0;
        if charge_w <= 0.0 {
            return 0.0;
        }
        let energy_wh = (ev.daily_miles * EV_WH_PER_MILE).min(ev.battery_kwh * 1000.0);
        let dur_s = energy_wh / charge_w * 3600.0;
        let sec_of_day = civil.sec_of_day as f64;
        let mut p = 0.0;
        // Today and yesterday (midnight spill).
        for day_offset in [0u64, 1] {
            let day = civil.day_number.saturating_sub(day_offset);
            let mut stream = rng::substream(
                self.master_seed,
                self.home_entity,
                RngPurpose::LoadNoise,
                DAY_KEY_OFFSET + day,
            );
            let u_plug: f64 = stream.gen();
            // Plug-in ~18:00 +/- 2 h (B.6.3 weekday commuters; M1 uses the
            // same schedule every day, documented simplification).
            let plug_s = (16.0 + 4.0 * u_plug) * 3600.0;
            let local_s = if day_offset == 0 {
                sec_of_day
            } else {
                // Seconds into yesterday's session frame.
                sec_of_day + 86_400.0
            };
            if local_s >= plug_s && local_s < plug_s + dur_s {
                p += charge_w;
            }
        }
        p
    }
}

/// Weighted signature pick by cumulative weight over `u in [0, 1)`.
fn pick_signature(u: f64) -> (f64, f64, f64) {
    let total: f64 = SIGNATURES.iter().map(|s| s.3).sum();
    let mut acc = 0.0;
    for &(power_w, lo, hi, w) in &SIGNATURES {
        acc += w / total;
        if u < acc {
            return (power_w, lo, hi);
        }
    }
    let &(power_w, lo, hi, _) = &SIGNATURES[SIGNATURES.len() - 1];
    (power_w, lo, hi)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 2026-07-06T00:00:00Z (Monday).
    const MON: u64 = 1_783_296_000;
    /// 2026-01-15T00:00:00Z (Thursday, winter).
    const JAN15: u64 = 1_768_435_200;
    /// 2026-01-01T00:00:00Z (Thursday, annual run start).
    const JAN1: u64 = 1_767_225_600;

    /// Reference archetype: 2400 sqft CentralAC family, Central TX.
    fn reference_config() -> LoadConfig {
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

    /// Summer ambient feed (C): clear diurnal cycle, 26 C night / 38 C
    /// afternoon, plausible Central-Texas July.
    fn summer_t_amb(unix_time_s: u64) -> f64 {
        let c = civil_local(unix_time_s);
        let h = c.sec_of_day as f64 / 3600.0;
        32.0 + 6.0 * crate::math::cos((h - 15.0) / 24.0 * 2.0 * std::f64::consts::PI)
    }

    #[test]
    fn determinism_same_seed_different_entity() {
        let cfg = reference_config();
        let mut a1 = LoadModel::new(&cfg, 11, 0x0010_1000);
        let mut a2 = LoadModel::new(&cfg, 11, 0x0010_1000);
        let mut b = LoadModel::new(&cfg, 11, 0x0010_1001);
        let mut identical = true;
        let mut differs = false;
        for tick in 0..(7 * 1440) {
            let t = MON + tick * 60;
            let amb = summer_t_amb(t);
            let p1 = a1.power_w(t, tick, 60, amb);
            let p2 = a2.power_w(t, tick, 60, amb);
            let pb = b.power_w(t, tick, 60, amb);
            if p1.to_bits() != p2.to_bits() {
                identical = false;
            }
            if p1.to_bits() != pb.to_bits() {
                differs = true;
            }
        }
        assert!(identical, "same seed+entity must be bit-identical");
        assert!(differs, "different entity must diverge");
    }

    #[test]
    fn series_nonnegative_above_vampire_floor() {
        let mut m = LoadModel::new(&reference_config(), 3, 0x0009_9000);
        let mut min_p = f64::MAX;
        for tick in 0..(7 * 1440) {
            let t = MON + tick * 60;
            let p = m.power_w(t, tick, 60, summer_t_amb(t));
            assert!(p.is_finite());
            min_p = min_p.min(p);
        }
        assert!(min_p >= VAMPIRE_FLOOR_W, "min {min_p} below vampire floor");
    }

    #[test]
    fn annual_energy_within_band() {
        // Full-year run at dt = 60 s for the reference home: the synthetic
        // tables must land in the 8-16 MWh Texas band (B.6.3 / RECS
        // quartile sanity, M1 estimate level).
        let mut m = LoadModel::new(&reference_config(), 8, 0x0007_7000);
        let mut wh = 0.0;
        let ticks_per_day = 1440u64;
        for tick in 0..(365 * ticks_per_day) {
            let t = JAN1 + tick * 60;
            wh += m.power_w(t, tick, 60, summer_t_amb(t)) * 60.0 / 3600.0;
        }
        let mwh = wh / 1.0e6;
        assert!((8.0..=16.0).contains(&mwh), "annual {mwh} MWh outside band");
    }

    #[test]
    fn summer_afternoon_exceeds_overnight() {
        let mut m = LoadModel::new(&reference_config(), 8, 0x0007_7001);
        let mut afternoon = 0.0;
        let mut overnight = 0.0;
        for tick in 0..(7 * 1440) {
            let t = MON + tick * 60;
            let p = m.power_w(t, tick, 60, summer_t_amb(t));
            let hour = (t / 3600 + 18) % 24; // UTC-6
            if (14..18).contains(&hour) {
                afternoon += p;
            }
            if hour < 6 {
                overnight += p;
            }
        }
        afternoon /= 4.0 * 7.0;
        overnight /= 6.0 * 7.0;
        assert!(
            afternoon > 1.8 * overnight,
            "afternoon {afternoon} W vs overnight {overnight} W"
        );
    }

    #[test]
    fn summer_peak_plausible_for_reference_home() {
        // 2400 sqft CentralAC home: July-afternoon per-home peaks land in
        // the 3-7 kW band (task acceptance).
        let mut m = LoadModel::new(&reference_config(), 9, 0x0007_7002);
        let mut peak = 0.0f64;
        for tick in 0..(7 * 1440) {
            let t = MON + tick * 60;
            let hour = (t / 3600 + 18) % 24;
            if (14..18).contains(&hour) {
                peak = peak.max(m.power_w(t, tick, 60, summer_t_amb(t)));
            }
        }
        assert!(
            (3000.0..=7000.0).contains(&peak),
            "reference summer afternoon peak {peak} W"
        );
    }

    #[test]
    fn fleet_of_200_load_factor_in_band() {
        // B.6.3 mandatory target: fleet-average load factor 0.45-0.6.
        // 200 mixed-archetype homes over a July week (Mon-Sun), dt = 60 s.
        let mut fleet: Vec<LoadModel> = (0..200u64)
            .map(|i| {
                let cfg = LoadConfig {
                    sqft: 1200 + u32::try_from((i * 37) % 2400).unwrap(),
                    hvac: match i % 20 {
                        0..=11 => HvacType::CentralAC,
                        12..=16 => HvacType::HeatPump,
                        17 | 18 => HvacType::WindowUnits,
                        _ => HvacType::None,
                    },
                    water_heat: match i % 10 {
                        0..=6 => WaterHeat::Resistance,
                        7 | 8 => WaterHeat::HeatPump,
                        _ => WaterHeat::Gas,
                    },
                    occupancy: u8::try_from(1 + i % 5).unwrap(),
                    pool: i % 4 == 0,
                    ev: if i % 3 == 0 {
                        Some(EvConfig {
                            battery_kwh: 60.0,
                            daily_miles: 25.0 + (i % 7) as f64 * 5.0,
                            home_charge_kw: 7.2,
                        })
                    } else {
                        None
                    },
                    climate_zone: match i % 8 {
                        0..=3 => TxClimateZone::Central,
                        4 | 5 => TxClimateZone::GulfCoast,
                        6 => TxClimateZone::North,
                        _ => TxClimateZone::West,
                    },
                    vintage: match i % 6 {
                        0 | 1 => Vintage::Pre1980,
                        2 | 3 => Vintage::Y1980_2000,
                        _ => Vintage::Post2000,
                    },
                    resolution: LoadResolution::Min1,
                };
                LoadModel::new(&cfg, 0x000F_1E57, 0x0005_5000 + i)
            })
            .collect();
        let mut total_energy_wh = 0.0f64;
        let mut peak_fleet_w = 0.0f64;
        let days = 7u64;
        for tick in 0..(days * 1440) {
            let t = MON + tick * 60;
            let amb = summer_t_amb(t);
            let mut fleet_w = 0.0;
            for m in &mut fleet {
                fleet_w += m.power_w(t, tick, 60, amb);
            }
            total_energy_wh += fleet_w / 60.0;
            peak_fleet_w = peak_fleet_w.max(fleet_w);
        }
        let mean_w = total_energy_wh / (f64::from(days as u32) * 24.0);
        let load_factor = mean_w / peak_fleet_w;
        assert!(
            (0.45..=0.60).contains(&load_factor),
            "fleet load factor {load_factor} outside 0.45-0.6 (mean {mean_w} W, peak {peak_fleet_w} W)"
        );
    }

    #[test]
    fn ev_evening_energy_matches_miles() {
        // Same seed+entity, EV vs no-EV: aligned streams isolate the EV
        // contribution exactly (module docs, RNG discipline).
        let mut cfg_ev = reference_config();
        cfg_ev.ev = Some(EvConfig {
            battery_kwh: 75.0,
            daily_miles: 40.0,
            home_charge_kw: 7.2,
        });
        let mut with_ev = LoadModel::new(&cfg_ev, 21, 0x0008_8000);
        let mut without_ev = LoadModel::new(&reference_config(), 21, 0x0008_8000);
        // One full LOCAL day (UTC-6): start at 06:00Z = local midnight of
        // Monday 2026-07-06, so exactly one EV session falls inside.
        let day_start = MON + 6 * 3600;
        let mut day_wh = 0.0;
        let mut evening_wh = 0.0;
        for tick in 0..1440u64 {
            let t = day_start + tick * 60;
            let amb = summer_t_amb(t);
            let diff = with_ev.power_w(t, tick, 60, amb) - without_ev.power_w(t, tick, 60, amb);
            assert!(diff >= 0.0, "EV must only add load");
            day_wh += diff / 60.0;
            let hour = (t / 3600 + 18) % 24;
            if (15..23).contains(&hour) {
                evening_wh += diff / 60.0;
            }
        }
        let expected = 40.0 * 0.28; // kWh
        let measured_day = day_wh / 1000.0;
        let measured_evening = evening_wh / 1000.0;
        assert!(
            (measured_day - expected).abs() <= 0.3,
            "daily EV energy {measured_day} kWh vs expected {expected}"
        );
        assert!(
            (measured_evening - expected).abs() <= 0.3,
            "evening EV energy {measured_evening} kWh vs expected {expected}"
        );
    }

    #[test]
    fn heat_pump_aux_heat_cold_snap() {
        // At -2 C ambient the heat pump's aux strip adds a 3-5 kW step
        // (B.6.3) that CentralAC lacks; aligned streams isolate HVAC.
        let mut hp_cfg = reference_config();
        hp_cfg.hvac = HvacType::HeatPump;
        let mut hp = LoadModel::new(&hp_cfg, 33, 0x0006_6000);
        let mut ac = LoadModel::new(&reference_config(), 33, 0x0006_6000);
        let mut diff_kwh = 0.0;
        for tick in 0..1440u64 {
            let t = JAN15 + tick * 60;
            let diff = hp.power_w(t, tick, 60, -2.0) - ac.power_w(t, tick, 60, -2.0);
            diff_kwh += diff / 60.0 / 1000.0;
        }
        // Aux strip ~3-5 kW all day at -2 C -> 72-120 kWh; allow duty
        // differences between the two HVAC types.
        assert!(diff_kwh > 40.0, "aux-heat day difference {diff_kwh} kWh");
    }

    #[test]
    fn min15_mode_shapes_plus_scaled_noise() {
        // Min15 disables the intra-minute stochastic layers: within a
        // 15-min block the only variation is the deterministic shape
        // interpolation and HVAC duty-mean, so consecutive-tick jumps are
        // small; Min1 shows large jumps (HVAC duty cycling, appliance
        // spikes) — the metric must be able to tell the two apart.
        let mut cfg = reference_config();
        cfg.resolution = LoadResolution::Min15;
        let mut m = LoadModel::new(&cfg, 44, 0x0005_5001);
        let mut native = LoadModel::new(&reference_config(), 44, 0x0005_5001);
        let mut min15_jump = 0.0f64;
        let mut native_jump = 0.0f64;
        let mut prev_m = f64::NAN;
        let mut prev_s = f64::NAN;
        let mut e_native5 = 0.0;
        let mut e_native = 0.0;
        for tick in 0..1440u64 {
            let t = MON + tick * 60;
            let amb = summer_t_amb(t);
            let pm = m.power_w(t, tick, 60, amb);
            let ps = native.power_w(t, tick, 60, amb);
            // Block boundaries (15 ticks at dt = 60 s) are excluded: the
            // Min15 scaled-noise draw changes there by design.
            if tick % 15 != 0 {
                min15_jump = min15_jump.max((pm - prev_m).abs());
                native_jump = native_jump.max((ps - prev_s).abs());
            }
            prev_m = pm;
            prev_s = ps;
            e_native5 += pm / 60.0;
            e_native += ps / 60.0;
        }
        assert!(
            min15_jump < 150.0,
            "Min15 within-block jump {min15_jump} W indicates a stochastic layer is active"
        );
        assert!(
            native_jump > 300.0,
            "Min1 within-block jump {native_jump} W: duty cycling/spikes missing?"
        );
        // Min15 daily energy stays in the Min1 ballpark.
        let ratio = e_native5 / e_native;
        assert!(
            (0.85..=1.15).contains(&ratio),
            "Min15/Min1 energy ratio {ratio}"
        );
    }

    #[test]
    fn critical_share_within_spec_band() {
        let m = LoadModel::new(&reference_config(), 55, 0x0004_4000);
        // Exercise one tick so last_* are populated.
        let mut m = m;
        let total = m.power_w(MON + 12 * 3600, 720, 60, 34.0);
        let share = m.last_critical_w() / total;
        assert!(
            (0.25..=0.35).contains(&share),
            "critical share {share} outside 25-35 % (B.6.4)"
        );
    }
}
