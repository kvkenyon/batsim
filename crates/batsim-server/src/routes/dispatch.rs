//! Fleet dispatch: targets expand to per-home commands with jittered,
//! deterministic execution latency; every command lands in the audit
//! log with per-target execution detail.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use batsim_core::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use rand::Rng;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use xxhash_rust::xxh3::xxh3_64;

use crate::engine::EngineMsg;
use crate::ids;
use crate::model::{
    ActionSpec, CommandDoc, CommandListParams, CommandStatus, CommandsPage, DispatchRequest,
    DispatchResponse, ExecutionSpec, LatencySpec, OkDoc, PageInfo, TargetExecution,
};
use crate::problem::{ApiResult, Problem};
use crate::state::AppState;

use super::homes::control_mode_of;
use super::{
    body_hash, decode_cursor, encode_cursor, idempotent, idempotency_key, page_ids, principal_of,
    Principal, ValidJson, ValidQuery,
};

/// Dispatch routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(dispatch))
        .route("/commands", get(list_commands))
        .route("/commands/{command_id}", get(get_command).delete(cancel_command))
}

/// Hash a command id to the engine's issuer tag.
#[must_use]
pub fn command_tag(command_id: &str) -> u64 {
    xxh3_64(command_id.as_bytes())
}

/// Deterministic per-target latency draw in milliseconds.
fn latency_ms(spec: Option<&ExecutionSpec>, command_id: &str, home_id: &str) -> u64 {
    let mut key = Vec::with_capacity(command_id.len() + home_id.len() + 1);
    key.extend_from_slice(command_id.as_bytes());
    key.push(0);
    key.extend_from_slice(home_id.as_bytes());
    let mut rng = ChaCha8Rng::seed_from_u64(xxh3_64(&key));
    match spec.and_then(|e| e.latency_ms) {
        None => rng.gen_range(250..=2000),
        Some(LatencySpec::Fixed(ms)) => ms,
        Some(LatencySpec::Range { uniform }) => {
            // `low..=u64::MAX` overflows the inclusive sampler's width
            // computation; shrink only that edge.
            if uniform[0] >= uniform[1] {
                uniform[0]
            } else {
                let hi = if uniform[1] == u64::MAX {
                    uniform[1] - 1
                } else {
                    uniform[1]
                };
                rng.gen_range(uniform[0]..=hi)
            }
        }
    }
}

/// Validate the action; returns the signed requested kW for power
/// actions (engine sign convention: + discharge, - charge).
fn requested_kw(action: &ActionSpec) -> ApiResult<Option<f64>> {
    match action {
        ActionSpec::ChargeTo { kw, .. } => {
            if !(kw.is_finite() && *kw > 0.0) {
                return Err(Problem::validation("kw must be finite and > 0"));
            }
            Ok(Some(-kw))
        }
        ActionSpec::DischargeTo { kw, .. } => {
            if !(kw.is_finite() && *kw > 0.0) {
                return Err(Problem::validation("kw must be finite and > 0"));
            }
            Ok(Some(*kw))
        }
        ActionSpec::SetReserveSoc { soc } => {
            if !(0.0..=1.0).contains(soc) {
                return Err(Problem::validation("soc must be within 0..=1"));
            }
            Ok(None)
        }
        ActionSpec::CurtailPv { pct } => {
            if !(pct.is_finite() && (0.0..=100.0).contains(pct)) {
                return Err(Problem::validation("pct must be within 0..=100"));
            }
            Ok(None)
        }
        ActionSpec::SetMode { .. } | ActionSpec::ClearOverride {} => Ok(None),
    }
}

/// Expand the request's target fields into a sorted home id list.
fn resolve_target_ids(state: &AppState, req: &DispatchRequest) -> ApiResult<Vec<String>> {
    let mut target_ids: Vec<String> = Vec::new();
    if let Some(fleet_id) = &req.target.fleet_id {
        let fleet = state
            .fleet(fleet_id)
            .ok_or_else(|| Problem::not_found("fleet", fleet_id))?;
        target_ids.extend(fleet.home_ids.iter().cloned());
    }
    if let Some(home_ids) = &req.target.home_ids {
        for id in home_ids {
            if state.home(id).is_none() {
                return Err(Problem::not_found("home", id));
            }
            if !target_ids.contains(id) {
                target_ids.push(id.clone());
            }
        }
    }
    if target_ids.is_empty() && req.target.fleet_id.is_none() && req.target.home_ids.is_none() {
        return Err(Problem::validation(
            "target must name a fleet_id and/or home_ids",
        ));
    }
    target_ids.sort();
    Ok(target_ids)
}

/// Apply mode/SOC filters and deterministic sampling.
async fn filter_targets(
    state: &AppState,
    req: &DispatchRequest,
    command_id: &str,
    target_ids: &[String],
) -> ApiResult<Vec<(String, u64)>> {
    let mut resolved: Vec<(String, u64)> = Vec::with_capacity(target_ids.len());
    for id in target_ids {
        let Some(entry) = state.home(id) else {
            continue;
        };
        if let Some(filter) = &req.target.filter {
            let dyn_state = state
                .engine
                .call(|tx| EngineMsg::HomeState {
                    idx: entry.idx,
                    reply: tx,
                })
                .await?
                .map_err(|_| Problem::internal())?;
            if let Some(modes) = &filter.mode {
                let api_mode = super::homes::api_mode_of(dyn_state.mode);
                if !modes.contains(&api_mode) {
                    continue;
                }
            }
            if let Some(floor) = filter.soc_gt {
                if dyn_state.soc <= floor {
                    continue;
                }
            }
        }
        if let Some(pct) = req.target.sample_pct {
            let h = xxh3_64(format!("{command_id}:{id}").as_bytes());
            if (h % 100) as f64 >= pct {
                continue;
            }
        }
        resolved.push((id.clone(), entry.idx));
    }
    if resolved.is_empty() {
        return Err(Problem::unprocessable(
            "target resolution produced an empty set",
        ));
    }
    Ok(resolved)
}

/// Translate an action into engine dispatches (tagged, tick-aligned).
fn actions_for(
    action: &ActionSpec,
    execute_at_tick: u64,
    dt_s: u32,
    tag: u64,
) -> Vec<ScheduledDispatch> {
    let at = |action| ScheduledDispatch {
        execute_at_tick,
        action,
        tag,
    };
    let revert_at = |ticks_from_now: u64, action| ScheduledDispatch {
        execute_at_tick: execute_at_tick.saturating_add(ticks_from_now),
        action,
        tag,
    };
    match *action {
        ActionSpec::ChargeTo { kw, duration_s } => {
            let mut v = vec![
                at(DispatchAction::SetMode(ControlMode::Manual)),
                at(DispatchAction::SetManualSetpoint(-kw * 1000.0)),
            ];
            if let Some(d) = duration_s {
                let ticks = d.div_ceil(u64::from(dt_s));
                v.push(revert_at(ticks, DispatchAction::SetManualSetpoint(0.0)));
                v.push(revert_at(
                    ticks,
                    DispatchAction::SetMode(ControlMode::SelfConsumption),
                ));
            }
            v
        }
        ActionSpec::DischargeTo { kw, duration_s } => {
            let mut v = vec![
                at(DispatchAction::SetMode(ControlMode::Manual)),
                at(DispatchAction::SetManualSetpoint(kw * 1000.0)),
            ];
            if let Some(d) = duration_s {
                let ticks = d.div_ceil(u64::from(dt_s));
                v.push(revert_at(ticks, DispatchAction::SetManualSetpoint(0.0)));
                v.push(revert_at(
                    ticks,
                    DispatchAction::SetMode(ControlMode::SelfConsumption),
                ));
            }
            v
        }
        ActionSpec::SetReserveSoc { soc } => vec![at(DispatchAction::SetReserve(soc))],
        ActionSpec::SetMode { mode } => vec![at(DispatchAction::SetMode(control_mode_of(mode)))],
        ActionSpec::CurtailPv { pct } => vec![at(DispatchAction::SetPvCurtail(pct / 100.0))],
        ActionSpec::ClearOverride {} => vec![
            at(DispatchAction::SetPvCurtail(0.0)),
            at(DispatchAction::SetManualSetpoint(0.0)),
            at(DispatchAction::SetMode(ControlMode::SelfConsumption)),
        ],
    }
}

/// Send a command to a home/fleet subset.
#[utoipa::path(
    post,
    path = "/v1/dispatch",
    request_body = DispatchRequest,
    responses(
        (status = 202, description = "Command accepted", body = DispatchResponse),
        (status = 400, description = "Validation error", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown target", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Idempotency conflict", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Empty target set", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "dispatch"
)]
pub async fn dispatch(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: Option<axum::Extension<Principal>>,
    ValidJson(req): ValidJson<DispatchRequest>,
) -> ApiResult<Response> {
    dispatch_inner(state, headers, req, principal).await
}

/// Shared dispatch path (also used by the fleet alias).
pub async fn dispatch_inner(
    state: AppState,
    headers: HeaderMap,
    req: DispatchRequest,
    principal: Option<axum::Extension<Principal>>,
) -> ApiResult<Response> {
    let hash = body_hash(&req);
    let key = idempotency_key(&headers);
    let principal = principal_of(principal.as_ref().map(|axum::Extension(p)| p));
    let key_for_store = key.clone();
    idempotent(&state, key.as_deref(), hash, || {
        let state = &state;
        async move {
            let resp =
                dispatch_execute(state, &req, &principal, key_for_store.as_deref(), hash).await?;
            Ok((StatusCode::ACCEPTED, resp))
        }
    })
    .await
}

async fn dispatch_execute(
    state: &AppState,
    req: &DispatchRequest,
    principal: &str,
    idem_key: Option<&str>,
    hash: u64,
) -> ApiResult<DispatchResponse> {
    let command_id = req
        .command_id
        .clone()
        .unwrap_or_else(|| ids::new_id(ids::COMMAND));

    // Command-id deduplication: a retried command returns its original
    // acceptance without re-enqueueing.
    if let Ok(audit) = state.audit.read() {
        if let Some(existing) = audit.get(&command_id) {
            return Ok(DispatchResponse {
                command_id: command_id.clone(),
                accepted: true,
                targets: existing.targets.len(),
                status: existing.status,
                status_url: format!("/v1/dispatch/commands/{command_id}"),
            });
        }
    }

    if let Some(exec) = &req.execution {
        if let Some(LatencySpec::Range { uniform }) = exec.latency_ms {
            if uniform[0] > uniform[1] {
                return Err(Problem::validation("latency range must be ordered"));
            }
        }
    }
    let req_kw = requested_kw(&req.action)?;
    if let Some(pct) = req.target.sample_pct {
        if !(pct.is_finite() && (0.0..=100.0).contains(&pct) && pct > 0.0) {
            return Err(Problem::validation("sample_pct must be within (0, 100]"));
        }
    }
    if let Some(filter) = &req.target.filter {
        if let Some(floor) = filter.soc_gt {
            if !(0.0..=1.0).contains(&floor) {
                return Err(Problem::validation("filter.soc_gt must be within 0..=1"));
            }
        }
    }

    // Resolve the target set.
    let target_ids = resolve_target_ids(state, req)?;

    // Filters and deterministic sampling.
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let resolved = filter_targets(state, req, &command_id, &target_ids).await?;

    // Expand per-target executions with jittered latency.
    let tag = command_tag(&command_id);
    let timeout_s = req.execution.and_then(|e| e.timeout_s).unwrap_or(30);
    let dt_s = status.dt_s;
    let mut items = Vec::with_capacity(resolved.len());
    let mut targets = Vec::with_capacity(resolved.len());
    for (pos, (home_id, idx)) in resolved.iter().enumerate() {
        let latency = latency_ms(req.execution.as_ref(), &command_id, home_id);
        let lag_ticks = latency.div_ceil(1000 * u64::from(dt_s));
        let execute_at = status.tick + lag_ticks;
        let actions = actions_for(&req.action, execute_at, dt_s, tag);
        items.push(crate::engine::DispatchItem {
            home_idx: *idx,
            tag,
            command_id: command_id.clone(),
            target_pos: pos,
            execute_at_tick: execute_at,
            timeout_ticks: timeout_s.div_ceil(u64::from(dt_s)),
            requested_kw: req_kw,
            actions,
        });
        targets.push(TargetExecution {
            home_id: home_id.clone(),
            status: None,
            requested_kw: req_kw,
            applied_kw: None,
            executed_at_sim_time: None,
            latency_ms: latency,
        });
    }

    // Audit first (the response is derived from it), then enqueue.
    let record = state.command_record(
        command_id.clone(),
        principal.to_owned(),
        idem_key.map(str::to_owned),
        format!("{hash:016x}"),
        req.clone(),
        targets,
    );
    let n_targets = record.targets.len();
    if let Ok(mut audit) = state.audit.write() {
        audit.insert(record);
    }
    state
        .engine
        .call(|tx| EngineMsg::EnqueueDispatch {
            items,
            reply: tx,
        })
        .await?;

    Ok(DispatchResponse {
        command_id: command_id.clone(),
        accepted: true,
        targets: n_targets,
        status: CommandStatus::Queued,
        status_url: format!("/v1/dispatch/commands/{command_id}"),
    })
}

/// Audit log.
#[utoipa::path(
    get,
    path = "/v1/dispatch/commands",
    params(CommandListParams),
    responses(
        (status = 200, description = "Page of commands", body = CommandsPage),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "dispatch"
)]
pub async fn list_commands(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<CommandListParams>,
) -> ApiResult<Json<CommandsPage>> {
    let limit = crate::model::PageParams {
        limit: q.limit,
        cursor: None,
    }
    .limit()?;
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let since_unix = q
        .since
        .as_deref()
        .map(|s| {
            crate::engine::unix_of(s).map_err(|e| Problem::validation(format!("since: {e}")))
        })
        .transpose()?;
    let audit = state.audit.read().map_err(|_| Problem::internal())?;
    let mut ids: Vec<String> = audit
        .records()
        .iter()
        .filter(|r| q.status.is_none_or(|s| r.status == s))
        .filter(|r| {
            q.target
                .as_ref()
                .is_none_or(|t| r.targets.iter().any(|x| &x.home_id == t))
        })
        .filter(|r| {
            since_unix.is_none_or(|s| {
                crate::engine::unix_of(&r.created_at).map_or(true, |c| c >= s)
            })
        })
        .map(|r| r.command_id.clone())
        .collect();
    ids.sort();
    let (page, has_more) = page_ids(&ids, cursor.as_deref(), limit);
    let data: Vec<CommandDoc> = page
        .iter()
        .filter_map(|id| audit.get(id))
        .cloned()
        .collect();
    let next_cursor = has_more.then(|| encode_cursor(page.last().map_or("", String::as_str)));
    Ok(Json(CommandsPage {
        data,
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

/// Command status and per-target execution detail.
#[utoipa::path(
    get,
    path = "/v1/dispatch/commands/{command_id}",
    params(("command_id" = String, Path, description = "Command id")),
    responses(
        (status = 200, description = "The command", body = CommandDoc),
        (status = 404, description = "Unknown command", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "dispatch"
)]
pub async fn get_command(
    State(state): State<AppState>,
    Path(command_id): Path<String>,
) -> ApiResult<Json<CommandDoc>> {
    let audit = state.audit.read().map_err(|_| Problem::internal())?;
    let rec = audit
        .get(&command_id)
        .ok_or_else(|| Problem::not_found("command", &command_id))?;
    Ok(Json(rec.clone()))
}

/// Cancel a command's still-queued targets.
#[utoipa::path(
    delete,
    path = "/v1/dispatch/commands/{command_id}",
    params(("command_id" = String, Path, description = "Command id")),
    responses(
        (status = 200, description = "Command cancelled", body = OkDoc),
        (status = 404, description = "Unknown command", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 409, description = "Command already finished", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "dispatch"
)]
pub async fn cancel_command(
    State(state): State<AppState>,
    Path(command_id): Path<String>,
) -> ApiResult<Response> {
    {
        let audit = state.audit.read().map_err(|_| Problem::internal())?;
        let rec = audit
            .get(&command_id)
            .ok_or_else(|| Problem::not_found("command", &command_id))?;
        if matches!(
            rec.status,
            CommandStatus::Completed | CommandStatus::CompletedWithErrors | CommandStatus::Cancelled
        ) {
            return Err(Problem::conflict(format!(
                "command {command_id} is already {:?}",
                rec.status
            )));
        }
    }
    let cancelled = state
        .engine
        .call(|tx| EngineMsg::CancelCommand {
            tag: command_tag(&command_id),
            command_id: command_id.clone(),
            reply: tx,
        })
        .await?;
    // Targets the engine no longer tracks (already executed) keep their
    // outcomes; everything else is cancelled.
    if let Ok(mut audit) = state.audit.write() {
        audit.mark_queued_cancelled(&command_id);
        let _ = audit.rollup(&command_id);
        let _ = cancelled;
    }
    Ok((StatusCode::OK, Json(OkDoc { ok: true })).into_response())
}
