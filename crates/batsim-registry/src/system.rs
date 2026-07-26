//! HomeSystem composition (spec §3.1, §4.4): the declarative system
//! document, per-vendor validation rules, and the resolved [`SystemSpec`]
//! the engine consumes at simulation-init.
//!
//! Split of responsibilities: this module validates and computes; it never
//! constructs engine types. batsim-core turns a [`SystemSpec`] into live
//! `Home` state.

use serde::{Deserialize, Serialize};

use crate::error::{RegistryError, Violation};
use crate::load::Registry;
use crate::types::{BatteryModel, ControllerModel, Coupling, InverterModel, InverterTopology};

/// A battery line item in a HomeSystem document (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryRef {
    /// Battery `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
    /// Expansion packs per head unit (PW3; 0 for all other models).
    #[serde(default)]
    pub expansion_packs_per_unit: u32,
    /// Initial SOC fraction of usable energy.
    #[serde(default = "default_initial_soc")]
    pub initial_soc_frac: f64,
    /// User backup reserve floor.
    #[serde(default = "default_reserve_frac")]
    pub reserve_frac: f64,
}

const fn default_initial_soc() -> f64 {
    0.5
}

const fn default_reserve_frac() -> f64 {
    0.2
}

/// An inverter line item (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InverterRef {
    /// Inverter `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// A controller line item (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerRef {
    /// Controller `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// Array orientation: named compass point or explicit azimuth degrees
/// (spec §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Orientation {
    /// Named orientation.
    Named(NamedOrientation),
    /// Azimuth degrees, 0..359 (180 = south).
    Azimuth(u32),
}

/// Named array orientations (spec §4.4 enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedOrientation {
    /// North.
    N,
    /// North-east.
    NE,
    /// East.
    E,
    /// South-east.
    SE,
    /// South.
    S,
    /// South-south-west.
    SSW,
    /// South-west.
    SW,
    /// West-south-west.
    WSW,
    /// West.
    W,
    /// North-west.
    NW,
    /// Flat (horizontal).
    FLAT,
}

impl Orientation {
    /// Resolve to azimuth degrees (180 = south, 90 = east, 270 = west).
    /// `FLAT` resolves to 180 (tilt carries the flatness).
    #[must_use]
    pub const fn azimuth_deg(self) -> u32 {
        match self {
            Self::Azimuth(a) => a,
            Self::Named(n) => match n {
                NamedOrientation::N => 0,
                NamedOrientation::NE => 45,
                NamedOrientation::E => 90,
                NamedOrientation::SE => 135,
                NamedOrientation::S | NamedOrientation::FLAT => 180,
                NamedOrientation::SSW => 202,
                NamedOrientation::SW => 225,
                NamedOrientation::WSW => 247,
                NamedOrientation::W => 270,
                NamedOrientation::NW => 315,
            },
        }
    }
}

/// PV array configuration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
    /// Array DC nameplate.
    pub kw_dc: f64,
    /// Array orientation.
    pub orientation: Orientation,
    /// Tilt in degrees.
    #[serde(default = "default_tilt")]
    pub tilt_deg: f64,
    /// DC/AC ratio.
    #[serde(default = "default_dc_ac_ratio")]
    pub dc_ac_ratio: f64,
    /// PV inverter `model_id`; `None` iff PV lands on a hybrid inverter's
    /// MPPTs.
    #[serde(default)]
    pub pv_inverter_model_id: Option<String>,
}

const fn default_tilt() -> f64 {
    25.0
}

const fn default_dc_ac_ratio() -> f64 {
    1.2
}

/// Main service panel (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainPanel {
    /// Service rating in amps (240 V split-phase assumed).
    #[serde(default = "default_service_rating")]
    pub service_rating_a: f64,
    /// Utility-imposed export cap; `None` = none.
    #[serde(default)]
    pub interconnection_limit_kw: Option<f64>,
}

const fn default_service_rating() -> f64 {
    200.0
}

/// Backup sub-panel declaration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPanel {
    /// Peak critical-load power.
    #[serde(default = "default_critical_peak")]
    pub critical_loads_peak_kw: f64,
    /// Whole-home backup topology (no critical-loads split).
    #[serde(default)]
    pub whole_home: bool,
}

const fn default_critical_peak() -> f64 {
    5.0
}

/// Generator input declaration (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorConfig {
    /// Rated power.
    pub rated_kw: f64,
    /// Auto-transfer-switch flag.
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

const fn default_true() -> bool {
    true
}

/// EV charger declaration: load-only (V1G), V2X out of scope (Part A §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvChargerConfig {
    /// Rated charge power (default 11.5 kW L2).
    #[serde(default = "default_ev_kw")]
    pub rated_kw: f64,
    /// V1G controllable load.
    #[serde(default = "default_true")]
    pub controllable: bool,
    /// Whether the charger sits on the backup sub-panel.
    #[serde(default)]
    pub on_backup_panel: bool,
}

const fn default_ev_kw() -> f64 {
    11.5
}

/// Grid meter point: the ERCOT ESIID binding (spec §4.4; consumed by
/// Part D in M3+).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridMeter {
    /// ERCOT ESI ID.
    pub esiid: String,
    /// TDSP name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdsp: Option<String>,
}

/// The HomeSystem composition document (spec §4.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSystem {
    /// Schema version; must equal [`crate::types::SCHEMA_VERSION`].
    pub schema_version: String,
    /// System UUID (server-assigned in M2+; any string accepted here).
    pub system_id: String,
    /// Human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Battery line items.
    pub batteries: Vec<BatteryRef>,
    /// Inverter line items.
    pub inverters: Vec<InverterRef>,
    /// Controller line items.
    #[serde(default)]
    pub controllers: Vec<ControllerRef>,
    /// PV array, if any.
    #[serde(default)]
    pub pv: Option<PvConfig>,
    /// Main service panel.
    pub main_panel: MainPanel,
    /// Whether the system asserts backup capability.
    pub backup_capable: bool,
    /// Backup sub-panel, if any.
    #[serde(default)]
    pub backup_panel: Option<BackupPanel>,
    /// Generator input, if any.
    #[serde(default)]
    pub generator: Option<GeneratorConfig>,
    /// EV chargers.
    #[serde(default)]
    pub ev_chargers: Vec<EvChargerConfig>,
    /// Grid meter point.
    pub grid_meter: GridMeter,
}

/// A resolved, validated system: the composition-time output consumed by
/// batsim-core at simulation-init (spec §3.1).
///
/// Beyond the four `total_*` aggregates required by spec §3.1, this struct
/// carries resolved composition facts that batsim-core would otherwise have
/// to re-derive: the grid-forming controller identity and the storage
/// coupling classification.
#[derive(Debug, Clone)]
pub struct SystemSpec {
    /// The (validated) source document.
    pub system: HomeSystem,
    /// Total usable battery energy across all units and packs, kWh:
    /// `Σ quantity × (head usable + packs-per-unit × pack usable)`.
    pub total_usable_energy_kwh: f64,
    /// Total continuous discharge power at device boundaries, kW.
    /// Expansion packs add energy only (spec §3.1), never power.
    pub total_discharge_power_kw: f64,
    /// Total continuous charge power, kW.
    pub total_charge_power_kw: f64,
    /// Computed backup-path continuous power:
    /// `min(total battery continuous, backup-path rating)` (spec §3.1).
    /// `None` when not backup-capable.
    ///
    /// Backup-path rating resolution order (first present wins):
    /// 1. Σ `max_backup_power_kw` over present controllers that declare it
    ///    (gateway/transfer-device throughput cap);
    /// 2. else Σ (`max_ac_output_kw_backup` else `rated_ac_output_kw`) over
    ///    present explicit inverters (hybrid backup rating);
    /// 3. else the batteries' own continuous discharge sum (covers
    ///    integrated-inverter batteries such as PW3 with no explicit
    ///    InverterModel entry), which makes the `min` an identity.
    pub backup_path_power_kw: Option<f64>,
    /// `model_id` of the single grid-forming controller resolved during
    /// validation (spec §3.1 backup rule). `None` when not backup-capable.
    pub resolved_controller_model_id: Option<String>,
    /// Whether any present battery is DC-coupled hybrid (spec §3.3). Part B
    /// uses this to select the single-inversion PV-storage loss path.
    pub has_dc_coupled_storage: bool,
}

impl HomeSystem {
    /// Validate this composition against the registry per spec §3.1 rules
    /// and §4.6 cross-reference checks; on success return the resolved
    /// [`SystemSpec`]. All violations are enumerated, never fail-fast.
    ///
    /// Enforced rules (spec §3.1, §4.4, §4.6):
    /// - `schema_version` equals [`crate::types::SCHEMA_VERSION`].
    /// - Every referenced `model_id` resolves to a registry entry of
    ///   matching kind (batteries, inverters, controllers, PV inverter).
    /// - Quantities are >= 1; SOC/reserve fractions lie in `[0, 1]`;
    ///   each battery's initial SOC lies inside its model's SOC window.
    /// - `backup_capable` requires exactly one present controller entry
    ///   with `provides_grid_forming`, and every battery's
    ///   `requires_controller_id` (when declared) present in `controllers[]`.
    /// - DC-coupled hybrid batteries without an integrated inverter
    ///   intersect some present inverter's `compatible_battery_ids`.
    /// - Per battery model, unit count <= Σ `max_batteries × quantity`
    ///   over present compatible inverters when every such inverter
    ///   declares `max_batteries` (SolarEdge: 3 × Home Hub count).
    /// - Expansion packs only on models declaring
    ///   `expansion_pack_model_id`; packs <= `max_units_per_inverter - 1`
    ///   (PW3: 3); `packs_add_power = false` (energy only).
    /// - Microinverter-based batteries (Enphase): continuous ratings equal
    ///   `microinverter_count × power_per_microinverter_kw` (0.64 kW per
    ///   IQ8D) within 1e-6.
    /// - A generator requires a present controller with
    ///   `supports_generator_input`.
    /// - A null `pv_inverter_model_id` requires a present hybrid landing
    ///   pad (hybrid inverter MPPTs or an integrated-inverter DC-coupled
    ///   battery such as PW3).
    ///
    /// # Errors
    /// [`RegistryError::Validation`] enumerating every violation.
    pub fn validate(&self, registry: &Registry) -> Result<SystemSpec, RegistryError> {
        let mut violations: Vec<Violation> = Vec::new();

        // -- Schema version -------------------------------------------------
        let expected = crate::types::SCHEMA_VERSION;
        let found = &self.schema_version;
        if found != expected {
            violations.push(violation(
                "schema_version",
                format!("unsupported schema_version `{found}`; expected `{expected}`"),
            ));
        }

        // -- Reference resolution + per-item numerics ------------------------
        let battery_models: Vec<Option<&BatteryModel>> = self
            .batteries
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if b.quantity == 0 {
                    violations.push(violation(
                        &format!("batteries[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                if !(0.0..=1.0).contains(&b.initial_soc_frac) {
                    violations.push(violation(
                        &format!("batteries[{i}].initial_soc_frac"),
                        "initial SOC fraction must lie in [0, 1]",
                    ));
                }
                if !(0.0..=1.0).contains(&b.reserve_frac) {
                    violations.push(violation(
                        &format!("batteries[{i}].reserve_frac"),
                        "reserve fraction must lie in [0, 1]",
                    ));
                }
                let model = registry.battery(&b.model_id);
                match model {
                    Some(m) => {
                        let soc = b.initial_soc_frac;
                        let (lo, hi) = (m.soc_window.min_soc_frac, m.soc_window.max_soc_frac);
                        if soc < lo || soc > hi {
                            let id = &m.model_id;
                            violations.push(violation(
                                &format!("batteries[{i}].initial_soc_frac"),
                                format!("initial SOC {soc} outside usable window [{lo}, {hi}] of `{id}`"),
                            ));
                        }
                    }
                    None => {
                        let id = &b.model_id;
                        violations.push(violation(
                            &format!("batteries[{i}].model_id"),
                            format!("unknown battery model `{id}`"),
                        ));
                    }
                }
                model
            })
            .collect();

        let inverter_models: Vec<Option<&InverterModel>> = self
            .inverters
            .iter()
            .enumerate()
            .map(|(i, inv)| {
                if inv.quantity == 0 {
                    violations.push(violation(
                        &format!("inverters[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                let model = registry.inverter(&inv.model_id);
                if model.is_none() {
                    let id = &inv.model_id;
                    violations.push(violation(
                        &format!("inverters[{i}].model_id"),
                        format!("unknown inverter model `{id}`"),
                    ));
                }
                model
            })
            .collect();

        let controller_models: Vec<Option<&ControllerModel>> = self
            .controllers
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if c.quantity == 0 {
                    violations.push(violation(
                        &format!("controllers[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                let model = registry.controller(&c.model_id);
                if model.is_none() {
                    let id = &c.model_id;
                    violations.push(violation(
                        &format!("controllers[{i}].model_id"),
                        format!("unknown controller model `{id}`"),
                    ));
                }
                model
            })
            .collect();

        // Present = referenced with quantity >= 1 and resolved in the registry.
        let present_inverters: Vec<(&InverterModel, u32)> = self
            .inverters
            .iter()
            .zip(&inverter_models)
            .filter(|(r, _)| r.quantity > 0)
            .filter_map(|(r, m)| m.map(|m| (m, r.quantity)))
            .collect();
        let present_controllers: Vec<(&ControllerModel, u32)> = self
            .controllers
            .iter()
            .zip(&controller_models)
            .filter(|(r, _)| r.quantity > 0)
            .filter_map(|(r, m)| m.map(|m| (m, r.quantity)))
            .collect();

        // -- Backup composition (spec §3.1 rule 1, §4.6) ----------------------
        if self.backup_capable {
            let grid_forming = present_controllers
                .iter()
                .filter(|(c, _)| c.provides_grid_forming)
                .count();
            if grid_forming != 1 {
                violations.push(violation(
                    "controllers",
                    format!(
                        "backup_capable requires exactly one present controller entry with \
                         provides_grid_forming = true; found {grid_forming}"
                    ),
                ));
            }
            for (i, b) in self.batteries.iter().enumerate() {
                if let Some(m) = battery_models[i] {
                    if let Some(req) = &m.requires_controller_id {
                        let present = self
                            .controllers
                            .iter()
                            .any(|c| c.quantity > 0 && &c.model_id == req);
                        if !present {
                            let id = &m.model_id;
                            violations.push(violation(
                                &format!("batteries[{i}].model_id"),
                                format!(
                                    "backup_capable: `{id}` requires controller `{req}`, which is \
                                     not present in controllers[]"
                                ),
                            ));
                        }
                    }
                }
            }
        }

        // -- DC-coupled batteries need a compatible hybrid inverter ----------
        // Batteries with an integrated inverter (PW3) ARE their own hybrid
        // inverter, so the explicit-inverter intersection is skipped for them.
        for (i, b) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if b.quantity == 0 || m.coupling != Coupling::DCCoupledHybrid {
                continue;
            }
            if m.integrated_inverter == Some(true) {
                continue;
            }
            let compatible = present_inverters
                .iter()
                .any(|(inv, _)| inv.compatible_battery_ids.iter().any(|id| id == &m.model_id));
            if !compatible {
                let id = &m.model_id;
                violations.push(violation(
                    &format!("batteries[{i}].model_id"),
                    format!(
                        "DC-coupled battery `{id}` requires a present hybrid inverter listing it in \
                         compatible_battery_ids"
                    ),
                ));
            }
        }

        // -- Per-model compatible-inverter capacity (SolarEdge: <= 3 per Hub) -
        // Generalized: for each battery model with at least one present
        // compatible inverter, the unit count is bounded by the sum of
        // max_batteries x quantity over those inverters — but only when every
        // compatible present inverter declares max_batteries (an undeclared
        // limit means unbounded capacity, so no bound is enforced).
        let mut checked_models: Vec<&str> = Vec::new();
        for (i, _) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if checked_models.contains(&m.model_id.as_str()) {
                continue;
            }
            checked_models.push(&m.model_id);
            let compatible: Vec<(&InverterModel, u32)> = present_inverters
                .iter()
                .filter(|(inv, _)| inv.compatible_battery_ids.iter().any(|id| id == &m.model_id))
                .map(|(inv, q)| (*inv, *q))
                .collect();
            if compatible.is_empty()
                || !compatible.iter().all(|(inv, _)| inv.max_batteries.is_some())
            {
                continue;
            }
            let capacity: u32 = compatible
                .iter()
                .map(|(inv, q)| inv.max_batteries.map_or(0, |mb| mb.saturating_mul(*q)))
                .sum();
            let units: u32 = self
                .batteries
                .iter()
                .filter(|x| x.model_id == m.model_id)
                .map(|x| x.quantity)
                .sum();
            if units > capacity {
                let id = &m.model_id;
                violations.push(violation(
                    &format!("batteries[{i}].model_id"),
                    format!(
                        "{units} unit(s) of `{id}` exceed compatible-inverter capacity {capacity} \
                         (sum of max_batteries over present inverters)"
                    ),
                ));
            }
        }

        // -- Expansion packs (PW3: <= 3 per head unit, energy only) -----------
        for (i, b) in self.batteries.iter().enumerate() {
            if b.expansion_packs_per_unit == 0 {
                continue;
            }
            let Some(m) = battery_models[i] else { continue };
            let field = format!("batteries[{i}].expansion_packs_per_unit");
            let id = &m.model_id;
            match &m.expansion {
                Some(exp) if exp.expansion_pack_model_id.is_some() => {
                    if let Some(max_units) = exp.max_units_per_inverter {
                        let max_packs = max_units.saturating_sub(1);
                        if b.expansion_packs_per_unit > max_packs {
                            let packs = b.expansion_packs_per_unit;
                            violations.push(violation(
                                &field,
                                format!(
                                    "{packs} expansion packs per unit exceed {max_packs} \
                                     (max_units_per_inverter {max_units} minus the head unit)"
                                ),
                            ));
                        }
                    }
                    if exp.packs_add_power == Some(true) {
                        violations.push(violation(
                            &field,
                            format!(
                                "`{id}` declares packs_add_power = true; expansion packs must add \
                                 energy only"
                            ),
                        ));
                    }
                    if let Some(pack_id) = &exp.expansion_pack_model_id {
                        if registry.battery(pack_id).is_none() {
                            violations.push(violation(
                                &field,
                                format!("expansion pack model `{pack_id}` does not resolve to a battery entry"),
                            ));
                        }
                    }
                }
                _ => violations.push(violation(
                    &field,
                    format!("`{id}` does not declare an expansion_pack_model_id; expansion packs are not supported"),
                )),
            }
        }

        // -- Microinverter power cross-check (Enphase: 0.64 kW x IQ8D count) --
        let mut checked_micro: Vec<&str> = Vec::new();
        for (i, _) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if checked_micro.contains(&m.model_id.as_str()) {
                continue;
            }
            checked_micro.push(&m.model_id);
            let (Some(count), Some(per_micro)) =
                (m.microinverter_count, m.power_per_microinverter_kw.as_ref())
            else {
                continue;
            };
            let recomputed = f64::from(count) * per_micro.value;
            let id = &m.model_id;
            let per = per_micro.value;
            for (label, rating) in [
                ("discharge", &m.continuous_discharge_power_kw),
                ("charge", &m.continuous_charge_power_kw),
            ] {
                let rated = rating.value;
                if (recomputed - rated).abs() > 1e-6 {
                    violations.push(violation(
                        &format!("batteries[{i}].model_id"),
                        format!(
                            "microinverter cross-check failed for `{id}`: {count} microinverter(s) x \
                             {per} kW = {recomputed} kW, but continuous {label} rating is {rated} kW"
                        ),
                    ));
                }
            }
        }

        // -- Generator interlock (spec §3.1) ----------------------------------
        if let Some(g) = &self.generator {
            if g.rated_kw <= 0.0 {
                violations.push(violation("generator.rated_kw", "rated power must be > 0"));
            }
            let supported = present_controllers
                .iter()
                .any(|(c, _)| c.supports_generator_input);
            if !supported {
                violations.push(violation(
                    "generator",
                    "a generator requires a present controller with supports_generator_input = true",
                ));
            }
        }

        // -- PV array ----------------------------------------------------------
        if let Some(pv) = &self.pv {
            if pv.kw_dc <= 0.0 {
                violations.push(violation("pv.kw_dc", "array nameplate must be > 0"));
            }
            if !(0.0..=90.0).contains(&pv.tilt_deg) {
                violations.push(violation("pv.tilt_deg", "tilt must lie in [0, 90] degrees"));
            }
            if pv.dc_ac_ratio <= 0.0 {
                violations.push(violation("pv.dc_ac_ratio", "DC/AC ratio must be > 0"));
            }
            if let Orientation::Azimuth(a) = pv.orientation {
                if a > 359 {
                    violations.push(violation(
                        "pv.orientation",
                        "azimuth must lie in 0..=359 degrees",
                    ));
                }
            }
            match &pv.pv_inverter_model_id {
                Some(pid) => {
                    if registry.inverter(pid).is_none() {
                        violations.push(violation(
                            "pv.pv_inverter_model_id",
                            format!("unknown inverter model `{pid}`"),
                        ));
                    }
                }
                None => {
                    // PV with no dedicated inverter must land on hybrid MPPTs:
                    // either an explicit hybrid inverter, or a DC-coupled
                    // battery with an integrated inverter (PW3 MPPTs).
                    let landing_pad = present_inverters
                        .iter()
                        .any(|(inv, _)| inv.topology == InverterTopology::HybridDCCoupled)
                        || self.batteries.iter().zip(&battery_models).any(|(r, m)| {
                            r.quantity > 0
                                && m.is_some_and(|m| {
                                    m.coupling == Coupling::DCCoupledHybrid
                                        && m.integrated_inverter == Some(true)
                                })
                        });
                    if !landing_pad {
                        violations.push(violation(
                            "pv.pv_inverter_model_id",
                            "null PV inverter requires a present hybrid landing pad (hybrid \
                             inverter MPPTs or an integrated-inverter DC-coupled battery)",
                        ));
                    }
                }
            }
        }

        // -- Remaining numerics ------------------------------------------------
        if self.main_panel.service_rating_a <= 0.0 {
            violations.push(violation(
                "main_panel.service_rating_a",
                "service rating must be > 0",
            ));
        }
        if let Some(limit) = self.main_panel.interconnection_limit_kw {
            if limit < 0.0 {
                violations.push(violation(
                    "main_panel.interconnection_limit_kw",
                    "interconnection limit must be >= 0",
                ));
            }
        }
        if let Some(bp) = &self.backup_panel {
            if bp.critical_loads_peak_kw < 0.0 {
                violations.push(violation(
                    "backup_panel.critical_loads_peak_kw",
                    "critical-loads peak must be >= 0",
                ));
            }
        }
        for (i, ev) in self.ev_chargers.iter().enumerate() {
            if ev.rated_kw <= 0.0 {
                violations.push(violation(
                    &format!("ev_chargers[{i}].rated_kw"),
                    "rated power must be > 0",
                ));
            }
        }

        if !violations.is_empty() {
            return Err(RegistryError::Validation { violations });
        }

        // -- Resolved SystemSpec (spec §3.1 aggregates) ------------------------
        let mut total_usable_energy_kwh = 0.0;
        let mut total_discharge_power_kw = 0.0;
        let mut total_charge_power_kw = 0.0;
        for (b, m) in self.batteries.iter().zip(&battery_models) {
            let Some(m) = m else { continue };
            let q = f64::from(b.quantity);
            let head_usable = m.usable_energy_kwh.value;
            // Packs add their usable energy per head unit, never power.
            let pack_usable = if b.expansion_packs_per_unit > 0 {
                m.expansion
                    .as_ref()
                    .and_then(|e| e.expansion_pack_model_id.as_deref())
                    .and_then(|pid| registry.battery(pid))
                    .map_or(0.0, |p| p.usable_energy_kwh.value)
            } else {
                0.0
            };
            total_usable_energy_kwh +=
                q * (head_usable + f64::from(b.expansion_packs_per_unit) * pack_usable);
            total_discharge_power_kw += q * m.continuous_discharge_power_kw.value;
            total_charge_power_kw += q * m.continuous_charge_power_kw.value;
        }

        // Backup-path rating resolution order (see SystemSpec field docs):
        // controller throughput cap > explicit inverter backup rating >
        // the batteries' own integrated rating.
        let backup_path_power_kw = if self.backup_capable {
            let mut controller_sum = 0.0;
            let mut any_controller_rating = false;
            for (c, q) in &present_controllers {
                if let Some(max) = &c.max_backup_power_kw {
                    controller_sum += max.value * f64::from(*q);
                    any_controller_rating = true;
                }
            }
            let inverter_sum: f64 = present_inverters
                .iter()
                .map(|(inv, q)| {
                    inv.max_ac_output_kw_backup
                        .as_ref()
                        .unwrap_or(&inv.rated_ac_output_kw)
                        .value
                        * f64::from(*q)
                })
                .sum();
            let path_rating = if any_controller_rating {
                controller_sum
            } else if inverter_sum > 0.0 {
                inverter_sum
            } else {
                total_discharge_power_kw
            };
            Some(total_discharge_power_kw.min(path_rating))
        } else {
            None
        };

        let resolved_controller_model_id = if self.backup_capable {
            present_controllers
                .iter()
                .find(|(c, _)| c.provides_grid_forming)
                .map(|(c, _)| c.model_id.clone())
        } else {
            None
        };

        let has_dc_coupled_storage = self.batteries.iter().zip(&battery_models).any(|(r, m)| {
            r.quantity > 0 && m.is_some_and(|m| m.coupling == Coupling::DCCoupledHybrid)
        });

        Ok(SystemSpec {
            system: self.clone(),
            total_usable_energy_kwh,
            total_discharge_power_kw,
            total_charge_power_kw,
            backup_path_power_kw,
            resolved_controller_model_id,
            has_dc_coupled_storage,
        })
    }

    /// Parse a HomeSystem JSON document.
    ///
    /// # Errors
    /// [`RegistryError::Parse`] on malformed JSON or schema-shape mismatch.
    pub fn from_json(json: &str) -> Result<Self, RegistryError> {
        serde_json::from_str(json).map_err(|source| RegistryError::Parse {
            path: "<home_system>".to_owned(),
            source,
        })
    }
}

/// Violation constructor helper for composition checks.
#[must_use]
pub fn violation(field: &str, message: impl Into<String>) -> Violation {
    Violation {
        path: "<home_system>".to_owned(),
        field: field.to_owned(),
        message: message.into(),
    }
}
