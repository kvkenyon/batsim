//! System endpoints: health, version, effective config.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::engine::EngineMsg;
use crate::model::{HealthDoc, VersionDoc};
use crate::problem::ApiResult;
use crate::state::AppState;

/// System routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/config", get(config))
}

/// Liveness/readiness.
#[utoipa::path(
    get,
    path = "/v1/system/health",
    responses(
        (status = 200, description = "Server is healthy", body = HealthDoc),
    ),
    tag = "system"
)]
pub async fn health(State(state): State<AppState>) -> ApiResult<Json<HealthDoc>> {
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    Ok(Json(HealthDoc {
        status: "ok".to_owned(),
        sim_state: status.state,
        uptime_s: state.started.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }))
}

/// Build and catalog versions.
#[utoipa::path(
    get,
    path = "/v1/system/version",
    responses(
        (status = 200, description = "Version information", body = VersionDoc),
    ),
    tag = "system"
)]
pub async fn version(State(state): State<AppState>) -> Json<VersionDoc> {
    Json(VersionDoc {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        git_sha: option_env!("BATSIM_GIT_SHA").unwrap_or("unknown").to_owned(),
        registry_version: state.registry.manifest().registry_version.clone(),
        openapi_version: "3.1.0".to_owned(),
    })
}

/// Effective configuration with secrets redacted.
#[utoipa::path(
    get,
    path = "/v1/system/config",
    responses(
        (status = 200, description = "Redacted effective configuration", content_type = "application/json"),
    ),
    tag = "system"
)]
pub async fn config(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let value = serde_json::to_value(state.config.redacted())
        .map_err(|_| crate::problem::Problem::internal())?;
    Ok(Json(value))
}
