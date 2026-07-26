//! Dispatch: control modes and setpoint computation (B.1.5 stage 4).
//!
//! M1 modes: manual setpoint, self-consumption, backup reserve hold.
//! Market dispatch (Part D) and execution jitter arrive with the API in
//! M2+; the jitter stream (`DispatchJitter`) is already reserved.

use serde::{Deserialize, Serialize};

/// Operating modes a home's battery fleet can be in (M1 subset of the
/// Part C action enum).
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

/// A dispatch action applied at a tick boundary (M1 subset of Part C
/// `/v1/dispatch` actions).
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
/// `execute_at_tick` (B.1.5 stage 4 reads the command queue at the tick
/// top; HTTP-layer latency modeling is M2+).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScheduledDispatch {
    /// Tick at which the action takes effect.
    pub execute_at_tick: u64,
    /// The action to apply.
    pub action: DispatchAction,
}
