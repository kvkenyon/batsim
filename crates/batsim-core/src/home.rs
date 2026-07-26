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

use crate::battery::{BatteryStepInput, BatteryStepOutput};
use crate::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use crate::inverter::resolve_shared_ac_cap;
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
    /// stage 4). Commands may be submitted in any tick order; the queue is
    /// kept sorted by `execute_at_tick`, and submission order is preserved
    /// for equal ticks.
    pub fn schedule(&mut self, cmd: ScheduledDispatch) {
        let at = self
            .dispatch_queue
            .partition_point(|c| c.execute_at_tick <= cmd.execute_at_tick);
        self.dispatch_queue.insert(at, cmd);
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
                let conv = inv.dc_to_ac_capped(p_pv_dc, self.pv_ac_cap_w());
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
                        inv.dc_to_ac_capped(exo.pv_dc, self.pv_ac_cap_w()).p_out_w
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
    ) -> Vec<BatteryStepOutput> {
        let n = self.devices.batteries.len();
        let mut realized = vec![BatteryStepOutput::default(); n];
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
            let p_dc_set = self.hybrid_dc_setpoint(p_batt_ac_set, exo);
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

    /// The DC-bus setpoint for the hybrid units, translated from the
    /// AC-boundary setpoint through the shared inverter (A.3.3). Every
    /// AC<->DC translation routes through the `inverter` helpers so the D1
    /// charge-path rule has a single owner.
    ///
    /// Zero when no hybrid inverter exists: `build_devices` guarantees one
    /// for any DC-coupled battery, so this is an unreachable safety floor
    /// rather than a silent energy drop.
    fn hybrid_dc_setpoint(&self, p_batt_ac_set: f64, exo: &Exogenous) -> f64 {
        let Some(inv) = self.devices.hybrid_inverter.as_ref() else {
            return 0.0;
        };
        let pv_surplus_dc = if matches!(self.mode, ControlMode::SelfConsumption)
            && self.devices.pv_inverter.is_none()
        {
            // DC left on the bus after covering the load (single-
            // inversion PV->battery path, A.3.3).
            (exo.pv_dc - inv.dc_required_for_ac(exo.load)).max(0.0)
        } else {
            0.0
        };
        if pv_surplus_dc > 0.0 && p_batt_ac_set <= 0.0 {
            -pv_surplus_dc
        } else if p_batt_ac_set >= 0.0 {
            // Discharge: DC required from the battery for an AC target.
            inv.dc_required_for_ac(p_batt_ac_set)
        } else {
            // Charge: DC delivered to the bus from an AC draw
            // (conservation-true: DC = AC x eta, D1 decision).
            -inv.ac_to_dc(-p_batt_ac_set).p_out_w
        }
    }

    /// AC-side cap on the PV path from the array's DC/AC ratio (B.7.4).
    fn pv_ac_cap_w(&self) -> f64 {
        self.devices.pv_ac_cap_w.unwrap_or(f64::INFINITY)
    }

    /// Split a setpoint across the given unit indices pro-rata by dynamic
    /// headroom and step each (B.3.4).
    fn split_and_step(
        batteries: &mut [crate::battery::BatteryUnit],
        indices: &[usize],
        p_set: f64,
        dt_s: u32,
        t_amb_c: f64,
        realized: &mut [BatteryStepOutput],
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
    fn stage_inverter(
        &mut self,
        exo: &Exogenous,
        unit_realized: &[BatteryStepOutput],
        dt_s: u32,
    ) -> (f64, f64) {
        let mut p_batt_ac: f64 = unit_realized
            .iter()
            .zip(&self.devices.batteries)
            .filter(|(_, u)| is_ac_terminal(u.model().coupling))
            .map(|(o, _)| o.p_term_w)
            .sum();
        let mut p_pv_ac = exo.pv_ac;

        let Some(inv) = &self.devices.hybrid_inverter else {
            return (p_pv_ac, p_batt_ac);
        };
        let hyb_batt_dc: f64 = unit_realized
            .iter()
            .zip(&self.devices.batteries)
            .filter(|(_, u)| !is_ac_terminal(u.model().coupling))
            .map(|(o, _)| o.p_term_w)
            .sum();
        let p_bus = exo.pv_dc + hyb_batt_dc;
        if p_bus > 0.0 {
            // One conversion of the summed DC (the physics), then attribute
            // the shared AC rating between the two sources that actually
            // put power on the bus. PV occupies the bus first; a charging
            // battery contributes no AC candidate (its share is negative
            // and already netted into `p_bus`).
            let eta = inv.eta_at_w(p_bus);
            let pv_dc_share = exo.pv_dc.max(0.0).min(p_bus);
            let batt_dc_share = p_bus - pv_dc_share;
            // The array's DC/AC ratio caps the PV path before the shared
            // rating does (B.7.4); the overhang is PV curtailment.
            let pv_ac_uncapped = pv_dc_share * eta;
            let pv_ac_candidate = pv_ac_uncapped.min(self.pv_ac_cap_w());
            let batt_ac_candidate = batt_dc_share * eta;
            let (pv_admitted, batt_admitted) = resolve_shared_ac_cap(
                inv.rated_ac_w(),
                pv_ac_candidate,
                batt_ac_candidate,
                self.devices.pv_priority,
            );
            p_pv_ac += pv_admitted;
            p_batt_ac += batt_admitted;
            // Clipping is attributed to whichever source actually
            // overflowed, measured back on the DC side (B.3.3).
            self.meters
                .pv_clipped
                .accumulate((pv_ac_uncapped - pv_admitted) / eta, dt_s);
            self.meters
                .batt_clipped
                .accumulate((batt_ac_candidate - batt_admitted) / eta, dt_s);
        } else if p_bus < 0.0 {
            // Net bus deficit: drawn from AC through the hybrid (grid
            // charge double conversion, A.3.3). The battery already
            // absorbed its DC, so the AC draw is metered in full.
            p_batt_ac -= inv.ac_required_for_dc(-p_bus);
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
        unit_realized: &[BatteryStepOutput],
    ) {
        if !self.record_truth {
            return;
        }
        let units: Vec<UnitTruth> = self
            .devices
            .batteries
            .iter()
            .zip(unit_realized)
            .map(|(u, o)| UnitTruth {
                soc: u.soc(),
                p_term_w: o.p_term_w,
                v_term_v: o.v_term_v,
                heat_w: o.heat_w,
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

/// Step one unit with a setpoint and return its full realized output.
fn step_one(
    unit: &mut crate::battery::BatteryUnit,
    p_set: f64,
    dt_s: u32,
    t_amb_c: f64,
) -> BatteryStepOutput {
    unit.step(&BatteryStepInput {
        dt_s,
        p_term_setpoint_w: p_set,
        t_amb_c,
        grid_present: true,
    })
}
