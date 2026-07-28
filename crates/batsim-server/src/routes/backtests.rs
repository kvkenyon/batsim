//! Backtest endpoints (M3): replay one ERCOT operating day against an
//! expanded fleet, stream per-interval settlement over the telemetry
//! stream, and serve the final P&L report.
//!
//! A run internally creates + activates a scenario (replay price source),
//! configures the engine-side settlement tracker, then starts the sim at
//! the requested speed (default unbounded). The engine auto-stops and
//! settles at the end of the day.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::engine::{
    rfc3339_of, unix_of, AsAwardInput, BacktestConfig, BacktestInfo, BacktestState, EngineMsg,
    StrategyEntry,
};
use crate::ids;
use crate::model::{
    ActionSpec, AsProductSpec, BacktestDoc, BacktestRequest, BacktestsPage, PageParams,
    RetailRateSpec, ScenarioRequest, ScenarioTime, SimState, StrategySpec,
};
use crate::price::PriceSourceSpec;
use crate::problem::{ApiResult, Problem};
use crate::state::{AppState, BacktestEntry, ScenarioEntry};

use super::fleets::now_rfc3339;
use super::scenarios::activate_impl;
use super::{decode_cursor, encode_cursor, page_ids, ValidJson, ValidQuery};

/// Backtest routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_backtest).get(list_backtests))
        .route("/{id}", get(get_backtest))
        .route("/{id}/settlement", get(get_settlement))
}

/// Convert the request's retail rate spec into the ercot rate structure;
/// default is wholesale pass-through x1.0 (pure wholesale exposure).
fn retail_rate_of(req: &BacktestRequest) -> batsim_ercot::settlement::RetailRate {
    match &req.retail_rate {
        None => batsim_ercot::settlement::RetailRate::WholesalePassThrough {
            multiplier: 1.0,
            adder_usd_per_kwh: 0.0,
        },
        Some(RetailRateSpec::Flat { usd_per_kwh }) => {
            batsim_ercot::settlement::RetailRate::Flat {
                usd_per_kwh: *usd_per_kwh,
            }
        }
        Some(RetailRateSpec::Tou { windows }) => batsim_ercot::settlement::RetailRate::Tou {
            windows: windows
                .iter()
                .map(|w| batsim_ercot::settlement::TouWindow {
                    start_hour_cpt: w.start_hour_cpt,
                    end_hour_cpt: w.end_hour_cpt,
                    usd_per_kwh: w.usd_per_kwh,
                })
                .collect(),
        },
        Some(RetailRateSpec::WholesalePassThrough {
            multiplier,
            adder_usd_per_kwh,
        }) => batsim_ercot::settlement::RetailRate::WholesalePassThrough {
            multiplier: *multiplier,
            adder_usd_per_kwh: *adder_usd_per_kwh,
        },
    }
}

fn as_product_of(p: AsProductSpec) -> batsim_ercot::AsProduct {
    match p {
        AsProductSpec::Rrs => batsim_ercot::AsProduct::Rrs,
        AsProductSpec::Ecrs => batsim_ercot::AsProduct::Ecrs,
        AsProductSpec::NonSpin => batsim_ercot::AsProduct::NonSpin,
        AsProductSpec::RegUp => batsim_ercot::AsProduct::RegUp,
        AsProductSpec::RegDown => batsim_ercot::AsProduct::RegDown,
    }
}

/// CPT operating day -> `[start, end)` UTC unix range.
fn day_range(date: &str) -> ApiResult<(u64, u64)> {
    let day = time::Date::parse(
        date,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|e| Problem::validation(format!("date `{date}`: {e}")))?;
    let next_day = day
        .checked_add(time::Duration::days(1))
        .ok_or_else(|| Problem::validation("date overflows"))?;
    let to_unix = |d: time::Date| -> ApiResult<u64> {
        let ts = batsim_ercot::cpt::cpt_interval_to_utc(d, 1, 1, 1, false)
            .map_err(|e| Problem::validation(format!("date: {e}")))?;
        u64::try_from(ts.unix_timestamp()).map_err(|_| Problem::validation("date before 1970"))
    };
    Ok((to_unix(day)?, to_unix(next_day)?))
}

/// Validate and translate the strategy schedule.
fn parse_schedule(
    req: &BacktestRequest,
    start: u64,
    end: u64,
) -> ApiResult<Vec<StrategyEntry>> {
    let mut schedule = Vec::new();
    if let StrategySpec::Schedule { entries } = &req.strategy {
        for e in entries {
            let unix = unix_of(&e.start)
                .map_err(|err| Problem::validation(format!("strategy start: {err}")))?;
            if unix < start || unix >= end {
                return Err(Problem::validation(format!(
                    "strategy entry `{}` is outside the operating day",
                    e.start
                )));
            }
            if !e.kw.is_finite() || e.kw <= 0.0 {
                return Err(Problem::validation("strategy kw must be positive and finite"));
            }
            let spec = match e.action {
                crate::model::StrategyAction::Charge => ActionSpec::ChargeTo {
                    kw: e.kw,
                    duration_s: e.duration_s,
                },
                crate::model::StrategyAction::Discharge => ActionSpec::DischargeTo {
                    kw: e.kw,
                    duration_s: e.duration_s,
                },
            };
            schedule.push(StrategyEntry { unix, spec });
        }
        schedule.sort_by_key(|e| e.unix);
    }
    Ok(schedule)
}

/// Validate and translate AS awards.
fn parse_awards(req: &BacktestRequest) -> ApiResult<Vec<AsAwardInput>> {
    let mut out = Vec::new();
    for a in &req.as_awards {
        let start_unix = unix_of(&a.start)
            .map_err(|err| Problem::validation(format!("as award start: {err}")))?;
        let end_unix =
            unix_of(&a.end).map_err(|err| Problem::validation(format!("as award end: {err}")))?;
        if end_unix <= start_unix {
            return Err(Problem::validation("as award end must be after start"));
        }
        if !a.awarded_mw.is_finite() || a.awarded_mw <= 0.0 {
            return Err(Problem::validation("awarded_mw must be positive and finite"));
        }
        out.push(AsAwardInput {
            product: as_product_of(a.product),
            start_unix,
            end_unix,
            awarded_mw: a.awarded_mw,
            mcpc_usd_per_mw: a.mcpc_usd_per_mw,
            deployed: a.deployed,
        });
    }
    Ok(out)
}

/// Validate and translate the 4CP configuration (rate, candidates).
fn parse_four_cp(req: &BacktestRequest) -> ApiResult<(f64, Vec<u64>)> {
    let Some(fcp) = &req.four_cp else {
        return Ok((0.0, Vec::new()));
    };
    let rate = fcp.transmission_rate_usd_per_kw_mo;
    if !rate.is_finite() || rate < 0.0 {
        return Err(Problem::validation(
            "transmission_rate_usd_per_kw_mo must be non-negative and finite",
        ));
    }
    let mut candidates = Vec::new();
    for c in &fcp.candidate_intervals {
        candidates
            .push(unix_of(c).map_err(|err| Problem::validation(format!("4cp candidate: {err}")))?);
    }
    Ok((rate, candidates))
}

/// Start a backtest run.
#[utoipa::path(
    post,
    path = "/v1/backtests",
    request_body = BacktestRequest,
    responses(
        (status = 202, description = "Backtest run started", body = BacktestDoc),
        (status = 400, description = "Invalid request", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is not stopped", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Replay data unavailable or invalid strategy", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "backtests"
)]
pub async fn create_backtest(
    State(state): State<AppState>,
    ValidJson(req): ValidJson<BacktestRequest>,
) -> ApiResult<Response> {
    let fleet = state
        .fleet(&req.fleet_id)
        .ok_or_else(|| Problem::not_found("fleet", &req.fleet_id))?;
    if fleet.home_ids.is_empty() {
        return Err(Problem::unprocessable(format!(
            "fleet {} has no homes; expand it first",
            req.fleet_id
        )));
    }

    let (start, end) = day_range(&req.date)?;
    let schedule = parse_schedule(&req, start, end)?;
    let as_awards = parse_awards(&req)?;
    let (transmission_rate, four_cp_candidates) = parse_four_cp(&req)?;

    // Sim must be stopped: a run rebinds the world.
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state != SimState::Stopped {
        return Err(Problem::sim_running(
            "starting a backtest requires a stopped simulation",
        ));
    }

    let (run_id, scenario_id) = start_run(&state, &fleet, &req, start, end, schedule, as_awards, transmission_rate, four_cp_candidates).await?;

    let bt_entry = BacktestEntry {
        id: run_id.clone(),
        fleet_id: req.fleet_id.clone(),
        scenario_id,
        request: req,
        created_at: now_rfc3339(),
        report: None,
    };
    if let Ok(mut backtests) = state.backtests.write() {
        backtests.insert(run_id.clone(), bt_entry.clone());
    }
    let doc = doc_of(&bt_entry, Some(&BacktestInfo {
        run_id: run_id.clone(),
        state: crate::engine::BacktestState::Running,
        sim_time: rfc3339_of(start),
        intervals_settled: 0,
        report: None,
    }));
    Ok((StatusCode::ACCEPTED, Json(doc)).into_response())
}

/// Bind the scenario, configure settlement, and start the sim; returns
/// `(run_id, scenario_id)`.
#[allow(clippy::too_many_arguments)]
async fn start_run(
    state: &AppState,
    fleet: &crate::state::FleetEntry,
    req: &BacktestRequest,
    start: u64,
    end: u64,
    schedule: Vec<StrategyEntry>,
    as_awards: Vec<AsAwardInput>,
    transmission_rate: f64,
    four_cp_candidates: Vec<u64>,
) -> ApiResult<(String, String)> {
    // Create + activate the scenario (binds the replay price feed; this
    // validates archive coverage and the settlement point).
    let scenario_req = ScenarioRequest {
        name: req
            .name
            .clone()
            .unwrap_or_else(|| format!("backtest-{}", req.date)),
        time: ScenarioTime {
            start: rfc3339_of(start),
            end: rfc3339_of(end),
            tick_seconds: Some(1),
        },
        prices: PriceSourceSpec::Replay {
            date_range: Some((req.date.clone(), req.date.clone())),
            market: Some("RTM".to_owned()),
            settlement_point: Some(req.settlement_point.clone()),
        },
        weather: None,
        outages: Vec::new(),
        seed: req.seed,
    };
    let scenario_id = ids::new_id(ids::SCENARIO);
    let entry = ScenarioEntry {
        id: scenario_id.clone(),
        request: scenario_req,
        created_at: now_rfc3339(),
    };
    if let Ok(mut scenarios) = state.scenarios.write() {
        scenarios.insert(scenario_id.clone(), entry);
    }
    activate_impl(state.clone(), &scenario_id).await?;

    // Reset the fleet's homes to their pristine composed state at their
    // existing arena indices: every backtest starts from identical
    // physical conditions (same index => same RNG substreams, so runs are
    // bit-identical given identical inputs).
    let mut fresh = Vec::new();
    for home_id in &fleet.home_ids {
        let home = state
            .home(home_id)
            .ok_or_else(|| Problem::not_found("home", home_id))?;
        let composed = crate::compose::compose_home(
            &state.registry,
            &home.plan,
            &home.id,
            req.seed,
            home.idx,
        )?;
        fresh.push((home.idx, composed.home));
    }
    state
        .engine
        .call(|tx| EngineMsg::ResetHomes {
            homes: fresh,
            reply: tx,
        })
        .await?
        .map_err(|e| Problem::internal().detail(e))?;

    // Configure the engine-side settlement tracker.
    let run_id = ids::new_id(ids::BACKTEST);
    let location = batsim_ercot::Location::from_settlement_point(&req.settlement_point);
    let config = BacktestConfig {
        run_id: run_id.clone(),
        end_unix: end,
        interval_secs: 0, // auto from the replay cadence
        location,
        retail_rate: retail_rate_of(req),
        baseline_method_label: "MeteredBeforeAfter{pre_event_intervals:4, interval_secs:auto}"
            .to_owned(),
        transmission_rate_usd_per_kw_mo: transmission_rate,
        program_costs_usd: 0.0,
        incentives_usd: std::collections::BTreeMap::new(),
        provenance: batsim_ercot::Provenance::SettlementFinal,
        schedule,
        as_awards,
        four_cp_candidates,
    };
    state
        .engine
        .call(|tx| EngineMsg::ConfigureBacktest {
            config: Box::new(config),
            reply: tx,
        })
        .await?
        .map_err(Problem::unprocessable)?;

    // Go.
    let speed = req.speed.unwrap_or(0.0);
    state
        .engine
        .call(|tx| EngineMsg::SetSpeed {
            multiplier: speed,
            reply: tx,
        })
        .await?
        .map_err(|e| Problem::unprocessable(format!("speed: {e}")))?;
    state
        .engine
        .call(|tx| EngineMsg::Start { reply: tx })
        .await?
        .map_err(Problem::conflict)?;
    Ok((run_id, scenario_id))
}

fn doc_of(entry: &BacktestEntry, live: Option<&BacktestInfo>) -> BacktestDoc {
    let (state, sim_time, intervals) = match live {
        Some(info) => {
            let state = match &info.state {
                BacktestState::Running => "running".to_owned(),
                BacktestState::Settled => "settled".to_owned(),
                BacktestState::Failed(reason) => format!("failed: {reason}"),
            };
            (
                state,
                info.sim_time.clone(),
                info.intervals_settled as u64,
            )
        }
        None => (
            if entry.report.is_some() {
                "settled".to_owned()
            } else {
                "unknown (engine rebound since this run)".to_owned()
            },
            String::new(),
            0,
        ),
    };
    BacktestDoc {
        id: entry.id.clone(),
        fleet_id: entry.fleet_id.clone(),
        scenario_id: entry.scenario_id.clone(),
        state,
        sim_time,
        intervals_settled: intervals,
        created_at: entry.created_at.clone(),
        settlement_url: format!("/v1/backtests/{}/settlement", entry.id),
    }
}

/// Live status from the engine when `id` is the currently configured run.
async fn live_info(state: &AppState, id: &str) -> ApiResult<Option<BacktestInfo>> {
    let info = state
        .engine
        .call(|tx| EngineMsg::BacktestStatus { reply: tx })
        .await?;
    Ok(info.filter(|i| i.run_id == id))
}

/// Capture a settled report into the entry so it survives later rebinds.
fn capture_report(state: &AppState, id: &str, info: &BacktestInfo) {
    if let Some(report) = &info.report {
        if let Ok(json) = serde_json::to_value(report) {
            if let Ok(mut backtests) = state.backtests.write() {
                if let Some(entry) = backtests.get_mut(id) {
                    if entry.report.is_none() {
                        entry.report = Some(json);
                    }
                }
            }
        }
    }
}

/// List backtest runs.
#[utoipa::path(
    get,
    path = "/v1/backtests",
    params(PageParams),
    responses(
        (status = 200, description = "Page of backtest runs", body = BacktestsPage),
    ),
    tag = "backtests"
)]
pub async fn list_backtests(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<PageParams>,
) -> ApiResult<Json<BacktestsPage>> {
    let limit = q.limit()?;
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut ids: Vec<String> = state
        .backtests
        .read()
        .map_err(|_| Problem::internal())?
        .keys()
        .cloned()
        .collect();
    ids.sort();
    let (page, has_more) = page_ids(&ids, cursor.as_deref(), limit);
    let live = state
        .engine
        .call(|tx| EngineMsg::BacktestStatus { reply: tx })
        .await?;
    let data = page
        .iter()
        .filter_map(|id| state.backtest(id))
        .map(|e| doc_of(&e, live.as_ref().filter(|i| i.run_id == e.id)))
        .collect();
    let next_cursor = has_more.then(|| encode_cursor(page.last().map_or("", String::as_str)));
    Ok(Json(BacktestsPage {
        data,
        page: crate::model::PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

/// Get one backtest run.
#[utoipa::path(
    get,
    path = "/v1/backtests/{id}",
    params(("id" = String, Path, description = "Backtest run id")),
    responses(
        (status = 200, description = "Backtest run", body = BacktestDoc),
        (status = 404, description = "Unknown backtest run", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "backtests"
)]
pub async fn get_backtest(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<BacktestDoc>> {
    let entry = state
        .backtest(&id)
        .ok_or_else(|| Problem::not_found("backtest", &id))?;
    let live = live_info(&state, &id).await?;
    if let Some(info) = &live {
        capture_report(&state, &id, info);
    }
    Ok(Json(doc_of(&entry, live.as_ref())))
}

/// Get the final settlement report for a run.
#[utoipa::path(
    get,
    path = "/v1/backtests/{id}/settlement",
    params(("id" = String, Path, description = "Backtest run id")),
    responses(
        (status = 200, description = "Settlement report (spec D.5 SettlementReport)"),
        (status = 404, description = "Unknown backtest run", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Run has not settled yet", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "backtests"
)]
pub async fn get_settlement(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let entry = state
        .backtest(&id)
        .ok_or_else(|| Problem::not_found("backtest", &id))?;
    if let Some(info) = live_info(&state, &id).await? {
        capture_report(&state, &id, &info);
        match info.state {
            BacktestState::Settled => {
                let report = info
                    .report
                    .and_then(|r| serde_json::to_value(r).ok())
                    .ok_or_else(Problem::internal)?;
                return Ok(Json(report));
            }
            BacktestState::Failed(reason) => {
                return Err(Problem::conflict(format!("backtest failed: {reason}")));
            }
            BacktestState::Running => {
                return Err(Problem::conflict(format!(
                    "backtest is still running ({} intervals settled)",
                    info.intervals_settled
                )));
            }
        }
    }
    if let Some(report) = entry.report {
        return Ok(Json(report));
    }
    Err(Problem::conflict(
        "backtest has not settled and is no longer the active run",
    ))
}
