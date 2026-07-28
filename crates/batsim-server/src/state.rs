//! Shared application state: registry handle, engine handle, resource
//! indexes, the dispatch audit log, and the idempotency store.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use batsim_registry::Registry;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::engine::{EngineHandle, SimEvent};
use crate::model::{
    CommandDoc, CommandStatus, DispatchRequest, FleetManifest, HomeConfigDoc, ScenarioRequest,
    TargetExecution, TargetStatus,
};

/// A registered home.
#[derive(Debug, Clone)]
pub struct HomeEntry {
    /// Home id.
    pub id: String,
    /// Engine arena index.
    pub idx: u64,
    /// Fleet membership.
    pub fleet_id: Option<String>,
    /// Validated configuration echo.
    pub config: HomeConfigDoc,
    /// Wall-clock creation time (RFC 3339).
    pub created_at: String,
}

/// A registered fleet.
#[derive(Debug, Clone)]
pub struct FleetEntry {
    /// Fleet id.
    pub id: String,
    /// Name.
    pub name: String,
    /// The manifest (kept for expansions).
    pub manifest: FleetManifest,
    /// Homes created so far (ids), in expansion order.
    pub home_ids: Vec<String>,
    /// Content hash of the expansion.
    pub expansion_hash: String,
    /// `(ordinal_base, count)` ranges composed so far, in order.
    pub expansion_ordinals: Vec<(u64, u64)>,
    /// Wall-clock creation time.
    pub created_at: String,
    /// Homes created by later expansions (ordinal offset base).
    pub expanded_count: u32,
}

/// A registered scenario.
#[derive(Debug, Clone)]
pub struct ScenarioEntry {
    /// Scenario id.
    pub id: String,
    /// The binding.
    pub request: ScenarioRequest,
    /// Wall-clock creation time.
    pub created_at: String,
}

/// A backtest run record.
#[derive(Debug, Clone)]
pub struct BacktestEntry {
    /// Run id.
    pub id: String,
    /// Fleet id.
    pub fleet_id: String,
    /// Internally created scenario id.
    pub scenario_id: String,
    /// The original request.
    pub request: crate::model::BacktestRequest,
    /// Wall-clock creation time.
    pub created_at: String,
    /// Final settlement report (JSON), captured once the run settles so it
    /// survives later runs rebinding the engine.
    pub report: Option<serde_json::Value>,
}

/// Append-only dispatch audit log (bounded).
#[derive(Debug, Default)]
pub struct AuditStore {
    records: std::collections::VecDeque<CommandDoc>,
    by_id: HashMap<String, u64>,
    base_seq: u64,
    next_seq: u64,
    cap: usize,
}

impl AuditStore {
    /// Create with a retention cap.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            records: std::collections::VecDeque::new(),
            by_id: HashMap::new(),
            base_seq: 0,
            next_seq: 0,
            cap,
        }
    }

    /// Insert a new command record.
    pub fn insert(&mut self, record: CommandDoc) {
        if self.records.len() >= self.cap {
            if let Some(old) = self.records.pop_front() {
                self.by_id.remove(&old.command_id);
                self.base_seq += 1;
            }
        }
        self.by_id.insert(record.command_id.clone(), self.next_seq);
        self.next_seq += 1;
        self.records.push_back(record);
    }

    /// Position of a sequence number inside the deque.
    fn pos(&self, seq: u64) -> Option<usize> {
        usize::try_from(seq.checked_sub(self.base_seq)?).ok()
    }

    /// Look up a command.
    #[must_use]
    pub fn get(&self, command_id: &str) -> Option<&CommandDoc> {
        let seq = *self.by_id.get(command_id)?;
        self.records.get(self.pos(seq)?)
    }

    /// Whether a command id is known (deduplication).
    #[must_use]
    pub fn contains(&self, command_id: &str) -> bool {
        self.by_id.contains_key(command_id)
    }

    /// All records, oldest first.
    #[must_use]
    pub fn records(&self) -> std::collections::vec_deque::Iter<'_, CommandDoc> {
        self.records.iter()
    }

    /// Record a target's execution outcome.
    pub fn record_target(
        &mut self,
        command_id: &str,
        target_pos: usize,
        status: TargetStatus,
        applied_kw: Option<f64>,
        executed_at: Option<String>,
    ) {
        let Some(seq) = self.by_id.get(command_id).copied() else {
            return;
        };
        let Some(pos) = self.pos(seq) else {
            return;
        };
        if let Some(target) = self
            .records
            .get_mut(pos)
            .and_then(|r| r.targets.get_mut(target_pos))
        {
            target.status = Some(status);
            target.applied_kw = applied_kw;
            target.executed_at_sim_time = executed_at;
        }
    }

    /// Recompute a command's rollup status; returns
    /// `(applied, rejected, done)` when the command exists.
    pub fn rollup(&mut self, command_id: &str) -> Option<(usize, usize, bool)> {
        let pos = self.pos(*self.by_id.get(command_id)?)?;
        let rec = &self.records[pos];
        let total = rec.targets.len();
        let mut applied = 0usize;
        let mut clean = 0usize;
        let mut done = 0usize;
        for t in &rec.targets {
            match t.status {
                None => {}
                Some(TargetStatus::Applied) => {
                    applied += 1;
                    clean += 1;
                    done += 1;
                }
                Some(_) => done += 1,
            }
        }
        let rejected = done - clean;
        let status = if done == 0 {
            CommandStatus::Queued
        } else if done < total {
            CommandStatus::InFlight
        } else if rejected == 0 {
            CommandStatus::Completed
        } else {
            CommandStatus::CompletedWithErrors
        };
        let all_cancelled = rec
            .targets
            .iter()
            .all(|t| t.status == Some(TargetStatus::Cancelled));
        self.records[pos].status = if done == total && all_cancelled && total > 0 {
            CommandStatus::Cancelled
        } else {
            status
        };
        Some((applied, rejected, done == total))
    }

    /// Mark every still-queued target cancelled (best-effort cancel of a
    /// command the engine has fully retracted).
    pub fn mark_queued_cancelled(&mut self, command_id: &str) {
        let Some(seq) = self.by_id.get(command_id).copied() else {
            return;
        };
        let Some(pos) = self.pos(seq) else {
            return;
        };
        if let Some(rec) = self.records.get_mut(pos) {
            for t in &mut rec.targets {
                if t.status.is_none() {
                    t.status = Some(TargetStatus::Cancelled);
                }
            }
        }
    }
}

/// A stored idempotency record.
#[derive(Debug, Clone)]
pub struct IdemRecord {
    /// SHA-256 hash of the canonical request body.
    pub body_hash: u64,
    /// Stored response status.
    pub status: u16,
    /// Stored response body.
    pub body: serde_json::Value,
    /// Insertion time.
    pub created: Instant,
}

/// Outcome of reserving an idempotency key for one in-flight request.
#[derive(Debug)]
pub enum IdemReservation {
    /// The key is free; this caller now owns it and must complete or
    /// abort the reservation.
    Reserved,
    /// A completed request with the same body hash exists; replay it.
    Replay(IdemRecord),
    /// A completed request with a different body hash exists.
    ConflictReuse,
    /// Another request with this key is still executing.
    InFlight,
}

/// Idempotency-key store with TTL.
#[derive(Debug)]
pub struct IdemStore {
    ttl: std::time::Duration,
    records: HashMap<String, IdemRecord>,
    pending: std::collections::HashSet<String>,
}

impl IdemStore {
    /// Create with the given TTL.
    #[must_use]
    pub fn new(ttl_hours: u64) -> Self {
        Self {
            ttl: std::time::Duration::from_secs(ttl_hours.saturating_mul(3600)),
            records: HashMap::new(),
            pending: std::collections::HashSet::new(),
        }
    }

    /// Fetch a live record for `key`.
    #[must_use]
    pub fn get(&mut self, key: &str) -> Option<&IdemRecord> {
        if self
            .records
            .get(key)
            .is_some_and(|r| r.created.elapsed() > self.ttl)
        {
            self.records.remove(key);
        }
        self.records.get(key)
    }

    /// Atomically check a key and reserve it when free, so concurrent
    /// requests carrying the same key cannot both execute.
    pub fn reserve(&mut self, key: &str, body_hash: u64) -> IdemReservation {
        if self.pending.contains(key) {
            return IdemReservation::InFlight;
        }
        if let Some(rec) = self.get(key) {
            let rec = rec.clone();
            return if rec.body_hash == body_hash {
                IdemReservation::Replay(rec)
            } else {
                IdemReservation::ConflictReuse
            };
        }
        self.pending.insert(key.to_owned());
        IdemReservation::Reserved
    }

    /// Complete a reservation with the produced response.
    pub fn complete(&mut self, key: &str, record: IdemRecord) {
        self.pending.remove(key);
        self.put(key.to_owned(), record);
    }

    /// Abandon a reservation whose request failed.
    pub fn abort(&mut self, key: &str) {
        self.pending.remove(key);
    }

    /// Store a record.
    pub fn put(&mut self, key: String, record: IdemRecord) {
        // Lazy sweep when growing large.
        if self.records.len() > 4096 {
            let ttl = self.ttl;
            self.records.retain(|_, r| r.created.elapsed() <= ttl);
        }
        self.records.insert(key, record);
    }
}

/// The shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Effective configuration.
    pub config: Arc<Config>,
    /// Device catalog.
    pub registry: Arc<Registry>,
    /// Engine command channel.
    pub engine: EngineHandle,
    /// Live-event broadcast (subscribe for SSE/WS).
    pub events: broadcast::Sender<SimEvent>,
    /// Homes by id.
    pub homes: Arc<RwLock<HashMap<String, HomeEntry>>>,
    /// Fleets by id.
    pub fleets: Arc<RwLock<HashMap<String, FleetEntry>>>,
    /// Scenarios by id.
    pub scenarios: Arc<RwLock<HashMap<String, ScenarioEntry>>>,
    /// Backtest runs by id.
    pub backtests: Arc<RwLock<HashMap<String, BacktestEntry>>>,
    /// The active scenario id, if any.
    pub active_scenario: Arc<RwLock<Option<String>>>,
    /// Dispatch audit log.
    pub audit: Arc<RwLock<AuditStore>>,
    /// Idempotency store.
    pub idempotency: Arc<RwLock<IdemStore>>,
    /// Wall-clock start instant.
    pub started: Instant,
    /// Serializes home composition against arena-index assignment.
    pub compose_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    /// Read a home entry by id.
    #[must_use]
    pub fn home(&self, id: &str) -> Option<HomeEntry> {
        self.homes.read().ok()?.get(id).cloned()
    }

    /// Read a fleet entry by id.
    #[must_use]
    pub fn fleet(&self, id: &str) -> Option<FleetEntry> {
        self.fleets.read().ok()?.get(id).cloned()
    }

    /// Read a scenario entry by id.
    #[must_use]
    pub fn scenario(&self, id: &str) -> Option<ScenarioEntry> {
        self.scenarios.read().ok()?.get(id).cloned()
    }

    /// Read a backtest entry by id.
    #[must_use]
    pub fn backtest(&self, id: &str) -> Option<BacktestEntry> {
        self.backtests.read().ok()?.get(id).cloned()
    }

    /// Build a dispatch command record for insertion.
    #[must_use]
    pub fn command_record(
        &self,
        command_id: String,
        principal: String,
        idempotency_key: Option<String>,
        request_hash: String,
        request: DispatchRequest,
        targets: Vec<TargetExecution>,
    ) -> CommandDoc {
        CommandDoc {
            command_id,
            status: CommandStatus::Queued,
            created_at: crate::engine::rfc3339_of(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            ),
            principal,
            idempotency_key,
            request_hash,
            request,
            targets,
        }
    }
}
