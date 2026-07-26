//! Dispatch: control modes and setpoint computation (B.1.5 stage 4).
//!
//! M1 modes: manual setpoint, self-consumption, backup reserve hold.
//! Market dispatch (Part D) and execution jitter arrive with the API in
//! M2+; the jitter stream (`DispatchJitter`) is already reserved.
//!
//! Filled in during engine integration.

/// Operating modes a home's battery fleet can be in (M1 subset of the
/// Part C action enum).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ControlMode {
    /// Do nothing; hold at standby (zero setpoint).
    Idle,
    /// Track a fixed manual setpoint (W at the fleet boundary; +
    /// discharge, - charge).
    Manual,
    /// Net-zero grid exchange: charge on PV surplus, discharge to cover
    /// load, respecting the reserve floor.
    SelfConsumption,
    /// Hold SOC at/above the reserve; charge from any source when below.
    BackupReserveHold,
}
