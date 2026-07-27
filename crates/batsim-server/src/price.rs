//! Real-time price feed for the simulation clock.
//!
//! Two deterministic sources ship now: a flat static price and a seeded
//! synthetic diurnal profile. Historical replay plugs in behind the same
//! [`PriceSource`] interface without touching the engine or the API.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Where settlement prices come from for a scenario.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PriceSourceSpec {
    /// A single flat price for the whole run.
    Static {
        /// Price in $/MWh applied at every tick.
        price_per_mwh: f64,
    },
    /// A deterministic synthetic profile.
    Synthetic {
        /// Profile shape.
        profile: SyntheticProfile,
        /// Mean price in $/MWh.
        #[serde(default = "default_base")]
        base_price_per_mwh: f64,
        /// Peak-to-mean swing amplitude in $/MWh.
        #[serde(default = "default_amplitude")]
        amplitude_per_mwh: f64,
        /// Seed reserved for stochastic profile extensions.
        #[serde(default)]
        seed: u64,
    },
    /// Historical replay. Not available yet; activating a scenario with
    /// this source fails validation until replay data support lands.
    Replay {
        /// Inclusive date range (ISO 8601 dates) to replay.
        date_range: Option<(String, String)>,
        /// Market (only the real-time market is modeled).
        market: Option<String>,
        /// Settlement point / hub.
        settlement_point: Option<String>,
    },
}

const fn default_base() -> f64 {
    45.0
}

const fn default_amplitude() -> f64 {
    25.0
}

/// Synthetic price profile shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyntheticProfile {
    /// Flat at the base price.
    Flat,
    /// Diurnal wave peaking in the late afternoon (Texas summer peak).
    SummerPeak,
}

/// A resolved, tick-queryable price feed.
#[derive(Debug, Clone)]
pub enum PriceSource {
    /// Flat price.
    Static(f64),
    /// Synthetic diurnal profile.
    Synthetic {
        /// Profile shape.
        profile: SyntheticProfile,
        /// Mean price $/MWh.
        base: f64,
        /// Swing amplitude $/MWh.
        amplitude: f64,
    },
}

impl PriceSource {
    /// Resolve a spec into a queryable feed.
    ///
    /// # Errors
    /// Returns a message when the spec names a source that is not
    /// available yet (replay) or carries invalid numbers.
    pub fn resolve(spec: &PriceSourceSpec) -> Result<Self, String> {
        match spec {
            PriceSourceSpec::Static { price_per_mwh } => {
                if price_per_mwh.is_finite() {
                    Ok(Self::Static(*price_per_mwh))
                } else {
                    Err("price_per_mwh must be finite".to_owned())
                }
            }
            PriceSourceSpec::Synthetic {
                profile,
                base_price_per_mwh,
                amplitude_per_mwh,
                ..
            } => {
                if !base_price_per_mwh.is_finite() || !amplitude_per_mwh.is_finite() {
                    return Err("synthetic price parameters must be finite".to_owned());
                }
                Ok(Self::Synthetic {
                    profile: *profile,
                    base: *base_price_per_mwh,
                    amplitude: *amplitude_per_mwh,
                })
            }
            PriceSourceSpec::Replay { .. } => {
                Err("price replay is not available yet; use `static` or `synthetic`".to_owned())
            }
        }
    }

    /// The default feed before any scenario binds one: flat $45/MWh.
    #[must_use]
    pub const fn default_feed() -> Self {
        Self::Static(45.0)
    }

    /// Price in $/MWh at a unix time. Pure and deterministic.
    #[must_use]
    pub fn price_at(&self, unix_time_s: u64) -> f64 {
        match self {
            Self::Static(p) => *p,
            Self::Synthetic {
                profile,
                base,
                amplitude,
            } => match profile {
                SyntheticProfile::Flat => *base,
                SyntheticProfile::SummerPeak => {
                    // Sine peaking at 16:00 local (UTC-5 CDT); trough in
                    // the early morning.
                    let day_s = 86_400.0;
                    let t = (unix_time_s as f64 + 5.0 * 3600.0) % day_s;
                    let phase = 2.0 * std::f64::consts::PI * (t / day_s - 16.0 / 24.0);
                    base - amplitude * libm_cos(phase)
                }
            },
        }
    }
}

/// Cosine via a deterministic polynomial-free path: platform libm cos is
/// fine here because prices never feed physics state, only telemetry.
fn libm_cos(x: f64) -> f64 {
    x.cos()
}
