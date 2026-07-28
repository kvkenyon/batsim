//! Core market data types for ERCOT integration (spec Part D.3.2).
//!
//! All timestamps are UTC interval-START with explicit `interval_secs`.
//! Money is USD `f64` (Part C chose `f64` once; physics is `f64` and
//! settlement consumes physics outputs directly). Determinism: every type
//! derives `PartialEq` and serializes through `serde`; bit-identical replay
//! is a tested contract.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::ErcotError;

/// ERCOT settlement location (spec D.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    /// A trading hub (e.g. HB_NORTH).
    Hub(TradingHub),
    /// A competitive load zone (e.g. LZ_NORTH).
    LoadZone(LoadZone),
    /// A resource node or any other settlement point name, verbatim.
    Node(String),
}

impl Location {
    /// Parse an ERCOT settlement-point name (`HB_NORTH`, `LZ_HOUSTON`, ...).
    ///
    /// Unknown prefixes map to `Location::Node(name)` verbatim so replay
    /// never silently drops a settlement point ERCOT adds later.
    #[must_use]
    pub fn from_settlement_point(name: &str) -> Self {
        match name {
            "HB_BUSAVG" => Self::Hub(TradingHub::BusAvg),
            "HB_NORTH" => Self::Hub(TradingHub::North),
            "HB_WEST" => Self::Hub(TradingHub::West),
            "HB_HOUSTON" => Self::Hub(TradingHub::Houston),
            "HB_SOUTH" => Self::Hub(TradingHub::South),
            "HB_PANHANDLE" => Self::Hub(TradingHub::Panhandle),
            "LZ_WEST" => Self::LoadZone(LoadZone::West),
            "LZ_NORTH" => Self::LoadZone(LoadZone::North),
            "LZ_SOUTH" => Self::LoadZone(LoadZone::South),
            "LZ_HOUSTON" => Self::LoadZone(LoadZone::Houston),
            other => Self::Node(other.to_string()),
        }
    }

    /// Canonical ERCOT settlement-point name.
    #[must_use]
    pub fn settlement_point(&self) -> String {
        match self {
            Self::Hub(h) => match h {
                TradingHub::BusAvg => "HB_BUSAVG",
                TradingHub::North => "HB_NORTH",
                TradingHub::West => "HB_WEST",
                TradingHub::Houston => "HB_HOUSTON",
                TradingHub::South => "HB_SOUTH",
                TradingHub::Panhandle => "HB_PANHANDLE",
            }
            .to_string(),
            Self::LoadZone(z) => match z {
                LoadZone::West => "LZ_WEST",
                LoadZone::North => "LZ_NORTH",
                LoadZone::South => "LZ_SOUTH",
                LoadZone::Houston => "LZ_HOUSTON",
            }
            .to_string(),
            Self::Node(n) => n.clone(),
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.settlement_point())
    }
}

/// ERCOT trading hubs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingHub {
    /// HB_BUSAVG (bus average).
    BusAvg,
    /// HB_NORTH.
    North,
    /// HB_WEST.
    West,
    /// HB_HOUSTON.
    Houston,
    /// HB_SOUTH.
    South,
    /// HB_PANHANDLE.
    Panhandle,
}

/// ERCOT competitive load zones (spec D.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadZone {
    /// LZ_WEST.
    West,
    /// LZ_NORTH.
    North,
    /// LZ_SOUTH.
    South,
    /// LZ_HOUSTON.
    Houston,
}

/// Row-level provenance (spec D.3.1 / provenance convention).
///
/// `Omitted` marks a component the pipeline does not supply (e.g. the ORDC
/// adder split when the adder report is not ingested): consumers must treat
/// the value as absent, never as measured zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Near-real-time indicative publication (not settlement quality).
    RealTimeIndicative,
    /// Settlement-quality historical publication (48-h reports).
    SettlementFinal,
    /// Settlement-final with a price-correction report applied.
    Corrected,
    /// Synthesized by `SyntheticPriceGenerator`; never present as real.
    Synthetic,
    /// Not supplied by the source; value is a placeholder.
    Omitted,
}

/// One market-signal sample, normalized (spec D.3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceSample {
    /// Interval start, UTC.
    pub ts: OffsetDateTime,
    /// Interval length in seconds (300 | 900 | 3600).
    pub interval_secs: u32,
    /// Settlement location.
    pub location: Location,
    /// Base energy price, $/MWh (adder-exclusive when adders are split out).
    pub lmp_usd_per_mwh: f64,
    /// ORDC scarcity adder component, $/MWh (0 when n/a or omitted).
    pub ordc_adder_usd_per_mwh: f64,
    /// Reliability-deployment adder component, $/MWh (0 when n/a or omitted).
    pub rdpa_adder_usd_per_mwh: f64,
    /// Row provenance.
    pub provenance: Provenance,
}

impl PriceSample {
    /// Effective settlement price: LMP plus adder components.
    #[must_use]
    pub fn spp_usd_per_mwh(&self) -> f64 {
        self.lmp_usd_per_mwh + self.ordc_adder_usd_per_mwh + self.rdpa_adder_usd_per_mwh
    }
}

/// ERCOT ancillary-service products (spec D.1.3).
///
/// `RegUp`/`RegDown` exist for completeness of the DAM price series but are
/// modeled as NOT available to aggregated residential fleets (rules config
/// carries `available_to_ader = false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsProduct {
    /// Regulation Up.
    RegUp,
    /// Regulation Down.
    RegDown,
    /// Responsive Reserve Service.
    Rrs,
    /// Non-Spinning Reserve.
    NonSpin,
    /// ERCOT Contingency Reserve Service (introduced 2023-06-10).
    Ecrs,
}

impl AsProduct {
    /// ERCOT DAM report column name for this product's clearing price.
    #[must_use]
    pub const fn dam_column(&self) -> &'static str {
        match self {
            Self::RegUp => "REGUP",
            Self::RegDown => "REGDN",
            Self::Rrs => "RRS",
            Self::NonSpin => "NSPIN",
            Self::Ecrs => "ECRS",
        }
    }

    /// All products in canonical order.
    pub const ALL: [Self; 5] = [
        Self::RegUp,
        Self::RegDown,
        Self::Rrs,
        Self::NonSpin,
        Self::Ecrs,
    ];
}

impl std::fmt::Display for AsProduct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.dam_column())
    }
}

/// DAM ancillary-service clearing price for capacity (hourly).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsPrice {
    /// Interval start, UTC (hourly).
    pub ts: OffsetDateTime,
    /// Product.
    pub product: AsProduct,
    /// Market clearing price for capacity, $/MW.
    pub mcpc_usd_per_mw: f64,
    /// Row provenance.
    pub provenance: Provenance,
}

/// Fuel-mix fractions by fuel name (sums to ~1.0).
pub type FuelMix = std::collections::BTreeMap<String, f64>;

/// System-wide signal sample (drives 4CP watch and emissions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSignal {
    /// Interval start, UTC.
    pub ts: OffsetDateTime,
    /// Total ERCOT system load, MW.
    pub system_load_mw: f64,
    /// Operating reserves, MW (drives ORDC state when split out).
    pub reserves_mw: Option<f64>,
    /// Generation fuel mix fractions, when published.
    pub fuel_mix: Option<FuelMix>,
}

/// Half-open UTC time range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    /// Start (inclusive).
    pub start: OffsetDateTime,
    /// End (exclusive).
    pub end: OffsetDateTime,
}

impl TimeRange {
    /// Construct, validating `start < end`.
    ///
    /// # Errors
    /// Returns `ErcotError::InvalidRange` when `start >= end`.
    pub fn new(start: OffsetDateTime, end: OffsetDateTime) -> Result<Self, ErcotError> {
        if start >= end {
            return Err(ErcotError::InvalidRange { start, end });
        }
        Ok(Self { start, end })
    }

    /// True if `ts` falls inside `[start, end)`.
    #[must_use]
    pub fn contains(&self, ts: OffsetDateTime) -> bool {
        ts >= self.start && ts < self.end
    }
}
