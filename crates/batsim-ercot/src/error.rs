//! Error type for batsim-ercot.

use time::OffsetDateTime;

/// All fallible operations in batsim-ercot.
#[derive(Debug, thiserror::Error)]
pub enum ErcotError {
    /// Feature not supported by this source (e.g. live streaming on replay).
    #[error("unsupported: {0}")]
    Unsupported(&'static str),

    /// No replay data covers the requested signal/location/range.
    #[error("replay data not found for {signal} at {location} covering {start}..{end} (looked under {root})")]
    DataNotFound {
        /// Signal name (rtm_spp, dam_spp, as_mcpc, system_load).
        signal: String,
        /// Location string.
        location: String,
        /// Range start.
        start: OffsetDateTime,
        /// Range end.
        end: OffsetDateTime,
        /// Archive root searched.
        root: String,
    },

    /// Parquet schema version mismatch; fail loud, never mis-map columns.
    #[error("unsupported parquet schema version {found} (expected {expected}) in {path}")]
    SchemaVersion {
        /// File path.
        path: String,
        /// Version found in file metadata.
        found: u32,
        /// Version this build supports.
        expected: u32,
    },

    /// Ingest parse failure.
    #[error("parse error in {context}: {detail}")]
    Parse {
        /// What was being parsed (file/sheet/report).
        context: String,
        /// Detail.
        detail: String,
    },

    /// Bad time range.
    #[error("invalid time range: {start} .. {end}")]
    InvalidRange {
        /// Range start.
        start: OffsetDateTime,
        /// Range end.
        end: OffsetDateTime,
    },

    /// Invalid scenario/rules parameter.
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Arrow/Parquet failure.
    #[error("parquet error: {0}")]
    Parquet(String),

    /// HTTP fetch failure (ingest only; never on a sim path).
    #[error("fetch error: {0}")]
    Fetch(String),

    /// Time component error.
    #[error("time error: {0}")]
    Time(String),
}

impl From<parquet::errors::ParquetError> for ErcotError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        Self::Parquet(e.to_string())
    }
}

impl From<arrow::error::ArrowError> for ErcotError {
    fn from(e: arrow::error::ArrowError) -> Self {
        Self::Parquet(e.to_string())
    }
}

/// Convenience alias.
pub type Result<T, E = ErcotError> = std::result::Result<T, E>;
