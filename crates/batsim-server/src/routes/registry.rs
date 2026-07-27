//! Device catalog endpoints.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::model::{
    BatteryList, BatteryListParams, BatterySummary, InverterList, InverterListParams,
    InverterSummary, RegistryVersionDoc,
};
use crate::problem::{ApiResult, Problem};
use crate::state::AppState;

use super::ValidQuery;

/// Registry routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/batteries", get(list_batteries))
        .route("/batteries/{model_id}", get(get_battery))
        .route("/inverters", get(list_inverters))
        .route("/inverters/{model_id}", get(get_inverter))
        .route("/version", get(catalog_version))
}

/// List battery models.
#[utoipa::path(
    get,
    path = "/v1/registry/batteries",
    params(BatteryListParams),
    responses(
        (status = 200, description = "Battery catalog entries", body = BatteryList),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "registry"
)]
pub async fn list_batteries(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<BatteryListParams>,
) -> ApiResult<Json<BatteryList>> {
    let data = state
        .registry
        .batteries()
        .filter(|b| {
            q.vendor
                .as_ref()
                .is_none_or(|v| b.vendor.contains(v.as_str()))
        })
        .filter(|b| {
            q.min_capacity_kwh
                .is_none_or(|m| b.usable_energy_kwh.value >= m)
        })
        .filter(|b| {
            q.max_capacity_kwh
                .is_none_or(|m| b.usable_energy_kwh.value <= m)
        })
        .filter(|b| {
            q.chemistry
                .as_ref()
                .is_none_or(|c| format!("{:?}", b.chemistry).eq_ignore_ascii_case(c))
        })
        .map(|b| BatterySummary {
            model_id: b.model_id.clone(),
            vendor: b.vendor.clone(),
            display_name: b.display_name.clone(),
            chemistry: format!("{:?}", b.chemistry),
            coupling: format!("{:?}", b.coupling),
            usable_energy_kwh: b.usable_energy_kwh.value,
            continuous_charge_power_kw: b.continuous_charge_power_kw.value,
            continuous_discharge_power_kw: b.continuous_discharge_power_kw.value,
        })
        .collect();
    Ok(Json(BatteryList { data }))
}

/// One battery model, full catalog schema.
#[utoipa::path(
    get,
    path = "/v1/registry/batteries/{model_id}",
    params(("model_id" = String, Path, description = "Battery model id")),
    responses(
        (status = 200, description = "The battery model", content_type = "application/json"),
        (status = 404, description = "Unknown model", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "registry"
)]
pub async fn get_battery(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let model = state
        .registry
        .battery(&model_id)
        .ok_or_else(|| Problem::not_found("battery model", &model_id))?;
    let value = serde_json::to_value(model).map_err(|_| Problem::internal())?;
    Ok(Json(value))
}

/// List inverter models.
#[utoipa::path(
    get,
    path = "/v1/registry/inverters",
    params(InverterListParams),
    responses(
        (status = 200, description = "Inverter catalog entries", body = InverterList),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "registry"
)]
pub async fn list_inverters(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<InverterListParams>,
) -> ApiResult<Json<InverterList>> {
    let data = state
        .registry
        .inverters()
        .filter(|i| {
            q.vendor
                .as_ref()
                .is_none_or(|v| i.vendor.contains(v.as_str()))
        })
        .filter(|i| q.min_power_kw.is_none_or(|m| i.rated_ac_output_kw.value >= m))
        .map(|i| InverterSummary {
            model_id: i.model_id.clone(),
            vendor: i.vendor.clone(),
            display_name: i.display_name.clone(),
            topology: format!("{:?}", i.topology),
            rated_ac_output_kw: i.rated_ac_output_kw.value,
        })
        .collect();
    Ok(Json(InverterList { data }))
}

/// One inverter model, full catalog schema.
#[utoipa::path(
    get,
    path = "/v1/registry/inverters/{model_id}",
    params(("model_id" = String, Path, description = "Inverter model id")),
    responses(
        (status = 200, description = "The inverter model", content_type = "application/json"),
        (status = 404, description = "Unknown model", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "registry"
)]
pub async fn get_inverter(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let model = state
        .registry
        .inverter(&model_id)
        .ok_or_else(|| Problem::not_found("inverter model", &model_id))?;
    let value = serde_json::to_value(model).map_err(|_| Problem::internal())?;
    Ok(Json(value))
}

/// Catalog version and integrity hash.
#[utoipa::path(
    get,
    path = "/v1/registry/version",
    responses(
        (status = 200, description = "Catalog version", body = RegistryVersionDoc),
    ),
    tag = "registry"
)]
pub async fn catalog_version(State(state): State<AppState>) -> Json<RegistryVersionDoc> {
    let manifest = state.registry.manifest();
    Json(RegistryVersionDoc {
        registry_version: manifest.registry_version.clone(),
        schema_version: manifest.schema_version.clone(),
        catalog_sha256: manifest.catalog_sha256.clone(),
        batteries: state.registry.batteries().count(),
        inverters: state.registry.inverters().count(),
    })
}
