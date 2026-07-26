//! One simulated home: the B.1.5 per-tick pipeline over its devices, with
//! coupling-aware energy paths (spec A.3, B.3.4; F16).
//!
//! Stage order per tick is mandatory (B.1.5): load -> pv -> price_signal
//! (M3) -> dispatch -> battery -> inverter -> metering -> telemetry.
//!
//! M1 scope: grid is always present (outages F11 are M4); the dispatch
//! stage reads stages 1-2 of the same tick (allowed by B.1.5); telemetry
//! is the lossless truth stream only.
//!
//! Sign conventions (B.0): battery power positive = discharging; grid
//! power positive = importing (B.1.5 stage 7 formula).

use serde::{Deserialize, Serialize};

use crate::battery::BatteryStepInput;
use crate::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use crate::telemetry::{HomeMeters, HomeTruth, UnitTruth};
use crate::topology::{is_ac_terminal, HomeDevices};

/// One home: devices + control state + meters (+ optional truth trace).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Home {
    devices: HomeDevices,
    mode: ControlMode,
    manual_setpoint_w: f64,
    dispatch_queue: Vec<ScheduledDispatch>,
    meters: HomeMeters,
    record_truth: bool,
    truth: Vec<HomeTruth>,
}

/// Intermediate per-tick exogenous values shared across stages.
#[derive(Debug, Clone, Copy, Default)]
struct Exogenous {
    load: f64,
    load_critical: f64,
    pv_dc: f64,
    pv_ac: f64,
    pv_clipped: f64,
}

impl Home {
    /// Build a home from its constructed device set.
    #[must_use]
    pub fn new(devices: HomeDevices, record_truth: bool) -> Self {
        Self {
            devices,
            mode: ControlMode::SelfConsumption,
            manual_setpoint_w: 0.0,
            dispatch_queue: Vec::new(),
            meters: HomeMeters::default(),
            record_truth,
            truth: Vec::new(),
        }
    }

    /// Queue a dispatch action (applied at the top of its tick, B.1.5
    /// stage 4). Queue order is preserved for equal ticks.
    pub fn schedule(&mut self, cmd: ScheduledDispatch) {
        self.dispatch_queue.push(cmd);
    }

    /// Set the control mode immediately (synchronous API path for tests).
    pub fn set_mode(&mut self, mode: ControlMode) {
        self.mode = mode;
    }

    /// Set the manual-mode setpoint (W; + discharge, - charge).
    pub fn set_manual_setpoint_w(&mut self, p_w: f64) {
        self.manual_setpoint_w = p_w;
    }

    /// Set the user backup-reserve floor on all units (fraction of usable).
    pub fn set_reserve_frac(&mut self, frac: f64) {
        for unit in &mut self.devices.batteries {
            unit.set_reserve_frac(frac);
        }
    }

    /// The home's meters.
    #[must_use]
    pub fn meters(&self) -> &HomeMeters {
        &self.meters
    }

    /// The device set.
    #[must_use]
    pub fn devices(&self) -> &HomeDevices {
        &self.devices
    }

    /// Mean SOC across battery units (0 when no batteries).
    #[must_use]
    pub fn soc_mean(&self) -> f64 {
        let n = self.devices.batteries.len();
        if n == 0 {
            return 0.0;
        }
        self.devices
            .batteries
            .iter()
            .map(crate::battery::BatteryUnit::soc)
            .sum::<f64>()
            / n as f64
    }

    /// Drain the recorded truth trace (leaves recording on).
    pub fn take_truth(&mut self) -> Vec<HomeTruth> {
        std::mem::take(&mut self.truth)
    }

    /// The recorded truth trace so far.
    #[must_use]
    pub fn truth(&self) -> &[HomeTruth] {
        &self.truth
    }

    /// Execute one tick of the B.1.5 pipeline.
    pub fn step(&mut self, tick: u64, unix_time_s: u64, dt_s: u32, t_amb_c: f64) {
        self.apply_due_dispatches(tick);
        let exo = self.stages_load_pv(tick, unix_time_s, dt_s, t_amb_c);
        let p_batt_ac_set = self.stage_dispatch(&exo);
        let unit_realized = self.stage_battery(p_batt_ac_set, &exo, dt_s, t_amb_c);
        let (p_pv_ac, p_batt_ac) = self.stage_inverter(&exo, &unit_realized, dt_s);
        self.stage_metering(&exo, p_pv_ac, p_batt_ac, dt_s);
        self.stage_telemetry(tick, unix_time_s, &exo, p_pv_ac, p_batt_ac, &unit_realized);
    }

    /// Tick-top: apply due dispatch actions in queue order.
    fn apply_due_dispatches(&mut self, tick: u64) {
        let split = self
            .dispatch_queue
            .partition_point(|c| c.execute_at_tick <= tick);
        let due: Vec<ScheduledDispatch> = self.dispatch_queue.drain(..split).collect();
        for cmd in due {
            match cmd.action {
                DispatchAction::SetMode(mode) => self.mode = mode,
                DispatchAction::SetManualSetpoint(p) => self.manual_setpoint_w = p,
                DispatchAction::SetReserve(frac) => self.set_reserve_frac(frac),
            }
        }
    }

    /// Stages 1-2: load and PV (B.1.5).
    fn stages_load_pv(
        &mut self,
        tick: u64,
        unix_time_s: u64,
        dt_s: u32,
        t_amb_c: f64,
    ) -> Exogenous {
        let p_load = self.devices.load.power_w(unix_time_s, tick, dt_s, t_amb_c);
        let p_load_critical = self.devices.load.last_critical_w();
        let p_pv_dc = self
            .devices
            .pv
            .as_mut()
            .map_or(0.0, |pv| pv.dc_power_w(unix_time_s, tick, dt_s, t_amb_c));
        // Dedicated string inverter: PV converts here (loss L1, A.3.2) and
        // never touches the battery path. Hybrid: PV DC lands on the bus
        // and converts in stage 6.
        let (pv_ac_conv, pv_clipped_conv) = match (&self.devices.pv_inverter, p_pv_dc > 0.0) {
            (Some(inv), true) => {
                let conv = inv.dc_to_ac(p_pv_dc);
                (conv.p_out_w, conv.clipped_w)
            }
            _ => (0.0, 0.0),
        };
        Exogenous {
            load: p_load,
            load_critical: p_load_critical,
            pv_dc: p_pv_dc,
            pv_ac: pv_ac_conv,
            pv_clipped: pv_clipped_conv,
        }
    }

    /// Stage 4: compute the battery-system AC-boundary setpoint from the
    /// active control mode (W; + discharge).
    fn stage_dispatch(&self, exo: &Exogenous) -> f64 {
        match self.mode {
            ControlMode::Idle => 0.0,
            ControlMode::Manual => self.manual_setpoint_w,
            ControlMode::SelfConsumption => {
                let p_pv_ac_est = if self.devices.pv_inverter.is_some() {
                    exo.pv_ac
                } else {
                    // Hybrid: PV converts at the shared inverter in stage 6;
                    // estimate its AC value for the setpoint.
                    self.devices.hybrid_inverter.as_ref().map_or(0.0, |inv| {
                        let conv = inv.dc_to_ac(exo.pv_dc);
                        conv.p_out_w
                    })
                };
                exo.load - p_pv_ac_est
            }
            ControlMode::BackupReserveHold => {
                let below = self
                    .devices
                    .batteries
                    .iter()
                    .any(|u| u.soc() < u.reserve_frac());
                if below {
                    // Recharge toward the reserve at a gentle rate; the
                    // units clamp to their own limits.
                    -0.25
                        * self
                            .devices
                            .batteries
                            .iter()
                            .map(crate::battery::BatteryUnit::max_charge_w)
                            .sum::<f64>()
                } else {
                    0.0
                }
            }
        }
    }

    /// Stage 5: split the setpoint across units (pro-rata by headroom,
    /// B.3.4) and step each unit. Hybrid units get DC-bus setpoints:
    /// PV-surplus charging routes DC->DC (single inversion, A.3.3); AC-side
    /// setpoints translate through the hybrid curve (grid charge remains a
    /// double conversion).
    fn stage_battery(
        &mut self,
        p_batt_ac_set: f64,
        exo: &Exogenous,
        dt_s: u32,
        t_amb_c: f64,
    ) -> Vec<f64> {
        let n = self.devices.batteries.len();
        let mut realized = vec![0.0; n];
        if n == 0 {
            return realized;
        }
        // Separate terminal classes.
        let ac_idx: Vec<usize> = (0..n)
            .filter(|&i| is_ac_terminal(self.devices.batteries[i].model().coupling))
            .collect();
        let dc_idx: Vec<usize> = (0..n).filter(|&i| !ac_idx.contains(&i)).collect();

        // AC-terminal units split the AC setpoint pro-rata by headroom.
        Self::split_and_step(
            &mut self.devices.batteries,
            &ac_idx,
            p_batt_ac_set,
            dt_s,
            t_amb_c,
            &mut realized,
        );

        // Hybrid (DC-bus) units.
        if !dc_idx.is_empty() {
            // Efficiency of the shared hybrid at an AC power (curve x-axis
            // is AC kW); one fixed-point step from the DC side is ample.
            let hyb_eta = |p_w: f64| -> f64 {
                self.devices
                    .hybrid_inverter
                    .as_ref()
                    .map_or(0.97, |inv| {
                        inv.model().efficiency_curve.eval(p_w.abs() / 1000.0)
                    })
                    .max(1e-6)
            };
            let pv_surplus_dc = if matches!(self.mode, ControlMode::SelfConsumption)
                && self.devices.pv_inverter.is_none()
            {
                // DC left on the bus after covering the load (single-
                // inversion PV->battery path, A.3.3).
                (exo.pv_dc - exo.load / hyb_eta(exo.load)).max(0.0)
            } else {
                0.0
            };
            let p_dc_set = if pv_surplus_dc > 0.0 && p_batt_ac_set <= 0.0 {
                -pv_surplus_dc
            } else if p_batt_ac_set >= 0.0 {
                // Discharge: DC required from the battery for an AC target.
                p_batt_ac_set / hyb_eta(p_batt_ac_set)
            } else {
                // Charge: DC delivered to the bus from an AC draw
                // (conservation-true: DC = AC x eta, D1 decision).
                p_batt_ac_set * hyb_eta(-p_batt_ac_set)
            };
            Self::split_and_step(
                &mut self.devices.batteries,
                &dc_idx,
                p_dc_set,
                dt_s,
                t_amb_c,
                &mut realized,
            );
        }
        realized
    }

    /// Split a setpoint across the given unit indices pro-rata by dynamic
    /// headroom and step each (B.3.4).
    fn split_and_step(
        batteries: &mut [crate::battery::BatteryUnit],
        indices: &[usize],
        p_set: f64,
        dt_s: u32,
        t_amb_c: f64,
        realized: &mut [f64],
    ) {
        if indices.is_empty() || p_set == 0.0 {
            for &i in indices {
                realized[i] = step_one(&mut batteries[i], 0.0, dt_s, t_amb_c);
            }
            return;
        }
        let headroom: Vec<f64> = indices
            .iter()
            .map(|&i| {
                if p_set > 0.0 {
                    batteries[i].max_discharge_w()
                } else {
                    batteries[i].max_charge_w()
                }
            })
            .collect();
        let total: f64 = headroom.iter().sum();
        for (slot, &i) in indices.iter().enumerate() {
            let share = if total > 0.0 {
                p_set * headroom[slot] / total
            } else {
                0.0
            };
            realized[i] = step_one(&mut batteries[i], share, dt_s, t_amb_c);
        }
    }

    /// Stage 6: resolve the hybrid inverter's shared AC cap (PV priority,
    /// B.3.3) and return (pv_ac_w, batt_ac_w) at the panel.
    fn stage_inverter(&mut self, exo: &Exogenous, unit_realized: &[f64], dt_s: u32) -> (f64, f64) {
        let mut p_batt_ac: f64 = unit_realized
            .iter()
            .zip(&self.devices.batteries)
            .filter(|(_, u)| is_ac_terminal(u.model().coupling))
            .map(|(p, _)| *p)
            .sum();
        let mut p_pv_ac = exo.pv_ac;

        if let Some(inv) = &self.devices.hybrid_inverter {
            let hyb_batt_dc: f64 = unit_realized
                .iter()
                .zip(&self.devices.batteries)
                .filter(|(_, u)| !is_ac_terminal(u.model().coupling))
                .map(|(p, _)| *p)
                .sum();
            let p_bus = exo.pv_dc + hyb_batt_dc;
            if p_bus >= 0.0 {
                let conv = inv.dc_to_ac(p_bus);
                // Attribute AC output to PV first (metering only; the
                // physics is one conversion of the summed DC).
                let pv_share = if p_bus > 0.0 {
                    conv.p_out_w * (exo.pv_dc / p_bus).min(1.0)
                } else {
                    0.0
                };
                p_pv_ac += pv_share;
                p_batt_ac += conv.p_out_w - pv_share;
                // Clipped energy: PV priority curtails the battery second
                // (B.3.3), so the clip lands on the battery counter.
                if self.devices.pv_priority {
                    self.meters.batt_clipped.accumulate(conv.clipped_w, dt_s);
                } else {
                    self.meters.pv_clipped.accumulate(conv.clipped_w, dt_s);
                }
            } else {
                // Net bus deficit: drawn from AC through the hybrid
                // (grid charge double conversion, A.3.3). The battery
                // already absorbed its DC; AC = DC / eta, one fixed-point
                // step (conservation-true, D1 decision).
                let eta = inv.model().efficiency_curve.eval(-p_bus / 1000.0).max(1e-6);
                let p_ac_draw = -p_bus / eta;
                p_batt_ac -= p_ac_draw;
            }
        }
        (p_pv_ac, p_batt_ac)
    }

    /// Stage 7: close the balance at the meter points and integrate energy
    /// counters (B.1.5: `P_grid = P_load - P_pv_ac - P_inv_ac`, + import;
    /// standby draw added on the AC side per B.3.2).
    fn stage_metering(&mut self, exo: &Exogenous, p_pv_ac: f64, p_batt_ac: f64, dt_s: u32) {
        let p_standby = self.standby_total_w();
        let p_grid = exo.load - p_pv_ac - p_batt_ac + p_standby;
        self.meters.main.accumulate(p_grid, dt_s);
        self.meters.pv_ac.accumulate(p_pv_ac, dt_s);
        self.meters.pv_clipped.accumulate(exo.pv_clipped, dt_s);
        // Battery meter: import = charged, export = discharged.
        self.meters.batt_ac.accumulate(-p_batt_ac, dt_s);
        self.meters.standby_loss.accumulate(p_standby, dt_s);
    }

    /// Total standby draw (W): battery units + controllers (B.3.2).
    fn standby_total_w(&self) -> f64 {
        self.devices
            .batteries
            .iter()
            .map(crate::battery::BatteryUnit::standby_power_w)
            .sum::<f64>()
            + self.devices.controller_standby_w
    }

    /// Stage 8: record the lossless truth record for this tick (B.9.2).
    fn stage_telemetry(
        &mut self,
        tick: u64,
        unix_time_s: u64,
        exo: &Exogenous,
        p_pv_ac: f64,
        p_batt_ac: f64,
        unit_realized: &[f64],
    ) {
        if !self.record_truth {
            return;
        }
        let units: Vec<UnitTruth> = self
            .devices
            .batteries
            .iter()
            .zip(unit_realized)
            .map(|(u, &p)| UnitTruth {
                soc: u.soc(),
                p_term_w: p,
                v_term_v: 0.0,
                heat_w: 0.0,
            })
            .collect();
        let soc_mean = self.soc_mean();
        let p_standby = self.standby_total_w();
        self.truth.push(HomeTruth {
            tick,
            unix_time_s,
            p_load_w: exo.load,
            p_load_critical_w: exo.load_critical,
            p_pv_dc_w: exo.pv_dc,
            p_pv_ac_w: p_pv_ac,
            p_batt_ac_w: p_batt_ac,
            p_grid_w: exo.load - p_pv_ac - p_batt_ac + p_standby,
            p_standby_w: p_standby,
            units,
            soc_mean,
        });
    }
}

/// Step one unit with a setpoint and return realized terminal power.
fn step_one(unit: &mut crate::battery::BatteryUnit, p_set: f64, dt_s: u32, t_amb_c: f64) -> f64 {
    unit.step(&BatteryStepInput {
        dt_s,
        p_term_setpoint_w: p_set,
        t_amb_c,
        grid_present: true,
    })
    .p_term_w
}
