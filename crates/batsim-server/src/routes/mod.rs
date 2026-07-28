//! Router assembly plus shared handler plumbing: validated extractors,
//! cursor pagination, idempotency, and auth.
//!
//! Handlers return [`crate::problem::Problem`] as a value for every
//! failure; per-handler `# Errors` sections would repeat the problem
//! model docs dozens of times, so the lint is relaxed module-wide.
#![allow(clippy::missing_errors_doc)]

pub mod backtests;
pub mod dispatch;
pub mod fleets;
pub mod homes;
pub mod registry;
pub mod scenarios;
pub mod sim;
pub mod system;
pub mod telemetry;

use std::future::Future;

use axum::body::Body;
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Method, Request as HttpRequest, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::{GeneralPurpose, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::Serialize;
use subtle::ConstantTimeEq;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa_swagger_ui::SwaggerUi;
use xxhash_rust::xxh3::xxh3_64;

use crate::problem::{ApiResult, Problem, ProblemCode};
use crate::state::AppState;

/// Build the full HTTP router.
pub fn build_router(state: AppState) -> Router {
    let v1 = Router::new()
        .without_v07_checks()
        .nest("/registry", registry::router())
        .nest("/homes", homes::router())
        .nest("/fleets", fleets::router())
        .nest("/scenarios", scenarios::router())
        .nest("/backtests", backtests::router())
        .merge(sim::router())
        .nest("/dispatch", dispatch::router())
        .nest("/telemetry", telemetry::router())
        .nest("/system", system::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let cors = if state.config.server.cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = state
            .config
            .server
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    Router::new()
        .without_v07_checks()
        .nest("/v1", v1)
        .route("/openapi.yaml", get(serve_openapi_yaml))
        .merge(
            SwaggerUi::new("/docs").url("/openapi.json", crate::openapi_document(&state.registry)),
        )
        .fallback(|| async { Problem::not_found("route", "") })
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn serve_openapi_yaml(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    match crate::openapi_document(&state.registry).to_yaml() {
        Ok(s) => ([(CONTENT_TYPE, "application/yaml")], s).into_response(),
        Err(_) => Problem::internal().into_response(),
    }
}

// ---------- extractors ----------

/// JSON body extractor that maps every rejection to a 400 problem.
/// Bodies must be JSON objects: serde otherwise accepts positional
/// arrays for structs, which no endpoint here intends.
pub struct ValidJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Problem;

    async fn from_request(req: HttpRequest<Body>, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<serde_json::Value>::from_request(req, state).await {
            Ok(Json(v)) => {
                if !v.is_object() && !v.is_null() {
                    return Err(Problem::validation("request body must be a JSON object"));
                }
                serde_json::from_value(v).map(Self).map_err(|e| {
                    Problem::validation(format!("Failed to deserialize the JSON body: {e}"))
                })
            }
            Err(rejection) => Err(Problem::validation(rejection.body_text())),
        }
    }
}

/// Query extractor that maps every rejection to a 400 problem.
pub struct ValidQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ValidQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(v)) => Ok(Self(v)),
            Err(rejection) => Err(Problem::validation(rejection.body_text())),
        }
    }
}

// ---------- auth ----------

/// The requesting principal, inserted by the auth middleware.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

/// Optional API-key auth. With no keys configured every request is
/// principal `local` (single-tenant default). Read-only keys may only
/// call GET/HEAD.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth = &state.config.auth;
    if auth.api_keys.is_empty() && auth.read_only_keys.is_empty() {
        req.extensions_mut().insert(Principal("local".to_owned()));
        return next.run(req).await;
    }
    let key = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| req.headers().get("x-api-key").and_then(|v| v.to_str().ok()))
        .map(str::to_owned);
    let Some(key) = key else {
        return Problem::new(
            ProblemCode::Unauthorized,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
        )
        .detail("missing credentials")
        .into_response();
    };
    if auth
        .api_keys
        .iter()
        .any(|k| k.as_bytes().ct_eq(key.as_bytes()).into())
    {
        req.extensions_mut().insert(Principal(format!(
            "key:{}",
            crate::config::fingerprint(&key)
        )));
        return next.run(req).await;
    }
    if auth
        .read_only_keys
        .iter()
        .any(|k| k.as_bytes().ct_eq(key.as_bytes()).into())
    {
        if req.method() != Method::GET && req.method() != Method::HEAD {
            return Problem::new(
                ProblemCode::Unauthorized,
                StatusCode::FORBIDDEN,
                "Forbidden",
            )
            .detail("read-only key may only call GET/HEAD")
            .into_response();
        }
        req.extensions_mut().insert(Principal(format!(
            "ro:{}",
            crate::config::fingerprint(&key)
        )));
        return next.run(req).await;
    }
    Problem::new(
        ProblemCode::Unauthorized,
        StatusCode::UNAUTHORIZED,
        "Unauthorized",
    )
    .detail("invalid credentials")
    .into_response()
}

/// Extract the principal (middleware always inserts one).
#[must_use]
pub fn principal_of(req: Option<&Principal>) -> String {
    req.map_or_else(|| "local".to_owned(), |p| p.0.clone())
}

// ---------- idempotency ----------

/// Hash of a request body for idempotency comparison.
#[must_use]
pub fn body_hash<T: Serialize>(body: &T) -> u64 {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    xxh3_64(&bytes)
}

/// Run a mutating operation under idempotency-key semantics.
///
/// - No key: execute and return.
/// - Key with same body hash: replay the stored response with
///   `Idempotent-Replay: true`.
/// - Key with a different body: 409 idempotency-key-reuse.
/// - Key with a request still executing: 409 conflict.
///
/// The key is reserved under the store lock before the operation runs,
/// so concurrent requests carrying the same key cannot both mutate.
///
/// # Errors
/// Propagates the operation's [`Problem`].
pub async fn idempotent<T, F, Fut>(
    state: &AppState,
    key: Option<&str>,
    hash: u64,
    produce: F,
) -> ApiResult<Response>
where
    T: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = ApiResult<(StatusCode, T)>>,
{
    let Some(key) = key else {
        let (status, body) = produce().await?;
        return Ok((status, Json(body)).into_response());
    };
    let reservation = {
        let mut store = state.idempotency.write().map_err(|_| Problem::internal())?;
        store.reserve(key, hash)
    };
    match reservation {
        crate::state::IdemReservation::Replay(rec) => {
            let mut resp = (
                StatusCode::from_u16(rec.status).unwrap_or(StatusCode::OK),
                Json(rec.body),
            )
                .into_response();
            resp.headers_mut()
                .insert("idempotent-replay", HeaderValue::from_static("true"));
            Ok(resp)
        }
        crate::state::IdemReservation::ConflictReuse => Err(Problem::new(
            ProblemCode::IdempotencyKeyReuse,
            StatusCode::CONFLICT,
            "Idempotency key reuse",
        )
        .detail("the same idempotency key arrived with a different request body")),
        crate::state::IdemReservation::InFlight => Err(Problem::conflict(
            "a request with this idempotency key is still executing; retry once it completes",
        )),
        crate::state::IdemReservation::Reserved => {
            let guard = IdemAbortGuard {
                store: state.idempotency.clone(),
                key: key.to_owned(),
            };
            let produced = produce().await;
            let mut store = state.idempotency.write().map_err(|_| Problem::internal())?;
            match produced {
                Ok((status, body)) => {
                    let value = serde_json::to_value(&body).map_err(|_| Problem::internal())?;
                    store.complete(
                        key,
                        crate::state::IdemRecord {
                            body_hash: hash,
                            status: status.as_u16(),
                            body: value,
                            created: std::time::Instant::now(),
                        },
                    );
                    drop(store);
                    drop(guard);
                    Ok((status, Json(body)).into_response())
                }
                Err(e) => {
                    store.abort(key);
                    drop(store);
                    drop(guard);
                    Err(e)
                }
            }
        }
    }
}

/// Releases an idempotency reservation on drop; `complete`/`abort`
/// already ran, so the drop-time abort is a no-op on every path except
/// early return or task cancellation, where it prevents a permanently
/// stuck key.
struct IdemAbortGuard {
    store: std::sync::Arc<std::sync::RwLock<crate::state::IdemStore>>,
    key: String,
}

impl Drop for IdemAbortGuard {
    fn drop(&mut self) {
        if let Ok(mut store) = self.store.write() {
            store.abort(&self.key);
        }
    }
}

/// The `Idempotency-Key` header.
#[must_use]
pub fn idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// 405 problem with the RFC 9110 `Allow` header.
#[must_use]
pub fn method_not_allowed_problem(allow: &'static str) -> Problem {
    let mut p = Problem::new(
        ProblemCode::NotFound,
        StatusCode::METHOD_NOT_ALLOWED,
        "Method not allowed",
    );
    p.headers = vec![("allow", allow)];
    p
}

// ---------- pagination ----------

/// Lenient base64url decoder for cursors: any alphabet string whose
/// length is not 1 mod 4 decodes; trailing bits are not inspected.
const CURSOR_ENGINE: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    base64::engine::general_purpose::NO_PAD.with_decode_allow_trailing_bits(true),
);

/// Encode a cursor (opaque, base64url).
#[must_use]
pub fn encode_cursor(id: &str) -> String {
    URL_SAFE_NO_PAD.encode(id.as_bytes())
}

/// Decode a cursor to its raw bytes. Any base64url-decodable value is a
/// well-formed (if meaningless) cursor; anything else is a 400.
///
/// # Errors
/// [`Problem::validation`] on non-base64url input.
pub fn decode_cursor(cursor: &str) -> ApiResult<Vec<u8>> {
    CURSOR_ENGINE
        .decode(cursor.as_bytes())
        .map_err(|_| Problem::validation("malformed cursor"))
}

/// Page a sorted (by id) id list: entries strictly after the cursor
/// (byte-wise comparison).
#[must_use]
pub fn page_ids(ids: &[String], cursor: Option<&[u8]>, limit: usize) -> (Vec<String>, bool) {
    let start = cursor.map_or(0, |c| {
        ids.iter()
            .position(|i| i.as_bytes() > c)
            .unwrap_or(ids.len())
    });
    let end = (start + limit).min(ids.len());
    (ids[start..end].to_vec(), end < ids.len())
}
