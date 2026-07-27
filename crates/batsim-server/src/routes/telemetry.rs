//! Telemetry: historical series plus live SSE/WebSocket streams.

use std::collections::BTreeMap;
use std::convert::Infallible;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use crate::engine::{EngineMsg, SimEvent, TickEvent};
use crate::model::{
    FleetAgg, Resolution, SeriesParams, SeriesResponse, StreamParams, TELEMETRY_FIELDS,
};
use crate::problem::{ApiResult, Problem};
use crate::state::AppState;
use crate::telemetry::HomeBucket;

use super::ValidQuery;

/// Telemetry routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/homes/{id}/series", get(home_series))
        .route("/fleets/{id}/series", get(fleet_series))
        .route("/stream", get(sse_stream))
        .route("/ws", get(ws_stream))
}

/// Parse and validate the `fields` parameter.
fn parse_fields(raw: Option<&str>) -> ApiResult<Vec<String>> {
    match raw {
        None => Ok(TELEMETRY_FIELDS.iter().map(|s| (*s).to_owned()).collect()),
        Some(s) => {
            let mut out = Vec::new();
            for f in s.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                if !TELEMETRY_FIELDS.contains(&f) {
                    return Err(Problem::validation(format!(
                        "unknown field `{f}` (allowed: {})",
                        TELEMETRY_FIELDS.join(", ")
                    )));
                }
                out.push(f.to_owned());
            }
            if out.is_empty() {
                return Err(Problem::validation("fields must not be empty"));
            }
            Ok(out)
        }
    }
}

/// Extract one field value from a bucket (kW for power fields).
fn field_value(field: &str, b: &HomeBucket) -> f64 {
    match field {
        "soc" => b.soc,
        "battery_power_kw" => b.batt_w / 1000.0,
        "pv_power_kw" => b.pv_w / 1000.0,
        "load_power_kw" => b.load_w / 1000.0,
        "grid_power_kw" => b.grid_w / 1000.0,
        "price_rtm" => b.price,
        _ => 0.0,
    }
}

fn parse_range(q: &SeriesParams, now_unix: u64) -> ApiResult<(u64, u64)> {
    let from = q
        .from
        .as_deref()
        .map(|s| crate::engine::unix_of(s).map_err(|e| Problem::validation(format!("from: {e}"))))
        .transpose()?
        .unwrap_or(0);
    let to =
        q.to.as_deref()
            .map(|s| crate::engine::unix_of(s).map_err(|e| Problem::validation(format!("to: {e}"))))
            .transpose()?
            .unwrap_or(now_unix.saturating_add(1));
    if to <= from {
        return Err(Problem::validation("`to` must be after `from`"));
    }
    Ok((from, to))
}

/// Historical series for one home.
#[utoipa::path(
    get,
    path = "/v1/telemetry/homes/{id}/series",
    params(
        ("id" = String, Path, description = "Home id"),
        SeriesParams
    ),
    responses(
        (status = 200, description = "Columnar series", body = SeriesResponse),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown home", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "telemetry"
)]
pub async fn home_series(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ValidQuery(q): ValidQuery<SeriesParams>,
) -> ApiResult<Json<SeriesResponse>> {
    let entry = state
        .home(&id)
        .ok_or_else(|| Problem::not_found("home", &id))?;
    let fields = parse_fields(q.fields.as_deref())?;
    let resolution = q.resolution.unwrap_or(Resolution::M1);
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let (from, to) = parse_range(&q, status.unix)?;
    let buckets = state
        .engine
        .call(|tx| EngineMsg::HomeSeries {
            idx: entry.idx,
            from,
            to,
            bucket_s: resolution.seconds(),
            reply: tx,
        })
        .await?;
    let t = buckets
        .iter()
        .map(|b| crate::engine::rfc3339_of(b.start_unix))
        .collect();
    let v = buckets
        .iter()
        .map(|b| fields.iter().map(|f| field_value(f, b)).collect())
        .collect();
    Ok(Json(SeriesResponse {
        home_id: Some(id),
        fleet_id: None,
        resolution,
        fields,
        t,
        v,
    }))
}

/// Percentile helper (nearest-rank).
fn percentile(sorted: &mut [f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

/// Bound on homes x buckets one fleet-series request may materialize:
/// the engine copies every bucket across the command channel and the
/// handler copies them again for aggregation, so an unbounded request
/// would stall ticks and exhaust memory.
const MAX_FLEET_SERIES_CELLS: u64 = 1_000_000;

/// Aggregated series for a fleet.
#[utoipa::path(
    get,
    path = "/v1/telemetry/fleets/{id}/series",
    params(
        ("id" = String, Path, description = "Fleet id"),
        SeriesParams
    ),
    responses(
        (status = 200, description = "Columnar series", body = SeriesResponse),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown fleet", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "telemetry"
)]
pub async fn fleet_series(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ValidQuery(q): ValidQuery<SeriesParams>,
) -> ApiResult<Json<SeriesResponse>> {
    let fleet = state
        .fleet(&id)
        .ok_or_else(|| Problem::not_found("fleet", &id))?;
    let fields = parse_fields(q.fields.as_deref())?;
    let resolution = q.resolution.unwrap_or(Resolution::M1);
    let agg = q.agg.unwrap_or(FleetAgg::Sum);
    let status = state
        .engine
        .call(|tx| EngineMsg::Status { reply: tx })
        .await?;
    let (from, to) = parse_range(&q, status.unix)?;
    let idxs: Vec<u64> = fleet
        .home_ids
        .iter()
        .filter_map(|h| state.home(h))
        .map(|e| e.idx)
        .collect();
    let span_buckets = (to - from) / resolution.seconds() + 1;
    let secs = resolution.seconds();
    let retained = if secs == 1 {
        state.config.telemetry.raw_ticks as u64
    } else if secs <= 60 {
        state.config.telemetry.rollup_minutes as u64 + 1
    } else {
        state.config.telemetry.rollup_minutes as u64 / (secs / 60) + 2
    };
    let per_home = span_buckets.min(retained);
    let cells = idxs.len() as u64 * per_home;
    if cells > MAX_FLEET_SERIES_CELLS {
        return Err(Problem::validation(format!(
            "fleet series too broad: {} homes x up to {per_home} buckets exceeds the {MAX_FLEET_SERIES_CELLS} cell budget; narrow the range or use a coarser resolution",
            idxs.len()
        )));
    }
    let per_home = state
        .engine
        .call(|tx| EngineMsg::FleetSeries {
            idxs,
            from,
            to,
            bucket_s: resolution.seconds(),
            reply: tx,
        })
        .await?;

    // Align buckets across homes by start time.
    let mut by_start: BTreeMap<u64, Vec<HomeBucket>> = BTreeMap::new();
    for home_buckets in &per_home {
        for b in home_buckets {
            by_start.entry(b.start_unix).or_default().push(*b);
        }
    }
    let mut t = Vec::with_capacity(by_start.len());
    let mut v = Vec::with_capacity(by_start.len());
    for (start, buckets) in by_start {
        t.push(crate::engine::rfc3339_of(start));
        let row = fields
            .iter()
            .map(|f| {
                let mut vals: Vec<f64> = buckets.iter().map(|b| field_value(f, b)).collect();
                // A market price is one signal for the whole fleet, not a
                // per-home quantity; it never sums.
                if f == "price_rtm" {
                    if vals.is_empty() {
                        0.0
                    } else {
                        vals.iter().sum::<f64>() / vals.len() as f64
                    }
                } else {
                    match agg {
                        FleetAgg::Sum => vals.iter().sum(),
                        FleetAgg::Mean => {
                            if vals.is_empty() {
                                0.0
                            } else {
                                vals.iter().sum::<f64>() / vals.len() as f64
                            }
                        }
                        FleetAgg::P95 => percentile(&mut vals, 95.0),
                    }
                }
            })
            .collect();
        v.push(row);
    }
    Ok(Json(SeriesResponse {
        home_id: None,
        fleet_id: Some(id),
        resolution,
        fields,
        t,
        v,
    }))
}

// ---------- live streams ----------

struct StreamFilter {
    fleet_id: Option<String>,
    home_ids: Option<std::collections::HashSet<String>>,
    raw: bool,
    downsample: u64,
}

impl StreamFilter {
    fn parse(q: &StreamParams, state: &AppState) -> ApiResult<Self> {
        let raw = match q.fields.as_deref() {
            None | Some("aggregate") => false,
            Some("raw") => true,
            Some(other) => {
                return Err(Problem::validation(format!(
                    "fields must be `aggregate` or `raw`, got `{other}`"
                )))
            }
        };
        let requested: Vec<String> = q
            .home_ids
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if !requested.is_empty() {
            if !raw {
                return Err(Problem::validation(
                    "home_ids applies to raw streams only; add `fields=raw`",
                ));
            }
            if requested.len() > 500 {
                return Err(Problem::validation("home_ids accepts at most 500 entries"));
            }
            for id in &requested {
                if state.home(id).is_none() {
                    return Err(Problem::not_found("home", id));
                }
            }
        }
        let mut home_set: std::collections::HashSet<String> = requested.into_iter().collect();
        if let Some(fleet_id) = &q.fleet_id {
            let fleet = state
                .fleet(fleet_id)
                .ok_or_else(|| Problem::not_found("fleet", fleet_id))?;
            home_set.extend(fleet.home_ids.iter().cloned());
        }
        // Raw per-home rows exist only while the whole world is small
        // enough to stream; the engine gates on its active-home count,
        // so the check here must match it exactly.
        if raw {
            let n = state.homes.read().map_err(|_| Problem::internal())?.len();
            if n > state.config.engine.raw_stream_max_homes {
                return Err(Problem::unprocessable(format!(
                    "raw streaming is limited to {} active homes; this world has {n}",
                    state.config.engine.raw_stream_max_homes
                )));
            }
        }
        let downsample = q.downsample.unwrap_or(1);
        if downsample == 0 {
            return Err(Problem::validation("downsample must be >= 1"));
        }
        Ok(Self {
            fleet_id: q.fleet_id.clone(),
            home_ids: (!home_set.is_empty()).then_some(home_set),
            raw,
            downsample,
        })
    }

    /// Project a tick event through this subscriber's filter.
    fn project_tick(&self, ev: &TickEvent) -> Option<serde_json::Value> {
        if ev.tick % self.downsample != 0 {
            return None;
        }
        let mut fleets: Vec<_> = ev
            .fleets
            .iter()
            .filter(|f| {
                self.fleet_id
                    .as_ref()
                    .is_none_or(|want| &f.fleet_id == want)
            })
            .collect();
        if self.fleet_id.is_some() && fleets.is_empty() {
            fleets = Vec::new();
        }
        let mut out = serde_json::json!({
            "sim_time": ev.sim_time,
            "tick": ev.tick,
            "price_rtm": ev.price_rtm,
        });
        if self.raw {
            let rows: Vec<_> = ev
                .homes
                .as_ref()?
                .iter()
                .filter(|r| {
                    self.home_ids
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&r.home_id))
                })
                .collect();
            out["homes"] = serde_json::json!(rows);
        } else {
            out["fleets"] = serde_json::json!(fleets);
        }
        Some(out)
    }
}

/// SSE live stream.
#[utoipa::path(
    get,
    path = "/v1/telemetry/stream",
    params(StreamParams),
    responses(
        (status = 200, description = "text/event-stream of tick and dispatch events"),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown fleet or home", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Raw stream too large", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "telemetry"
)]
pub async fn sse_stream(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<StreamParams>,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>> {
    let filter = StreamFilter::parse(&q, &state)?;
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let event = match item {
            Ok(SimEvent::Tick(tick)) => filter.project_tick(&tick).map(|data| {
                Event::default()
                    .event("tick")
                    .id(tick.tick.to_string())
                    .json_data(data)
                    .unwrap_or_else(|_| Event::default().comment("serialization error"))
            }),
            Ok(SimEvent::Dispatch {
                command_id,
                tick,
                targets_applied,
                targets_rejected,
            }) => {
                let data = serde_json::json!({
                    "command_id": command_id,
                    "targets_applied": targets_applied,
                    "targets_rejected": targets_rejected,
                });
                Some(
                    Event::default()
                        .event("dispatch")
                        .id(tick.to_string())
                        .json_data(data)
                        .unwrap_or_else(|_| Event::default().comment("serialization error")),
                )
            }
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                let data = serde_json::json!({ "missed_events": n });
                Some(
                    Event::default()
                        .event("gap")
                        .json_data(data)
                        .unwrap_or_else(|_| Event::default().comment("serialization error")),
                )
            }
        };
        std::future::ready(event.map(Ok))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// WebSocket live stream (same messages as SSE; subprotocol
/// `batsim.v1+json`).
#[utoipa::path(
    get,
    path = "/v1/telemetry/ws",
    params(StreamParams),
    responses(
        (status = 101, description = "WebSocket upgrade; JSON tick and dispatch messages"),
        (status = 400, description = "Invalid query parameters", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 404, description = "Unknown fleet or home", body = crate::problem::Problem, content_type = "application/problem+json"),
        (status = 422, description = "Raw stream too large", body = crate::problem::Problem, content_type = "application/problem+json"),
    ),
    tag = "telemetry"
)]
pub async fn ws_stream(
    State(state): State<AppState>,
    ValidQuery(q): ValidQuery<StreamParams>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    let filter = StreamFilter::parse(&q, &state)?;
    let rx = state.events.subscribe();
    Ok(ws
        .protocols(["batsim.v1+json"])
        .on_upgrade(move |socket| ws_loop(socket, rx, filter)))
}

async fn ws_loop(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<SimEvent>,
    filter: StreamFilter,
) {
    loop {
        tokio::select! {
            recv = rx.recv() => {
                let payload = match recv {
                    Ok(SimEvent::Tick(tick)) => filter.project_tick(&tick),
                    Ok(SimEvent::Dispatch {
                        command_id,
                        targets_applied,
                        targets_rejected,
                        ..
                    }) => Some(serde_json::json!({
                        "event": "dispatch",
                        "command_id": command_id,
                        "targets_applied": targets_applied,
                        "targets_rejected": targets_rejected,
                    })),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        Some(serde_json::json!({ "event": "gap", "missed_events": n }))
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let Some(payload) = payload else {
                    continue;
                };
                let Ok(text) = serde_json::to_string(&payload) else {
                    continue;
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }
}
