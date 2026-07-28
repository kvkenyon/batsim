//! Canonical Parquet archive layout (spec D.3.3).
//!
//! Layout: `<root>/<signal>/date=YYYY-MM-DD/location=<LOC>.parquet` plus a
//! top-level `manifest.json`. Every file carries `batsim.schema_version` in
//! its Parquet key-value metadata; the reader refuses unknown versions.
//!
//! All timestamps are UTC interval-start stored as `INT64` epoch seconds
//! (sufficient for market data; avoids timestamp-unit ambiguity across
//! writers). `interval_secs` makes cadence explicit per row.

use serde::{Deserialize, Serialize};

/// Parquet schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// Parquet file-metadata key carrying the schema version.
pub const SCHEMA_VERSION_KEY: &str = "batsim.schema_version";

/// Signal: real-time market settlement point prices.
pub const SIGNAL_RTM_SPP: &str = "rtm_spp";
/// Signal: day-ahead market settlement point prices.
pub const SIGNAL_DAM_SPP: &str = "dam_spp";
/// Signal: DAM ancillary-service clearing prices for capacity.
pub const SIGNAL_AS_MCPC: &str = "as_mcpc";
/// Signal: system-wide load (and reserves when available).
pub const SIGNAL_SYSTEM_LOAD: &str = "system_load";

/// All known signals.
pub const SIGNALS: [&str; 4] = [
    SIGNAL_RTM_SPP,
    SIGNAL_DAM_SPP,
    SIGNAL_AS_MCPC,
    SIGNAL_SYSTEM_LOAD,
];

/// Price-table column names (rtm_spp / dam_spp).
pub mod price_cols {
    /// Interval start, UTC epoch seconds (i64).
    pub const TS: &str = "interval_start_utc";
    /// Interval length, seconds (u32).
    pub const INTERVAL_SECS: &str = "interval_secs";
    /// Settlement-point name (utf8).
    pub const LOCATION: &str = "location";
    /// Base energy price $/MWh (f64).
    pub const LMP: &str = "lmp_usd_per_mwh";
    /// ORDC adder $/MWh (f64).
    pub const ORDC: &str = "ordc_adder_usd_per_mwh";
    /// RDPA adder $/MWh (f64).
    pub const RDPA: &str = "rdpa_adder_usd_per_mwh";
    /// Provenance label (utf8, `Provenance` serde name).
    pub const PROVENANCE: &str = "provenance";
    /// Pre-correction LMP retained for auditability (f64; equals `lmp`
    /// when no correction was applied).
    pub const LMP_RAW: &str = "lmp_usd_per_mwh_raw";
}

/// AS-price table column names (as_mcpc).
pub mod as_cols {
    /// Interval start, UTC epoch seconds (i64).
    pub const TS: &str = "interval_start_utc";
    /// Product name (utf8, e.g. "ECRS").
    pub const PRODUCT: &str = "product";
    /// Clearing price $/MW (f64).
    pub const MCPC: &str = "mcpc_usd_per_mw";
    /// Provenance label (utf8).
    pub const PROVENANCE: &str = "provenance";
}

/// System-load table column names (system_load).
pub mod load_cols {
    /// Interval start, UTC epoch seconds (i64).
    pub const TS: &str = "interval_start_utc";
    /// System load MW (f64).
    pub const LOAD: &str = "system_load_mw";
    /// Operating reserves MW (f64, nullable).
    pub const RESERVES: &str = "reserves_mw";
}

/// One dataset entry in `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Signal name (`rtm_spp` ...).
    pub signal: String,
    /// Date partition (YYYY-MM-DD, CPT operating day).
    pub date: String,
    /// Location partition (`ALL` for location-less signals).
    pub location: String,
    /// Parquet path relative to the archive root.
    pub path: String,
    /// Row count.
    pub rows: u64,
    /// Row-level provenance of the data.
    pub provenance: crate::types::Provenance,
}

/// Archive-level manifest: what is here, where it came from, when ingested.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Parquet schema version (`SCHEMA_VERSION`).
    pub schema_version: u32,
    /// ERCOT rules version used at ingest (`ErcotRules::meta.protocol_version`).
    pub rules_version: String,
    /// Source report identifier (ERCOT report type ID or "synthetic").
    pub source_report: String,
    /// ERCOT MIS DocID(s) the data was parsed from, when fetched.
    pub source_doc_ids: Vec<u64>,
    /// Ingest timestamp (RFC 3339 UTC).
    pub ingested_at: String,
    /// Dataset entries.
    pub entries: Vec<ManifestEntry>,
}
