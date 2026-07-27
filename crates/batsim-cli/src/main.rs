//! `batsimctl`: admin CLI mirroring the batsim HTTP API one-to-one.
//!
//! Machine-first: every command prints the API's JSON response on
//! stdout. Exit codes: 0 success, 2 usage/config error, 3 API error
//! (the problem document goes to stderr).

use std::io::Read;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// batsim fleet simulator admin CLI.
#[derive(Debug, Parser)]
#[command(name = "batsimctl", version, about)]
struct Cli {
    /// Base URL of the simulator.
    #[arg(long, env = "BATSIM_URL", default_value = "http://127.0.0.1:8080", global = true)]
    url: String,
    /// API key (sent as a bearer token when the server requires auth).
    #[arg(long, env = "BATSIM_API_KEY", global = true)]
    api_key: Option<String>,
    /// Output mode.
    #[arg(long, default_value = "json", global = true)]
    output: OutputMode,

    #[command(subcommand)]
    cmd: Cmd,
}

/// Output modes.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputMode {
    /// Raw JSON (default).
    Json,
    /// Human-readable table for list endpoints.
    Table,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Manage simulated homes.
    Homes {
        #[command(subcommand)]
        cmd: HomesCmd,
    },
    /// Manage fleets.
    Fleets {
        #[command(subcommand)]
        cmd: FleetsCmd,
    },
    /// Manage scenarios.
    Scenarios {
        #[command(subcommand)]
        cmd: ScenariosCmd,
    },
    /// Control virtual time.
    Sim {
        #[command(subcommand)]
        cmd: SimCmd,
    },
    /// Dispatch control commands and inspect the audit log.
    Dispatch {
        #[command(subcommand)]
        cmd: DispatchCmd,
    },
    /// Query telemetry.
    Telemetry {
        #[command(subcommand)]
        cmd: TelemetryCmd,
    },
    /// Inspect the device catalog.
    Registry {
        #[command(subcommand)]
        cmd: RegistryCmd,
    },
    /// Server introspection.
    System {
        #[command(subcommand)]
        cmd: SystemCmd,
    },
    /// Print the OpenAPI document.
    Openapi,
}

#[derive(Debug, Subcommand)]
enum HomesCmd {
    /// Create a home from a JSON file or stdin (`-`).
    Create {
        /// Request body JSON.
        body: String,
    },
    /// List homes.
    List {
        /// Restrict to a fleet.
        #[arg(long)]
        fleet_id: Option<String>,
        /// Page size.
        #[arg(long)]
        limit: Option<u32>,
        /// Continuation cursor.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Get one home.
    Get { id: String },
    /// Patch a home (mode and/or reserve).
    Patch {
        id: String,
        /// New operating mode.
        #[arg(long)]
        mode: Option<String>,
        /// New reserve SOC fraction.
        #[arg(long)]
        reserve_soc: Option<f64>,
    },
    /// Delete a home.
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
enum FleetsCmd {
    /// Create a fleet from a manifest JSON file or stdin (`-`).
    Create {
        /// Manifest JSON.
        body: String,
    },
    /// List fleets.
    List,
    /// Get one fleet.
    Get { id: String },
    /// Expand a fleet by N homes.
    Expand { id: String, count: u32 },
    /// Delete a fleet.
    Delete { id: String },
    /// Dispatch to an entire fleet.
    Dispatch {
        id: String,
        /// Action JSON, e.g. `{"type":"discharge_to","kw":5.0,"duration_s":3600}`.
        action: String,
    },
}

#[derive(Debug, Subcommand)]
enum ScenariosCmd {
    /// Create a scenario from a JSON file or stdin (`-`).
    Create {
        /// Scenario JSON.
        body: String,
    },
    /// List scenarios.
    List,
    /// Get one scenario.
    Get { id: String },
    /// Activate a scenario.
    Activate { id: String },
    /// Deactivate the active scenario.
    Deactivate { id: String },
}

#[derive(Debug, Subcommand)]
enum SimCmd {
    /// Start ticking.
    Start,
    /// Pause.
    Pause,
    /// Resume.
    Resume,
    /// Stop (state retained).
    Stop,
    /// Advance N ticks synchronously (paused only).
    Step {
        ticks: u64,
        /// Permit advances beyond one sim-day.
        #[arg(long)]
        allow_large: bool,
    },
    /// Advance to a simulation time (RFC 3339) synchronously.
    RunUntil {
        until: String,
        /// Permit advances beyond one sim-day.
        #[arg(long)]
        allow_large: bool,
    },
    /// Set the speed multiplier (0 = as fast as possible).
    Speed { multiplier: f64 },
    /// Simulation status.
    Status,
}

#[derive(Debug, Subcommand)]
enum DispatchCmd {
    /// Send a dispatch command from a JSON file or stdin (`-`).
    Run {
        /// Dispatch request JSON.
        body: String,
        /// Idempotency key.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Audit log.
    Commands {
        /// Filter by rollup status.
        #[arg(long)]
        status: Option<String>,
    },
    /// Command detail.
    Get { command_id: String },
    /// Cancel a command's queued targets.
    Cancel { command_id: String },
}

#[derive(Debug, Subcommand)]
enum TelemetryCmd {
    /// Home series.
    HomeSeries {
        id: String,
        /// Comma-separated fields.
        #[arg(long)]
        fields: Option<String>,
        /// Resolution: 1s, 1m, 5m, 15m, 1h.
        #[arg(long)]
        resolution: Option<String>,
    },
    /// Fleet series.
    FleetSeries {
        id: String,
        /// Comma-separated fields.
        #[arg(long)]
        fields: Option<String>,
        /// Resolution: 1s, 1m, 5m, 15m, 1h.
        #[arg(long)]
        resolution: Option<String>,
        /// Fleet aggregation: sum, mean, p95.
        #[arg(long)]
        agg: Option<String>,
    },
    /// Follow the live stream (SSE) until interrupted.
    Tail {
        /// Restrict to a fleet.
        #[arg(long)]
        fleet_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum RegistryCmd {
    /// List battery models.
    Batteries {
        /// Filter by vendor substring.
        #[arg(long)]
        vendor: Option<String>,
    },
    /// One battery model.
    Battery { model_id: String },
    /// List inverter models.
    Inverters {
        /// Filter by vendor substring.
        #[arg(long)]
        vendor: Option<String>,
    },
    /// One inverter model.
    Inverter { model_id: String },
    /// Catalog version.
    Version,
}

#[derive(Debug, Subcommand)]
enum SystemCmd {
    /// Health.
    Health,
    /// Version.
    Version,
    /// Effective (redacted) config.
    Config,
}

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => {}
        Err(e) => {
            if let Some(api) = e.downcast_ref::<ApiError>() {
                eprintln!("{}", api.body);
                std::process::exit(3);
            }
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    }
}

/// An API error response (problem document).
#[derive(Debug)]
struct ApiError {
    /// The response body.
    body: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "API error: {}", self.body)
    }
}

impl std::error::Error for ApiError {}

/// HTTP helpers over ureq.
struct Client<'a> {
    base: &'a str,
    api_key: &'a Option<String>,
}

impl Client<'_> {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), path)
    }

    fn read_body(resp: ureq::http::Response<ureq::Body>) -> Result<serde_json::Value> {
        let mut s = String::new();
        resp.into_body()
            .into_reader()
            .read_to_string(&mut s)
            .context("read response")?;
        if s.is_empty() {
            Ok(serde_json::json!({"ok": true}))
        } else {
            Ok(serde_json::from_str(&s).context("parse response JSON")?)
        }
    }

    fn get(&self, path: &str) -> Result<serde_json::Value> {
        let mut req = ureq::get(self.url(path));
        if let Some(k) = self.api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        match req.call() {
            Ok(resp) => Self::read_body(resp),
            Err(ureq::Error::StatusCode(code)) => Err(ApiError {
                body: format!("status {code}"),
            }
            .into()),
            Err(e) => Err(e.into()),
        }
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        idempotency_key: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = self.url(path);
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(k) = self.api_key {
            headers.push(("Authorization".to_owned(), format!("Bearer {k}")));
        }
        if let Some(k) = idempotency_key {
            headers.push(("Idempotency-Key".to_owned(), k.to_owned()));
        }
        let result = if method == "DELETE" {
            let mut req = ureq::delete(&url);
            for (k, v) in &headers {
                req = req.header(k, v);
            }
            req.call()
        } else {
            let mut req = match method {
                "POST" => ureq::post(&url),
                "PUT" => ureq::put(&url),
                "PATCH" => ureq::patch(&url),
                other => anyhow::bail!("unsupported method {other}"),
            };
            for (k, v) in &headers {
                req = req.header(k, v);
            }
            match body {
                Some(b) => req.send_json(b),
                None => req.send_empty(),
            }
        };
        match result {
            Ok(resp) => Self::read_body(resp),
            Err(ureq::Error::StatusCode(code)) => Err(ApiError {
                body: format!("status {code}"),
            }
            .into()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Read a JSON argument: a path, `-` for stdin, or an inline document.
fn read_json_arg(arg: &str) -> Result<serde_json::Value> {
    if arg == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).context("read stdin")?;
        return serde_json::from_str(&s).context("parse stdin JSON");
    }
    if let Ok(s) = std::fs::read_to_string(arg) {
        return serde_json::from_str(&s).with_context(|| format!("parse {arg}"));
    }
    serde_json::from_str(arg).context("parse inline JSON")
}

fn emit(cli: &Cli, value: &serde_json::Value) -> Result<()> {
    match cli.output {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(value)?),
        OutputMode::Table => print_table(value),
    }
    Ok(())
}

fn print_table(value: &serde_json::Value) {
    let rows = value
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_else(|| vec![value.clone()]);
    let mut keys: Vec<String> = Vec::new();
    for row in &rows {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if !keys.contains(k) && keys.len() < 8 {
                    keys.push(k.clone());
                }
            }
        }
    }
    println!("{}", keys.join("\t"));
    for row in &rows {
        let cells: Vec<String> = keys
            .iter()
            .map(|k| {
                row.get(k).map_or_else(String::new, |v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
            })
            .collect();
        println!("{}", cells.join("\t"));
    }
}

#[allow(clippy::too_many_lines)]
fn run(cli: &Cli) -> Result<()> {
    let http = Client {
        base: &cli.url,
        api_key: &cli.api_key,
    };
    match &cli.cmd {
        Cmd::Homes { cmd } => match cmd {
            HomesCmd::Create { body } => {
                let v = read_json_arg(body)?;
                emit(cli, &http.send("POST", "/v1/homes", Some(&v), None)?)
            }
            HomesCmd::List {
                fleet_id,
                limit,
                cursor,
            } => {
                let mut q = Vec::new();
                if let Some(f) = fleet_id {
                    q.push(format!("fleet_id={f}"));
                }
                if let Some(l) = limit {
                    q.push(format!("limit={l}"));
                }
                if let Some(c) = cursor {
                    q.push(format!("cursor={c}"));
                }
                emit(cli, &http.get(&format!("/v1/homes?{}", q.join("&")))?)
            }
            HomesCmd::Get { id } => emit(cli, &http.get(&format!("/v1/homes/{id}"))?),
            HomesCmd::Patch { id, mode, reserve_soc } => {
                let mut v = serde_json::Map::new();
                if let Some(m) = mode {
                    v.insert("mode".into(), serde_json::Value::String(m.clone()));
                }
                if let Some(r) = reserve_soc {
                    v.insert("reserve_soc".into(), serde_json::json!(r));
                }
                emit(
                    cli,
                    &http.send("PATCH", &format!("/v1/homes/{id}"), Some(&v.into()), None)?,
                )
            }
            HomesCmd::Delete { id } => {
                emit(cli, &http.send("DELETE", &format!("/v1/homes/{id}"), None, None)?)
            }
        },
        Cmd::Fleets { cmd } => match cmd {
            FleetsCmd::Create { body } => {
                let v = read_json_arg(body)?;
                emit(cli, &http.send("POST", "/v1/fleets", Some(&v), None)?)
            }
            FleetsCmd::List => emit(cli, &http.get("/v1/fleets")?),
            FleetsCmd::Get { id } => emit(cli, &http.get(&format!("/v1/fleets/{id}"))?),
            FleetsCmd::Expand { id, count } => emit(
                cli,
                &http.send(
                    "POST",
                    &format!("/v1/fleets/{id}:expand"),
                    Some(&serde_json::json!({"count": count})),
                    None,
                )?,
            ),
            FleetsCmd::Delete { id } => {
                emit(cli, &http.send("DELETE", &format!("/v1/fleets/{id}"), None, None)?)
            }
            FleetsCmd::Dispatch { id, action } => {
                let a = read_json_arg(action)?;
                emit(
                    cli,
                    &http.send(
                        "POST",
                        &format!("/v1/fleets/{id}:dispatch"),
                        Some(&serde_json::json!({"action": a})),
                        None,
                    )?,
                )
            }
        },
        Cmd::Scenarios { cmd } => match cmd {
            ScenariosCmd::Create { body } => {
                let v = read_json_arg(body)?;
                emit(cli, &http.send("POST", "/v1/scenarios", Some(&v), None)?)
            }
            ScenariosCmd::List => emit(cli, &http.get("/v1/scenarios")?),
            ScenariosCmd::Get { id } => emit(cli, &http.get(&format!("/v1/scenarios/{id}"))?),
            ScenariosCmd::Activate { id } => emit(
                cli,
                &http.send("POST", &format!("/v1/scenarios/{id}:activate"), None, None)?,
            ),
            ScenariosCmd::Deactivate { id } => emit(
                cli,
                &http.send("POST", &format!("/v1/scenarios/{id}:deactivate"), None, None)?,
            ),
        },
        Cmd::Sim { cmd } => match cmd {
            SimCmd::Start => emit(cli, &http.send("POST", "/v1/sim:start", None, None)?),
            SimCmd::Pause => emit(cli, &http.send("POST", "/v1/sim:pause", None, None)?),
            SimCmd::Resume => emit(cli, &http.send("POST", "/v1/sim:resume", None, None)?),
            SimCmd::Stop => emit(cli, &http.send("POST", "/v1/sim:stop", None, None)?),
            SimCmd::Step { ticks, allow_large } => emit(
                cli,
                &http.send(
                    "POST",
                    &format!("/v1/sim:step?allow_large={allow_large}"),
                    Some(&serde_json::json!({"ticks": ticks})),
                    None,
                )?,
            ),
            SimCmd::RunUntil { until, allow_large } => emit(
                cli,
                &http.send(
                    "POST",
                    &format!("/v1/sim:run-until?allow_large={allow_large}"),
                    Some(&serde_json::json!({"until": until})),
                    None,
                )?,
            ),
            SimCmd::Speed { multiplier } => emit(
                cli,
                &http.send(
                    "PUT",
                    "/v1/sim:speed",
                    Some(&serde_json::json!({"multiplier": multiplier})),
                    None,
                )?,
            ),
            SimCmd::Status => emit(cli, &http.get("/v1/sim:status")?),
        },
        Cmd::Dispatch { cmd } => match cmd {
            DispatchCmd::Run {
                body,
                idempotency_key,
            } => {
                let v = read_json_arg(body)?;
                emit(
                    cli,
                    &http.send("POST", "/v1/dispatch", Some(&v), idempotency_key.as_deref())?,
                )
            }
            DispatchCmd::Commands { status } => {
                let q = status.as_ref().map_or(String::new(), |s| format!("?status={s}"));
                emit(cli, &http.get(&format!("/v1/dispatch/commands{q}"))?)
            }
            DispatchCmd::Get { command_id } => {
                emit(cli, &http.get(&format!("/v1/dispatch/commands/{command_id}"))?)
            }
            DispatchCmd::Cancel { command_id } => emit(
                cli,
                &http.send("DELETE", &format!("/v1/dispatch/commands/{command_id}"), None, None)?,
            ),
        },
        Cmd::Telemetry { cmd } => match cmd {
            TelemetryCmd::HomeSeries {
                id,
                fields,
                resolution,
            } => {
                let mut q = Vec::new();
                if let Some(f) = fields {
                    q.push(format!("fields={f}"));
                }
                if let Some(r) = resolution {
                    q.push(format!("resolution={r}"));
                }
                emit(
                    cli,
                    &http.get(&format!("/v1/telemetry/homes/{id}/series?{}", q.join("&")))?,
                )
            }
            TelemetryCmd::FleetSeries {
                id,
                fields,
                resolution,
                agg,
            } => {
                let mut q = Vec::new();
                if let Some(f) = fields {
                    q.push(format!("fields={f}"));
                }
                if let Some(r) = resolution {
                    q.push(format!("resolution={r}"));
                }
                if let Some(a) = agg {
                    q.push(format!("agg={a}"));
                }
                emit(
                    cli,
                    &http.get(&format!("/v1/telemetry/fleets/{id}/series?{}", q.join("&")))?,
                )
            }
            TelemetryCmd::Tail { fleet_id } => {
                let q = fleet_id
                    .as_ref()
                    .map_or(String::new(), |f| format!("?fleet_id={f}"));
                let url = http.url(&format!("/v1/telemetry/stream{q}"));
                let mut req = ureq::get(&url).header("Accept", "text/event-stream");
                if let Some(k) = &cli.api_key {
                    req = req.header("Authorization", format!("Bearer {k}"));
                }
                let resp = req.call().context("open stream")?;
                let mut reader = resp.into_body().into_reader();
                let mut buf = [0u8; 4096];
                let mut pending = String::new();
                loop {
                    let n = reader.read(&mut buf).context("read stream")?;
                    if n == 0 {
                        break;
                    }
                    pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(pos) = pending.find('\n') {
                        let line: String = pending.drain(..=pos).collect();
                        if let Some(data) = line.strip_prefix("data:") {
                            println!("{}", data.trim());
                        }
                    }
                }
                Ok(())
            }
        },
        Cmd::Registry { cmd } => match cmd {
            RegistryCmd::Batteries { vendor } => {
                let q = vendor
                    .as_ref()
                    .map_or(String::new(), |v| format!("?vendor={v}"));
                emit(cli, &http.get(&format!("/v1/registry/batteries{q}"))?)
            }
            RegistryCmd::Battery { model_id } => {
                emit(cli, &http.get(&format!("/v1/registry/batteries/{model_id}"))?)
            }
            RegistryCmd::Inverters { vendor } => {
                let q = vendor
                    .as_ref()
                    .map_or(String::new(), |v| format!("?vendor={v}"));
                emit(cli, &http.get(&format!("/v1/registry/inverters{q}"))?)
            }
            RegistryCmd::Inverter { model_id } => {
                emit(cli, &http.get(&format!("/v1/registry/inverters/{model_id}"))?)
            }
            RegistryCmd::Version => emit(cli, &http.get("/v1/registry/version")?),
        },
        Cmd::System { cmd } => match cmd {
            SystemCmd::Health => emit(cli, &http.get("/v1/system/health")?),
            SystemCmd::Version => emit(cli, &http.get("/v1/system/version")?),
            SystemCmd::Config => emit(cli, &http.get("/v1/system/config")?),
        },
        Cmd::Openapi => emit(cli, &http.get("/openapi.json")?),
    }
}
