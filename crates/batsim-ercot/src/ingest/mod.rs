//! ERCOT MIS ingestion pipeline (spec D.3.3).
//!
//! Pipeline: ERCOT MIS report (`.xlsx` / `.csv` / zip-of-csv) → parse and
//! normalize (CPT → UTC via [`crate::cpt`], hour-ending → interval-start) →
//! canonical Parquet partitions (`<signal>/date=YYYY-MM-DD/location=<LOC>.parquet`)
//! → `manifest.json` read/upsert.
//!
//! - [`writers`]: canonical Parquet partition writers + manifest handling
//!   (pure functions; no network, no wall clock).
//! - [`parse`]: report parsers (calamine `.xlsx`, CSV, zip-of-CSV).
//! - [`fetch`]: MIS download client (`ureq`). Used only by the
//!   `batsim-ercot-ingest` binary; never on a simulation path.
//!
//! Provenance: historical MIS reports are settlement-quality, so parsed rows
//! carry [`Provenance::SettlementFinal`]. The ORDC/RDPA adder split is NOT
//! ingested in v1 (the adder reports moved behind the data.ercot.com
//! registration wall); adder columns are written as `0.0` and the omission is
//! documented in the crate README per the provenance convention in
//! [`crate::types::Provenance`].

pub mod fetch;
pub mod parse;
pub mod writers;

pub use fetch::{download_document, find_year_document, http_agent, list_documents, MisDocument};
pub use parse::{
    parse_as_report, parse_spp_report, ParseStats, ParsedReport, ReportFormat, ReportKind,
};
pub use writers::{
    partition_rel_path, read_manifest, upsert_manifest, write_as_partition, write_load_partition,
    write_price_partition, ManifestMeta, ALL_LOCATION,
};
