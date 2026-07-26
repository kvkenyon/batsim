//! Battery unit physics (spec B.2; F3 split-efficiency SOC model, F4
//! Thevenin sag, F5 chemistry modules).
//!
//! # Model boundary conventions
//!
//! A [`BatteryUnit`] is ONE physical battery unit (spec B.2.1). A home with
//! N units holds N instances; setpoint splitting across units is the home
//! controller's job (spec B.3.4: pro-rata by remaining headroom).
//!
//! **Terminal power convention** (`p_term_*`): positive = discharging out of
//! the device boundary (toward home/grid); negative = charging. The boundary
//! depends on coupling (spec A.1.4, A.3):
//! - `ACCoupled` / `MicroinverterBased`: boundary is the unit's AC meter
//!   point; the registry efficiency curves cover AC<->pack conversion.
//! - `DCCoupledHybrid`: boundary is the hybrid inverter's DC bus; the
//!   registry battery curves are the bidirectional DC-DC converter
//!   (`eta_dcdc`); the separate `InverterModel` owns DC->AC inversion.
//!
//! # Energy path (spec B.2.2)
//!
//! Charge: `P_pack = |P_term| * eta_chg(|P_term|)`; stored energy integrates
//! `P_pack * eta_coul(chemistry)` per tick. Discharge: pack gives up
//! `dE = P_pack * dt`, terminal delivers `P_term = P_pack * eta_dis`.
//! Electrical losses become heat (B.4.1: electrical losses = heat, energy
//! conservation by construction); the Thevenin `R_int` is used ONLY for
//! power-limit sag, never as a second energy loss.
//!
//! # M1 scope notes (spec 0.2)
//!
//! F7 (lumped thermal model) and F8 (degradation) are M4. In M1:
//! - cell temperature == the ambient feed passed to [`BatteryUnit::step`]
//!   (drives Thevenin cold rise, thermal derate B.4.4, cold-charge rules);
//! - `Q_avail` is fixed at usable energy (+ expansion packs), `R_growth` = 0.

use batsim_registry::{BatteryModel, Chemistry, Coupling};
use serde::{Deserialize, Serialize};

use crate::chemistry;
use crate::error::CoreError;

/// Static behavioral config for a battery unit (spec B.2.4, B.2.6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BatteryConfig {
    /// Compute Thevenin voltage / power sag (B.2.4; default true). When
    /// false, static registry limits apply without SOC/temperature sag.
    pub thevenin_enabled: bool,
    /// Release the user reserve down to the hard SOC floor during outages
    /// (B.2.6; default true, matching Tesla behavior).
    pub release_reserve_in_outage: bool,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            thevenin_enabled: true,
            release_reserve_in_outage: true,
        }
    }
}

/// Per-tick inputs to one battery unit.
#[derive(Debug, Clone, Copy)]
pub struct BatteryStepInput {
    /// Timestep in seconds (engine `dt`).
    pub dt_s: u32,
    /// Requested terminal power (W; + discharge, - charge), after the
    /// home's dispatch stage and ramp application upstream is NOT assumed —
    /// the unit applies its own ramp/min-on-off internally.
    pub p_term_setpoint_w: f64,
    /// Ambient temperature seen by the pack (M1: stands in for T_cell).
    pub t_amb_c: f64,
    /// Grid state: reserve floor applies when true (B.2.6).
    pub grid_present: bool,
}

/// Per-tick realized output of one battery unit.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatteryStepOutput {
    /// Realized terminal power (W; + discharge, - charge).
    pub p_term_w: f64,
    /// Conversion losses turned to heat this tick (W) — for the (M4)
    /// thermal model and energy-conservation checks.
    pub heat_w: f64,
    /// Terminal voltage from the Thevenin model (0.0 when disabled).
    pub v_term_v: f64,
    /// Pack current (A; + discharge).
    pub current_a: f64,
    /// Limit/trip flags raised this tick.
    pub flags: BatteryFlags,
}

/// Telemetry-visible limit flags (spec B.9.1 vocabulary).
///
/// A flag struct (not a bitfield) for serde transparency; the bool count
/// mirrors the B.9.1 vocabulary exactly.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BatteryFlags {
    /// Thevenin/clipping clamped the request (`PowerLimited`).
    pub power_limited: bool,
    /// Thermal derate (B.4.4) reduced the limit.
    pub thermal_derated: bool,
    /// LFP cold-charge inhibition active (B.2.5).
    pub charge_inhibited_cold: bool,
    /// SOC hit the top of the window.
    pub at_soc_max: bool,
    /// SOC hit the bottom of the window (or reserve floor).
    pub at_soc_min: bool,
    /// Min on/off timer suppressed a direction change (B.2.7).
    pub min_on_off_suppressed: bool,
}

/// Device state enum (spec B.9.1 shared vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceState {
    /// Energized, no throughput.
    Standby,
    /// Charging.
    Charging,
    /// Discharging.
    Discharging,
    /// Zero setpoint, contactors closed.
    Idle,
    /// Mid backup transfer (M4).
    Transferring,
    /// Operating islanded (M4).
    Islanded,
    /// Protective trip.
    Tripped,
    /// Fault latch.
    Faulted,
    /// De-energized.
    Off,
}

/// Sub-step ceiling for the internal integrator (spec B.1.6: models
/// sub-step at <= 5 s when the engine dt is larger).
const MAX_SUB_STEP_S: u32 = 5;

/// Fixed-point bisection depth for the SOC-window energy feasibility
/// solve: 60 halvings bring the interval to ~2^-60 of the request, far
/// below f64 ULP of any realistic power, so the limit is exact.
const WINDOW_BISECT_ITERS: usize = 60;

/// Default self-discharge fold-in when the catalog omits the field
/// (Part A §5: 0.2 %/day, includes idle/standby draw).
const DEFAULT_SELF_DISCHARGE_PER_DAY: f64 = 0.002;

/// One physical battery unit with full per-tick physics state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryUnit {
    /// Registry model data (ratings, curves, window).
    model: BatteryModel,
    /// Behavioral config (Thevenin toggle, outage reserve release).
    config: BatteryConfig,
    /// Available capacity `Q_avail` (Wh): usable energy + expansion packs.
    q_avail_wh: f64,
    /// Energy spanned by the SOC window: `(max - min) * q_avail` (Wh).
    e_window_wh: f64,
    /// Stored energy above the SOC-window floor (Wh). Integrating this is
    /// the SOC ODE (B.2.2).
    e_stored_wh: f64,
    /// SOC window bounds (fractions of `q_avail`).
    soc_min: f64,
    /// See `soc_min`.
    soc_max: f64,
    /// User backup-reserve floor (fraction of `q_avail`).
    reserve_frac: f64,
    /// Cumulative charge throughput at the terminal boundary (Wh).
    cum_charge_wh: f64,
    /// Cumulative discharge throughput at the terminal boundary (Wh).
    cum_discharge_wh: f64,
    /// Continuous discharge rating at the terminal boundary (W).
    continuous_discharge_w: f64,
    /// Continuous charge rating at the terminal boundary (W).
    continuous_charge_w: f64,
    /// Peak (short-duration) discharge rating (W); equals the continuous
    /// rating when the model declares none.
    peak_discharge_w: f64,
    /// Peak-power budget remaining (W*s; B.2.6 accumulator).
    peak_budget_ws: f64,
    /// Peak budget capacity: `(peak - continuous) * peak_duration_s` (W*s).
    peak_budget_cap_ws: f64,
    /// Ramp slew limit (W/s) from the registry ramp rate.
    ramp_w_per_s: f64,
    /// Ramp integrator: last tick's realized terminal power (W).
    p_ramp_w: f64,
    /// Min-on time (s; B.2.7: 60 for hybrid inverters, else 0).
    min_on_s: f64,
    /// Min-off time (s).
    min_off_s: f64,
    /// Ticks spent in the current activity state (integer, B.2.7).
    ticks_in_state: u64,
    /// Ticks since the last nonzero throughput (integer, B.2.7).
    ticks_since_active: u64,
    /// Base internal resistance of the 400 V pack (ohm; chemistry module).
    r_base_ohm: f64,
    /// Pack terminal-voltage cutoff (V; chemistry module calibration).
    v_min_v: f64,
    /// Coulombic efficiency (cached from the chemistry module).
    eta_coul: f64,
    /// Self-discharge fold-in (fraction/day, Part A §5).
    self_discharge_frac_per_day: f64,
    /// Activity state (B.9.1 vocabulary).
    state: DeviceState,
}

/// Dynamic power limits at one cell temperature and tick length (W).
struct DynamicLimits {
    /// Discharge limit before the SOC-window energy clamp (W).
    discharge_w: f64,
    /// Charge limit before the SOC-window energy clamp (W).
    charge_w: f64,
    /// Thermal derate factor applied (B.4.4).
    derate: f64,
    /// Cold charge-acceptance factor applied (B.2.5).
    cold: f64,
    /// Thermally-derated continuous discharge rating (W): the boundary
    /// between "continuous" and "peak budget" throughput (B.2.6).
    continuous_derated_w: f64,
}

impl BatteryUnit {
    /// Construct one unit from a registry model.
    ///
    /// `expansion_pack` is `Some((pack_model, count))` for PW3-style
    /// energy-only packs (spec A.3.1); the pack's usable energy adds to
    /// `Q_avail`, power limits stay the head unit's.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] when the SOC window, initial SOC,
    /// reserve, or pack combination is inconsistent with the model data.
    pub fn new(
        model: &BatteryModel,
        expansion_pack: Option<(&BatteryModel, u32)>,
        initial_soc_frac: f64,
        reserve_frac: f64,
        config: BatteryConfig,
    ) -> Result<Self, CoreError> {
        let soc_min = model.soc_window.min_soc_frac;
        let soc_max = model.soc_window.max_soc_frac;
        if !(0.0..1.0).contains(&soc_min) || !(0.0..=1.0).contains(&soc_max) || soc_min >= soc_max {
            return Err(CoreError::InvalidConfig(format!(
                "{}: invalid SOC window [{soc_min}, {soc_max}]",
                model.model_id
            )));
        }
        if !(soc_min..=soc_max).contains(&initial_soc_frac) {
            return Err(CoreError::InvalidConfig(format!(
                "{}: initial SOC {initial_soc_frac} outside window [{soc_min}, {soc_max}]",
                model.model_id
            )));
        }
        if !(soc_min..=soc_max).contains(&reserve_frac) {
            return Err(CoreError::InvalidConfig(format!(
                "{}: reserve {reserve_frac} outside window [{soc_min}, {soc_max}]",
                model.model_id
            )));
        }
        let mut q_avail_wh = model.usable_energy_kwh.value * 1000.0;
        if q_avail_wh <= 0.0 {
            return Err(CoreError::InvalidConfig(format!(
                "{}: usable energy must be positive",
                model.model_id
            )));
        }
        if let Some((pack, count)) = expansion_pack {
            let expansion = model.expansion.as_ref().ok_or_else(|| {
                CoreError::InvalidConfig(format!("{}: accepts no expansion packs", model.model_id))
            })?;
            if expansion.expansion_pack_model_id.as_deref() != Some(pack.model_id.as_str()) {
                return Err(CoreError::InvalidConfig(format!(
                    "{}: expansion pack `{}` does not match declared `{}`",
                    model.model_id,
                    pack.model_id,
                    expansion
                        .expansion_pack_model_id
                        .as_deref()
                        .unwrap_or("<none>")
                )));
            }
            if expansion.packs_add_power == Some(true) {
                return Err(CoreError::InvalidConfig(format!(
                    "{}: power-adding expansion packs are not supported (B.2.1)",
                    model.model_id
                )));
            }
            if let Some(max_units) = expansion.max_units_per_inverter {
                if count + 1 > max_units {
                    return Err(CoreError::InvalidConfig(format!(
                        "{}: {count} packs + head exceeds max_units_per_inverter {max_units}",
                        model.model_id
                    )));
                }
            }
            q_avail_wh += pack.usable_energy_kwh.value * 1000.0 * f64::from(count);
        }
        let continuous_discharge_w = model.continuous_discharge_power_kw.value * 1000.0;
        let peak_discharge_w = model
            .peak_discharge_power_kw
            .as_ref()
            .map_or(continuous_discharge_w, |p| p.value * 1000.0);
        let peak_duration_s = model.peak_duration_s.as_ref().map_or(0.0, |d| d.value);
        let peak_budget_cap_ws =
            ((peak_discharge_w - continuous_discharge_w) * peak_duration_s).max(0.0);
        let ramp_kw_per_s = model.ramp_rate.max_kw_per_s;
        // A nonpositive registry ramp rate means "no slew limit declared".
        let ramp_w_per_s = if ramp_kw_per_s > 0.0 {
            ramp_kw_per_s * 1000.0
        } else {
            f64::INFINITY
        };
        // B.2.7 defaults: 60/60 s for hybrid inverters; 0 for
        // battery-integrated AC systems that tolerate rapid cycling.
        let (min_on_s, min_off_s) = match model.coupling {
            Coupling::DCCoupledHybrid => (60.0, 60.0),
            Coupling::ACCoupled | Coupling::MicroinverterBased => (0.0, 0.0),
        };
        Ok(Self {
            model: model.clone(),
            config,
            q_avail_wh,
            e_window_wh: (soc_max - soc_min) * q_avail_wh,
            e_stored_wh: (initial_soc_frac - soc_min) * q_avail_wh,
            soc_min,
            soc_max,
            reserve_frac,
            cum_charge_wh: 0.0,
            cum_discharge_wh: 0.0,
            continuous_discharge_w,
            continuous_charge_w: model.continuous_charge_power_kw.value * 1000.0,
            peak_discharge_w,
            peak_budget_ws: peak_budget_cap_ws,
            peak_budget_cap_ws,
            ramp_w_per_s,
            p_ramp_w: 0.0,
            min_on_s,
            min_off_s,
            ticks_in_state: 0,
            ticks_since_active: u64::MAX,
            r_base_ohm: chemistry::base_internal_resistance(continuous_discharge_w),
            v_min_v: chemistry::V_MIN_CUTOFF_FRAC * chemistry::NOMINAL_PACK_V,
            eta_coul: chemistry::eta_coul(model.chemistry),
            self_discharge_frac_per_day: model
                .self_discharge_frac_per_day
                .as_ref()
                .map_or(DEFAULT_SELF_DISCHARGE_PER_DAY, |a| a.value),
            state: DeviceState::Standby,
        })
    }

    /// Advance the unit one tick. Applies, in order (spec B.2/B.1.5 stage
    /// 5): ramp slew, min on/off enforcement, dynamic limits (chemistry
    /// cold rules, thermal derate, Thevenin sag, charge taper, peak
    /// budget), clamped integration of the SOC ODE with separated
    /// charge/discharge efficiencies, throughput bookkeeping.
    pub fn step(&mut self, input: &BatteryStepInput) -> BatteryStepOutput {
        let dt_s = f64::from(input.dt_s.max(1));
        // B.1.6: internal sub-stepping at <= 5 s, ceil(dt/5) sub-steps.
        // Limits are computed at tick level; the sub-stepped integration
        // keeps per-sub-step SOC-window boundary checks exact.
        let n_sub = input.dt_s.max(1).div_ceil(MAX_SUB_STEP_S);
        let dt_sub_s = dt_s / f64::from(n_sub);
        let mut flags = BatteryFlags::default();

        // 1. Ramp slew (B.2.7): the command slews toward the setpoint.
        let slew_w = self.ramp_w_per_s * dt_s;
        let mut p_w =
            self.p_ramp_w + (input.p_term_setpoint_w - self.p_ramp_w).clamp(-slew_w, slew_w);

        // 2. Min on/off suppression (B.2.7): direction changes and stops
        // are held until the integer tick timers expire.
        let min_on_ticks = (self.min_on_s / f64::from(input.dt_s.max(1))).ceil() as u64;
        let min_off_ticks = (self.min_off_s / f64::from(input.dt_s.max(1))).ceil() as u64;
        let intent: i8 = if input.p_term_setpoint_w > 0.0 {
            1
        } else if input.p_term_setpoint_w < 0.0 {
            -1
        } else {
            0
        };
        let suppressed = match self.state {
            DeviceState::Discharging => self.ticks_in_state < min_on_ticks && intent != 1,
            DeviceState::Charging => self.ticks_in_state < min_on_ticks && intent != -1,
            _ => self.ticks_since_active < min_off_ticks && intent != 0,
        };
        if suppressed {
            flags.min_on_off_suppressed = true;
            p_w = match self.state {
                DeviceState::Discharging | DeviceState::Charging => self.p_ramp_w,
                _ => 0.0,
            };
        }

        // 3. Dynamic power limits (B.2.4 sag, B.2.6 budget, B.4.4 derate,
        // B.2.5 cold rules, chemistry discharge cutoff).
        let limits = self.dynamic_limits(input.t_amb_c, dt_s);
        let pre_clamp_w = p_w;
        if p_w > limits.discharge_w {
            p_w = limits.discharge_w;
        } else if p_w < 0.0 - limits.charge_w {
            p_w = 0.0 - limits.charge_w;
        }
        if p_w.abs() < pre_clamp_w.abs() {
            flags.power_limited = true;
        }
        if limits.derate < 1.0 && pre_clamp_w.abs() > 0.0 {
            flags.thermal_derated = true;
        }
        if limits.cold < 1.0 && pre_clamp_w < 0.0 {
            flags.charge_inhibited_cold = true;
        }

        // 4. SOC window + reserve clamp (B.2.2/B.2.6): solve the largest
        // power whose energy this tick fits the remaining headroom.
        let pre_window_w = p_w;
        if p_w > 0.0 {
            p_w = self.energy_window_limit(p_w, true, dt_s, input.grid_present);
            if p_w < pre_window_w {
                flags.at_soc_min = true;
            }
        } else if p_w < 0.0 {
            p_w = 0.0 - self.energy_window_limit(-p_w, false, dt_s, input.grid_present);
            if p_w > pre_window_w {
                flags.at_soc_max = true;
            }
        }

        // 5. Integrate the SOC ODE (module-doc energy path), sub-stepped.
        let mut heat_w = 0.0;
        if p_w > 0.0 {
            let eta = self
                .model
                .discharge_efficiency_curve
                .eval(p_w / 1000.0)
                .max(1e-9);
            for _ in 0..n_sub {
                let drain_wh = p_w * dt_sub_s / eta / 3600.0;
                self.e_stored_wh = (self.e_stored_wh - drain_wh).max(0.0);
            }
            heat_w = p_w * (1.0 / eta - 1.0);
            self.cum_discharge_wh += p_w * dt_s / 3600.0;
        } else if p_w < 0.0 {
            let q_w = -p_w;
            let eta = self.model.charge_efficiency_curve.eval(q_w / 1000.0);
            for _ in 0..n_sub {
                let gain_wh = q_w * eta * self.eta_coul * dt_sub_s / 3600.0;
                self.e_stored_wh = (self.e_stored_wh + gain_wh).min(self.e_window_wh);
            }
            heat_w = q_w * (1.0 - eta * self.eta_coul);
            self.cum_charge_wh += q_w * dt_s / 3600.0;
        }

        // 6. Peak-budget accumulator (B.2.6 exact update rule): throughput
        // above the (derated) continuous rating discharges the budget;
        // anything at/below recharges it at the continuous rate.
        if p_w > limits.continuous_derated_w {
            self.peak_budget_ws =
                (self.peak_budget_ws - (p_w - limits.continuous_derated_w) * dt_s).max(0.0);
        } else {
            self.peak_budget_ws = (self.peak_budget_ws + limits.continuous_derated_w * dt_s)
                .min(self.peak_budget_cap_ws);
        }

        // 7. Bookkeeping: ramp integrator, activity state, integer timers.
        self.p_ramp_w = p_w;
        let new_state = if p_w > 0.0 {
            DeviceState::Discharging
        } else if p_w < 0.0 {
            DeviceState::Charging
        } else {
            DeviceState::Standby
        };
        if new_state == self.state {
            self.ticks_in_state = self.ticks_in_state.saturating_add(1);
        } else {
            self.state = new_state;
            self.ticks_in_state = 1;
        }
        if p_w.abs() > 0.0 {
            self.ticks_since_active = 0;
        } else {
            self.ticks_since_active = self.ticks_since_active.saturating_add(1);
        }

        // 8. Thevenin telemetry (sag model only; never an energy loss).
        let (v_term_v, current_a) = self.thevenin_report(p_w, input.t_amb_c);

        BatteryStepOutput {
            p_term_w: p_w,
            heat_w,
            v_term_v,
            current_a,
            flags,
        }
    }

    /// Dynamic discharge/charge power limits at `t_cell_c` for a `dt_s`
    /// tick, before the SOC-window energy clamp.
    fn dynamic_limits(&self, t_cell_c: f64, dt_s: f64) -> DynamicLimits {
        let derate = chemistry::thermal_derate(t_cell_c);
        let continuous_derated_w = self.continuous_discharge_w * derate;
        // B.2.6: continuous + remaining budget, capped at the peak rating.
        let mut discharge_w =
            (continuous_derated_w + self.peak_budget_ws / dt_s).min(self.peak_discharge_w * derate);
        // B.2.5: hard chemistry discharge cutoff (NMC -20 degC).
        if let Some(cut_c) = chemistry::discharge_cutoff_c(self.model.chemistry) {
            if t_cell_c < cut_c {
                discharge_w = 0.0;
            }
        }
        // B.2.4: Thevenin sag under the terminal-voltage cutoff.
        if self.config.thevenin_enabled {
            let soc = self.soc();
            let r = chemistry::r_int(self.r_base_ohm, soc, 0.0, t_cell_c);
            let v = chemistry::v_oc(self.model.chemistry, soc);
            discharge_w = discharge_w.min(chemistry::thevenin_max_discharge_w(v, r, self.v_min_v));
        }
        // B.2.5: cold charge-acceptance factor on the charge rating.
        let cold = chemistry::cold_charge_factor(self.model.chemistry, t_cell_c);
        DynamicLimits {
            discharge_w,
            charge_w: self.continuous_charge_w * derate * cold,
            derate,
            cold,
            continuous_derated_w,
        }
    }

    /// Largest power in `[0, p_w]` whose conversion energy this tick fits
    /// the SOC-window headroom (discharge: stored energy above the
    /// effective floor; charge: remaining window). Solved by bisection on
    /// the exact per-tick energy expression, so the clamp is
    /// energy-exact, never an f64 drift source.
    fn energy_window_limit(&self, p_w: f64, discharge: bool, dt_s: f64, grid_present: bool) -> f64 {
        if p_w <= 0.0 {
            return 0.0;
        }
        let headroom_wh = if discharge {
            let floor = self.effective_floor_soc(grid_present);
            (self.e_stored_wh - (floor - self.soc_min) * self.q_avail_wh).max(0.0)
        } else {
            (self.e_window_wh - self.e_stored_wh).max(0.0)
        };
        let energy_at = |p: f64| -> f64 {
            if discharge {
                let eta = self
                    .model
                    .discharge_efficiency_curve
                    .eval(p / 1000.0)
                    .max(1e-9);
                p * dt_s / eta / 3600.0
            } else {
                let eta = self.model.charge_efficiency_curve.eval(p / 1000.0);
                p * eta * self.eta_coul * dt_s / 3600.0
            }
        };
        if energy_at(p_w) <= headroom_wh {
            return p_w;
        }
        let mut lo = 0.0;
        let mut hi = p_w;
        for _ in 0..WINDOW_BISECT_ITERS {
            let mid = 0.5 * (lo + hi);
            if energy_at(mid) <= headroom_wh {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Effective discharge floor (fraction of `q_avail`): the user reserve
    /// applies while the grid is present (or when reserve release is
    /// disabled); outages with release enabled drop to the hard floor.
    fn effective_floor_soc(&self, grid_present: bool) -> f64 {
        if grid_present || !self.config.release_reserve_in_outage {
            self.soc_min.max(self.reserve_frac)
        } else {
            self.soc_min
        }
    }

    /// Thevenin terminal voltage and pack current for telemetry. Voltage
    /// is 0.0 when the Thevenin model is disabled; current is always the
    /// pack-side DC current (+ discharge), derived from the lossless
    /// approximation when the model is disabled.
    fn thevenin_report(&self, p_w: f64, t_cell_c: f64) -> (f64, f64) {
        let soc = self.soc();
        let v_oc = chemistry::v_oc(self.model.chemistry, soc);
        if p_w > 0.0 {
            let eta = self
                .model
                .discharge_efficiency_curve
                .eval(p_w / 1000.0)
                .max(1e-9);
            let p_pack_w = p_w / eta;
            if self.config.thevenin_enabled {
                let r = chemistry::r_int(self.r_base_ohm, soc, 0.0, t_cell_c);
                let (i, _) = chemistry::thevenin_current_discharge(v_oc, r, p_pack_w);
                ((v_oc - i * r).max(0.0), i)
            } else {
                (0.0, p_pack_w / v_oc.max(1.0))
            }
        } else if p_w < 0.0 {
            let eta = self.model.charge_efficiency_curve.eval(-p_w / 1000.0);
            let p_pack_w = -p_w * eta;
            let i_chg = p_pack_w / v_oc.max(1.0);
            if self.config.thevenin_enabled {
                let r = chemistry::r_int(self.r_base_ohm, soc, 0.0, t_cell_c);
                (v_oc + i_chg * r, -i_chg)
            } else {
                (0.0, -i_chg)
            }
        } else if self.config.thevenin_enabled {
            (v_oc, 0.0)
        } else {
            (0.0, 0.0)
        }
    }

    /// SOC as a fraction of current available (usable) capacity.
    #[must_use]
    pub const fn soc(&self) -> f64 {
        self.soc_min + self.e_stored_wh / self.q_avail_wh
    }

    /// Energy currently stored above the SOC-window floor (Wh).
    #[must_use]
    pub const fn energy_stored_wh(&self) -> f64 {
        self.e_stored_wh
    }

    /// Current available capacity `Q_avail` (Wh). Constant in M1.
    #[must_use]
    pub const fn usable_energy_wh(&self) -> f64 {
        self.q_avail_wh
    }

    /// Dynamic discharge limit at the terminal boundary (W) under current
    /// SOC/temperature: continuous rating x thermal derate, Thevenin sag,
    /// SOC taper, and remaining peak budget.
    ///
    /// Evaluated at the reference condition (25 degC cell, 1 s tick, grid
    /// present); [`BatteryUnit::step`] recomputes the exact limit for its
    /// own input. Dispatchers use this for headroom pro-rata splits.
    #[must_use]
    pub fn max_discharge_w(&self) -> f64 {
        let limits = self.dynamic_limits(25.0, 1.0);
        self.energy_window_limit(limits.discharge_w, true, 1.0, true)
    }

    /// Dynamic charge limit at the terminal boundary (W), including
    /// chemistry cold-charge rules and high-SOC taper.
    ///
    /// Same reference-condition convention as [`Self::max_discharge_w`].
    #[must_use]
    pub fn max_charge_w(&self) -> f64 {
        let limits = self.dynamic_limits(25.0, 1.0);
        self.energy_window_limit(limits.charge_w, false, 1.0, true)
    }

    /// Standby/self-consumption draw (W) while energized, taken from the AC
    /// side in metering (spec B.3.2). Derived from the registry
    /// `self_discharge_frac_per_day` fold-in per Part A §5:
    /// `frac_per_day * usable_energy_wh / 24 h`.
    #[must_use]
    pub const fn standby_power_w(&self) -> f64 {
        self.self_discharge_frac_per_day * self.q_avail_wh / 24.0
    }

    /// Cumulative charge throughput at the terminal boundary (Wh).
    #[must_use]
    pub const fn cumulative_charge_wh(&self) -> f64 {
        self.cum_charge_wh
    }

    /// Cumulative discharge throughput at the terminal boundary (Wh).
    #[must_use]
    pub const fn cumulative_discharge_wh(&self) -> f64 {
        self.cum_discharge_wh
    }

    /// The registry model this unit runs.
    #[must_use]
    pub const fn model(&self) -> &BatteryModel {
        &self.model
    }

    /// Chemistry of the unit (dispatchers use it for cold-charge policy).
    #[must_use]
    pub const fn chemistry(&self) -> Chemistry {
        self.model().chemistry
    }

    /// Current device state (B.9.1 vocabulary).
    #[must_use]
    pub const fn state(&self) -> DeviceState {
        self.state
    }

    /// User backup-reserve floor (fraction of usable).
    #[must_use]
    pub const fn reserve_frac(&self) -> f64 {
        self.reserve_frac
    }

    /// Update the user backup-reserve floor (dispatch `SetReserve`).
    ///
    /// Clamped into the SOC window; non-finite values are ignored.
    pub fn set_reserve_frac(&mut self, frac: f64) {
        if frac.is_finite() {
            self.reserve_frac = frac.clamp(self.soc_min, self.soc_max);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    // float_cmp is allowed in tests: several assertions check exact
    // constants produced by branch-free arithmetic (e.g. 0.5 * x).
    use super::*;
    use batsim_registry::Coupling;

    /// Build a BatteryModel from JSON (registry types only; no catalog).
    fn model_json(
        model_id: &str,
        chemistry: Chemistry,
        coupling: Coupling,
        usable_kwh: f64,
        continuous_kw: f64,
        charge_kw: f64,
        peak_kw: Option<f64>,
        peak_s: Option<f64>,
        ramp_kw_s: f64,
        curve: serde_json::Value,
        extra: serde_json::Value,
    ) -> BatteryModel {
        let mut doc = serde_json::json!({
            "schema_version": "1.0.0",
            "entry_version": "1.0.0",
            "model_id": model_id,
            "vendor": "test",
            "display_name": "Test",
            "chemistry": chemistry,
            "coupling": coupling,
            "nameplate_energy_kwh": {"value": usable_kwh, "provenance": "spec", "unit": "kWh"},
            "usable_energy_kwh": {"value": usable_kwh, "provenance": "spec", "unit": "kWh"},
            "continuous_discharge_power_kw": {"value": continuous_kw, "provenance": "spec", "unit": "kW"},
            "continuous_charge_power_kw": {"value": charge_kw, "provenance": "spec", "unit": "kW"},
            "soc_window": {"min_soc_frac": 0.0, "max_soc_frac": 1.0, "provenance": "spec"},
            "charge_efficiency_curve": curve,
            "discharge_efficiency_curve": curve,
            "grid_forming_in_backup": true,
            "warranty": {},
            "operating_temperature": {"min_c": -20.0, "max_c": 50.0, "provenance": "spec"},
            "ramp_rate": {"max_kw_per_s": ramp_kw_s, "provenance": "estimated"},
            "vendor_api": {"family": "generic", "auth_style": "none", "endpoints": [], "provenance": "estimated"}
        });
        if let Some(pk) = peak_kw {
            doc["peak_discharge_power_kw"] =
                serde_json::json!({"value": pk, "provenance": "spec", "unit": "kW"});
        }
        if let Some(ps) = peak_s {
            doc["peak_duration_s"] =
                serde_json::json!({"value": ps, "provenance": "spec", "unit": "s"});
        }
        for (k, v) in extra.as_object().unwrap() {
            doc[k.as_str()] = v.clone();
        }
        serde_json::from_value(doc).unwrap()
    }

    /// Flat 0.96 efficiency at every power (2-point clamped curve).
    fn flat_curve(cont_kw: f64) -> serde_json::Value {
        serde_json::json!({
            "points": [
                {"x_kw": 0.05 * cont_kw, "efficiency": 0.96},
                {"x_kw": cont_kw, "efficiency": 0.96}
            ],
            "provenance": "estimated"
        })
    }

    /// PW3-shaped LFP unit model: 13.5 kWh, 11.5 kW continuous, no peak.
    fn pw3_like() -> BatteryModel {
        model_json(
            "test.pw3_like",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            serde_json::json!({
                "points": [
                    {"x_kw": 0.575, "efficiency": 0.90},
                    {"x_kw": 2.875, "efficiency": 0.955},
                    {"x_kw": 5.75, "efficiency": 0.965},
                    {"x_kw": 11.5, "efficiency": 0.955}
                ],
                "provenance": "estimated"
            }),
            serde_json::json!({}),
        )
    }

    fn input(p_w: f64, t_c: f64) -> BatteryStepInput {
        BatteryStepInput {
            dt_s: 1,
            p_term_setpoint_w: p_w,
            t_amb_c: t_c,
            grid_present: true,
        }
    }

    fn approx(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
    }

    #[test]
    fn thevenin_sag_anchor_b11_unit_level() {
        // B.11 thevenin_sag: PW3-shaped LFP unit at 5 % SOC and -5 degC
        // delivers 40-60 % of nameplate continuous (11.5 kW).
        let model = pw3_like();
        let mut unit = BatteryUnit::new(&model, None, 0.05, 0.0, BatteryConfig::default()).unwrap();
        let out = unit.step(&input(11_500.0, -5.0));
        let frac = out.p_term_w / 11_500.0;
        assert!(
            (0.40..=0.60).contains(&frac),
            "deliverable {} W = {frac} of nameplate",
            out.p_term_w
        );
        assert!(out.flags.power_limited);
        // Warm mid-SOC: full rating available.
        let mut warm = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        let out = warm.step(&input(11_500.0, 25.0));
        approx(out.p_term_w, 11_500.0, 1e-9);
    }

    #[test]
    fn lfp_cold_charge_block_and_nmc_cutoff_b11() {
        // B.11 lfp_cold_charge_block at the unit level.
        let lfp = pw3_like();
        // Below 0 degC: charge limit is exactly 0.
        let mut unit = BatteryUnit::new(&lfp, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        let out = unit.step(&input(-11_500.0, -5.0));
        assert_eq!(out.p_term_w, 0.0);
        assert!(out.flags.charge_inhibited_cold);
        // Linear recovery: at 5 degC half the charge rating.
        let out = unit.step(&input(-11_500.0, 5.0));
        approx(-out.p_term_w, 5_750.0, 1e-6);
        // NMC: derated only (never blocked) at -5 degC ...
        let nmc = model_json(
            "test.nmc",
            Chemistry::NMC,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            flat_curve(11.5),
            serde_json::json!({}),
        );
        let mut nunit = BatteryUnit::new(&nmc, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        let out = nunit.step(&input(-11_500.0, -5.0));
        let cold = chemistry::cold_charge_factor(Chemistry::NMC, -5.0);
        assert!(cold > 0.0 && cold < 1.0);
        // Charge limit = rating x thermal derate (B.4.4) x cold factor.
        approx(-out.p_term_w, 11_500.0 * chemistry::thermal_derate(-5.0) * cold, 1e-6);
        // ... and a hard discharge cutoff below -20 degC.
        let out = nunit.step(&input(11_500.0, -21.0));
        assert_eq!(out.p_term_w, 0.0);
    }

    #[test]
    fn soc_accounting_split_efficiencies() {
        // Flat 0.96 curve: 1 kWh AC-side charge stores eta * eta_coul.
        let model = model_json(
            "test.flat",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            flat_curve(11.5),
            serde_json::json!({}),
        );
        let mut unit = BatteryUnit::new(
            &model,
            None,
            0.0,
            0.0,
            BatteryConfig {
                thevenin_enabled: false,
                release_reserve_in_outage: true,
            },
        )
        .unwrap();
        for _ in 0..3600 {
            unit.step(&input(-1_000.0, 25.0));
        }
        // 1000 W * 3600 s * 0.96 * 0.99 / 3600 s/h = 950.4 Wh.
        approx(unit.energy_stored_wh(), 950.4, 1e-6);
        approx(unit.cumulative_charge_wh(), 1_000.0, 1e-6);
        // Discharge 3000 s at 1 kW AC: pack drains 1000/0.96 W * 3000 s.
        for _ in 0..3000 {
            unit.step(&input(1_000.0, 25.0));
        }
        approx(
            unit.energy_stored_wh(),
            950.4 - 1_000.0 / 0.96 * 3000.0 / 3600.0,
            1e-6,
        );
        approx(
            unit.cumulative_discharge_wh(),
            1_000.0 * 3000.0 / 3600.0,
            1e-6,
        );
    }

    #[test]
    fn soc_window_adversarial_setpoints() {
        let model = model_json(
            "test.flat",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            flat_curve(11.5),
            serde_json::json!({}),
        );
        // Charge while full: realized collapses, SOC pinned at max.
        let mut full = BatteryUnit::new(&model, None, 1.0, 0.0, BatteryConfig::default()).unwrap();
        for _ in 0..100 {
            let out = full.step(&input(-11_500.0, 25.0));
            assert_eq!(out.p_term_w, 0.0);
            assert!(out.flags.at_soc_max);
            assert!(full.soc() <= 1.0 + 1e-12);
        }
        // Discharge while empty: realized collapses, SOC pinned at min.
        let mut empty = BatteryUnit::new(&model, None, 0.0, 0.0, BatteryConfig::default()).unwrap();
        for _ in 0..100 {
            let out = empty.step(&input(11_500.0, 25.0));
            assert_eq!(out.p_term_w, 0.0);
            assert!(out.flags.at_soc_min);
            assert!(empty.soc() >= -1e-12);
        }
    }

    #[test]
    fn energy_identity_matches_realized_power() {
        // The exact C.7.2 identity from REALIZED terminal power:
        // delta_stored = chg * eta_chg * eta_coul - dis / eta_dis.
        let model = model_json(
            "test.identity",
            Chemistry::NMC,
            Coupling::ACCoupled,
            10.0,
            5.0,
            5.0,
            Some(7.5),
            Some(10.0),
            5.0,
            serde_json::json!({
                "points": [
                    {"x_kw": 0.25, "efficiency": 0.90},
                    {"x_kw": 1.25, "efficiency": 0.94},
                    {"x_kw": 2.5, "efficiency": 0.95},
                    {"x_kw": 5.0, "efficiency": 0.935}
                ],
                "provenance": "estimated"
            }),
            serde_json::json!({}),
        );
        let mut unit = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        let eta_coul = chemistry::eta_coul(Chemistry::NMC);
        let mut expected = unit.energy_stored_wh();
        for sp in [-5_000.0, 4_200.0, -1_300.0, 0.0, 7_500.0, -5_000.0, 2_000.0] {
            let out = unit.step(&input(sp, 25.0));
            let delta = if out.p_term_w >= 0.0 {
                let eta = model.discharge_efficiency_curve.eval(out.p_term_w / 1000.0);
                -(out.p_term_w / eta.max(1e-9)) / 3600.0
            } else {
                let eta = model.charge_efficiency_curve.eval(-out.p_term_w / 1000.0);
                (-out.p_term_w) * eta * eta_coul / 3600.0
            };
            expected += delta;
            approx(unit.energy_stored_wh(), expected, 1e-9);
        }
    }

    #[test]
    fn peak_budget_window_and_recovery() {
        // 10 kW continuous, 15 kW peak for 10 s: 10 s at peak, then the
        // clamp falls to continuous; the budget recovers at the
        // continuous rate (B.2.6).
        let model = model_json(
            "test.peak",
            Chemistry::LFP,
            Coupling::ACCoupled,
            20.0,
            10.0,
            10.0,
            Some(15.0),
            Some(10.0),
            15.0,
            flat_curve(10.0),
            serde_json::json!({}),
        );
        let mut unit = BatteryUnit::new(&model, None, 0.9, 0.0, BatteryConfig::default()).unwrap();
        for i in 0..10 {
            let out = unit.step(&input(15_000.0, 25.0));
            assert_eq!(out.p_term_w, 15_000.0, "tick {i} should sustain peak");
        }
        // Budget exhausted: within the 60 s window the unit clamps to
        // continuous.
        let out = unit.step(&input(15_000.0, 25.0));
        approx(out.p_term_w, 10_000.0, 1e-6);
        assert!(out.flags.power_limited);
        // 5 s at zero setpoint recovers 5 * 10 kW*s = full budget.
        for _ in 0..5 {
            unit.step(&input(0.0, 25.0));
        }
        let out = unit.step(&input(15_000.0, 25.0));
        approx(out.p_term_w, 15_000.0, 1e-6);
    }

    #[test]
    fn ramp_slew_and_min_on_off_suppression() {
        // Ramp: 2 kW/s slew toward the setpoint (AC-coupled: no min
        // on/off).
        let model = model_json(
            "test.ramp",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            10.0,
            10.0,
            None,
            None,
            2.0,
            flat_curve(10.0),
            serde_json::json!({}),
        );
        let mut unit = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        let out = unit.step(&input(10_000.0, 25.0));
        approx(out.p_term_w, 2_000.0, 1e-9);
        let out = unit.step(&input(10_000.0, 25.0));
        approx(out.p_term_w, 4_000.0, 1e-9);
        let out = unit.step(&input(-10_000.0, 25.0));
        approx(out.p_term_w, 2_000.0, 1e-9); // slews down through zero

        // Min on/off: hybrid coupling enforces 60 s / 60 s (dt = 10 s ->
        // 6 tick counters).
        let hybrid = model_json(
            "test.hybrid",
            Chemistry::LFP,
            Coupling::DCCoupledHybrid,
            10.0,
            5.0,
            5.0,
            None,
            None,
            5.0,
            flat_curve(5.0),
            serde_json::json!({}),
        );
        let step10 = |unit: &mut BatteryUnit, p: f64| -> BatteryStepOutput {
            unit.step(&BatteryStepInput {
                dt_s: 10,
                p_term_setpoint_w: p,
                t_amb_c: 25.0,
                grid_present: true,
            })
        };
        let mut h = BatteryUnit::new(&hybrid, None, 0.9, 0.0, BatteryConfig::default()).unwrap();
        let out = step10(&mut h, 5_000.0);
        approx(out.p_term_w, 5_000.0, 1e-9); // ramp allows full swing in 10 s
        assert!(!out.flags.min_on_off_suppressed);
        // Direction change and stop requests are held for 5 more ticks.
        for (i, sp) in [-5_000.0, 0.0, 0.0, -5_000.0, 0.0].iter().enumerate() {
            let out = step10(&mut h, *sp);
            assert!(out.flags.min_on_off_suppressed, "tick {} suppressed", i + 2);
            approx(out.p_term_w, 5_000.0, 1e-9);
        }
        // 60 s elapsed: stop is allowed.
        let out = step10(&mut h, 0.0);
        assert!(!out.flags.min_on_off_suppressed);
        assert_eq!(out.p_term_w, 0.0);
        // Min-off: restart requests are held while the off-time is below
        // 60 s (the stop tick itself counts, so 5 request ticks suppress).
        for i in 0..5 {
            let out = step10(&mut h, 5_000.0);
            assert!(out.flags.min_on_off_suppressed, "min-off tick {i}");
            assert_eq!(out.p_term_w, 0.0);
        }
        let out = step10(&mut h, 5_000.0);
        assert!(!out.flags.min_on_off_suppressed);
        approx(out.p_term_w, 5_000.0, 1e-9);
    }

    #[test]
    fn reserve_floor_grid_and_outage_release() {
        let model = model_json(
            "test.reserve",
            Chemistry::LFP,
            Coupling::ACCoupled,
            10.0,
            10.0,
            10.0,
            None,
            None,
            10.0,
            flat_curve(10.0),
            serde_json::json!({}),
        );
        // Grid present: discharge stops at the 20 % reserve floor.
        let mut unit = BatteryUnit::new(&model, None, 0.9, 0.2, BatteryConfig::default()).unwrap();
        for _ in 0..200 {
            unit.step(&BatteryStepInput {
                dt_s: 60,
                p_term_setpoint_w: 10_000.0,
                t_amb_c: 25.0,
                grid_present: true,
            });
        }
        approx(unit.soc(), 0.2, 1e-6);
        // Outage with release: drains to the hard floor (0).
        for _ in 0..200 {
            unit.step(&BatteryStepInput {
                dt_s: 60,
                p_term_setpoint_w: 10_000.0,
                t_amb_c: 25.0,
                grid_present: false,
            });
        }
        approx(unit.soc(), 0.0, 1e-6);
        // Outage without release: floor stays at the reserve.
        let cfg = BatteryConfig {
            thevenin_enabled: true,
            release_reserve_in_outage: false,
        };
        let mut held = BatteryUnit::new(&model, None, 0.9, 0.2, cfg).unwrap();
        for _ in 0..200 {
            held.step(&BatteryStepInput {
                dt_s: 60,
                p_term_setpoint_w: 10_000.0,
                t_amb_c: 25.0,
                grid_present: false,
            });
        }
        approx(held.soc(), 0.2, 1e-6);
    }

    #[test]
    fn standby_power_from_self_discharge_fold_in() {
        let model = model_json(
            "test.standby",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            flat_curve(11.5),
            serde_json::json!({
                "self_discharge_frac_per_day": {"value": 0.002, "provenance": "estimated", "unit": "frac/day"}
            }),
        );
        let unit = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        approx(unit.standby_power_w(), 0.002 * 13_500.0 / 24.0, 1e-12);
        // Absent field: Part A §5 default 0.002/day.
        let mut bare = model.clone();
        bare.self_discharge_frac_per_day = None;
        let unit = BatteryUnit::new(&bare, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        approx(unit.standby_power_w(), 0.002 * 13_500.0 / 24.0, 1e-12);
    }

    #[test]
    fn expansion_packs_add_energy_only() {
        let head = model_json(
            "test.head",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            11.5,
            flat_curve(11.5),
            serde_json::json!({
                "expansion": {
                    "max_units_per_inverter": 4,
                    "expansion_pack_model_id": "test.pack",
                    "packs_add_power": false
                }
            }),
        );
        let pack = model_json(
            "test.pack",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            0.0,
            0.0,
            None,
            None,
            1.0,
            flat_curve(1.0),
            serde_json::json!({}),
        );
        let unit =
            BatteryUnit::new(&head, Some((&pack, 2)), 0.5, 0.0, BatteryConfig::default()).unwrap();
        approx(unit.usable_energy_wh(), 3.0 * 13_500.0, 1e-9);
        // Power limits stay the head unit's.
        approx(unit.max_discharge_w(), 11_500.0, 1.0);
        // Wrong pack identity rejected; too many packs rejected.
        let wrong = model_json(
            "test.other",
            Chemistry::LFP,
            Coupling::ACCoupled,
            5.0,
            0.0,
            0.0,
            None,
            None,
            1.0,
            flat_curve(1.0),
            serde_json::json!({}),
        );
        assert!(
            BatteryUnit::new(&head, Some((&wrong, 1)), 0.5, 0.0, BatteryConfig::default()).is_err()
        );
        assert!(
            BatteryUnit::new(&head, Some((&pack, 4)), 0.5, 0.0, BatteryConfig::default()).is_err()
        );
        // No expansion declaration on the head: rejected.
        assert!(
            BatteryUnit::new(&pack, Some((&pack, 1)), 0.5, 0.0, BatteryConfig::default()).is_err()
        );
    }

    #[test]
    fn new_validates_window_soc_and_reserve() {
        let model = pw3_like();
        assert!(BatteryUnit::new(&model, None, 1.5, 0.0, BatteryConfig::default()).is_err());
        assert!(BatteryUnit::new(&model, None, -0.1, 0.0, BatteryConfig::default()).is_err());
        assert!(BatteryUnit::new(&model, None, 0.5, 1.2, BatteryConfig::default()).is_err());
        assert!(BatteryUnit::new(&model, None, 0.5, 0.2, BatteryConfig::default()).is_ok());
    }

    #[test]
    fn sub_stepped_large_dt_matches_summed_small_dt() {
        // B.1.6: one 30 s tick integrates the same stored energy as six
        // 5 s ticks at constant setpoint (limits are tick-level, so only
        // rounding differs).
        let model = model_json(
            "test.substep",
            Chemistry::LFP,
            Coupling::ACCoupled,
            13.5,
            11.5,
            11.5,
            None,
            None,
            100.0, // fast ramp: slew never binds at these setpoints
            flat_curve(11.5),
            serde_json::json!({}),
        );
        let mut big = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        big.step(&BatteryStepInput {
            dt_s: 30,
            p_term_setpoint_w: 1_000.0,
            t_amb_c: 25.0,
            grid_present: true,
        });
        let mut small = BatteryUnit::new(&model, None, 0.5, 0.0, BatteryConfig::default()).unwrap();
        for _ in 0..6 {
            small.step(&BatteryStepInput {
                dt_s: 5,
                p_term_setpoint_w: 1_000.0,
                t_amb_c: 25.0,
                grid_present: true,
            });
        }
        approx(big.energy_stored_wh(), small.energy_stored_wh(), 1e-9);
        // SOC stays inside the window under a large-dt charge-while-full.
        let mut full = BatteryUnit::new(&model, None, 1.0, 0.0, BatteryConfig::default()).unwrap();
        full.step(&BatteryStepInput {
            dt_s: 300,
            p_term_setpoint_w: -11_500.0,
            t_amb_c: 25.0,
            grid_present: true,
        });
        assert!(full.soc() <= 1.0 + 1e-12);
    }
}
