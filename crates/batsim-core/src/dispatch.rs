//! Dispatch: control modes and setpoint computation (the dispatch stage
//! of the per-tick pipeline).
//!
//! Current modes: manual setpoint, self-consumption, backup reserve hold.
//! Market dispatch (the planned market-dispatch layer) and execution
//! jitter arrive with the planned HTTP API; the jitter stream
//! (`DispatchJitter`) is already reserved.

use serde::{Deserialize, Serialize};

/// Operating modes a home's battery fleet can be in (the subset the
/// engine currently supports of the action enum planned for the HTTP
/// API).
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

/// A dispatch action applied at a tick boundary (the subset the engine
/// currently supports of the `/v1/dispatch` actions planned for the HTTP
/// API).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DispatchAction {
    /// Switch control mode.
    SetMode(ControlMode),
    /// Set the manual-mode setpoint (W at the battery-system AC boundary;
    /// + discharge, - charge).
    SetManualSetpoint(f64),
    /// Set the user backup-reserve floor (fraction of usable).
    SetReserve(f64),
}

/// A scheduled dispatch command: applied when the engine reaches
/// `execute_at_tick` (the dispatch stage reads the command queue at the
/// tick top; HTTP-layer latency modeling is planned future work).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScheduledDispatch {
    /// Tick at which the action takes effect.
    pub execute_at_tick: u64,
    /// The action to apply.
    pub action: DispatchAction,
}
