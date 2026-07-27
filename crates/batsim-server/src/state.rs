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

/// Append-only dispatch audit log (bounded).
#[derive(Debug, Default)]
pub struct AuditStore {
    records: Vec<CommandDoc>,
    by_id: HashMap<String, usize>,
    cap: usize,
}

impl AuditStore {
    /// Create with a retention cap.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            records: Vec::new(),
            by_id: HashMap::new(),
            cap,
        }
    }

    /// Insert a new command record.
    pub fn insert(&mut self, record: CommandDoc) {
        if self.records.len() >= self.cap {
            if let Some(old) = self.records.first() {
                self.by_id.remove(&old.command_id);
            }
            self.records.remove(0);
            // Re-index: positions shifted by one.
            self.by_id = self
                .records
                .iter()
                .enumerate()
                .map(|(i, r)| (r.command_id.clone(), i))
                .collect();
        }
        self.by_id
            .insert(record.command_id.clone(), self.records.len());
        self.records.push(record);
    }

    /// Look up a command.
    #[must_use]
    pub fn get(&self, command_id: &str) -> Option<&CommandDoc> {
        self.by_id.get(command_id).map(|i| &self.records[*i])
    }

    /// Whether a command id is known (deduplication).
    #[must_use]
    pub fn contains(&self, command_id: &str) -> bool {
        self.by_id.contains_key(command_id)
    }

    /// All records, oldest first.
    #[must_use]
    pub fn records(&self) -> &[CommandDoc] {
        &self.records
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
        let Some(i) = self.by_id.get(command_id).copied() else {
            return;
        };
        if let Some(target) = self.records[i].targets.get_mut(target_pos) {
            target.status = Some(status);
            target.applied_kw = applied_kw;
            target.executed_at_sim_time = executed_at;
        }
    }

    /// Recompute a command's rollup status; returns
    /// `(applied, rejected, done)` when the command exists.
    pub fn rollup(&mut self, command_id: &str) -> Option<(usize, usize, bool)> {
        let i = *self.by_id.get(command_id)?;
        let rec = &self.records[i];
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
        self.records[i].status = if done == total && all_cancelled && total > 0 {
            CommandStatus::Cancelled
        } else {
            status
        };
        Some((applied, rejected, done == total))
    }

    /// Mark every still-queued target cancelled (best-effort cancel of a
    /// command the engine has fully retracted).
    pub fn mark_queued_cancelled(&mut self, command_id: &str) {
        let Some(i) = self.by_id.get(command_id).copied() else {
            return;
        };
        for t in &mut self.records[i].targets {
            if t.status.is_none() {
                t.status = Some(TargetStatus::Cancelled);
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

/// Idempotency-key store with TTL.
#[derive(Debug)]
pub struct IdemStore {
    ttl: std::time::Duration,
    records: HashMap<String, IdemRecord>,
}

impl IdemStore {
    /// Create with the given TTL.
    #[must_use]
    pub fn new(ttl_hours: u64) -> Self {
        Self {
            ttl: std::time::Duration::from_secs(ttl_hours.saturating_mul(3600)),
            records: HashMap::new(),
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
