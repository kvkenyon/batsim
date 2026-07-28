//! batsim-ercot: ERCOT market integration (spec Part D).
//!
//! - [`types`] / [`source`]: normalized market data + the `PriceSource` trait.
//! - [`replay`]: Parquet-archive replay source (primary backtesting mode).
//! - [`synthetic`]: seeded regime-switching stress generator (spec D.4).
//! - [`ingest`]: ERCOT MIS -> Parquet normalization pipeline (spec D.3.3).
//! - [`settlement`]: per-interval P&L engine (spec D.5).
//! - [`baseline`]: pluggable counterfactual-load baselines (spec D.2).
//! - [`as_market`]: AS products, duration derate, performance scoring.
//! - [`four_cp`]: 4CP watch + savings attribution (spec D.1.4).
//! - [`rules`]: versioned ERCOT constants (spec D.8).
//! - [`cpt`]: Central Prevailing Time / DST handling for ERCOT reports.
//!
//! Scope rule: ERCOT only. No other ISO is modeled, abstracted for, or
//! stubbed. No network I/O on any simulation path; replay data is local.

pub mod as_market;
pub mod baseline;
pub mod cpt;
pub mod error;
pub mod four_cp;
pub mod ingest;
pub mod replay;
pub mod rules;
pub mod schema;
pub mod settlement;
pub mod source;
pub mod synthetic;
pub mod types;

pub use error::{ErcotError, Result};
pub use source::PriceSource;
pub use types::{
    AsPrice, AsProduct, LoadZone, Location, PriceSample, Provenance, SystemSignal, TimeRange,
    TradingHub,
};
