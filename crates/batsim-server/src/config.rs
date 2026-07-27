//! Server configuration: file + env + CLI layering.
//!
//! Precedence: `batsim.toml` < `BATSIM_*` environment variables
//! (double underscore = nesting) < CLI flags. Every effective key is
//! visible via `--print-config` and `GET /v1/system/config` (redacted).

use std::net::IpAddr;
use std::path::PathBuf;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

/// Top-level server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// HTTP listener settings.
    pub server: ServerConfig,
    /// Simulation engine settings.
    pub engine: EngineConfig,
    /// Telemetry retention settings.
    pub telemetry: TelemetryConfig,
    /// Optional API-key auth.
    pub auth: AuthConfig,
    /// Dispatch audit-log retention.
    pub audit: AuditConfig,
    /// Logging.
    pub logging: LoggingConfig,
    /// Registry catalog override directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_dir: Option<PathBuf>,
    /// Data directory for any on-disk state.
    pub data_dir: PathBuf,
}

/// HTTP listener settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Bind address.
    pub host: IpAddr,
    /// Bind port.
    pub port: u16,
    /// Allowed CORS origins (`*` = permissive, local tooling default).
    pub cors_origins: Vec<String>,
}

/// Engine settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Master seed used before a scenario overrides it.
    pub seed: u64,
    /// Default simulation epoch (RFC 3339) before a scenario overrides
    /// it. Must be 5-minute aligned.
    pub epoch: String,
    /// Tick length in seconds (1..=60).
    pub tick_seconds: u32,
    /// Default speed multiplier when the simulation starts.
    pub speed: f64,
    /// Broadcast capacity per telemetry subscriber.
    pub stream_buffer: usize,
    /// Maximum homes for raw per-home streaming.
    pub raw_stream_max_homes: usize,
}

/// Telemetry retention settings (ring-buffer capacities).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Per-home raw ticks retained (default one hour of 1-s ticks).
    pub raw_ticks: usize,
    /// Per-home one-minute rollups retained (default one day).
    pub rollup_minutes: usize,
}

/// Optional API-key auth. Empty `api_keys` = open (single-tenant local
/// default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Full-access keys.
    pub api_keys: Vec<String>,
    /// Keys restricted to GETs and the telemetry stream.
    pub read_only_keys: Vec<String>,
}

/// Audit-log settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    /// Idempotency record TTL in hours.
    pub idempotency_ttl_hours: u64,
    /// Maximum retained command records (oldest dropped beyond this).
    pub max_commands: usize,
}

/// Logging settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// EnvFilter directive string.
    pub filter: String,
    /// Emit JSON lines instead of pretty text.
    pub json: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            engine: EngineConfig::default(),
            telemetry: TelemetryConfig::default(),
            auth: AuthConfig::default(),
            audit: AuditConfig::default(),
            logging: LoggingConfig::default(),
            data_dir: PathBuf::from("data"),
            registry_dir: None,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::from([127, 0, 0, 1]),
            port: 8080,
            cors_origins: vec!["*".to_owned()],
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            seed: 1,
            epoch: "2025-01-01T00:00:00Z".to_owned(),
            tick_seconds: 1,
            speed: 1.0,
            stream_buffer: 1024,
            raw_stream_max_homes: 500,
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            raw_ticks: 3600,
            rollup_minutes: 1440,
        }
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            idempotency_ttl_hours: 24,
            max_commands: 10_000,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            filter: "info,tower_http=info".to_owned(),
            json: false,
        }
    }
}

impl Config {
    /// Load with the full precedence chain: defaults < file < env.
    ///
    /// # Errors
    /// Returns a description of the first layering or parse failure.
    pub fn load(config_path: Option<&std::path::Path>) -> Result<Self, anyhow::Error> {
        let mut fig = Figment::from(Serialized::defaults(Self::default()));
        if let Some(path) = config_path {
            fig = fig.merge(Toml::file(path));
        }
        fig = fig.merge(Env::prefixed("BATSIM_").split("__"));
        let cfg: Self = fig.extract()?;
        Ok(cfg)
    }

    /// Serialize with secrets redacted (API keys appear as SHA-256
    /// fingerprints).
    #[must_use]
    pub fn redacted(&self) -> Self {
        let mut out = self.clone();
        out.auth.api_keys = self.auth.api_keys.iter().map(|k| fingerprint(k)).collect();
        out.auth.read_only_keys = self.auth.read_only_keys.iter().map(|k| fingerprint(k)).collect();
        out
    }
}

/// `sha256:<hex-prefix>` fingerprint for log/config-safe key display.
#[must_use]
pub fn fingerprint(secret: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(secret.as_bytes());
    format!("sha256:{}", hex_prefix(&hash))
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes[..8].iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}
