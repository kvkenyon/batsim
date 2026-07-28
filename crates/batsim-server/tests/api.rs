//! Route-level integration tests over an in-process server.
//!
//! Every endpoint's happy path plus each documented problem type is
//! exercised through the real router (no mocks).

#![allow(clippy::unwrap_used, clippy::expect_used, unused_variables)]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::time::SimClock;
use batsim_registry::Registry;
use batsim_server::config::Config;
use batsim_server::engine as sim_engine;
use batsim_server::price::PriceSource;
use batsim_server::state::{AppState, AuditStore, IdemStore};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;

fn test_state() -> AppState {
    test_state_with_raw_cap(500)
}

fn test_state_with_raw_cap(raw_stream_max_homes: usize) -> AppState {
    let mut config = Config::default();
    config.engine.raw_stream_max_homes = raw_stream_max_homes;
    let registry = Registry::embedded().expect("embedded catalog");
    let world = SimWorld::new(
        SimClock::from_rfc3339("2025-01-01T00:00:00Z", 1).unwrap(),
        1,
        AmbientFeed::Constant(25.0),
    )
    .unwrap();
    let audit = Arc::new(RwLock::new(AuditStore::new(1000)));
    let (engine, events) = sim_engine::spawn(
        world,
        1.0,
        PriceSource::default_feed(),
        3600,
        1440,
        raw_stream_max_homes,
        128,
        audit.clone(),
    )
    .expect("engine thread");
    AppState {
        config: Arc::new(config),
        registry: Arc::new(registry),
        engine,
        events,
        homes: Arc::new(RwLock::new(HashMap::new())),
        fleets: Arc::new(RwLock::new(HashMap::new())),
        scenarios: Arc::new(RwLock::new(HashMap::new())),
        backtests: Arc::new(RwLock::new(HashMap::new())),
        active_scenario: Arc::new(RwLock::new(None)),
        audit,
        idempotency: Arc::new(RwLock::new(IdemStore::new(24))),
        started: std::time::Instant::now(),
        compose_lock: Arc::new(tokio::sync::Mutex::new(())),
    }
}

struct App {
    router: axum::Router,
}

impl App {
    fn new() -> Self {
        Self {
            router: batsim_server::build_router(test_state()),
        }
    }

    fn with_state(state: AppState) -> Self {
        Self {
            router: batsim_server::build_router(state),
        }
    }

    async fn call(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    /// GET without consuming the body (for infinite SSE responses).
    async fn get_status(&self, path: &str) -> StatusCode {
        let resp = self
            .router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        resp.status()
    }

    async fn post(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        self.call(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    async fn post_raw(&self, path: &str, raw: &str) -> (StatusCode, Value) {
        self.call(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(raw.to_owned()))
                .unwrap(),
        )
        .await
    }

    async fn delete(&self, path: &str) -> (StatusCode, Value) {
        self.call(Request::delete(path).body(Body::empty()).unwrap())
            .await
    }
}

fn home_body() -> Value {
    json!({
        "battery": {"model_id": "tesla.powerwall_3", "count": 1},
        "pv": {"peak_kw": 8.0},
        "load": {"archetype": "sfh_family"},
        "location": {"ercot_load_zone": "LZ_NORTH"},
        "initial_soc": 0.5
    })
}

fn fleet_body(count: u32) -> Value {
    json!({
        "name": "test-fleet",
        "seed": 42,
        "archetypes": [{
            "weight": 1.0,
            "template": {
                "battery": {"model_id": "tesla.powerwall_3", "count": 1},
                "pv": {"peak_kw": {"uniform": [5.0, 8.0]}},
                "load": {"archetype": "sfh_family"}
            }
        }],
        "geo": {"ercot_load_zones": {"LZ_NORTH": 1.0}},
        "count": count
    })
}

fn scenario_body() -> Value {
    json!({
        "name": "test-day",
        "time": {"start": "2025-06-15T00:00:00Z", "end": "2025-06-16T00:00:00Z"},
        "prices": {"source": "static", "price_per_mwh": 50.0},
        "seed": 99
    })
}

async fn start_paused(app: &App) {
    let (s, _) = app.post("/v1/sim:start", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = app.post("/v1/sim:pause", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn system_endpoints() {
    let app = App::new();
    let (s, b) = app.get("/v1/system/health").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "ok");
    let (s, b) = app.get("/v1/system/version").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["openapi_version"], "3.1.0");
    let (s, _) = app.get("/v1/system/config").await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.get("/openapi.json").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["openapi"], "3.1.0");
}

#[tokio::test]
async fn registry_endpoints() {
    let app = App::new();
    let (s, b) = app.get("/v1/registry/batteries").await;
    assert_eq!(s, StatusCode::OK);
    let n = b["data"].as_array().unwrap().len();
    assert!(n >= 10);
    let (s, b) = app.get("/v1/registry/batteries?vendor=Tesla").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["data"].as_array().unwrap().len() < n);
    let (s, b) = app.get("/v1/registry/batteries/tesla.powerwall_3").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["model_id"], "tesla.powerwall_3");
    let (s, b) = app.get("/v1/registry/batteries/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["code"], "NOT_FOUND");
    let (s, b) = app.get("/v1/registry/inverters?min_power_kw=10").await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.get("/v1/registry/version").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["catalog_sha256"].as_str().unwrap().len(), 64);
    // Unknown query parameters are rejected.
    let (s, b) = app.get("/v1/registry/batteries?bogus=1").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn home_crud() {
    let app = App::new();
    let (s, b) = app.post("/v1/homes", &home_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let id = b["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("home_"));
    assert_eq!(b["state"]["soc"], 0.5);

    let (s, b) = app.get(&format!("/v1/homes/{id}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["config"]["battery"]["model_id"], "tesla.powerwall_3");

    let (s, b) = app.get("/v1/homes?limit=10").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);

    let (s, b) = app.get("/v1/homes/home_missing").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(b["code"], "NOT_FOUND");

    // Unknown battery model -> validation error.
    let mut bad = home_body();
    bad["battery"]["model_id"] = json!("not.a.model");
    let (s, b) = app.post("/v1/homes", &bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");

    // Malformed JSON -> 400.
    let (s, b) = app.post_raw("/v1/homes", "{not json").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");

    // Patch requires a non-running simulation.
    start_paused(&app).await;
    let (s, b) = app
        .call(
            Request::patch(format!("/v1/homes/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"mode": "backup-only", "reserve_soc": 0.6}).to_string(),
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["state"]["mode"], "backup-only");

    // Patch while running -> 409 sim-running.
    let (s, _) = app.post("/v1/sim:resume", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app
        .call(
            Request::patch(format!("/v1/homes/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"reserve_soc": 0.5}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_RUNNING");
    let _ = app.post("/v1/sim:pause", &Value::Null).await;

    // Delete while running is rejected; stopped it works.
    let (s, _) = app.post("/v1/sim:resume", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.delete(&format!("/v1/homes/{id}")).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_RUNNING");
    let _ = app.post("/v1/sim:stop", &Value::Null).await;
    let (s, _) = app.delete(&format!("/v1/homes/{id}")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = app.get(&format!("/v1/homes/{id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fleet_lifecycle() {
    let app = App::new();
    let (s, b) = app.post("/v1/fleets", &fleet_body(10)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let fleet_id = b["id"].as_str().unwrap().to_owned();
    assert_eq!(b["home_count"], 10);

    // Expansion is deterministic: same manifest, same hash.
    let hash = b["expansion_hash"].as_str().unwrap().to_owned();
    let (s, b2) = app.post("/v1/fleets", &fleet_body(10)).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b2["expansion_hash"], hash);

    // Homes are queryable by fleet.
    let (s, b) = app.get(&format!("/v1/homes?fleet_id={fleet_id}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 10);

    // Expand adds homes deterministically.
    let (s, b) = app
        .post(
            &format!("/v1/fleets/{fleet_id}:expand"),
            &json!({"count": 5}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["home_count"], 15);

    // Bad manifests.
    let (s, b) = app
        .post(
            "/v1/fleets",
            &json!({"name": "x", "seed": 1, "archetypes": [], "count": 1}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
    let (s, _) = app.get("/v1/fleets/flt_missing").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Delete requires stopped sim; clean here.
    let (s, _) = app.delete(&format!("/v1/fleets/{fleet_id}")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = app.get(&format!("/v1/fleets/{fleet_id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scenario_lifecycle() {
    let app = App::new();
    let (s, b) = app.post("/v1/scenarios", &scenario_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let scn = b["id"].as_str().unwrap().to_owned();

    // Unaligned start -> 400.
    let mut bad = scenario_body();
    bad["time"]["start"] = json!("2025-06-15T00:02:00Z");
    let (s, b) = app.post("/v1/scenarios", &bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");

    // Replay prices are honest 422s until replay data support lands.
    let mut replay = scenario_body();
    replay["prices"] = json!({"source": "replay", "date_range": ["2021-02-14", "2021-02-15"]});
    let (s, b) = app.post("/v1/scenarios", &replay).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(b["code"], "UNPROCESSABLE");

    // Activation requires a stopped simulation.
    start_paused(&app).await;
    let (s, b) = app
        .post(&format!("/v1/scenarios/{scn}:activate"), &Value::Null)
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_RUNNING");
    let _ = app.post("/v1/sim:stop", &Value::Null).await;
    let (s, b) = app
        .post(&format!("/v1/scenarios/{scn}:activate"), &Value::Null)
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["active"], true);

    let (s, b) = app.get("/v1/sim:status").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["sim_time"], "2025-06-15T00:00:00Z");
    assert_eq!(b["active_scenario"], scn);

    let (s, _) = app
        .post(&format!("/v1/scenarios/{scn}:deactivate"), &Value::Null)
        .await;
    assert_eq!(s, StatusCode::OK);
    // Second deactivation conflicts.
    let (s, b) = app
        .post(&format!("/v1/scenarios/{scn}:deactivate"), &Value::Null)
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "CONFLICT");
}

#[tokio::test]
async fn sim_time_control() {
    let app = App::new();
    let (s, b) = app.get("/v1/sim:status").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["state"], "stopped");

    // Pause while stopped -> sim-not-running.
    let (s, b) = app.post("/v1/sim:pause", &Value::Null).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_NOT_RUNNING");

    // Step while stopped -> sim-not-running.
    let (s, b) = app.post("/v1/sim:step", &json!({"ticks": 10})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_NOT_RUNNING");

    let (s, _) = app.post("/v1/sim:start", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    // Double start -> sim-running.
    let (s, b) = app.post("/v1/sim:start", &Value::Null).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_RUNNING");

    // Step while running -> sim-running.
    let (s, b) = app.post("/v1/sim:step", &json!({"ticks": 10})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "SIM_RUNNING");

    let (s, _) = app.post("/v1/sim:pause", &Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.post("/v1/sim:step", &json!({"ticks": 60})).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["ticks_executed"], 60);

    // Run-until must be in the future.
    let (s, b) = app
        .post(
            "/v1/sim:run-until",
            &json!({"until": "2020-01-01T00:00:00Z"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = app
        .post(
            "/v1/sim:run-until",
            &json!({"until": "2025-01-01T00:10:00Z"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // Speed validation.
    let (s, b) = app
        .call(
            Request::put("/v1/sim:speed")
                .header("content-type", "application/json")
                .body(Body::from(json!({"multiplier": -1.0}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");

    // Step cap without allow_large.
    let (s, b) = app.post("/v1/sim:step", &json!({"ticks": 90000})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dispatch_flow() {
    let app = App::new();
    let (s, b) = app.post("/v1/fleets", &fleet_body(5)).await;
    assert_eq!(s, StatusCode::CREATED);
    let fleet_id = b["id"].as_str().unwrap().to_owned();
    let (s, b) = app.post("/v1/scenarios", &scenario_body()).await;
    let scn = b["id"].as_str().unwrap().to_owned();
    let _ = app
        .post(&format!("/v1/scenarios/{scn}:activate"), &Value::Null)
        .await;
    start_paused(&app).await;

    // Empty target -> 400.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({"target": {}, "action": {"type": "clear_override"}}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Unknown fleet -> 404.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({"target": {"fleet_id": "flt_nope"}, "action": {"type": "clear_override"}}),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Bad action values -> 400.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({"target": {"fleet_id": fleet_id}, "action": {"type": "discharge_to", "kw": -1.0}}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Reserve command applies to every home.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({
                "command_id": "cmd_test_reserve",
                "target": {"fleet_id": fleet_id},
                "action": {"type": "set_reserve_soc", "soc": 0.4},
                "execution": {"latency_ms": 100}
            }),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    assert_eq!(b["targets"], 5);

    let (s, _) = app.post("/v1/sim:step", &json!({"ticks": 5})).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.get("/v1/dispatch/commands/cmd_test_reserve").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "completed");
    assert!(b["targets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["status"] == "applied"));

    // Command id dedup: resubmission does not re-enqueue.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({
                "command_id": "cmd_test_reserve",
                "target": {"fleet_id": fleet_id},
                "action": {"type": "set_reserve_soc", "soc": 0.9}
            }),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(b["status"], "completed");

    // Audit log is queryable.
    let (s, b) = app.get("/v1/dispatch/commands?status=completed").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1);
    let (s, _) = app.get("/v1/dispatch/commands/cmd_missing").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Cancel a still-queued command.
    let (s, b) = app
        .post(
            "/v1/dispatch",
            &json!({
                "command_id": "cmd_test_cancel",
                "target": {"fleet_id": fleet_id},
                "action": {"type": "curtail_pv", "pct": 50.0},
                "execution": {"latency_ms": 60000}
            }),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, _) = app.delete("/v1/dispatch/commands/cmd_test_cancel").await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = app.get("/v1/dispatch/commands/cmd_test_cancel").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "cancelled");
}

#[tokio::test]
async fn dispatch_idempotency() {
    let app = App::new();
    let (s, b) = app.post("/v1/fleets", &fleet_body(3)).await;
    assert_eq!(s, StatusCode::CREATED);
    let fleet_id = b["id"].as_str().unwrap().to_owned();

    let body = json!({
        "target": {"fleet_id": fleet_id},
        "action": {"type": "set_reserve_soc", "soc": 0.3}
    });
    let req = |key: &str| {
        Request::post("/v1/dispatch")
            .header("content-type", "application/json")
            .header("Idempotency-Key", key)
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (s, _) = app.call(req("key-1")).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    // Replay with the same body returns the stored response.
    let resp = app.router.clone().oneshot(req("key-1")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(resp.headers().get("idempotent-replay").unwrap(), "true");
    // Same key with a different body -> 409 idempotency-key-reuse.
    let mut other = body.clone();
    other["action"] = json!({"type": "set_reserve_soc", "soc": 0.8});
    let (s, b) = app
        .call(
            Request::post("/v1/dispatch")
                .header("content-type", "application/json")
                .header("Idempotency-Key", "key-1")
                .body(Body::from(other.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b["code"], "IDEMPOTENCY_KEY_REUSE");
}

#[tokio::test]
async fn telemetry_series() {
    let app = App::new();
    let (s, b) = app.post("/v1/fleets", &fleet_body(3)).await;
    assert_eq!(s, StatusCode::CREATED);
    let fleet_id = b["id"].as_str().unwrap().to_owned();
    let (s, b) = app.post("/v1/scenarios", &scenario_body()).await;
    let scn = b["id"].as_str().unwrap().to_owned();
    let _ = app
        .post(&format!("/v1/scenarios/{scn}:activate"), &Value::Null)
        .await;
    start_paused(&app).await;
    let (s, _) = app.post("/v1/sim:step", &json!({"ticks": 600})).await;
    assert_eq!(s, StatusCode::OK);

    let (s, b) = app
        .get(&format!("/v1/homes?fleet_id={fleet_id}&limit=1"))
        .await;
    let home_id = b["data"][0]["id"].as_str().unwrap().to_owned();

    let (s, b) = app
        .get(&format!(
            "/v1/telemetry/homes/{home_id}/series?fields=soc,battery_power_kw&resolution=1m"
        ))
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["fields"], json!(["soc", "battery_power_kw"]));
    assert!(b["t"].as_array().unwrap().len() >= 5);
    assert_eq!(b["v"][0].as_array().unwrap().len(), 2);

    // 5-minute buckets land on settlement boundaries.
    let (s, b) = app
        .get(&format!(
            "/v1/telemetry/homes/{home_id}/series?resolution=5m"
        ))
        .await;
    assert_eq!(s, StatusCode::OK);
    for t in b["t"].as_array().unwrap() {
        let ts = t.as_str().unwrap();
        let unix = batsim_server::engine::unix_of(ts).unwrap();
        assert_eq!(unix % 300, 0, "bucket start {ts} not 5-min aligned");
    }

    // Unknown field -> 400.
    let (s, b) = app
        .get(&format!(
            "/v1/telemetry/homes/{home_id}/series?fields=bogus"
        ))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Fleet series.
    let (s, b) = app
        .get(&format!(
            "/v1/telemetry/fleets/{fleet_id}/series?fields=battery_power_kw,price_rtm&resolution=5m&agg=sum"
        ))
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let price_col = &b["v"][0][1];
    assert_eq!(*price_col, json!(50.0));
    let (s, _) = app.get("/v1/telemetry/fleets/flt_nope/series").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = app.get("/v1/telemetry/homes/home_nope/series").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn telemetry_stream_validation() {
    let app = App::new();
    // Unknown fleet -> 404.
    let (s, _) = app.get("/v1/telemetry/stream?fleet_id=flt_nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Unknown fields mode -> 400.
    let (s, b) = app.get("/v1/telemetry/stream?fields=bogus").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
    // home_ids requires fields=raw.
    let (s, b) = app.get("/v1/telemetry/stream?home_ids=h1").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
    // fleet_id and home_ids are mutually exclusive.
    let (s, b) = app
        .get("/v1/telemetry/stream?fields=raw&fleet_id=flt_x&home_ids=h1")
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
    // Comma-separated home_ids over the 500-entry cap -> 400.
    let too_many = (0..=500)
        .map(|i| format!("h{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let (s, b) = app
        .get(&format!(
            "/v1/telemetry/stream?fields=raw&home_ids={too_many}"
        ))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["code"], "VALIDATION_ERROR");
    // Unknown home id -> 404.
    let (s, _) = app
        .get("/v1/telemetry/stream?fields=raw&home_ids=home_nope")
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // A world larger than the raw-stream cap -> 422.
    let small = App::with_state(test_state_with_raw_cap(1));
    let (s, b) = small.post("/v1/homes", &home_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let home_a = b["id"].as_str().unwrap().to_owned();
    let (s, b) = small.post("/v1/homes", &home_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = small.get("/v1/telemetry/stream?fields=raw").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(b["code"], "UNPROCESSABLE");
    // Comma-separated home_ids parse: a known home subscribes fine.
    let s = small
        .get_status(&format!(
            "/v1/telemetry/stream?fields=raw&home_ids={home_a}"
        ))
        .await;
    // Two homes exceed the cap of 1, so even a filtered raw subscribe
    // is refused; the parse itself succeeded (not a 400).
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Once the world grows past the raw-stream cap mid-subscription, tick
/// events keep flowing without `homes` and a one-time gap notice names
/// the suspension; the stream never goes silent.
#[tokio::test]
async fn telemetry_stream_raw_mid_stream_growth() {
    let app = App::with_state(test_state_with_raw_cap(1));
    let (s, b) = app.post("/v1/homes", &home_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::get("/v1/telemetry/stream?fields=raw")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();

    start_paused(&app).await;
    let (s, _) = app.post("/v1/sim:step", &json!({"ticks": 2})).await;
    assert_eq!(s, StatusCode::OK);
    // Grow the world past the cap, then tick again.
    let (s, b) = app.post("/v1/homes", &home_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, _) = app.post("/v1/sim:step", &json!({"ticks": 3})).await;
    assert_eq!(s, StatusCode::OK);

    // Drain whatever the stream produced within a short window.
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let frame = tokio::time::timeout_at(deadline, body.frame()).await;
        match frame {
            Ok(Some(Ok(frame))) => {
                if let Ok(data) = frame.into_data() {
                    text.push_str(&String::from_utf8_lossy(&data));
                }
            }
            Ok(Some(Err(_)) | None) | Err(_) => break,
        }
        if text.matches("raw_home_rows_suspended").count() >= 1
            && text.matches("event: tick").count() >= 5
        {
            break;
        }
    }

    // Before the growth: tick events carried raw home rows.
    // After it: tick events carry fleet rollups instead, and exactly
    // one gap notice explains the suspension.
    assert!(text.contains("\"homes\""), "pre-growth raw rows: {text}");
    assert!(text.contains("\"fleets\""), "post-growth rollups: {text}");
    assert_eq!(
        text.matches("raw_home_rows_suspended").count(),
        1,
        "one-time gap notice: {text}"
    );
    assert_eq!(text.matches("event: gap").count(), 1, "{text}");
    // The suspended tick events omit the homes field: the last tick
    // event in the stream has fleets but no homes.
    let last_tick = text.rmatch_indices("event: tick").next().unwrap().0;
    let tail = &text[last_tick..];
    assert!(tail.contains("\"fleets\""), "{tail}");
    assert!(!tail.contains("\"homes\""), "{tail}");
}
