//! Scenario endpoints: bind time, price, weather, outages, and seed to
//! the current fleet. Exactly one scenario is active at a time.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use batsim_core::engine::AmbientFeed;

use crate::engine::EngineMsg;
use crate::ids;
use crate::model::{
    AmbientSpec, OkDoc, PageInfo, PageParams, ScenarioDoc, ScenarioRequest, ScenariosPage,
    SimState, WeatherSpec,
};
use crate::price::PriceSource;
use crate::problem::{ApiResult, Problem};
use crate::state::{AppState, ScenarioEntry};

use super::fleets::now_rfc3339;
use super::{
    body_hash, decode_cursor, encode_cursor, idempotent, idempotency_key, page_ids, ValidJson,
    ValidQuery,
};

/// Scenario routes.
///
/// The action suffixes (`{id}:activate`, `{id}:deactivate`) share a
/// path segment with the id, which the router cannot express; a
/// catch-all preserves the exact URL shape.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_scenarios).post(create_scenario))
        .route("/{*rest}", get(get_scenario).post(scenario_action))
}

/// POST /v1/scenarios/{id}:activate and /v1/scenarios/{id}:deactivate.
pub async fn scenario_action(
    State(state): State<AppState>,
    Path(rest): Path<String>,
) -> ApiResult<Response> {
    let Some((id, action)) = rest.rsplit_once(':') else {
        // POST on a bare scenario id is not an operation.
        return Err(super::method_not_allowed_problem("GET"));
    };
    match action {
        "activate" => activate_impl(state, id)
            .await
            .map(|doc| (StatusCode::OK, Json(doc)).into_response()),
        "deactivate" => deactivate_impl(state, id)
            .await
            .map(|doc| (StatusCode::OK, Json(doc)).into_response()),
        _ => Err(Problem::not_found("route", &rest)),
    }
}

fn validate_scenario(req: &ScenarioRequest) -> ApiResult<(u64, u64, u32)> {
    let start = crate::engine::unix_of(&req.time.start)
        .map_err(|e| Problem::validation(format!("time.start: {e}")))?;
    let end = crate::engine::unix_of(&req.time.end)
        .map_err(|e| Problem::validation(format!("time.end: {e}")))?;
    if end <= start {
        return Err(Problem::validation("time.end must be after time.start"));
    }
    if start % 300 != 0 {
        return Err(Problem::validation(
            "time.start must be aligned to a 5-minute boundary",
        ));
    }
    let tick_s = req.time.tick_seconds.unwrap_or(1);
    if !(1..=60).contains(&tick_s) {
        return Err(Problem::validation("time.tick_seconds must be within 1..=60"));
    }
    PriceSource::resolve(&req.prices).map_err(Problem::unprocessable)?;
    if matches!(req.weather, Some(WeatherSpec::Replay { .. })) {
        return Err(Problem::unprocessable(
            "weather replay is not available yet; use a synthetic ambient feed",
        ));
    }
    Ok((start, end, tick_s))
}

fn ambient_of(req: &ScenarioRequest) -> AmbientFeed {
    match &req.weather {
        Some(WeatherSpec::Synthetic { ambient }) => match ambient {
            AmbientSpec::Constant { c } => AmbientFeed::Constant(*c),
            AmbientSpec::Diurnal {
                mean_c,
                amplitude_c,
            } => AmbientFeed::DiurnalSine {
                mean_c: *mean_c,
                amplitude_c: *amplitude_c,
            },
        },
        _ => AmbientFeed::Constant(25.0),
    }
}

/// Create a scenario.
#[utoipa::path(
    post,
    path = "/v1/scenarios",
    request_body = ScenarioRequest,
    responses(
        (status = 201, description = "Scenario created", body = ScenarioDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Idempotency conflict", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Unavailable data source", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "scenarios"
)]
pub async fn create_scenario(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidJson(req): ValidJson<ScenarioRequest>,
) -> ApiResult<Response> {
    let hash = body_hash(&req);
    let key = idempotency_key(&headers);
    idempotent(&state, key.as_deref(), hash, || {
        let state = &state;
        async move {
            validate_scenario(&req)?;
            let entry = ScenarioEntry {
                id: ids::new_id(ids::SCENARIO),
                request: req.clone(),
                created_at: now_rfc3339(),
            };
            if let Ok(mut scenarios) = state.scenarios.write() {
                scenarios.insert(entry.id.clone(), entry.clone());
            }
            let active = state
                .active_scenario
                .read()
                .ok()
                .and_then(|a| a.clone())
                .is_some_and(|id| id == entry.id);
            let doc = ScenarioDoc {
                id: entry.id,
                request: entry.request,
                created_at: entry.created_at,
                active,
            };
            Ok((StatusCode::CREATED, doc))
        }
    })
    .await
}

fn scenario_doc(state: &AppState, entry: &ScenarioEntry) -> ScenarioDoc {
    let active = state
        .active_scenario
        .read()
        .ok()
        .and_then(|a| a.clone())
        .is_some_and(|id| id == entry.id);
    ScenarioDoc {
        id: entry.id.clone(),
        request: entry.request.clone(),
        created_at: entry.created_at.clone(),
        active,
    }
}

/// List scenarios.
#[utoipa::path(
    get,
    path = "/v1/scenarios",
    params(PageParams),
    responses(
        (status = 200, description = "Page of scenarios", body = ScenariosPage),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "scenarios"
)]
pub async fn list_scenarios(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<PageParams>,
) -> ApiResult<Json<ScenariosPage>> {
    let limit = q.limit()?;
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut ids: Vec<String> = state
        .scenarios
        .read()
        .map_err(|_| Problem::internal())?
        .keys()
        .cloned()
        .collect();
    ids.sort();
    let (page, has_more) = page_ids(&ids, cursor.as_deref(), limit);
    let data = page
        .iter()
        .filter_map(|id| state.scenario(id))
        .map(|e| scenario_doc(&state, &e))
        .collect();
    let next_cursor = has_more.then(|| encode_cursor(page.last().map_or("", String::as_str)));
    Ok(Json(ScenariosPage {
        data,
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

/// Get one scenario.
#[utoipa::path(
    get,
    path = "/v1/scenarios/{id}",
    params(("id" = String, Path, description = "Scenario id")),
    responses(
        (status = 200, description = "The scenario", body = ScenarioDoc),
        (status = 404, description = "Unknown scenario", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "scenarios"
)]
pub async fn get_scenario(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScenarioDoc>> {
    if id.contains(':') {
        // Action-suffixed paths are POST-only; the router's catch-all
        // would otherwise answer GET with a misleading 404.
        return Err(super::method_not_allowed_problem("POST"));
    }
    let entry = state
        .scenario(&id)
        .ok_or_else(|| Problem::not_found("scenario", &id))?;
    Ok(Json(scenario_doc(&state, &entry)))
}

/// Activate a scenario (requires a stopped simulation).
#[utoipa::path(
    post,
    path = "/v1/scenarios/{id}:activate",
    params(("id" = String, Path, description = "Scenario id")),
    responses(
        (status = 200, description = "Activated scenario", body = ScenarioDoc),
        (status = 404, description = "Unknown scenario", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is not stopped", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Unavailable data source", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "scenarios"
)]
pub async fn activate_scenario(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScenarioDoc>> {
    activate_impl(state, &id).await.map(Json)
}

async fn activate_impl(state: AppState, id: &str) -> ApiResult<ScenarioDoc> {
    let entry = state
        .scenario(id)
        .ok_or_else(|| Problem::not_found("scenario", id))?;
    let (start, _end, tick_s) = validate_scenario(&entry.request)?;
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state != SimState::Stopped {
        return Err(Problem::sim_running(
            "scenario activation requires a stopped simulation",
        ));
    }
    let price = PriceSource::resolve(&entry.request.prices).map_err(Problem::unprocessable)?;
    state
        .engine
        .call(|tx| EngineMsg::Rebind {
            epoch_s: start,
            tick_s,
            seed: entry.request.seed,
            ambient: ambient_of(&entry.request),
            price,
            reply: tx,
        })
        .await?
        .map_err(Problem::unprocessable)?;
    if let Ok(mut active) = state.active_scenario.write() {
        *active = Some(id.to_owned());
    }
    Ok(scenario_doc(&state, &entry))
}

/// Deactivate the active scenario.
#[utoipa::path(
    post,
    path = "/v1/scenarios/{id}:deactivate",
    params(("id" = String, Path, description = "Scenario id")),
    responses(
        (status = 200, description = "Scenario deactivated", body = OkDoc),
        (status = 404, description = "Unknown scenario", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is not stopped or scenario not active", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "scenarios"
)]
pub async fn deactivate_scenario(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkDoc>> {
    deactivate_impl(state, &id).await.map(Json)
}

async fn deactivate_impl(state: AppState, id: &str) -> ApiResult<OkDoc> {
    if state.scenario(id).is_none() {
        return Err(Problem::not_found("scenario", id));
    }
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state != SimState::Stopped {
        return Err(Problem::sim_running(
            "scenario deactivation requires a stopped simulation",
        ));
    }
    let mut was_active = false;
    if let Ok(mut active) = state.active_scenario.write() {
        if active.as_deref() == Some(id) {
            *active = None;
            was_active = true;
        }
    }
    if !was_active {
        return Err(Problem::conflict("scenario is not active"));
    }
    Ok(OkDoc { ok: true })
}
