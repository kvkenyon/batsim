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
//!
//! # M1 model choices (documented estimates; `assets/DATA_SOURCES.md`)
//!
//! - **Solar position**: NOAA solar calculator series (geom. mean
//!   longitude/anomaly, equation of center, apparent longitude, obliquity
//!   correction, equation of time) — PSA/NOAA-class, accuracy <= 0.05 deg
//!   for 1950-2050 (B.7.2). Extraterrestrial irradiance `G_sc = 1367 W/m^2`
//!   with the Spencer/Iqbal eccentricity-correction day-angle series. Only
//!   `std` `f64` transcendentals (B.10.1: no fast-math flags anywhere in
//!   the workspace profile).
//! - **Clear sky**: Hottel (1976) beam transmittance model A (23 km
//!   visibility standard atmosphere) at fixed 0.2 km site altitude, with
//!   the Liu & Jordan (1960) diffuse transmittance relation and the
//!   Kasten & Young (1989) airmass expression. Documented estimated; the
//!   NSRDB feed replaces it in M2.
//! - **System loss stack** (B.7.2 PVWatts-style): mismatch 2 %, DC wiring
//!   2 %, connections 0.5 %, nameplate rating 1 %, light-induced
//!   degradation 2 %, availability 1 % -> fixed product 0.9179; plus
//!   monthly soiling (zone-proxy table, <= 5 % worst month) and the
//!   per-home `shading_factor`. The PV-inverter loss (~4 %) is applied in
//!   the downstream inverter stage (B.7.2 note for DC-coupled topology),
//!   giving the PVWatts-consistent ~14 % total end to end.
//! - **Cloud overlay** (B.7.5): Markov sky-state chain (clear/partly/
//!   broken; per-season dwell times, fitted-order magnitudes only) plus a
//!   within-state additive AR(1) flicker (sigma up to 30 % of clear-sky
//!   GHI in the broken state, 30 s correlation time). All draws come from
//!   the `PvCloud` per-tick substream. Energy neutrality is enforced by a
//!   causal per-clock-hour servo (see [`PvArray::dc_power_w`]). M1 gap:
//!   the fleet cell-correlation blend (`m = 0.6 m_cell + 0.4 m_local`)
//!   needs a scenario-supplied cell id that `PvConfig` does not carry yet;
//!   only the local process is implemented and the deviation is recorded
//!   in `assets/DATA_SOURCES.md`.

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::rng::{self, RngPurpose};

/// Standard-normal variate from two uniform (0, 1) draws via Box-Muller.
/// Deterministic, `std`-only transcendentals (B.10.1); the workspace has
/// no `rand_distr` dependency and none may be added, so Gaussian draws
/// for the AR(1) layers are transformed in-module.
pub(crate) fn normal_from_uniforms(u1: f64, u2: f64) -> f64 {
    // Map u1 into (0, 1] so the log never sees 0.
    let r = (-2.0 * (1.0 - u1).ln()).sqrt();
    r * (2.0 * std::f64::consts::PI * u2).cos()
}

/// One roof sub-array (B.7.3: multiple sub-arrays per home MUST be
/// supported).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SubArray {
    /// DC nameplate at STC (kW).
    pub kw_dc: f64,
    /// Tilt from horizontal (deg, 0..=90).
    pub tilt_deg: f64,
    /// Azimuth (deg, 0 = N, 90 = E, 180 = S, 270 = W).
    pub azimuth_deg: f64,
}

/// Static PV configuration for one home.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvConfig {
    /// One or more roof sub-arrays; POA computed per sub-array, DC summed.
    pub sub_arrays: Vec<SubArray>,
    /// Site latitude (deg).
    pub latitude_deg: f64,
    /// Site longitude (deg, east positive).
    pub longitude_deg: f64,
    /// Fixed shading derate in [0, 0.3] (B.7.4).
    pub shading_factor: f64,
    /// Enable the seeded cloud-variability overlay (B.7.5).
    pub cloud_noise: bool,
}

/// Solar geometry at one instant.
#[derive(Debug, Clone, Copy, Default)]
pub struct SolarPosition {
    /// Solar elevation above horizon (deg; negative = night).
    pub elevation_deg: f64,
    /// Solar azimuth (deg, 0 = N, 90 = E, 180 = S, 270 = W).
    pub azimuth_deg: f64,
    /// Extraterrestrial normal irradiance (W/m^2) with eccentricity
    /// correction.
    pub extra_terrestrial_w_m2: f64,
}

/// Clear-sky or scenario irradiance at one instant (W/m^2).
#[derive(Debug, Clone, Copy, Default)]
pub struct Irradiance {
    /// Global horizontal irradiance.
    pub ghi_w_m2: f64,
    /// Direct normal irradiance.
    pub dni_w_m2: f64,
    /// Diffuse horizontal irradiance.
    pub dhi_w_m2: f64,
}

/// Seconds the local civil clock lags UTC (fixed UTC-6, no DST; see
/// [`civil_local`]).
pub(crate) const CST_OFFSET_S: u64 = 6 * 3600;

/// Broken-down local civil time at fixed UTC-6 (documented simplification:
/// America/Chicago DST is ignored; Texas CDT ~= CST for load-shape and
/// settlement-hour purposes, and solar geometry is computed from true UTC
/// + longitude independently of this civil clock).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CivilLocal {
    /// Month of year, 1..=12.
    pub(crate) month: u64,
    /// Hour of day, 0..=23.
    pub(crate) hour: u64,
    /// Minute of hour, 0..=59.
    pub(crate) minute: u64,
    /// Day of week, 0 = Sunday .. 6 = Saturday.
    pub(crate) day_of_week: u64,
    /// Day of year, 1..=366.
    pub(crate) day_of_year: u64,
    /// Whole days since 1970-01-01 in the shifted local frame (stable
    /// per-day key for RNG substreams).
    pub(crate) day_number: u64,
    /// Seconds since local midnight.
    pub(crate) sec_of_day: u64,
}

/// Days since 1970-01-01 of a Gregorian date (Howard Hinnant's
/// `days_from_civil`; valid for the full u64 civil range used here).
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * if month > 2 { month - 3 } else { month + 9 } + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Pure integer civil-time math (Howard Hinnant's `civil_from_days`),
/// local frame = UTC - 6 h. No chrono, no wall clock (B.1.1).
pub(crate) fn civil_local(unix_time_s: u64) -> CivilLocal {
    let local = unix_time_s.saturating_sub(CST_OFFSET_S);
    let days = local / 86_400;
    let sec_of_day = local % 86_400;
    // Hinnant civil_from_days: days since 1970-01-01 -> (y, m, d).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    CivilLocal {
        month,
        hour: sec_of_day / 3600,
        minute: (sec_of_day % 3600) / 60,
        // 1970-01-01 was a Thursday (= 4 with Sunday = 0).
        day_of_week: (days + 4) % 7,
        day_of_year: days - days_from_civil(year, 1, 1) + 1,
        day_number: days,
        sec_of_day,
    }
}

/// Solar constant G_sc (W/m^2; Iqbal 1983 / PVWatts convention).
const G_SC: f64 = 1367.0;

/// Degrees-to-radians factor.
const DEG: f64 = std::f64::consts::PI / 180.0;

/// Pure-function solar position (NOAA/PSA-lite; spec B.7.2, accuracy
/// <= 0.05 deg for 1950-2050). Deterministic: only `std` `f64`
/// transcendentals, no iteration (B.10.1).
#[must_use]
pub fn solar_position(unix_time_s: u64, latitude_deg: f64, longitude_deg: f64) -> SolarPosition {
    let julian_day = unix_time_s as f64 / 86_400.0 + 2_440_587.5;
    let jc = (julian_day - 2_451_545.0) / 36_525.0;
    // NOAA Solar Calculator series (Meeus; Reda & Andreas class accuracy).
    let geom_mean_long = (280.466_46 + jc * (36_000.769_83 + jc * 0.000_303_2)).rem_euclid(360.0);
    let geom_mean_anom = 357.529_11 + jc * (35_999.050_29 - 0.000_153_7 * jc);
    let ecc = 0.016_708_634 - jc * (0.000_042_037 + 0.000_000_126_7 * jc);
    let gma_rad = geom_mean_anom * DEG;
    let eq_center = gma_rad.sin() * (1.914_602 - jc * (0.004_817 + 0.000_014 * jc))
        + (2.0 * gma_rad).sin() * (0.019_993 - 0.000_101 * jc)
        + (3.0 * gma_rad).sin() * 0.000_289;
    let sun_true_long = geom_mean_long + eq_center;
    let omega_rad = (125.04 - 1934.136 * jc) * DEG;
    let sun_app_long = sun_true_long - 0.005_69 - 0.004_78 * omega_rad.sin();
    let mean_obliq =
        23.0 + (26.0 + (21.448 - jc * (46.815 + jc * (0.000_59 - jc * 0.001_813))) / 60.0) / 60.0;
    let obliq_rad = (mean_obliq + 0.002_56 * omega_rad.cos()) * DEG;
    let decl_rad = (obliq_rad.sin() * (sun_app_long * DEG).sin()).asin();
    let var_y = (obliq_rad / 2.0).tan().powi(2);
    let gml_rad = geom_mean_long * DEG;
    let eq_time_min = 4.0
        * (var_y * (2.0 * gml_rad).sin() - 2.0 * ecc * gma_rad.sin()
            + 4.0 * ecc * var_y * gma_rad.sin() * (2.0 * gml_rad).cos()
            - 0.5 * var_y * var_y * (4.0 * gml_rad).sin()
            - 1.25 * ecc * ecc * (2.0 * gma_rad).sin())
        / DEG;
    // True solar time (minutes) -> hour angle.
    let utc_min = (unix_time_s % 86_400) as f64 / 60.0;
    let tst_min = (utc_min + eq_time_min + 4.0 * longitude_deg).rem_euclid(1440.0);
    let ha_rad = (tst_min / 4.0 - 180.0) * DEG;
    let lat_rad = latitude_deg * DEG;
    let cos_zen =
        (lat_rad.sin() * decl_rad.sin() + lat_rad.cos() * decl_rad.cos() * ha_rad.cos())
            .clamp(-1.0, 1.0);
    let elevation_deg = 90.0 - cos_zen.acos() / DEG;
    let az_deg = ((ha_rad
        .sin()
        .atan2(ha_rad.cos() * lat_rad.sin() - decl_rad.tan() * lat_rad.cos()))
        / DEG
        + 180.0)
        .rem_euclid(360.0);
    // Eccentricity correction (Spencer 1971 / Iqbal 1983 day-angle series).
    let day_angle =
        2.0 * std::f64::consts::PI * (civil_local(unix_time_s).day_of_year - 1) as f64 / 365.0;
    let e0 = 1.000_11
        + 0.034_221 * day_angle.cos()
        + 0.001_28 * day_angle.sin()
        + 0.000_719 * (2.0 * day_angle).cos()
        + 0.000_077 * (2.0 * day_angle).sin();
    SolarPosition {
        elevation_deg,
        azimuth_deg: az_deg,
        extra_terrestrial_w_m2: G_SC * e0,
    }
}

/// Hottel (1976) model-A transmittance coefficients at 0.2 km altitude
/// (23 km visibility standard atmosphere; `a0 = 0.4237 - 0.00821(6-A)^2`,
/// `a1 = 0.5055 + 0.00595(6.5-A)^2`, `k = 0.2711 + 0.01858(2.5-A)^2` with
/// `A = 0.2 km`). Documented estimated (module docs).
const HOTTEL_A0: f64 = 0.147_52;
/// See [`HOTTEL_A0`].
const HOTTEL_A1: f64 = 0.741_66;
/// See [`HOTTEL_A0`].
const HOTTEL_K: f64 = 0.369_39;

/// Deterministic clear-sky irradiance (estimated built-in feed; see module
/// docs). Zero at/below the horizon.
#[must_use]
pub fn clear_sky(position: &SolarPosition) -> Irradiance {
    if position.elevation_deg <= 0.0 {
        return Irradiance::default();
    }
    let el_rad = position.elevation_deg * DEG;
    let sin_el = el_rad.sin();
    // Kasten & Young (1989) relative optical airmass.
    let airmass = 1.0 / (sin_el + 0.505_72 * (position.elevation_deg + 6.079_95).powf(-1.636_4));
    // Hottel (1976) beam transmittance; Liu & Jordan (1960) diffuse.
    let tau_b = HOTTEL_A0 + HOTTEL_A1 * (-HOTTEL_K * airmass).exp();
    let dni = position.extra_terrestrial_w_m2 * tau_b;
    let tau_d = (0.271 - 0.294 * tau_b).max(0.0);
    let dhi = position.extra_terrestrial_w_m2 * tau_d * sin_el;
    Irradiance {
        ghi_w_m2: dni * sin_el + dhi,
        dni_w_m2: dni,
        dhi_w_m2: dhi,
    }
}

/// Hay-Davies plane-of-array transposition (spec B.7.2: picked over Perez
/// for simplicity). Includes an isotropic ground-reflection term with
/// fixed albedo 0.2 (PVWatts convention). Returns W/m^2 on the plane.
#[must_use]
pub fn poa_irradiance(
    irr: &Irradiance,
    position: &SolarPosition,
    tilt_deg: f64,
    azimuth_deg: f64,
) -> f64 {
    if position.elevation_deg <= 0.0 {
        return 0.0;
    }
    let el_rad = position.elevation_deg * DEG;
    let sin_el = el_rad.sin();
    let tilt_rad = tilt_deg * DEG;
    // Angle of incidence on the tilted plane.
    let cos_aoi = (sin_el * tilt_rad.cos()
        + el_rad.cos() * tilt_rad.sin() * ((position.azimuth_deg - azimuth_deg) * DEG).cos())
    .clamp(-1.0, 1.0);
    let beam = irr.dni_w_m2 * cos_aoi.max(0.0);
    // Hay-Davies anisotropy index and tilt factor (Rb clamped >= 0 so a
    // back-facing plane keeps only its isotropic + ground terms).
    let aniso = if position.extra_terrestrial_w_m2 > 0.0 {
        (irr.dni_w_m2 / position.extra_terrestrial_w_m2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let rb = (cos_aoi / sin_el).max(0.0);
    let diffuse = irr.dhi_w_m2 * ((1.0 - aniso) * 0.5 * (1.0 + tilt_rad.cos()) + aniso * rb);
    let ground = 0.2 * irr.ghi_w_m2 * 0.5 * (1.0 - tilt_rad.cos());
    (beam + diffuse + ground).max(0.0)
}

/// Markov sky states for the cloud overlay (B.7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SkyState {
    /// Clear-sky regime: multiplier mean 1.00, flicker sigma 0.02.
    Clear,
    /// Partly cloudy: multiplier mean 0.85, flicker sigma 0.12.
    Partly,
    /// Broken/overcast: multiplier mean 0.55, flicker sigma 0.30 (the
    /// spec's "up to 30 % of GHI", B.7.5).
    Broken,
}

impl SkyState {
    /// Within-state multiplier mean (fitted-order magnitudes, B.7.5).
    const fn mean(self) -> f64 {
        match self {
            Self::Clear => 1.00,
            Self::Partly => 0.85,
            Self::Broken => 0.55,
        }
    }

    /// Within-state AR(1) flicker sigma (fraction of clear-sky value).
    const fn sigma(self) -> f64 {
        match self {
            Self::Clear => 0.02,
            Self::Partly => 0.12,
            Self::Broken => 0.30,
        }
    }

    /// Mean dwell time (s) by state and season (fitted-order magnitudes
    /// only; B.7.5 asks for a matrix "per zone and season" — M1 resolves
    /// season and collapses zone to a Texas-wide average because
    /// [`PvConfig`] carries lat/lon but no zone id; recorded deviation).
    fn dwell_s(self, season: Season) -> f64 {
        match season {
            Season::Summer => match self {
                Self::Clear => 2400.0,
                Self::Partly => 420.0,
                Self::Broken => 900.0,
            },
            Season::Winter => match self {
                Self::Clear => 1200.0,
                Self::Partly => 600.0,
                Self::Broken => 1500.0,
            },
            Season::Shoulder => match self {
                Self::Clear => 1800.0,
                Self::Partly => 480.0,
                Self::Broken => 1100.0,
            },
        }
    }
}

/// Texas season of the cooling calendar (Jun-Sep cooling, Dec-Feb
/// heating, rest shoulder; shared heuristic with the load model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Season {
    /// June-September.
    Summer,
    /// December-February.
    Winter,
    /// March-May, October-November.
    Shoulder,
}

impl Season {
    /// Season of a 1-based month.
    pub(crate) const fn of_month(month: u64) -> Self {
        match month {
            6..=9 => Self::Summer,
            12 | 1 | 2 => Self::Winter,
            _ => Self::Shoulder,
        }
    }
}

/// Monthly soiling loss fraction (B.7.2 `eta_soiling = 1 - 0.02 *
/// soiling_factor`, worst month <= 5 %). Central-Texas base table
/// (dust/pollen peaks in the dry summer months).
const SOILING_CENTRAL: [f64; 12] = [
    0.010, 0.012, 0.015, 0.020, 0.025, 0.030, 0.035, 0.035, 0.030, 0.025, 0.015, 0.012,
];

/// Monthly soiling table for the site's climate-zone proxy. [`PvConfig`]
/// carries lat/lon but no zone id, so M1 infers a crude zone from
/// coordinates (documented in module docs and `assets/DATA_SOURCES.md`):
/// West (El Paso/Midland dust, x1.4, capped at 5 %), North (Panhandle,
/// x0.9), Gulf Coast (wet, x0.8), otherwise Central.
fn soiling_table(latitude_deg: f64, longitude_deg: f64) -> [f64; 12] {
    let mult = if longitude_deg <= -102.0 {
        1.4
    } else if latitude_deg >= 33.5 {
        0.9
    } else if (latitude_deg <= 30.5 && longitude_deg >= -95.5)
        || (latitude_deg <= 29.0 && longitude_deg >= -98.0)
    {
        0.8
    } else {
        1.0
    };
    let mut out = SOILING_CENTRAL;
    let mut i = 0;
    while i < 12 {
        out[i] = (out[i] * mult).min(0.05);
        i += 1;
    }
    out
}

/// Fixed (month-independent) system loss stack product, itemized in the
/// module docs: 0.98 mismatch x 0.98 DC wiring x 0.995 connections x 0.99
/// nameplate x 0.98 light-induced degradation x 0.99 availability.
const ETA_FIXED: f64 = 0.98 * 0.98 * 0.995 * 0.99 * 0.98 * 0.99;

/// PVWatts temperature coefficient of DC power (B.7.2 mono-Si default).
const GAMMA_PDC_PER_C: f64 = -0.0035;
/// Cell-to-ambient rise at 1 kW/m^2 POA (B.7.2 `T_cell = T_amb +
/// (G_poa/1000) * 30`).
const NOCT_DELTA_C: f64 = 30.0;
/// AR(1) flicker correlation time (B.7.5: 30 s).
const FLICKER_TAU_S: f64 = 30.0;
/// Servo normalization gain lower bound (see [`PvArray::dc_power_w`]).
const SERVO_MIN: f64 = 0.5;
/// Servo normalization gain upper bound (see [`PvArray::dc_power_w`]).
const SERVO_MAX: f64 = 2.0;
/// Cloud multiplier lower clamp (B.7.5: `m(t) in [0.2, 1.05]`).
const M_MIN: f64 = 0.2;
/// Cloud multiplier upper clamp (B.7.5).
const M_MAX: f64 = 1.05;

/// Per-home PV array with cloud-noise state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PvArray {
    config: PvConfig,
    master_seed: u64,
    home_entity: u64,
    /// Azimuth per sub-array after the one-time +/-20 deg PvPhase jitter
    /// (B.7.3 fleet realism), normalized to [0, 360).
    azimuth_deg: Vec<f64>,
    /// Monthly soiling loss fractions resolved at init from the site
    /// zone proxy.
    soiling: [f64; 12],
    /// Current Markov sky state (B.7.5).
    sky: SkyState,
    /// AR(1) flicker value (fraction of clear-sky irradiance).
    flicker: f64,
    /// Local clock hour (`local_s / 3600`) the accumulators belong to;
    /// `u64::MAX` = not yet opened.
    hour_id: u64,
    /// Smooth POA-weighted basis integrated so far this hour (see
    /// [`PvArray::dc_power_w`] for the exact basis).
    hour_smooth_j: f64,
    /// Cloud-overlaid basis integrated so far this hour.
    hour_noisy_j: f64,
    /// Full-hour smooth-energy target A(T), trapezoid-integrated at hour
    /// open (the feed is deterministic, so the future of the hour is
    /// exactly known and the servo stays causal).
    hour_target_j: f64,
}

impl PvArray {
    /// Construct from static config, applying one-time seeded per-home
    /// draws from the `PvPhase` stream (+/-20 deg azimuth jitter per
    /// sub-array, B.7.3).
    #[must_use]
    pub fn new(config: &PvConfig, master_seed: u64, home_entity: u64) -> Self {
        let mut phase = rng::substream(master_seed, home_entity, RngPurpose::PvPhase, 0);
        let azimuth_deg = config
            .sub_arrays
            .iter()
            .map(|s| (s.azimuth_deg + phase.gen_range(-20.0..20.0)).rem_euclid(360.0))
            .collect();
        Self {
            config: config.clone(),
            master_seed,
            home_entity,
            azimuth_deg,
            soiling: soiling_table(config.latitude_deg, config.longitude_deg),
            sky: SkyState::Clear,
            flicker: 0.0,
            hour_id: u64::MAX,
            hour_smooth_j: 0.0,
            hour_noisy_j: 0.0,
            hour_target_j: 0.0,
        }
    }

    /// PV DC power at the array terminals (W; >= 0) for one tick (stage 2
    /// of B.1.5). AC conversion and clipping are downstream (module docs).
    ///
    /// # Cloud-overlay energy neutrality (B.7.5)
    ///
    /// The servo basis is the array's **POA-weighted pre-derate power**
    /// `S(t) = sum_i kw_dc_i * G_poa_i(t)`; the cloud multiplier `m`
    /// scales DNI/DHI before transposition, so by linearity the noisy
    /// basis is exactly `m(t) * S(t)`. At the first tick of each local
    /// clock hour the smooth full-hour energy `A(T)` is trapezoid-
    /// integrated from the deterministic feed (13 nodes at 5-min spacing —
    /// exact for a smooth feed, and causal since the feed is a pure
    /// function of time). Per tick, with `B(t)` the noisy energy so far
    /// and `S_int(t)` the smooth energy so far:
    ///
    /// ```text
    /// n(t) = clamp( (A(T) - B(t)) / (A(T) - S_int(t)), 0.5, 2.0 )
    /// m(t) = clamp( n(t) * (mu_state + flicker_ar1(t)), 0.2, 1.05 )
    /// ```
    ///
    /// Early in the hour `n ~= 1`; any deficit/surplus the flicker
    /// accumulates is spread over the hour's remaining energy, steering
    /// `B(T) -> A(T)`. Ticks whose smooth basis is below 15 W/m^2
    /// equivalent (night/dawn) skip the overlay (`m = 1`) and the
    /// accumulators, where division noise would otherwise dominate. The
    /// scheme matches every clock hour's energy to the smooth feed to a
    /// small fraction of a percent (test
    /// `cloud_overlay_hourly_energy_neutrality`), comfortably inside the
    /// spec's +/-2 % settlement bound, and is causal, deterministic, and
    /// allocation-free per tick.
    #[must_use]
    pub fn dc_power_w(&mut self, unix_time_s: u64, tick: u64, dt_s: u32, t_amb_c: f64) -> f64 {
        let position =
            solar_position(unix_time_s, self.config.latitude_deg, self.config.longitude_deg);
        let smooth = clear_sky(&position);
        let dt = f64::from(dt_s.max(1));
        let civil = civil_local(unix_time_s);
        let hour_id = civil.sec_of_day / 3600 + 24 * civil.day_number;

        // POA-weighted pre-derate basis S(t) (kW x W/m^2).
        let smooth_basis = self.poa_basis_w(&smooth, &position);
        let kw_total: f64 = self.config.sub_arrays.iter().map(|s| s.kw_dc).sum();
        let threshold = 15.0 * kw_total.max(0.1);

        let mult = if !self.config.cloud_noise || smooth_basis < threshold {
            1.0
        } else {
            // Open a new hour: reset accumulators and integrate the exact
            // smooth-hour target over the hour's REMAINDER (the hour may
            // open mid-hour at scenario start or at dawn, when the basis
            // first crosses the threshold).
            if hour_id != self.hour_id {
                self.hour_id = hour_id;
                self.hour_smooth_j = 0.0;
                self.hour_noisy_j = 0.0;
                let hour_end_unix = (hour_id + 1) * 3600 + CST_OFFSET_S;
                self.hour_target_j = self.basis_integral_j(unix_time_s, hour_end_unix);
            }
            let mut stream =
                rng::substream(self.master_seed, self.home_entity, RngPurpose::PvCloud, tick);
            let u_exit: f64 = stream.gen();
            let u_target: f64 = stream.gen();
            let bm1: f64 = stream.gen();
            let bm2: f64 = stream.gen();
            self.advance_sky(u_exit, u_target, normal_from_uniforms(bm1, bm2), dt, civil.month);
            // Causal servo toward the exact hour target.
            let remaining = (self.hour_target_j - self.hour_smooth_j).max(1.0);
            let needed = (self.hour_target_j - self.hour_noisy_j).max(0.0);
            let servo = (needed / remaining).clamp(SERVO_MIN, SERVO_MAX);
            let m = (servo * (self.sky.mean() + self.flicker)).clamp(M_MIN, M_MAX);
            self.hour_smooth_j += smooth_basis * dt;
            self.hour_noisy_j += m * smooth_basis * dt;
            m
        };

        let effective = Irradiance {
            ghi_w_m2: smooth.ghi_w_m2 * mult,
            dni_w_m2: smooth.dni_w_m2 * mult,
            dhi_w_m2: smooth.dhi_w_m2 * mult,
        };
        let soiling = 1.0 - self.soiling[(civil.month - 1) as usize];
        let shading = 1.0 - self.config.shading_factor.clamp(0.0, 0.3);
        let mut p_dc_w = 0.0;
        for (i, sub) in self.config.sub_arrays.iter().enumerate() {
            let poa = poa_irradiance(&effective, &position, sub.tilt_deg, self.azimuth_deg[i]);
            if poa <= 0.0 {
                continue;
            }
            let t_cell = t_amb_c + poa / 1000.0 * NOCT_DELTA_C;
            let temp_factor = 1.0 + GAMMA_PDC_PER_C * (t_cell - 25.0);
            p_dc_w += sub.kw_dc * poa * temp_factor * ETA_FIXED * soiling * shading;
        }
        p_dc_w.max(0.0)
    }

    /// POA-weighted pre-derate basis `sum_i kw_dc_i[kW] * G_poa_i[W/m^2]`
    /// (used only as the servo energy basis).
    fn poa_basis_w(&self, irr: &Irradiance, position: &SolarPosition) -> f64 {
        self.config
            .sub_arrays
            .iter()
            .zip(&self.azimuth_deg)
            .map(|(sub, &az)| sub.kw_dc * poa_irradiance(irr, position, sub.tilt_deg, az))
            .sum()
    }

    /// Trapezoid integral of the smooth basis over `[from_unix, to_unix]`
    /// (nodes at 5-min spacing plus the exact endpoints).
    fn basis_integral_j(&self, from_unix: u64, to_unix: u64) -> f64 {
        const STEP_S: u64 = 300;
        let eval = |t: u64| {
            let pos = solar_position(t, self.config.latitude_deg, self.config.longitude_deg);
            let irr = clear_sky(&pos);
            self.poa_basis_w(&irr, &pos)
        };
        let mut acc = 0.0;
        let mut t_prev = from_unix;
        let mut v_prev = eval(from_unix);
        loop {
            let t = (t_prev + STEP_S).min(to_unix);
            if t <= t_prev {
                break;
            }
            let v = eval(t);
            acc += 0.5 * (v_prev + v) * (t - t_prev) as f64;
            if t >= to_unix {
                break;
            }
            t_prev = t;
            v_prev = v;
        }
        acc
    }

    /// Advance the Markov sky state and the within-state AR(1) flicker by
    /// one tick using the three per-tick substream draws.
    fn advance_sky(&mut self, u_exit: f64, u_target: f64, eps: f64, dt_s: f64, month: u64) {
        let season = Season::of_month(month);
        // Markov transition: per-tick exit probability dt/dwell; the
        // target draw picks the next state (fitted-order split, B.7.5).
        if u_exit < dt_s / self.sky.dwell_s(season) {
            self.sky = match self.sky {
                SkyState::Clear => {
                    if u_target < 0.85 {
                        SkyState::Partly
                    } else {
                        SkyState::Broken
                    }
                }
                SkyState::Partly => {
                    if u_target < 0.45 {
                        SkyState::Clear
                    } else {
                        SkyState::Broken
                    }
                }
                SkyState::Broken => {
                    if u_target < 0.70 {
                        SkyState::Partly
                    } else {
                        SkyState::Clear
                    }
                }
            };
        }
        // Within-state AR(1) flicker, sigma as fraction of clear sky.
        let phi = (-dt_s / FLICKER_TAU_S).exp();
        let sigma = self.sky.sigma();
        self.flicker = phi * self.flicker + sigma * (1.0 - phi * phi).sqrt() * eps;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Austin, TX site used across PV tests.
    const AUSTIN_LAT: f64 = 30.267;
    /// Austin longitude.
    const AUSTIN_LON: f64 = -97.743;
    /// 2026-03-20T00:00:00Z (equinox day; equinox moment ~14:46 UTC).
    const EQUINOX: u64 = 1_773_964_800;
    /// 2026-06-21T00:00:00Z (solstice).
    const SOLSTICE: u64 = 1_782_000_000;
    /// 2026-07-01T00:00:00Z.
    const JUL1: u64 = 1_782_864_000;

    fn austin_config(cloud_noise: bool) -> PvConfig {
        PvConfig {
            sub_arrays: vec![SubArray {
                kw_dc: 8.0,
                tilt_deg: AUSTIN_LAT,
                azimuth_deg: 180.0,
            }],
            latitude_deg: AUSTIN_LAT,
            longitude_deg: AUSTIN_LON,
            shading_factor: 0.0,
            cloud_noise,
        }
    }

    #[test]
    fn civil_time_roundtrip_spot_checks() {
        // 2026-07-06 is a Monday; 06:00 UTC == 00:00 local (UTC-6).
        let c = civil_local(1_783_296_000 + 6 * 3600);
        assert_eq!((c.month, c.hour, c.day_of_week), (7, 0, 1));
        // 2026-01-01T06:00Z -> local Jan 1 00:00, day-of-year 1.
        let jan1 = civil_local(1_767_225_600 + 6 * 3600);
        assert_eq!(jan1.day_of_year, 1);
        // 2026-03-01T06:00Z (non-leap year): local Mar 1, doy 60.
        let mar1 = civil_local(1_772_323_200 + 6 * 3600);
        assert_eq!((mar1.month, mar1.day_of_year), (3, 60));
        // Leap year: 2024-03-01 is doy 61.
        let leap = civil_local(1_709_251_200 + 6 * 3600);
        assert_eq!((leap.month, leap.day_of_year), (3, 61));
    }

    #[test]
    fn solar_position_equinox_noon_elevation() {
        // Scan the equinox day; at true solar noon elevation must equal
        // 90 - |lat - decl| with decl ~ 0 -> 90 - lat (+/-0.5 deg).
        let mut max_el = -90.0;
        let mut max_az = 0.0;
        for m in 0..1440u64 {
            let pos = solar_position(EQUINOX + m * 60, AUSTIN_LAT, AUSTIN_LON);
            if pos.elevation_deg > max_el {
                max_el = pos.elevation_deg;
                max_az = pos.azimuth_deg;
            }
        }
        let expected = 90.0 - AUSTIN_LAT;
        assert!(
            (max_el - expected).abs() <= 0.5,
            "equinox noon elevation {max_el} vs expected {expected}"
        );
        // Culmination is essentially due south (equinox decl ~ +0.1 deg).
        assert!((max_az - 180.0).abs() < 1.5, "noon azimuth {max_az}");
    }

    #[test]
    fn solar_position_night_and_azimuth_quadrants() {
        // Austin local midnight (06:00 UTC) -> sun well below horizon.
        let night = solar_position(SOLSTICE + 6 * 3600, AUSTIN_LAT, AUSTIN_LON);
        assert!(night.elevation_deg < -5.0);
        // Summer morning sun in the eastern half, evening in the western.
        let morning = solar_position(SOLSTICE + 13 * 3600, AUSTIN_LAT, AUSTIN_LON); // 07:00 local
        assert!(morning.elevation_deg > 0.0);
        assert!(morning.azimuth_deg > 45.0 && morning.azimuth_deg < 135.0);
        let evening = solar_position(SOLSTICE + 23 * 3600, AUSTIN_LAT, AUSTIN_LON); // 17:00 local
        assert!(evening.elevation_deg > 0.0);
        assert!(evening.azimuth_deg > 225.0 && evening.azimuth_deg < 315.0);
        // Extraterrestrial irradiance within the physical 1320-1412 band.
        assert!(night.extra_terrestrial_w_m2 > 1320.0 && night.extra_terrestrial_w_m2 < 1412.0);
    }

    #[test]
    fn clear_sky_plausible_at_summer_noon() {
        // Austin summer solar noon: GHI ~ 900-1100 W/m^2, DNI >> DHI.
        let mut best = Irradiance::default();
        let mut best_ghi = 0.0;
        for m in 0..1440u64 {
            let pos = solar_position(SOLSTICE + m * 60, AUSTIN_LAT, AUSTIN_LON);
            let irr = clear_sky(&pos);
            if irr.ghi_w_m2 > best_ghi {
                best_ghi = irr.ghi_w_m2;
                best = irr;
            }
        }
        assert!(best_ghi > 900.0 && best_ghi < 1100.0, "GHI {best_ghi}");
        assert!(best.dni_w_m2 > 700.0 && best.dni_w_m2 < 1000.0);
        assert!(best.dhi_w_m2 > 40.0 && best.dhi_w_m2 < 200.0);
    }

    #[test]
    fn dc_power_zero_at_night_and_peak_band() {
        let mut pv = PvArray::new(&austin_config(false), 42, 0x1000);
        let mut peak = 0.0f64;
        let mut night_max = 0.0f64;
        for tick in 0..1440u64 {
            let t = SOLSTICE + tick * 60;
            let p = pv.dc_power_w(t, tick, 60, 30.0);
            assert!(p >= 0.0);
            let local_hour = (t + 18 * 3600) / 3600 % 24; // UTC-6 frame
            if (7..=19).contains(&local_hour) {
                peak = peak.max(p);
            } else {
                night_max = night_max.max(p);
            }
        }
        assert_eq!(night_max, 0.0, "night power {night_max}");
        assert!(
            (5500.0..=7000.0).contains(&peak),
            "8 kW array summer-noon DC peak {peak} W outside 5.5-7 kW band"
        );
    }

    #[test]
    fn subarray_orientation_west_peaks_later() {
        let mk = |az: f64| {
            PvArray::new(
                &PvConfig {
                    sub_arrays: vec![SubArray {
                        kw_dc: 5.0,
                        tilt_deg: 25.0,
                        azimuth_deg: az,
                    }],
                    latitude_deg: AUSTIN_LAT,
                    longitude_deg: AUSTIN_LON,
                    shading_factor: 0.0,
                    cloud_noise: false,
                },
                7,
                0x2000,
            )
        };
        let mut south = mk(180.0);
        let mut west = mk(270.0);
        let mut south_peak_hour = 0u64;
        let mut west_peak_hour = 0u64;
        let mut south_peak = 0.0f64;
        let mut west_peak = 0.0f64;
        for tick in 0..1440u64 {
            let t = JUL1 + tick * 60;
            let local_hour = (t + 18 * 3600) / 3600 % 24;
            let ps = south.dc_power_w(t, tick, 60, 32.0);
            let pw = west.dc_power_w(t, tick, 60, 32.0);
            if ps > south_peak {
                south_peak = ps;
                south_peak_hour = local_hour;
            }
            if pw > west_peak {
                west_peak = pw;
                west_peak_hour = local_hour;
            }
        }
        assert!(
            west_peak_hour >= south_peak_hour + 2,
            "west peaks at {west_peak_hour}, south at {south_peak_hour}"
        );
    }

    #[test]
    fn determinism_same_seed_different_entity() {
        let cfg = austin_config(true);
        let mut a1 = PvArray::new(&cfg, 99, 0x3000);
        let mut a2 = PvArray::new(&cfg, 99, 0x3000);
        let mut b = PvArray::new(&cfg, 99, 0x3001);
        let mut identical = true;
        let mut differs = false;
        for tick in 0..(7 * 1440) {
            let t = JUL1 + tick * 60;
            let p1 = a1.dc_power_w(t, tick, 60, 31.0);
            let p2 = a2.dc_power_w(t, tick, 60, 31.0);
            let pb = b.dc_power_w(t, tick, 60, 31.0);
            if p1.to_bits() != p2.to_bits() {
                identical = false;
            }
            if p1.to_bits() != pb.to_bits() {
                differs = true;
            }
        }
        assert!(identical, "same seed+entity must be bit-identical");
        assert!(differs, "different entity must diverge (jitter/cloud)");
    }

    #[test]
    fn cloud_overlay_hourly_energy_neutrality() {
        // 30-day run at dt = 60 s; every clock hour with meaningful smooth
        // energy must land within +/-2 % of the smooth feed (B.7.5).
        let mut smooth_pv = PvArray::new(&austin_config(false), 5, 0x4000);
        let mut noisy_pv = PvArray::new(&austin_config(true), 5, 0x4000);
        let mut hour_smooth = 0.0f64;
        let mut hour_noisy = 0.0f64;
        let mut cur_hour = u64::MAX;
        let mut worst = 0.0f64;
        let mut hours_checked = 0u32;
        for tick in 0..(30 * 1440u64) {
            let t = JUL1 + tick * 60;
            let hour = t / 3600;
            if hour != cur_hour {
                if cur_hour != u64::MAX && hour_smooth > 300.0 {
                    let ratio = hour_noisy / hour_smooth;
                    assert!(
                        (0.98..=1.02).contains(&ratio),
                        "hour {cur_hour}: noisy/smooth = {ratio}"
                    );
                    worst = worst.max((ratio - 1.0).abs());
                    hours_checked += 1;
                }
                cur_hour = hour;
                hour_smooth = 0.0;
                hour_noisy = 0.0;
            }
            hour_smooth += smooth_pv.dc_power_w(t, tick, 60, 33.0);
            hour_noisy += noisy_pv.dc_power_w(t, tick, 60, 33.0);
        }
        assert!(hours_checked > 300, "only {hours_checked} hours checked");
        assert!(worst <= 0.02, "worst hourly deviation {worst}");
        // The overlay must actually wiggle: tick-level paths differ.
        let mut s2 = PvArray::new(&austin_config(false), 5, 0x4000);
        let mut n2 = PvArray::new(&austin_config(true), 5, 0x4000);
        let any_diff = (0..1440u64).any(|tick| {
            let t = JUL1 + (400 + tick) * 60;
            s2.dc_power_w(t, tick, 60, 33.0).to_bits()
                != n2.dc_power_w(t, tick, 60, 33.0).to_bits()
        });
        assert!(any_diff, "cloud overlay had no tick-level effect");
    }

    #[test]
    fn shading_derate_exact() {
        // 30 % shading is a pure multiplier on top of an otherwise
        // identical config -> ratio exactly 0.70.
        let mut unshaded = PvArray::new(&austin_config(false), 3, 0x5000);
        let mut cfg = austin_config(false);
        cfg.shading_factor = 0.3;
        let mut shaded = PvArray::new(&cfg, 3, 0x5000);
        let noon = SOLSTICE + 18 * 3600 + 30 * 60;
        let p0 = unshaded.dc_power_w(noon, 1110, 60, 30.0);
        let p1 = shaded.dc_power_w(noon, 1110, 60, 30.0);
        let ratio = p1 / p0;
        assert!((ratio - 0.70).abs() < 1e-9, "shading ratio {ratio}");
    }
}
