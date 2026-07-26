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

use batsim_registry::{BatteryModel, Chemistry};
use serde::{Deserialize, Serialize};

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

/// One physical battery unit with full per-tick physics state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryUnit {
    // Implemented by the physics task. Internals include (spec B.2.1):
    // model data, soc, q_avail_wh, cumulative throughput counters, peak
    // budget accumulator (B.2.6), ramp integrator (B.2.7), integer min
    // on/off tick counters, DeviceState.
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
        let _ = (model, expansion_pack, initial_soc_frac, reserve_frac, config);
        todo!("implemented by physics task")
    }

    /// Advance the unit one tick. Applies, in order (spec B.2/B.1.5 stage
    /// 5): ramp slew, min on/off enforcement, dynamic limits (chemistry
    /// cold rules, thermal derate, Thevenin sag, charge taper, peak
    /// budget), clamped integration of the SOC ODE with separated
    /// charge/discharge efficiencies, throughput bookkeeping.
    pub fn step(&mut self, input: &BatteryStepInput) -> BatteryStepOutput {
        let _ = input;
        todo!("implemented by physics task")
    }

    /// SOC as a fraction of current available (usable) capacity.
    #[must_use]
    pub fn soc(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Energy currently stored above the SOC-window floor (Wh).
    #[must_use]
    pub fn energy_stored_wh(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Current available capacity `Q_avail` (Wh). Constant in M1.
    #[must_use]
    pub fn usable_energy_wh(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Dynamic discharge limit at the terminal boundary (W) under current
    /// SOC/temperature: continuous rating x thermal derate, Thevenin sag,
    /// SOC taper, and remaining peak budget.
    #[must_use]
    pub fn max_discharge_w(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Dynamic charge limit at the terminal boundary (W), including
    /// chemistry cold-charge rules and high-SOC taper.
    #[must_use]
    pub fn max_charge_w(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Standby/self-consumption draw (W) while energized, taken from the AC
    /// side in metering (spec B.3.2). Derived from the registry
    /// `self_discharge_frac_per_day` fold-in per Part A §5:
    /// `frac_per_day * usable_energy_wh / 24 h`.
    #[must_use]
    pub fn standby_power_w(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Cumulative charge throughput at the terminal boundary (Wh).
    #[must_use]
    pub fn cumulative_charge_wh(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// Cumulative discharge throughput at the terminal boundary (Wh).
    #[must_use]
    pub fn cumulative_discharge_wh(&self) -> f64 {
        todo!("implemented by physics task")
    }

    /// The registry model this unit runs.
    #[must_use]
    pub fn model(&self) -> &BatteryModel {
        todo!("implemented by physics task")
    }

    /// Chemistry of the unit (dispatchers use it for cold-charge policy).
    #[must_use]
    pub fn chemistry(&self) -> Chemistry {
        self.model().chemistry
    }

    /// Current device state (B.9.1 vocabulary).
    #[must_use]
    pub fn state(&self) -> DeviceState {
        todo!("implemented by physics task")
    }

    /// User backup-reserve floor (fraction of usable).
    #[must_use]
    pub fn reserve_frac(&self) -> f64 {
        todo!("implemented by physics task")
    }
}
