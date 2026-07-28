//! Backtest API integration tests (M3): an end-to-end replay run against a
//! synthetic ERCOT archive, settlement-report invariants, and bit-identical
//! determinism on re-run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::time::SimClock;
use batsim_ercot::ingest::writers::ALL_LOCATION;
use batsim_ercot::ingest::writers::{
    upsert_manifest, write_as_partition, write_price_partition, ManifestMeta,
};
use batsim_ercot::rules::ErcotRules;
use batsim_ercot::schema::{ManifestEntry, SIGNAL_AS_MCPC, SIGNAL_DAM_SPP, SIGNAL_RTM_SPP};
use batsim_ercot::synthetic::{Season, SyntheticParams, SyntheticPriceGenerator};
use batsim_ercot::{Location, PriceSource as _, Provenance, TimeRange};
use batsim_registry::Registry;
use batsim_server::config::Config;
use batsim_server::engine as sim_engine;
use batsim_server::price::PriceSource;
use batsim_server::state::{AppState, AuditStore, IdemStore};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use time::Date;
use tower::ServiceExt as _;

const DAY: &str = "2026-08-14";

/// Generate one synthetic CPT day and write it as a replay archive.
fn synth_archive(root: &std::path::Path) -> (Vec<batsim_ercot::PriceSample>, Date) {
    let rules = ErcotRules::current().unwrap();
    let start = batsim_ercot::cpt::cpt_interval_to_utc(
        Date::from_calendar_date(2026, time::Month::August, 14).unwrap(),
        1,
        1,
        1,
        false,
    )
    .unwrap();
    let end = start + time::Duration::hours(25); // slack past CPT midnight
    let range = TimeRange::new(start, end).unwrap();
    let params = SyntheticParams {
        seed: 42,
        season: Season::Summer,
        solar_penetration: 0.35,
        reserve_margin: 0.10,
        interval_secs: 900,
        location: Location::from_settlement_point("LZ_HOUSTON"),
        ..SyntheticParams::default()
    };
    let gen = SyntheticPriceGenerator::new(params, range, &rules).unwrap();
    let loc = Location::from_settlement_point("LZ_HOUSTON");
    let rtm = gen.rt_spps(&loc, range).unwrap();
    let dam = gen.dam_spps(&loc, range).unwrap();
    let as_prices = gen.as_prices(range).unwrap();
    let op_day = Date::from_calendar_date(2026, time::Month::August, 14).unwrap();

    let mut entries = Vec::new();
    for (signal, rows) in [(SIGNAL_RTM_SPP, &rtm), (SIGNAL_DAM_SPP, &dam)] {
        let path = write_price_partition(root, signal, op_day, &loc, rows, None).unwrap();
        entries.push(ManifestEntry {
            signal: signal.to_string(),
            date: DAY.to_string(),
            location: "LZ_HOUSTON".to_string(),
            path,
            rows: rows.len() as u64,
            provenance: Provenance::Synthetic,
        });
    }
    let as_rows: Vec<_> = as_prices
        .iter()
        .map(|p| (p.ts, p.product, p.mcpc_usd_per_mw, p.provenance))
        .collect();
    let path = write_as_partition(root, op_day, &as_rows).unwrap();
    entries.push(ManifestEntry {
        signal: SIGNAL_AS_MCPC.to_string(),
        date: DAY.to_string(),
        location: ALL_LOCATION.to_string(),
        path,
        rows: as_rows.len() as u64,
        provenance: Provenance::Synthetic,
    });
    upsert_manifest(
        root,
        &ManifestMeta {
            rules_version: rules.meta.protocol_version.clone(),
            source_report: "synthetic".to_string(),
            source_doc_ids: Vec::new(),
            ingested_at: "2026-01-01T00:00:00Z".to_string(),
        },
        &entries,
    )
    .unwrap();
    (rtm, op_day)
}

fn test_state(data_dir: &std::path::Path) -> AppState {
    let config = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
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
        500,
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

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    call(
        app,
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    call(app, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn make_fleet(app: &axum::Router) -> String {
    let (status, body) = post(
        app,
        "/v1/fleets",
        json!({
            "name": "bt-fleet", "seed": 42, "count": 3,
            "archetypes": [{
                "weight": 1.0,
                "template": {
                    "battery": { "model_id": "tesla.powerwall_3", "count": 1 },
                    "pv": { "peak_kw": 8.0 },
                    "load": { "archetype": "sfh_family" }
                }
            }],
            "geo": { "ercot_load_zones": { "LZ_HOUSTON": 1.0 } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "fleet create: {body}");
    body["id"].as_str().unwrap().to_string()
}

async fn run_backtest(app: &axum::Router, fleet_id: &str, request: Value) -> (String, Value) {
    let mut request = request;
    request["fleet_id"] = json!(fleet_id);
    let (status, body) = post(app, "/v1/backtests", request).await;
    assert_eq!(status, StatusCode::ACCEPTED, "backtest start: {body}");
    let id = body["id"].as_str().unwrap().to_string();
    // Poll until settled (debug-build tick rates vary; cap at ~5 minutes).
    for _ in 0..600 {
        let (_, doc) = get(app, &format!("/v1/backtests/{id}")).await;
        match doc["state"].as_str().unwrap_or("") {
            "settled" => break,
            s if s.starts_with("failed") => panic!("backtest failed: {s}"),
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
    let (status, report) = get(app, &format!("/v1/backtests/{id}/settlement")).await;
    assert_eq!(status, StatusCode::OK, "settlement: {report}");
    (id, report)
}

fn base_request() -> Value {
    json!({
        "name": "itest",
        "date": DAY,
        "settlement_point": "LZ_HOUSTON",
        "seed": 7,
        "retail_rate": { "kind": "flat", "usd_per_kwh": 0.12 }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backtest_end_to_end_and_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let (rtm, _day) = synth_archive(&tmp.path().join("ercot"));
    let app = batsim_server::build_router(test_state(tmp.path()));
    let fleet = make_fleet(&app).await;

    // Baseline run.
    let (_, base) = run_backtest(&app, &fleet, base_request()).await;
    assert_eq!(base["settlement_interval_secs"], json!(900));
    assert_eq!(base["intervals"].as_array().unwrap().len(), 96);
    assert_eq!(base["homes"].as_array().unwrap().len(), 3);
    assert_eq!(base["provenance"], json!("synthetic"));
    assert_eq!(base["rules_version"], json!("v2025"));
    // No dispatch: every interval's fleet export is just PV surplus minus
    // load, so wholesale and the ledger are finite and per-home ledgers sum
    // to the fleet totals.
    let home_wholesale: f64 = base["homes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["wholesale_usd"].as_f64().unwrap())
        .sum();
    let fleet_wholesale = base["totals"]["energy"]["wholesale_usd"].as_f64().unwrap();
    assert!(
        (home_wholesale - fleet_wholesale).abs() < 1e-6,
        "home ledgers must sum to fleet wholesale: {home_wholesale} vs {fleet_wholesale}"
    );

    // Heuristic: charge at the cheapest interval, discharge at the most
    // expensive one. Must beat the baseline on wholesale revenue.
    let cheapest = rtm
        .iter()
        .min_by(|a, b| a.spp_usd_per_mwh().total_cmp(&b.spp_usd_per_mwh()))
        .unwrap();
    let priciest = rtm
        .iter()
        .max_by(|a, b| a.spp_usd_per_mwh().total_cmp(&b.spp_usd_per_mwh()))
        .unwrap();
    let fmt = |ts: time::OffsetDateTime| {
        ts.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let mut heuristic = base_request();
    heuristic["strategy"] = json!({
        "kind": "schedule",
        "entries": [
            { "start": fmt(cheapest.ts), "action": "charge", "kw": 5.0, "duration_s": 1800 },
            { "start": fmt(priciest.ts), "action": "discharge", "kw": 5.0, "duration_s": 5400 }
        ]
    });
    let (_, strat) = run_backtest(&app, &fleet, heuristic).await;
    let strat_wholesale = strat["totals"]["energy"]["wholesale_usd"].as_f64().unwrap();
    assert!(
        strat_wholesale > fleet_wholesale,
        "discharging into the peak must beat baseline: {strat_wholesale} vs {fleet_wholesale}"
    );

    // Determinism: an identical re-run is bit-identical modulo the run id.
    let (_, rerun) = run_backtest(&app, &fleet, {
        let mut r = base_request();
        r["strategy"] = json!({
            "kind": "schedule",
            "entries": [
                { "start": fmt(cheapest.ts), "action": "charge", "kw": 5.0, "duration_s": 1800 },
                { "start": fmt(priciest.ts), "action": "discharge", "kw": 5.0, "duration_s": 5400 }
            ]
        });
        r
    })
    .await;
    let mut a = strat.clone();
    let mut b = rerun.clone();
    a["run_id"] = json!("X");
    b["run_id"] = json!("X");
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "re-run must be bit-identical modulo run_id"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backtest_validation_errors() {
    let tmp = tempfile::tempdir().unwrap();
    synth_archive(&tmp.path().join("ercot"));
    let app = batsim_server::build_router(test_state(tmp.path()));

    // Unknown fleet -> 404.
    let (status, body) = post(&app, "/v1/backtests", {
        let mut r = base_request();
        r["fleet_id"] = json!("flt_nope");
        r
    })
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], json!("NOT_FOUND"));

    let fleet = make_fleet(&app).await;

    // Bad date -> 400.
    let (status, _) = post(&app, "/v1/backtests", {
        let mut r = base_request();
        r["fleet_id"] = json!(fleet);
        r["date"] = json!("not-a-date");
        r
    })
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown run -> 404 on status and settlement.
    let (status, _) = get(&app, "/v1/backtests/bt_nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&app, "/v1/backtests/bt_nope/settlement").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
