//! Versioned ERCOT rules configuration (spec D.8).
//!
//! Every ERCOT-specific constant lives in `config/ercot_rules.v<year>.toml`,
//! never as a bare literal in logic. The rules version is recorded in every
//! `SettlementReport` and every ingested Parquet manifest.

use serde::{Deserialize, Serialize};

use crate::error::{ErcotError, Result};
use crate::types::AsProduct;

/// Embedded rules document for the current protocol environment.
pub const RULES_V2025_TOML: &str = include_str!("../config/ercot_rules.v2025.toml");

/// Root rules document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErcotRules {
    /// Version/verification metadata.
    pub meta: Meta,
    /// Offer caps and emergency pricing.
    pub offer_caps: OfferCaps,
    /// Simplified ORDC parameters.
    pub ordc: Ordc,
    /// Settlement cadence policy.
    pub settlement: Settlement,
    /// 4CP policy.
    pub four_cp: FourCp,
    /// Ancillary-service product parameters.
    #[serde(rename = "as")]
    pub as_: std::collections::BTreeMap<String, AsRule>,
    /// AS performance/penalty parameters (the `[as.performance]` section).
    pub as_performance: AsPerformance,
    /// Emission factors.
    pub emissions: Emissions,
}

/// Metadata block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// Rules version string recorded in reports/manifests.
    pub protocol_version: String,
    /// Date the constants were verified against ERCOT publications.
    pub verification_date: String,
    /// Free-form verification notes.
    pub notes: String,
}

/// Offer caps (post-Uri defaults; Uri-era override kept separate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferCaps {
    /// High system-wide offer cap, $/MWh.
    pub hcap_usd_per_mwh: f64,
    /// Low system-wide offer cap, $/MWh.
    pub lcap_usd_per_mwh: f64,
    /// Cumulative hours at HCAP that trigger the emergency cap drop.
    pub emergency_hours_at_hcap: f64,
    /// Rolling window for the emergency trigger, hours.
    pub emergency_rolling_window_hours: f64,
    /// Cap in force Feb 2021 (Uri-era replays).
    pub winter_storm_uri_cap_usd_per_mwh: f64,
}

/// Simplified ORDC parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ordc {
    /// Reserves below this engage the adder, MW.
    pub threshold_mw: f64,
    /// Reserves at/below this drive the adder to ~VOLL, MW.
    pub floor_mw: f64,
    /// Value of lost load, $/MWh (adder asymptote).
    pub voll_usd_per_mwh: f64,
}

/// Settlement cadence policy (spec D.1.1: interval length is configuration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settlement {
    /// Default settlement interval, seconds.
    pub default_interval_secs: u32,
    /// Allowed settlement intervals, seconds.
    pub allowed_interval_secs: Vec<u32>,
}

/// 4CP policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FourCp {
    /// Coincident-peak months (June-September).
    pub months: Vec<u32>,
    /// Candidate threshold as fraction of season-to-date peak.
    pub candidate_window_pct_of_peak: f64,
    /// Share of the annual transmission tag each confirmed CP month carries.
    pub annual_allocation_per_cp: f64,
}

/// Per-product AS parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsRule {
    /// Scoring starts at `t0 + response_deadline_secs`.
    pub response_deadline_secs: u32,
    /// Required sustain duration, hours.
    pub sustain_hours: f64,
    /// Duration needed to sell full rated power, hours.
    pub full_duration_hours: f64,
    /// Whether aggregated residential DER may sell this product.
    pub available_to_ader: bool,
}

/// AS performance/penalty parameters live under `[as.performance]`; TOML
/// flattens it into the same map key space, so it is split out by key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AsPerformance {
    /// Delivered/instructed ratio below which the clawback applies.
    pub threshold: f64,
    /// Clawback multiplier applied to the shortfall revenue.
    pub clawback_multiplier: f64,
}

/// Emission factors (average-mix attribution, kg CO2/MWh by fuel name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Emissions {
    /// Per-fuel factors.
    pub kg_co2_per_mwh: std::collections::BTreeMap<String, f64>,
}

impl ErcotRules {
    /// Parse the embedded current-version rules.
    ///
    /// # Errors
    /// Fails if the embedded TOML is malformed (a build-time bug).
    pub fn current() -> Result<Self> {
        Self::from_toml(RULES_V2025_TOML)
    }

    /// Parse rules from TOML text.
    ///
    /// # Errors
    /// Returns `ErcotError::Parse` on malformed TOML or missing sections.
    pub fn from_toml(text: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            meta: Meta,
            offer_caps: OfferCaps,
            ordc: Ordc,
            settlement: Settlement,
            four_cp: FourCp,
            #[serde(rename = "as")]
            as_: std::collections::BTreeMap<String, toml::Value>,
            emissions: Emissions,
        }
        let raw: Raw = toml::from_str(text).map_err(|e| ErcotError::Parse {
            context: "ercot_rules.toml".to_string(),
            detail: e.to_string(),
        })?;
        let mut products = std::collections::BTreeMap::new();
        let mut performance = None;
        for (name, value) in raw.as_ {
            if name == "performance" {
                let perf: AsPerformance =
                    value.try_into().map_err(|e: toml::de::Error| ErcotError::Parse {
                        context: "ercot_rules.toml [as.performance]".to_string(),
                        detail: e.to_string(),
                    })?;
                performance = Some(perf);
                continue;
            }
            let rule: AsRule = value.try_into().map_err(|e: toml::de::Error| {
                ErcotError::Parse {
                    context: format!("ercot_rules.toml [as.{name}]"),
                    detail: e.to_string(),
                }
            })?;
            products.insert(name, rule);
        }
        Ok(Self {
            meta: raw.meta,
            offer_caps: raw.offer_caps,
            ordc: raw.ordc,
            settlement: raw.settlement,
            four_cp: raw.four_cp,
            as_: products,
            as_performance: performance.ok_or_else(|| ErcotError::Parse {
                context: "ercot_rules.toml".to_string(),
                detail: "missing [as.performance]".to_string(),
            })?,
            emissions: raw.emissions,
        })
    }

    /// Rule for one product.
    #[must_use]
    pub fn as_rule(&self, product: AsProduct) -> Option<&AsRule> {
        self.as_.get(product.dam_column())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn embedded_rules_parse() {
        let rules = ErcotRules::current().unwrap();
        assert_eq!(rules.meta.protocol_version, "v2025");
        assert!((rules.offer_caps.hcap_usd_per_mwh - 5000.0).abs() < f64::EPSILON);
        assert_eq!(rules.settlement.default_interval_secs, 900);
        let ecrs = rules.as_rule(AsProduct::Ecrs).unwrap();
        assert!(ecrs.available_to_ader);
        assert!((ecrs.full_duration_hours - 2.0).abs() < f64::EPSILON);
        let regup = rules.as_rule(AsProduct::RegUp).unwrap();
        assert!(!regup.available_to_ader);
        let perf = rules.as_performance;
        assert!((perf.threshold - 0.90).abs() < f64::EPSILON);
    }
}
