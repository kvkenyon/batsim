//! One simulated home: the per-tick pipeline over its devices, with
//! coupling-aware energy-path routing.
//!
//! Stage order per tick is mandatory: load -> pv -> price_signal
//! (planned future work) -> dispatch -> battery -> inverter -> metering
//! -> telemetry.
//!
//! Current scope: grid is always present (outage simulation is planned
//! future work); the dispatch stage reads stages 1-2 of the same tick;
//! telemetry is the lossless truth stream only.
//!
//! Sign conventions: battery power positive = discharging; grid
//! power positive = importing (the metering-stage formula).

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
    #[serde(default)]
    pv_curtail_frac: f64,
    #[serde(default)]
    retired: bool,
    dispatch_queue: Vec<ScheduledDispatch>,
    meters: HomeMeters,
    record_truth: bool,
    truth: Vec<HomeTruth>,
}

/// Stage-5 result: the per-unit realized outputs plus the battery AC
/// discharge that was curtailed *before* integration because the shared
/// hybrid inverter had no AC headroom left for it (the shared-inverter
/// PV-priority rule).
#[derive(Debug, Clone, Default)]
struct BatteryStage {
    units: Vec<BatteryStepOutput>,
    curtailed_ac_w: f64,
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
            pv_curtail_frac: 0.0,
            retired: false,
            dispatch_queue: Vec::new(),
            meters: HomeMeters::default(),
            record_truth,
            truth: Vec::new(),
        }
    }

    /// Queue a dispatch action (applied at the top of its tick, in the
    /// dispatch stage). Commands may be submitted in any tick order; the
    /// queue is kept sorted by `execute_at_tick`, and submission order is
    /// preserved for equal ticks.
    pub fn schedule(&mut self, cmd: ScheduledDispatch) {
        let at = self
            .dispatch_queue
            .partition_point(|c| c.execute_at_tick <= cmd.execute_at_tick);
        self.dispatch_queue.insert(at, cmd);
    }

    /// Retract every still-queued command carrying `tag`; returns the
    /// number removed. Already-applied commands are untouched.
    pub fn cancel_tagged(&mut self, tag: u64) -> usize {
        let before = self.dispatch_queue.len();
        self.dispatch_queue.retain(|c| c.tag != tag);
        before - self.dispatch_queue.len()
    }

    /// Number of commands still queued.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.dispatch_queue.len()
    }

    /// The active control mode.
    #[must_use]
    pub const fn mode(&self) -> ControlMode {
        self.mode
    }

    /// The manual-mode setpoint (W; + discharge).
    #[must_use]
    pub const fn manual_setpoint_w(&self) -> f64 {
        self.manual_setpoint_w
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

    /// Set the PV curtailment fraction (0 = full output, 1 = fully
    /// curtailed). Curtailment is lossless: the array simply produces
    /// less, as an MPPT moving off its maximum-power point.
    pub fn set_pv_curtail_frac(&mut self, frac: f64) {
        self.pv_curtail_frac = frac.clamp(0.0, 1.0);
    }

    /// The active PV curtailment fraction.
    #[must_use]
    pub fn pv_curtail_frac(&self) -> f64 {
        self.pv_curtail_frac
    }

    /// Retire or un-retire the home. A retired home keeps its arena slot
    /// (indices key every RNG substream, so removal would re-key the
    /// whole fleet) but skips all physics and telemetry.
    pub fn set_retired(&mut self, retired: bool) {
        self.retired = retired;
    }

    /// Whether the home is retired.
    #[must_use]
    pub fn is_retired(&self) -> bool {
        self.retired
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

    /// Execute one tick of the per-tick pipeline. Retired homes hold
    /// their state untouched so fleet edits never perturb neighbors.
    pub fn step(&mut self, tick: u64, unix_time_s: u64, dt_s: u32, t_amb_c: f64) {
        if self.retired {
            return;
        }
        self.apply_due_dispatches(tick);
        let exo = self.stages_load_pv(tick, unix_time_s, dt_s, t_amb_c);
        let p_batt_ac_set = self.stage_dispatch(&exo);
        let batt = self.stage_battery(p_batt_ac_set, &exo, dt_s, t_amb_c);
        let (p_pv_ac, p_batt_ac) = self.stage_inverter(&exo, &batt, dt_s);
        self.stage_metering(&exo, p_pv_ac, p_batt_ac, dt_s);
        self.stage_telemetry(tick, unix_time_s, &exo, p_pv_ac, p_batt_ac, &batt.units);
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
                DispatchAction::SetPvCurtail(frac) => self.set_pv_curtail_frac(frac),
            }
        }
    }

    /// Stages 1-2: load and PV.
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
            .map_or(0.0, |pv| pv.dc_power_w(unix_time_s, tick, dt_s, t_amb_c))
            * (1.0 - self.pv_curtail_frac);
        // Dedicated string inverter: PV converts here (the string-
        // inverter loss point) and never touches the battery path.
        // Hybrid: PV DC lands on the bus and converts in stage 6.
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
        let out = match self.mode {
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
        };
        out
    }

    /// Stage 5: split the setpoint across ALL units in one AC-boundary
    /// pro-rata pass and step each unit. Every unit's weight is
    /// its dynamic headroom expressed at the AC boundary, so mixed
    /// couplings never realize more than the setpoint. Hybrid shares are
    /// then translated to DC-bus setpoints: PV-surplus charging routes
    /// DC->DC (single inversion); AC-side shares translate through
    /// the hybrid curve (grid charge remains a double conversion).
    fn stage_battery(
        &mut self,
        p_batt_ac_set: f64,
        exo: &Exogenous,
        dt_s: u32,
        t_amb_c: f64,
    ) -> BatteryStage {
        let n = self.devices.batteries.len();
        let mut realized = vec![BatteryStepOutput::default(); n];
        if n == 0 {
            return BatteryStage {
                units: realized,
                curtailed_ac_w: 0.0,
            };
        }
        let discharge = p_batt_ac_set > 0.0;
        let mut ac_idx: Vec<usize> = Vec::new();
        let mut dc_idx: Vec<usize> = Vec::new();
        let mut weights: Vec<f64> = Vec::with_capacity(n);
        for (i, unit) in self.devices.batteries.iter().enumerate() {
            let base = if discharge {
                unit.max_discharge_w()
            } else {
                unit.max_charge_w()
            };
            if is_ac_terminal(unit.model().coupling) {
                ac_idx.push(i);
                weights.push(base);
            } else {
                dc_idx.push(i);
                // DC-terminal headroom at the AC boundary: discharge is
                // the AC the shared inverter would deliver from it;
                // charge is the AC draw needed to push it through.
                let w = self.devices.hybrid_inverter.as_ref().map_or(base, |inv| {
                    if discharge {
                        base * inv.eta_at_w(base)
                    } else {
                        inv.ac_required_for_dc(base)
                    }
                });
                weights.push(w);
            }
        }
        let total: f64 = weights.iter().sum();
        let mut curtailed_ac_w = 0.0;
        if total <= 0.0 {
            for (i, slot) in realized.iter_mut().enumerate() {
                *slot = step_one(&mut self.devices.batteries[i], 0.0, dt_s, t_amb_c);
            }
            return BatteryStage {
                units: realized,
                curtailed_ac_w,
            };
        }
        // AC-terminal units take their AC-boundary shares directly.
        for &i in &ac_idx {
            let share = p_batt_ac_set * weights[i] / total;
            realized[i] = step_one(&mut self.devices.batteries[i], share, dt_s, t_amb_c);
        }
        // Hybrid (DC-bus) units: translate only their combined share,
        // then split it pro-rata by DC headroom.
        if !dc_idx.is_empty() {
            let dc_weight: f64 = dc_idx.iter().map(|&i| weights[i]).sum();
            let hybrid_share = p_batt_ac_set * dc_weight / total;
            let (p_dc_set, curtailed) = self.hybrid_dc_setpoint(hybrid_share, exo);
            curtailed_ac_w = curtailed;
            Self::split_and_step(
                &mut self.devices.batteries,
                &dc_idx,
                p_dc_set,
                dt_s,
                t_amb_c,
                &mut realized,
            );
        }
        BatteryStage {
            units: realized,
            curtailed_ac_w,
        }
    }

    /// The DC-bus setpoint for the hybrid units, translated from their
    /// AC-boundary share through the shared inverter. Every
    /// AC<->DC translation routes through the `inverter` helpers so the
    /// energy-conserving charge-path rule has a single owner.
    ///
    /// Zero when no hybrid inverter exists: `build_devices` guarantees one
    /// for any DC-coupled battery, so this is an unreachable safety floor
    /// rather than a silent energy drop.
    ///
    /// Returns `(p_dc_setpoint_w, curtailed_ac_w)`. The discharge target is
    /// curtailed here, before the pack integrates it, by the AC headroom PV
    /// already occupies at the shared inverter (PV priority at the shared
    /// inverter): a pack
    /// may never discharge energy the shared inverter cannot pass, so the
    /// curtailment is a command reduction, not a downstream clip.
    fn hybrid_dc_setpoint(&self, p_batt_ac_set: f64, exo: &Exogenous) -> (f64, f64) {
        let Some(inv) = self.devices.hybrid_inverter.as_ref() else {
            return (0.0, 0.0);
        };
        let pv_surplus_dc = if matches!(self.mode, ControlMode::SelfConsumption)
            && self.devices.pv_inverter.is_none()
        {
            // DC left on the bus after covering the load (single-
            // inversion PV->battery path).
            (exo.pv_dc - inv.dc_required_for_ac(exo.load)).max(0.0)
        } else {
            0.0
        };
        if pv_surplus_dc > 0.0 && p_batt_ac_set <= 0.0 {
            // Mixed coupling: AC-terminal units serve their own share of
            // the surplus through the AC path, so the hybrid soaks only
            // the DC behind its share. A pure-hybrid home has no other
            // sink: soak the exact surplus DC (export avoidance).
            let soak = if self.has_ac_terminal_units() {
                pv_surplus_dc.min(inv.dc_required_for_ac(-p_batt_ac_set))
            } else {
                pv_surplus_dc
            };
            (-soak, 0.0)
        } else if p_batt_ac_set >= 0.0 {
            // Discharge: DC required from the battery for the AC target it
            // is actually allowed to reach through the shared inverter.
            let ac_target = p_batt_ac_set.min(self.hybrid_batt_ac_headroom_w(inv, exo));
            (inv.dc_required_for_ac(ac_target), p_batt_ac_set - ac_target)
        } else {
            // Charge: DC delivered to the bus from an AC draw
            // (conservation-true: DC delivered equals AC drawn times
            // efficiency).
            (-inv.ac_to_dc(-p_batt_ac_set).p_out_w, 0.0)
        }
    }

    /// Whether any battery unit is AC-terminal (mixed coupling).
    fn has_ac_terminal_units(&self) -> bool {
        self.devices
            .batteries
            .iter()
            .any(|u| is_ac_terminal(u.model().coupling))
    }

    /// AC output the shared hybrid inverter can still pass to the battery
    /// once PV has been admitted. With `pv_priority` off the battery
    /// is admitted first and owns the whole rating.
    fn hybrid_batt_ac_headroom_w(
        &self,
        inv: &crate::inverter::InverterUnit,
        exo: &Exogenous,
    ) -> f64 {
        if !self.devices.pv_priority || self.devices.pv_inverter.is_some() {
            return inv.rated_ac_w();
        }
        let pv_ac = inv.dc_to_ac_capped(exo.pv_dc, self.pv_ac_cap_w()).p_out_w;
        (inv.rated_ac_w() - pv_ac).max(0.0)
    }

    /// AC-side cap on the PV path from the array's DC/AC ratio.
    fn pv_ac_cap_w(&self) -> f64 {
        self.devices.pv_ac_cap_w.unwrap_or(f64::INFINITY)
    }

    /// Split a setpoint across the given unit indices pro-rata by dynamic
    /// headroom and step each.
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

    /// Stage 6: resolve the hybrid inverter's shared AC cap (PV priority
    /// at the shared inverter) and return (pv_ac_w, batt_ac_w) at the
    /// panel.
    fn stage_inverter(&mut self, exo: &Exogenous, batt: &BatteryStage, dt_s: u32) -> (f64, f64) {
        let unit_realized = &batt.units;
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
        // The array lands on the hybrid bus ONLY when it is MPPT-landed
        // (`pv_inverter_model_id == null` in the device catalog). With a
        // dedicated PV
        // inverter, stage 2 already converted the array - converting it
        // again here would double-count and double-meter the same DC.
        let pv_bus_dc = if self.devices.pv_inverter.is_some() {
            0.0
        } else {
            exo.pv_dc
        };
        let p_bus = pv_bus_dc + hyb_batt_dc;
        // Command curtailment from stage 5 (the shared-inverter
        // PV-priority rule), converted to the DC
        // side at the realized bus operating point (the marginal DC the
        // shared inverter would have drawn for the curtailed AC slice),
        // so this counter is homogeneous with the residual clip below and
        // with `pv_clipped` (both DC-side).
        self.meters
            .batt_clipped
            .accumulate(batt.curtailed_ac_w / inv.eta_at_w(p_bus.max(1.0)), dt_s);
        if p_bus > 0.0 {
            // One conversion of the summed DC (the physics), then attribute
            // the shared AC rating between the two sources that actually put
            // power on the bus. PV priority was already enforced upstream by
            // curtailing the battery command, so here the pack's realized DC
            // is non-negotiable (its energy has left the cells) and any
            // residual overflow curtails PV, which is losslessly curtailable
            // at the MPPT. A charging battery contributes no AC candidate:
            // its share is negative and already netted into `p_bus`.
            let eta = inv.eta_at_w(p_bus);
            let batt_dc_share = hyb_batt_dc.max(0.0).min(p_bus);
            let pv_dc_share = p_bus - batt_dc_share;
            // The array's DC/AC ratio caps the PV path before the shared
            // rating does; the overhang is PV curtailment.
            let pv_ac_uncapped = pv_dc_share * eta;
            let pv_ac_candidate = pv_ac_uncapped.min(self.pv_ac_cap_w());
            let batt_ac_candidate = batt_dc_share * eta;
            let (pv_admitted, batt_admitted) =
                resolve_shared_ac_cap(inv.rated_ac_w(), pv_ac_candidate, batt_ac_candidate, false);
            p_pv_ac += pv_admitted;
            p_batt_ac += batt_admitted;
            self.meters
                .pv_clipped
                .accumulate((pv_ac_uncapped - pv_admitted) / eta, dt_s);
            self.meters
                .batt_clipped
                .accumulate((batt_ac_candidate - batt_admitted) / eta, dt_s);
        } else if p_bus < 0.0 {
            // Net bus deficit: drawn from AC through the hybrid (grid
            // charge double conversion). The battery already
            // absorbed its DC, so the AC draw is metered in full.
            p_batt_ac -= inv.ac_required_for_dc(-p_bus);
        }
        (p_pv_ac, p_batt_ac)
    }

    /// Stage 7: close the balance at the meter points and integrate energy
    /// counters (`P_grid = P_load - P_pv_ac - P_inv_ac`, + import;
    /// standby draw added on the AC side).
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

    /// Total standby draw (W): battery units + controllers.
    fn standby_total_w(&self) -> f64 {
        self.devices
            .batteries
            .iter()
            .map(crate::battery::BatteryUnit::standby_power_w)
            .sum::<f64>()
            + self.devices.controller_standby_w
    }

    /// Stage 8: record the lossless truth record for this tick.
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
