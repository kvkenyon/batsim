//! The `batsim` binary: config loading, logging, engine startup, HTTP
//! serving.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::time::SimClock;
use batsim_registry::Registry;
use batsim_server::config::Config;
use batsim_server::engine as sim_engine;
use batsim_server::price::PriceSource;
use batsim_server::state::{AppState, AuditStore, IdemStore};
use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Residential battery fleet simulator.
#[derive(Debug, Parser)]
#[command(name = "batsim", version, about)]
struct Cli {
    /// Config file path.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Bind port (overrides config and env).
    #[arg(long)]
    port: Option<u16>,
    /// Data directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Registry catalog shadow directory.
    #[arg(long)]
    registry_dir: Option<PathBuf>,
    /// Master seed (overrides config).
    #[arg(long)]
    seed: Option<u64>,
    /// Print the effective config and exit.
    #[arg(long)]
    print_config: bool,
    /// Print the OpenAPI document and exit.
    #[arg(long)]
    dump_openapi: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.dump_openapi {
        let registry = Registry::load(cli.registry_dir.as_deref()).context("load registry")?;
        let doc = batsim_server::openapi_document(&registry);
        let json = serde_json::to_string_pretty(&doc).context("serialize OpenAPI")?;
        println!("{json}");
        return Ok(());
    }

    let mut config = Config::load(cli.config.as_deref()).context("load config")?;
    if let Some(port) = cli.port {
        config.server.port = port;
    }
    if let Some(dir) = cli.data_dir {
        config.data_dir = dir;
    }
    if let Some(dir) = cli.registry_dir {
        config.registry_dir = Some(dir);
    }
    if let Some(seed) = cli.seed {
        config.engine.seed = seed;
    }

    if cli.print_config {
        let json = serde_json::to_string_pretty(&config.redacted()).context("serialize config")?;
        println!("{json}");
        return Ok(());
    }

    init_logging(&config);

    let registry = Registry::load(config.registry_dir.as_deref()).context("load registry")?;
    tracing::info!(
        version = %registry.manifest().registry_version,
        batteries = registry.batteries().count(),
        inverters = registry.inverters().count(),
        "device catalog loaded"
    );

    let clock = SimClock::from_rfc3339(&config.engine.epoch, config.engine.tick_seconds)
        .map_err(|e| anyhow::anyhow!("invalid engine.epoch: {e}"))?;
    let world = SimWorld::new(
        clock,
        config.engine.seed,
        AmbientFeed::DiurnalSine {
            mean_c: 28.0,
            amplitude_c: 7.0,
        },
    )
    .map_err(|e| anyhow::anyhow!("engine init: {e}"))?;

    let audit = Arc::new(RwLock::new(AuditStore::new(config.audit.max_commands)));
    let (engine, events) = sim_engine::spawn(
        world,
        config.engine.speed,
        PriceSource::default_feed(),
        config.telemetry.raw_ticks,
        config.telemetry.rollup_minutes,
        config.engine.raw_stream_max_homes,
        config.engine.stream_buffer,
        audit.clone(),
    )
    .context("spawn engine thread")?;

    let state = AppState {
        config: Arc::new(config.clone()),
        registry: Arc::new(registry),
        engine,
        events,
        homes: Arc::new(RwLock::new(std::collections::HashMap::new())),
        fleets: Arc::new(RwLock::new(std::collections::HashMap::new())),
        scenarios: Arc::new(RwLock::new(std::collections::HashMap::new())),
        active_scenario: Arc::new(RwLock::new(None)),
        audit,
        idempotency: Arc::new(RwLock::new(IdemStore::new(
            config.audit.idempotency_ttl_hours,
        ))),
        started: std::time::Instant::now(),
        compose_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

    let addr = SocketAddr::new(config.server.host, config.server.port);
    let router = batsim_server::build_router(state);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("bind {addr}"))?;
        tracing::info!(%addr, "batsim listening");
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("serve")
    })?;
    Ok(())
}

fn init_logging(config: &Config) {
    let filter =
        EnvFilter::try_new(&config.logging.filter).unwrap_or_else(|_| EnvFilter::new("info"));
    if config.logging.json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

async fn shutdown_signal() {
    drop(tokio::signal::ctrl_c().await);
}
