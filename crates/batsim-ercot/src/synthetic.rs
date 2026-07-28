//! Seeded regime-switching synthetic price generator (spec D.4).
//!
//! [`SyntheticPriceGenerator`] composes, per interval, the spec D.4.1
//! formula `base_dow_shape(season, hour) + renewable_dip +
//! regime_component + noise`, floored at `floor_usd_per_mwh` and capped by
//! the effective cap (HCAP, dropping to LCAP once
//! `emergency_hours_at_hcap` cumulative hours at HCAP accumulate inside
//! the rolling window — latched for the rest of the run, a documented
//! simplification of the PUCT trigger language).
//!
//! Regimes (`Normal`, `SolarNegative`, `ScarcityOrdc`, `WinterStorm`) switch
//! on a 4-state Markov chain. The default transition matrix derives from
//! `solar_penetration` (more solar -> more midday-negative episodes) and
//! `reserve_margin` (lower -> more scarcity episodes), with off-diagonal
//! rates scaled by `interval_secs / 900` so episode FREQUENCY is
//! cadence-independent; `WinterStorm` is entered only in `winter`. A
//! user-supplied `regime_matrix` ([from][to], order
//! `[Normal, SolarNegative, ScarcityOrdc, WinterStorm]`) is used verbatim
//! as per-interval probabilities.
//!
//! Determinism contract: one `ChaCha8Rng` seeded by `params.seed`; a fixed
//! draw order per interval (Markov transition, price noise (2 uniforms via
//! Box-Muller), reserves, scarcity spike, load noise (2 uniforms)), then
//! one uniform per (hour, product) for AS prices in `(ts, product)` order.
//! DAM SPPs are pure hourly averages of the RT series (no draws). All
//! transcendentals route through `libm` (the `batsim-core` math wrappers
//! are crate-private), so identical `seed` + params produce bit-identical
//! series on every platform.

use std::collections::{BTreeMap, VecDeque};

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, Weekday};

use crate::cpt;
use crate::error::{ErcotError, Result};
use crate::replay::SourceIndex;
use crate::rules::ErcotRules;
use crate::source::PriceSource;
use crate::types::{
    AsPrice, AsProduct, FuelMix, LoadZone, Location, PriceSample, Provenance, SystemSignal,
    TimeRange,
};

/// `libm`-backed transcendentals for bit-exact cross-platform output
/// (mirrors the crate-private `batsim_core::math` wrappers).
mod math {
    /// Cosine.
    pub(crate) fn cos(x: f64) -> f64 {
        libm::cos(x)
    }
    /// Natural exponential.
    pub(crate) fn exp(x: f64) -> f64 {
        libm::exp(x)
    }
    /// Natural logarithm.
    pub(crate) fn ln(x: f64) -> f64 {
        libm::log(x)
    }
}

/// RFC 3339 serde for [`OffsetDateTime`]: scenario documents carry
/// ISO/RFC strings (spec D.4.2), while the workspace's `time` serde setup
/// defaults to the compact array form.
mod rfc3339_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    pub(super) fn serialize<S: Serializer>(ts: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&ts.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OffsetDateTime, D::Error> {
        let raw = String::deserialize(d)?;
        OffsetDateTime::parse(&raw, &Rfc3339).map_err(serde::de::Error::custom)
    }
}

/// 2*pi (full turn), for Box-Muller and diurnal curves.
const TAU: f64 = std::f64::consts::TAU;

/// Scenario season: drives base price/load shapes (spec D.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    /// Winter: morning/evening load peaks; winter storms possible.
    Winter,
    /// Summer: strong afternoon peak (4CP season).
    Summer,
    /// Shoulder (spring/fall): mild shape, low prices.
    Shoulder,
}

/// Scripted event kinds (spec D.4.2 `event_overlay`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Uri-2021-style extended winter emergency: forces the `WinterStorm`
    /// regime and replaces the scenario cap for the event window (use
    /// 9000 for era-correct Uri replays).
    WinterStorm,
}

/// A scripted event window (spec D.4.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOverlay {
    /// Event kind.
    pub kind: EventKind,
    /// Window start (inclusive; RFC 3339 in scenario documents, compared
    /// by instant).
    #[serde(with = "rfc3339_serde")]
    pub start: OffsetDateTime,
    /// Window length, hours.
    pub duration_h: f64,
    /// System-wide cap in force during the window, $/MWh (replaces HCAP;
    /// emergency-trigger accounting pauses while an overlay is active).
    #[serde(rename = "cap")]
    pub cap_usd_per_mwh: f64,
}

/// Scenario override for the rules' offer caps (spec D.4.2 `caps`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapsOverride {
    /// High system-wide offer cap, $/MWh.
    pub hcap_usd_per_mwh: f64,
    /// Low system-wide offer cap after the emergency trigger, $/MWh.
    pub lcap_usd_per_mwh: f64,
    /// Cumulative hours at HCAP (in the rolling window) that trip the drop.
    pub emergency_hours_at_hcap: f64,
    /// Rolling window for the trigger, hours.
    pub emergency_rolling_window_hours: f64,
}

/// Scenario override for the simplified ORDC (spec D.4.2 `ordc`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdcOverride {
    /// Reserves below this engage the adder, MW.
    pub threshold_mw: f64,
    /// Reserves at/below this drive the adder to ~VOLL, MW.
    pub floor_mw: f64,
    /// Value of lost load, $/MWh (adder asymptote).
    pub voll_usd_per_mwh: f64,
}

const fn default_reserve_margin() -> f64 {
    0.10
}

const fn default_interval_secs() -> u32 {
    900
}

const fn default_floor() -> f64 {
    -20.0
}

/// Scenario-document parameters for [`SyntheticPriceGenerator`]
/// (spec D.4.2). `seed` is REQUIRED in scenario documents: same seed +
/// params => bit-identical series.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticParams {
    /// Master seed (REQUIRED).
    pub seed: u64,
    /// Scenario season (drives base shapes; `winter` enables storm entry).
    pub season: Season,
    /// Fraction of midday energy from solar [0, 1]; drives the depth and
    /// frequency of solar-driven negative prices.
    #[serde(default)]
    pub solar_penetration: f64,
    /// Lower => more `ScarcityOrdc` transitions (spec D.4.2).
    #[serde(default = "default_reserve_margin")]
    pub reserve_margin: f64,
    /// Optional Markov transition override ([from][to], row order
    /// `[Normal, SolarNegative, ScarcityOrdc, WinterStorm]`, per-interval
    /// probabilities, rows must sum to 1).
    #[serde(default)]
    pub regime_matrix: Option<[[f64; 4]; 4]>,
    /// Offer-cap override; defaults from [`ErcotRules::offer_caps`].
    #[serde(default)]
    pub caps: Option<CapsOverride>,
    /// ORDC override; defaults from [`ErcotRules::ordc`].
    #[serde(default)]
    pub ordc: Option<OrdcOverride>,
    /// Scripted events (e.g. Uri-class winter storm windows).
    #[serde(default)]
    pub event_overlay: Vec<EventOverlay>,
    /// Assumed competing storage, GW; AS prices discount by
    /// `1 / (1 + 0.5 * as_saturation_gw)`.
    #[serde(default)]
    pub as_saturation_gw: f64,
    /// Settlement cadence, seconds; must be one of the rules'
    /// `settlement.allowed_interval_secs` (300 or 900 in v2025).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u32,
    /// Price floor, $/MWh (rules-free stress parameter, [-50, -20]).
    #[serde(default = "default_floor")]
    pub floor_usd_per_mwh: f64,
    /// Settlement location stamped on every generated price row.
    pub location: Location,
}

impl Default for SyntheticParams {
    /// Programmatic defaults mirroring the serde field defaults (seed 0,
    /// shoulder season, LZ_HOUSTON). Scenarios SHOULD set `seed` and
    /// `season` explicitly.
    fn default() -> Self {
        Self {
            seed: 0,
            season: Season::Shoulder,
            solar_penetration: 0.0,
            reserve_margin: default_reserve_margin(),
            regime_matrix: None,
            caps: None,
            ordc: None,
            event_overlay: Vec::new(),
            as_saturation_gw: 0.0,
            interval_secs: default_interval_secs(),
            floor_usd_per_mwh: default_floor(),
            location: Location::LoadZone(LoadZone::Houston),
        }
    }
}

/// Market regime; index order matches `regime_matrix` rows/columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Regime {
    /// Standard conditions.
    Normal,
    /// High-solar midday: negative prices, steep evening ramp.
    SolarNegative,
    /// Tight reserves: ORDC adder active, spiky prices.
    ScarcityOrdc,
    /// Uri-class extended emergency: sustained at/near cap.
    WinterStorm,
}

impl Regime {
    /// Index in the transition matrix.
    const fn index(self) -> usize {
        match self {
            Self::Normal => 0,
            Self::SolarNegative => 1,
            Self::ScarcityOrdc => 2,
            Self::WinterStorm => 3,
        }
    }
}

/// All regimes in matrix index order.
const REGIMES: [Regime; 4] = [
    Regime::Normal,
    Regime::SolarNegative,
    Regime::ScarcityOrdc,
    Regime::WinterStorm,
];

/// Caps/ORDC/matrix resolved from params + rules at construction.
#[derive(Debug, Clone, Copy)]
struct Resolved {
    /// Effective HCAP, $/MWh.
    hcap: f64,
    /// Effective LCAP (post-trigger), $/MWh.
    lcap: f64,
    /// Trigger hours at HCAP.
    emergency_hours: f64,
    /// Rolling trigger window, seconds.
    emergency_window_secs: i64,
    /// ORDC reserves threshold, MW.
    ordc_threshold_mw: f64,
    /// ORDC reserves floor, MW.
    ordc_floor_mw: f64,
    /// Value of lost load, $/MWh.
    voll: f64,
    /// Effective transition matrix ([from][to], rows sum to 1).
    matrix: [[f64; 4]; 4],
}

/// Emergency-pricing trigger state (simplified PUCT rule: latches LCAP for
/// the remainder of the run once tripped — spec D.4.2 keeps the exact
/// trigger language parameterized).
#[derive(Debug, Default)]
struct CapTracker {
    /// (unix ts, hours) of recent at-HCAP intervals inside the window.
    history: VecDeque<(i64, f64)>,
    /// Sum of hours in `history`.
    at_cap_hours: f64,
    /// Whether the trigger has tripped.
    latched: bool,
}

impl CapTracker {
    /// Effective cap for this interval, $/MWh.
    fn effective_cap(&self, overlay: Option<&EventOverlay>, resolved: &Resolved) -> f64 {
        if let Some(overlay) = overlay {
            overlay.cap_usd_per_mwh
        } else if self.latched {
            resolved.lcap
        } else {
            resolved.hcap
        }
    }

    /// Record one interval priced against HCAP and update the latch. An
    /// interval counts when the UNCAPPED price would have reached HCAP.
    fn record(
        &mut self,
        ts_unix: i64,
        hours: f64,
        would_exceed_hcap: bool,
        overlay_active: bool,
        resolved: &Resolved,
    ) {
        if overlay_active || self.latched || !would_exceed_hcap {
            return;
        }
        self.history.push_back((ts_unix, hours));
        self.at_cap_hours += hours;
        let window_start = ts_unix - resolved.emergency_window_secs;
        while let Some((old_ts, old_hours)) = self.history.front().copied() {
            if old_ts >= window_start {
                break;
            }
            self.history.pop_front();
            self.at_cap_hours -= old_hours;
        }
        if self.at_cap_hours >= resolved.emergency_hours {
            self.latched = true;
        }
    }
}

/// Circular hour distance on a 24 h clock.
fn hour_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).abs();
    d.min(24.0 - d)
}

/// Unit Gaussian bump centered at `center` (circular) with width `width`.
fn gauss(hour: f64, center: f64, width: f64) -> f64 {
    let x = hour_dist(hour, center) / width;
    math::exp(-x * x)
}

/// Seasonal base-shape constants for prices and load.
#[derive(Debug, Clone, Copy)]
struct SeasonShape {
    /// Base price level, $/MWh.
    price_base: f64,
    /// Morning price bump amplitude, $/MWh.
    price_morning: f64,
    /// Evening price bump amplitude, $/MWh.
    price_evening: f64,
    /// Overnight price dip amplitude, $/MWh.
    price_night: f64,
    /// Evening bump center hour (CPT).
    evening_center: f64,
    /// Base system load, MW.
    load_base_mw: f64,
    /// Load bump amplitude, MW.
    load_amp_mw: f64,
    /// Load bump center hour (CPT; afternoon peak).
    load_center: f64,
}

/// Per-season shape table (Normal-regime prices land in the spec's
/// $15-60/MWh band modulo noise).
fn season_shape(season: Season) -> SeasonShape {
    match season {
        Season::Winter => SeasonShape {
            price_base: 30.0,
            price_morning: 10.0,
            price_evening: 12.0,
            price_night: 6.0,
            evening_center: 19.0,
            load_base_mw: 42_000.0,
            load_amp_mw: 7_000.0,
            load_center: 19.0,
        },
        Season::Summer => SeasonShape {
            price_base: 34.0,
            price_morning: 6.0,
            price_evening: 22.0,
            price_night: 4.0,
            evening_center: 18.0,
            load_base_mw: 55_000.0,
            load_amp_mw: 18_000.0,
            load_center: 16.5,
        },
        Season::Shoulder => SeasonShape {
            price_base: 24.0,
            price_morning: 6.0,
            price_evening: 10.0,
            price_night: 6.0,
            evening_center: 19.0,
            load_base_mw: 38_000.0,
            load_amp_mw: 5_000.0,
            load_center: 19.0,
        },
    }
}

/// `base_dow_shape(season, hour)`: seasonal base + morning/evening bumps -
/// overnight dip, with a weekend derate (spec D.4.1).
fn base_dow_shape(shape: &SeasonShape, hour: f64, weekend: bool) -> f64 {
    let price = shape.price_base + shape.price_morning * gauss(hour, 8.0, 2.0)
        + shape.price_evening * gauss(hour, shape.evening_center, 2.5)
        - shape.price_night * gauss(hour, 4.0, 3.0);
    if weekend {
        price * 0.85
    } else {
        price
    }
}

/// Midday solar dip, $/MWh (always <= 0; depth scales with penetration).
fn renewable_dip(solar_penetration: f64, hour: f64) -> f64 {
    -45.0 * solar_penetration * gauss(hour, 13.0, 2.5)
}

/// Regime LMP component and noise sigma, $/MWh. `u_spike` in [0, 1).
fn regime_component(regime: Regime, hour: f64, u_spike: f64) -> (f64, f64) {
    match regime {
        Regime::Normal => (0.0, 5.0),
        Regime::SolarNegative => (
            18.0 * gauss(hour, 19.0, 1.6) - 55.0 * gauss(hour, 13.5, 2.2),
            6.0,
        ),
        Regime::ScarcityOrdc => (120.0 + 220.0 * u_spike, 40.0),
        Regime::WinterStorm => (250.0 + 150.0 * u_spike, 25.0),
    }
}

/// Reserves draw by regime, MW. `u` in [0, 1). Scarcity draws land in
/// `[floor, threshold)`, storm draws below the floor (adder ~ VOLL).
fn reserves_draw(regime: Regime, u: f64, threshold_mw: f64, floor_mw: f64) -> f64 {
    match regime {
        Regime::Normal | Regime::SolarNegative => threshold_mw + 500.0 + 4_000.0 * u,
        Regime::ScarcityOrdc => floor_mw + 0.95 * (threshold_mw - floor_mw) * u,
        Regime::WinterStorm => floor_mw * (0.2 + 0.6 * u),
    }
}

/// Simplified ORDC adder (rules' `ordc`): 0 at the threshold, ~VOLL at/below
/// the floor, linear between — monotone increasing in scarcity (spec D.4).
fn ordc_adder(reserves_mw: f64, resolved: &Resolved) -> f64 {
    let frac = (resolved.ordc_threshold_mw - reserves_mw)
        / (resolved.ordc_threshold_mw - resolved.ordc_floor_mw);
    resolved.voll * frac.clamp(0.0, 1.0)
}

/// Box-Muller standard normal from two uniforms (deterministic under a
/// seeded stream; `sqrt` is correctly rounded, `ln`/`cos` via `libm`).
fn standard_normal(rng: &mut ChaCha8Rng) -> f64 {
    let u1 = 1.0 - rng.gen::<f64>(); // (0, 1]: ln(0) unreachable
    let u2 = rng.gen::<f64>();
    (-2.0 * math::ln(u1)).sqrt() * math::cos(TAU * u2)
}

/// Markov transition: sample the next regime from row `from` with `u` in
/// [0, 1). Rows sum to 1 (validated); float dust falls through to the last.
fn markov_next(from: Regime, u: f64, matrix: &[[f64; 4]; 4]) -> Regime {
    let mut acc = 0.0;
    for (idx, p) in matrix[from.index()].iter().enumerate() {
        acc += p;
        if u < acc {
            return REGIMES[idx];
        }
    }
    Regime::WinterStorm
}

/// Default transition matrix from scenario drivers; off-diagonal rates
/// scaled by `cadence_scale` (`interval_secs / 900`) so daily episode
/// frequency is cadence-independent.
fn default_matrix(params: &SyntheticParams, cadence_scale: f64) -> [[f64; 4]; 4] {
    let winter = params.season == Season::Winter;
    let solar_entry = (0.02 * params.solar_penetration / 0.35).clamp(0.0, 0.10);
    let scarcity_entry = (0.004 * 0.10 / params.reserve_margin.max(0.02)).clamp(0.0, 0.05);
    let storm_entry = if winter { 0.0004 } else { 0.0 };
    let off_diagonal = [
        [0.0, solar_entry, scarcity_entry, storm_entry],
        [0.15, 0.0, 0.02, 0.0],
        [0.10, 0.0, 0.0, if winter { 0.005 } else { 0.0 }],
        [0.005, 0.0, 0.0, 0.0],
    ];
    let mut matrix = [[0.0; 4]; 4];
    for (from, row) in off_diagonal.iter().enumerate() {
        let mut off_sum = 0.0;
        for (to, p) in row.iter().enumerate() {
            matrix[from][to] = p * cadence_scale;
            off_sum += matrix[from][to];
        }
        matrix[from][from] = 1.0 - off_sum.min(0.999);
    }
    matrix
}

/// System load, MW: seasonal base + diurnal bump + regime uplift + noise.
fn system_load_mw(shape: &SeasonShape, regime: Regime, hour: f64, weekend: bool, z: f64) -> f64 {
    let mut load = shape.load_base_mw + shape.load_amp_mw * gauss(hour, shape.load_center, 3.0);
    if weekend {
        load *= 0.92;
    }
    if regime == Regime::WinterStorm {
        load *= 1.15;
    }
    (load + 800.0 * z).max(1_000.0)
}

/// Synthetic fuel mix (fractions normalized to sum to 1; keys match the
/// rules' `emissions.kg_co2_per_mwh` names).
fn fuel_mix(solar_penetration: f64, hour: f64) -> FuelMix {
    let solar = (1.5 * solar_penetration * gauss(hour, 13.0, 3.0)).clamp(0.0, 0.6);
    let wind = 0.15 + 0.15 * gauss(hour, 2.0, 4.0);
    let nuclear = 0.10;
    let coal = 0.12;
    let gas = (1.0 - solar - wind - nuclear - coal).max(0.05);
    let total = solar + wind + nuclear + coal + gas;
    let mut mix = FuelMix::new();
    mix.insert("solar".to_string(), solar / total);
    mix.insert("wind".to_string(), wind / total);
    mix.insert("nuclear".to_string(), nuclear / total);
    mix.insert("coal".to_string(), coal / total);
    mix.insert("natural_gas".to_string(), gas / total);
    mix
}

/// AS MCPC for one product/hour, correlated with regime and discounted by
/// assumed competing storage (`u` in [0, 1)).
fn as_mcpc(product: AsProduct, regime: Regime, u: f64, as_saturation_gw: f64) -> f64 {
    let base = match product {
        AsProduct::RegUp => 12.0,
        AsProduct::RegDown => 8.0,
        AsProduct::Rrs => 6.0,
        AsProduct::NonSpin => 4.0,
        AsProduct::Ecrs => 10.0,
    };
    let mult = match regime {
        Regime::Normal => 1.0,
        Regime::SolarNegative => match product {
            AsProduct::RegUp => 1.5,
            AsProduct::RegDown => 1.3,
            _ => 1.0,
        },
        Regime::ScarcityOrdc => match product {
            AsProduct::Ecrs => 8.0,
            AsProduct::Rrs => 6.0,
            AsProduct::NonSpin => 4.0,
            AsProduct::RegUp => 3.0,
            AsProduct::RegDown => 2.0,
        },
        Regime::WinterStorm => match product {
            AsProduct::Ecrs => 15.0,
            AsProduct::Rrs => 12.0,
            AsProduct::NonSpin => 8.0,
            AsProduct::RegUp => 5.0,
            AsProduct::RegDown => 3.0,
        },
    };
    let saturation = 1.0 / (1.0 + 0.5 * as_saturation_gw);
    base * mult * saturation * (0.8 + 0.4 * u)
}

/// Running DAM accumulator for one UTC hour (sums of the RT series).
#[derive(Debug, Default)]
struct DamAccum {
    /// Sum of RT LMPs in the hour.
    lmp_sum: f64,
    /// Sum of RT ORDC adders in the hour.
    ordc_sum: f64,
    /// RT intervals in the hour.
    count: u64,
}

/// Per-interval generation state: the seeded stream plus everything the
/// step function mutates.
#[derive(Debug)]
struct GenDriver<'a> {
    /// Scenario parameters.
    params: &'a SyntheticParams,
    /// Resolved caps/ORDC/matrix.
    resolved: &'a Resolved,
    /// Seasonal shape table.
    shape: SeasonShape,
    /// The single seeded stream.
    rng: ChaCha8Rng,
    /// Current Markov state (advances even under an overlay).
    state: Regime,
    /// Emergency-pricing trigger state.
    cap_tracker: CapTracker,
}

impl GenDriver<'_> {
    /// Generate one interval: advance the Markov chain, draw in the fixed
    /// documented order, price against the effective cap, and return the
    /// RT sample, the system signal, and the effective regime.
    fn step_interval(
        &mut self,
        ts: OffsetDateTime,
        ts_unix: i64,
        interval_hours: f64,
    ) -> (PriceSample, SystemSignal, Regime) {
        let civil = cpt::utc_to_cpt(ts);
        let hour = f64::from(civil.hour())
            + f64::from(civil.minute()) / 60.0
            + f64::from(civil.second()) / 3600.0;
        let weekend = matches!(civil.weekday(), Weekday::Saturday | Weekday::Sunday);
        let overlay = active_overlay(&self.params.event_overlay, ts);

        // Fixed draw order (determinism contract): markov, price noise
        // (2), reserves, spike, load noise (2).
        self.state = markov_next(self.state, self.rng.gen::<f64>(), &self.resolved.matrix);
        let regime = overlay.map_or(self.state, |_| Regime::WinterStorm);
        let z_price = standard_normal(&mut self.rng);
        let u_reserves = self.rng.gen::<f64>();
        let u_spike = self.rng.gen::<f64>();
        let z_load = standard_normal(&mut self.rng);

        let reserves_mw = reserves_draw(
            regime,
            u_reserves,
            self.resolved.ordc_threshold_mw,
            self.resolved.ordc_floor_mw,
        );
        let adder_pre = match regime {
            Regime::ScarcityOrdc | Regime::WinterStorm => ordc_adder(reserves_mw, self.resolved),
            Regime::Normal | Regime::SolarNegative => 0.0,
        };
        let cap_now = self.cap_tracker.effective_cap(overlay, self.resolved);
        let (regime_lmp, sigma) = regime_component(regime, hour, u_spike);
        let lmp_raw = base_dow_shape(&self.shape, hour, weekend)
            + renewable_dip(self.params.solar_penetration, hour)
            + regime_lmp
            + sigma * z_price;
        let lmp = lmp_raw.clamp(self.params.floor_usd_per_mwh, cap_now);
        let adder = adder_pre.clamp(0.0, cap_now - lmp);
        self.cap_tracker.record(
            ts_unix,
            interval_hours,
            lmp_raw + adder_pre >= self.resolved.hcap,
            overlay.is_some(),
            self.resolved,
        );
        let sample = PriceSample {
            ts,
            interval_secs: self.params.interval_secs,
            location: self.params.location.clone(),
            lmp_usd_per_mwh: lmp,
            ordc_adder_usd_per_mwh: adder,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::Synthetic,
        };
        let signal = SystemSignal {
            ts,
            system_load_mw: system_load_mw(&self.shape, regime, hour, weekend, z_load),
            reserves_mw: Some(reserves_mw),
            fuel_mix: Some(fuel_mix(self.params.solar_penetration, hour)),
        };
        (sample, signal, regime)
    }
}

/// Seeded regime-switching stress generator (spec D.4). Fully deterministic
/// given `params`; generation is eager at construction and served from the
/// same in-memory index shape as [`crate::replay::Replay`]. Every row
/// carries [`Provenance::Synthetic`] — synthetic series are never real data.
#[derive(Debug)]
pub struct SyntheticPriceGenerator {
    /// Parameters the series was generated from.
    params: SyntheticParams,
    /// Generated range.
    range: TimeRange,
    /// In-memory index built at construction.
    index: SourceIndex,
}

impl SyntheticPriceGenerator {
    /// Generate the full series for `range` eagerly.
    ///
    /// # Errors
    /// [`ErcotError::InvalidParam`] when any parameter fails validation
    /// (bad cadence, out-of-range fractions, malformed matrix, non-finite
    /// or inconsistent caps/ORDC values); [`ErcotError::Time`] on a
    /// timestamp overflow (unreachable for realistic ranges).
    pub fn new(params: SyntheticParams, range: TimeRange, rules: &ErcotRules) -> Result<Self> {
        let resolved = resolve_and_validate(&params, rules)?;
        let mut index = SourceIndex::default();
        let mut driver = GenDriver {
            params: &params,
            resolved: &resolved,
            shape: season_shape(params.season),
            rng: ChaCha8Rng::seed_from_u64(params.seed),
            state: Regime::Normal,
            cap_tracker: CapTracker::default(),
        };
        let interval = i64::from(params.interval_secs);
        let interval_hours = f64::from(params.interval_secs) / 3600.0;
        let start_unix = range.start.unix_timestamp();
        let mut ts_unix = start_unix - start_unix.rem_euclid(interval);
        let end_unix = range.end.unix_timestamp();
        let mut regimes_at: BTreeMap<i64, Regime> = BTreeMap::new();
        let mut dam_hours: BTreeMap<i64, DamAccum> = BTreeMap::new();
        while ts_unix < end_unix {
            let ts = OffsetDateTime::from_unix_timestamp(ts_unix)
                .map_err(|e| ErcotError::Time(format!("interval ts {ts_unix}: {e}")))?;
            let (sample, signal, regime) = driver.step_interval(ts, ts_unix, interval_hours);
            let accum = dam_hours.entry(ts_unix.div_euclid(3600)).or_default();
            accum.lmp_sum += sample.lmp_usd_per_mwh;
            accum.ordc_sum += sample.ordc_adder_usd_per_mwh;
            accum.count += 1;
            index.insert_rt(sample);
            index.insert_sys(signal);
            regimes_at.insert(ts_unix, regime);
            ts_unix += interval;
        }
        finalize_dam(&params, &dam_hours, &mut index);
        finalize_as(&params, &regimes_at, &mut index, &mut driver.rng);
        Ok(Self { params, range, index })
    }

    /// The parameters this series was generated from.
    #[must_use]
    pub fn params(&self) -> &SyntheticParams {
        &self.params
    }

    /// The generated range.
    #[must_use]
    pub const fn range(&self) -> TimeRange {
        self.range
    }

    /// The generated cadence, seconds.
    #[must_use]
    pub const fn interval_secs(&self) -> u32 {
        self.params.interval_secs
    }

    /// The RT sample whose interval contains `ts`, if any (same lookup
    /// contract as [`crate::replay::Replay::rt_spp_at`]).
    #[must_use]
    pub fn rt_spp_at(&self, loc: &Location, ts: OffsetDateTime) -> Option<&PriceSample> {
        self.index.rt_spp_at(loc, ts)
    }
}

impl PriceSource for SyntheticPriceGenerator {
    fn dam_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>> {
        Ok(self.index.dam_spps(loc, r))
    }

    fn rt_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>> {
        Ok(self.index.rt_spps(loc, r))
    }

    fn as_prices(&self, r: TimeRange) -> Result<Vec<AsPrice>> {
        Ok(self.index.as_prices(r))
    }

    fn system_signals(&self, r: TimeRange) -> Result<Vec<SystemSignal>> {
        Ok(self.index.system_signals(r))
    }
}

/// The overlay active at `ts`, if any (first match wins).
fn active_overlay(overlays: &[EventOverlay], ts: OffsetDateTime) -> Option<&EventOverlay> {
    overlays
        .iter()
        .find(|o| ts >= o.start && ts < o.start + Duration::seconds_f64(o.duration_h * 3600.0))
}

/// DAM SPPs: hourly averages of the generated RT series (a deterministic
/// function of the same seed — no additional draws, spec D.4).
fn finalize_dam(
    params: &SyntheticParams,
    dam_hours: &BTreeMap<i64, DamAccum>,
    index: &mut SourceIndex,
) {
    for (hour_unix, accum) in dam_hours {
        let Ok(ts) = OffsetDateTime::from_unix_timestamp(hour_unix.saturating_mul(3600)) else {
            continue;
        };
        let n = accum.count as f64;
        index.insert_dam(PriceSample {
            ts,
            interval_secs: 3600,
            location: params.location.clone(),
            lmp_usd_per_mwh: accum.lmp_sum / n,
            ordc_adder_usd_per_mwh: accum.ordc_sum / n,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::Synthetic,
        });
    }
}

/// DAM AS MCPCs: per-product hourly prices correlated with the hour's
/// regime (regime of the hour's first RT interval) and discounted by
/// `as_saturation_gw`. Draws: one uniform per (hour, product), hours
/// ascending, products in [`AsProduct::ALL`] order.
fn finalize_as(
    params: &SyntheticParams,
    regimes_at: &BTreeMap<i64, Regime>,
    index: &mut SourceIndex,
    rng: &mut ChaCha8Rng,
) {
    let mut hours: Vec<i64> = regimes_at.keys().map(|ts| ts.div_euclid(3600)).collect();
    hours.dedup();
    for hour_unix in hours {
        let regime = regimes_at
            .range(hour_unix.saturating_mul(3600)..)
            .next()
            .map_or(Regime::Normal, |(_, regime)| *regime);
        let Ok(ts) = OffsetDateTime::from_unix_timestamp(hour_unix.saturating_mul(3600)) else {
            continue;
        };
        for product in AsProduct::ALL {
            let mcpc = as_mcpc(product, regime, rng.gen::<f64>(), params.as_saturation_gw);
            index.insert_as(AsPrice {
                ts,
                product,
                mcpc_usd_per_mw: mcpc,
                provenance: Provenance::Synthetic,
            });
        }
    }
}

/// Upper bound for event-overlay durations and the emergency-cap rolling
/// window: one leap year in hours. Keeps `Duration::seconds_f64` and the
/// `as i64` window conversion far from overflow.
const MAX_EVENT_WINDOW_H: f64 = 8784.0;

/// Resolve caps/ORDC/matrix from params + rules and validate everything.
fn resolve_and_validate(params: &SyntheticParams, rules: &ErcotRules) -> Result<Resolved> {
    validate_scalars(params, rules)?;
    let (hcap, lcap, emergency_hours, window_hours) = resolve_caps(params, rules)?;
    let (ordc_threshold_mw, ordc_floor_mw, voll) = resolve_ordc(params, rules)?;
    let matrix = resolve_matrix(params)?;
    Ok(Resolved {
        hcap,
        lcap,
        emergency_hours,
        emergency_window_secs: (window_hours * 3600.0) as i64,
        ordc_threshold_mw,
        ordc_floor_mw,
        voll,
        matrix,
    })
}

/// Validate the scalar scenario fields and event overlays.
fn validate_scalars(params: &SyntheticParams, rules: &ErcotRules) -> Result<()> {
    if !rules.settlement.allowed_interval_secs.contains(&params.interval_secs) {
        return Err(ErcotError::InvalidParam(format!(
            "interval_secs {} not in rules settlement.allowed_interval_secs {:?}",
            params.interval_secs, rules.settlement.allowed_interval_secs
        )));
    }
    if !(0.0..=1.0).contains(&params.solar_penetration) {
        return Err(ErcotError::InvalidParam(format!(
            "solar_penetration {} outside [0, 1]",
            params.solar_penetration
        )));
    }
    if !(params.reserve_margin > 0.0 && params.reserve_margin <= 1.0) {
        return Err(ErcotError::InvalidParam(format!(
            "reserve_margin {} outside (0, 1]",
            params.reserve_margin
        )));
    }
    if !(params.as_saturation_gw >= 0.0 && params.as_saturation_gw.is_finite()) {
        return Err(ErcotError::InvalidParam(format!(
            "as_saturation_gw {} must be finite and >= 0",
            params.as_saturation_gw
        )));
    }
    if !(-50.0..=-20.0).contains(&params.floor_usd_per_mwh) {
        return Err(ErcotError::InvalidParam(format!(
            "floor_usd_per_mwh {} outside [-50, -20]",
            params.floor_usd_per_mwh
        )));
    }
    for overlay in &params.event_overlay {
        if !(overlay.duration_h.is_finite()
            && overlay.duration_h > 0.0
            && overlay.duration_h <= MAX_EVENT_WINDOW_H)
        {
            return Err(ErcotError::InvalidParam(format!(
                "event_overlay duration_h {} must be in (0, {MAX_EVENT_WINDOW_H}]",
                overlay.duration_h
            )));
        }
        if !(overlay.cap_usd_per_mwh.is_finite() && overlay.cap_usd_per_mwh > 0.0) {
            return Err(ErcotError::InvalidParam(format!(
                "event_overlay cap {} must be > 0",
                overlay.cap_usd_per_mwh
            )));
        }
    }
    Ok(())
}

/// Resolve the effective offer caps (params override wins) and validate.
fn resolve_caps(
    params: &SyntheticParams,
    rules: &ErcotRules,
) -> Result<(f64, f64, f64, f64)> {
    let (hcap, lcap, emergency_hours, window_hours) = params.caps.as_ref().map_or(
        (
            rules.offer_caps.hcap_usd_per_mwh,
            rules.offer_caps.lcap_usd_per_mwh,
            rules.offer_caps.emergency_hours_at_hcap,
            rules.offer_caps.emergency_rolling_window_hours,
        ),
        |o| {
            (
                o.hcap_usd_per_mwh,
                o.lcap_usd_per_mwh,
                o.emergency_hours_at_hcap,
                o.emergency_rolling_window_hours,
            )
        },
    );
    if !(hcap.is_finite() && hcap > 0.0 && lcap.is_finite() && lcap > 0.0 && lcap <= hcap) {
        return Err(ErcotError::InvalidParam(format!(
            "caps require 0 < lcap <= hcap, got lcap {lcap} hcap {hcap}"
        )));
    }
    if !(emergency_hours.is_finite()
        && emergency_hours > 0.0
        && window_hours.is_finite()
        && window_hours >= emergency_hours
        && window_hours <= MAX_EVENT_WINDOW_H)
    {
        return Err(ErcotError::InvalidParam(format!(
            "emergency trigger requires 0 < hours <= window <= {MAX_EVENT_WINDOW_H}, got hours {emergency_hours} window {window_hours}"
        )));
    }
    Ok((hcap, lcap, emergency_hours, window_hours))
}

/// Resolve the effective ORDC params (params override wins) and validate.
fn resolve_ordc(params: &SyntheticParams, rules: &ErcotRules) -> Result<(f64, f64, f64)> {
    let ordc_threshold_mw = params.ordc.as_ref().map_or(rules.ordc.threshold_mw, |o| o.threshold_mw);
    let ordc_floor_mw = params.ordc.as_ref().map_or(rules.ordc.floor_mw, |o| o.floor_mw);
    let voll = params.ordc.as_ref().map_or(rules.ordc.voll_usd_per_mwh, |o| o.voll_usd_per_mwh);
    if !(ordc_threshold_mw > ordc_floor_mw && ordc_floor_mw > 0.0 && voll > 0.0 && voll.is_finite()) {
        return Err(ErcotError::InvalidParam(format!(
            "ordc requires threshold {ordc_threshold_mw} > floor {ordc_floor_mw} > 0 and voll {voll} > 0"
        )));
    }
    Ok((ordc_threshold_mw, ordc_floor_mw, voll))
}

/// Resolve the transition matrix (override verbatim, else derived) and
/// validate rows.
fn resolve_matrix(params: &SyntheticParams) -> Result<[[f64; 4]; 4]> {
    match params.regime_matrix {
        Some(m) => {
            for row in &m {
                let sum: f64 = row.iter().sum();
                if row.iter().any(|p| !(p.is_finite() && *p >= 0.0)) || (sum - 1.0).abs() > 1e-6 {
                    return Err(ErcotError::InvalidParam(format!(
                        "regime_matrix rows must be finite, >= 0, and sum to 1 (got {row:?})"
                    )));
                }
            }
            Ok(m)
        }
        None => Ok(default_matrix(params, f64::from(params.interval_secs) / 900.0)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::types::TradingHub;
    use sha2::{Digest, Sha256};
    use time::macros::datetime;

    fn loc() -> Location {
        Location::Hub(TradingHub::North)
    }

    fn three_days() -> TimeRange {
        TimeRange::new(datetime!(2026-08-14 05:00 UTC), datetime!(2026-08-17 05:00 UTC)).unwrap()
    }

    fn base_params(seed: u64) -> SyntheticParams {
        SyntheticParams {
            seed,
            season: Season::Summer,
            solar_penetration: 0.35,
            location: loc(),
            ..SyntheticParams::default()
        }
    }

    /// Rows that pin the chain in one regime from the first interval.
    fn pinned_matrix(idx: usize) -> [[f64; 4]; 4] {
        let mut m = [[0.0; 4]; 4];
        for row in &mut m {
            row[idx] = 1.0;
        }
        m
    }

    #[test]
    fn identical_params_are_bit_identical() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let a = SyntheticPriceGenerator::new(base_params(42), range, &rules).unwrap();
        let b = SyntheticPriceGenerator::new(base_params(42), range, &rules).unwrap();
        let va = a.rt_spps(&loc(), range).unwrap();
        let vb = b.rt_spps(&loc(), range).unwrap();
        assert_eq!(va, vb);
        assert_eq!(a.dam_spps(&loc(), range).unwrap(), b.dam_spps(&loc(), range).unwrap());
        assert_eq!(a.as_prices(range).unwrap(), b.as_prices(range).unwrap());
        assert_eq!(a.system_signals(range).unwrap(), b.system_signals(range).unwrap());
        let ha = Sha256::digest(serde_json::to_string(&va).unwrap().as_bytes());
        let hb = Sha256::digest(serde_json::to_string(&vb).unwrap().as_bytes());
        assert_eq!(ha, hb);
        // A different seed produces a different series.
        let c = SyntheticPriceGenerator::new(base_params(43), range, &rules).unwrap();
        assert_ne!(va, c.rt_spps(&loc(), range).unwrap());
    }

    #[test]
    fn series_shape_and_cadence() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let gen = SyntheticPriceGenerator::new(base_params(7), range, &rules).unwrap();
        let rt = gen.rt_spps(&loc(), range).unwrap();
        // 72 h * 4 = 288 intervals at 900 s.
        assert_eq!(rt.len(), 288);
        assert!(rt.windows(2).all(|w| w[1].ts - w[0].ts == Duration::seconds(900)));
        assert!(rt.iter().all(|s| s.interval_secs == 900));
        assert!(rt.iter().all(|s| s.provenance == Provenance::Synthetic));
        assert!(rt.iter().all(|s| s.location == loc()));
        assert_eq!(gen.interval_secs(), 900);
        // DAM: 72 hourly averages; AS: 72 h * 5 products ordered (ts, product).
        assert_eq!(gen.dam_spps(&loc(), range).unwrap().len(), 72);
        let as_prices = gen.as_prices(range).unwrap();
        assert_eq!(as_prices.len(), 360);
        assert!(as_prices
            .windows(2)
            .all(|w| (w[0].ts, w[0].product) < (w[1].ts, w[1].product)));
        // System signals: load positive, reserves present, fuel mix ~1.
        let sys = gen.system_signals(range).unwrap();
        assert_eq!(sys.len(), 288);
        assert!(sys.iter().all(|s| s.system_load_mw > 0.0));
        assert!(sys.iter().all(|s| s.reserves_mw.is_some()));
        for s in &sys {
            let mix = s.fuel_mix.as_ref().unwrap();
            let total: f64 = mix.values().sum();
            assert!((total - 1.0).abs() < 1e-12);
        }
        // 300 s cadence triples the interval count.
        let mut p = base_params(7);
        p.interval_secs = 300;
        let gen300 = SyntheticPriceGenerator::new(p, range, &rules).unwrap();
        assert_eq!(gen300.rt_spps(&loc(), range).unwrap().len(), 864);
        assert_eq!(gen300.interval_secs(), 300);
    }

    #[test]
    fn dam_is_hourly_average_of_rt() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let mut p = base_params(11);
        p.regime_matrix = Some(pinned_matrix(0)); // Normal all the way.
        let gen = SyntheticPriceGenerator::new(p, range, &rules).unwrap();
        let rt = gen.rt_spps(&loc(), range).unwrap();
        let dam = gen.dam_spps(&loc(), range).unwrap();
        let expected: f64 = rt.iter().take(4).map(|s| s.lmp_usd_per_mwh).sum::<f64>() / 4.0;
        assert!((dam[0].lmp_usd_per_mwh - expected).abs() < 1e-9);
        assert_eq!(dam[0].ts, range.start);
        assert!(dam.iter().all(|s| s.interval_secs == 3600));
    }

    #[test]
    fn prices_respect_caps_and_emergency_latch() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let mut p = base_params(5);
        p.season = Season::Winter;
        p.regime_matrix = Some(pinned_matrix(3)); // WinterStorm from t0.
        let gen = SyntheticPriceGenerator::new(p, range, &rules).unwrap();
        let rt = gen.rt_spps(&loc(), range).unwrap();
        let hcap = rules.offer_caps.hcap_usd_per_mwh;
        let lcap = rules.offer_caps.lcap_usd_per_mwh;
        let floor = gen.params().floor_usd_per_mwh;
        // No price ever exceeds HCAP; none dips below the floor.
        for s in &rt {
            assert!(s.spp_usd_per_mwh() <= hcap + 1e-9);
            assert!(s.lmp_usd_per_mwh >= floor - 1e-9);
        }
        // The storm pins every interval at HCAP (reserves << floor -> adder
        // ~VOLL), so the 12 h trigger trips after exactly 48 x 900 s...
        for s in &rt[..48] {
            assert!(
                (s.spp_usd_per_mwh() - hcap).abs() < 1e-9,
                "{} != {hcap}",
                s.spp_usd_per_mwh()
            );
        }
        // ...and from interval 48 on the effective cap is LCAP.
        for s in &rt[48..] {
            assert!(
                (s.spp_usd_per_mwh() - lcap).abs() < 1e-9,
                "{} != {lcap}",
                s.spp_usd_per_mwh()
            );
        }
    }

    #[test]
    fn scarcity_regime_splits_ordc_adder() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let mut p = base_params(9);
        p.regime_matrix = Some(pinned_matrix(2)); // ScarcityOrdc from t0.
        let gen = SyntheticPriceGenerator::new(p, range, &rules).unwrap();
        let rt = gen.rt_spps(&loc(), range).unwrap();
        assert!(rt.iter().all(|s| s.ordc_adder_usd_per_mwh > 0.0));
        assert!(rt
            .iter()
            .all(|s| s.ordc_adder_usd_per_mwh <= rules.ordc.voll_usd_per_mwh));
        // Adder is split out: spp = lmp + ordc (+ rdpa = 0).
        assert!(rt.iter().all(|s| {
            (s.spp_usd_per_mwh() - s.lmp_usd_per_mwh - s.ordc_adder_usd_per_mwh).abs() < 1e-9
        }));
        // Reserves are drawn inside the scarcity band.
        let sys = gen.system_signals(range).unwrap();
        for s in &sys {
            let r = s.reserves_mw.unwrap();
            assert!(r >= rules.ordc.floor_mw && r < rules.ordc.threshold_mw);
        }
    }

    #[test]
    fn as_prices_correlate_with_regime_and_saturation() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let mut normal = base_params(21);
        normal.regime_matrix = Some(pinned_matrix(0));
        let mut scarcity = base_params(21);
        scarcity.regime_matrix = Some(pinned_matrix(2));
        let normal_gen = SyntheticPriceGenerator::new(normal, range, &rules).unwrap();
        let scarcity_gen = SyntheticPriceGenerator::new(scarcity, range, &rules).unwrap();
        let mean_ecrs = |g: &SyntheticPriceGenerator| {
            let v: Vec<f64> = g
                .as_prices(range)
                .unwrap()
                .iter()
                .filter(|a| a.product == AsProduct::Ecrs)
                .map(|a| a.mcpc_usd_per_mw)
                .collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        assert!(mean_ecrs(&scarcity_gen) > 3.0 * mean_ecrs(&normal_gen));
        // Competing storage discounts AS prices ~ 1 / (1 + 0.5 * gw).
        let mut saturated = base_params(21);
        saturated.regime_matrix = Some(pinned_matrix(2));
        saturated.as_saturation_gw = 10.0;
        let sat_gen = SyntheticPriceGenerator::new(saturated, range, &rules).unwrap();
        let ratio = mean_ecrs(&scarcity_gen) / mean_ecrs(&sat_gen);
        assert!((ratio - 6.0).abs() < 0.6, "ratio {ratio}");
    }

    #[test]
    fn winter_storm_overlay_uses_era_cap() {
        let rules = ErcotRules::current().unwrap();
        let range = three_days();
        let storm_start = range.start + Duration::hours(24);
        let mut p = base_params(13);
        p.season = Season::Winter;
        p.regime_matrix = Some(pinned_matrix(0)); // Normal outside the overlay.
        p.event_overlay = vec![EventOverlay {
            kind: EventKind::WinterStorm,
            start: storm_start,
            duration_h: 24.0,
            cap_usd_per_mwh: rules.offer_caps.winter_storm_uri_cap_usd_per_mwh,
        }];
        let gen = SyntheticPriceGenerator::new(p, range, &rules).unwrap();
        let rt = gen.rt_spps(&loc(), range).unwrap();
        let hcap = rules.offer_caps.hcap_usd_per_mwh;
        let uri_cap = rules.offer_caps.winter_storm_uri_cap_usd_per_mwh;
        for s in &rt {
            if s.ts >= storm_start && s.ts < storm_start + Duration::hours(24) {
                // Overlay window: above the 2026 HCAP, at/below the era cap.
                assert!(s.spp_usd_per_mwh() > hcap, "{} <= {hcap}", s.spp_usd_per_mwh());
                assert!(s.spp_usd_per_mwh() <= uri_cap + 1e-9);
            } else {
                assert!(s.spp_usd_per_mwh() <= hcap + 1e-9);
            }
        }
    }

    #[test]
    fn params_are_scenario_document_friendly() {
        // `Location` serializes as the canonical settlement-point string.
        let doc = serde_json::json!({
            "seed": 42,
            "season": "summer",
            "solar_penetration": 0.35,
            "reserve_margin": 0.10,
            "event_overlay": [
                {"kind": "winter_storm", "start": "2026-02-14T00:00:00-06:00",
                 "duration_h": 96, "cap": 9000}
            ],
            "location": "HB_NORTH"
        });
        let params: SyntheticParams = serde_json::from_value(doc.clone()).unwrap();
        assert_eq!(params.seed, 42);
        assert_eq!(params.season, Season::Summer);
        assert_eq!(params.location, loc());
        assert_eq!(params.interval_secs, 900);
        assert!((params.floor_usd_per_mwh - -20.0).abs() < f64::EPSILON);
        assert!((params.reserve_margin - 0.10).abs() < f64::EPSILON);
        assert_eq!(params.event_overlay.len(), 1);
        assert!((params.event_overlay[0].cap_usd_per_mwh - 9000.0).abs() < f64::EPSILON);
        // Unknown fields are refused loudly.
        let mut bad = doc;
        bad.as_object_mut().unwrap().insert("bogus".to_string(), serde_json::json!(1));
        assert!(serde_json::from_value::<SyntheticParams>(bad).is_err());
        // Bad cadence is refused against the rules.
        let rules = ErcotRules::current().unwrap();
        let mut p = base_params(1);
        p.interval_secs = 600;
        assert!(matches!(
            SyntheticPriceGenerator::new(p, three_days(), &rules),
            Err(ErcotError::InvalidParam(_))
        ));
        // Default mirrors the serde defaults.
        let d = SyntheticParams::default();
        assert_eq!(d.seed, 0);
        assert_eq!(d.interval_secs, 900);
        assert_eq!(d.location, Location::LoadZone(LoadZone::Houston));
    }
}
