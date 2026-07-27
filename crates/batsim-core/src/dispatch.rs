//! Dispatch: control modes and setpoint computation (the dispatch stage
//! of the per-tick pipeline).
//!
//! Current modes: manual setpoint, self-consumption, backup reserve hold.
//! The HTTP API (`batsim-server`) exposes these actions at `/v1/dispatch`
//! and models execution latency by scheduling `execute_at_tick` in the
//! future. Market dispatch is planned future work; the jitter stream
//! (`DispatchJitter`) remains reserved.

use serde::{Deserialize, Serialize};

/// Operating modes a home's battery fleet can be in (the engine-side
/// set behind the HTTP API's `set_mode` dispatch action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMode {
    /// Do nothing; hold at standby (zero setpoint).
    Idle,
    /// Track a fixed manual setpoint (W at the home's battery-system AC
    /// boundary; + discharge, - charge).
    Manual,
    /// Net-zero grid exchange: charge on PV surplus, discharge to cover
    /// load, respecting the reserve floor.
    SelfConsumption,
    /// Hold SOC at/above the reserve; charge from any source when below.
    BackupReserveHold,
}

/// A dispatch action applied at a tick boundary (the engine-side set
/// behind the HTTP API's `/v1/dispatch` actions).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DispatchAction {
    /// Switch control mode.
    SetMode(ControlMode),
    /// Set the manual-mode setpoint (W at the battery-system AC boundary;
    /// + discharge, - charge).
    SetManualSetpoint(f64),
    /// Set the user backup-reserve floor (fraction of usable).
    SetReserve(f64),
    /// Set the PV curtailment fraction (0 = full output, 1 = fully
    /// curtailed; lossless at the MPPT).
    SetPvCurtail(f64),
}

/// A scheduled dispatch command: applied when the engine reaches
/// `execute_at_tick` (the dispatch stage reads the command queue at the
/// tick top; the HTTP layer models execution latency by scheduling
/// `execute_at_tick` in the future).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScheduledDispatch {
    /// Tick at which the action takes effect.
    pub execute_at_tick: u64,
    /// The action to apply.
    pub action: DispatchAction,
    /// Opaque issuer tag (0 = untagged). Lets an issuer retract its own
    /// still-queued commands without disturbing anyone else's.
    #[serde(default)]
    pub tag: u64,
}
