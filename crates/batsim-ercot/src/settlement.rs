//! Settlement & revenue simulation (spec D.5).
//!
//! Feed-style engine: the run loop records per-interval per-home net export
//! energy against RTM prices, AS awards/deployments, and 4CP candidate
//! flags; [`SettlementEngine::finish`] produces a deterministic,
//! JSON-serializable [`SettlementReport`] with one ledger row per interval,
//! per-home ledgers, and fleet rollups.
//!
//! Conventions:
//! - Net export is export-positive kWh per settlement interval, integrated
//!   by the Part B engine from 1-s device powers (spec D.7 — never snapshot
//!   power x duration).
//! - `wholesale_usd` is signed revenue at the effective SPP (negative =
//!   charging cost at SPP).
//! - `charging_cost_usd` is a non-negative cost magnitude at the retail
//!   rate, reported for transparency.
//! - Money is USD `f64`, unrounded until the report boundary; iteration
//!   order is fixed (`BTreeMap` for keyed maps, input `Vec`/slice order for
//!   feeds), so replaying a run is bit-identical.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::as_market;
use crate::cpt;
use crate::four_cp;
use crate::rules::ErcotRules;
use crate::types::{AsProduct, Location, PriceSample, Provenance};

/// Retail rate structure (scenario input, spec D.1.5 / D.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RetailRate {
    /// Flat $/kWh at all hours.
    Flat {
        /// Rate, USD per kWh.
        usd_per_kwh: f64,
    },
    /// Time-of-use windows matched on the CPT hour of the interval start;
    /// the first matching window wins and unmatched hours price at 0.
    Tou {
        /// Rate windows (should tile 0-24).
        windows: Vec<TouWindow>,
    },
    /// Griddy-style wholesale pass-through:
    /// `SPP($/kWh) x multiplier + adder_usd_per_kwh`.
    WholesalePassThrough {
        /// Multiplier applied to the effective SPP.
        multiplier: f64,
        /// Fixed adder, USD per kWh.
        adder_usd_per_kwh: f64,
    },
}

impl RetailRate {
    /// Retail $/kWh in force for an interval starting at `ts` (UTC) with
    /// effective SPP `spp_usd_per_mwh`.
    #[must_use]
    pub fn rate_at(&self, ts: OffsetDateTime, spp_usd_per_mwh: f64) -> f64 {
        match self {
            Self::Flat { usd_per_kwh } => *usd_per_kwh,
            Self::Tou { windows } => {
                let hour = cpt::utc_to_cpt(ts).hour();
                windows
                    .iter()
                    .find(|w| w.contains_hour(hour))
                    .map_or(0.0, |w| w.usd_per_kwh)
            }
            Self::WholesalePassThrough { multiplier, adder_usd_per_kwh } =>
                spp_usd_per_mwh / 1000.0 * multiplier + adder_usd_per_kwh,
        }
    }
}

/// One TOU rate window over CPT hours `[start_hour_cpt, end_hour_cpt)`,
/// wrapping midnight when `start_hour_cpt > end_hour_cpt`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TouWindow {
    /// First covered CPT hour (0-23).
    pub start_hour_cpt: u8,
    /// First uncovered CPT hour (1-24; wrap to early morning when less than
    /// or equal to `start_hour_cpt`).
    pub end_hour_cpt: u8,
    /// Rate, USD per kWh.
    pub usd_per_kwh: f64,
}

impl TouWindow {
    /// True when `hour` (CPT, 0-23) falls inside the window.
    #[must_use]
    pub const fn contains_hour(&self, hour: u8) -> bool {
        if self.start_hour_cpt <= self.end_hour_cpt {
            hour >= self.start_hour_cpt && hour < self.end_hour_cpt
        } else {
            hour >= self.start_hour_cpt || hour < self.end_hour_cpt
        }
    }
}

/// One settlement-ledger row per interval (spec D.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntervalSettlement {
    /// Interval start, UTC.
    pub ts: OffsetDateTime,
    /// Interval length, seconds.
    pub interval_secs: u32,
    /// Effective settlement price (LMP + ORDC/RDPA adders), USD per MWh.
    pub spp_usd_per_mwh: f64,
    /// Fleet net export, kWh (export positive).
    pub fleet_net_export_kwh: f64,
    /// Wholesale revenue at SPP, USD (negative = charging cost at SPP).
    pub wholesale_usd: f64,
    /// Retail avoided cost from discharge, USD (non-negative).
    pub retail_avoided_cost_usd: f64,
    /// Retail charging cost, USD (non-negative cost magnitude).
    pub charging_cost_usd: f64,
    /// Gross AS award revenue allocated to this interval by product
    /// (before performance adjustment; populated by
    /// [`SettlementEngine::finish`]).
    pub as_revenue_usd: BTreeMap<AsProduct, f64>,
    /// True when the interval was flagged a 4CP candidate.
    pub four_cp_candidate: bool,
}

/// Per-home run totals (spec D.5.1 per-home ledger lines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HomeLedger {
    /// Home identifier (as fed to [`SettlementEngine::record_interval`]).
    pub home_id: String,
    /// Net export over the run, kWh (export positive; may be negative).
    pub export_kwh: f64,
    /// Wholesale revenue at SPP, USD (signed).
    pub wholesale_usd: f64,
    /// Retail avoided cost from discharge, USD.
    pub retail_avoided_usd: f64,
    /// Sum of this home's net-load reduction (kW) over flagged 4CP
    /// candidate intervals.
    pub four_cp_contribution_kw: f64,
    /// Incentive/rebate payment attributed to this home, USD.
    pub incentive_usd: f64,
}

/// Energy-margin rollup (spec D.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnergyTotals {
    /// Wholesale revenue at SPP, USD (signed).
    pub wholesale_usd: f64,
    /// Retail avoided cost from discharge, USD.
    pub retail_avoided_cost_usd: f64,
    /// Retail charging cost, USD (non-negative cost magnitude; NOT part of
    /// `retailer_margin_usd` — see [`SettlementEngine`] docs).
    pub charging_cost_usd: f64,
}

/// Per-product AS rollup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsTotals {
    /// Awarded energy, MWh (sum of `awarded_mw x award hours`).
    pub awarded_mwh: f64,
    /// Energy-weighted average clearing price, USD per MW
    /// (`gross_usd / awarded_mwh`; 0 when nothing was awarded).
    pub mcpc_avg_usd_per_mw: f64,
    /// Gross award revenue before performance adjustment, USD.
    pub gross_usd: f64,
    /// Aggregate delivered / instructed factor in `[0, 1]` (1.0 when no
    /// deployment was instructed).
    pub performance_factor: f64,
    /// Net AS revenue after the shortfall clawback, USD.
    pub net_usd: f64,
}

/// 4CP confirmation state (spec D.1.4: candidates confirm retroactively
/// once the season's actual peaks are known).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FourCpStatus {
    /// Candidates flagged; season peaks not yet known.
    CandidateUnconfirmed,
    /// Candidates confirmed against the season's actual peaks.
    Confirmed,
}

/// 4CP rollup (spec D.1.4 / D.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FourCpTotals {
    /// Number of distinct flagged candidate intervals.
    pub candidate_intervals_hit: u32,
    /// Sum of fleet net-load reduction over candidate intervals, kW.
    pub candidate_reduction_kw: f64,
    /// Estimated annual transmission-tag savings, USD (formula in
    /// [`four_cp::attribute_savings`]).
    pub est_annual_savings_usd: f64,
    /// Transmission rate used, USD per kW-month.
    pub transmission_rate_usd_per_kw_mo: f64,
    /// Confirmation state.
    pub status: FourCpStatus,
}

/// Fleet rollups (spec D.5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportTotals {
    /// Energy-margin rollup.
    pub energy: EnergyTotals,
    /// AS rollup by product.
    #[serde(rename = "as")]
    pub as_: BTreeMap<AsProduct, AsTotals>,
    /// 4CP rollup.
    pub four_cp: FourCpTotals,
    /// Retailer margin delta vs no-fleet, USD (formula documented on
    /// [`SettlementEngine`]).
    pub retailer_margin_usd: f64,
    /// Run-level emissions delta vs the no-dispatch counterfactual, kg CO2
    /// (`None` when no emissions feed was recorded; average-mix attribution
    /// is a simplification, spec D.5.1).
    pub emissions_kgco2_delta: Option<f64>,
}

/// The settlement report: per-interval ledger rows, per-home ledgers, and
/// fleet rollups (spec D.5.1). Field order is the serialization order;
/// keyed maps are `BTreeMap`s, so JSON output is deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettlementReport {
    /// Run identifier (from [`SettlementEngine::finish`]).
    pub run_id: String,
    /// Rules version that produced this report (spec D.8 auditability).
    pub rules_version: String,
    /// Baseline methodology label (spec D.2 auditability).
    pub baseline_method: String,
    /// Settlement cadence, seconds (configuration, spec D.1.1).
    pub settlement_interval_secs: u32,
    /// Settlement location.
    pub location: Location,
    /// Per-interval ledger rows, in record order.
    pub intervals: Vec<IntervalSettlement>,
    /// Per-home ledgers, sorted by home id.
    pub homes: Vec<HomeLedger>,
    /// Fleet rollups.
    pub totals: ReportTotals,
    /// Report provenance.
    pub provenance: Provenance,
}

/// Engine configuration (scenario inputs).
#[derive(Debug, Clone)]
pub struct SettlementConfig {
    /// Settlement location the fleet settles against.
    pub location: Location,
    /// Settlement cadence, seconds (spec D.1.1: configuration, not a
    /// constant).
    pub settlement_interval_secs: u32,
    /// Retail rate structure.
    pub retail_rate: RetailRate,
    /// Versioned ERCOT rules.
    pub rules: ErcotRules,
    /// Baseline methodology label recorded in the report
    /// (`BaselineMethod::label()` produces the canonical form).
    pub baseline_method_label: String,
    /// Transmission rate for 4CP savings, USD per kW-month.
    pub transmission_rate_usd_per_kw_mo: f64,
    /// Fleet program costs over the run, USD.
    pub program_costs_usd: f64,
    /// Per-home incentive/rebate payments, USD.
    pub incentives_usd: BTreeMap<String, f64>,
    /// Report provenance.
    pub provenance: Provenance,
}

/// Running per-home accumulators.
#[derive(Debug, Default)]
struct HomeAccum {
    export_kwh: f64,
    wholesale_usd: f64,
    retail_avoided_usd: f64,
    charging_cost_usd: f64,
    four_cp_contribution_kw: f64,
}

/// One recorded AS award (hourly MW at an hourly clearing price, spec
/// D.1.3; `hours` lets one call cover a multi-hour block at one MCPC).
#[derive(Debug)]
struct AsAward {
    product: AsProduct,
    ts: OffsetDateTime,
    awarded_mw: f64,
    hours: f64,
    mcpc_usd_per_mw: f64,
}

/// One recorded AS deployment (performance evidence).
#[derive(Debug)]
struct AsDeployment {
    product: AsProduct,
    start: OffsetDateTime,
    end: OffsetDateTime,
    instructed_mw: f64,
    delivered_mwh: f64,
    mcpc_avg_usd_per_mw: f64,
}

/// One flagged 4CP candidate interval.
#[derive(Debug)]
struct FourCpFlag {
    ts: OffsetDateTime,
    fleet_reduction_kw: f64,
    per_home: Vec<(String, f64)>,
}

/// Feed-style settlement engine (spec D.5). Record inputs as the run
/// proceeds (feed order is free: awards, deployments, and 4CP flags are
/// reconciled against interval rows at [`SettlementEngine::finish`]), then
/// finish once.
///
/// # Retailer margin (documented simplification, spec D.5.1)
///
/// ```text
/// retailer_margin_usd = retail_avoided_cost_usd
///                     + wholesale_usd         (signed; nets charging at SPP)
///                     + as_net_usd            (after shortfall clawback)
///                     + four_cp_savings_usd
///                     - program_costs_usd
///                     - incentives_usd
/// ```
///
/// This is the fleet-attributable margin DELTA vs a no-fleet
/// counterfactual — the quantity a dispatch-strategy harness optimizes.
/// `charging_cost_usd` (the retail-rate cost of charging energy) is
/// reported for transparency but is NOT subtracted here: charging energy is
/// already costed at SPP inside `wholesale_usd`, and the corresponding
/// retail billing is customer-side income outside this delta metric.
#[derive(Debug)]
pub struct SettlementEngine {
    config: SettlementConfig,
    intervals: Vec<IntervalSettlement>,
    /// Epoch-second -> index of the LAST interval row recorded for that ts.
    interval_by_ts: BTreeMap<i64, usize>,
    homes: BTreeMap<String, HomeAccum>,
    awards: Vec<AsAward>,
    deployments: Vec<AsDeployment>,
    /// Keyed by epoch second; a later flag for the same ts replaces the
    /// earlier one.
    four_cp_flags: BTreeMap<i64, FourCpFlag>,
    four_cp_confirmed: bool,
    emissions_delta_kgco2: Option<f64>,
}

impl SettlementEngine {
    /// New engine from scenario configuration.
    #[must_use]
    pub fn new(config: SettlementConfig) -> Self {
        Self {
            config,
            intervals: Vec::new(),
            interval_by_ts: BTreeMap::new(),
            homes: BTreeMap::new(),
            awards: Vec::new(),
            deployments: Vec::new(),
            four_cp_flags: BTreeMap::new(),
            four_cp_confirmed: false,
            emissions_delta_kgco2: None,
        }
    }

    /// The most recently recorded interval row (for streaming).
    #[must_use]
    pub fn last_interval(&self) -> Option<&IntervalSettlement> {
        self.intervals.last()
    }

    /// Record one settlement interval: per-home net export kWh (export
    /// positive) against the interval's RTM price sample. Retail avoided
    /// cost and charging cost are computed per home (a home's net export
    /// offsets retail purchases; its net import is charged at the retail
    /// rate) and summed.
    pub fn record_interval(
        &mut self,
        ts: OffsetDateTime,
        per_home: &[(&str, f64)],
        price: &PriceSample,
    ) {
        let spp = price.spp_usd_per_mwh();
        let rate = self.config.retail_rate.rate_at(ts, spp);
        let mut fleet_net_export_kwh = 0.0;
        let mut wholesale_usd = 0.0;
        let mut retail_avoided_cost_usd = 0.0;
        let mut charging_cost_usd = 0.0;
        for &(home_id, net_export_kwh) in per_home {
            let home_wholesale = spp * net_export_kwh / 1000.0;
            let (avoided, charging) = if net_export_kwh >= 0.0 {
                (rate * net_export_kwh, 0.0)
            } else {
                (0.0, rate * -net_export_kwh)
            };
            let accum = self.homes.entry(home_id.to_string()).or_default();
            accum.export_kwh += net_export_kwh;
            accum.wholesale_usd += home_wholesale;
            accum.retail_avoided_usd += avoided;
            accum.charging_cost_usd += charging;
            fleet_net_export_kwh += net_export_kwh;
            wholesale_usd += home_wholesale;
            retail_avoided_cost_usd += avoided;
            charging_cost_usd += charging;
        }
        self.interval_by_ts.insert(ts.unix_timestamp(), self.intervals.len());
        self.intervals.push(IntervalSettlement {
            ts,
            interval_secs: price.interval_secs,
            spp_usd_per_mwh: spp,
            fleet_net_export_kwh,
            wholesale_usd,
            retail_avoided_cost_usd,
            charging_cost_usd,
            as_revenue_usd: BTreeMap::new(),
            four_cp_candidate: false,
        });
    }

    /// Record an AS award: `awarded_mw` for `hours` hours starting at `ts`
    /// at the given clearing price. Awards are scenario inputs (the
    /// simulator does not clear the DAM, spec D.1.3); the engine is a
    /// ledger and records them as given, including for products not
    /// available to ADERs.
    pub fn record_as_award(
        &mut self,
        product: AsProduct,
        ts: OffsetDateTime,
        awarded_mw: f64,
        hours: f64,
        mcpc_usd_per_mw: f64,
    ) {
        self.awards.push(AsAward { product, ts, awarded_mw, hours, mcpc_usd_per_mw });
    }

    /// Record an AS deployment's performance evidence: `instructed_mw` over
    /// `[start, end)` and the delivered energy integrated by the Part B
    /// engine. Deployments drive the performance factor of their product.
    /// When a product has NO recorded awards, its deployments self-settle:
    /// instructed energy is treated as awarded energy at `mcpc_avg`
    /// (documented simplification for replay loops that script deployments
    /// without explicit DAM awards).
    #[allow(clippy::too_many_arguments)]
    pub fn record_as_deployment(
        &mut self,
        product: AsProduct,
        start: OffsetDateTime,
        end: OffsetDateTime,
        instructed_mw: f64,
        delivered_mwh: f64,
        mcpc_avg_usd_per_mw: f64,
    ) {
        self.deployments.push(AsDeployment {
            product,
            start,
            end,
            instructed_mw,
            delivered_mwh,
            mcpc_avg_usd_per_mw,
        });
    }

    /// Flag a 4CP candidate interval with the fleet's net-load reduction
    /// (kW) and its per-home attribution. Flagging the same `ts` again
    /// replaces the earlier flag.
    pub fn flag_4cp_candidate(
        &mut self,
        ts: OffsetDateTime,
        fleet_reduction_kw: f64,
        per_home: &[(&str, f64)],
    ) {
        self.four_cp_flags.insert(
            ts.unix_timestamp(),
            FourCpFlag {
                ts,
                fleet_reduction_kw,
                per_home: per_home.iter().map(|&(id, kw)| (id.to_string(), kw)).collect(),
            },
        );
    }

    /// Retro-confirm all flagged candidates against the season's actual
    /// peaks (spec D.1.4).
    pub fn confirm_4cp(&mut self) {
        self.four_cp_confirmed = true;
    }

    /// Record an emissions delta contribution (kg CO2 vs the no-dispatch
    /// counterfactual; average-mix attribution, spec D.5.1). The report
    /// carries the sum, or `None` when never called.
    pub fn record_emissions_delta_kgco2(&mut self, kg_co2: f64) {
        self.emissions_delta_kgco2 = Some(self.emissions_delta_kgco2.unwrap_or(0.0) + kg_co2);
    }

    /// Close the run and produce the settlement report.
    #[must_use]
    pub fn finish(self, run_id: String) -> SettlementReport {
        let Self {
            config,
            mut intervals,
            interval_by_ts,
            mut homes,
            awards,
            deployments,
            four_cp_flags,
            four_cp_confirmed,
            emissions_delta_kgco2,
        } = self;

        allocate_as_revenue(&mut intervals, &awards);
        fold_four_cp(&four_cp_flags, &interval_by_ts, &mut intervals, &mut homes);
        for home_id in config.incentives_usd.keys() {
            homes.entry(home_id.clone()).or_default();
        }
        let home_ledgers = build_home_ledgers(homes, &config.incentives_usd);

        let mut energy = EnergyTotals {
            wholesale_usd: 0.0,
            retail_avoided_cost_usd: 0.0,
            charging_cost_usd: 0.0,
        };
        for row in &intervals {
            energy.wholesale_usd += row.wholesale_usd;
            energy.retail_avoided_cost_usd += row.retail_avoided_cost_usd;
            energy.charging_cost_usd += row.charging_cost_usd;
        }

        let (as_totals, as_net_usd) = as_rollup(&awards, &deployments, &config.rules);

        let confirmed_pairs: Vec<(OffsetDateTime, f64)> =
            four_cp_flags.values().map(|f| (f.ts, f.fleet_reduction_kw)).collect();
        let four_cp_totals = FourCpTotals {
            candidate_intervals_hit: four_cp_flags.len() as u32,
            candidate_reduction_kw: confirmed_pairs.iter().map(|(_, kw)| kw).sum(),
            est_annual_savings_usd: four_cp::attribute_savings(
                &confirmed_pairs,
                config.transmission_rate_usd_per_kw_mo,
                &config.rules,
            ),
            transmission_rate_usd_per_kw_mo: config.transmission_rate_usd_per_kw_mo,
            status: if four_cp_confirmed {
                FourCpStatus::Confirmed
            } else {
                FourCpStatus::CandidateUnconfirmed
            },
        };

        // Fleet-attributable margin delta vs no-fleet (see struct docs).
        let incentives_total: f64 = config.incentives_usd.values().sum();
        let retailer_margin_usd = energy.retail_avoided_cost_usd
            + energy.wholesale_usd
            + as_net_usd
            + four_cp_totals.est_annual_savings_usd
            - config.program_costs_usd
            - incentives_total;

        SettlementReport {
            run_id,
            rules_version: config.rules.meta.protocol_version.clone(),
            baseline_method: config.baseline_method_label.clone(),
            settlement_interval_secs: config.settlement_interval_secs,
            location: config.location.clone(),
            intervals,
            homes: home_ledgers,
            totals: ReportTotals {
                energy,
                as_: as_totals,
                four_cp: four_cp_totals,
                retailer_margin_usd,
                emissions_kgco2_delta: emissions_delta_kgco2,
            },
            provenance: config.provenance,
        }
    }
}

/// Allocate gross AS award revenue to the interval rows each award covers,
/// proportional to interval length (before performance adjustment; the
/// performance factor is computed over whole deployments and applied only
/// in the report totals).
fn allocate_as_revenue(intervals: &mut [IntervalSettlement], awards: &[AsAward]) {
    for award in awards {
        let window_secs = award.hours * 3600.0;
        for row in intervals.iter_mut() {
            let offset = (row.ts - award.ts).as_seconds_f64();
            if offset >= 0.0 && offset < window_secs {
                let alloc =
                    award.awarded_mw * award.mcpc_usd_per_mw * (f64::from(row.interval_secs) / 3600.0);
                *row.as_revenue_usd.entry(award.product).or_insert(0.0) += alloc;
            }
        }
    }
}

/// Mark candidate interval rows and fold per-home 4CP contributions into
/// the home accumulators (creating ledger rows for homes seen only in 4CP
/// flags).
fn fold_four_cp(
    flags: &BTreeMap<i64, FourCpFlag>,
    interval_by_ts: &BTreeMap<i64, usize>,
    intervals: &mut [IntervalSettlement],
    homes: &mut BTreeMap<String, HomeAccum>,
) {
    for flag in flags.values() {
        if let Some(&idx) = interval_by_ts.get(&flag.ts.unix_timestamp()) {
            intervals[idx].four_cp_candidate = true;
        }
        for (home_id, kw) in &flag.per_home {
            homes.entry(home_id.clone()).or_default().four_cp_contribution_kw += kw;
        }
    }
}

/// Build per-home ledgers sorted by home id (`BTreeMap` order).
fn build_home_ledgers(
    homes: BTreeMap<String, HomeAccum>,
    incentives_usd: &BTreeMap<String, f64>,
) -> Vec<HomeLedger> {
    homes
        .into_iter()
        .map(|(home_id, accum)| HomeLedger {
            incentive_usd: incentives_usd.get(&home_id).copied().unwrap_or(0.0),
            home_id,
            export_kwh: accum.export_kwh,
            wholesale_usd: accum.wholesale_usd,
            retail_avoided_usd: accum.retail_avoided_usd,
            four_cp_contribution_kw: accum.four_cp_contribution_kw,
        })
        .collect()
}

/// Per-product AS accumulators.
#[derive(Debug, Default)]
struct AsAccum {
    has_award: bool,
    awarded_mwh: f64,
    gross_usd: f64,
    instructed_mwh: f64,
    delivered_mwh: f64,
    /// `instructed_mwh x mcpc_avg` over deployments; used only when the
    /// product has no recorded awards (self-settling deployments).
    deployment_gross_usd: f64,
}

/// AS rollup: awards set gross revenue; deployments set the performance
/// factor. Returns the per-product totals and the fleet net AS revenue.
fn as_rollup(
    awards: &[AsAward],
    deployments: &[AsDeployment],
    rules: &ErcotRules,
) -> (BTreeMap<AsProduct, AsTotals>, f64) {
    let mut accums: BTreeMap<AsProduct, AsAccum> = BTreeMap::new();
    for award in awards {
        let accum = accums.entry(award.product).or_default();
        accum.has_award = true;
        accum.awarded_mwh += award.awarded_mw * award.hours;
        accum.gross_usd += award.awarded_mw * award.mcpc_usd_per_mw * award.hours;
    }
    for deployment in deployments {
        let accum = accums.entry(deployment.product).or_default();
        let instructed_mwh =
            deployment.instructed_mw * (deployment.end - deployment.start).as_seconds_f64() / 3600.0;
        accum.instructed_mwh += instructed_mwh;
        accum.delivered_mwh += deployment.delivered_mwh;
        accum.deployment_gross_usd += instructed_mwh * deployment.mcpc_avg_usd_per_mw;
    }
    let mut totals = BTreeMap::new();
    let mut net_total = 0.0;
    for (product, accum) in &accums {
        let (awarded_mwh, gross_usd) = if accum.has_award {
            (accum.awarded_mwh, accum.gross_usd)
        } else {
            (accum.instructed_mwh, accum.deployment_gross_usd)
        };
        let perf = as_market::performance_factor(
            accum.delivered_mwh,
            accum.instructed_mwh,
            &rules.as_performance,
        );
        let net_usd = as_market::net_from_gross(gross_usd, &perf);
        let mcpc_avg_usd_per_mw = if awarded_mwh > 0.0 { gross_usd / awarded_mwh } else { 0.0 };
        net_total += net_usd;
        totals.insert(
            *product,
            AsTotals {
                awarded_mwh,
                mcpc_avg_usd_per_mw,
                gross_usd,
                performance_factor: perf.factor,
                net_usd,
            },
        );
    }
    (totals, net_total)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use crate::types::LoadZone;

    fn price(ts: OffsetDateTime, spp: f64) -> PriceSample {
        PriceSample {
            ts,
            interval_secs: 900,
            location: Location::LoadZone(LoadZone::Houston),
            lmp_usd_per_mwh: spp,
            ordc_adder_usd_per_mwh: 0.0,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::Synthetic,
        }
    }

    fn config(retail_rate: RetailRate) -> SettlementConfig {
        SettlementConfig {
            location: Location::LoadZone(LoadZone::Houston),
            settlement_interval_secs: 900,
            retail_rate,
            rules: ErcotRules::current().unwrap(),
            baseline_method_label: "LastNDaysAverage{n:10, exclusion: event_days}".to_string(),
            transmission_rate_usd_per_kw_mo: 3.5,
            program_costs_usd: 0.0,
            incentives_usd: BTreeMap::new(),
            provenance: Provenance::SettlementFinal,
        }
    }

    fn flat() -> RetailRate {
        RetailRate::Flat { usd_per_kwh: 0.12 }
    }

    fn assert_close(actual: f64, expected: f64) {
        let tol = 1e-9 * expected.abs().max(1.0);
        assert!((actual - expected).abs() <= tol, "expected {expected}, got {actual}");
    }

    #[test]
    fn two_intervals_energy_ledger_hand_computed() {
        let mut engine = SettlementEngine::new(config(flat()));
        let ts1 = datetime!(2026-08-14 22:00:00 UTC);
        let ts2 = datetime!(2026-08-14 22:15:00 UTC);
        // Interval 1: home A exports 2 kWh at $100/MWh.
        engine.record_interval(ts1, &[("A", 2.0)], &price(ts1, 100.0));
        // Interval 2: home A imports 3 kWh at $40/MWh.
        engine.record_interval(ts2, &[("A", -3.0)], &price(ts2, 40.0));
        let report = engine.finish("r_energy".to_string());

        assert_eq!(report.intervals.len(), 2);
        let row1 = &report.intervals[0];
        assert_close(row1.fleet_net_export_kwh, 2.0);
        assert_close(row1.wholesale_usd, 0.2);
        assert_close(row1.retail_avoided_cost_usd, 0.24);
        assert_close(row1.charging_cost_usd, 0.0);
        let row2 = &report.intervals[1];
        assert_close(row2.fleet_net_export_kwh, -3.0);
        assert_close(row2.wholesale_usd, -0.12);
        assert_close(row2.retail_avoided_cost_usd, 0.0);
        assert_close(row2.charging_cost_usd, 0.36);

        // wholesale = 0.002 x 100 - 0.003 x 40 = 0.08; avoided 0.24;
        // charging 3 x 0.12 = 0.36.
        assert_close(report.totals.energy.wholesale_usd, 0.08);
        assert_close(report.totals.energy.retail_avoided_cost_usd, 0.24);
        assert_close(report.totals.energy.charging_cost_usd, 0.36);

        assert_eq!(report.homes.len(), 1);
        let home = &report.homes[0];
        assert_eq!(home.home_id, "A");
        assert_close(home.export_kwh, -1.0);
        assert_close(home.wholesale_usd, 0.08);
        assert_close(home.retail_avoided_usd, 0.24);

        // margin = 0.24 + 0.08 + 0 + 0 - 0 - 0.
        assert_close(report.totals.retailer_margin_usd, 0.32);
        assert_eq!(report.totals.emissions_kgco2_delta, None);
        assert_eq!(report.rules_version, "v2025");
        assert_eq!(report.baseline_method, "LastNDaysAverage{n:10, exclusion: event_days}");
        assert_eq!(report.settlement_interval_secs, 900);
        assert_eq!(report.location, Location::LoadZone(LoadZone::Houston));
        assert_eq!(report.provenance, Provenance::SettlementFinal);
    }

    #[test]
    fn as_award_with_performance_and_clawback_hand_computed() {
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        let end = datetime!(2026-08-15 02:00:00 UTC);

        // Nominal: 8 MW ECRS x 4 h x $184.20 = $5894.40 gross;
        // delivered 29.76 of 32 MWh -> factor 0.93 (>= 0.90, no clawback).
        let mut engine = SettlementEngine::new(config(flat()));
        engine.record_as_award(AsProduct::Ecrs, ts, 8.0, 4.0, 184.20);
        engine.record_as_deployment(AsProduct::Ecrs, ts, end, 8.0, 29.76, 184.20);
        let report = engine.finish("r_as".to_string());
        let totals = &report.totals.as_[&AsProduct::Ecrs];
        assert_close(totals.awarded_mwh, 32.0);
        assert_close(totals.mcpc_avg_usd_per_mw, 184.20);
        assert_close(totals.gross_usd, 5894.40);
        assert_close(totals.performance_factor, 0.93);
        assert_close(totals.net_usd, 5481.792);
        assert_close(report.totals.retailer_margin_usd, 5481.792);

        // Clawback: delivered 25.6 of 32 MWh -> factor 0.80 (< 0.90):
        // net = gross x (0.8 - 0.2 x 2.0) = gross x 0.4 = 2357.76.
        let mut engine = SettlementEngine::new(config(flat()));
        engine.record_as_award(AsProduct::Ecrs, ts, 8.0, 4.0, 184.20);
        engine.record_as_deployment(AsProduct::Ecrs, ts, end, 8.0, 25.6, 184.20);
        let report = engine.finish("r_as_claw".to_string());
        let totals = &report.totals.as_[&AsProduct::Ecrs];
        assert_close(totals.performance_factor, 0.80);
        assert_close(totals.net_usd, 2357.76);
    }

    #[test]
    fn as_revenue_allocated_to_covered_intervals() {
        let mut engine = SettlementEngine::new(config(flat()));
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        for k in 0..4_i64 {
            let tsk = ts + time::Duration::minutes(15 * k);
            engine.record_interval(tsk, &[("A", 0.0)], &price(tsk, 50.0));
        }
        // 1 h award covers all four 15-min rows: 8 x 184.20 x 0.25 = 368.4 each.
        engine.record_as_award(AsProduct::Ecrs, ts, 8.0, 1.0, 184.20);
        let report = engine.finish("r_alloc".to_string());
        for row in &report.intervals {
            assert_close(row.as_revenue_usd[&AsProduct::Ecrs], 368.4);
        }
    }

    #[test]
    fn four_cp_savings_status_and_per_home_hand_computed() {
        let mut engine = SettlementEngine::new(config(flat()));
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        engine.record_interval(ts, &[("A", 2.0), ("B", 1.82)], &price(ts, 100.0));
        engine.flag_4cp_candidate(ts, 38_200.0, &[("A", 20_000.0), ("B", 18_200.0)]);
        let report = engine.finish("r_4cp".to_string());

        let cp = &report.totals.four_cp;
        assert_eq!(cp.candidate_intervals_hit, 1);
        assert_close(cp.candidate_reduction_kw, 38_200.0);
        // Exact per spec D.6: 38200 x 3.5 x 12 x 0.25 = 401100.
        assert_eq!(cp.est_annual_savings_usd, 401_100.0);
        assert_close(cp.transmission_rate_usd_per_kw_mo, 3.5);
        assert_eq!(cp.status, FourCpStatus::CandidateUnconfirmed);
        assert!(report.intervals[0].four_cp_candidate);
        assert_close(report.homes[0].four_cp_contribution_kw, 20_000.0);
        assert_close(report.homes[1].four_cp_contribution_kw, 18_200.0);
        // margin = avoided 3.82 x 0.12 + wholesale 100 x 0.00382 + 401100.
        assert_close(report.totals.retailer_margin_usd, 401_100.840_4);

        // Retro-confirmation flips the status.
        let mut engine = SettlementEngine::new(config(flat()));
        engine.flag_4cp_candidate(ts, 38_200.0, &[("A", 38_200.0)]);
        engine.confirm_4cp();
        let report = engine.finish("r_4cp_conf".to_string());
        assert_eq!(report.totals.four_cp.status, FourCpStatus::Confirmed);
        assert_eq!(report.totals.four_cp.est_annual_savings_usd, 401_100.0);
    }

    #[test]
    fn tou_and_passthrough_rates() {
        // 2026-08-14 23:00 UTC = 18:00 CDT -> peak window price.
        let tou = RetailRate::Tou {
            windows: vec![
                TouWindow { start_hour_cpt: 16, end_hour_cpt: 21, usd_per_kwh: 0.30 },
                TouWindow { start_hour_cpt: 0, end_hour_cpt: 16, usd_per_kwh: 0.05 },
                TouWindow { start_hour_cpt: 21, end_hour_cpt: 24, usd_per_kwh: 0.05 },
            ],
        };
        let ts = datetime!(2026-08-14 23:00:00 UTC);
        let mut engine = SettlementEngine::new(config(tou));
        engine.record_interval(ts, &[("A", 1.0)], &price(ts, 100.0));
        let report = engine.finish("r_tou".to_string());
        assert_close(report.intervals[0].retail_avoided_cost_usd, 0.30);

        // Wrap-around window semantics.
        let night = TouWindow { start_hour_cpt: 22, end_hour_cpt: 6, usd_per_kwh: 0.1 };
        assert!(night.contains_hour(23));
        assert!(night.contains_hour(5));
        assert!(!night.contains_hour(12));
        assert!(!night.contains_hour(6));

        // Pass-through: 100 $/MWh -> 0.1 x 1.2 + 0.01 = 0.13 $/kWh.
        let pt = RetailRate::WholesalePassThrough { multiplier: 1.2, adder_usd_per_kwh: 0.01 };
        assert_close(pt.rate_at(ts, 100.0), 0.13);
        // Flat ignores ts/spp.
        assert_close(flat().rate_at(ts, 9000.0), 0.12);
    }

    #[test]
    fn margin_subtracts_program_costs_and_incentives() {
        let mut cfg = config(flat());
        cfg.program_costs_usd = 100.0;
        cfg.incentives_usd.insert("A".to_string(), 25.0);
        let mut engine = SettlementEngine::new(cfg);
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        engine.record_interval(ts, &[("A", 2.0)], &price(ts, 100.0));
        let report = engine.finish("r_margin".to_string());
        // margin = 0.24 + 0.2 - 100 - 25 = -124.56.
        assert_close(report.totals.retailer_margin_usd, -124.56);
        assert_close(report.homes[0].incentive_usd, 25.0);
    }

    #[test]
    fn emissions_delta_accumulates_when_fed() {
        let mut engine = SettlementEngine::new(config(flat()));
        engine.record_emissions_delta_kgco2(-100.0);
        engine.record_emissions_delta_kgco2(-50.5);
        let report = engine.finish("r_em".to_string());
        assert_eq!(report.totals.emissions_kgco2_delta, Some(-150.5));
    }

    #[test]
    fn report_serde_round_trip_uses_as_key() {
        let mut engine = SettlementEngine::new(config(flat()));
        let ts = datetime!(2026-08-14 22:00:00 UTC);
        engine.record_interval(ts, &[("A", 2.0)], &price(ts, 100.0));
        engine.record_as_award(AsProduct::Ecrs, ts, 8.0, 1.0, 184.20);
        let report = engine.finish("r_serde".to_string());
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"as\":{"));
        assert!(json.contains("\"candidate_unconfirmed\""));
        let back: SettlementReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn retail_rate_serde_rejects_unknown_fields() {
        let bad = r#"{"flat":{"usd_per_kwh":0.12,"surprise":1}}"#;
        assert!(serde_json::from_str::<RetailRate>(bad).is_err());
        let good = r#"{"wholesale_pass_through":{"multiplier":1.2,"adder_usd_per_kwh":0.01}}"#;
        let parsed = serde_json::from_str::<RetailRate>(good).unwrap();
        assert_eq!(
            parsed,
            RetailRate::WholesalePassThrough { multiplier: 1.2, adder_usd_per_kwh: 0.01 }
        );
    }
}
