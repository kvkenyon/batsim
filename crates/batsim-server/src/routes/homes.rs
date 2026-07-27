//! Simulated home CRUD.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use batsim_core::dispatch::ControlMode;

use crate::compose::{compose_home, fixed_pv, HomePlan};
use crate::engine::EngineMsg;
use crate::ids;
use crate::model::{
    CreateHomeRequest, HomeDoc, HomeListParams, HomeStateDoc, HomesPage, OkDoc, OperatingMode,
    PageInfo, PatchHomeRequest,
};
use crate::problem::{ApiResult, Problem};
use crate::state::{AppState, HomeEntry};

use super::{
    body_hash, decode_cursor, encode_cursor, idempotent, idempotency_key, page_ids, ValidJson,
    ValidQuery,
};

/// Home routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_homes).post(create_home))
        .route("/{id}", get(get_home).patch(patch_home).delete(delete_home))
}

/// Map an API operating mode onto the engine control mode.
#[must_use]
pub fn control_mode_of(mode: OperatingMode) -> ControlMode {
    match mode {
        OperatingMode::SelfConsumption | OperatingMode::TimeOfUse => ControlMode::SelfConsumption,
        OperatingMode::BackupOnly => ControlMode::BackupReserveHold,
        OperatingMode::GridServices => ControlMode::Manual,
    }
}

/// Map an engine control mode back to the API vocabulary.
#[must_use]
pub fn api_mode_of(mode: ControlMode) -> OperatingMode {
    match mode {
        ControlMode::SelfConsumption => OperatingMode::SelfConsumption,
        ControlMode::BackupReserveHold => OperatingMode::BackupOnly,
        ControlMode::Manual | ControlMode::Idle => OperatingMode::GridServices,
    }
}

/// Build the state section of a home document from engine dynamics.
#[must_use]
pub fn state_doc(dyn_state: &crate::engine::HomeDyn, sim_time: String) -> HomeStateDoc {
    HomeStateDoc {
        soc: dyn_state.soc,
        mode: api_mode_of(dyn_state.mode),
        battery_power_kw: dyn_state.batt_w / 1000.0,
        pv_power_kw: dyn_state.pv_w / 1000.0,
        load_power_kw: dyn_state.load_w / 1000.0,
        grid_power_kw: dyn_state.grid_w / 1000.0,
        sim_time,
        pv_curtail_frac: dyn_state.curtail,
        manual_setpoint_kw: dyn_state.manual_setpoint_w / 1000.0,
    }
}

/// Fetch a full home document (config + live state).
pub async fn home_doc(state: &AppState, entry: &HomeEntry) -> ApiResult<HomeDoc> {
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let dyn_state = state
        .engine
        .call(|tx| EngineMsg::HomeState {
            idx: entry.idx,
            reply: tx,
        })
        .await?
        .map_err(|_| Problem::internal())?;
    Ok(HomeDoc {
        id: entry.id.clone(),
        config: entry.config.clone(),
        state: state_doc(&dyn_state, crate::engine::rfc3339_of(status.unix)),
        created_at: entry.created_at.clone(),
    })
}

/// Create a home.
#[utoipa::path(
    post,
    path = "/v1/homes",
    request_body = CreateHomeRequest,
    responses(
        (status = 201, description = "Home created", body = HomeDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Fleet not found", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Idempotency conflict", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Composition rule violation", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "homes"
)]
pub async fn create_home(
    State(state): State<AppState>,
    headers: HeaderMap,
    ValidJson(req): ValidJson<CreateHomeRequest>,
) -> ApiResult<Response> {
    let hash = body_hash(&req);
    let key = idempotency_key(&headers);
    idempotent(&state, key.as_deref(), hash, || {
        let state = &state;
        async move {
            let doc = create_home_inner(state, &req).await?;
            Ok((StatusCode::CREATED, doc))
        }
    })
    .await
}

async fn create_home_inner(state: &AppState, req: &CreateHomeRequest) -> ApiResult<HomeDoc> {
    if let Some(fleet_id) = &req.fleet_id {
        if state.fleet(fleet_id).is_none() {
            return Err(Problem::not_found("fleet", fleet_id));
        }
    }
    let _compose_guard = state.compose_lock.lock().await;
    let (peak, azimuth, tilt) = match &req.pv {
        Some(pv) => {
            let (kw, az, ti) = fixed_pv(pv)?;
            (Some(kw), az, ti)
        }
        None => (None, 180.0, 25.0),
    };
    let plan = HomePlan {
        battery: req.battery.clone(),
        inverter: req.inverter.clone(),
        pv_peak_kw: peak,
        pv_azimuth_deg: azimuth,
        pv_tilt_deg: tilt,
        load: req.load.clone(),
        location: req.location.clone(),
        initial_soc: req.initial_soc.unwrap_or(0.5),
    };
    let home_id = ids::new_id(ids::HOME);
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let home = compose_home(
        &state.registry,
        &plan,
        &home_id,
        status.master_seed,
        status.home_count as u64,
    )?;
    let meta = crate::engine::SlotMeta {
        home_id: home_id.clone(),
        fleet_id: req.fleet_id.clone(),
    };
    let idxs = state
        .engine
        .call(|tx| EngineMsg::AddHomes {
            homes: vec![(home, meta)],
            reply: tx,
        })
        .await?;
    let idx = idxs.first().copied().unwrap_or(0);
    let entry = HomeEntry {
        id: home_id,
        idx,
        fleet_id: req.fleet_id.clone(),
        config: crate::model::HomeConfigDoc {
            fleet_id: req.fleet_id.clone(),
            battery: plan.battery.clone(),
            inverter_model_id: plan.inverter.as_ref().map(|i| i.model_id.clone()),
            controller_model_id: None,
            pv_peak_kw: plan.pv_peak_kw,
            load_archetype: plan.load.archetype.clone(),
            ercot_load_zone: plan.location.ercot_load_zone.clone(),
        },
        created_at: crate::engine::rfc3339_of(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        ),
    };
    if let Ok(mut homes) = state.homes.write() {
        homes.insert(entry.id.clone(), entry.clone());
    }
    if let Some(fleet_id) = &req.fleet_id {
        if let Ok(mut fleets) = state.fleets.write() {
            if let Some(f) = fleets.get_mut(fleet_id) {
                f.home_ids.push(entry.id.clone());
            }
        }
    }
    home_doc(state, &entry).await
}

/// List homes.
#[utoipa::path(
    get,
    path = "/v1/homes",
    params(HomeListParams),
    responses(
        (status = 200, description = "Page of homes", body = HomesPage),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "homes"
)]
pub async fn list_homes(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<HomeListParams>,
) -> ApiResult<Json<HomesPage>> {
    let limit = crate::model::PageParams {
        limit: q.limit,
        cursor: None,
    }
    .limit()?;
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let mut entries: Vec<HomeEntry> = state
        .homes
        .read()
        .map_err(|_| Problem::internal())?
        .values()
        .filter(|h| q.fleet_id.as_ref().is_none_or(|f| h.fleet_id.as_ref() == Some(f)))
        .filter(|h| {
            q.load_zone
                .as_ref()
                .is_none_or(|z| &h.config.ercot_load_zone == z)
        })
        .filter(|h| {
            q.battery_model
                .as_ref()
                .is_none_or(|m| &h.config.battery.model_id == m)
        })
        .cloned()
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let (page, has_more) = page_ids(&ids, cursor.as_deref(), limit);
    let mut data = Vec::with_capacity(page.len());
    for id in &page {
        let entry = state.home(id).ok_or_else(Problem::internal)?;
        let doc = home_doc(&state, &entry).await?;
        if let Some(mode) = q.mode {
            if doc.state.mode != mode {
                continue;
            }
        }
        data.push(doc);
    }
    let next_cursor = has_more.then(|| encode_cursor(page.last().map_or("", String::as_str)));
    Ok(Json(HomesPage {
        data,
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

/// Get one home.
#[utoipa::path(
    get,
    path = "/v1/homes/{id}",
    params(("id" = String, Path, description = "Home id")),
    responses(
        (status = 200, description = "The home", body = HomeDoc),
        (status = 404, description = "Unknown home", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "homes"
)]
pub async fn get_home(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<HomeDoc>> {
    let entry = state.home(&id).ok_or_else(|| Problem::not_found("home", &id))?;
    Ok(Json(home_doc(&state, &entry).await?))
}

/// Update mutable home configuration.
#[utoipa::path(
    patch,
    path = "/v1/homes/{id}",
    params(("id" = String, Path, description = "Home id")),
    request_body = PatchHomeRequest,
    responses(
        (status = 200, description = "Updated home", body = HomeDoc),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown home", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is running", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "homes"
)]
pub async fn patch_home(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ValidJson(req): ValidJson<PatchHomeRequest>,
) -> ApiResult<Json<HomeDoc>> {
    let entry = state.home(&id).ok_or_else(|| Problem::not_found("home", &id))?;
    if let Some(r) = req.reserve_soc {
        if !(0.0..=1.0).contains(&r) {
            return Err(Problem::validation("reserve_soc must be within 0..=1"));
        }
    }
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state == crate::model::SimState::Running {
        return Err(Problem::sim_running(
            "pause the simulation before changing home configuration",
        ));
    }
    state
        .engine
        .call(|tx| EngineMsg::PatchHome {
            idx: entry.idx,
            mode: req.mode.map(control_mode_of),
            reserve: req.reserve_soc,
            reply: tx,
        })
        .await?
        .map_err(|_| Problem::internal())?;
    Ok(Json(home_doc(&state, &entry).await?))
}

/// Delete a home.
#[utoipa::path(
    delete,
    path = "/v1/homes/{id}",
    params(("id" = String, Path, description = "Home id")),
    responses(
        (status = 200, description = "Home deleted", body = OkDoc),
        (status = 404, description = "Unknown home", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Simulation is running", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "homes"
)]
pub async fn delete_home(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkDoc>> {
    let entry = state.home(&id).ok_or_else(|| Problem::not_found("home", &id))?;
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    if status.state == crate::model::SimState::Running {
        return Err(Problem::sim_running(
            "pause or stop the simulation before deleting homes",
        ));
    }
    state
        .engine
        .call(|tx| EngineMsg::RemoveHome {
            idx: entry.idx,
            reply: tx,
        })
        .await?
        .map_err(|_| Problem::internal())?;
    if let Ok(mut homes) = state.homes.write() {
        homes.remove(&id);
    }
    if let Some(fleet_id) = &entry.fleet_id {
        if let Ok(mut fleets) = state.fleets.write() {
            if let Some(f) = fleets.get_mut(fleet_id) {
                f.home_ids.retain(|h| h != &id);
            }
        }
    }
    Ok(Json(OkDoc { ok: true }))
}
