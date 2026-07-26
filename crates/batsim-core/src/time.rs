//! Virtual clock (spec B.1.1, B.1.2). Wall time is never read in engine
//! code; all timestamps derive from `t_sim`.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::CoreError;

/// The engine's virtual clock: seconds since the simulation epoch plus a
/// tick counter at fixed `dt`.
///
/// Invariants (spec B.1.2): `1 <= dt_s <= 60`; the epoch is 5-minute
/// aligned so settlement boundaries fall on `t_sim % 300 == 0`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimClock {
    epoch_s: u64,
    tick: u64,
    dt_s: u32,
}

impl SimClock {
    /// Create a clock at a unix-epoch (seconds) simulation epoch.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] when `dt_s` is outside `1..=60` or the
    /// epoch is not 5-minute aligned (spec B.1.2).
    pub fn new(epoch_s: u64, dt_s: u32) -> Result<Self, CoreError> {
        if !(1..=60).contains(&dt_s) {
            return Err(CoreError::InvalidConfig(format!(
                "dt_s must be within 1..=60 s, got {dt_s}"
            )));
        }
        if epoch_s % 300 != 0 {
            return Err(CoreError::InvalidConfig(format!(
                "epoch must be 5-minute aligned, got {epoch_s}"
            )));
        }
        Ok(Self {
            epoch_s,
            tick: 0,
            dt_s,
        })
    }

    /// Create a clock from an RFC 3339 / ISO-8601 UTC epoch string
    /// (config boundary; parsed once, spec B.1.1).
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] on unparsable input, plus the
    /// [`SimClock::new`] invariants.
    pub fn from_rfc3339(epoch: &str, dt_s: u32) -> Result<Self, CoreError> {
        let parsed = OffsetDateTime::parse(epoch, &Rfc3339)
            .map_err(|e| CoreError::InvalidConfig(format!("invalid epoch `{epoch}`: {e}")))?;
        let unix = parsed.unix_timestamp();
        let epoch_s = u64::try_from(unix)
            .map_err(|_| CoreError::InvalidConfig(format!("epoch `{epoch}` is before 1970")))?;
        Self::new(epoch_s, dt_s)
    }

    /// Seconds since the simulation epoch of the current tick start.
    #[must_use]
    pub fn t_sim(&self) -> u64 {
        self.tick * u64::from(self.dt_s)
    }

    /// Unix-epoch seconds of the current tick (epoch + `t_sim`).
    #[must_use]
    pub fn unix_time(&self) -> u64 {
        self.epoch_s + self.t_sim()
    }

    /// Current tick index.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// Timestep in seconds.
    #[must_use]
    pub const fn dt_s(&self) -> u32 {
        self.dt_s
    }

    /// The configured epoch (unix seconds).
    #[must_use]
    pub const fn epoch_s(&self) -> u64 {
        self.epoch_s
    }

    /// Advance one tick.
    pub fn advance(&mut self) {
        self.tick += 1;
    }

    /// Current sim time formatted as RFC 3339 UTC (telemetry boundary).
    #[must_use]
    pub fn rfc3339(&self) -> String {
        let Ok(unix) = i64::try_from(self.unix_time()) else {
            return "<overflow>".to_owned();
        };
        OffsetDateTime::from_unix_timestamp(unix).map_or_else(
            |_| "<invalid>".to_owned(),
            |dt| {
                dt.format(&Rfc3339)
                    .unwrap_or_else(|_| "<invalid>".to_owned())
            },
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_dt_and_unaligned_epoch() {
        assert!(SimClock::new(0, 0).is_err());
        assert!(SimClock::new(0, 61).is_err());
        assert!(SimClock::new(301, 1).is_err());
        assert!(SimClock::new(300, 1).is_ok());
    }

    #[test]
    fn advances_virtual_time() {
        let mut clock = SimClock::from_rfc3339("2025-08-15T00:00:00Z", 1).unwrap();
        assert_eq!(clock.t_sim(), 0);
        assert_eq!(clock.unix_time() % 300, 0);
        for _ in 0..3600 {
            clock.advance();
        }
        assert_eq!(clock.t_sim(), 3600);
        assert_eq!(clock.rfc3339(), "2025-08-15T01:00:00Z");
    }
}
