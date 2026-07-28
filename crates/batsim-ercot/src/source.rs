//! `PriceSource` trait (spec D.3.2): one market-signal binding per run.

use futures_util::stream::BoxStream;

use crate::error::Result;
use crate::types::{AsPrice, Location, PriceSample, SystemSignal, TimeRange};

/// Market-signal source for a simulation run (spec D.3.2).
///
/// Implementations: [`crate::replay::Replay`] (primary, backtesting),
/// [`crate::synthetic::SyntheticPriceGenerator`] (stress scenarios).
/// A `Live` ERCOT adapter is intentionally not implemented in v1: the sim
/// loop never performs network I/O, and api.ercot.com requires registered
/// credentials. The trait surface supports it without change.
///
/// All methods are synchronous and pure-after-load: replay data is loaded
/// and indexed before the run starts; the tick loop reads from memory.
pub trait PriceSource: Send + Sync {
    /// DAM hourly SPPs for `[start, end)`.
    fn dam_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>>;
    /// Real-time SPPs at the native cadence of the source.
    fn rt_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>>;
    /// DAM AS clearing prices for capacity (hourly, per product).
    fn as_prices(&self, r: TimeRange) -> Result<Vec<AsPrice>>;
    /// System load / reserves / fuel mix (drives 4CP watch and emissions).
    fn system_signals(&self, r: TimeRange) -> Result<Vec<SystemSignal>>;
    /// Streaming view for live mode. Default: unsupported.
    ///
    /// # Errors
    /// Always errors for non-live sources.
    fn subscribe_rt(&self, _loc: &Location) -> Result<BoxStream<'static, PriceSample>> {
        Err(crate::error::ErcotError::Unsupported("source is not live"))
    }
}
