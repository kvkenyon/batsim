//! Truth telemetry records and meter counters (spec B.9.1; M1 scope).
//!
//! M1 emits the lossless per-tick `debug_truth` stream only (B.9.2):
//! vendor noise classes, quantization, and rate decimation are F12 (M4).
//! Meter points follow the A.3 topology diagrams: MAIN, PV_AC, BATT_AC,
//! BACKUP_PANEL (backup-panel metering arrives with outages, F11/M4).

use serde::{Deserialize, Serialize};

/// Bidirectional energy meter: exact f64 accumulators (B.9.1; reported
/// quantization is an M4 concern).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Meter {
    /// Cumulative import (Wh).
    pub import_wh: f64,
    /// Cumulative export (Wh).
    pub export_wh: f64,
}

impl Meter {
    /// Accumulate one tick of signed power (W; + import, - export).
    pub fn accumulate(&mut self, p_w: f64, dt_s: u32) {
        let e = p_w * f64::from(dt_s) / 3600.0;
        if e >= 0.0 {
            self.import_wh += e;
        } else {
            self.export_wh += -e;
        }
    }
}

/// Unidirectional energy meter (PV production, battery throughput).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct EnergyCounter {
    /// Cumulative energy (Wh).
    pub wh: f64,
}

impl EnergyCounter {
    /// Accumulate one tick of unsigned power.
    pub fn accumulate(&mut self, p_w: f64, dt_s: u32) {
        self.wh += p_w.abs() * f64::from(dt_s) / 3600.0;
    }
}

/// Per-home meter points (A.3 topology diagrams).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HomeMeters {
    /// Service entrance, bidirectional. Sign convention for the
    /// instantaneous value: positive = importing from grid (B.1.5 stage 7
    /// formula `P_grid = P_load - P_pv_ac - P_inv_ac`).
    pub main: Meter,
    /// PV production meter (AC side), export-only.
    pub pv_ac: EnergyCounter,
    /// Battery system meter (AC side), bidirectional.
    pub batt_ac: Meter,
    /// Standby/self-consumption losses of battery system + controllers,
    /// accounted at the AC side (B.3.2).
    pub standby_loss: EnergyCounter,
    /// PV energy clipped at an inverter (B.3.3, B.7.4).
    pub pv_clipped: EnergyCounter,
    /// Battery discharge clipped at a shared inverter (B.3.3).
    pub batt_clipped: EnergyCounter,
}

/// One unit's truth record for a tick.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UnitTruth {
    /// SOC fraction of usable capacity.
    pub soc: f64,
    /// Realized terminal power (W; + discharge).
    pub p_term_w: f64,
    /// Terminal voltage (Thevenin; 0 when disabled).
    pub v_term_v: f64,
    /// Conversion heat this tick (W).
    pub heat_w: f64,
}

/// One home's lossless per-tick truth record (B.9.2 `debug_truth`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomeTruth {
    /// Engine tick index.
    pub tick: u64,
    /// Unix seconds of the tick.
    pub unix_time_s: u64,
    /// Home load power (W).
    pub p_load_w: f64,
    /// Critical-loads share of the load (W).
    pub p_load_critical_w: f64,
    /// PV power at the array terminals (W DC).
    pub p_pv_dc_w: f64,
    /// PV power delivered to the AC panel (W).
    pub p_pv_ac_w: f64,
    /// Battery system power at the AC panel (W; + discharge).
    pub p_batt_ac_w: f64,
    /// Grid exchange (W; + import).
    pub p_grid_w: f64,
    /// Standby draw served this tick (W).
    pub p_standby_w: f64,
    /// Per-battery-unit records, in unit order.
    pub units: Vec<UnitTruth>,
    /// Mean SOC across units (convenience for traces).
    pub soc_mean: f64,
}
