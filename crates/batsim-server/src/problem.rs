//! RFC 9457 problem details (`application/problem+json`).
//!
//! Every error the API produces uses this shape, with a stable
//! machine-readable `code` (SCREAMING_SNAKE) and a fixed registry of
//! problem `type` URNs. Handlers never hand-roll status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

/// Base URN for problem types.
const PROBLEM_BASE: &str = "https://batsim.dev/problems/";

/// Stable machine-readable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProblemCode {
    /// Request failed validation (malformed body, unknown field, bad
    /// query parameter).
    ValidationError,
    /// Credentials missing or wrong.
    Unauthorized,
    /// The addressed resource does not exist.
    NotFound,
    /// State conflict, including idempotency-key reuse with a different
    /// body.
    Conflict,
    /// Well-formed request violating a physics or composition rule.
    Unprocessable,
    /// The requested time-control transition is illegal while the
    /// simulation is running.
    SimRunning,
    /// The requested time-control transition requires a running or
    /// paused simulation.
    SimNotRunning,
    /// The same idempotency key arrived with a different request body.
    IdempotencyKeyReuse,
    /// An internal error.
    Internal,
}

impl ProblemCode {
    /// The problem type URN (kebab-case sibling of the code).
    #[must_use]
    pub const fn type_slug(self) -> &'static str {
        match self {
            Self::ValidationError => "validation-error",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::Unprocessable => "unprocessable",
            Self::SimRunning => "sim-running",
            Self::SimNotRunning => "sim-not-running",
            Self::IdempotencyKeyReuse => "idempotency-key-reuse",
            Self::Internal => "internal",
        }
    }
}

/// RFC 9457 problem document; the only error body the API emits.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Problem {
    /// Problem type URN.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// Short human-readable summary.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Specifics of this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The request path that produced the problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// Stable machine-readable code.
    pub code: ProblemCode,
    /// Request correlation id (echoed as `X-Request-Id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Extension members (e.g. per-target rejection detail).
    #[serde(flatten)]
    #[schema(ignore)]
    pub extra: serde_json::Map<String, serde_json::Value>,
    /// Extra response headers (not serialized into the body).
    #[serde(skip)]
    #[schema(ignore)]
    pub headers: Vec<(&'static str, &'static str)>,
}

impl Problem {
    /// Build a problem for `code` at `status`.
    #[must_use]
    pub fn new(code: ProblemCode, status: StatusCode, title: impl Into<String>) -> Self {
        Self {
            problem_type: format!("{PROBLEM_BASE}{}", code.type_slug()),
            title: title.into(),
            status: status.as_u16(),
            detail: None,
            instance: None,
            code,
            trace_id: None,
            extra: serde_json::Map::new(),
            headers: Vec::new(),
        }
    }

    /// Attach a detail message.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// 400 validation error.
    #[must_use]
    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(
            ProblemCode::ValidationError,
            StatusCode::BAD_REQUEST,
            "Validation error",
        )
        .detail(detail)
    }

    /// 404 not found.
    #[must_use]
    pub fn not_found(kind: &str, id: &str) -> Self {
        Self::new(ProblemCode::NotFound, StatusCode::NOT_FOUND, "Not found")
            .detail(format!("no {kind} with id `{id}`"))
    }

    /// 409 conflict with a detail message.
    #[must_use]
    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(ProblemCode::Conflict, StatusCode::CONFLICT, "Conflict").detail(detail)
    }

    /// 422 rule violation.
    #[must_use]
    pub fn unprocessable(detail: impl Into<String>) -> Self {
        Self::new(
            ProblemCode::Unprocessable,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unprocessable request",
        )
        .detail(detail)
    }

    /// 409: the simulation is running and the operation requires it not
    /// to be.
    #[must_use]
    pub fn sim_running(detail: impl Into<String>) -> Self {
        Self::new(
            ProblemCode::SimRunning,
            StatusCode::CONFLICT,
            "Simulation is running",
        )
        .detail(detail)
    }

    /// 409: the operation requires a started simulation.
    #[must_use]
    pub fn sim_not_running(detail: impl Into<String>) -> Self {
        Self::new(
            ProblemCode::SimNotRunning,
            StatusCode::CONFLICT,
            "Simulation is not running",
        )
        .detail(detail)
    }

    /// 500 internal error; the cause is logged, not leaked.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(
            ProblemCode::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error",
        )
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let headers = self.headers.clone();
        let mut resp = (status, Json(self)).into_response();
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        for (name, value) in headers {
            if let Ok(v) = axum::http::HeaderValue::from_str(value) {
                resp.headers_mut()
                    .insert(axum::http::header::HeaderName::from_static(name), v);
            }
        }
        resp
    }
}

/// Handler result alias.
pub type ApiResult<T> = Result<T, Problem>;
