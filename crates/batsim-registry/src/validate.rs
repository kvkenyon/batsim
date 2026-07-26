//! Semantic validation of catalog entries beyond JSON Schema: cross-field
//! invariants the schema cannot express, plus the §4.6 cross-reference
//! checks. All violations are collected, never fail-fast.
//!
//! Checks (non-exhaustive, see implementations):
//! - `schema_version` equals [`crate::types::SCHEMA_VERSION`].
//! - `model_id` matches `^vendor.model$` (`^[a-z0-9_]+\.[a-z0-9_]+$`) and
//!   the vendor prefix matches the `vendor` field (case-insensitive).
//! - Efficiency curves: >= 2 points, strictly ascending `x_kw`,
//!   efficiencies in `[0, 1]`.
//! - SOC window: `0 <= min < max <= 1`, reserve floor inside the window.
//! - `usable_energy_kwh <= nameplate_energy_kwh`.
//! - Power/energy values are finite and non-negative.
//! - Enphase rule (spec §3.1, ceiling form): declared continuous power
//!   must not exceed `power_per_microinverter_kw x microinverter_count`.
//!   Enclosure AC limits (IQ Battery 10/10C) legitimately sit below the
//!   aggregate IQ8D ceiling, so equality is not required.
//! - Cross-references: `requires_controller_id`, `compatible_battery_ids`,
//!   `expansion_pack_model_id`, PV preset inverter ids resolve (§4.6).

use crate::error::Violation;
use crate::load::Registry;
use crate::types::{
    AnnotatedNumber, BatteryModel, ControllerModel, Coupling, EfficiencyCurve, EntryKind,
    InverterModel, InverterTopology, PvPreset,
};

/// Violation constructor helper.
fn violation(path: &str, field: &str, message: impl Into<String>) -> Violation {
    Violation {
        path: path.to_owned(),
        field: field.to_owned(),
        message: message.into(),
    }
}

/// `schema_version` must equal the supported catalog schema version.
fn check_schema_version(path: &str, actual: &str, out: &mut Vec<Violation>) {
    if actual != crate::types::SCHEMA_VERSION {
        out.push(violation(
            path,
            "schema_version",
            format!(
                "unsupported schema_version `{actual}`; expected `{}`",
                crate::types::SCHEMA_VERSION
            ),
        ));
    }
}

/// `model_id` (or `preset_id`) must match `^[a-z0-9_]+\.[a-z0-9_]+$`
/// (exactly one dot; lowercase alphanumerics and underscores). When the
/// entry carries a `vendor` field, the prefix before the dot must match it
/// case-insensitively (vendor-prefix consistency).
fn check_identifier(
    path: &str,
    field: &str,
    id: &str,
    vendor: Option<&str>,
    out: &mut Vec<Violation>,
) {
    let valid_segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    let mut parts = id.split('.');
    let (prefix, rest) = (parts.next(), parts.next());
    let well_formed = matches!((prefix, rest, parts.next()), (Some(p), Some(r), None) if valid_segment(p) && valid_segment(r));
    if !well_formed {
        out.push(violation(
            path,
            field,
            format!("`{id}` does not match `^[a-z0-9_]+\\.[a-z0-9_]+$`"),
        ));
        return;
    }
    if let (Some(vendor), Some(prefix)) = (vendor, prefix) {
        if vendor.to_ascii_lowercase() != prefix {
            out.push(violation(
                path,
                field,
                format!("model_id prefix `{prefix}` does not match vendor `{vendor}`"),
            ));
        }
    }
}

/// Efficiency curve invariants: minimum 2 points, finite non-negative
/// strictly ascending `x_kw`, efficiencies finite in `[0, 1]`.
fn check_curve(path: &str, field: &str, curve: &EfficiencyCurve, out: &mut Vec<Violation>) {
    if curve.points.len() < 2 {
        out.push(violation(
            path,
            field,
            format!(
                "efficiency curve needs >= 2 points, has {}",
                curve.points.len()
            ),
        ));
    }
    let mut prev_x: Option<f64> = None;
    for (i, point) in curve.points.iter().enumerate() {
        let pf = format!("{field}.points[{i}]");
        if !point.x_kw.is_finite() || point.x_kw < 0.0 {
            out.push(violation(
                path,
                &pf,
                format!("x_kw {} is not finite and non-negative", point.x_kw),
            ));
        }
        if !point.efficiency.is_finite() || !(0.0..=1.0).contains(&point.efficiency) {
            out.push(violation(
                path,
                &pf,
                format!("efficiency {} is outside [0, 1]", point.efficiency),
            ));
        }
        if let Some(prev) = prev_x {
            if point.x_kw <= prev {
                out.push(violation(
                    path,
                    &pf,
                    format!(
                        "x_kw {} is not strictly ascending (previous {prev})",
                        point.x_kw
                    ),
                ));
            }
        }
        prev_x = Some(point.x_kw);
    }
}

/// A power/energy annotated number must be finite and non-negative.
fn check_non_negative(path: &str, field: &str, n: &AnnotatedNumber, out: &mut Vec<Violation>) {
    if !n.value.is_finite() || n.value < 0.0 {
        out.push(violation(
            path,
            field,
            format!("value {} is not finite and non-negative", n.value),
        ));
    }
}

/// A rated value must be finite and strictly positive.
fn check_positive(path: &str, field: &str, n: &AnnotatedNumber, out: &mut Vec<Violation>) {
    if !n.value.is_finite() || n.value <= 0.0 {
        out.push(violation(
            path,
            field,
            format!("value {} is not finite and positive", n.value),
        ));
    }
}

/// An optional fraction in `[0, 1]` (RTE, self-discharge).
fn check_frac(path: &str, field: &str, n: &AnnotatedNumber, out: &mut Vec<Violation>) {
    if !n.value.is_finite() || !(0.0..=1.0).contains(&n.value) {
        out.push(violation(
            path,
            field,
            format!("value {} is outside [0, 1]", n.value),
        ));
    }
}

/// Energy fields: non-negative; usable must fit inside nameplate.
fn check_battery_energy(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    check_non_negative(
        path,
        "nameplate_energy_kwh",
        &model.nameplate_energy_kwh,
        out,
    );
    check_non_negative(path, "usable_energy_kwh", &model.usable_energy_kwh, out);
    if model.usable_energy_kwh.value > model.nameplate_energy_kwh.value {
        out.push(violation(
            path,
            "usable_energy_kwh",
            format!(
                "usable {} kWh exceeds nameplate {} kWh",
                model.usable_energy_kwh.value, model.nameplate_energy_kwh.value
            ),
        ));
    }
}

/// Power fields: finite non-negative; peak >= continuous; peak and its
/// sustain duration are declared together.
fn check_battery_power(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    check_non_negative(
        path,
        "continuous_discharge_power_kw",
        &model.continuous_discharge_power_kw,
        out,
    );
    check_non_negative(
        path,
        "continuous_charge_power_kw",
        &model.continuous_charge_power_kw,
        out,
    );
    if let Some(peak) = &model.peak_discharge_power_kw {
        check_non_negative(path, "peak_discharge_power_kw", peak, out);
        if peak.value < model.continuous_discharge_power_kw.value {
            out.push(violation(
                path,
                "peak_discharge_power_kw",
                format!(
                    "peak {} kW below continuous {} kW",
                    peak.value, model.continuous_discharge_power_kw.value
                ),
            ));
        }
    }
    if let Some(duration) = &model.peak_duration_s {
        check_positive(path, "peak_duration_s", duration, out);
    }
    if model.peak_discharge_power_kw.is_some() != model.peak_duration_s.is_some() {
        out.push(violation(
            path,
            "peak_discharge_power_kw",
            "peak power and peak duration must be declared together".to_owned(),
        ));
    }
}

/// SOC window: `0 <= min < max <= 1`, reserve floor inside the window.
fn check_battery_soc(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    let soc = &model.soc_window;
    if !soc.min_soc_frac.is_finite()
        || !soc.max_soc_frac.is_finite()
        || soc.min_soc_frac < 0.0
        || soc.min_soc_frac >= soc.max_soc_frac
        || soc.max_soc_frac > 1.0
    {
        out.push(violation(
            path,
            "soc_window",
            format!(
                "SOC window [{}, {}] violates 0 <= min < max <= 1",
                soc.min_soc_frac, soc.max_soc_frac
            ),
        ));
    } else if let Some(floor) = soc.reserve_floor_frac {
        if !floor.is_finite() || floor < soc.min_soc_frac || floor > soc.max_soc_frac {
            out.push(violation(
                path,
                "soc_window.reserve_floor_frac",
                format!(
                    "reserve floor {floor} outside SOC window [{}, {}]",
                    soc.min_soc_frac, soc.max_soc_frac
                ),
            ));
        }
    }
}

/// Enphase rule (spec §3.1, ceiling form): declared continuous power may
/// not exceed `power_per_microinverter_kw x microinverter_count`. Units
/// with enclosure AC limits (IQ Battery 10/10C) sit below the ceiling.
fn check_battery_microinverters(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    if let Some(count) = model.microinverter_count {
        if count == 0 {
            out.push(violation(
                path,
                "microinverter_count",
                "must be >= 1 when declared".to_owned(),
            ));
        }
        if model.coupling != Coupling::MicroinverterBased {
            out.push(violation(
                path,
                "microinverter_count",
                "declared on a non-microinverter-based entry".to_owned(),
            ));
        }
        if let Some(per_micro) = &model.power_per_microinverter_kw {
            check_positive(path, "power_per_microinverter_kw", per_micro, out);
            let ceiling = per_micro.value * f64::from(count);
            if model.continuous_discharge_power_kw.value > ceiling + 1e-9 {
                out.push(violation(
                    path,
                    "continuous_discharge_power_kw",
                    format!(
                        "continuous {} kW exceeds {} kW x {} microinverters ({} kW ceiling, spec §3.1)",
                        model.continuous_discharge_power_kw.value, per_micro.value, count, ceiling
                    ),
                ));
            }
        }
    }
    if model.coupling == Coupling::MicroinverterBased && model.microinverter_count.is_none() {
        out.push(violation(
            path,
            "microinverter_count",
            "microinverter-based coupling requires an explicit microinverter_count (spec §2.3)"
                .to_owned(),
        ));
    }
}

/// Expansion metadata shape; reference resolution is a §4.6 cross-check.
fn check_battery_expansion(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    if let Some(expansion) = &model.expansion {
        if let Some(max_units) = expansion.max_units_per_inverter {
            if max_units < 1 {
                out.push(violation(
                    path,
                    "expansion.max_units_per_inverter",
                    "must be >= 1 when declared".to_owned(),
                ));
            }
        }
        if let Some(pack_id) = &expansion.expansion_pack_model_id {
            check_identifier(
                path,
                "expansion.expansion_pack_model_id",
                pack_id,
                None,
                out,
            );
        }
    }
}

/// Warranty figures are telemetry-only but must be sane.
fn check_battery_warranty(path: &str, model: &BatteryModel, out: &mut Vec<Violation>) {
    if let Some(years) = &model.warranty.years {
        check_positive(path, "warranty.years", years, out);
    }
    if let Some(cycles) = &model.warranty.cycles {
        check_non_negative(path, "warranty.cycles", cycles, out);
    }
    if let Some(throughput) = &model.warranty.throughput_mwh {
        check_non_negative(path, "warranty.throughput_mwh", throughput, out);
    }
    if let Some(retention) = &model.warranty.capacity_retention_pct {
        if !retention.value.is_finite() || !(0.0..=100.0).contains(&retention.value) {
            out.push(violation(
                path,
                "warranty.capacity_retention_pct",
                format!("retention {} outside [0, 100] pct", retention.value),
            ));
        }
    }
}

/// Validate one battery entry; returns all violations found.
#[must_use]
pub fn check_battery(path: &str, model: &BatteryModel) -> Vec<Violation> {
    let mut out = Vec::new();
    check_schema_version(path, &model.schema_version, &mut out);
    check_identifier(
        path,
        "model_id",
        &model.model_id,
        Some(&model.vendor),
        &mut out,
    );
    check_battery_energy(path, model, &mut out);
    check_battery_power(path, model, &mut out);
    check_battery_soc(path, model, &mut out);
    check_curve(
        path,
        "charge_efficiency_curve",
        &model.charge_efficiency_curve,
        &mut out,
    );
    check_curve(
        path,
        "discharge_efficiency_curve",
        &model.discharge_efficiency_curve,
        &mut out,
    );
    if let Some(rte) = &model.rte_pv_coupled {
        check_frac(path, "rte_pv_coupled", rte, &mut out);
    }
    if let Some(rte) = &model.rte_ac_coupled {
        check_frac(path, "rte_ac_coupled", rte, &mut out);
    }
    if let Some(sd) = &model.self_discharge_frac_per_day {
        check_frac(path, "self_discharge_frac_per_day", sd, &mut out);
    }
    check_battery_microinverters(path, model, &mut out);
    check_battery_expansion(path, model, &mut out);

    // Thermal / ramp.
    let temp = &model.operating_temperature;
    if !temp.min_c.is_finite() || !temp.max_c.is_finite() || temp.min_c >= temp.max_c {
        out.push(violation(
            path,
            "operating_temperature",
            format!(
                "temperature range [{}, {}] is invalid",
                temp.min_c, temp.max_c
            ),
        ));
    }
    if !model.ramp_rate.max_kw_per_s.is_finite() || model.ramp_rate.max_kw_per_s < 0.0 {
        out.push(violation(
            path,
            "ramp_rate.max_kw_per_s",
            format!(
                "ramp rate {} is not finite and non-negative",
                model.ramp_rate.max_kw_per_s
            ),
        ));
    }
    check_battery_warranty(path, model, &mut out);
    out
}

/// Validate one inverter entry; returns all violations found.
#[must_use]
pub fn check_inverter(path: &str, model: &InverterModel) -> Vec<Violation> {
    let mut out = Vec::new();
    check_schema_version(path, &model.schema_version, &mut out);
    check_identifier(
        path,
        "model_id",
        &model.model_id,
        Some(&model.vendor),
        &mut out,
    );

    check_positive(
        path,
        "rated_ac_output_kw",
        &model.rated_ac_output_kw,
        &mut out,
    );
    if let Some(backup) = &model.max_ac_output_kw_backup {
        check_positive(path, "max_ac_output_kw_backup", backup, &mut out);
    }
    if let Some(pv) = &model.max_pv_dc_input_kw {
        check_positive(path, "max_pv_dc_input_kw", pv, &mut out);
    }
    if let Some(mppt) = &model.mppt_count {
        check_positive(path, "mppt_count", mppt, &mut out);
    }
    if let Some(volts) = &model.max_pv_voltage_v {
        check_positive(path, "max_pv_voltage_v", volts, &mut out);
    }
    check_curve(path, "efficiency_curve", &model.efficiency_curve, &mut out);

    // Entry-level hybrid rule: a DC-coupled hybrid inverter is meaningless
    // without at least one compatible battery (spec §3.1, §4.3).
    if model.topology == InverterTopology::HybridDCCoupled
        && model.compatible_battery_ids.is_empty()
    {
        out.push(violation(
            path,
            "compatible_battery_ids",
            "hybrid DC-coupled inverter must declare at least one compatible battery".to_owned(),
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (i, id) in model.compatible_battery_ids.iter().enumerate() {
        let field = format!("compatible_battery_ids[{i}]");
        check_identifier(path, &field, id, None, &mut out);
        if seen.contains(&id.as_str()) {
            out.push(violation(
                path,
                &field,
                format!("duplicate compatible battery `{id}`"),
            ));
        }
        seen.push(id.as_str());
    }
    if let Some(max_batteries) = model.max_batteries {
        if max_batteries < 1 {
            out.push(violation(
                path,
                "max_batteries",
                "must be >= 1 when declared".to_owned(),
            ));
        }
    }

    out
}

/// Validate one controller entry; returns all violations found.
#[must_use]
pub fn check_controller(path: &str, model: &ControllerModel) -> Vec<Violation> {
    let mut out = Vec::new();
    check_schema_version(path, &model.schema_version, &mut out);
    check_identifier(
        path,
        "model_id",
        &model.model_id,
        Some(&model.vendor),
        &mut out,
    );

    check_positive(path, "transfer_time_s", &model.transfer_time_s, &mut out);
    if let Some(reconnect) = &model.reconnect_s {
        check_positive(path, "reconnect_s", reconnect, &mut out);
    }
    if let Some(curve) = &model.frequency_shift_curtailment {
        if !curve.start_hz.is_finite()
            || !curve.full_curtail_hz.is_finite()
            || curve.start_hz <= 0.0
            || curve.start_hz >= curve.full_curtail_hz
        {
            out.push(violation(
                path,
                "frequency_shift_curtailment",
                format!(
                    "curtailment span {} -> {} Hz violates 0 < start < full",
                    curve.start_hz, curve.full_curtail_hz
                ),
            ));
        }
    }
    if let Some(backup) = &model.max_backup_power_kw {
        check_positive(path, "max_backup_power_kw", backup, &mut out);
    }
    if let Some(standby) = &model.standby_power_w {
        check_non_negative(path, "standby_power_w", standby, &mut out);
    }

    out
}

/// Validate one PV preset entry; returns all violations found.
#[must_use]
pub fn check_pv_preset(path: &str, preset: &PvPreset) -> Vec<Violation> {
    let mut out = Vec::new();
    check_schema_version(path, &preset.schema_version, &mut out);
    check_identifier(path, "preset_id", &preset.preset_id, None, &mut out);

    check_positive(path, "kw_dc", &preset.kw_dc, &mut out);
    if !preset.tilt_deg.is_finite() || !(0.0..=90.0).contains(&preset.tilt_deg) {
        out.push(violation(
            path,
            "tilt_deg",
            format!("tilt {} outside [0, 90] degrees", preset.tilt_deg),
        ));
    }
    if !preset.azimuth_deg.is_finite() || !(0.0..360.0).contains(&preset.azimuth_deg) {
        out.push(violation(
            path,
            "azimuth_deg",
            format!("azimuth {} outside [0, 360) degrees", preset.azimuth_deg),
        ));
    }
    if !preset.dc_ac_ratio.is_finite() || preset.dc_ac_ratio <= 0.0 {
        out.push(violation(
            path,
            "dc_ac_ratio",
            format!(
                "DC/AC ratio {} is not finite and positive",
                preset.dc_ac_ratio
            ),
        ));
    }
    if let Some(inverter_id) = &preset.pv_inverter_model_id {
        check_identifier(path, "pv_inverter_model_id", inverter_id, None, &mut out);
    }
    if let Some(count) = preset.microinverter_count {
        if count < 1 {
            out.push(violation(
                path,
                "microinverter_count",
                "must be >= 1 when declared".to_owned(),
            ));
        }
    }

    out
}

/// Reconstruct the canonical registry-relative path of an entry
/// (`model_id` dots become underscores in filenames, spec §1.1/§4.6).
fn entry_path(kind: EntryKind, model_id: &str) -> String {
    format!("{}/{}.json", kind.dir(), model_id.replace('.', "_"))
}

/// Cross-reference validation across the whole loaded registry (§4.6):
/// controller references, battery/inverter compatibility, expansion-pack
/// references. Run after all entries pass per-entry checks.
#[must_use]
pub fn check_cross_references(registry: &Registry) -> Vec<Violation> {
    let mut out = Vec::new();

    for battery in registry.batteries() {
        let path = entry_path(EntryKind::Battery, &battery.model_id);
        if let Some(controller_id) = &battery.requires_controller_id {
            if registry.controller(controller_id).is_none() {
                out.push(violation(
                    &path,
                    "requires_controller_id",
                    format!("unknown controller model `{controller_id}`"),
                ));
            }
        }
        if let Some(expansion) = &battery.expansion {
            if let Some(pack_id) = &expansion.expansion_pack_model_id {
                if pack_id == &battery.model_id {
                    out.push(violation(
                        &path,
                        "expansion.expansion_pack_model_id",
                        "expansion pack must not reference its own model_id".to_owned(),
                    ));
                } else if registry.battery(pack_id).is_none() {
                    out.push(violation(
                        &path,
                        "expansion.expansion_pack_model_id",
                        format!("unknown battery model `{pack_id}`"),
                    ));
                }
            }
        }
        // Intersection rule: a DC-coupled battery without an integrated
        // inverter must be claimed by at least one hybrid inverter's
        // compatible_battery_ids (spec §3.1, §4.6).
        if battery.coupling == Coupling::DCCoupledHybrid
            && battery.integrated_inverter != Some(true)
        {
            let claimed = registry.inverters().any(|inv| {
                inv.topology == InverterTopology::HybridDCCoupled
                    && inv.compatible_battery_ids.contains(&battery.model_id)
            });
            if !claimed {
                out.push(violation(
                    &path,
                    "coupling",
                    "DC-coupled battery without integrated inverter is not listed in any hybrid inverter's compatible_battery_ids".to_owned(),
                ));
            }
        }
    }

    for inverter in registry.inverters() {
        let path = entry_path(EntryKind::Inverter, &inverter.model_id);
        for (i, id) in inverter.compatible_battery_ids.iter().enumerate() {
            if registry.battery(id).is_none() {
                out.push(violation(
                    &path,
                    &format!("compatible_battery_ids[{i}]"),
                    format!("unknown battery model `{id}`"),
                ));
            }
        }
    }

    for preset in registry.pv_presets() {
        let path = entry_path(EntryKind::PvPreset, &preset.preset_id);
        if let Some(inverter_id) = &preset.pv_inverter_model_id {
            if registry.inverter(inverter_id).is_none() {
                out.push(violation(
                    &path,
                    "pv_inverter_model_id",
                    format!("unknown inverter model `{inverter_id}`"),
                ));
            }
        }
    }

    out
}

#[cfg(test)]
// unwrap/expect: test assertions may abort on fixture setup. float_cmp:
// catalog constants round-trip JSON bit-exactly, so exact equality against
// the same literal is the intended assertion.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn pw2() -> BatteryModel {
        Registry::embedded()
            .unwrap()
            .battery("tesla.powerwall_2")
            .unwrap()
            .clone()
    }

    fn fields(violations: &[Violation]) -> Vec<&str> {
        violations.iter().map(|v| v.field.as_str()).collect()
    }

    #[test]
    fn embedded_entries_pass_all_checks() {
        let registry = Registry::embedded().unwrap();
        for battery in registry.batteries() {
            assert!(
                check_battery(&battery.model_id, battery).is_empty(),
                "{}: {:?}",
                battery.model_id,
                check_battery(&battery.model_id, battery)
            );
        }
        for inverter in registry.inverters() {
            assert!(check_inverter(&inverter.model_id, inverter).is_empty());
        }
        for controller in registry.controllers() {
            assert!(check_controller(&controller.model_id, controller).is_empty());
        }
        for preset in registry.pv_presets() {
            assert!(check_pv_preset(&preset.preset_id, preset).is_empty());
        }
        assert!(check_cross_references(&registry).is_empty());
    }

    #[test]
    fn broken_curve_is_enumerated() {
        let mut battery = pw2();
        // Single point: below the 2-point minimum.
        battery.charge_efficiency_curve.points.truncate(1);
        let violations = check_battery("batteries/tesla_powerwall_2.json", &battery);
        assert!(
            fields(&violations).contains(&"charge_efficiency_curve"),
            "{violations:?}"
        );

        // Non-ascending x_kw.
        let mut battery = pw2();
        battery.discharge_efficiency_curve.points.reverse();
        let violations = check_battery("batteries/tesla_powerwall_2.json", &battery);
        assert!(
            fields(&violations)
                .iter()
                .any(|f| f.starts_with("discharge_efficiency_curve")),
            "{violations:?}"
        );

        // Efficiency out of [0, 1].
        let mut battery = pw2();
        battery.charge_efficiency_curve.points[0].efficiency = 1.4;
        let violations = check_battery("batteries/tesla_powerwall_2.json", &battery);
        assert!(
            fields(&violations)
                .iter()
                .any(|f| f.starts_with("charge_efficiency_curve")),
            "{violations:?}"
        );
    }

    #[test]
    fn bad_model_id_is_enumerated() {
        for bad in [
            "Tesla.Powerwall_2",
            "tesla-powerwall-2",
            "tesla.powerwall.2",
            ".pw2",
        ] {
            let mut battery = pw2();
            battery.model_id = bad.to_owned();
            let violations = check_battery("batteries/x.json", &battery);
            assert!(
                fields(&violations).contains(&"model_id"),
                "{bad}: {violations:?}"
            );
        }
        // Vendor-prefix mismatch.
        let mut battery = pw2();
        battery.vendor = "Acme".to_owned();
        let violations = check_battery("batteries/x.json", &battery);
        assert!(fields(&violations).contains(&"model_id"), "{violations:?}");
    }

    #[test]
    fn inverted_soc_window_is_enumerated() {
        let mut battery = pw2();
        battery.soc_window.min_soc_frac = 0.8;
        battery.soc_window.max_soc_frac = 0.2;
        let violations = check_battery("batteries/x.json", &battery);
        assert!(
            fields(&violations).contains(&"soc_window"),
            "{violations:?}"
        );

        // Reserve floor outside the window.
        let mut battery = pw2();
        battery.soc_window.reserve_floor_frac = Some(1.5);
        let violations = check_battery("batteries/x.json", &battery);
        assert!(
            fields(&violations).contains(&"soc_window.reserve_floor_frac"),
            "{violations:?}"
        );
    }

    #[test]
    fn usable_above_nameplate_is_enumerated() {
        let mut battery = pw2();
        battery.usable_energy_kwh.value = 15.0;
        let violations = check_battery("batteries/x.json", &battery);
        assert!(
            fields(&violations).contains(&"usable_energy_kwh"),
            "{violations:?}"
        );
    }

    #[test]
    fn enphase_ceiling_rule_is_enforced() {
        let five_p = Registry::embedded()
            .unwrap()
            .battery("enphase.iq_battery_5p")
            .unwrap()
            .clone();
        // Declared 3.84 kW == 0.64 x 6 ceiling: passes.
        assert!(check_battery("batteries/x.json", &five_p).is_empty());
        // Overstated continuous power exceeds the microinverter ceiling.
        let mut broken = five_p;
        broken.continuous_discharge_power_kw.value = 7.68;
        let violations = check_battery("batteries/x.json", &broken);
        assert!(
            fields(&violations).contains(&"continuous_discharge_power_kw"),
            "{violations:?}"
        );
    }

    #[test]
    fn cross_reference_failures_are_enumerated() {
        let battery = pw2();
        let registry = Registry::from_parts(vec![battery], vec![], vec![]);
        let violations = check_cross_references(&registry);
        assert!(
            fields(&violations).contains(&"requires_controller_id"),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .all(|v| v.path == "batteries/tesla_powerwall_2.json"),
            "{violations:?}"
        );

        // Inverter referencing an unknown battery.
        let hub = Registry::embedded()
            .unwrap()
            .inverter("solaredge.home_hub_hd_wave")
            .unwrap()
            .clone();
        let registry = Registry::from_parts(vec![], vec![hub], vec![]);
        let violations = check_cross_references(&registry);
        assert!(
            fields(&violations).contains(&"compatible_battery_ids[0]"),
            "{violations:?}"
        );

        // DC-coupled battery without integrated inverter and no claiming
        // hybrid inverter.
        let se = Registry::embedded()
            .unwrap()
            .battery("solaredge.home_battery_400v")
            .unwrap()
            .clone();
        let controller = Registry::embedded()
            .unwrap()
            .controller("solaredge.backup_interface")
            .unwrap()
            .clone();
        let registry = Registry::from_parts(vec![se], vec![], vec![controller]);
        let violations = check_cross_references(&registry);
        assert!(fields(&violations).contains(&"coupling"), "{violations:?}");

        // Expansion pack self-reference.
        let mut pw3 = Registry::embedded()
            .unwrap()
            .battery("tesla.powerwall_3")
            .unwrap()
            .clone();
        pw3.expansion.as_mut().unwrap().expansion_pack_model_id =
            Some("tesla.powerwall_3".to_owned());
        let pack = Registry::embedded()
            .unwrap()
            .battery("tesla.pw3_expansion_pack")
            .unwrap()
            .clone();
        let gateway = Registry::embedded()
            .unwrap()
            .controller("tesla.gateway_2")
            .unwrap()
            .clone();
        let registry = Registry::from_parts(vec![pw3, pack], vec![], vec![gateway]);
        let violations = check_cross_references(&registry);
        assert!(
            fields(&violations).contains(&"expansion.expansion_pack_model_id"),
            "{violations:?}"
        );
    }

    #[test]
    fn controller_and_preset_checks() {
        let mut controller = Registry::embedded()
            .unwrap()
            .controller("enphase.iq_system_controller_2")
            .unwrap()
            .clone();
        controller
            .frequency_shift_curtailment
            .as_mut()
            .unwrap()
            .start_hz = 63.0;
        let violations = check_controller("controllers/x.json", &controller);
        assert!(
            fields(&violations).contains(&"frequency_shift_curtailment"),
            "{violations:?}"
        );

        let mut preset = Registry::embedded()
            .unwrap()
            .pv_preset("residential.south_8kw")
            .unwrap()
            .clone();
        preset.tilt_deg = 120.0;
        let violations = check_pv_preset("pv_presets/x.json", &preset);
        assert!(fields(&violations).contains(&"tilt_deg"), "{violations:?}");
    }
}
