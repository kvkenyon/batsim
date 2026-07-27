//! Virtual time control endpoints.

use axum::extract::State;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::engine::{EngineMsg, EngineStatus, StepOutcome};
use crate::model::{
    RunUntilRequest, SimState, SimStatusDoc, SpeedRequest, StepRequest, StepResponse,
};
use crate::problem::{ApiResult, Problem};
use crate::state::AppState;

use super::{ValidJson, ValidQuery};

/// Sim routes. The time-control verbs (`/v1/sim:start`) are single
/// path segments with a literal colon, so they merge flat rather than
/// nesting.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sim:start", post(start))
        .route("/sim:pause", post(pause))
        .route("/sim:resume", post(resume))
        .route("/sim:stop", post(stop))
        .route("/sim:step", post(step))
        .route("/sim:run-until", post(run_until))
        .route("/sim:speed", put(set_speed))
        .route("/sim:status", get(status))
}

fn status_doc(state: &AppState, s: &EngineStatus) -> SimStatusDoc {
    SimStatusDoc {
        state: s.state,
        sim_time: crate::engine::rfc3339_of(s.unix),
        tick: s.tick,
        speed: s.speed,
        achieved_speed: s.achieved_speed,
        lag_ticks: s.lag_ticks,
        queued_commands: s.queued_commands,
        active_scenario: state
            .active_scenario
            .read()
            .ok()
            .and_then(|a| a.clone()),
    }
}

async fn engine_status(state: &AppState) -> ApiResult<EngineStatus> {
    state.engine.call(|tx| EngineMsg::Status { reply: tx }).await
}

/// Map an engine state-machine rejection to the right problem.
fn transition_error(msg: String, current: SimState) -> Problem {
    if current == SimState::Running {
        Problem::sim_running(msg)
    } else {
        Problem::sim_not_running(msg)
    }
}

/// Begin ticking.
#[utoipa::path(
    post,
    path = "/v1/sim:start",
    responses(
        (status = 200, description = "Simulation started", body = SimStatusDoc),
        (status = 409, description = "Illegal transition", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn start(State(state): State<AppState>) -> ApiResult<Json<SimStatusDoc>> {
    state
        .engine
        .call(|tx| EngineMsg::Start { reply: tx })
        .await?
        .map_err(Problem::sim_running)?;
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}

/// Pause at a tick boundary.
#[utoipa::path(
    post,
    path = "/v1/sim:pause",
    responses(
        (status = 200, description = "Simulation paused", body = SimStatusDoc),
        (status = 409, description = "Illegal transition", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn pause(State(state): State<AppState>) -> ApiResult<Json<SimStatusDoc>> {
    let cur = engine_status(&state).await?;
    state
        .engine
        .call(|tx| EngineMsg::Pause { reply: tx })
        .await?
        .map_err(|m| transition_error(m, cur.state))?;
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}

/// Resume from paused.
#[utoipa::path(
    post,
    path = "/v1/sim:resume",
    responses(
        (status = 200, description = "Simulation resumed", body = SimStatusDoc),
        (status = 409, description = "Illegal transition", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn resume(State(state): State<AppState>) -> ApiResult<Json<SimStatusDoc>> {
    let cur = engine_status(&state).await?;
    state
        .engine
        .call(|tx| EngineMsg::Resume { reply: tx })
        .await?
        .map_err(|m| transition_error(m, cur.state))?;
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}

/// Stop (state retained).
#[utoipa::path(
    post,
    path = "/v1/sim:stop",
    responses(
        (status = 200, description = "Simulation stopped", body = SimStatusDoc),
        (status = 409, description = "Illegal transition", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn stop(State(state): State<AppState>) -> ApiResult<Json<SimStatusDoc>> {
    let cur = engine_status(&state).await?;
    state
        .engine
        .call(|tx| EngineMsg::Stop { reply: tx })
        .await?
        .map_err(|m| transition_error(m, cur.state))?;
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}

/// `allow_large` query flag shared by step and run-until.
#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct AllowLarge {
    /// Permit advances beyond one simulation day.
    pub allow_large: Option<bool>,
}

const MAX_SYNC_TICKS: u64 = 86_400;

fn outcome_doc(o: &StepOutcome) -> StepResponse {
    StepResponse {
        sim_time: crate::engine::rfc3339_of(o.unix),
        tick: o.tick,
        ticks_executed: o.ticks,
        wall_ms: o.wall_ms,
    }
}

/// Advance N ticks synchronously while paused.
#[utoipa::path(
    post,
    path = "/v1/sim:step",
    params(AllowLarge),
    request_body = StepRequest,
    responses(
        (status = 200, description = "Advanced", body = StepResponse),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is not paused", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn step(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<AllowLarge>,
    ValidJson(req): ValidJson<StepRequest>,
) -> ApiResult<Json<StepResponse>> {
    if req.ticks == 0 {
        return Err(Problem::validation("ticks must be >= 1"));
    }
    if req.ticks > MAX_SYNC_TICKS && q.allow_large != Some(true) {
        return Err(Problem::validation(format!(
            "ticks is capped at {MAX_SYNC_TICKS} (one sim-day) unless allow_large=true"
        )));
    }
    let cur = engine_status(&state).await?;
    if cur.state == SimState::Running {
        return Err(Problem::sim_running("pause the simulation before stepping"));
    }
    if cur.state == SimState::Stopped {
        return Err(Problem::sim_not_running(
            "start and pause the simulation before stepping",
        ));
    }
    let outcome = state
        .engine
        .call(|tx| EngineMsg::Step {
            ticks: req.ticks,
            reply: tx,
        })
        .await?
        .map_err(Problem::conflict)?;
    Ok(Json(outcome_doc(&outcome)))
}

/// Advance to a target simulation time synchronously while paused.
#[utoipa::path(
    post,
    path = "/v1/sim:run-until",
    params(AllowLarge),
    request_body = RunUntilRequest,
    responses(
        (status = 200, description = "Advanced", body = StepResponse),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is not paused", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn run_until(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<AllowLarge>,
    ValidJson(req): ValidJson<RunUntilRequest>,
) -> ApiResult<Json<StepResponse>> {
    let until = crate::engine::unix_of(&req.until)
        .map_err(|e| Problem::validation(format!("until: {e}")))?;
    let cur = engine_status(&state).await?;
    if cur.state == SimState::Running {
        return Err(Problem::sim_running("pause the simulation before run-until"));
    }
    if cur.state == SimState::Stopped {
        return Err(Problem::sim_not_running(
            "start and pause the simulation before run-until",
        ));
    }
    if until <= cur.unix {
        return Err(Problem::validation("until must be in the future"));
    }
    let ticks = (until - cur.unix).div_ceil(u64::from(cur.dt_s));
    if ticks > MAX_SYNC_TICKS && q.allow_large != Some(true) {
        return Err(Problem::validation(format!(
            "advance would exceed {MAX_SYNC_TICKS} ticks (one sim-day); pass allow_large=true"
        )));
    }
    let outcome = state
        .engine
        .call(|tx| EngineMsg::RunUntil { unix: until, reply: tx })
        .await?
        .map_err(Problem::conflict)?;
    Ok(Json(outcome_doc(&outcome)))
}

/// Change the speed multiplier.
#[utoipa::path(
    put,
    path = "/v1/sim:speed",
    request_body = SpeedRequest,
    responses(
        (status = 200, description = "Speed updated", body = SimStatusDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "sim"
)]
pub async fn set_speed(
    State(state): State<AppState>,
    ValidJson(req): ValidJson<SpeedRequest>,
) -> ApiResult<Json<SimStatusDoc>> {
    if !(req.multiplier.is_finite() && req.multiplier >= 0.0) {
        return Err(Problem::validation("multiplier must be finite and >= 0"));
    }
    state
        .engine
        .call(|tx| EngineMsg::SetSpeed {
            multiplier: req.multiplier,
            reply: tx,
        })
        .await?
        .map_err(Problem::validation)?;
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}

/// Simulation status.
#[utoipa::path(
    get,
    path = "/v1/sim:status",
    responses(
        (status = 200, description = "Status", body = SimStatusDoc),
    ),
    tag = "sim"
)]
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<SimStatusDoc>> {
    Ok(Json(status_doc(&state, &engine_status(&state).await?)))
}
