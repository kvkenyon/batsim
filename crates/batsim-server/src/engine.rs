//! The engine thread: owns the simulation world, the telemetry store,
//! and the dispatch execution tracker; speaks to HTTP handlers over a
//! command channel with oneshot replies.
//!
//! All mutation of simulation state happens on this one thread, so the
//! tick loop never contends with the API. While running, messages are
//! drained between ticks; while paused or stopped the thread blocks on
//! the channel. Pacing reads wall time only to sleep between ticks and
//! never enters simulation state.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use batsim_core::dispatch::{ControlMode, ScheduledDispatch};
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::time::SimClock;
use batsim_ercot::settlement::{SettlementConfig, SettlementEngine, SettlementReport};
use batsim_ercot::PriceSource as _;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::{broadcast, oneshot};

use crate::model::{SimState, TargetStatus};
use crate::price::PriceSource;
use crate::problem::Problem;
use crate::state::AuditStore;
use crate::telemetry::{HomeBucket, TelemetryStore, TickPoint};

/// Format unix seconds as RFC 3339 UTC.
#[must_use]
pub fn rfc3339_of(unix: u64) -> String {
    let ts = i64::try_from(unix).unwrap_or(i64::MAX);
    OffsetDateTime::from_unix_timestamp(ts).map_or_else(
        |_| "1970-01-01T00:00:00Z".to_owned(),
        |t| {
            t.format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
        },
    )
}

/// Parse an RFC 3339 timestamp to unix seconds.
///
/// # Errors
/// Returns a message when the input is not RFC 3339.
pub fn unix_of(ts: &str) -> Result<u64, String> {
    let t =
        OffsetDateTime::parse(ts, &Rfc3339).map_err(|e| format!("invalid RFC 3339 time: {e}"))?;
    u64::try_from(t.unix_timestamp()).map_err(|_| "time must not be before 1970".to_owned())
}

/// Per-fleet live aggregate in a tick event.
#[derive(Debug, Clone, Serialize)]
pub struct FleetTick {
    /// Fleet id.
    pub fleet_id: String,
    /// Homes contributing.
    pub homes: usize,
    /// Fleet battery power (kW; + discharge).
    pub battery_power_kw: f64,
    /// Fleet PV power (kW).
    pub pv_power_kw: f64,
    /// Fleet load (kW).
    pub load_power_kw: f64,
    /// Fleet grid exchange (kW; + import).
    pub grid_power_kw: f64,
    /// Mean SOC.
    pub soc_mean: f64,
}

/// Per-home row in raw tick events.
#[derive(Debug, Clone, Serialize)]
pub struct HomeTickRow {
    /// Home id.
    pub home_id: String,
    /// Mean SOC.
    pub soc: f64,
    /// Battery power (kW; + discharge).
    pub battery_power_kw: f64,
    /// PV power (kW).
    pub pv_power_kw: f64,
    /// Load (kW).
    pub load_power_kw: f64,
    /// Grid exchange (kW; + import).
    pub grid_power_kw: f64,
}

/// A committed tick.
#[derive(Debug, Clone, Serialize)]
pub struct TickEvent {
    /// Simulation time (RFC 3339 UTC).
    pub sim_time: String,
    /// Unix seconds.
    pub unix: u64,
    /// Tick index.
    pub tick: u64,
    /// Real-time price ($/MWh).
    pub price_rtm: f64,
    /// Per-fleet aggregates.
    pub fleets: Vec<FleetTick>,
    /// Per-home rows; present only for small fleets.
    pub homes: Option<Vec<HomeTickRow>>,
}

/// Live event broadcast to stream subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SimEvent {
    /// A committed tick.
    Tick(TickEvent),
    /// A command finished executing on all remaining targets.
    Dispatch {
        /// Command id.
        command_id: String,
        /// Tick of completion.
        tick: u64,
        /// Targets that applied fully.
        targets_applied: usize,
        /// Targets that did not (partial/rejected/timeout/cancelled).
        targets_rejected: usize,
    },
    /// A settlement interval closed during a backtest run.
    Settlement(Box<batsim_ercot::settlement::IntervalSettlement>),
    /// A backtest run reached its end and settled.
    RunFinished {
        /// Backtest run id.
        run_id: String,
        /// Final tick.
        tick: u64,
    },
}

/// One strategy schedule entry: apply an action to every active home at
/// `unix` (deterministic, engine-side — the HTTP layer cannot time
/// dispatches against an unbounded-speed run).
#[derive(Debug, Clone)]
pub struct StrategyEntry {
    /// Unix time at which the action fires.
    pub unix: u64,
    /// Action to apply (charge_to / discharge_to with per-home kW).
    pub spec: crate::model::ActionSpec,
}

/// An ancillary-service award to settle (spec D.5.1). When `deployed` is
/// set, the award is also scored as a deployment: delivered energy is
/// measured from fleet battery-meter export during the window.
#[derive(Debug, Clone)]
pub struct AsAwardInput {
    /// Product.
    pub product: batsim_ercot::AsProduct,
    /// Window start (unix).
    pub start_unix: u64,
    /// Window end (unix).
    pub end_unix: u64,
    /// Awarded capacity (MW).
    pub awarded_mw: f64,
    /// Clearing price ($/MW); when absent, the DAM MCPC average over the
    /// window from the bound replay feed is used.
    pub mcpc_usd_per_mw: Option<f64>,
    /// Whether a deployment event is scored for this window.
    pub deployed: bool,
}

/// Backtest configuration handed to the engine at run start.
#[derive(Debug)]
pub struct BacktestConfig {
    /// Run id (bt_…).
    pub run_id: String,
    /// Run end (unix); the sim auto-stops and settles here.
    pub end_unix: u64,
    /// Settlement interval (seconds; 0 = auto from the replay cadence).
    pub interval_secs: u32,
    /// Settlement location.
    pub location: batsim_ercot::Location,
    /// Retail rate structure for the retailer-margin view.
    pub retail_rate: batsim_ercot::settlement::RetailRate,
    /// Baseline methodology label recorded in the report.
    pub baseline_method_label: String,
    /// Transmission rate for 4CP savings, USD per kW-month.
    pub transmission_rate_usd_per_kw_mo: f64,
    /// Fleet program costs over the run, USD.
    pub program_costs_usd: f64,
    /// Per-home incentive payments, USD.
    pub incentives_usd: std::collections::BTreeMap<String, f64>,
    /// Report provenance.
    pub provenance: batsim_ercot::Provenance,
    /// Strategy schedule entries, sorted by `unix`.
    pub schedule: Vec<StrategyEntry>,
    /// AS awards to settle.
    pub as_awards: Vec<AsAwardInput>,
    /// Explicit 4CP candidate interval starts (unix). When the bound feed
    /// carries a system-load signal, the 4CP watch flags additional
    /// candidates automatically.
    pub four_cp_candidates: Vec<u64>,
}

/// Backtest lifecycle state reported to the API.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestState {
    /// Run in progress.
    Running,
    /// Run settled; report available.
    Settled,
    /// Run failed (e.g. price coverage gap).
    Failed(String),
}

/// Backtest status snapshot for the API.
#[derive(Debug, Clone, Serialize)]
pub struct BacktestInfo {
    /// Run id.
    pub run_id: String,
    /// Lifecycle state.
    pub state: BacktestState,
    /// Current sim time (RFC 3339 UTC).
    pub sim_time: String,
    /// Settlement intervals closed so far.
    pub intervals_settled: usize,
    /// Final report (when settled).
    pub report: Option<SettlementReport>,
}

/// Meter snapshot for interval energy deltas (cumulative Wh).
#[derive(Debug, Clone, Copy, Default)]
struct MeterSnapshot {
    import_main: f64,
    export_main: f64,
    export_batt: f64,
}

/// Per-home interval accumulator: meter deltas (Wh).
#[derive(Debug, Clone, Copy, Default)]
struct IntervalAcc {
    import_main: f64,
    export_main: f64,
    export_batt: f64,
}

/// Engine-side backtest run state.
struct BacktestRun {
    engine: SettlementEngine,
    settlement_cfg: SettlementConfig,
    config: BacktestConfig,
    interval_start: u64,
    last_meters: std::collections::BTreeMap<u64, MeterSnapshot>,
    acc: std::collections::BTreeMap<u64, IntervalAcc>,
    next_entry: usize,
    intervals_settled: usize,
    batt_export_kwh_by_interval: Vec<(u64, f64)>,
    net_kw_history: std::collections::BTreeMap<u64, std::collections::VecDeque<f64>>,
    system_loads: std::collections::BTreeMap<i64, f64>,
    watch: batsim_ercot::four_cp::FourCpWatch,
    failed: Option<String>,
    report: Option<SettlementReport>,
}

/// Metadata the engine keeps per arena slot.
#[derive(Debug, Clone)]
pub struct SlotMeta {
    /// Home id.
    pub home_id: String,
    /// Fleet membership.
    pub fleet_id: Option<String>,
}

/// Dynamic per-home state snapshot.
#[derive(Debug, Clone)]
pub struct HomeDyn {
    /// Active control mode.
    pub mode: ControlMode,
    /// Mean SOC.
    pub soc: f64,
    /// Battery AC power (W).
    pub batt_w: f64,
    /// PV AC power (W).
    pub pv_w: f64,
    /// Load (W).
    pub load_w: f64,
    /// Grid exchange (W).
    pub grid_w: f64,
    /// Active PV curtailment fraction.
    pub curtail: f64,
    /// Queued commands.
    pub queued: usize,
    /// Manual-mode setpoint (W; + discharge).
    pub manual_setpoint_w: f64,
}

/// Engine status snapshot.
#[derive(Debug, Clone)]
pub struct EngineStatus {
    /// Run state.
    pub state: SimState,
    /// Current tick.
    pub tick: u64,
    /// Current unix time.
    pub unix: u64,
    /// Tick length (s).
    pub dt_s: u32,
    /// Master seed of the current world binding.
    pub master_seed: u64,
    /// Homes in the arena (including retired).
    pub home_count: usize,
    /// Configured speed multiplier (0 = unbounded).
    pub speed: f64,
    /// Measured speed.
    pub achieved_speed: f64,
    /// Scheduling lag in ticks.
    pub lag_ticks: u64,
    /// Queued commands across homes.
    pub queued_commands: usize,
}

/// Result of a synchronous advance.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    /// Ticks executed.
    pub ticks: u64,
    /// Tick reached.
    pub tick: u64,
    /// Unix time reached.
    pub unix: u64,
    /// Wall milliseconds taken.
    pub wall_ms: u64,
}

/// One dispatch target to schedule.
#[derive(Debug)]
pub struct DispatchItem {
    /// Arena index.
    pub home_idx: u64,
    /// Issuer tag (hashed command id) for cancellation.
    pub tag: u64,
    /// Command id (audit).
    pub command_id: String,
    /// Position of this target in the command record.
    pub target_pos: usize,
    /// Tick at which the actions apply.
    pub execute_at_tick: u64,
    /// Timeout in ticks after `execute_at_tick`.
    pub timeout_ticks: u64,
    /// Requested power (kW) for power actions.
    pub requested_kw: Option<f64>,
    /// Actions to enqueue, in order.
    pub actions: Vec<ScheduledDispatch>,
}

/// Messages to the engine thread.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum EngineMsg {
    /// Start ticking.
    Start {
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Pause at the next tick boundary.
    Pause {
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Resume from paused.
    Resume {
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Stop (state retained).
    Stop {
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Advance N ticks synchronously (paused only).
    Step {
        /// Ticks to advance.
        ticks: u64,
        /// Outcome reply.
        reply: oneshot::Sender<Result<StepOutcome, String>>,
    },
    /// Advance to a unix time synchronously (paused only).
    RunUntil {
        /// Target unix time.
        unix: u64,
        /// Outcome reply.
        reply: oneshot::Sender<Result<StepOutcome, String>>,
    },
    /// Change speed multiplier.
    SetSpeed {
        /// Sim-seconds per wall-second (0 = unbounded).
        multiplier: f64,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Status query.
    Status {
        /// Status reply.
        reply: oneshot::Sender<EngineStatus>,
    },
    /// Add homes; returns arena indices in order.
    AddHomes {
        /// Homes with their slot metadata.
        homes: Vec<(Home, SlotMeta)>,
        /// Index reply.
        reply: oneshot::Sender<Vec<u64>>,
    },
    /// Retire a home.
    RemoveHome {
        /// Arena index.
        idx: u64,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Apply a config patch immediately.
    PatchHome {
        /// Arena index.
        idx: u64,
        /// New control mode.
        mode: Option<ControlMode>,
        /// New reserve fraction.
        reserve: Option<f64>,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Dynamic state of one home.
    HomeState {
        /// Arena index.
        idx: u64,
        /// State reply.
        reply: oneshot::Sender<Result<HomeDyn, String>>,
    },
    /// Rebind the world to a scenario (stopped only).
    Rebind {
        /// Scenario epoch (unix seconds).
        epoch_s: u64,
        /// Tick length (s).
        tick_s: u32,
        /// Scenario master seed.
        seed: u64,
        /// Ambient feed.
        ambient: AmbientFeed,
        /// Price feed.
        price: PriceSource,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Replace homes at existing arena indices with freshly composed
    /// instances (backtest pristine-state reset; indices — and therefore
    /// RNG substreams — are preserved).
    ResetHomes {
        /// `(arena index, fresh home)` pairs.
        homes: Vec<(u64, Home)>,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Configure a backtest run (stopped only, replay-backed price feed).
    ConfigureBacktest {
        /// Run configuration.
        config: Box<BacktestConfig>,
        /// Completion reply.
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Backtest run status (None when no run is configured).
    BacktestStatus {
        /// Status reply.
        reply: oneshot::Sender<Option<BacktestInfo>>,
    },
    /// Enqueue a dispatch command's per-home actions.
    EnqueueDispatch {
        /// Per-target items.
        items: Vec<DispatchItem>,
        /// Completion reply.
        reply: oneshot::Sender<()>,
    },
    /// Cancel a command's still-queued targets by tag; returns the
    /// number of targets cancelled.
    CancelCommand {
        /// Issuer tag.
        tag: u64,
        /// Command id.
        command_id: String,
        /// Cancelled-target count reply.
        reply: oneshot::Sender<usize>,
    },
    /// Telemetry series for one home.
    HomeSeries {
        /// Arena index.
        idx: u64,
        /// Range start (unix).
        from: u64,
        /// Range end (unix).
        to: u64,
        /// Bucket length (s).
        bucket_s: u64,
        /// Buckets reply.
        reply: oneshot::Sender<Vec<HomeBucket>>,
    },
    /// Telemetry series for a set of homes (fleet aggregation happens in
    /// the handler from these per-home buckets).
    FleetSeries {
        /// Arena indices.
        idxs: Vec<u64>,
        /// Range start (unix).
        from: u64,
        /// Range end (unix).
        to: u64,
        /// Bucket length (s).
        bucket_s: u64,
        /// Per-home buckets reply.
        reply: oneshot::Sender<Vec<Vec<HomeBucket>>>,
    },
}

/// Handle to the running engine.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<EngineMsg>,
}

impl EngineHandle {
    /// Send a message and await the oneshot reply.
    ///
    /// # Errors
    /// [`Problem::internal`] when the engine thread is gone.
    pub async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> EngineMsg,
    ) -> Result<T, Problem> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).map_err(|_| Problem::internal())?;
        rx.await.map_err(|_| Problem::internal())
    }
}

struct PendingExec {
    tag: u64,
    command_id: String,
    target_pos: usize,
    home_idx: u64,
    execute_at_tick: u64,
    timeout_ticks: u64,
    requested_kw: Option<f64>,
}

struct Engine {
    world: SimWorld,
    state: SimState,
    speed: f64,
    price: PriceSource,
    telemetry: TelemetryStore,
    slots: Vec<(SlotMeta, bool)>,
    pending: Vec<PendingExec>,
    backtest: Option<BacktestRun>,
    audit: Arc<RwLock<AuditStore>>,
    events: broadcast::Sender<SimEvent>,
    rx: Receiver<EngineMsg>,
    raw_stream_max_homes: usize,
    next_deadline: Instant,
    lag_ticks: u64,
    achieved_speed: f64,
    window_start: Instant,
    window_ticks: u64,
}

impl Engine {
    fn tick_interval(&self) -> Duration {
        if self.speed <= 0.0 {
            return Duration::ZERO;
        }
        let secs = f64::from(self.world.clock().dt_s()) / self.speed;
        // Extreme multipliers are legal inputs: vanishingly small speeds
        // mean "barely ever tick" (clamped, not infinite) and huge speeds
        // approach unbounded.
        if !secs.is_finite() {
            return Duration::from_secs(3600);
        }
        Duration::from_secs_f64(secs.clamp(0.0, 3600.0))
    }

    fn reset_pacing(&mut self) {
        self.next_deadline = Instant::now() + self.tick_interval();
        self.window_start = Instant::now();
        self.window_ticks = 0;
    }

    fn handle(&mut self, msg: EngineMsg) {
        match msg {
            EngineMsg::Start { reply } => drop(reply.send(self.transition_to(
                SimState::Running,
                SimState::Stopped,
                "simulation can only start from stopped",
            ))),
            EngineMsg::Pause { reply } => drop(reply.send(self.transition_to(
                SimState::Paused,
                SimState::Running,
                "simulation is not running",
            ))),
            EngineMsg::Resume { reply } => drop(reply.send(self.transition_to(
                SimState::Running,
                SimState::Paused,
                "simulation is not paused",
            ))),
            EngineMsg::Stop { reply } => {
                let r = if self.state == SimState::Stopped {
                    Err("simulation is already stopped".to_owned())
                } else {
                    self.state = SimState::Stopped;
                    Ok(())
                };
                drop(reply.send(r));
            }
            EngineMsg::Step { ticks, reply } => {
                let r = if self.state == SimState::Paused {
                    Ok(self.advance(ticks))
                } else {
                    Err("synchronous step requires a paused simulation".to_owned())
                };
                drop(reply.send(r));
            }
            EngineMsg::RunUntil { unix, reply } => {
                let r = self.handle_run_until(unix);
                drop(reply.send(r));
            }
            EngineMsg::SetSpeed { multiplier, reply } => {
                let r = if multiplier.is_finite() && multiplier >= 0.0 {
                    self.speed = multiplier;
                    self.reset_pacing();
                    Ok(())
                } else {
                    Err("multiplier must be finite and >= 0".to_owned())
                };
                drop(reply.send(r));
            }
            EngineMsg::Status { reply } => {
                drop(reply.send(self.status()));
            }
            EngineMsg::ConfigureBacktest { config, reply } => {
                let r = if self.state == SimState::Stopped {
                    self.configure_backtest(*config)
                } else {
                    Err("backtest configuration requires a stopped simulation".to_owned())
                };
                drop(reply.send(r));
            }
            EngineMsg::BacktestStatus { reply } => {
                drop(reply.send(self.backtest_info()));
            }
            EngineMsg::ResetHomes { homes, reply } => {
                let r = if self.state == SimState::Stopped {
                    self.reset_homes(homes)
                } else {
                    Err("home reset requires a stopped simulation".to_owned())
                };
                drop(reply.send(r));
            }
            other => self.handle_homes_and_dispatch(other),
        }
    }

    fn handle_homes_and_dispatch(&mut self, msg: EngineMsg) {
        match msg {
            EngineMsg::AddHomes { homes, reply } => {
                let mut idxs = Vec::with_capacity(homes.len());
                for (home, meta) in homes {
                    let idx = self.world.add_home(home);
                    self.slots.push((meta, true));
                    idxs.push(idx);
                }
                drop(reply.send(idxs));
            }
            EngineMsg::RemoveHome { idx, reply } => {
                let r = self.mutate_home(idx, |h| h.set_retired(true)).map(|()| {
                    if let Some(slot) = self.slots.get_mut(idx as usize) {
                        slot.1 = false;
                    }
                    self.telemetry.remove(idx);
                });
                drop(reply.send(r));
            }
            EngineMsg::PatchHome {
                idx,
                mode,
                reserve,
                reply,
            } => {
                let r = self.mutate_home(idx, |h| {
                    if let Some(m) = mode {
                        h.set_mode(m);
                    }
                    if let Some(f) = reserve {
                        h.set_reserve_frac(f);
                    }
                });
                drop(reply.send(r));
            }
            EngineMsg::HomeState { idx, reply } => {
                let r = self.home_dyn(idx);
                drop(reply.send(r));
            }
            EngineMsg::Rebind {
                epoch_s,
                tick_s,
                seed,
                ambient,
                price,
                reply,
            } => {
                let r = if self.state == SimState::Stopped {
                    self.rebind(epoch_s, tick_s, seed, ambient, price)
                } else {
                    Err("scenario activation requires a stopped simulation".to_owned())
                };
                drop(reply.send(r));
            }
            EngineMsg::EnqueueDispatch { items, reply } => {
                self.enqueue(items);
                let _ = reply.send(());
            }
            EngineMsg::CancelCommand {
                tag,
                command_id,
                reply,
            } => {
                let cancelled = self.handle_cancel(tag, &command_id);
                let _ = reply.send(cancelled);
            }
            EngineMsg::HomeSeries {
                idx,
                from,
                to,
                bucket_s,
                reply,
            } => {
                drop(reply.send(self.telemetry.home_buckets(idx, from, to, bucket_s)));
            }
            EngineMsg::FleetSeries {
                idxs,
                from,
                to,
                bucket_s,
                reply,
            } => {
                let out = idxs
                    .iter()
                    .map(|i| self.telemetry.home_buckets(*i, from, to, bucket_s))
                    .collect();
                drop(reply.send(out));
            }
            _ => unreachable!("time-control messages are handled above"),
        }
    }

    fn mutate_home(&mut self, idx: u64, f: impl FnOnce(&mut Home)) -> Result<(), String> {
        match self.world.home_mut(idx as usize) {
            Some(h) => {
                f(h);
                Ok(())
            }
            None => Err("no home at that index".to_owned()),
        }
    }

    fn enqueue(&mut self, items: Vec<DispatchItem>) {
        for item in items {
            for action in &item.actions {
                if let Some(h) = self.world.home_mut(item.home_idx as usize) {
                    h.schedule(*action);
                }
            }
            self.pending.push(PendingExec {
                tag: item.tag,
                command_id: item.command_id,
                target_pos: item.target_pos,
                home_idx: item.home_idx,
                execute_at_tick: item.execute_at_tick,
                timeout_ticks: item.timeout_ticks,
                requested_kw: item.requested_kw,
            });
        }
    }

    fn transition_to(&mut self, next: SimState, from: SimState, err: &str) -> Result<(), String> {
        if self.state == from {
            self.state = next;
            if next == SimState::Running {
                self.reset_pacing();
            }
            Ok(())
        } else {
            Err(err.to_owned())
        }
    }

    fn home_dyn(&self, idx: u64) -> Result<HomeDyn, String> {
        self.world
            .home(idx as usize)
            .map(|h| {
                let latest = self.telemetry.latest(idx);
                HomeDyn {
                    mode: h.mode(),
                    soc: h.soc_mean(),
                    batt_w: latest.map_or(0.0, |p| p.batt_w),
                    pv_w: latest.map_or(0.0, |p| p.pv_w),
                    load_w: latest.map_or(0.0, |p| p.load_w),
                    grid_w: latest.map_or(0.0, |p| p.grid_w),
                    curtail: h.pv_curtail_frac(),
                    queued: h.queued_len(),
                    manual_setpoint_w: h.manual_setpoint_w(),
                }
            })
            .ok_or_else(|| "no home at that index".to_owned())
    }

    fn handle_run_until(&mut self, unix: u64) -> Result<StepOutcome, String> {
        if self.state != SimState::Paused {
            return Err("run-until requires a paused simulation".to_owned());
        }
        let cur = self.world.clock().unix_time();
        if unix <= cur {
            return Err("target time must be in the future".to_owned());
        }
        let dt = u64::from(self.world.clock().dt_s());
        let ticks = (unix - cur).div_ceil(dt);
        Ok(self.advance(ticks))
    }

    fn handle_cancel(&mut self, tag: u64, command_id: &str) -> usize {
        let mut cancelled = 0usize;
        for i in 0..self.world.home_count() {
            if let Some(h) = self.world.home_mut(i) {
                h.cancel_tagged(tag);
            }
        }
        let sim_time = rfc3339_of(self.world.clock().unix_time());
        let mut still_pending = Vec::with_capacity(self.pending.len());
        for p in std::mem::take(&mut self.pending) {
            if p.tag == tag {
                cancelled += 1;
                self.record_target(
                    command_id,
                    p.target_pos,
                    TargetStatus::Cancelled,
                    None,
                    Some(sim_time.clone()),
                );
            } else {
                still_pending.push(p);
            }
        }
        self.pending = still_pending;
        self.rollup_command(command_id);
        cancelled
    }

    fn rebind(
        &mut self,
        epoch_s: u64,
        tick_s: u32,
        seed: u64,
        ambient: AmbientFeed,
        price: PriceSource,
    ) -> Result<(), String> {
        let clock = SimClock::new(epoch_s, tick_s).map_err(|e| e.to_string())?;
        let mut world = SimWorld::new(clock, seed, ambient).map_err(|e| e.to_string())?;
        for i in 0..self.world.home_count() {
            if let Some(h) = self.world.home(i) {
                let mut h = h.clone();
                h.clear_dispatch_queue();
                world.add_home(h);
            }
        }
        self.world = world;
        self.telemetry.clear();
        self.price = price;
        // A rebound world invalidates any configured backtest run.
        self.backtest = None;
        // Anything still in flight belongs to the old binding.
        let pending = std::mem::take(&mut self.pending);
        let mut ids: Vec<String> = Vec::new();
        for p in pending {
            self.record_target(
                &p.command_id,
                p.target_pos,
                TargetStatus::Cancelled,
                None,
                None,
            );
            if !ids.contains(&p.command_id) {
                ids.push(p.command_id);
            }
        }
        for id in ids {
            self.rollup_command(&id);
        }
        self.lag_ticks = 0;
        Ok(())
    }

    fn status(&self) -> EngineStatus {
        let queued = (0..self.world.home_count())
            .filter_map(|i| self.world.home(i))
            .map(batsim_core::home::Home::queued_len)
            .sum();
        EngineStatus {
            state: self.state,
            tick: self.world.clock().tick(),
            unix: self.world.clock().unix_time(),
            dt_s: self.world.clock().dt_s(),
            master_seed: self.world.master_seed(),
            home_count: self.world.home_count(),
            speed: self.speed,
            achieved_speed: self.achieved_speed,
            lag_ticks: self.lag_ticks,
            queued_commands: queued,
        }
    }

    /// Advance `n` ticks synchronously; returns the outcome.
    fn advance(&mut self, n: u64) -> StepOutcome {
        let start = Instant::now();
        for _ in 0..n {
            self.do_tick();
        }
        StepOutcome {
            ticks: n,
            tick: self.world.clock().tick(),
            unix: self.world.clock().unix_time(),
            wall_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// One paced tick while running, plus schedule bookkeeping.
    fn tick_running(&mut self) {
        self.do_tick();
        self.window_ticks += 1;
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_secs(2) {
            let dt = f64::from(self.world.clock().dt_s());
            self.achieved_speed = self.window_ticks as f64 * dt / elapsed.as_secs_f64();
            self.window_start = Instant::now();
            self.window_ticks = 0;
        }
        let interval = self.tick_interval();
        if interval.is_zero() {
            return;
        }
        self.next_deadline += interval;
        let now = Instant::now();
        if self.next_deadline < now {
            let behind = now.duration_since(self.next_deadline);
            let missed = (behind.as_secs_f64() / interval.as_secs_f64()) as u64;
            self.lag_ticks = self.lag_ticks.saturating_add(missed);
            self.next_deadline = now + interval;
        } else {
            self.lag_ticks = 0;
        }
    }

    /// One committed tick: physics, telemetry, dispatch bookkeeping,
    /// broadcast.
    fn do_tick(&mut self) {
        let tick = self.world.clock().tick();
        let unix = self.world.clock().unix_time();
        self.world.step();
        let price = self.price.price_at(unix);

        let active = self.slots.iter().filter(|s| s.1).count();
        let include_raw = active <= self.raw_stream_max_homes;
        let mut rows: Vec<HomeTickRow> = Vec::new();
        let mut fleet_acc: std::collections::HashMap<String, (usize, f64, f64, f64, f64, f64)> =
            std::collections::HashMap::new();
        let mut points: Vec<(u64, TickPoint)> = Vec::with_capacity(active);

        for idx in 0..self.world.home_count() {
            let Some(home) = self.world.home_mut(idx) else {
                continue;
            };
            if home.is_retired() {
                continue;
            }
            for truth in home.take_truth() {
                let point = TickPoint::from_truth(&truth, price);
                let idx64 = idx as u64;
                self.telemetry.push(idx64, point);
                points.push((idx64, point));
                if let Some((meta, _)) = self.slots.get(idx) {
                    if include_raw {
                        rows.push(HomeTickRow {
                            home_id: meta.home_id.clone(),
                            soc: point.soc,
                            battery_power_kw: point.batt_w / 1000.0,
                            pv_power_kw: point.pv_w / 1000.0,
                            load_power_kw: point.load_w / 1000.0,
                            grid_power_kw: point.grid_w / 1000.0,
                        });
                    }
                    if let Some(fleet) = &meta.fleet_id {
                        let e = fleet_acc.entry(fleet.clone()).or_default();
                        e.0 += 1;
                        e.1 += point.batt_w;
                        e.2 += point.pv_w;
                        e.3 += point.load_w;
                        e.4 += point.grid_w;
                        e.5 += point.soc;
                    }
                }
            }
        }

        self.process_pending(tick, &points);

        let fleets = fleet_acc
            .into_iter()
            .map(|(fleet_id, (n, batt, pv, load, grid, soc))| FleetTick {
                fleet_id,
                homes: n,
                battery_power_kw: batt / 1000.0,
                pv_power_kw: pv / 1000.0,
                load_power_kw: load / 1000.0,
                grid_power_kw: grid / 1000.0,
                soc_mean: if n == 0 { 0.0 } else { soc / n as f64 },
            })
            .collect();
        let event = SimEvent::Tick(TickEvent {
            sim_time: rfc3339_of(unix),
            unix,
            tick,
            price_rtm: price,
            fleets,
            homes: include_raw.then_some(rows),
        });
        drop(self.events.send(event));

        self.backtest_tick(unix, tick);
    }

    /// Apply execution outcomes for everything due at or before `tick`.
    fn process_pending(&mut self, tick: u64, points: &[(u64, TickPoint)]) {
        if self.pending.is_empty() {
            return;
        }
        let sim_time = rfc3339_of(self.world.clock().unix_time());
        let mut keep = Vec::with_capacity(self.pending.len());
        let mut finished: Vec<String> = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            if p.execute_at_tick <= tick {
                let point = points
                    .iter()
                    .find(|(idx, _)| *idx == p.home_idx)
                    .map(|(_, pt)| pt);
                let (status, applied_kw) = match (p.requested_kw, point) {
                    (_, None) => (TargetStatus::Timeout, None),
                    (None, Some(_)) => (TargetStatus::Applied, None),
                    (Some(req), Some(pt)) => {
                        let applied = pt.batt_w / 1000.0;
                        let ratio = if req.abs() < f64::EPSILON {
                            1.0
                        } else {
                            applied / req
                        };
                        let status = if ratio >= 0.95 {
                            TargetStatus::Applied
                        } else if ratio >= 0.05 {
                            TargetStatus::Partial
                        } else {
                            TargetStatus::Rejected
                        };
                        (status, Some(applied))
                    }
                };
                self.record_target(
                    &p.command_id,
                    p.target_pos,
                    status,
                    applied_kw,
                    Some(sim_time.clone()),
                );
                if !finished.contains(&p.command_id) {
                    finished.push(p.command_id);
                }
            } else if p.execute_at_tick.saturating_add(p.timeout_ticks) < tick {
                self.record_target(
                    &p.command_id,
                    p.target_pos,
                    TargetStatus::Timeout,
                    None,
                    Some(sim_time.clone()),
                );
                if !finished.contains(&p.command_id) {
                    finished.push(p.command_id);
                }
            } else {
                keep.push(p);
            }
        }
        self.pending = keep;
        for command_id in finished {
            if let Some((applied, rejected, done)) = self.rollup_command(&command_id) {
                if done {
                    drop(self.events.send(SimEvent::Dispatch {
                        command_id,
                        tick,
                        targets_applied: applied,
                        targets_rejected: rejected,
                    }));
                }
            }
        }
    }

    fn record_target(
        &self,
        command_id: &str,
        target_pos: usize,
        status: TargetStatus,
        applied_kw: Option<f64>,
        executed_at: Option<String>,
    ) {
        if let Ok(mut audit) = self.audit.write() {
            audit.record_target(command_id, target_pos, status, applied_kw, executed_at);
        }
    }

    /// Recompute a command's rollup; returns (applied, rejected, done).
    fn rollup_command(&self, command_id: &str) -> Option<(usize, usize, bool)> {
        let mut audit = self.audit.write().ok()?;
        audit.rollup(command_id)
    }

    // ---------- backtest (M3) ----------

    /// Configure a backtest run against the freshly rebound world.
    fn configure_backtest(&mut self, config: BacktestConfig) -> Result<(), String> {
        let feed = self
            .price
            .replay_feed()
            .ok_or_else(|| {
                "backtests require a replay-backed price source (scenario prices.source = \
                 replay)"
                    .to_owned()
            })?
            .clone();
        let epoch = self.world.clock().unix_time();
        let (settlement, watch, config) = settlement_for(&feed, epoch, config)?;
        let range = batsim_ercot::TimeRange::new(
            OffsetDateTime::from_unix_timestamp(i64::try_from(epoch).map_err(|_| "epoch")?)
                .map_err(|e| e.to_string())?,
            OffsetDateTime::from_unix_timestamp(
                i64::try_from(config.end_unix).map_err(|_| "end")?,
            )
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        let mut engine = SettlementEngine::new(settlement.clone());
        // Record AS awards up front; MCPC falls back to the DAM average
        // over the award window. Resolved prices are written back into the
        // stored config so deployment scoring reuses them.
        let mut resolved_awards = Vec::with_capacity(config.as_awards.len());
        for award in &config.as_awards {
            let hours = (award.end_unix - award.start_unix) as f64 / 3600.0;
            let mcpc = if let Some(m) = award.mcpc_usd_per_mw {
                m
            } else {
                dam_mcpc_avg(&feed, range, award)?
            };
            let ts = OffsetDateTime::from_unix_timestamp(
                i64::try_from(award.start_unix).map_err(|_| "start")?,
            )
            .map_err(|e| e.to_string())?;
            engine.record_as_award(award.product, ts, award.awarded_mw, hours, mcpc);
            resolved_awards.push(AsAwardInput {
                mcpc_usd_per_mw: Some(mcpc),
                ..award.clone()
            });
        }
        let config = BacktestConfig {
            as_awards: resolved_awards,
            ..config
        };

        // System-load signal for the 4CP watch (absent in archives without
        // the system_load signal; explicit candidates still work).
        let system_loads = feed
            .replay()
            .system_signals(range)
            .map(|signals| {
                signals
                    .iter()
                    .map(|s| (s.ts.unix_timestamp(), s.system_load_mw))
                    .collect()
            })
            .unwrap_or_default();

        let last_meters = self.meter_snapshots();
        self.backtest = Some(BacktestRun {
            engine,
            settlement_cfg: settlement,
            config,
            interval_start: epoch,
            last_meters,
            acc: std::collections::BTreeMap::new(),
            next_entry: 0,
            intervals_settled: 0,
            batt_export_kwh_by_interval: Vec::new(),
            net_kw_history: std::collections::BTreeMap::new(),
            system_loads,
            watch,
            failed: None,
            report: None,
        });
        Ok(())
    }

    /// Replace homes in place at their arena indices (pristine reset).
    fn reset_homes(&mut self, homes: Vec<(u64, Home)>) -> Result<(), String> {
        for (idx, home) in homes {
            match self.world.home_mut(idx as usize) {
                Some(slot) => *slot = home,
                None => return Err(format!("no home at arena index {idx}")),
            }
        }
        Ok(())
    }

    /// Meter snapshots for every active home (backtest baseline).
    fn meter_snapshots(&self) -> std::collections::BTreeMap<u64, MeterSnapshot> {
        let mut out = std::collections::BTreeMap::new();
        for idx in 0..self.world.home_count() {
            if let Some(h) = self.world.home(idx) {
                if h.is_retired() {
                    continue;
                }
                let m = h.meters();
                out.insert(
                    idx as u64,
                    MeterSnapshot {
                        import_main: m.main.import_wh,
                        export_main: m.main.export_wh,
                        export_batt: m.batt_ac.export_wh,
                    },
                );
            }
        }
        out
    }

    /// Backtest status for the API.
    fn backtest_info(&self) -> Option<BacktestInfo> {
        let run = self.backtest.as_ref()?;
        let state = if let Some(reason) = &run.failed {
            BacktestState::Failed(reason.clone())
        } else if run.report.is_some() {
            BacktestState::Settled
        } else {
            BacktestState::Running
        };
        Some(BacktestInfo {
            run_id: run.config.run_id.clone(),
            state,
            sim_time: rfc3339_of(self.world.clock().unix_time()),
            intervals_settled: run.intervals_settled,
            report: run.report.clone(),
        })
    }

    /// Per-tick backtest accounting: strategy schedule, interval close,
    /// interval energy accumulation from meter deltas, run finalize.
    /// `unix` is the pre-step tick time; the interval ending at `unix` is
    /// closed BEFORE the just-stepped `[unix, unix+dt)` energy is
    /// accumulated, so each settled interval covers exactly
    /// `[ts, ts+interval)` and no energy beyond `end_unix` is booked.
    fn backtest_tick(&mut self, unix: u64, tick: u64) {
        let inactive = self
            .backtest
            .as_ref()
            .is_none_or(|b| b.failed.is_some() || b.report.is_some());
        if inactive {
            return;
        }
        // Fire due strategy entries (execute next tick: the dispatch stage
        // for this tick already ran inside `world.step`).
        loop {
            let due = self
                .backtest
                .as_ref()
                .is_some_and(|b| b.next_entry < b.config.schedule.len()
                    && b.config.schedule[b.next_entry].unix <= unix);
            if !due {
                break;
            }
            let entry = self
                .backtest
                .as_ref()
                .map(|b| b.config.schedule[b.next_entry].clone());
            let Some(entry) = entry else { break };
            let dt = self.world.clock().dt_s();
            for idx in 0..self.world.home_count() {
                let Some(home) = self.world.home_mut(idx) else {
                    continue;
                };
                if home.is_retired() {
                    continue;
                }
                for action in
                    crate::routes::dispatch::actions_for(&entry.spec, tick + 1, dt, 0)
                {
                    home.schedule(action);
                }
            }
            if let Some(b) = self.backtest.as_mut() {
                b.next_entry += 1;
            }
        }

        let interval_secs = self
            .backtest
            .as_ref()
            .map_or(900, |b| u64::from(b.config.interval_secs));
        let interval_start = self.backtest.as_ref().map_or(0, |b| b.interval_start);
        if unix % interval_secs == 0 && unix > interval_start {
            self.close_backtest_interval(unix);
            if self.backtest.as_ref().is_some_and(|b| b.failed.is_some()) {
                return;
            }
        }
        let end = self.backtest.as_ref().map_or(u64::MAX, |b| b.config.end_unix);
        if unix >= end {
            let open = self.backtest.as_ref().is_some_and(|b| b.interval_start < unix);
            if open {
                self.close_backtest_interval(unix);
            }
            self.finalize_backtest();
            return;
        }

        // Accumulate meter deltas for the step [unix, unix+dt).
        for idx in 0..self.world.home_count() {
            let Some(home) = self.world.home(idx) else {
                continue;
            };
            if home.is_retired() {
                continue;
            }
            let m = home.meters();
            let now = MeterSnapshot {
                import_main: m.main.import_wh,
                export_main: m.main.export_wh,
                export_batt: m.batt_ac.export_wh,
            };
            let Some(b) = self.backtest.as_mut() else {
                return;
            };
            let last = b.last_meters.insert(idx as u64, now).unwrap_or_default();
            let acc = b.acc.entry(idx as u64).or_default();
            acc.import_main += now.import_main - last.import_main;
            acc.export_main += now.export_main - last.export_main;
            acc.export_batt += now.export_batt - last.export_batt;
        }
    }

    /// Close settlement interval `[interval_start, unix)` and broadcast it.
    fn close_backtest_interval(&mut self, unix: u64) {
        let Some(feed) = self.price.replay_feed().cloned() else {
            return;
        };
        let interval_start = self.backtest.as_ref().map_or(0, |b| b.interval_start);
        let interval_secs = self.backtest.as_ref().map_or(900, |b| b.config.interval_secs);
        let Some(sample) = feed.sample_at(interval_start) else {
            if let Some(b) = self.backtest.as_mut() {
                b.failed = Some(format!(
                    "no RTM price coverage at {}",
                    rfc3339_of(interval_start)
                ));
            }
            self.state = SimState::Stopped;
            return;
        };

        // Per-home net export kWh (export positive) from meter deltas.
        let mut rows: Vec<(String, f64)> = Vec::new();
        let mut batt_export_kwh = 0.0;
        let mut net_kw: Vec<(u64, f64)> = Vec::new();
        if let Some(b) = self.backtest.as_mut() {
            let acc = std::mem::take(&mut b.acc);
            for (idx, a) in acc {
                let export_kwh = (a.export_main - a.import_main) / 1000.0;
                batt_export_kwh += a.export_batt / 1000.0;
                let hours = f64::from(interval_secs) / 3600.0;
                net_kw.push((idx, (a.import_main - a.export_main) / 1000.0 / hours));
                if let Some((meta, _)) = self.slots.get(idx as usize) {
                    rows.push((meta.home_id.clone(), export_kwh));
                }
            }
            b.interval_start = unix;
            b.intervals_settled += 1;
            b.batt_export_kwh_by_interval
                .push((interval_start, batt_export_kwh));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let refs: Vec<(&str, f64)> = rows.iter().map(|(id, e)| (id.as_str(), *e)).collect();
        let ts = OffsetDateTime::from_unix_timestamp(i64::try_from(interval_start).unwrap_or(0))
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);

        // 4CP: explicit candidates, or the watch over the system-load
        // signal. Baseline: MeteredBeforeAfter over the previous 4
        // intervals (per home).
        let explicit = self
            .backtest
            .as_ref()
            .is_some_and(|b| b.config.four_cp_candidates.contains(&interval_start));
        let watch_hit = self
            .backtest
            .as_mut()
            .and_then(|b| {
                b.system_loads.get(&i64::try_from(interval_start).unwrap_or(0)).map(|load| {
                    b.watch.observe(&batsim_ercot::SystemSignal {
                        ts,
                        system_load_mw: *load,
                        reserves_mw: None,
                        fuel_mix: None,
                    })
                })
            })
            .unwrap_or(false);
        if explicit || watch_hit {
            let mut per_home: Vec<(String, f64)> = Vec::new();
            let mut fleet_reduction_kw = 0.0;
            if let Some(b) = self.backtest.as_mut() {
                for (idx, current_kw) in &net_kw {
                    let history = b.net_kw_history.entry(*idx).or_default();
                    let baseline = if history.is_empty() {
                        *current_kw
                    } else {
                        history.iter().sum::<f64>() / history.len() as f64
                    };
                    let reduction = (baseline - current_kw).max(0.0);
                    fleet_reduction_kw += reduction;
                    if let Some((meta, _)) = self.slots.get(*idx as usize) {
                        per_home.push((meta.home_id.clone(), reduction));
                    }
                }
            }
            per_home.sort_by(|a, b| a.0.cmp(&b.0));
            let refs: Vec<(&str, f64)> =
                per_home.iter().map(|(id, r)| (id.as_str(), *r)).collect();
            if let Some(b) = self.backtest.as_mut() {
                b.engine.flag_4cp_candidate(ts, fleet_reduction_kw, &refs);
            }
        }
        // Update baseline history after scoring (bounded to 4 intervals).
        if let Some(b) = self.backtest.as_mut() {
            for (idx, kw) in net_kw {
                let history = b.net_kw_history.entry(idx).or_default();
                history.push_back(kw);
                while history.len() > 4 {
                    history.pop_front();
                }
            }
            b.engine.record_interval(ts, &refs, &sample);
            if let Some(row) = b.engine.last_interval() {
                drop(self.events.send(SimEvent::Settlement(Box::new(row.clone()))));
            }
        }
    }

    /// Finalize the run: score AS deployments, produce the report, stop.
    fn finalize_backtest(&mut self) {
        let Some(run) = self.backtest.as_mut() else {
            return;
        };
        if run.failed.is_some() {
            self.state = SimState::Stopped;
            return;
        }
        let awards = run.config.as_awards.clone();
        for award in &awards {
            if !award.deployed {
                continue;
            }
            let delivered_mwh: f64 = run
                .batt_export_kwh_by_interval
                .iter()
                .filter(|(ts, _)| *ts >= award.start_unix && *ts < award.end_unix)
                .map(|(_, kwh)| kwh / 1000.0)
                .sum();
            let mcpc_avg = award.mcpc_usd_per_mw.unwrap_or(0.0);
            let start =
                OffsetDateTime::from_unix_timestamp(i64::try_from(award.start_unix).unwrap_or(0))
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
            let end =
                OffsetDateTime::from_unix_timestamp(i64::try_from(award.end_unix).unwrap_or(0))
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
            run.engine.record_as_deployment(
                award.product,
                start,
                end,
                award.awarded_mw,
                delivered_mwh,
                mcpc_avg,
            );
        }
        let run_id = run.config.run_id.clone();
        let tick = self.world.clock().tick();
        let engine = std::mem::replace(
            &mut run.engine,
            SettlementEngine::new(run.settlement_cfg.clone()),
        );
        let report = engine.finish(run_id.clone());
        if let Some(run) = self.backtest.as_mut() {
            run.report = Some(report);
        }
        self.state = SimState::Stopped;
        drop(self.events.send(SimEvent::RunFinished { run_id, tick }));
    }
}

/// Resolve the settlement interval (0 = feed cadence), validate epoch
/// alignment, and build the settlement config + 4CP watch.
fn settlement_for(
    feed: &crate::price::ReplayFeed,
    epoch: u64,
    config: BacktestConfig,
) -> Result<(SettlementConfig, batsim_ercot::four_cp::FourCpWatch, BacktestConfig), String> {
    let interval_secs = if config.interval_secs == 0 {
        feed.interval_secs()
    } else {
        config.interval_secs
    };
    if interval_secs != feed.interval_secs() {
        return Err(format!(
            "settlement interval {interval_secs} does not match the replay RTM cadence {}",
            feed.interval_secs()
        ));
    }
    if config.end_unix <= epoch {
        return Err("backtest end must be after the scenario start".to_owned());
    }
    if epoch % u64::from(interval_secs) != 0 {
        return Err("scenario start must align to the settlement interval".to_owned());
    }
    let rules = batsim_ercot::rules::ErcotRules::current().map_err(|e| e.to_string())?;
    let watch = batsim_ercot::four_cp::FourCpWatch::new(&rules);
    // Provenance comes from the loaded data, never from the request: a
    // synthetic archive must never produce a settlement-final report.
    let provenance = feed
        .sample_at(epoch)
        .map_or(config.provenance, |s| s.provenance);
    let settlement = SettlementConfig {
        location: config.location.clone(),
        settlement_interval_secs: interval_secs,
        retail_rate: config.retail_rate.clone(),
        baseline_method_label: config.baseline_method_label.clone(),
        transmission_rate_usd_per_kw_mo: config.transmission_rate_usd_per_kw_mo,
        program_costs_usd: config.program_costs_usd,
        incentives_usd: config.incentives_usd.clone(),
        provenance,
        rules,
    };
    let config = BacktestConfig {
        interval_secs,
        ..config
    };
    Ok((settlement, watch, config))
}

/// DAM MCPC average for one award window (error when the archive has no
/// DAM AS prices for the product in the window).
fn dam_mcpc_avg(
    feed: &crate::price::ReplayFeed,
    range: batsim_ercot::TimeRange,
    award: &AsAwardInput,
) -> Result<f64, String> {
    let prices = feed.replay().as_prices(range).map_err(|e| e.to_string())?;
    let start = i64::try_from(award.start_unix).unwrap_or(0);
    let end = i64::try_from(award.end_unix).unwrap_or(0);
    let window: Vec<f64> = prices
        .iter()
        .filter(|p| {
            p.product == award.product
                && p.ts.unix_timestamp() >= start
                && p.ts.unix_timestamp() < end
        })
        .map(|p| p.mcpc_usd_per_mw)
        .collect();
    if window.is_empty() {
        return Err(format!(
            "no DAM MCPC for {} in the award window; pass mcpc_usd_per_mw",
            award.product
        ));
    }
    Ok(window.iter().sum::<f64>() / window.len() as f64)
}

/// Spawn the engine thread.
///
/// # Errors
/// Returns the thread-spawn failure.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    world: SimWorld,
    speed: f64,
    price: PriceSource,
    raw_cap: usize,
    rollup_cap: usize,
    raw_stream_max_homes: usize,
    stream_buffer: usize,
    audit: Arc<RwLock<AuditStore>>,
) -> std::io::Result<(EngineHandle, broadcast::Sender<SimEvent>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (events, _) = broadcast::channel(stream_buffer);
    let engine_events = events.clone();
    std::thread::Builder::new()
        .name("batsim-engine".to_owned())
        .spawn(move || {
            let mut engine = Engine {
                world,
                state: SimState::Stopped,
                speed,
                price,
                telemetry: TelemetryStore::new(raw_cap, rollup_cap),
                slots: Vec::new(),
                pending: Vec::new(),
                backtest: None,
                audit,
                events: engine_events,
                rx,
                raw_stream_max_homes,
                next_deadline: Instant::now(),
                lag_ticks: 0,
                achieved_speed: 0.0,
                window_start: Instant::now(),
                window_ticks: 0,
            };
            engine.run();
        })?;
    Ok((EngineHandle { tx }, events))
}

impl Engine {
    fn run(&mut self) {
        loop {
            if self.state == SimState::Running {
                let timeout = self.next_deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(timeout) {
                    Ok(msg) => self.handle(msg),
                    Err(RecvTimeoutError::Timeout) => self.tick_running(),
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match self.rx.recv() {
                    Ok(msg) => self.handle(msg),
                    Err(_) => break,
                }
            }
        }
    }
}
