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
    /// Historical replay from a normalized ERCOT Parquet archive
    /// (`<data_dir>/ercot`, written by `batsim-ercot-ingest`).
    Replay {
        /// Inclusive date range (ISO 8601 dates, CPT operating days) to replay.
        date_range: Option<(String, String)>,
        /// Market (only the real-time market is modeled).
        market: Option<String>,
        /// Settlement point / hub (required; e.g. `LZ_HOUSTON`).
        settlement_point: Option<String>,
    },
}

/// Context needed to resolve data-backed price sources.
pub struct ResolveCtx<'a> {
    /// Server data directory (the ERCOT archive lives at `<root>/ercot`).
    pub data_root: &'a std::path::Path,
    /// Scenario time range (unix seconds); used when the spec carries no
    /// explicit `date_range`.
    pub range: Option<(u64, u64)>,
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
    /// ERCOT replay feed (RTM settlement prices from the Parquet archive).
    Replay(ReplayFeed),
}

/// A loaded ERCOT replay archive binding: the shared archive plus the
/// settlement point this scenario settles against.
#[derive(Clone)]
pub struct ReplayFeed {
    replay: std::sync::Arc<batsim_ercot::replay::Replay>,
    location: batsim_ercot::Location,
}

impl std::fmt::Debug for ReplayFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayFeed")
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

impl ReplayFeed {
    /// The settlement location.
    #[must_use]
    pub fn location(&self) -> &batsim_ercot::Location {
        &self.location
    }

    /// The shared replay archive (DAM / AS / system-signal queries).
    #[must_use]
    pub fn replay(&self) -> &batsim_ercot::replay::Replay {
        &self.replay
    }

    /// RTM cadence of the loaded data (defaults to 900 s when empty).
    #[must_use]
    pub fn interval_secs(&self) -> u32 {
        self.replay.interval_secs().unwrap_or(900)
    }

    /// Full price sample for the interval containing `unix_time_s`.
    #[must_use]
    pub fn sample_at(&self, unix_time_s: u64) -> Option<batsim_ercot::PriceSample> {
        let ts =
            time::OffsetDateTime::from_unix_timestamp(i64::try_from(unix_time_s).ok()?).ok()?;
        self.replay.rt_spp_at(&self.location, ts).cloned()
    }
}

impl PriceSource {
    /// Resolve a spec into a queryable feed.
    ///
    /// # Errors
    /// Returns a message when the spec carries invalid numbers, names a
    /// settlement point without data, or the replay archive lacks coverage.
    pub fn resolve(spec: &PriceSourceSpec, ctx: &ResolveCtx<'_>) -> Result<Self, String> {
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
            PriceSourceSpec::Replay {
                date_range,
                settlement_point,
                ..
            } => {
                let point = settlement_point.as_deref().ok_or_else(|| {
                    "replay requires an explicit settlement_point (e.g. LZ_HOUSTON)".to_owned()
                })?;
                let location = batsim_ercot::Location::from_settlement_point(point);
                let range = resolve_replay_range(date_range.as_ref(), ctx)?;
                let root = ctx.data_root.join("ercot");
                // RTM is required; DAM / AS / system signals are optional
                // and are loaded when the archive carries them.
                let signal_sets: &[&[&str]] = &[
                    &[
                        batsim_ercot::schema::SIGNAL_RTM_SPP,
                        batsim_ercot::schema::SIGNAL_DAM_SPP,
                        batsim_ercot::schema::SIGNAL_AS_MCPC,
                        batsim_ercot::schema::SIGNAL_SYSTEM_LOAD,
                    ],
                    &[
                        batsim_ercot::schema::SIGNAL_RTM_SPP,
                        batsim_ercot::schema::SIGNAL_DAM_SPP,
                        batsim_ercot::schema::SIGNAL_AS_MCPC,
                    ],
                    &[
                        batsim_ercot::schema::SIGNAL_RTM_SPP,
                        batsim_ercot::schema::SIGNAL_DAM_SPP,
                    ],
                    &[batsim_ercot::schema::SIGNAL_RTM_SPP],
                ];
                let mut last_err = String::new();
                let mut loaded = None;
                for signals in signal_sets {
                    match batsim_ercot::replay::Replay::load(&root, range, signals) {
                        Ok(r) => {
                            loaded = Some(r);
                            break;
                        }
                        Err(e @ batsim_ercot::ErcotError::DataNotFound { .. }) => {
                            last_err = e.to_string();
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
                let replay = loaded.ok_or(last_err)?;
                Ok(Self::Replay(ReplayFeed {
                    replay: std::sync::Arc::new(replay),
                    location,
                }))
            }
        }
    }

    /// The replay feed, when this source is replay-backed.
    #[must_use]
    pub const fn replay_feed(&self) -> Option<&ReplayFeed> {
        match self {
            Self::Replay(f) => Some(f),
            _ => None,
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
                    // Sine peaking at 16:00 local (UTC-5 CDT); trough
                    // at 04:00 local.
                    let day_s = 86_400.0;
                    let t = (unix_time_s as f64 - 5.0 * 3600.0).rem_euclid(day_s);
                    let phase = 2.0 * std::f64::consts::PI * (t / day_s - 16.0 / 24.0);
                    base + amplitude * libm_cos(phase)
                }
            },
            Self::Replay(feed) => feed
                .sample_at(unix_time_s)
                .map_or(0.0, |s| s.spp_usd_per_mwh()),
        }
    }
}

/// Cosine through the `libm` crate: identical results on every
/// target, so the price series cannot drift cross-platform.
fn libm_cos(x: f64) -> f64 {
    libm::cos(x)
}

/// Resolve the replay time range: explicit CPT `date_range` wins,
/// otherwise the scenario's own time range.
fn resolve_replay_range(
    date_range: Option<&(String, String)>,
    ctx: &ResolveCtx<'_>,
) -> Result<batsim_ercot::TimeRange, String> {
    let (start, end) = if let Some((first, last)) = date_range {
        let parse_day = |s: &str| -> Result<time::Date, String> {
            time::Date::parse(
                s,
                &time::macros::format_description!("[year]-[month]-[day]"),
            )
            .map_err(|e| format!("date_range `{s}`: {e}"))
        };
        let first = parse_day(first)?;
        let last = parse_day(last)?;
        let end_day = last
            .checked_add(time::Duration::days(1))
            .ok_or_else(|| "date_range end overflows".to_owned())?;
        // CPT civil midnight -> UTC: hour-ending 1 interval start is
        // local 00:00 (unambiguous under US Central DST rules).
        let start = batsim_ercot::cpt::cpt_interval_to_utc(first, 1, 1, 1, false)
            .map_err(|e| e.to_string())?;
        let end = batsim_ercot::cpt::cpt_interval_to_utc(end_day, 1, 1, 1, false)
            .map_err(|e| e.to_string())?;
        (start, end)
    } else {
        let (s, e) = ctx
            .range
            .ok_or_else(|| "replay requires date_range or a scenario time range".to_owned())?;
        let start = time::OffsetDateTime::from_unix_timestamp(
            i64::try_from(s).map_err(|_| "range start overflows i64".to_owned())?,
        )
        .map_err(|e| e.to_string())?;
        let end = time::OffsetDateTime::from_unix_timestamp(
            i64::try_from(e).map_err(|_| "range end overflows i64".to_owned())?,
        )
        .map_err(|e| e.to_string())?;
        (start, end)
    };
    batsim_ercot::TimeRange::new(start, end).map_err(|e| e.to_string())
}
