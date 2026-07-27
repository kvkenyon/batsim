//! Fleet composition endpoints: manifests expand deterministically into
//! homes.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::compose::{compose_home, expand_manifest, expansion_hash};
use crate::engine::{EngineMsg, SlotMeta};
use crate::ids;
use crate::model::{
    DispatchRequest, DispatchResponse, ExpandFleetRequest, FleetDoc, FleetManifest, FleetsPage,
    HomeConfigDoc, OkDoc, PageInfo, PageParams, TargetSpec,
};
use crate::problem::{ApiResult, Problem};
use crate::state::{AppState, FleetEntry, HomeEntry};

use super::{
    body_hash, decode_cursor, encode_cursor, idempotent, idempotency_key, page_ids, ValidJson,
    ValidQuery,
};

/// Fleet routes.
///
/// The action suffixes (`{id}:expand`, `{id}:dispatch`) live in the
/// same path segment as the id, which the router cannot express; a
/// catch-all preserves the exact URL shape.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_fleets).post(create_fleet))
        .route(
            "/{*rest}",
            get(get_fleet).post(fleet_action).delete(delete_fleet),
        )
}

/// POST /v1/fleets/{id}:expand and /v1/fleets/{id}:dispatch.
pub async fn fleet_action(
    state: State<AppState>,
    Path(rest): Path<String>,
    headers: HeaderMap,
    principal: Option<axum::Extension<super::Principal>>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let Some((id, action)) = rest.rsplit_once(':') else {
        return Err(Problem::not_found("route", &rest));
    };
    match action {
        "expand" => {
            let req: ExpandFleetRequest = serde_json::from_slice(&body)
                .map_err(|e| Problem::validation(format!("invalid JSON body: {e}")))?;
            expand_fleet_impl(state.0, id.to_owned(), req)
                .await
                .map(|doc| (StatusCode::OK, Json(doc)).into_response())
        }
        "dispatch" => {
            let req: FleetDispatchRequest = serde_json::from_slice(&body)
                .map_err(|e| Problem::validation(format!("invalid JSON body: {e}")))?;
            dispatch_fleet_impl(state.0, id.to_owned(), headers, principal, req).await
        }
        _ => Err(Problem::not_found("route", &rest)),
    }
}

/// Create a fleet from a manifest.
#[utoipa::path(
    post,
    path = "/v1/fleets",
    request_body = FleetManifest,
    responses(
        (status = 201, description = "Fleet created", body = FleetDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem),
        (status = 409, description = "Idempotency conflict", body = crate::problem::Problem),
        (status = 422, description = "Composition rule violation", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn create_fleet(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidJson(manifest): ValidJson<FleetManifest>,
) -> ApiResult<Response> {
    let hash = body_hash(&manifest);
    let key = idempotency_key(&headers);
    idempotent(&state, key.as_deref(), hash, || {
        let state = &state;
        async move {
            let doc = create_fleet_inner(state, &manifest, 0).await?;
            Ok((StatusCode::CREATED, doc))
        }
    })
    .await
}

async fn create_fleet_inner(
    state: &AppState,
    manifest: &FleetManifest,
    ordinal_base: u64,
) -> ApiResult<FleetDoc> {
    let plans = expand_manifest(manifest, ordinal_base)?;
    let fleet_id = ids::new_id(ids::FLEET);
    let _compose_guard = state.compose_lock.lock().await;
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let base_idx = status.home_count as u64;
    let mut homes = Vec::with_capacity(plans.len());
    let mut entries = Vec::with_capacity(plans.len());
    for (i, plan) in plans.iter().enumerate() {
        let home_id = ids::new_id(ids::HOME);
        let home = compose_home(
            &state.registry,
            plan,
            &home_id,
            status.master_seed,
            base_idx + i as u64,
        )?;
        let meta = SlotMeta {
            home_id: home_id.clone(),
            fleet_id: Some(fleet_id.clone()),
        };
        homes.push((home, meta));
        entries.push(HomeEntry {
            id: home_id,
            idx: base_idx + i as u64,
            fleet_id: Some(fleet_id.clone()),
            config: HomeConfigDoc {
                fleet_id: Some(fleet_id.clone()),
                battery: plan.battery.clone(),
                inverter_model_id: plan.inverter.as_ref().map(|i| i.model_id.clone()),
                controller_model_id: None,
                pv_peak_kw: plan.pv_peak_kw,
                load_archetype: plan.load.archetype.clone(),
                ercot_load_zone: plan.location.ercot_load_zone.clone(),
            },
            created_at: now_rfc3339(),
        });
    }
    let idxs = state
        .engine
        .call(|tx| EngineMsg::AddHomes {
            homes,
            reply: tx,
        })
        .await?;
    for (entry, idx) in entries.iter_mut().zip(idxs) {
        entry.idx = idx;
    }
    let hash = expansion_hash(manifest, ordinal_base);
    let entry = FleetEntry {
        id: fleet_id.clone(),
        name: manifest.name.clone(),
        manifest: manifest.clone(),
        home_ids: entries.iter().map(|e| e.id.clone()).collect(),
        expansion_hash: hash.clone(),
        created_at: now_rfc3339(),
        expanded_count: manifest.count,
    };
    if let Ok(mut fleets) = state.fleets.write() {
        fleets.insert(fleet_id.clone(), entry.clone());
    }
    if let Ok(mut homes_map) = state.homes.write() {
        for e in entries {
            homes_map.insert(e.id.clone(), e);
        }
    }
    Ok(fleet_doc(&entry))
}

/// Wall clock as RFC 3339.
#[must_use]
pub fn now_rfc3339() -> String {
    crate::engine::rfc3339_of(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
}

/// Render a fleet document.
#[must_use]
pub fn fleet_doc(entry: &FleetEntry) -> FleetDoc {
    FleetDoc {
        id: entry.id.clone(),
        name: entry.name.clone(),
        created_at: entry.created_at.clone(),
        home_count: entry.home_ids.len(),
        expansion_hash: entry.expansion_hash.clone(),
        manifest: entry.manifest.clone(),
    }
}

/// List fleets.
#[utoipa::path(
    get,
    path = "/v1/fleets",
    params(PageParams),
    responses(
        (status = 200, description = "Page of fleets", body = FleetsPage),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn list_fleets(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<PageParams>,
) -> ApiResult<Json<FleetsPage>> {
    let limit = q.limit()?;
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut ids: Vec<String> = state
        .fleets
        .read()
        .map_err(|_| Problem::internal())?
        .keys()
        .cloned()
        .collect();
    ids.sort();
    let (page, has_more) = page_ids(&ids, cursor.as_deref(), limit);
    let data = page
        .iter()
        .filter_map(|id| state.fleet(id))
        .map(|e| fleet_doc(&e))
        .collect();
    let next_cursor = has_more.then(|| encode_cursor(page.last().map_or("", String::as_str)));
    Ok(Json(FleetsPage {
        data,
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

/// Get one fleet.
#[utoipa::path(
    get,
    path = "/v1/fleets/{id}",
    params(("id" = String, Path, description = "Fleet id")),
    responses(
        (status = 200, description = "The fleet", body = FleetDoc),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn get_fleet(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<FleetDoc>> {
    let entry = state.fleet(&id).ok_or_else(|| Problem::not_found("fleet", &id))?;
    Ok(Json(fleet_doc(&entry)))
}

/// Expand a fleet from its manifest.
#[utoipa::path(
    post,
    path = "/v1/fleets/{id}:expand",
    params(("id" = String, Path, description = "Fleet id")),
    request_body = ExpandFleetRequest,
    responses(
        (status = 200, description = "Expanded fleet", body = FleetDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem),
        (status = 422, description = "Composition rule violation", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn expand_fleet(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<ExpandFleetRequest>,
) -> ApiResult<Json<FleetDoc>> {
    expand_fleet_impl(state, id, req).await.map(Json)
}

async fn expand_fleet_impl(
    state: AppState,
    id: String,
    req: ExpandFleetRequest,
) -> ApiResult<FleetDoc> {
    let entry = state.fleet(&id).ok_or_else(|| Problem::not_found("fleet", &id))?;
    let ordinal_base = u64::from(entry.expanded_count);
    let mut manifest = entry.manifest.clone();
    manifest.count = req.count;
    // Reuse the creation path against a scratch fleet, then fold the
    // new homes into this one. The manifest+seed+ordinal chain keeps
    // the expansion deterministic.
    let plans = expand_manifest(&manifest, ordinal_base)?;
    let _compose_guard = state.compose_lock.lock().await;
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let base_idx = status.home_count as u64;
    let mut homes = Vec::with_capacity(plans.len());
    let mut entries = Vec::with_capacity(plans.len());
    for (i, plan) in plans.iter().enumerate() {
        let home_id = ids::new_id(ids::HOME);
        let home = compose_home(
            &state.registry,
            plan,
            &home_id,
            status.master_seed,
            base_idx + i as u64,
        )?;
        homes.push((
            home,
            SlotMeta {
                home_id: home_id.clone(),
                fleet_id: Some(id.clone()),
            },
        ));
        entries.push(HomeEntry {
            id: home_id,
            idx: base_idx + i as u64,
            fleet_id: Some(id.clone()),
            config: HomeConfigDoc {
                fleet_id: Some(id.clone()),
                battery: plan.battery.clone(),
                inverter_model_id: plan.inverter.as_ref().map(|i| i.model_id.clone()),
                controller_model_id: None,
                pv_peak_kw: plan.pv_peak_kw,
                load_archetype: plan.load.archetype.clone(),
                ercot_load_zone: plan.location.ercot_load_zone.clone(),
            },
            created_at: now_rfc3339(),
        });
    }
    let idxs = state
        .engine
        .call(|tx| EngineMsg::AddHomes {
            homes,
            reply: tx,
        })
        .await?;
    if let Ok(mut fleets) = state.fleets.write() {
        if let Some(f) = fleets.get_mut(&id) {
            for (e, idx) in entries.iter().zip(idxs) {
                let mut e = e.clone();
                e.idx = idx;
                f.home_ids.push(e.id.clone());
                if let Ok(mut homes_map) = state.homes.write() {
                    homes_map.insert(e.id.clone(), e);
                }
            }
            f.expanded_count = f.expanded_count.saturating_add(req.count);
            f.expansion_hash = expansion_hash(&f.manifest, 0);
        }
    }
    let updated = state.fleet(&id).ok_or_else(Problem::internal)?;
    Ok(fleet_doc(&updated))
}

/// Delete a fleet and its homes.
#[utoipa::path(
    delete,
    path = "/v1/fleets/{id}",
    params(("id" = String, Path, description = "Fleet id")),
    responses(
        (status = 200, description = "Fleet deleted", body = OkDoc),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem),
        (status = 409, description = "Simulation is running", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn delete_fleet(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkDoc>> {
    let entry = state.fleet(&id).ok_or_else(|| Problem::not_found("fleet", &id))?;
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state == crate::model::SimState::Running {
        return Err(Problem::sim_running(
            "pause or stop the simulation before deleting fleets",
        ));
    }
    for home_id in &entry.home_ids {
        if let Some(h) = state.home(home_id) {
            state
                .engine
                .call(|tx| EngineMsg::RemoveHome {
                    idx: h.idx,
                    reply: tx,
                })
                .await?
                .map_err(|_| Problem::internal())?;
        }
    }
    if let Ok(mut homes) = state.homes.write() {
        for home_id in &entry.home_ids {
            homes.remove(home_id);
        }
    }
    if let Ok(mut fleets) = state.fleets.write() {
        fleets.remove(&id);
    }
    Ok(Json(OkDoc { ok: true }))
}

/// Fleet dispatch body: a dispatch request without the target.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetDispatchRequest {
    /// Client-supplied command id.
    pub command_id: Option<String>,
    /// Action.
    pub action: crate::model::ActionSpec,
    /// Execution shaping.
    pub execution: Option<crate::model::ExecutionSpec>,
}

/// Dispatch to every home in a fleet.
#[utoipa::path(
    post,
    path = "/v1/fleets/{id}:dispatch",
    params(("id" = String, Path, description = "Fleet id")),
    request_body = FleetDispatchRequest,
    responses(
        (status = 202, description = "Command accepted", body = DispatchResponse),
        (status = 400, description = "Validation error", body = crate::problem::Problem),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem),
        (status = 409, description = "Idempotency conflict", body = crate::problem::Problem),
    ),
    tag = "fleets"
)]
pub async fn dispatch_fleet(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    principal: Option<axum::Extension<super::Principal>>,
    ValidJson(req): ValidJson<FleetDispatchRequest>,
) -> ApiResult<Response> {
    dispatch_fleet_impl(state, id, headers, principal, req).await
}

async fn dispatch_fleet_impl(
    state: AppState,
    id: String,
    headers: HeaderMap,
    principal: Option<axum::Extension<super::Principal>>,
    req: FleetDispatchRequest,
) -> ApiResult<Response> {
    if state.fleet(&id).is_none() {
        return Err(Problem::not_found("fleet", &id));
    }
    let full = DispatchRequest {
        command_id: req.command_id,
        target: TargetSpec {
            fleet_id: Some(id),
            home_ids: None,
            filter: None,
            sample_pct: None,
        },
        action: req.action,
        execution: req.execution,
    };
    super::dispatch::dispatch_inner(state, headers, full, principal).await
}
