//! PV array model (spec B.7; F10).
//!
//! Pipeline per home array: solar position (pure-function SPA-lite,
//! accuracy <= 0.05 deg) -> clear-sky/scenario GHI-DNI-DHI -> Hay-Davies
//! plane-of-array transposition per sub-array -> PVWatts-style DC derate
//! with cell-temperature correction -> (optionally) seeded cloud-noise
//! overlay -> DC power at the array terminals. AC conversion happens
//! downstream: through a dedicated PV inverter for AC-coupled systems, or
//! through the shared hybrid inverter for DC-coupled systems (B.3.4), so
//! clipping at the shared inverter is resolved by the home tick.
//!
//! # Data provenance (spec B.7.1)
//!
//! NSRDB site series arrive with the scenario pipeline (M2+). M1 ships a
//! deterministic clear-sky irradiance model (documented estimated) as the
//! built-in feed; the architecture accepts an externally supplied series
//! per tick without code changes.

use serde::{Deserialize, Serialize};

/// One roof sub-array (B.7.3: multiple sub-arrays per home MUST be
/// supported).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SubArray {
    /// DC nameplate (kW).
    pub kw_dc: f64,
    /// Tilt (deg; 0 flat, 90 vertical).
    pub tilt_deg: f64,
    /// Azimuth (deg; 180 south, 90 east, 270 west).
    pub azimuth_deg: f64,
}

/// Static PV configuration for one home.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvConfig {
    /// Sub-arrays (>= 1).
    pub sub_arrays: Vec<SubArray>,
    /// Site latitude (deg).
    pub latitude_deg: f64,
    /// Site longitude (deg).
    pub longitude_deg: f64,
    /// Fixed shading derate in [0, 0.3] (B.7.4).
    pub shading_factor: f64,
    /// Enable the seeded cloud-variability overlay (B.7.5; default true).
    pub cloud_noise: bool,
}

/// Solar position result.
#[derive(Debug, Clone, Copy)]
pub struct SolarPosition {
    /// Elevation above horizon (deg).
    pub elevation_deg: f64,
    /// Azimuth from north, eastward (deg).
    pub azimuth_deg: f64,
    /// Extraterrestrial normal irradiance (W/m^2).
    pub ghi_extraterrestrial: f64,
}

/// Clear-sky irradiance decomposition at the surface.
#[derive(Debug, Clone, Copy)]
pub struct Irradiance {
    /// Global horizontal irradiance (W/m^2).
    pub ghi: f64,
    /// Direct normal irradiance (W/m^2).
    pub dni: f64,
    /// Diffuse horizontal irradiance (W/m^2).
    pub dhi: f64,
}

/// Pure-function solar position (SPA-lite class, B.7.1/B.7.2). Deterministic
/// across platforms: only std transcendental fns on the pinned toolchain.
#[must_use]
pub fn solar_position(unix_time_s: u64, latitude_deg: f64, longitude_deg: f64) -> SolarPosition {
    let _ = (unix_time_s, latitude_deg, longitude_deg);
    todo!("implemented by world task")
}

/// Deterministic clear-sky irradiance (estimated built-in feed; see module
/// docs). Zero at/below the horizon.
#[must_use]
pub fn clear_sky(position: &SolarPosition) -> Irradiance {
    let _ = position;
    todo!("implemented by world task")
}

/// Hay-Davies plane-of-array transposition (spec B.7.2: picked over Perez
/// for simplicity).
#[must_use]
pub fn poa_irradiance(
    irr: &Irradiance,
    position: &SolarPosition,
    tilt_deg: f64,
    azimuth_deg: f64,
) -> f64 {
    let _ = (irr, position, tilt_deg, azimuth_deg);
    todo!("implemented by world task")
}

/// Per-home PV array with cloud-noise state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvArray {
    // Implemented by the world task: config, Markov sky-state + AR(1)
    // flicker state, hourly energy-neutral normalization accumulators
    // (B.7.5), per-home azimuth jitter (PvPhase stream, B.7.3).
}

impl PvArray {
    /// Build from config. Draws the one-time per-home azimuth jitter
    /// (B.7.3, +/-20 deg) from `substream(master_seed, home_entity,
    /// PvPhase, 0)`.
    #[must_use]
    pub fn new(config: &PvConfig, master_seed: u64, home_entity: u64) -> Self {
        let _ = (config, master_seed, home_entity);
        todo!("implemented by world task")
    }

    /// Total DC power at the array terminals for this tick (W): summed
    /// sub-array POA -> PVWatts-style derate (`gamma_pdc = -0.0035/degC`,
    /// `dT_noct = 30 degC`, ~14 % system losses stack, soiling/shading) ->
    /// optional cloud overlay with hourly energy-neutral normalization
    /// (B.7.5: settlement energies match the smooth feed within +/-2 %).
    pub fn dc_power_w(&mut self, unix_time_s: u64, tick: u64, dt_s: u32, t_amb_c: f64) -> f64 {
        let _ = (unix_time_s, tick, dt_s, t_amb_c);
        todo!("implemented by world task")
    }

    /// Total DC nameplate (W).
    #[must_use]
    pub fn nameplate_dc_w(&self) -> f64 {
        todo!("implemented by world task")
    }
}
