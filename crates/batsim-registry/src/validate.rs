//! Semantic validation of catalog entries beyond JSON Schema: cross-field
//! invariants the schema cannot express, plus the §4.6 cross-reference
//! checks. All violations are collected, never fail-fast.
//!
//! Checks (non-exhaustive, see implementations):
//! - `schema_version` equals [`crate::types::SCHEMA_VERSION`].
//! - `model_id` matches `^vendor.model$` and the vendor prefix matches the
//!   `vendor` field.
//! - Efficiency curves: >= 2 points, strictly ascending `x_kw`,
//!   efficiencies in `[0, 1]`.
//! - SOC window: `0 <= min < max <= 1`, reserve floor inside the window.
//! - `usable_energy_kwh <= nameplate_energy_kwh`.
//! - Power/energy values are finite and non-negative.
//! - Enphase rule: `continuous_power == 0.64 kW x microinverter_count`
//!   when both are declared (spec §3.1).
//! - Cross-references: `requires_controller_id`, `compatible_battery_ids`,
//!   `expansion_pack_model_id`, PV preset inverter ids resolve (§4.6).

use crate::error::Violation;
use crate::load::Registry;
use crate::types::{BatteryModel, ControllerModel, InverterModel, PvPreset};

/// Validate one battery entry; returns all violations found.
#[must_use]
pub fn check_battery(path: &str, model: &BatteryModel) -> Vec<Violation> {
    let _ = (path, model);
    todo!("implemented by catalog task")
}

/// Validate one inverter entry; returns all violations found.
#[must_use]
pub fn check_inverter(path: &str, model: &InverterModel) -> Vec<Violation> {
    let _ = (path, model);
    todo!("implemented by catalog task")
}

/// Validate one controller entry; returns all violations found.
#[must_use]
pub fn check_controller(path: &str, model: &ControllerModel) -> Vec<Violation> {
    let _ = (path, model);
    todo!("implemented by catalog task")
}

/// Validate one PV preset entry; returns all violations found.
#[must_use]
pub fn check_pv_preset(path: &str, preset: &PvPreset) -> Vec<Violation> {
    let _ = (path, preset);
    todo!("implemented by catalog task")
}

/// Cross-reference validation across the whole loaded registry (§4.6):
/// controller references, battery/inverter compatibility, expansion-pack
/// references. Run after all entries pass per-entry checks.
#[must_use]
pub fn check_cross_references(registry: &Registry) -> Vec<Violation> {
    let _ = registry;
    todo!("implemented by catalog task")
}
